//! Solvency proofs (E11): "I own one of this market's positions, and it meets these claims",
//! without saying which position.
//!
//! An auditor reads a market from the chain and sends a [`Request`]: the market's positions (the
//! cover set), the valuation it uses (price, ratios, LLTV), the [`Claims`] to prove and a nonce. The
//! owner proves, in the `disclosure` guest, that one listed position is theirs (its address comes
//! from their spending key, design §4) and passes [`check`]. The proof's journal is a [`Journal`]:
//! the request's digest and the prover's [`disclosure_id`], so the auditor learns the claims hold
//! for whoever registered that id, and nothing else. Positions are public in Option 1; what the
//! proof hides is which one is the prover's.

use borsh::{BorshDeserialize, BorshSerialize};
use bosphorus_core::{
    Health, Position, U256, WAD,
    position::{debt, health_factor},
    shares::to_assets_down,
};
use hmac::{Hmac, Mac as _};
use sha2::{Digest as _, Sha256};

pub type Hash = [u8; 32];

/// What the owner must show about their position. `None` means "not asked".
#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Claims {
    /// Health factor (WAD) at least this. A position without debt passes.
    pub min_health: Option<u128>,
    /// Collateral (collateral-asset base units) at least this.
    pub min_collateral: Option<u128>,
    /// Debt (loan-asset base units, rounded up) at most this.
    pub max_debt: Option<u128>,
    /// Supplied assets (loan-asset base units, rounded down) at least this.
    pub min_supplied: Option<u128>,
}

impl Claims {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The valuation every position is checked at, read by the auditor at `at_ms`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Valuation {
    /// Feed price (1e36 scale).
    pub price: U256,
    /// Borrow ratio (WAD, rounded up) and supply ratio (WAD, rounded down), accrued to `at_ms`.
    pub rb: u128,
    pub rs: u128,
    pub lltv: u128,
    /// ms.
    pub at_ms: u64,
}

/// What an auditor asks for, but the cover set.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Header {
    /// Who is asking and why, shown to the owner (≤ [`MAX_LABEL_LEN`] bytes).
    pub label: String,
    pub nonce: Hash,
    /// The `lending` program instance.
    pub lending: Hash,
    pub market_id: Hash,
    pub valuation: Valuation,
    pub claims: Claims,
}

impl Header {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.label.len() > MAX_LABEL_LEN {
            return Err("label too long");
        }
        if self.claims.is_empty() {
            return Err("no claims asked");
        }
        Ok(())
    }
}

/// What an auditor asks for. Built from chain state by the auditor; the owner's client checks it
/// against the chain before proving (a request listing only the suspected position, or claims only
/// one position meets, would identify the owner).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Request {
    pub header: Header,
    /// The cover set: every position of the market, sorted by id, no duplicates.
    pub positions: Vec<(Hash, Position)>,
}

pub const MAX_LABEL_LEN: usize = 128;
pub const REQUEST_DOMAIN: &[u8] = b"/bosphorus/v1/disclosure/request/";

impl Request {
    /// What the proof commits to: [`digest`] of the header and the cover set's Merkle root.
    pub fn digest(&self) -> Hash {
        digest(&self.header, &root(&self.leaves()), self.positions.len() as u32)
    }

    pub fn leaves(&self) -> Vec<Hash> {
        self.positions.iter().map(|(id, p)| leaf(id, p)).collect()
    }

    /// Shape checks both sides run: the header's, and positions sorted and in the market.
    pub fn validate(&self) -> Result<(), &'static str> {
        self.header.validate()?;
        if !self.positions.windows(2).all(|w| w[0].0 < w[1].0) {
            return Err("positions not sorted or duplicated");
        }
        if self.positions.iter().any(|(_, p)| p.market_id != self.header.market_id) {
            return Err("a position from another market");
        }
        Ok(())
    }

    /// The positions of the cover set that meet the claims: the prover hides among these only.
    pub fn qualifying_ids(&self) -> Vec<Hash> {
        self.positions.iter().filter(|(_, p)| check(&self.header.valuation, &self.header.claims, p).is_ok()).map(|(id, _)| *id).collect()
    }

    pub fn qualifying(&self) -> usize {
        self.qualifying_ids().len()
    }
}

/// `SHA256(REQUEST_DOMAIN ‖ borsh(header) ‖ root ‖ n)`.
pub fn digest(header: &Header, root: &Hash, n: u32) -> Hash {
    let mut h = Sha256::new();
    h.update(REQUEST_DOMAIN);
    h.update(borsh::to_vec(header).expect("borsh into a Vec can't fail"));
    h.update(root);
    h.update(n.to_le_bytes());
    h.finalize().into()
}

