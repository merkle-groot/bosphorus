//! Essential screened-market tests (design/screened-markets.md), in-process with the real guests.
//! Money safety: a screened market takes inflows only from approved lots (or market lots) summing to
//! the amount, pays outflows only into market lots, and only a lending instance the pool registered
//! can credit them; the pool's bytecode is checked at market creation. Authorisation: only the
//! lots' holder spends them. Privacy: a private lender's screened supply publishes no holder.
//!
//! `RISC0_DEV_MODE=1 cargo test -p bosphorus-methods --release --test screened`.

mod common;

use bosphorus_core::{WAD, claims};
use bosphorus_lending::{Claimed, Funding, Instruction, Interval, MarketAccounts, U256, config_id as lending_config};
use bosphorus_methods::{LENDING_ELF, POOL_ELF, TOKEN_ELF};
use bosphorus_pool::{
    Approved, Instruction as PoolIx, Label, Lots, Shard, Spend, approved_id, config_id, decode_lots, deposit_label, escrow_id,
    market_label, record_id, sha256,
};
use common::{
    addr, key,
    private::Keys,
    world::{ADMIN, ALICE, BOB, KEEPER, MALLORY, World, ZERO, setup},
};
use lee::AccountId;
use lee_core::native_token::encode_balance;

const ASP: u8 = 21;

struct S {
    w: World,
    pool: AccountId,
}

/// The lending world with its market screened by a pool whose ASP is `ASP`, lending registered.
fn screened() -> S {
    assert!(std::env::var("RISC0_DEV_MODE").is_ok_and(|v| v == "1" || v == "true"), "run with RISC0_DEV_MODE=1");
    let mut w = setup();
    let pool = w.chain.deploy(POOL_ELF, &key(20));
    let accounts = vec![(pool, pool), (config_id(&pool), pool), (approved_id(&pool), pool)];
    w.chain.call(pool, accounts, &[&key(20)], PoolIx::Init { asp: addr(&key(ASP)) }).unwrap();
    enable_pool(&mut w, pool);
    let mut s = S { w, pool };
    s.set_market(s.w.lending, true).unwrap();
    s.w.params.pool = Some(pool);
    s.w.create_market().unwrap();
    s
}

fn enable_pool(w: &mut World, pool: AccountId) {
    let image_id = w.chain.header(pool).image_id;
    let admin = vec![(addr(&key(ADMIN)), w.lending), (lending_config(&w.lending), w.lending)];
    w.chain.call(w.lending, admin, &[&key(ADMIN)], Instruction::EnableAsset { image_id }).unwrap();
}

impl S {
    fn m(&self) -> MarketAccounts {
        self.w.accounts()
    }

    fn escrow(&self, asset: AccountId) -> AccountId {
        escrow_id(&self.pool, &asset)
    }

    fn set_market(&mut self, lending: AccountId, registered: bool) -> anyhow::Result<u64> {
        let accounts = vec![(addr(&key(ASP)), self.pool), (config_id(&self.pool), self.pool)];
        self.w.chain.call(self.pool, accounts, &[&key(ASP)], PoolIx::SetMarket { lending, registered })
    }

    fn approved(&self) -> Approved {
        match borsh::from_slice(&self.w.chain.shard(approved_id(&self.pool), self.pool)).unwrap() {
            Shard::Approved(approved) => approved,
            other => panic!("{other:?}"),
        }
    }

    fn lots(&self, of: AccountId) -> Lots {
        decode_lots(&self.w.chain.shard(of, self.pool)).unwrap()
    }

    fn market_lot(&self, of: AccountId, asset: AccountId) -> u128 {
        self.lots(of).0.iter().find(|l| l.label == market_label(&asset)).map_or(0, |l| l.amount)
    }

