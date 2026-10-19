//! Interval selection: which end of each claim feeds each calculation (constraints §6, design §5).
//!
//! `plan` derives every amount from the end that is worse for the user, so whatever the real value
//! inside the claim, the protocol is never short:
//!
//! | op | derives | from | because the real ratio `r` satisfies |
//! |---|---|---|---|
//! | supply | shares ⌊assets / rs⌋ | `rs.hi` | `r ≤ hi` ⇒ fewer shares than `assets / r` |
//! | withdraw | assets ⌊shares · rs⌋ | `rs.lo` | `r ≥ lo` ⇒ fewer assets than `shares · r` |
//! | borrow | shares ⌈assets / rb⌉ | `rb.lo` | `r ≥ lo` ⇒ more debt shares than `assets / r` |
//! | repay | assets ⌈shares · rb⌉ | `rb.hi` | `r ≤ hi` ⇒ pays at least `shares · r` |
//! | health | debt at `rb.hi`, collateral at `price.lo` | both | debt only falls and value only rises inside |
//! | liquidate: unhealthy | debt at `rb.lo`, collateral at `price.hi` | both | unhealthy at the borrower's best ends ⇒ unhealthy |
//! | liquidate: seized | ⌊⌊S·rb⌋·LIF / p⌋ | `rb.lo`, `price.hi` | the liquidator gets no more than Morpho would give |
//! | liquidate: repaid | ⌈S·rb⌉ (and S from a seize: ⌈c·p / LIF / rb⌉) | `rb.hi` (`price.hi`, `rb.lo`) | the liquidator pays no less |
//!
//! The client side ([`ratio_claim`], [`price_claim`]) builds the intervals from state it has read.

use borsh::{BorshDeserialize, BorshSerialize};

use crate::{
    Error, Interval, PRICE_SCALE, Result, U256, WAD,
    irm::IrmParams,
    market::MarketState,
    math::{div_down, div_up, mul_div_down, mul_div_up, narrow, u256, w_mul_down},
    position::{Claimed, Health, liquidation_incentive_factor},
    shares::{ratio_down, ratio_up, to_assets_down, to_assets_up, to_shares_down, to_shares_up},
};

fn nonzero(x: u128) -> Result<u128> {
    if x == 0 { Err(Error::ZeroAmount) } else { Ok(x) }
}

pub fn supply_shares(assets: u128, rs: &Interval<u128>) -> Result<u128> {
    rs.validate()?;
    nonzero(to_shares_down(nonzero(assets)?, rs.hi)?)
}

pub fn withdraw_assets(shares: u128, rs: &Interval<u128>) -> Result<u128> {
    rs.validate()?;
    nonzero(to_assets_down(nonzero(shares)?, rs.lo)?)
}

pub fn borrow_shares(assets: u128, rb: &Interval<u128>) -> Result<u128> {
    rb.validate()?;
    to_shares_up(nonzero(assets)?, rb.lo)
}

pub fn repay_assets(shares: u128, rb: &Interval<u128>) -> Result<u128> {
    rb.validate()?;
    to_assets_up(nonzero(shares)?, rb.hi)
}

pub fn health(price: &Interval<U256>, rb: &Interval<u128>, lltv: u128) -> Result<Health> {
    price.validate()?;
    rb.validate()?;
    Ok(Health { price: price.lo, rb: rb.hi, lltv })
}

/// What the oracle feed's inspect checks: the price is inside the claim and still fresh at the
/// window's end (so it is never older than `max_age` whenever the tx lands).
pub fn check_price(claim: &Interval<U256>, price: U256, updated_at: u64, max_age: u64, window_end: u64) -> Result<()> {
    if !claim.contains(price) {
        return Err(Error::PriceOutsideClaim);
    }
    if updated_at.saturating_add(max_age) < window_end {
        return Err(Error::StalePrice);
    }
    Ok(())
}

/// How a liquidation is sized (Morpho: exactly one of `seizedAssets` / `repaidShares`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum LiquidationAmount {
    RepayShares(u128),
    SeizeCollateral(u128),
}

/// What a liquidation moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Liquidation {
    pub repaid_shares: u128,
    /// Paid by the liquidator.
    pub repaid_assets: u128,
    /// Collateral to the liquidator.
    pub seized: u128,
    /// Debt shares left on a position whose collateral is all seized: written off (bad debt).
    pub bad_shares: u128,
}

