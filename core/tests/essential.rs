//! Essential `core` tests (plan Phase 2): rounding direction, inflation resistance, bounds
//! soundness, no over-accrual.

use bosphorus_core::{
    Error, Health, Interval, IrmParams, MarketOp, MarketState, Position, PositionOp, U256, VIRTUAL_ASSETS,
    VIRTUAL_SHARES, WAD, claims,
    position::{is_healthy, max_borrow},
    shares::{check_ratio, exact_to_assets_down, exact_to_assets_up, ratio_down, ratio_up},
};
use proptest::prelude::*;

const SECOND: u64 = 1000;
const YEAR: u64 = 365 * 24 * 3600 * SECOND;

fn u(x: u128) -> U256 {
    U256::from(x)
}

/// A reachable `(assets, shares)` pair: between 1e-6 (fresh market) and ~1 asset per share.
fn totals() -> impl Strategy<Value = (u128, u128)> {
    (0u128..1_000_000_000_000_000_000_000_000, 1u128..=1_000_000).prop_map(|(assets, per)| (assets, assets * per))
}

/// The tightest claim a client can make for a ratio: `[⌊r⌋, ⌈r⌉]`.
fn tight(assets: u128, shares: u128) -> Interval<u128> {
    Interval::new(ratio_down(assets, shares).unwrap(), ratio_up(assets, shares).unwrap()).unwrap()
}

/// `x / y` as exact rational `(x, y)` comparisons: `a/b ≤ c/d`.
fn le(a: U256, b: U256, c: U256, d: U256) -> bool {
    a * d <= c * b
}

fn irm() -> IrmParams {
    IrmParams::linear_kink_apr(WAD / 50, WAD / 10, 3 * WAD, 8 * WAD / 10)
}