    /// `from` deposits publicly into lots on its own public account (a keeper's way in); approved
    /// unless `pending`.
    fn deposit(&mut self, from: u8, asset: AccountId, amount: u128, nonce: u8, pending: bool) -> Label {
        let (pool, depositor) = (self.pool, addr(&key(from)));
        let label = deposit_label(&pool, &asset, &depositor, &[nonce; 32]);
        let accounts = vec![(depositor, asset), (record_id(&pool, &label), pool), (depositor, pool), (self.escrow(asset), asset)];
        self.w.chain.call(pool, accounts, &[&key(from)], PoolIx::Deposit { asset, amount, nonce: [nonce; 32] }).unwrap();
        if !pending {
            let mut set = self.approved();
            set.approve(sha256, &[label]).unwrap();
            let accounts = vec![(addr(&key(ASP)), pool), (config_id(&pool), pool), (approved_id(&pool), pool)];
            self.w.chain.call(pool, accounts, &[&key(ASP)], PoolIx::Approve { labels: vec![label], root: set.root }).unwrap();
        }
        label
    }

    /// Spends `(label, amount)`: the market lot without a path, others with their path (a label
    /// outside the set gets the first leaf's).
    fn funding(&self, spends: &[(Label, u128)]) -> Funding {
        let set = self.approved();
        let spends = spends
            .iter()
            .map(|(label, amount)| {
                let market = [self.w.tka, self.w.tkb].iter().any(|a| market_label(a) == *label);
                let path = (!market).then(|| set.path(sha256, set.index_of(label).unwrap_or(0)).unwrap());
                Spend { label: *label, amount: *amount, path }
            })
            .collect();
        Funding { spends, root: set.root }
    }

    fn tail(&self, asset: AccountId) -> [(AccountId, AccountId); 2] {
        [(config_id(&self.pool), self.pool), (self.escrow(asset), asset)]
    }

    fn supply(&mut self, payer: u8, position: AccountId, assets: u128, funding: Option<Funding>) -> anyhow::Result<u64> {
        let (l, m, tkb) = (self.w.lending, self.m(), self.w.tkb);
        let mut accounts = vec![(m.market, l), (position, l), (addr(&key(payer)), self.pool), (m.loan_vault, tkb)];
        accounts.extend(self.tail(tkb));
        let ix = Instruction::Supply { params: self.w.params.clone(), assets, rs: self.w.ratios().0, window: self.w.window(), funding };
        self.w.lend(ix, accounts, &[&key(payer)])
    }

    fn supply_collateral(&mut self, payer: u8, assets: u128, funding: Funding) -> anyhow::Result<u64> {
        let (l, m, tka) = (self.w.lending, self.m(), self.w.tka);
        let mut accounts = vec![(self.w.position_of(payer), l), (addr(&key(payer)), self.pool), (m.collateral_vault, tka)];
        accounts.extend(self.tail(tka));
        let ix = Instruction::SupplyCollateral { params: self.w.params.clone(), assets, funding: Some(funding) };
        self.w.lend(ix, accounts, &[&key(payer)])
    }

    fn borrow(&mut self, owner: u8, assets: u128) -> anyhow::Result<u64> {
        let (l, m, tkb) = (self.w.lending, self.m(), self.w.tkb);
        let mut accounts =
            vec![(addr(&key(owner)), tkb), (m.market, l), (self.w.position_of(owner), l), (self.w.feed, self.w.oracle), (m.loan_vault, tkb)];
        accounts.extend(self.tail(tkb));
        let ix = Instruction::Borrow { params: self.w.params.clone(), assets, salt: ZERO, health: self.w.health() };
        self.w.lend(ix, accounts, &[&key(owner)])
    }

    fn repay(&mut self, payer: u8, shares: u128, funding: Funding) -> anyhow::Result<u64> {
        let (l, m, tkb) = (self.w.lending, self.m(), self.w.tkb);
        let mut accounts = vec![(m.market, l), (self.w.position_of(payer), l), (addr(&key(payer)), self.pool), (m.loan_vault, tkb)];
        accounts.extend(self.tail(tkb));
        let ix = Instruction::Repay { params: self.w.params.clone(), shares, rb: self.w.ratios().1, window: self.w.window(), funding: Some(funding) };
        self.w.lend(ix, accounts, &[&key(payer)])
    }

    fn withdraw(&mut self, owner: u8, shares: u128) -> anyhow::Result<u64> {
        let (l, m, tkb) = (self.w.lending, self.m(), self.w.tkb);
        let mut accounts = vec![(addr(&key(owner)), tkb), (m.market, l), (self.w.position_of(owner), l), (m.loan_vault, tkb)];
        accounts.extend(self.tail(tkb));
        let ix = Instruction::Withdraw { params: self.w.params.clone(), shares, salt: ZERO, rs: self.w.ratios().0, window: self.w.window() };
        self.w.lend(ix, accounts, &[&key(owner)])
    }

