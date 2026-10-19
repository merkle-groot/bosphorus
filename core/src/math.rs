//! Fixed-point helpers. Every product goes through 256 bits; every quotient states its rounding.

use crate::{Error, Result, U256, WAD};

pub(crate) fn u256(x: u128) -> U256 {
    U256::from(x)
}

/// `U256 → u128`, or `Overflow`.
pub fn narrow(x: U256) -> Result<u128> {
    u128::try_from(x).map_err(|_| Error::Overflow)
}

pub fn div_down(n: U256, d: U256) -> Result<U256> {
    if d.is_zero() {
        return Err(Error::DivisionByZero);
    }
    Ok(n / d)
}

pub fn div_up(n: U256, d: U256) -> Result<U256> {
    if d.is_zero() {
        return Err(Error::DivisionByZero);
    }
    Ok(n.div_ceil(d))
}

/// `⌊x·y / d⌋`.
pub fn mul_div_down(x: u128, y: u128, d: u128) -> Result<u128> {
    narrow(div_down(u256(x) * u256(y), u256(d))?)
}

/// `⌈x·y / d⌉`.
pub fn mul_div_up(x: u128, y: u128, d: u128) -> Result<u128> {
    narrow(div_up(u256(x) * u256(y), u256(d))?)
}

/// `⌊x·y / WAD⌋`.
pub fn w_mul_down(x: u128, y: u128) -> Result<u128> {
    mul_div_down(x, y, WAD)
}

/// `e^(x) − 1` to three Taylor terms (Morpho's `wTaylorCompounded`), `x` in WAD. A lower bound for
/// every `x ≥ 0`, so compounding this way never over-accrues.
pub fn taylor3(x: U256) -> U256 {
    let wad = u256(WAD);
    let first = x;
    let second = first * first / (wad * U256::from(2u8));
    let third = second * first / (wad * U256::from(3u8));
    first + second + third
}
