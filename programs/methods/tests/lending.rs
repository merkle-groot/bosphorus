//! Essential `lending` tests (plan Phase 2): ownership, LLTV, liquidity, claims, admin.
//! Run in-process on `V03State` with the real token, oracle and lending guests.
//!
//! `cargo test -p bosphorus-methods --release --test lending -- --ignored --nocapture` prints the
//! per-operation cycle table (plan 2.5).

mod common;

use bosphorus_core::{MAX_WINDOW_MS, WAD, claims};
use bosphorus_lending::{HealthClaim, Instruction, Interval, LiquidationAmount, Shard, U256, Window, config_id, position_id};
use common::{
    addr, key,
    world::{ADMIN, ALICE, BOB, KEEPER, LENDING, MALLORY, W, World, ZERO, at_the_limit, funded, setup},
};

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

/// Findings F1: a market's assets must run enabled bytecode behind an immutable header, or a
/// malicious "token" could move the balances of anyone who signs a tx in that market.
#[test]
fn markets_only_take_enabled_immutable_asset_bytecode() {
    let mut w = setup();
    let token = w.chain.header(w.tka).image_id;
    let as_ = |k: u8| vec![(addr(&key(k)), w.lending), (config_id(&w.lending), w.lending)];
    assert!(w.lend(Instruction::EnableAsset { image_id: [7; 8] }, as_(MALLORY), &[&key(MALLORY)]).is_err(), "not the admin");

    // Other bytecode (the oracle program) as collateral, claimed honestly or as the token's.
    w.params.collateral_asset = w.oracle;
    let oracle = w.chain.header(w.oracle).image_id;
    assert!(w.create_market_claiming(vec![token, oracle]).is_err(), "bytecode not enabled");
    assert!(w.create_market_claiming(vec![token, token]).is_err(), "claimed image doesn't match the header");

    // The token's own bytecode behind a header that can still be updated.
    let first = w.chain.header(w.tka).program_first_segment;
    w.params.collateral_asset = w.chain.instance_with(first, &key(30), false);
    assert!(w.create_market().is_err(), "mutable header");

    // The native token needs no header.
    w.params.collateral_asset = lee_core::native_token::NATIVE_TOKEN_PROGRAM_ID;
    w.create_market().unwrap();
}

#[test]
fn a_healthy_position_cant_be_liquidated() {
    let mut w = at_the_limit();
    let (claimed, price, rb) = w.liquidation_claims(ALICE);
    let some = LiquidationAmount::RepayShares(claimed.borrow_shares / 10);
    assert!(w.liquidate_with(ALICE, claimed, some, price, rb).is_err(), "healthy, honest claims");
    // Claims that would make it look unhealthy are rejected by the feed's and the market's applies.
    let low_price = Interval { lo: U256::ZERO, hi: price.hi * U256::from(9u8) / U256::from(10u8) };
    assert!(w.liquidate_with(ALICE, claimed, some, low_price, rb).is_err(), "price claim below the feed");
    let high_rb = Interval { lo: rb.hi * 2, hi: rb.hi * 3 };
    assert!(w.liquidate_with(ALICE, claimed, some, price, high_rb).is_err(), "rb claim above the market");
    let mut more_debt = claimed;
    more_debt.borrow_shares *= 2;
    assert!(w.liquidate_with(ALICE, more_debt, some, price, rb).is_err(), "position claim not exact");
    assert_eq!(w.position(ALICE).borrow_shares, claimed.borrow_shares);

    // The price drops to 2.4: debt 2,000,000 > 1,000,000 × 2.4 × 0.8. The keeper repays it all.
    w.publish("2.4");
    let (keeper_tkb, keeper_tka) = (w.balance(w.tkb, addr(&key(KEEPER))), w.balance(w.tka, addr(&key(KEEPER))));
    let (stale_claimed, ..) = w.liquidation_claims(ALICE);
    let (_, l) = w.liquidate(ALICE).unwrap();
    assert_eq!((l.repaid_shares, l.bad_shares), (stale_claimed.borrow_shares, 0), "repaid in full, no bad debt");
    // The amounts moved are exactly what core derives from the claims.
    assert_eq!(w.balance(w.tkb, addr(&key(KEEPER))), keeper_tkb - l.repaid_assets);
    assert_eq!(w.balance(w.tka, addr(&key(KEEPER))), keeper_tka + l.seized);
    // 2,000,000 × 1.0638 / 2.4 = 886,524 TKA seized; alice keeps the rest.
    assert_eq!((l.repaid_assets, l.seized), (2_000_000, 886_524));
    let p = w.position(ALICE);
    assert_eq!((p.borrow_shares, p.collateral), (0, 1_000_000 - 886_524));
    // A second liquidation of the now empty, healthy position fails.
    assert!(w.liquidate(ALICE).is_err());
}