    fn withdraw_collateral(&mut self, owner: u8, assets: u128) -> anyhow::Result<u64> {
        let (l, m, tka) = (self.w.lending, self.m(), self.w.tka);
        let mut accounts =
            vec![(addr(&key(owner)), tka), (m.market, l), (self.w.position_of(owner), l), (self.w.feed, self.w.oracle), (m.collateral_vault, tka)];
        accounts.extend(self.tail(tka));
        let ix = Instruction::WithdrawCollateral { params: self.w.params.clone(), assets, salt: ZERO, health: Some(self.w.health()) };
        self.w.lend(ix, accounts, &[&key(owner)])
    }

    /// Alice borrows against 1,000,000 TKA from lots; bob lends 3,000,000 TKB from lots.
    fn funded(&mut self) {
        let (tka, tkb) = (self.w.tka, self.w.tkb);
        let lent = self.deposit(BOB, tkb, 3_000_000, 1, false);
        self.supply(BOB, self.w.position_of(BOB), 3_000_000, Some(self.funding(&[(lent, 3_000_000)]))).unwrap();
        let collateral = self.deposit(ALICE, tka, 1_000_000, 2, false);
        self.supply_collateral(ALICE, 1_000_000, self.funding(&[(collateral, 1_000_000)])).unwrap();
    }
}

/// Inflows: only approved lots (or a market lot), only summing to the amount, never a direct
/// transfer, only the holder's own lots.
#[test]
fn a_screened_market_takes_only_approved_lots() {
    let mut s = screened();
    let tkb = s.w.tkb;
    let approved = s.deposit(BOB, tkb, 1_000, 1, false);
    let pending = s.deposit(BOB, tkb, 500, 2, true);
    let position = s.w.position_of(BOB);
    assert!(s.supply(BOB, position, 1_000, None).is_err(), "a direct payment into a screened market");
    let (l, m) = (s.w.lending, s.m());
    let direct = vec![(m.market, l), (position, l), (addr(&key(BOB)), tkb), (m.loan_vault, tkb)];
    let ix = Instruction::Supply { params: s.w.params.clone(), assets: 1_000, rs: s.w.ratios().0, window: s.w.window(), funding: None };
    assert!(s.w.lend(ix, direct, &[&key(BOB)]).is_err(), "a direct payment without the pool accounts");
    assert!(s.supply(BOB, position, 500, Some(s.funding(&[(pending, 500)]))).is_err(), "a pending lot paid");
    assert!(s.supply(BOB, position, 600, Some(s.funding(&[(approved, 500), (pending, 100)]))).is_err(), "a pending lot mixed in");
    assert!(s.supply(BOB, position, 1_000, Some(s.funding(&[(approved, 900)]))).is_err(), "lots short of the amount");
    assert!(s.supply(BOB, position, 900, Some(s.funding(&[(approved, 1_000)]))).is_err(), "lots above the amount");
    assert!(s.supply(BOB, position, 1_001, Some(s.funding(&[(approved, 1_001)]))).is_err(), "more than the lot holds");
    assert!(s.supply(MALLORY, position, 1_000, Some(s.funding(&[(approved, 1_000)]))).is_err(), "someone else's lot");
    let fake = Spend { label: approved, amount: 1_000, path: None };
    let no_path = Funding { spends: vec![fake], root: s.approved().root };
    assert!(s.supply(BOB, position, 1_000, Some(no_path)).is_err(), "a deposit's lot spent without a path");

    s.supply(BOB, position, 600, Some(s.funding(&[(approved, 600)]))).unwrap();
    assert_eq!((s.w.balance(tkb, s.m().loan_vault), s.w.balance(tkb, s.escrow(tkb))), (600, 900));
    assert_eq!(s.lots(addr(&key(BOB))).total(tkb.value()), 900);
    assert!(s.w.position(BOB).supply_shares > 0);
}