proptest! {
    /// Every derivation rounds against the user, at every ratio inside a claim the market accepts.
    #[test]
    fn conversions_round_in_the_protocols_favour(
        (ta, ts) in totals(),
        amount in 1u128..1_000_000_000_000_000_000_000,
        widen_lo in 0u128..1_000_000,
        widen_hi in 0u128..1_000_000,
    ) {
        let claim = Interval::new(ratio_down(ta, ts).unwrap().saturating_sub(widen_lo), ratio_up(ta, ts).unwrap() + widen_hi).unwrap();
        prop_assert!(check_ratio(&claim, ta, ts).is_ok());
        let (a, s) = (u(ta) + u(VIRTUAL_ASSETS), u(ts) + u(VIRTUAL_SHARES));
        // real ratio r = a / s (assets per share)
        if let Ok(shares) = claims::supply_shares(amount, &claim) {
            prop_assert!(le(u(shares), u(1), u(amount) * s, a), "supply: shares ≤ assets / r");
        }
        if let Ok(assets) = claims::withdraw_assets(amount, &claim) {
            prop_assert!(le(u(assets), u(1), u(amount) * a, s), "withdraw: assets ≤ shares · r");
        }
        let shares = claims::borrow_shares(amount, &claim).unwrap();
        prop_assert!(le(u(amount) * s, a, u(shares), u(1)), "borrow: shares ≥ assets / r");
        let assets = claims::repay_assets(amount, &claim).unwrap();
        prop_assert!(le(u(amount) * a, s, u(assets), u(1)), "repay: assets ≥ shares · r");
    }

    /// A claim the real ratio isn't in fails the market's check.
    #[test]
    fn claims_are_checked_exactly((ta, ts) in totals()) {
        let (lo, hi) = (ratio_down(ta, ts).unwrap(), ratio_up(ta, ts).unwrap());
        prop_assert_eq!(check_ratio(&Interval::new(lo, hi).unwrap(), ta, ts), Ok(()));
        prop_assert_eq!(check_ratio(&Interval::new(hi + 1, hi + 2).unwrap(), ta, ts), Err(Error::RatioOutsideClaim));
        if lo > 0 {
            prop_assert_eq!(check_ratio(&Interval::new(0, lo - 1).unwrap(), ta, ts), Err(Error::RatioOutsideClaim));
        }
    }

    /// Supplying then withdrawing the same shares, or borrowing then repaying them, never profits.
    #[test]
    fn round_trips_never_profit(
        (ta, ts) in totals(),
        borrowed_pct in 0u128..=100,
        amount in 1u128..1_000_000_000_000_000_000_000,
    ) {
        let (tb, bs) = (ta * borrowed_pct / 100, ta * borrowed_pct / 100 * 1000);
        let mut m = MarketState { total_supply_assets: ta, total_supply_shares: ts, total_borrow_assets: tb, total_borrow_shares: bs, ..MarketState::new(0) };
        let irm = irm();

        let rs = tight(m.total_supply_assets, m.total_supply_shares);
        let Ok(shares) = claims::supply_shares(amount, &rs) else { return Ok(()) };
        m.apply(&irm, 0, &MarketOp::Supply { rs, assets: amount, shares }).unwrap();
        let rs = tight(m.total_supply_assets, m.total_supply_shares);
        if let Ok(out) = claims::withdraw_assets(shares, &rs) {
            m.apply(&irm, 0, &MarketOp::Withdraw { rs, shares, assets: out }).unwrap();
            prop_assert!(out <= amount, "withdrew {out} after supplying {amount}");
        }

        let rb = tight(m.total_borrow_assets, m.total_borrow_shares);
        let liquidity = m.total_supply_assets - m.total_borrow_assets;
        let borrow = amount.min(liquidity);
        if borrow == 0 { return Ok(()) }
        let shares = claims::borrow_shares(borrow, &rb).unwrap();
        m.apply(&irm, 0, &MarketOp::Borrow { rb, assets: borrow, shares }).unwrap();
        let rb = tight(m.total_borrow_assets, m.total_borrow_shares);
        let repaid = claims::repay_assets(shares, &rb).unwrap();
        prop_assert!(repaid >= borrow, "repaid {repaid} after borrowing {borrow}");
    }

    /// Bounds soundness (constraints §6): if the market accepts the `rb` claim, the oracle accepts
    /// the price claim, and the position is healthy at the conservative ends, then it is healthy by
    /// Morpho's exact formula at every real value inside the claims.
    #[test]
    fn healthy_at_the_bounds_means_healthy_inside(
        (tb, bs) in totals(),
        collateral in 0u128..1_000_000_000_000_000_000_000_000,
        borrow_shares in 1u128..1_000_000_000_000_000_000_000_000_000,
        price in 1u128..100_000_000_000_000_000_000_000_000_000_000_000_000,
        lltv_pct in 1u128..100,
        price_width in 0u128..WAD / 5,
        rb_width in 0u128..WAD / 100,
        at_price in 0u128..=1000,
        at_rb in 0u128..=1000,
    ) {
        let lltv = lltv_pct * WAD / 100;
        let price_claim = claims::price_claim(u(price), price_width).unwrap();
        let rb_claim = claims::ratio_claim(ratio_down(tb, bs).unwrap(), ratio_up(tb, bs).unwrap(), rb_width).unwrap();
        prop_assert!(check_ratio(&rb_claim, tb, bs).is_ok());
        let health = claims::health(&price_claim, &rb_claim, lltv).unwrap();
        if !is_healthy(collateral, borrow_shares, &health).unwrap() { return Ok(()) }

        // Some real state inside the claims: the price anywhere in the claim, the market's own
        // totals at the real ratio.
        let real_price = price_claim.lo + (price_claim.hi - price_claim.lo) * u(at_price) / u(1000);
        let debt = exact_to_assets_up(borrow_shares, tb, bs).unwrap();
        prop_assert!(u(debt) <= max_borrow(collateral, real_price, lltv).unwrap(), "unhealthy at the real values");
        // And at any ratio inside the rb claim.
        let rb = rb_claim.lo + (rb_claim.hi - rb_claim.lo) * at_rb / 1000;
        let at = Health { price: real_price, rb, lltv };
        prop_assert!(is_healthy(collateral, borrow_shares, &at).unwrap());
    }

    /// Interest never exceeds continuous compounding at the IRM's rate, however the accrual is split.
    #[test]
    fn accrual_never_over_accrues(
        tb in 1u128..1_000_000_000_000_000_000_000_000_000,
        apr_pct in 0u128..1_000,
        steps in proptest::collection::vec(1u64..YEAR / 12, 1..8),
    ) {
        // Constant rate, so the bound doesn't depend on utilisation moving between steps.
        let irm = IrmParams::linear_kink_apr(apr_pct * WAD / 100, 0, 0, WAD / 2);
        let IrmParams::LinearKink { base: rate, .. } = irm;
        let mut m = MarketState { total_supply_assets: tb * 2, total_supply_shares: tb * 2_000_000, total_borrow_assets: tb, total_borrow_shares: tb * 1_000_000, ..MarketState::new(0) };
        let mut t = 0;
        for dt in &steps {
            t += dt;
            m.accrue(&irm, t).unwrap();
            prop_assert_eq!(m.accrue(&irm, t).unwrap(), 0, "accruing to the same t twice adds nothing");
            prop_assert_eq!(m.accrue(&irm, t - 1).unwrap(), 0, "accruing backwards adds nothing");
        }
        let x = rate as f64 / WAD as f64 * t as f64 / 1000.0;
        let bound = tb as f64 * x.exp_m1();
        let interest = (m.total_borrow_assets - tb) as f64;
        prop_assert!(interest <= bound * (1.0 + 1e-9) + 1.0, "interest {interest} > continuous {bound}");
        prop_assert_eq!(m.total_supply_assets - tb * 2, m.total_borrow_assets - tb, "suppliers get exactly the interest");
    }

    /// Inflation resistance (Morpho's virtual offsets): whatever an attacker does first on an empty
    /// market, and however much interest then grows the supply side, a victim's deposit loses at
    /// most a few base units to rounding.
    #[test]
    fn a_first_depositor_cannot_inflate_shares(
        attacker in 1u128..1_000_000_000_000_000_000,
        growth in 0u128..1_000_000_000_000_000_000,
        victim in 1u128..1_000_000_000_000_000_000_000_000,
    ) {
        let irm = irm();
        let mut m = MarketState::new(0);
        let rs = tight(0, 0);
        let shares = claims::supply_shares(attacker, &rs).unwrap();
        m.apply(&irm, 0, &MarketOp::Supply { rs, assets: attacker, shares }).unwrap();
        // The only way to raise assets per share without minting shares is interest.
        m.total_supply_assets += growth;
        let rs = tight(m.total_supply_assets, m.total_supply_shares);
        let shares = claims::supply_shares(victim, &rs).unwrap();
        m.apply(&irm, 0, &MarketOp::Supply { rs, assets: victim, shares }).unwrap();
        let worth = exact_to_assets_down(shares, m.total_supply_assets, m.total_supply_shares).unwrap();
        // Loss ≤ one share's worth + 1 (floors), plus `victim / rs.hi` for the claim's WAD rounding
        // (relative 1e-12 on a fresh market). A share is worth (attacker + growth + 1) / (1e6·(attacker + 1)).
        let per_share = (attacker + growth + VIRTUAL_ASSETS).div_ceil(attacker * VIRTUAL_SHARES + VIRTUAL_SHARES);
        let quantisation = victim.div_ceil(rs.hi);
        prop_assert!(victim - worth <= per_share + quantisation + 1, "victim lost {} of {victim}", victim - worth);
    }
}

