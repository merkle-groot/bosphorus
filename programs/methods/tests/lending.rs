//! Essential `lending` tests (plan Phase 2): ownership, LLTV, liquidity, claims, admin.
//! Run in-process on `V03State` with the real token, oracle and lending guests.
//!
//! `cargo test -p bosphorus-methods --release --test lending -- --ignored --nocapture` prints the
//! per-operation cycle table (plan 2.5).

mod common;

use bosphorus_core::{MAX_WINDOW_MS, WAD, claims, scale_price};
use bosphorus_lending::{
    HealthClaim, Instruction, Interval, IrmParams, Market, MarketAccounts, MarketParams, Position, Shard, U256,
    Window, config_id, position_id,
};
use bosphorus_methods::{LENDING_ELF, ORACLE_ELF, TOKEN_ELF};
use common::{Chain, addr, key};
use lee::{AccountId, PrivateKey};

const ALICE: u8 = 2; // borrower
const BOB: u8 = 3; // lender
const MALLORY: u8 = 4;
const ADMIN: u8 = 5;
const MINTER: u8 = 1;
const PUBLISHER: u8 = 6;
const LENDING: u8 = 7;
const W: u64 = 60_000;
const ZERO: [u8; 32] = [0; 32];

struct World {
    chain: Chain,
    /// Collateral.
    tka: AccountId,
    /// Loan asset.
    tkb: AccountId,
    oracle: AccountId,
    feed: AccountId,
    lending: AccountId,
    params: MarketParams,
}

fn irm() -> IrmParams {
    IrmParams::linear_kink_apr(WAD / 50, WAD / 10, 3 * WAD, 8 * WAD / 10)
}