/// Outflows land as market lots; a market lot pays back in without a path and out of the pool;
/// it has no ragequit.
#[test]
fn outflows_land_as_market_lots_and_pay_back_in() {
    let mut s = screened();
    s.funded();
    let (tka, tkb, alice) = (s.w.tka, s.w.tkb, addr(&key(ALICE)));
    let tkb_before = s.w.balance(tkb, alice);
    s.borrow(ALICE, 1_000_000).unwrap();
    assert_eq!(s.market_lot(alice, tkb), 1_000_000, "the borrow is alice's market lot");
    assert_eq!(s.w.balance(tkb, alice), tkb_before, "nothing paid to her balance");
    assert_eq!(s.w.balance(tkb, s.escrow(tkb)), 1_000_000);

    let shares = s.w.position(ALICE).borrow_shares;
    let assets = claims::repay_assets(shares, &s.w.ratios().1).unwrap();
    // The borrowed lot, topped up from an approved lot for the rounding (repay pays up).
    let extra = s.deposit(ALICE, tkb, 10, 9, false);
    let mut spends = vec![(market_label(&tkb), assets.min(1_000_000))];
    if assets > 1_000_000 {
        spends.push((extra, assets - 1_000_000));
    }
    let funding = s.funding(&spends);
    s.repay(ALICE, shares, funding).unwrap();
    assert_eq!(s.w.position(ALICE).borrow_shares, 0);
    assert_eq!(s.market_lot(alice, tkb), 0);

    s.withdraw_collateral(ALICE, 400_000).unwrap();
    assert_eq!(s.market_lot(alice, tka), 400_000, "collateral back as a market lot");
    let bob_shares = s.w.position(BOB).supply_shares;
    s.withdraw(BOB, bob_shares / 2).unwrap();
    assert!(s.market_lot(addr(&key(BOB)), tkb) > 0, "a lender's withdrawal as a market lot");

    // Out of the pool: the market lot pays out without a path.
    let to = addr(&key(30));
    let pool = s.pool;
    let accounts = vec![(alice, pool), (config_id(&pool), pool), (s.escrow(tka), tka), (to, tka)];
    let spends = vec![Spend { label: market_label(&tka), amount: 400_000, path: None }];
    let ix = PoolIx::Withdraw { asset: tka, spends, root: [0; 32] };
    s.w.chain.call(pool, accounts, &[&key(ALICE)], ix).unwrap();
    assert_eq!(s.w.balance(tka, to), 400_000);
    assert_eq!(s.market_lot(alice, tka), 0);

    s.withdraw_collateral(ALICE, 100_000).unwrap();
    let accounts = vec![(alice, tka), (record_id(&pool, &market_label(&tka)), pool), (alice, pool), (s.escrow(tka), tka)];
    let ix = PoolIx::Ragequit { asset: tka, label: market_label(&tka), amount: 100_000 };
    assert!(s.w.chain.call(pool, accounts, &[&key(ALICE)], ix).is_err(), "a market lot ragequit");
}

/// Only a lending instance the pool registered pays into market lots; nobody calls `Pay` directly.
#[test]
fn only_a_registered_lending_pays_into_lots() {
    let mut s = screened();
    s.funded();
    s.set_market(s.w.lending, false).unwrap();
    assert!(s.borrow(ALICE, 1_000).is_err(), "an unregistered lending paid into a lot");
    s.set_market(s.w.lending, true).unwrap();
    s.borrow(ALICE, 1_000).unwrap();

    // Called directly, nothing authorises the vault.
    let (pool, tkb, m) = (s.pool, s.w.tkb, s.m());
    let accounts = vec![(m.loan_vault, tkb), (config_id(&pool), pool), (addr(&key(MALLORY)), pool), (s.escrow(tkb), tkb)];
    assert!(s.w.chain.call(pool, accounts, &[&key(MALLORY)], PoolIx::Pay { asset: tkb, amount: 1 }).is_err(), "a direct Pay");
    let accounts = vec![(addr(&key(MALLORY)), pool), (config_id(&pool), pool)];
    let ix = PoolIx::SetMarket { lending: addr(&key(MALLORY)), registered: true };
    assert!(s.w.chain.call(pool, accounts, &[&key(MALLORY)], ix).is_err(), "only the ASP registers");
}