#[test]
fn a_position_without_a_price_must_be_debt_free() {
    let mut p = Position { collateral: 10, borrow_shares: 1, ..Position::default() };
    assert_eq!(p.apply(&PositionOp::WithdrawCollateral { assets: 1, health: None }), Err(Error::Unhealthy));
    p.borrow_shares = 0;
    p.apply(&PositionOp::WithdrawCollateral { assets: 10, health: None }).unwrap();
    assert_eq!(p.collateral, 0);
}

#[test]
fn liquidity_and_fee_limits() {
    let irm = irm();
    let mut m = MarketState { total_supply_assets: 100, total_supply_shares: 100_000_000, ..MarketState::new(0) };
    let rb = tight(0, 0);
    let borrow = |assets| MarketOp::Borrow { rb, assets, shares: claims::borrow_shares(assets, &rb).unwrap() };
    assert_eq!(m.clone().apply(&irm, 0, &borrow(101)), Err(Error::InsufficientLiquidity));
    m.apply(&irm, 0, &borrow(100)).unwrap();
    let rs = tight(m.total_supply_assets, m.total_supply_shares);
    let withdraw = MarketOp::Withdraw { rs, shares: 1_000_000, assets: claims::withdraw_assets(1_000_000, &rs).unwrap() };
    assert_eq!(m.clone().apply(&irm, 0, &withdraw), Err(Error::InsufficientLiquidity));
    assert_eq!(m.apply(&irm, 0, &MarketOp::SetFee { fee: WAD / 4 + 1 }), Err(Error::FeeTooHigh));
}