/// TKA (collateral) and TKB (loan) minted to alice and bob, TKA/TKB = 2.5, lending initialised by
/// the admin with LLTV 0.8 and one IRM, and a market created.
fn setup() -> World {
    let mut chain = Chain::new();
    let tka = chain.deploy(TOKEN_ELF, &key(10));
    let tkb = chain.instance(chain.header(tka).program_first_segment, &key(11));
    for (token, deployer) in [(tka, key(10)), (tkb, key(11))] {
        let init = bosphorus_token::Instruction::Init { mint_authority: addr(&key(MINTER)), decimals: 6 };
        let config = bosphorus_token::config_id(&token);
        chain.call(token, vec![(token, token), (config, token)], &[&deployer], init).unwrap();
        for holder in [ALICE, BOB, MALLORY] {
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
    fn accounts(&self) -> MarketAccounts {
        MarketAccounts::new(&self.lending, &self.params.id())
    }

    fn position_of(&self, owner: u8) -> AccountId {
        position_id(&self.lending, &self.params.id(), &addr(&key(owner)), &ZERO)
    }

    fn market(&self) -> Market {
        match borsh::from_slice(&self.chain.shard(self.accounts().market, self.lending)).unwrap() {
            Shard::Market(market) => market,
            other => panic!("{other:?}"),
        }
    }

    fn position(&self, owner: u8) -> Position {
        match borsh::from_slice(&self.chain.shard(self.position_of(owner), self.lending)).unwrap() {
            Shard::Position(position) => position,
            other => panic!("{other:?}"),
        }
    }

    fn balance(&self, token: AccountId, of: AccountId) -> u128 {
        bosphorus_token::decode_balance(&self.chain.shard(of, token)).unwrap()
    }

    fn publish(&mut self, price: &str) {
        let publisher = key(PUBLISHER);
        let accounts = vec![(addr(&publisher), self.oracle), (self.feed, self.oracle)];
        let price = scale_price(price, 6, 6).unwrap();
        let publish = bosphorus_oracle::Instruction::Publish { price, timestamp: self.chain.now };
        self.chain.call(self.oracle, accounts, &[&publisher], publish).unwrap();
    }

    fn price(&self) -> U256 {
        let feed: bosphorus_oracle::Feed = borsh::from_slice(&self.chain.shard(self.feed, self.oracle)).unwrap();
        feed.price
    }

    fn window(&self) -> Window {
        Window { t: self.chain.now, w: W }
    }

    /// The tightest ratio claims at the current time: `(rs, rb)`.
    fn ratios(&self) -> (Interval<u128>, Interval<u128>) {
        claims::ratio_claims(&self.market().state, &self.params.irm, self.chain.now, 0, 0).unwrap()
    }

    fn health(&self) -> HealthClaim {
        HealthClaim { price: Interval::exact(self.price()), rb: self.ratios().1, window: self.window() }
    }

    fn lend(&mut self, ix: Instruction, accounts: Vec<(AccountId, AccountId)>, signers: &[&PrivateKey]) -> anyhow::Result<u64> {
        self.chain.call(self.lending, accounts, signers, ix)
    }

    fn create_market(&mut self) -> anyhow::Result<u64> {
        let accounts = vec![(config_id(&self.lending), self.lending), (self.accounts().market, self.lending)];
        self.lend(Instruction::CreateMarket { params: self.params.clone(), window: self.window() }, accounts, &[])
    }

    fn supply(&mut self, payer: u8, position: AccountId, assets: u128) -> anyhow::Result<u64> {
        let (l, m) = (self.lending, self.accounts());
        let accounts = vec![(m.market, l), (position, l), (addr(&key(payer)), self.tkb), (m.loan_vault, self.tkb)];
        let ix = Instruction::Supply { params: self.params.clone(), assets, rs: self.ratios().0, window: self.window() };
        self.lend(ix, accounts, &[&key(payer)])
    }

    fn supply_collateral(&mut self, payer: u8, position: AccountId, assets: u128) -> anyhow::Result<u64> {
        let (l, m) = (self.lending, self.accounts());
        let accounts = vec![(position, l), (addr(&key(payer)), self.tka), (m.collateral_vault, self.tka)];
        self.lend(Instruction::SupplyCollateral { params: self.params.clone(), assets }, accounts, &[&key(payer)])
    }

    /// `owner` is who the tx names as owner, `signer` who actually signs (if anyone).
    fn withdraw_as(&mut self, owner: u8, signer: Option<u8>, position: AccountId, shares: u128, rs: Interval<u128>) -> anyhow::Result<u64> {
        let (l, m) = (self.lending, self.accounts());
        let accounts = vec![(addr(&key(owner)), self.tkb), (m.market, l), (position, l), (m.loan_vault, self.tkb)];
        let ix = Instruction::Withdraw { params: self.params.clone(), shares, salt: ZERO, rs, window: self.window() };
        let signer = signer.map(key);
        self.lend(ix, accounts, &signer.iter().collect::<Vec<_>>())
    }

    fn withdraw(&mut self, owner: u8, shares: u128) -> anyhow::Result<u64> {
        let rs = self.ratios().0;
        self.withdraw_as(owner, Some(owner), self.position_of(owner), shares, rs)
    }

    fn borrow_as(&mut self, owner: u8, signer: Option<u8>, position: AccountId, assets: u128, health: HealthClaim) -> anyhow::Result<u64> {
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

    fn borrow(&mut self, owner: u8, assets: u128) -> anyhow::Result<u64> {
        let health = self.health();
        self.borrow_as(owner, Some(owner), self.position_of(owner), assets, health)
    }

    fn withdraw_collateral_as(&mut self, owner: u8, signer: Option<u8>, position: AccountId, assets: u128, health: Option<HealthClaim>) -> anyhow::Result<u64> {
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

    fn withdraw_collateral(&mut self, owner: u8, assets: u128) -> anyhow::Result<u64> {
        let health = self.health();
        self.withdraw_collateral_as(owner, Some(owner), self.position_of(owner), assets, Some(health))
    }

    fn repay(&mut self, payer: u8, position: AccountId, shares: u128) -> anyhow::Result<u64> {
        let (l, m) = (self.lending, self.accounts());
        let accounts = vec![(m.market, l), (position, l), (addr(&key(payer)), self.tkb), (m.loan_vault, self.tkb)];
        let ix = Instruction::Repay { params: self.params.clone(), shares, rb: self.ratios().1, window: self.window() };
        self.lend(ix, accounts, &[&key(payer)])
    }
}

/// Bob lends 1,000,000 TKB; alice posts 1,000,000 TKA (worth 2.5M TKB, 2M borrowable).
fn funded() -> World {
    let mut w = setup();
    w.supply(BOB, w.position_of(BOB), 1_000_000).unwrap();
    w.supply_collateral(ALICE, w.position_of(ALICE), 1_000_000).unwrap();
    w
}

#[test]
fn a_non_owner_cant_withdraw_borrow_or_withdraw_collateral() {
    let mut w = funded();
    let (bob, alice) = (w.position_of(BOB), w.position_of(ALICE));
    let rs = w.ratios().0;
    let shares = w.position(BOB).supply_shares;

    // Mallory signs as herself, naming bob's position; or names bob as owner without his signature.
    assert!(w.withdraw_as(MALLORY, Some(MALLORY), bob, shares, rs).is_err(), "position not derived from mallory");
    assert!(w.withdraw_as(BOB, None, bob, shares, rs).is_err(), "bob didn't sign");
    assert!(w.withdraw_as(BOB, Some(MALLORY), bob, shares, rs).is_err(), "mallory signed for bob");
    let health = w.health();
    assert!(w.borrow_as(MALLORY, Some(MALLORY), alice, 1000, health).is_err(), "borrow against alice");
    assert!(w.borrow_as(ALICE, None, alice, 1000, health).is_err(), "alice didn't sign");
    assert!(w.withdraw_collateral_as(MALLORY, Some(MALLORY), alice, 1, Some(health)).is_err(), "collateral of alice");
    assert!(w.withdraw_collateral_as(MALLORY, Some(MALLORY), alice, 1, None).is_err(), "collateral, no-debt path");
    assert!(w.withdraw_collateral_as(ALICE, None, alice, 1, None).is_err(), "alice didn't sign");
    assert_eq!(w.position(ALICE).collateral, 1_000_000);
    assert_eq!(w.position(BOB).supply_shares, shares);

    // The owners can.
    w.borrow(ALICE, 1000).unwrap();
    assert_eq!(w.balance(w.tkb, addr(&key(ALICE))), 1_000_000_000_000 + 1000);
    w.withdraw(BOB, shares / 2).unwrap();
    assert_eq!(w.position(BOB).supply_shares, shares - shares / 2);
}

#[test]
fn borrowing_past_the_lltv_fails() {
    let mut w = funded();
    // max borrow = 1,000,000 TKA × 2.5 × 0.8 = 2,000,000 TKB; bob only lent 1,000,000.
    w.supply(BOB, w.position_of(BOB), 2_000_000).unwrap();
    assert!(w.borrow(ALICE, 2_000_001).is_err(), "past LLTV");
    w.borrow(ALICE, 2_000_000).unwrap();
    assert!(w.borrow(ALICE, 1).is_err(), "at the limit");
    assert!(w.withdraw_collateral(ALICE, 1).is_err(), "collateral that backs the debt");
    assert!(w.withdraw_collateral_as(ALICE, Some(ALICE), w.position_of(ALICE), 1, None).is_err(), "no-price path with debt");

    // Interest pushes it over: still can't borrow even 1 more.
    w.chain.now += 3_600_000;
    w.publish("2.5");
    assert!(w.borrow(ALICE, 1).is_err(), "after interest");

    // Repaying everything frees all the collateral, even on the no-price path.
    let shares = w.position(ALICE).borrow_shares;
    w.repay(ALICE, w.position_of(ALICE), shares).unwrap();
    assert_eq!(w.position(ALICE).borrow_shares, 0);
    w.withdraw_collateral_as(ALICE, Some(ALICE), w.position_of(ALICE), 1_000_000, None).unwrap();
    assert_eq!(w.balance(w.tka, w.accounts().collateral_vault), 0);
}

#[test]
fn withdrawing_beyond_available_liquidity_fails() {
    let mut w = funded();
    w.borrow(ALICE, 600_000).unwrap();
    let shares = w.position(BOB).supply_shares;
    assert!(w.withdraw(BOB, shares).is_err(), "only 400,000 is left");
    // ~400,000 TKB worth of shares is fine; the rest has to wait for repayment.
    let (rs, _) = w.ratios();
    let available = claims::supply_shares(400_000, &rs).unwrap();
    w.withdraw(BOB, available).unwrap();
    assert!(w.withdraw(BOB, shares - available).is_err());
    let m = w.market().state;
    assert!(m.total_borrow_assets <= m.total_supply_assets);
    assert_eq!(w.balance(w.tkb, w.accounts().loan_vault), 1_000_000 - 600_000 - claims::withdraw_assets(available, &rs).unwrap());
}

#[test]
fn a_claim_outside_its_interval_fails() {
    let mut w = funded();
    w.borrow(ALICE, 500_000).unwrap();
    w.chain.now += 7 * 24 * 3_600_000; // a week of interest moves both ratios
    w.publish("2.5");
    let (rs, rb) = w.ratios();
    let (bob, alice) = (w.position_of(BOB), w.position_of(ALICE));
    let shares = w.position(BOB).supply_shares;
    let above = |r: Interval<u128>| Interval { lo: r.hi + 1, hi: r.hi + 10 };
    let below = |r: Interval<u128>| Interval { lo: r.lo - 10, hi: r.lo - 1 };

    // Supply ratio.
    assert!(w.withdraw_as(BOB, Some(BOB), bob, shares / 4, above(rs)).is_err(), "rs above");
    assert!(w.withdraw_as(BOB, Some(BOB), bob, shares / 4, below(rs)).is_err(), "rs below");
    // A claim read before a week of accrual: the classic stale-read case.
    let stale = claims::ratio_claims(&w.market().state, &w.params.irm, w.market().state.last_update, 0, 0).unwrap();
    assert!(w.withdraw_as(BOB, Some(BOB), bob, shares / 4, stale.0).is_err(), "stale rs");

    // Borrow ratio and price.
    let good = w.health();
    let borrow = |w: &mut World, h: HealthClaim| w.borrow_as(ALICE, Some(ALICE), alice, 1000, h);
    assert!(borrow(&mut w, HealthClaim { rb: above(rb), ..good }).is_err(), "rb above");
    assert!(borrow(&mut w, HealthClaim { rb: below(rb), ..good }).is_err(), "rb below");
    let p = w.price();
    assert!(borrow(&mut w, HealthClaim { price: Interval { lo: p + U256::from(1u8), hi: p * U256::from(2u8) }, ..good }).is_err(), "price above");
    assert!(borrow(&mut w, HealthClaim { price: Interval { lo: U256::ZERO, hi: p - U256::from(1u8) }, ..good }).is_err(), "price below");

    // Time: a future t (block time < t), a window over the cap, a price too old for the window.
    let future = Window { t: w.chain.now + 1, w: W };
    assert!(borrow(&mut w, HealthClaim { window: future, ..good }).is_err(), "t in the future");
    let long = Window { t: w.chain.now, w: MAX_WINDOW_MS + 1 };
    assert!(borrow(&mut w, HealthClaim { window: long, ..good }).is_err(), "window too long");
    w.chain.now += w.params.max_price_age - W + 1; // still fresh now, stale by the window's end
    let late = w.health();
    assert!(borrow(&mut w, late).is_err(), "price stale within the window");
    w.publish("2.5");
    let fresh = w.health();
    borrow(&mut w, fresh).unwrap();
}

#[test]
fn donations_dont_move_the_ratios() {
    let mut w = funded();
    let before = w.ratios();
    let vault = w.accounts().loan_vault;
    let tkb = w.tkb;
    w.chain.call(tkb, vec![(addr(&key(MALLORY)), tkb), (vault, tkb)], &[&key(MALLORY)], bosphorus_token::Instruction::Transfer { amount: 1_000_000_000 }).unwrap();
    assert_eq!(w.ratios(), before);
    let shares = w.position(BOB).supply_shares;
    w.supply(BOB, w.position_of(BOB), 1_000_000).unwrap();
    assert_eq!(w.position(BOB).supply_shares, 2 * shares, "same shares per asset as before the donation");
}

#[test]
fn admin_ops_need_the_admin() {
    let mut w = setup();
    let (l, config) = (w.lending, config_id(&w.lending));
    let as_ = |k: u8| vec![(addr(&key(k)), l), (config, l)];
    let lltv = WAD / 2;
    assert!(w.lend(Instruction::EnableLltv { lltv }, as_(MALLORY), &[&key(MALLORY)]).is_err());
    assert!(w.lend(Instruction::EnableLltv { lltv }, as_(ADMIN), &[]).is_err(), "unsigned");
    assert!(w.lend(Instruction::SetFeeRecipient { fee_recipient: addr(&key(MALLORY)) }, as_(MALLORY), &[&key(MALLORY)]).is_err());
    let set_fee = |w: &World| Instruction::SetFee { params: w.params.clone(), fee: WAD / 10, window: w.window() };
    let mut accounts = as_(MALLORY);
    accounts.push((w.accounts().market, l));
    assert!(w.lend(set_fee(&w), accounts, &[&key(MALLORY)]).is_err());
    let init = Instruction::Init { admin: addr(&key(MALLORY)), fee_recipient: addr(&key(MALLORY)) };
    assert!(w.lend(init.clone(), vec![(l, l), (config, l)], &[&key(LENDING)]).is_err(), "re-init");

    // A market with a disabled LLTV can't be created; the admin can enable it.
    w.params.lltv = lltv;
    assert!(w.create_market().is_err(), "LLTV not enabled");
    w.lend(Instruction::EnableLltv { lltv }, as_(ADMIN), &[&key(ADMIN)]).unwrap();
    w.create_market().unwrap();
    assert!(w.create_market().is_err(), "already exists");
    let mut accounts = as_(ADMIN);
    accounts.push((w.accounts().market, l));
    w.lend(set_fee(&w), accounts, &[&key(ADMIN)]).unwrap();
    assert_eq!(w.market().state.fee, WAD / 10);
}

/// Plan 2.5: charged cycles per operation (the sequencer's execution gas), small shards.
#[test]
#[ignore = "prints the cycle table"]
fn cycle_table() {
    let mut w = setup();
    let mut rows = vec![];
    let mut fresh = setup();
    fresh.params.max_price_age += 1; // a new market, to measure creation
    rows.push(("create_market", fresh.create_market().unwrap()));
    rows.push(("supply (new position)", w.supply(BOB, w.position_of(BOB), 1_000_000).unwrap()));
    rows.push(("supply", w.supply(BOB, w.position_of(BOB), 1_000_000).unwrap()));
    rows.push(("supply_collateral (new position)", w.supply_collateral(ALICE, w.position_of(ALICE), 1_000_000).unwrap()));
    rows.push(("supply_collateral", w.supply_collateral(ALICE, w.position_of(ALICE), 1_000_000).unwrap()));
    rows.push(("borrow (first)", w.borrow(ALICE, 500_000).unwrap()));
    w.chain.now += 60_000;
    rows.push(("borrow (with accrual)", w.borrow(ALICE, 500_000).unwrap()));
    w.chain.now += 60_000;
    let shares = w.position(ALICE).borrow_shares / 2;
    rows.push(("repay", w.repay(ALICE, w.position_of(ALICE), shares).unwrap()));
    rows.push(("withdraw_collateral (with price)", w.withdraw_collateral(ALICE, 100_000).unwrap()));
    let shares = w.position(BOB).supply_shares / 4;
    rows.push(("withdraw", w.withdraw(BOB, shares).unwrap()));
    let shares = w.position(ALICE).borrow_shares;
    w.repay(ALICE, w.position_of(ALICE), shares).unwrap();
    let alice = w.position_of(ALICE);
    rows.push(("withdraw_collateral (no debt, no price)", w.withdraw_collateral_as(ALICE, Some(ALICE), alice, 100_000, None).unwrap()));
    let tkb = w.tkb;
    let transfer = bosphorus_token::Instruction::Transfer { amount: 1 };
    rows.push(("(token transfer, for reference)", w.chain.call(tkb, vec![(addr(&key(BOB)), tkb), (addr(&key(ALICE)), tkb)], &[&key(BOB)], transfer).unwrap()));
    let market_len = w.chain.shard(w.accounts().market, w.lending).len();
    let position_len = w.chain.shard(w.position_of(ALICE), w.lending).len();
    println!("\n| op | cycles |\n|---|---:|");
    for (op, cycles) in rows {
        println!("| {op} | {cycles} |");
    }
    println!("\nshards: market {market_len} B, position {position_len} B");
}