/// The pool moves the vaults' tokens, so a screened market needs its bytecode enabled; the same
/// lots can't fund an unscreened market's inflow, nor a market name another pool's accounts.
#[test]
fn create_market_checks_the_pool() {
    let mut w = setup();
    let pool = w.chain.deploy(POOL_ELF, &key(20));
    w.params.pool = Some(pool);
    w.params.lltv = 7 * WAD / 10; // a new market id
    let admin = vec![(addr(&key(ADMIN)), w.lending), (lending_config(&w.lending), w.lending)];
    w.chain.call(w.lending, admin, &[&key(ADMIN)], Instruction::EnableLltv { lltv: w.params.lltv }).unwrap();
    assert!(w.create_market().is_err(), "a screened market with unenabled pool bytecode");
    let images = vec![w.chain.header(w.tkb).image_id, w.chain.header(w.tka).image_id, w.chain.header(w.tka).image_id];
    assert!(w.create_market_claiming(images).is_err(), "the pool claimed as token bytecode");
    // A mutable pool header is refused too, even with its image enabled.
    enable_pool(&mut w, pool);
    let mutable = w.chain.instance_with(w.chain.header(pool).program_first_segment, &key(22), false);
    w.params.pool = Some(mutable);
    assert!(w.create_market().is_err(), "a pool that can be updated");
    w.params.pool = Some(pool);
    w.create_market().unwrap();
}

/// A liquidator repays from its lots; the seized collateral lands as its market lot.
#[test]
fn a_screened_liquidation() {
    let mut s = screened();
    s.funded();
    s.borrow(ALICE, 2_000_000).unwrap();
    s.w.publish("2.0");
    let (tka, tkb, keeper) = (s.w.tka, s.w.tkb, addr(&key(KEEPER)));
    let p = s.w.position(ALICE);
    let claimed = Claimed { collateral: p.collateral, borrow_shares: p.borrow_shares };
    let (price, rb) = (Interval { lo: U256::ZERO, hi: s.w.price() }, s.w.ratios().1);
    let amount = claims::full_liquidation(&claimed, &price, &rb, s.w.params.lltv).unwrap();
    let l = claims::liquidation(amount, &claimed, &price, &rb, s.w.params.lltv).unwrap();
    let lot = s.deposit(KEEPER, tkb, l.repaid_assets, 7, false);
    let (lending, m, pool) = (s.w.lending, s.m(), s.pool);
    let accounts = vec![
        (keeper, pool),
        (m.market, lending),
        (s.w.position_of(ALICE), lending),
        (s.w.feed, s.w.oracle),
        (m.loan_vault, tkb),
        (m.collateral_vault, tka),
        (config_id(&pool), pool),
        (s.escrow(tkb), tkb),
        (s.escrow(tka), tka),
    ];
    let (params, window) = (s.w.params.clone(), s.w.window());
    let ix = |funding| Instruction::Liquidate { params: params.clone(), claimed, amount, price, rb, window, funding };
    assert!(s.w.lend(ix(None), accounts.clone(), &[&key(KEEPER)]).is_err(), "a liquidation paid directly");
    let funding = s.funding(&[(lot, l.repaid_assets)]);
    s.w.lend(ix(Some(funding)), accounts, &[&key(KEEPER)]).unwrap();
    assert_eq!(s.market_lot(keeper, tka), l.seized);
    assert_eq!(s.lots(keeper).total(tkb.value()), 0);
}

