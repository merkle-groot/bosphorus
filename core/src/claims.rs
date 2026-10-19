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
//!
//! The client side ([`ratio_claim`], [`price_claim`]) builds the intervals from state it has read.

use crate::{
    Error, Interval, Result, U256, WAD,
    irm::IrmParams,
    market::MarketState,
    math::{div_down, div_up, mul_div_down, mul_div_up, u256},
    position::Health,
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

// ---- client side ---------------------------------------------------------------------------

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