#[test]
fn fees_are_a_share_of_interest() {
    let irm = IrmParams::linear_kink_apr(WAD, 0, 0, WAD / 2); // 100% APR flat
    let mut m = MarketState { total_supply_assets: 1_000_000, total_supply_shares: 1_000_000_000_000, total_borrow_assets: 1_000_000, total_borrow_shares: 1_000_000_000_000, ..MarketState::new(0) };
    m.apply(&irm, 0, &MarketOp::SetFee { fee: WAD / 10 }).unwrap();
    let interest = m.accrue(&irm, YEAR).unwrap();
    // taylor3(1) = 1 + 1/2 + 1/6, below e − 1.
    assert!((1_666_000..=1_666_667).contains(&interest), "{interest}");
    let fee_assets = exact_to_assets_down(m.pending_fee_shares, m.total_supply_assets, m.total_supply_shares).unwrap();
    assert!(fee_assets <= interest / 10 && fee_assets + 2 >= interest / 10, "fee {fee_assets} of {interest}");
}

// ---- Phase 3: liquidation ---------------------------------------------------------------------

use bosphorus_core::{
    Claimed, LiquidationAmount, PRICE_SCALE,
    position::liquidation_incentive_factor,
};

/// A market's real borrow side plus a position in it, and claims that contain the real values.
#[derive(Debug, Clone)]
struct Case {
    tb: u128,
    bs: u128,
    claimed: Claimed,
    price: U256,
    price_claim: Interval<U256>,
    rb_claim: Interval<u128>,
    lltv: u128,
}

fn case() -> impl Strategy<Value = Case> {
    (
        totals(),
        0u128..1_000_000_000_000_000_000_000_000,
        1u128..1_000_000_000_000_000_000_000_000_000,
        1u128..100_000_000_000_000_000_000_000_000_000_000_000_000,
        1u128..100,
        (0u128..WAD / 5, 0u128..WAD / 5, 0u128..WAD / 100),
    )
        .prop_map(|((tb, bs), collateral, borrow_shares, price, lltv_pct, (below, above, rb_width))| {
            // The price claim is anywhere around the real price, not centred on it.
            let p = u(price);
            let lo = p - p * u(below) / u(WAD);
            let hi = p + p * u(above) / u(WAD);
            // The market holds at least this position's shares.
            let bs = bs.max(borrow_shares);
            let rb_claim = claims::ratio_claim(ratio_down(tb, bs).unwrap(), ratio_up(tb, bs).unwrap(), rb_width).unwrap();
            Case {
                tb,
                bs,
                claimed: Claimed { collateral, borrow_shares },
                price: p,
                price_claim: Interval::new(lo, hi).unwrap(),
                rb_claim,
                lltv: lltv_pct * WAD / 100,
            }
        })
}