/// `SHA256(0 ‖ id ‖ borsh(position))`.
pub fn leaf(id: &Hash, p: &Position) -> Hash {
    let mut h = Sha256::new();
    h.update([0u8]);
    h.update(id);
    h.update(borsh::to_vec(p).expect("borsh into a Vec can't fail"));
    h.finalize().into()
}

fn node(l: &Hash, r: &Hash) -> Hash {
    let mut h = Sha256::new();
    h.update([1u8]);
    h.update(l);
    h.update(r);
    h.finalize().into()
}

/// The levels of the tree over `leaves`, padded with zero hashes to a power of two.
fn levels(leaves: &[Hash]) -> Vec<Vec<Hash>> {
    let mut level = leaves.to_vec();
    level.resize(leaves.len().next_power_of_two().max(1), [0; 32]);
    let mut levels = vec![level];
    while levels.last().unwrap().len() > 1 {
        let next = levels.last().unwrap().chunks(2).map(|c| node(&c[0], &c[1])).collect();
        levels.push(next);
    }
    levels
}

/// The Merkle root of `leaves` (zero hashes pad to a power of two; `n` goes in the digest).
pub fn root(leaves: &[Hash]) -> Hash {
    levels(leaves).last().unwrap()[0]
}

/// The sibling hashes from leaf `i` up to the root.
pub fn path(leaves: &[Hash], i: usize) -> Vec<Hash> {
    let levels = levels(leaves);
    levels[..levels.len() - 1].iter().enumerate().map(|(depth, level)| level[(i >> depth) ^ 1]).collect()
}

/// The root that `leaf` at `i` with `path` leads to.
pub fn root_from_path(leaf: Hash, i: u32, path: &[Hash]) -> Hash {
    path.iter().enumerate().fold(leaf, |acc, (depth, sibling)| if (i >> depth) & 1 == 0 { node(&acc, sibling) } else { node(sibling, &acc) })
}

/// The guest's public output.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Journal {
    pub request: Hash,
    pub disclosure_id: Hash,
}

/// What the owner feeds the guest, never published: the request's header and size, its own
/// position with its Merkle path, and its keys.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Witness {
    pub header: Header,
    /// Positions in the cover set.
    pub n: u32,
    /// The position's place in the (sorted) cover set, its id, contents and path.
    pub at: u32,
    pub position_id: Hash,
    pub position: Position,
    pub path: Vec<Hash>,
    /// The owner's private-account spending key (the HMAC key of the position salt).
    pub secret: Hash,
    /// The owner's private account id.
    pub owner: Hash,
    /// The position's index for that owner and market (design §4).
    pub index: u32,
}

impl Witness {
    /// The witness for the cover set's position `position_id`.
    pub fn new(request: &Request, position_id: Hash, secret: Hash, owner: Hash, index: u32) -> Option<Self> {
        let at = request.positions.binary_search_by_key(&position_id, |(id, _)| *id).ok()?;
        Some(Self {
            header: request.header.clone(),
            n: request.positions.len() as u32,
            at: at as u32,
            position_id,
            position: request.positions[at].1.clone(),
            path: path(&request.leaves(), at),
            secret,
            owner,
            index,
        })
    }
}

/// `p` meets `claims` at `v`.
pub fn check(v: &Valuation, claims: &Claims, p: &Position) -> Result<(), &'static str> {
    let maths = |_| "arithmetic overflow";
    if let Some(min) = claims.min_health {
        let health = Health { price: v.price, rb: v.rb, lltv: v.lltv };
        if let Some(hf) = health_factor(p.collateral, p.borrow_shares, &health).map_err(maths)?
            && hf < U256::from(min)
        {
            return Err("health below the claim");
        }
    }
    if let Some(min) = claims.min_collateral
        && p.collateral < min
    {
        return Err("collateral below the claim");
    }
    if let Some(max) = claims.max_debt
        && debt(p.borrow_shares, v.rb).map_err(maths)? > U256::from(max)
    {
        return Err("debt above the claim");
    }
    if let Some(min) = claims.min_supplied
        && to_assets_down(p.supply_shares, v.rs).map_err(maths)? < min
    {
        return Err("supply below the claim");
    }
    Ok(())
}

/// `HMAC-SHA256(secret, "/bosphorus/v1/position-salt/" ‖ market_id ‖ owner ‖ index)`. The same as
/// `bosphorus_client::lending::position_salt` (tested there).
pub fn position_salt(secret: &Hash, market_id: &Hash, owner: &Hash, index: u32) -> Hash {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC takes any key length");
    mac.update(b"/bosphorus/v1/position-salt/");
    mac.update(market_id);
    mac.update(owner);
    mac.update(&index.to_le_bytes());
    mac.finalize().into_bytes().into()
}

/// `SHA256("/bosphorus/v1/disclosure/id/" ‖ secret)`: the owner's stable name for auditors. Not
/// derivable from anything on chain, and the same for every proof from one private account.
pub fn disclosure_id(secret: &Hash) -> Hash {
    let mut h = Sha256::new();
    h.update(b"/bosphorus/v1/disclosure/id/");
    h.update(secret);
    h.finalize().into()
}

