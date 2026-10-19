//! The lending test world: TKA (collateral) and TKB (loan) token instances, an oracle feed, a
//! lending instance with LLTV 0.8 and one IRM, and a TKB/TKA market. Shared by the lending and
//! private-mode tests.

use bosphorus_core::{WAD, claims, scale_price};
use bosphorus_lending::{
    Claimed, HealthClaim, Instruction, Interval, IrmParams, Liquidation, LiquidationAmount, Market, MarketAccounts,
    MarketParams, Position, Shard, U256, Window, config_id, position_id,
};
use bosphorus_methods::{LENDING_ELF, ORACLE_ELF, TOKEN_ELF};
use lee::{AccountId, PrivateKey};

use super::{Chain, addr, key};

pub const ALICE: u8 = 2; // borrower
pub const BOB: u8 = 3; // lender
pub const MALLORY: u8 = 4;
pub const ADMIN: u8 = 5;
pub const MINTER: u8 = 1;
pub const PUBLISHER: u8 = 6;
pub const LENDING: u8 = 7;
pub const KEEPER: u8 = 8;
pub const W: u64 = 60_000;
pub const ZERO: [u8; 32] = [0; 32];

#[derive(Clone)]
pub struct World {
    pub chain: Chain,
    /// Collateral.
    pub tka: AccountId,
    /// Loan asset.
    pub tkb: AccountId,
    pub oracle: AccountId,
    pub feed: AccountId,
    pub lending: AccountId,
    pub params: MarketParams,
}

pub fn irm() -> IrmParams {
    IrmParams::linear_kink_apr(WAD / 50, WAD / 10, 3 * WAD, 8 * WAD / 10)
}

/// TKA (collateral) and TKB (loan) minted to alice and bob, TKA/TKB = 2.5, lending initialised by
/// the admin with LLTV 0.8 and one IRM, and a market created.
pub fn setup() -> World {
    let mut chain = Chain::new();
    let tka = chain.deploy(TOKEN_ELF, &key(10));
    let tkb = chain.instance(chain.header(tka).program_first_segment, &key(11));
    for (token, deployer) in [(tka, key(10)), (tkb, key(11))] {
        let init = bosphorus_token::Instruction::Init { mint_authority: addr(&key(MINTER)), decimals: 6 };
        let config = bosphorus_token::config_id(&token);
        chain.call(token, vec![(token, token), (config, token)], &[&deployer], init).unwrap();
        for holder in [ALICE, BOB, MALLORY, KEEPER] {
            let accounts = vec![(addr(&key(MINTER)), token), (config, token), (addr(&key(holder)), token)];
            let mint = bosphorus_token::Instruction::Mint { amount: 1_000_000_000_000 };
            chain.call(token, accounts, &[&key(MINTER)], mint).unwrap();
        }
    }

    let oracle = chain.deploy(ORACLE_ELF, &key(12));
    let publisher = key(PUBLISHER);
    let feed = bosphorus_oracle::feed_id(&oracle, &addr(&publisher), "TKA/TKB");
    let accounts = vec![(addr(&publisher), oracle), (feed, oracle)];
    let create = bosphorus_oracle::Instruction::CreateFeed { name: "TKA/TKB".into() };
    chain.call(oracle, accounts, &[&publisher], create).unwrap();

    let lending = chain.deploy(LENDING_ELF, &key(LENDING));
    let config = config_id(&lending);
    let init = Instruction::Init { admin: addr(&key(ADMIN)), fee_recipient: addr(&key(ADMIN)) };
    chain.call(lending, vec![(lending, lending), (config, lending)], &[&key(LENDING)], init).unwrap();
    let params = MarketParams {
        loan_asset: tkb,
        collateral_asset: tka,
        oracle_program: oracle,
        oracle_feed: feed,
        irm: irm(),
        lltv: 8 * WAD / 10,
        max_price_age: 3_600_000,
    };
    let admin = vec![(addr(&key(ADMIN)), lending), (config, lending)];
    chain.call(lending, admin.clone(), &[&key(ADMIN)], Instruction::EnableLltv { lltv: params.lltv }).unwrap();
    chain.call(lending, admin, &[&key(ADMIN)], Instruction::EnableIrm { irm: params.irm }).unwrap();

    let mut world = World { chain, tka, tkb, oracle, feed, lending, params };
    world.publish("2.5");
    world.create_market().unwrap();
    world
}

impl World {
    pub fn accounts(&self) -> MarketAccounts {
        MarketAccounts::new(&self.lending, &self.params.id())
    }

    pub fn position_of(&self, owner: u8) -> AccountId {
        position_id(&self.lending, &self.params.id(), &addr(&key(owner)), &ZERO)
    }