#[test]
fn bad_debt_lowers_rs_and_zeroes_the_position() {
    let mut w = at_the_limit();
    let rs_before = w.ratios().0;
    let supplied_before = w.market().state.total_supply_assets;
    // The price crashes to 1: 1,000,000 TKA is worth 1,000,000 against 2,000,000 of debt.
    w.publish("1");
    let (_, l) = w.liquidate(ALICE).unwrap();
    assert_eq!(l.seized, 1_000_000, "all the collateral");
    assert!(l.bad_shares > 0);
    let p = w.position(ALICE);
    assert_eq!((p.collateral, p.borrow_shares), (0, 0), "position zeroed");
    let m = w.market().state;
    assert_eq!((m.total_borrow_assets, m.total_borrow_shares), (0, 0));
    // 1,000,000 / 1.0638 ≈ 940,000 repaid; the remaining ~1,060,000 is written off against bob.
    let lost = supplied_before - m.total_supply_assets;
    assert_eq!(lost, 2_000_000 - l.repaid_assets);
    assert!(w.ratios().0.hi < rs_before.lo, "rs fell");
    assert_eq!(w.balance(w.tka, w.accounts().collateral_vault), 0);
    // Bob can still withdraw everything that's left.
    let shares = w.position(BOB).supply_shares;
    w.withdraw(BOB, shares).unwrap();
}

#[test]
fn fees_go_to_the_fee_recipient() {
    let mut w = at_the_limit();
    let accounts = vec![(addr(&key(ADMIN)), w.lending), (config_id(&w.lending), w.lending), (w.accounts().market, w.lending)];
    let set_fee = Instruction::SetFee { params: w.params.clone(), fee: WAD / 10, window: w.window() };
    w.lend(set_fee, accounts, &[&key(ADMIN)]).unwrap();
    w.chain.now += 30 * 24 * 3_600_000;
    w.publish("2.5");
    w.repay(ALICE, w.position_of(ALICE), 1_000_000).unwrap(); // accrues a month of interest
    let pending = w.market().state.pending_fee_shares;
    assert!(pending > 0);

    assert!(w.claim_fees(addr(&key(MALLORY)), pending).is_err(), "not the fee recipient");
    assert!(w.claim_fees(addr(&key(ADMIN)), pending + 1).is_err(), "more than pending");
    w.claim_fees(addr(&key(ADMIN)), pending).unwrap(); // anyone can submit it
    assert_eq!(w.market().state.pending_fee_shares, 0);
    let recipient = position_id(&w.lending, &w.params.id(), &addr(&key(ADMIN)), &ZERO);
    let shard = w.chain.shard(recipient, w.lending);
    let Shard::Position(p) = borsh::from_slice(&shard).unwrap() else { panic!() };
    assert_eq!(p.supply_shares, pending);
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
    let mut fees = at_the_limit();
    let accounts = vec![(addr(&key(ADMIN)), fees.lending), (config_id(&fees.lending), fees.lending), (fees.accounts().market, fees.lending)];
    let set_fee = Instruction::SetFee { params: fees.params.clone(), fee: WAD / 10, window: fees.window() };
    rows.push(("set_fee", fees.lend(set_fee, accounts, &[&key(ADMIN)]).unwrap()));
    fees.chain.now += 30 * 24 * 3_600_000;
    fees.publish("2.5");
    fees.repay(ALICE, fees.position_of(ALICE), 1_000_000).unwrap();
    let pending = fees.market().state.pending_fee_shares;
    rows.push(("claim_fees", fees.claim_fees(addr(&key(ADMIN)), pending).unwrap()));
    let mut liq = at_the_limit();
    liq.publish("2.4");
    rows.push(("liquidate (repay all)", liq.liquidate(ALICE).unwrap().0));
    let mut bad = at_the_limit();
    bad.publish("1");
    rows.push(("liquidate (seize all, bad debt)", bad.liquidate(ALICE).unwrap().0));
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
