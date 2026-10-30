//! Solvency proofs (E11, `bosphorus_disclosure`): an auditor asks, an owner proves one of a
//! market's positions is theirs and meets the claims, and the auditor checks the receipt.
//!
//! The auditor builds the [`Request`] from chain state, so they trust its numbers. The owner checks
//! it against the chain before proving: the cover set must be the market's whole set of positions
//! as they stand, and enough of them must meet the claims, or the proof would point at the owner.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use borsh::{BorshDeserialize, BorshSerialize};
use bosphorus_core::{Position, claims, shares::{ratio_down, ratio_up}};
use bosphorus_lending::MarketId;
use lee::AccountId;
use risc0_zkvm::{ExecutorEnv, InnerReceipt, ProverOpts, Receipt, default_prover};

pub use bosphorus_disclosure::{Claims, Header, Journal, Request, Valuation, Witness, check, disclosure_id, fmt_wad, parse_wad};
pub use bosphorus_methods::{DISCLOSURE_ELF, DISCLOSURE_ID};

use crate::{Client, Holder, lending::Scanner, now_ms, pool::hex32};

/// Below this many positions meeting the claims, the owner's client refuses to prove (unless forced).
pub const MIN_COVER: usize = 3;
/// The request's price may differ from the feed's by this much (basis points) when the owner checks it.
pub const PRICE_TOLERANCE_BPS: u128 = 200;

/// What the owner sends back: the request it answers and the receipt.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Attestation {
    pub request: Request,
    /// `bincode(risc0_zkvm::Receipt)`.
    pub receipt: Vec<u8>,
}

/// The owner's view of a request.
#[derive(Debug, Clone)]
pub struct Checked {
    /// Positions in the cover set that meet the claims.
    pub qualifying: usize,
    pub cover: usize,
}

/// What a verified attestation says.
#[derive(Debug, Clone)]
pub struct Verified {
    pub disclosure_id: [u8; 32],
    pub request: Request,
    pub qualifying: usize,
    /// Dev-mode receipt (`RISC0_DEV_MODE=1`): proves nothing.
    pub fake: bool,
}

impl Client {
    /// A market's positions (found by scanning blocks from `from_block`: the lending instance's
    /// deploy, or 1) and their current contents. Refuses if the sequencer skipped blocks: the
    /// cover set would silently lack whatever they touched.
    async fn market_positions(&self, lending: AccountId, market: &MarketId, from_block: u64) -> Result<BTreeMap<[u8; 32], Position>> {
        let mut scanner = Scanner::new(lending, from_block);
        scanner.scan(self).await?;
        if let Some((a, b)) = scanner.missing.first() {
            bail!("the sequencer doesn't return blocks {a}..={b} ({} gaps), so the cover set may be incomplete; scan from a later block (--from-block) if the lending instance is newer", scanner.missing.len());
        }
        let mut out = BTreeMap::new();
        for (&id, m) in &scanner.positions {
            if m != market {
                continue;
            }
            if let Some(p) = self.position(lending, id).await? {
                out.insert(id.into_value(), p);
            }
        }
        Ok(out)
    }

    /// The market valued now, at the feed price.
    async fn valuation(&self, lending: AccountId, market: &MarketId, at_ms: u64) -> Result<Valuation> {
        let m = self.market(lending, market).await?;
        let feed = self.feed(m.params.oracle_feed, m.params.oracle_program).await?;
        ensure!(feed.updated_at > 0, "the market's feed has no price yet");
        let s = claims::project(&m.state, &m.params.irm, at_ms)?;
        Ok(Valuation {
            price: feed.price,
            rb: ratio_up(s.total_borrow_assets, s.total_borrow_shares)?,
            rs: ratio_down(s.total_supply_assets, s.total_supply_shares)?,
            lltv: m.params.lltv,
            at_ms,
        })
    }

    /// The auditor's side: a fresh request for `claims` on a market, from chain state.
    pub async fn disclosure_request(&self, lending: AccountId, market: &MarketId, claims: Claims, label: String, from_block: u64) -> Result<Request> {
        let valuation = self.valuation(lending, market, now_ms()?).await?;
        let positions = self.market_positions(lending, market, from_block).await?.into_iter().collect();
        let header = Header { label, nonce: rand::random(), lending: lending.into_value(), market_id: *market, valuation, claims };
        let request = Request { header, positions };
        request.validate().map_err(|e| anyhow!(e))?;
        Ok(request)
    }

    /// The owner's side, before proving: the request matches the chain, so its cover set is real.
    pub async fn check_disclosure_request(&self, request: &Request, from_block: u64) -> Result<Checked> {
        request.validate().map_err(|e| anyhow!(e))?;
        let lending = AccountId::new(request.header.lending);
        let chain = self.market_positions(lending, &request.header.market_id, from_block).await?;
        let asked: BTreeMap<_, _> = request.positions.iter().cloned().collect();
        if let Some(id) = asked.keys().find(|id| !chain.contains_key(*id)) {
            bail!("the request lists {}, which isn't a position of this market", AccountId::new(*id));
        }
        let missing = chain.keys().filter(|id| !asked.contains_key(*id)).count();
        ensure!(missing == 0, "the request leaves out {missing} of the market's positions (a narrowed cover set); ask for a fresh request");
        let changed = chain.iter().filter(|(id, p)| asked[*id] != **p).count();
        ensure!(changed == 0, "{changed} positions changed since the request was made; ask for a fresh request");

        let now = self.valuation(lending, &request.header.market_id, now_ms()?).await?;
        let v = &request.header.valuation;
        ensure!(v.lltv == now.lltv, "the request's LLTV isn't the market's");
        let off = now.price.abs_diff(v.price) * bosphorus_core::U256::from(10_000u64);
        ensure!(off <= now.price * bosphorus_core::U256::from(PRICE_TOLERANCE_BPS), "the request's price is more than {PRICE_TOLERANCE_BPS} bp from the feed's");
        // Ratios only grow with accrual, so the request's can't be ahead of now's. Older (lower) ones
        // flatter every position alike: the auditor's loss, and the cover count below still holds.
        ensure!(v.rb <= now.rb && v.rs <= now.rs, "the request's ratios are ahead of the market's");
        Ok(Checked { qualifying: request.qualifying(), cover: request.positions.len() })
    }