    pub fn market(&self) -> Market {
        match borsh::from_slice(&self.chain.shard(self.accounts().market, self.lending)).unwrap() {
            Shard::Market(market) => market,
            other => panic!("{other:?}"),
        }
    }

    pub fn position(&self, owner: u8) -> Position {
        match borsh::from_slice(&self.chain.shard(self.position_of(owner), self.lending)).unwrap() {
            Shard::Position(position) => position,
            other => panic!("{other:?}"),
        }
    }

    pub fn balance(&self, token: AccountId, of: AccountId) -> u128 {
        bosphorus_token::decode_balance(&self.chain.shard(of, token)).unwrap()
    }

    /// Publishes at `now + 1 ms` (feed timestamps strictly increase).
    pub fn publish(&mut self, price: &str) {
        self.chain.now += 1;
        let publisher = key(PUBLISHER);
        let accounts = vec![(addr(&publisher), self.oracle), (self.feed, self.oracle)];
        let price = scale_price(price, 6, 6).unwrap();
        let publish = bosphorus_oracle::Instruction::Publish { price, timestamp: self.chain.now };
        self.chain.call(self.oracle, accounts, &[&publisher], publish).unwrap();
    }

    pub fn price(&self) -> U256 {
        let feed: bosphorus_oracle::Feed = borsh::from_slice(&self.chain.shard(self.feed, self.oracle)).unwrap();
        feed.price
    }

    pub fn window(&self) -> Window {
        Window { t: self.chain.now, w: W }
    }

    /// The tightest ratio claims at the current time: `(rs, rb)`.
    pub fn ratios(&self) -> (Interval<u128>, Interval<u128>) {
        claims::ratio_claims(&self.market().state, &self.params.irm, self.chain.now, 0, 0).unwrap()
    }

    pub fn health(&self) -> HealthClaim {
        HealthClaim { price: Interval::exact(self.price()), rb: self.ratios().1, window: self.window() }
    }

    pub fn lend(&mut self, ix: Instruction, accounts: Vec<(AccountId, AccountId)>, signers: &[&PrivateKey]) -> anyhow::Result<u64> {
        self.chain.call(self.lending, accounts, signers, ix)
    }

    pub fn create_market(&mut self) -> anyhow::Result<u64> {
        let accounts = vec![(config_id(&self.lending), self.lending), (self.accounts().market, self.lending)];
        self.lend(Instruction::CreateMarket { params: self.params.clone(), window: self.window() }, accounts, &[])
    }

    pub fn supply(&mut self, payer: u8, position: AccountId, assets: u128) -> anyhow::Result<u64> {
        let (l, m) = (self.lending, self.accounts());
        let accounts = vec![(m.market, l), (position, l), (addr(&key(payer)), self.tkb), (m.loan_vault, self.tkb)];
        let ix = Instruction::Supply { params: self.params.clone(), assets, rs: self.ratios().0, window: self.window() };
        self.lend(ix, accounts, &[&key(payer)])
    }

    pub fn supply_collateral(&mut self, payer: u8, position: AccountId, assets: u128) -> anyhow::Result<u64> {
        let (l, m) = (self.lending, self.accounts());
        let accounts = vec![(position, l), (addr(&key(payer)), self.tka), (m.collateral_vault, self.tka)];
        self.lend(Instruction::SupplyCollateral { params: self.params.clone(), assets }, accounts, &[&key(payer)])
    }

    /// `owner` is who the tx names as owner, `signer` who actually signs (if anyone).
    pub fn withdraw_as(&mut self, owner: u8, signer: Option<u8>, position: AccountId, shares: u128, rs: Interval<u128>) -> anyhow::Result<u64> {
        let (l, m) = (self.lending, self.accounts());
        let accounts = vec![(addr(&key(owner)), self.tkb), (m.market, l), (position, l), (m.loan_vault, self.tkb)];
        let ix = Instruction::Withdraw { params: self.params.clone(), shares, salt: ZERO, rs, window: self.window() };
        let signer = signer.map(key);
        self.lend(ix, accounts, &signer.iter().collect::<Vec<_>>())
    }

    pub fn withdraw(&mut self, owner: u8, shares: u128) -> anyhow::Result<u64> {
        let rs = self.ratios().0;
        self.withdraw_as(owner, Some(owner), self.position_of(owner), shares, rs)
    }

    pub fn borrow_as(&mut self, owner: u8, signer: Option<u8>, position: AccountId, assets: u128, health: HealthClaim) -> anyhow::Result<u64> {
        let (l, m) = (self.lending, self.accounts());
        let accounts = vec![
            (addr(&key(owner)), self.tkb),
            (m.market, l),
            (position, l),
            (self.feed, self.oracle),
            (m.loan_vault, self.tkb),
        ];
        let ix = Instruction::Borrow { params: self.params.clone(), assets, salt: ZERO, health };
        let signer = signer.map(key);
        self.lend(ix, accounts, &signer.iter().collect::<Vec<_>>())
    }