/// A health factor as a WAD, from a decimal like `"1.5"`.
pub fn parse_wad(s: &str) -> Option<u128> {
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    if frac.len() > 18 || (int.is_empty() && frac.is_empty()) {
        return None;
    }
    let int: u128 = if int.is_empty() { 0 } else { int.parse().ok()? };
    let frac: u128 = if frac.is_empty() { 0 } else { format!("{frac:0<18}").parse().ok()? };
    int.checked_mul(WAD)?.checked_add(frac)
}

/// A WAD as a decimal, trailing zeros dropped (`1.5`).
pub fn fmt_wad(x: u128) -> String {
    let frac = format!("{:018}", x % WAD);
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() { format!("{}", x / WAD) } else { format!("{}.{frac}", x / WAD) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wad(x: u128) -> u128 {
        x * WAD
    }

    /// Price 1 (1e36), ratios 1, LLTV 80%: 100 collateral backs 80 debt.
    fn valuation() -> Valuation {
        Valuation { price: bosphorus_core::PRICE_SCALE, rb: WAD, rs: WAD, lltv: WAD / 10 * 8, at_ms: 0 }
    }

    fn position(collateral: u128, borrow_shares: u128, supply_shares: u128) -> Position {
        Position { market_id: [7; 32], supply_shares, borrow_shares, collateral }
    }

    #[test]
    fn health_claim() {
        let v = valuation();
        let claims = Claims { min_health: Some(wad(3) / 2), ..Default::default() };
        // max borrow 80: debt 50 → 1.6, debt 60 → 1.33.
        assert!(check(&v, &claims, &position(100, 50, 0)).is_ok());
        assert_eq!(check(&v, &claims, &position(100, 60, 0)), Err("health below the claim"));
        assert!(check(&v, &claims, &position(0, 0, 0)).is_ok(), "no debt passes a health claim");
    }

    #[test]
    fn amount_claims() {
        let v = valuation();
        let p = position(100, 50, 30);
        let ok = |claims: Claims| check(&v, &claims, &p).is_ok();
        assert!(ok(Claims { min_collateral: Some(100), ..Default::default() }));
        assert!(!ok(Claims { min_collateral: Some(101), ..Default::default() }));
        assert!(ok(Claims { max_debt: Some(50), ..Default::default() }));
        assert!(!ok(Claims { max_debt: Some(49), ..Default::default() }));
        assert!(ok(Claims { min_supplied: Some(30), ..Default::default() }));
        assert!(!ok(Claims { min_supplied: Some(31), ..Default::default() }));
    }

    #[test]
    fn request_shape() {
        let mut r = Request {
            header: Header {
                label: "audit".into(),
                nonce: [1; 32],
                lending: [2; 32],
                market_id: [7; 32],
                valuation: valuation(),
                claims: Claims { min_health: Some(WAD), ..Default::default() },
            },
            positions: vec![([1; 32], position(100, 50, 0)), ([2; 32], position(100, 79, 0))],
        };
        assert_eq!(r.validate(), Ok(()));
        assert_eq!(r.qualifying(), 2);
        r.header.claims.min_health = Some(wad(3) / 2);
        assert_eq!(r.qualifying(), 1);
        let digest = r.digest();
        r.header.nonce = [9; 32];
        assert_ne!(r.digest(), digest, "the nonce is bound");
        r.positions.swap(0, 1);
        assert_eq!(r.validate(), Err("positions not sorted or duplicated"));
        r.positions.swap(0, 1);
        r.positions[0].1.market_id = [8; 32];
        assert_eq!(r.validate(), Err("a position from another market"));
    }

    #[test]
    fn merkle_paths() {
        for n in [1usize, 2, 3, 5, 8, 13] {
            let leaves: Vec<Hash> = (0..n).map(|i| [i as u8 + 1; 32]).collect();
            let r = root(&leaves);
            for i in 0..n {
                assert_eq!(root_from_path(leaves[i], i as u32, &path(&leaves, i)), r, "n {n} leaf {i}");
                assert_ne!(root_from_path([0xaa; 32], i as u32, &path(&leaves, i)), r);
            }
        }
    }

    #[test]
    fn wad_parsing() {
        assert_eq!(parse_wad("1.5"), Some(wad(3) / 2));
        assert_eq!(parse_wad("2"), Some(wad(2)));
        assert_eq!(parse_wad(".25"), Some(WAD / 4));
        assert_eq!(parse_wad("1.0000000000000000001"), None);
        assert_eq!(parse_wad("x"), None);
        assert_eq!(fmt_wad(wad(3) / 2), "1.5");
        assert_eq!(fmt_wad(wad(2)), "2");
    }
}