/// Morpho's `toSharesUp` against real totals.
fn exact_to_shares_up(assets: u128, ta: u128, ts: u128) -> U256 {
    (u(assets) * (u(ts) + u(VIRTUAL_SHARES))).div_ceil(u(ta) + u(VIRTUAL_ASSETS))
}

proptest! {
    /// Whatever claims a liquidator makes that the oracle and market would accept, a position that
    /// is healthy by Morpho's exact formula at the real values can't be liquidated.
    #[test]
    fn a_healthy_position_cant_be_liquidated(c in case()) {
        prop_assert!(check_ratio(&c.rb_claim, c.tb, c.bs).is_ok());
        prop_assert!(c.price_claim.contains(c.price));
        let debt = exact_to_assets_up(c.claimed.borrow_shares, c.tb, c.bs).unwrap();
        let healthy = u(debt) <= max_borrow(c.claimed.collateral, c.price, c.lltv).unwrap();
        if !healthy { return Ok(()) }
        let health = claims::liquidation_health(&c.price_claim, &c.rb_claim, c.lltv).unwrap();
        let amount = LiquidationAmount::RepayShares(c.claimed.borrow_shares);
        let Ok(l) = claims::liquidation(amount, &c.claimed, &c.price_claim, &c.rb_claim, c.lltv) else { return Ok(()) };
        let mut p = Position { collateral: c.claimed.collateral, borrow_shares: c.claimed.borrow_shares, ..Position::default() };
        prop_assert_eq!(p.apply(&PositionOp::Liquidate { claimed: c.claimed, health, liquidation: l }), Err(Error::Healthy));
    }

    /// Seized and repaid amounts are never better for the liquidator than Morpho's formulas at the
    /// real price and ratio: no more collateral seized, no fewer shares or assets repaid.
    #[test]
    fn liquidation_amounts_are_conservative(c in case(), fraction in 1u128..=1000) {
        let lif = liquidation_incentive_factor(c.lltv).unwrap();
        // Repay part of the debt.
        let shares = (c.claimed.borrow_shares * fraction / 1000).max(1);
        if let Ok(l) = claims::liquidation(LiquidationAmount::RepayShares(shares), &c.claimed, &c.price_claim, &c.rb_claim, c.lltv) {
            let assets = exact_to_assets_down(shares, c.tb, c.bs).unwrap();
            let morpho_seized = u(assets) * u(lif) / u(WAD) * PRICE_SCALE / c.price;
            prop_assert!(u(l.seized) <= morpho_seized, "seized {} > Morpho's {morpho_seized}", l.seized);
            prop_assert!(l.repaid_assets >= exact_to_assets_up(shares, c.tb, c.bs).unwrap(), "repaid less than the shares are worth");
        }
        // Seize part of the collateral.
        let collateral = (c.claimed.collateral * fraction / 1000).max(1);
        if let Ok(l) = claims::liquidation(LiquidationAmount::SeizeCollateral(collateral), &c.claimed, &c.price_claim, &c.rb_claim, c.lltv) {
            let quoted = (u(collateral) * c.price).div_ceil(PRICE_SCALE);
            let morpho_assets = u128::try_from((quoted * u(WAD)).div_ceil(u(lif))).unwrap();
            prop_assert!(u(l.repaid_shares) >= exact_to_shares_up(morpho_assets, c.tb, c.bs), "fewer shares repaid than Morpho");
            prop_assert!(l.repaid_assets >= exact_to_assets_up(l.repaid_shares, c.tb, c.bs).unwrap());
        }
    }
}