    pub fn borrow(&mut self, owner: u8, assets: u128) -> anyhow::Result<u64> {
        let health = self.health();
        self.borrow_as(owner, Some(owner), self.position_of(owner), assets, health)
    }

    pub fn withdraw_collateral_as(&mut self, owner: u8, signer: Option<u8>, position: AccountId, assets: u128, health: Option<HealthClaim>) -> anyhow::Result<u64> {
        let (l, m) = (self.lending, self.accounts());
        let mut accounts = vec![(addr(&key(owner)), self.tka)];
        if health.is_some() {
            accounts.extend([(m.market, l), (position, l), (self.feed, self.oracle)]);
        } else {
            accounts.push((position, l));
        }
        accounts.push((m.collateral_vault, self.tka));
        let ix = Instruction::WithdrawCollateral { params: self.params.clone(), assets, salt: ZERO, health };
        let signer = signer.map(key);
        self.lend(ix, accounts, &signer.iter().collect::<Vec<_>>())
    }

    pub fn withdraw_collateral(&mut self, owner: u8, assets: u128) -> anyhow::Result<u64> {
        let health = self.health();
        self.withdraw_collateral_as(owner, Some(owner), self.position_of(owner), assets, Some(health))
    }

    /// What a keeper submits: exact position claim, price `[0, feed]`, tight ratios.
    pub fn liquidation_claims(&self, owner: u8) -> (Claimed, Interval<U256>, Interval<u128>) {
        let p = self.position(owner);
        let claimed = Claimed { collateral: p.collateral, borrow_shares: p.borrow_shares };
        (claimed, Interval { lo: U256::ZERO, hi: self.price() }, self.ratios().1)
    }

    pub fn liquidate_with(
        &mut self,
        owner: u8,
        claimed: Claimed,
        amount: LiquidationAmount,
        price: Interval<U256>,
        rb: Interval<u128>,
    ) -> anyhow::Result<u64> {
        let (l, m) = (self.lending, self.accounts());
        let accounts = vec![
            (addr(&key(KEEPER)), self.tkb),
            (m.market, l),
            (self.position_of(owner), l),
            (self.feed, self.oracle),
            (m.loan_vault, self.tkb),
            (m.collateral_vault, self.tka),
        ];
        let ix = Instruction::Liquidate { params: self.params.clone(), claimed, amount, price, rb, window: self.window() };
        self.lend(ix, accounts, &[&key(KEEPER)])
    }

    /// A full liquidation as the keeper would size it; returns what core derived.
    pub fn liquidate(&mut self, owner: u8) -> anyhow::Result<(u64, Liquidation)> {
        let (claimed, price, rb) = self.liquidation_claims(owner);
        let amount = claims::full_liquidation(&claimed, &price, &rb, self.params.lltv)?;
        let derived = claims::liquidation(amount, &claimed, &price, &rb, self.params.lltv)?;
        Ok((self.liquidate_with(owner, claimed, amount, price, rb)?, derived))
    }

    pub fn claim_fees(&mut self, recipient: AccountId, shares: u128) -> anyhow::Result<u64> {
        let l = self.lending;
        let position = position_id(&l, &self.params.id(), &recipient, &ZERO);
        let accounts = vec![(config_id(&l), l), (self.accounts().market, l), (position, l)];
        self.lend(Instruction::ClaimFees { params: self.params.clone(), recipient, shares }, accounts, &[])
    }

    pub fn repay(&mut self, payer: u8, position: AccountId, shares: u128) -> anyhow::Result<u64> {
        let (l, m) = (self.lending, self.accounts());
        let accounts = vec![(m.market, l), (position, l), (addr(&key(payer)), self.tkb), (m.loan_vault, self.tkb)];
        let ix = Instruction::Repay { params: self.params.clone(), shares, rb: self.ratios().1, window: self.window() };
        self.lend(ix, accounts, &[&key(payer)])
    }
}

/// Bob lends 1,000,000 TKB; alice posts 1,000,000 TKA (worth 2.5M TKB, 2M borrowable).
pub fn funded() -> World {
    let mut w = setup();
    w.supply(BOB, w.position_of(BOB), 1_000_000).unwrap();
    w.supply_collateral(ALICE, w.position_of(ALICE), 1_000_000).unwrap();
    w
}

/// Alice at her LLTV limit: bob lends 3,000,000, alice borrows 2,000,000 against 1,000,000 TKA at 2.5.
pub fn at_the_limit() -> World {
    let mut w = funded();
    w.supply(BOB, w.position_of(BOB), 2_000_000).unwrap();
    w.borrow(ALICE, 2_000_000).unwrap();
    w
}