/// A private lender supplies from its lots into shares held on the same private account, and
/// withdraws them back into its market lot. Nothing published names the lender or its labels.
#[test]
fn a_private_screened_lender() {
    let mut s = screened();
    let (pool, tkb, l, m) = (s.pool, s.w.tkb, s.w.lending, s.m());
    let mut lender = s.w.chain.private_account(Keys::new(60), &[(tkb, encode_balance(0))]);
    let id = lender.keys.id();
    // Bob's public deposit lands in the lender's lots (as the app's screened dive does).
    let label = deposit_label(&pool, &tkb, &addr(&key(BOB)), &[5; 32]);
    let accounts = vec![(addr(&key(BOB)), tkb), (record_id(&pool, &label), pool), (id, pool), (s.escrow(tkb), tkb)];
    let witness = lender.witness(&s.w.chain, true);
    let ix = PoolIx::Deposit { asset: tkb, amount: 5_000, nonce: [5; 32] };
    let tx = s.w.chain.prove_signed(pool, POOL_ELF, &[(tkb, TOKEN_ELF)], accounts, &[&key(BOB)], vec![witness], ix).unwrap();
    s.w.chain.settle(&tx).unwrap();
    let mut lots = Lots::default();
    lots.credit(bosphorus_pool::Lot { asset: *tkb.value(), label, amount: 5_000 }).unwrap();
    lender.advance(&tx, |a| a.data.set_shard(pool, bosphorus_pool::encode_lots(lots.clone()).try_into().unwrap()));
    let mut set = s.approved();
    set.approve(sha256, &[label]).unwrap();
    let accounts = vec![(addr(&key(ASP)), pool), (config_id(&pool), pool), (approved_id(&pool), pool)];
    s.w.chain.call(pool, accounts, &[&key(ASP)], PoolIx::Approve { labels: vec![label], root: set.root }).unwrap();

    let deps = [(s.w.tka, TOKEN_ELF), (tkb, TOKEN_ELF), (pool, POOL_ELF)];
    let rs = s.w.ratios().0;
    let ix = Instruction::Supply { params: s.w.params.clone(), assets: 5_000, rs, window: s.w.window(), funding: Some(s.funding(&[(label, 5_000)])) };
    let selectors = vec![(m.market, l), (id, l), (id, pool), (m.loan_vault, tkb), (config_id(&pool), pool), (s.escrow(tkb), tkb)];
    let witness = lender.witness(&s.w.chain, true);
    let tx = s.w.chain.prove(l, LENDING_ELF, &deps, selectors, vec![witness], ix).unwrap();
    s.w.chain.settle(&tx).unwrap();
    let public: Vec<_> = tx.message.public_actions.iter().map(|a| a.account_id).collect();
    for account in &public {
        assert!([m.market, m.loan_vault, config_id(&pool), s.escrow(tkb)].contains(account), "touched {account}");
    }
    let bytes = borsh::to_vec(&tx).unwrap();
    for (what, needle) in lender.keys.identifying_bytes().into_iter().chain([("label", label.to_vec())]) {
        assert!(!bytes.windows(needle.len()).any(|w| w == needle), "{what} published");
    }
    assert_eq!(s.w.balance(tkb, m.loan_vault), 5_000);
    let shares = claims::supply_shares(5_000, &rs).unwrap();
    let market_id = s.w.params.id();
    let position = |supply_shares| {
        let p = bosphorus_lending::Position { market_id, supply_shares, ..Default::default() };
        borsh::to_vec(&bosphorus_lending::Shard::Position(p)).unwrap()
    };
    lender.advance(&tx, |a| {
        a.data.set_shard(pool, vec![].try_into().unwrap());
        a.data.set_shard(l, position(shares).try_into().unwrap());
    });

    let rs = s.w.ratios().0;
    let assets = claims::withdraw_assets(shares, &Interval { lo: rs.lo, hi: u128::MAX }).unwrap();
    let ix = Instruction::Withdraw { params: s.w.params.clone(), shares, salt: ZERO, rs, window: s.w.window() };
    let selectors = vec![(id, tkb), (m.market, l), (id, l), (m.loan_vault, tkb), (config_id(&pool), pool), (s.escrow(tkb), tkb)];
    let witness = lender.witness(&s.w.chain, true);
    let tx = s.w.chain.prove(l, LENDING_ELF, &deps, selectors, vec![witness], ix).unwrap();
    s.w.chain.settle(&tx).unwrap();
    let mut lots = Lots::default();
    lots.merge(bosphorus_pool::Lot { asset: *tkb.value(), label: market_label(&tkb), amount: assets }).unwrap();
    lender.advance(&tx, |a| {
        a.data.set_shard(pool, bosphorus_pool::encode_lots(lots).try_into().unwrap());
        a.data.set_shard(l, position(0).try_into().unwrap());
    });
    assert_eq!(s.w.balance(tkb, s.escrow(tkb)), assets);
}