#[test]
fn liquidation_matches_morphos_formulas_at_exact_claims() {
    // lltv 0.8 ⇒ LIF = 1 / (1 − 0.3·0.2) = 1.0638…; 1 collateral = 2 loan; rb = 1e12 (fresh market).
    let lltv = 8 * WAD / 10;
    let lif = liquidation_incentive_factor(lltv).unwrap();
    assert_eq!(lif, 1_063_829_787_234_042_553);
    let price = Interval::exact(U256::from(2u8) * PRICE_SCALE);
    let rb = Interval::exact(ratio_down(0, 0).unwrap());
    let claimed = Claimed { collateral: 1_000_000, borrow_shares: 1_000_000_000_000 }; // 1,000,000 debt
    let l = claims::liquidation(LiquidationAmount::RepayShares(500_000_000_000), &claimed, &price, &rb, lltv).unwrap();
    // 500,000 debt repaid × 1.0638 / 2 = 265,957 collateral.
    assert_eq!((l.repaid_assets, l.seized, l.bad_shares), (500_000, 265_957, 0));
    let full = claims::full_liquidation(&claimed, &price, &rb, lltv).unwrap();
    assert_eq!(full, LiquidationAmount::RepayShares(1_000_000_000_000), "collateral covers 1,000,000 × 1.0638 / 2");
}

/// Bad debt: a position whose collateral is all seized has its remaining debt written off against
/// the suppliers, so `rs` falls and the position ends empty.
#[test]
fn bad_debt_lowers_rs_and_zeroes_the_position() {
    let irm = irm();
    let lltv = 8 * WAD / 10;
    let mut m = MarketState::new(0);
    let rs = tight(0, 0);
    m.apply(&irm, 0, &MarketOp::Supply { rs, assets: 1_000_000, shares: claims::supply_shares(1_000_000, &rs).unwrap() }).unwrap();
    let rb = tight(0, 0);
    let shares = claims::borrow_shares(500_000, &rb).unwrap();
    m.apply(&irm, 0, &MarketOp::Borrow { rb, assets: 500_000, shares }).unwrap();
    let mut p = Position { collateral: 300_000, borrow_shares: shares, ..Position::default() };
    let rs_before = ratio_down(m.total_supply_assets, m.total_supply_shares).unwrap();

    // The price falls to 1: 300,000 collateral against 500,000 debt.
    let price = Interval::exact(PRICE_SCALE);
    let rb = tight(m.total_borrow_assets, m.total_borrow_shares);
    let claimed = Claimed { collateral: p.collateral, borrow_shares: p.borrow_shares };
    let amount = claims::full_liquidation(&claimed, &price, &rb, lltv).unwrap();
    assert_eq!(amount, LiquidationAmount::SeizeCollateral(300_000));
    let l = claims::liquidation(amount, &claimed, &price, &rb, lltv).unwrap();
    assert!(l.bad_shares > 0);
    let health = claims::liquidation_health(&price, &rb, lltv).unwrap();
    p.apply(&PositionOp::Liquidate { claimed, health, liquidation: l }).unwrap();
    let op = MarketOp::Liquidate { rb, repaid_shares: l.repaid_shares, repaid_assets: l.repaid_assets, bad_shares: l.bad_shares };
    m.apply(&irm, 0, &op).unwrap();

    assert_eq!((p.collateral, p.borrow_shares), (0, 0), "position zeroed");
    assert_eq!((m.total_borrow_assets, m.total_borrow_shares), (0, 0), "no debt left in the market");
    // 300,000 / 1.0638 = 282,000 repaid (+1 from rounding up twice); the other ~218,000 is lost by
    // the suppliers.
    assert_eq!(l.repaid_assets, 282_001);
    assert_eq!(m.total_supply_assets, 1_000_000 - (500_000 - 282_001));
    assert!(ratio_down(m.total_supply_assets, m.total_supply_shares).unwrap() < rs_before, "rs fell");
}
