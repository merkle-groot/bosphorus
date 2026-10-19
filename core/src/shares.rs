//! Assets ↔ shares, with Morpho's virtual offsets (design §2.3, §3).
//!
//! The ratio `r = (assets + VIRTUAL_ASSETS) / (shares + VIRTUAL_SHARES)` lives in the market's shard,
//! so `plan` works from a claimed WAD interval for it and the market's `apply` checks the claim
//! **exactly** (cross-multiplied, no rounding). Each conversion then rounds in the protocol's favour
//! from the end of the interval chosen in [`crate::claims`].

use crate::{
    Error, Interval, Result, U256, VIRTUAL_ASSETS, VIRTUAL_SHARES, WAD,
    math::{div_down, div_up, mul_div_down, mul_div_up, narrow, u256},
};

fn offsets(total_assets: u128, total_shares: u128) -> (U256, U256) {
    (u256(total_assets) + u256(VIRTUAL_ASSETS), u256(total_shares) + u256(VIRTUAL_SHARES))
}

/// `⌊r·WAD⌋`: what a client starts its claim from.
pub fn ratio_down(total_assets: u128, total_shares: u128) -> Result<u128> {
    let (a, s) = offsets(total_assets, total_shares);
    narrow(div_down(a * u256(WAD), s)?)
}

/// `⌈r·WAD⌉`.
pub fn ratio_up(total_assets: u128, total_shares: u128) -> Result<u128> {
    let (a, s) = offsets(total_assets, total_shares);
    narrow(div_up(a * u256(WAD), s)?)
}

/// `claim.lo ≤ r·WAD ≤ claim.hi`, compared exactly.
pub fn check_ratio(claim: &Interval<u128>, total_assets: u128, total_shares: u128) -> Result<()> {
    let (a, s) = offsets(total_assets, total_shares);
    let r = a * u256(WAD);
    if u256(claim.lo) * s > r || r > u256(claim.hi) * s {
        return Err(Error::RatioOutsideClaim);
    }
    Ok(())
}

/// `⌊assets / r⌋` for a claimed ratio `r` (WAD).
pub fn to_shares_down(assets: u128, r: u128) -> Result<u128> {
    mul_div_down(assets, WAD, r)
}

/// `⌈assets / r⌉`.
pub fn to_shares_up(assets: u128, r: u128) -> Result<u128> {
    mul_div_up(assets, WAD, r)
}

/// `⌊shares · r⌋`.
pub fn to_assets_down(shares: u128, r: u128) -> Result<u128> {
    mul_div_down(shares, r, WAD)
}

/// `⌈shares · r⌉`.
pub fn to_assets_up(shares: u128, r: u128) -> Result<u128> {
    mul_div_up(shares, r, WAD)
}

/// Morpho's `toSharesDown` against real totals (used where the totals are in hand: fee shares).
pub fn exact_to_shares_down(assets: u128, total_assets: u128, total_shares: u128) -> Result<u128> {
    let (a, s) = offsets(total_assets, total_shares);
    narrow(div_down(u256(assets) * s, a)?)
}

/// Morpho's `toAssetsUp` against real totals.
pub fn exact_to_assets_up(shares: u128, total_assets: u128, total_shares: u128) -> Result<u128> {
    let (a, s) = offsets(total_assets, total_shares);
    narrow(div_up(u256(shares) * a, s)?)
}

/// Morpho's `toAssetsDown` against real totals.
pub fn exact_to_assets_down(shares: u128, total_assets: u128, total_shares: u128) -> Result<u128> {
    let (a, s) = offsets(total_assets, total_shares);
    narrow(div_down(u256(shares) * a, s)?)
}