/// Derives a liquidation from the claims, at the ends that favour the borrower and the protocol
/// over the liquidator (Morpho's formulas otherwise).
pub fn liquidation(
    amount: LiquidationAmount,
    claimed: &Claimed,
    price: &Interval<U256>,
    rb: &Interval<u128>,
    lltv: u128,
) -> Result<Liquidation> {
    price.validate()?;
    rb.validate()?;
    let lif = liquidation_incentive_factor(lltv)?;
    let (repaid_shares, seized) = match amount {
        LiquidationAmount::RepayShares(shares) => {
            let value = w_mul_down(to_assets_down(nonzero(shares)?, rb.lo)?, lif)?;
            (shares, narrow(div_down(u256(value) * PRICE_SCALE, price.hi)?)?)
        }
        LiquidationAmount::SeizeCollateral(collateral) => {
            let quoted = u256(nonzero(collateral)?).checked_mul(price.hi).ok_or(Error::Overflow)?;
            let quoted = div_up(quoted, PRICE_SCALE)?;
            let assets = narrow(div_up(quoted.checked_mul(u256(WAD)).ok_or(Error::Overflow)?, u256(lif))?)?;
            (to_shares_up(assets, rb.lo)?, collateral)
        }
    };
    if repaid_shares > claimed.borrow_shares || seized > claimed.collateral {
        return Err(Error::BadLiquidation);
    }
    let bad_shares = if seized == claimed.collateral { claimed.borrow_shares - repaid_shares } else { 0 };
    Ok(Liquidation { repaid_shares, repaid_assets: to_assets_up(repaid_shares, rb.hi)?, seized, bad_shares })
}

/// The bounds a liquidation's unhealthiness is checked at: the price's `hi`, the ratio's `lo`.
pub fn liquidation_health(price: &Interval<U256>, rb: &Interval<u128>, lltv: u128) -> Result<Health> {
    price.validate()?;
    rb.validate()?;
    Ok(Health { price: price.hi, rb: rb.lo, lltv })
}

// ---- client side ---------------------------------------------------------------------------

/// Closes a position: repays all its debt if the collateral covers the incentive, otherwise seizes
/// all the collateral (and the rest of the debt is bad debt).
pub fn full_liquidation(claimed: &Claimed, price: &Interval<U256>, rb: &Interval<u128>, lltv: u128) -> Result<LiquidationAmount> {
    let repay_all = LiquidationAmount::RepayShares(claimed.borrow_shares);
    match liquidation(repay_all, claimed, price, rb, lltv) {
        Ok(_) => Ok(repay_all),
        Err(Error::BadLiquidation) => {
            let seize_all = LiquidationAmount::SeizeCollateral(claimed.collateral);
            liquidation(seize_all, claimed, price, rb, lltv).map(|_| seize_all)
        }
        Err(e) => Err(e),
    }
}

/// The market as its `apply` will see it after accruing to `t` (assuming nothing else lands first).
pub fn project(state: &MarketState, irm: &IrmParams, t: u64) -> Result<MarketState> {
    let mut state = state.clone();
    state.accrue(irm, t)?;
    Ok(state)
}

/// Supply and borrow ratio claims for a tx claiming time `t`: from the ratios accrued to `t`, up to
/// the ratios accrued to `t + ahead` (other txs may accrue further before ours lands), widened by
/// `width` (WAD fraction) on each side for rounding and IRM moves.
pub fn ratio_claims(
    state: &MarketState,
    irm: &IrmParams,
    t: u64,
    ahead: u64,
    width: u128,
) -> Result<(Interval<u128>, Interval<u128>)> {
    let now = project(state, irm, t)?;
    let later = project(state, irm, t.saturating_add(ahead))?;
    let rs = ratio_claim(
        ratio_down(now.total_supply_assets, now.total_supply_shares)?,
        ratio_up(later.total_supply_assets, later.total_supply_shares)?,
        width,
    )?;
    let rb = ratio_claim(
        ratio_down(now.total_borrow_assets, now.total_borrow_shares)?,
        ratio_up(later.total_borrow_assets, later.total_borrow_shares)?,
        width,
    )?;
    Ok((rs, rb))
}

/// `[⌊lo·(1 − width)⌋, ⌈hi·(1 + width)⌉]`.
pub fn ratio_claim(lo: u128, hi: u128, width: u128) -> Result<Interval<u128>> {
    let lo = mul_div_down(lo, WAD - width.min(WAD), WAD)?;
    let hi = mul_div_up(hi, WAD + width, WAD)?;
    Interval::new(lo, hi)
}

/// `[⌊p·(1 − width)⌋, ⌈p·(1 + width)⌉]`.
pub fn price_claim(price: U256, width: u128) -> Result<Interval<U256>> {
    let wad = u256(WAD);
    let lo = div_down(price.checked_mul(wad - u256(width.min(WAD))).ok_or(Error::Overflow)?, wad)?;
    let hi = div_up(price.checked_mul(wad + u256(width)).ok_or(Error::Overflow)?, wad)?;
    Interval::new(lo, hi)
}