    /// Proves `request` for `holder`'s position `index` in the request's market (the first one
    /// that meets the claims if `None`). Proves locally (the GPU with the `prove` feature).
    ///
    /// Refuses unless `force` when fewer than [`MIN_COVER`] positions could be the prover's: those
    /// that meet the claims now *and* met every earlier request this id proved in this market
    /// (intersecting the cover sets of repeated proofs narrows the owner down). `history` keeps those
    /// sets (one file per market and id).
    pub async fn disclosure_prove(&self, request: &Request, holder: Holder, index: Option<u32>, force: bool, history: &Path, from_block: u64) -> Result<Attestation> {
        let Holder::Private(owner) = holder else { bail!("only private owners have anything to hide: use a Private/ account") };
        let checked = self.check_disclosure_request(request, from_block).await?;
        let h = &request.header;
        let secret = self.spending_key(owner)?;
        let file = history.join(format!("{}-{}", hex32(&h.market_id), hex32(&disclosure_id(&secret))));
        let mut cover: BTreeSet<[u8; 32]> = request.qualifying_ids().into_iter().collect();
        if let Ok(past) = std::fs::read_to_string(&file) {
            let past: BTreeSet<[u8; 32]> = past.lines().filter_map(unhex).collect();
            cover.retain(|id| past.contains(id));
        }
        ensure!(
            force || cover.len() >= MIN_COVER,
            "a proof would narrow you down to {} positions ({} of {} meet these claims{}); need {MIN_COVER}, --force to prove anyway",
            cover.len(),
            checked.qualifying,
            checked.cover,
            if cover.len() < checked.qualifying { ", fewer counting your earlier proofs in this market" } else { "" }
        );

        let lending = AccountId::new(h.lending);
        let owned = self.owned_positions(lending, &h.market_id, holder).await?;
        let (index, position, _) = owned
            .into_iter()
            .find(|(i, _, p)| index.is_none_or(|want| want == *i) && check(&h.valuation, &h.claims, p).is_ok())
            .with_context(|| format!("{holder} has no position in this market that meets the claims"))?;
        let witness = Witness::new(request, position.into_value(), secret, owner.into_value(), index)
            .context("your position isn't in the request's cover set")?;
        let bytes = borsh::to_vec(&witness)?;
        let receipt = tokio::task::spawn_blocking(move || -> Result<Receipt> {
            let env = ExecutorEnv::builder().write(&(bytes.len() as u32))?.write_slice(&bytes).build()?;
            Ok(default_prover().prove_with_opts(env, DISCLOSURE_ELF, &ProverOpts::succinct())?.receipt)
        })
        .await??;
        std::fs::create_dir_all(history)?;
        std::fs::write(&file, cover.iter().map(|id| hex32(id) + "\n").collect::<String>())?;
        Ok(Attestation { request: request.clone(), receipt: bincode::serialize(&receipt)? })
    }

    /// The spending key of one of this wallet's private accounts (the position salts' HMAC key).
    fn spending_key(&self, id: AccountId) -> Result<[u8; 32]> {
        let found = self
            .wallet
            .storage()
            .key_chain()
            .private_account(id)
            .with_context(|| format!("{id} isn't a private account in this wallet"))?;
        Ok(found.key_chain.secret_spending_key.0)
    }

    /// The disclosure id of one of this wallet's private accounts: give it to auditors once.
    pub fn disclosure_id(&self, holder: Holder) -> Result<[u8; 32]> {
        let Holder::Private(id) = holder else { bail!("disclosure ids belong to private accounts") };
        Ok(disclosure_id(&self.spending_key(id)?))
    }
}

fn unhex(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    (s.len() == 64).then_some(out)
}

/// The auditor's check, offline: the receipt is the disclosure guest's, and it answers `asked`
/// (the request the auditor made and kept).
pub fn disclosure_verify(asked: &Request, attestation: &Attestation) -> Result<Verified> {
    ensure!(attestation.request == *asked, "the attestation answers a different request");
    let receipt: Receipt = bincode::deserialize(&attestation.receipt).context("not a receipt")?;
    let fake = matches!(receipt.inner, InnerReceipt::Fake(_));
    receipt.verify(DISCLOSURE_ID).context("the proof doesn't verify against the disclosure guest")?;
    let journal: Journal = borsh::from_slice(&receipt.journal.bytes).context("not a disclosure journal")?;
    ensure!(journal.request == asked.digest(), "the proof is for another request");
    Ok(Verified { disclosure_id: journal.disclosure_id, request: asked.clone(), qualifying: asked.qualifying(), fake })
}

#[cfg(test)]
mod tests {
    #[test]
    fn salts_match_the_client() {
        let (secret, market, owner) = ([3; 32], [4; 32], [5; 32]);
        let client = crate::lending::position_salt(&secret, &market, &lee::AccountId::new(owner), 7);
        assert_eq!(client, bosphorus_disclosure::position_salt(&secret, &market, &owner, 7));
    }
}
