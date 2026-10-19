//! Pure maths and types for Bosphorus, a lending market on LEZ: shares, accrual, IRM, health, LIF,
//! intervals.
//!
//! No `lee_core` types here (constraints §9): the programs' plan/apply glue decodes, calls into this
//! crate and encodes, so the actor-model migration only rewrites the glue.

pub mod claims;
pub mod interval;
pub mod irm;
pub mod market;
pub mod math;
pub mod position;
pub mod shares;

pub use interval::{Interval, Window};
pub use irm::IrmParams;
pub use market::{MarketOp, MarketState};
pub use position::{Health, Position, PositionOp};
pub use ruint::aliases::U256;

pub const WAD: u128 = 1_000_000_000_000_000_000;
/// Morpho's virtual offsets: an empty market starts at 1e-6 assets per share, which makes share
/// inflation by a tiny first deposit uneconomic. Totals are tracked internally, so donations to a
/// vault never move a ratio.
pub const VIRTUAL_SHARES: u128 = 1_000_000;
pub const VIRTUAL_ASSETS: u128 = 1;
/// Highest market fee: 25% of interest.
pub const MAX_FEE: u128 = WAD / 4;
/// Longest validity window a market op may claim (constraints §4): long enough for a private
/// proof (~10 min, S4), short enough to bound what a backdated `t` skips.
pub const MAX_WINDOW_MS: u64 = 30 * 60 * 1000;
pub const SECONDS_PER_YEAR: u64 = 365 * 24 * 3600;

/// Oracle prices are loan-asset base units per collateral-asset base unit × 1e36 (Morpho's
/// `ORACLE_PRICE_SCALE`). They need 256 bits: with equal decimals, a u128 would cap the price at ~340.
pub const PRICE_SCALE_DECIMALS: u32 = 36;
pub const PRICE_SCALE: U256 = U256::from_limbs([0xb34b9f1000000000, 0x00c097ce7bc90715, 0, 0]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Overflow,
    DivisionByZero,
    ZeroAmount,
    EmptyInterval,
    BadWindow,
    BadIrm,
    BadLltv,
    FeeTooHigh,
    RatioOutsideClaim,
    PriceOutsideClaim,
    StalePrice,
    InsufficientShares,
    InsufficientCollateral,
    InsufficientLiquidity,
    Unhealthy,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = match self {
            Self::Overflow => "arithmetic overflow",
            Self::DivisionByZero => "division by zero",
            Self::ZeroAmount => "zero amount",
            Self::EmptyInterval => "claimed interval has lo > hi",
            Self::BadWindow => "validity window must be 0 < w <= MAX_WINDOW_MS",
            Self::BadIrm => "invalid IRM parameters",
            Self::BadLltv => "LLTV must be in (0, 1)",
            Self::FeeTooHigh => "fee above MAX_FEE",
            Self::RatioOutsideClaim => "share ratio outside the claimed interval",
            Self::PriceOutsideClaim => "oracle price outside the claimed interval",
            Self::StalePrice => "oracle price too old for the validity window",
            Self::InsufficientShares => "not enough shares",
            Self::InsufficientCollateral => "not enough collateral",
            Self::InsufficientLiquidity => "not enough liquidity",
            Self::Unhealthy => "position would be unhealthy",
        };
        f.write_str(s)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = core::result::Result<T, Error>;

/// `0 < lltv < 1`.
pub fn validate_lltv(lltv: u128) -> Result<()> {
    if lltv == 0 || lltv >= WAD { Err(Error::BadLltv) } else { Ok(()) }
}

/// Parses a decimal fraction ("0.8", "12.5%") into WAD, rounding down.
pub fn parse_wad(s: &str) -> Option<u128> {
    let (s, percent) = match s.strip_suffix('%') {
        Some(s) => (s, true),
        None => (s, false),
    };
    // scale_price multiplies by 10^(36 + 0 − 18) = 10^18 (or 10^16 for a percentage).
    u128::try_from(scale_price(s, 0, if percent { 20 } else { 18 })?).ok()
}

/// Formats a WAD value as a decimal with `digits` fractional digits (rounded down).
pub fn format_wad(x: u128, digits: u32) -> String {
    let int = x / WAD;
    let frac = (x % WAD) / 10u128.pow(18 - digits.min(18));
    format!("{int}.{frac:0width$}", width = digits as usize)
}

pub fn scale_price(price: &str, loan_decimals: u8, collateral_decimals: u8) -> Option<U256> {
    let (int, frac) = price.split_once('.').unwrap_or((price, ""));
    if int.is_empty() && frac.is_empty() || !(int.chars().chain(frac.chars())).all(|c| c.is_ascii_digit()) {
        return None;
    }
    let digits = U256::from_str_radix(&format!("{int}{frac}"), 10).ok()?;
    // price × 10^(36 + loan − collateral) / 10^frac_len
    let up = PRICE_SCALE_DECIMALS + u32::from(loan_decimals);
    let down = u32::from(collateral_decimals) + u32::try_from(frac.len()).ok()?;
    let ten = U256::from(10u8);
    if up >= down {
        digits.checked_mul(ten.checked_pow(U256::from(up - down))?)
    } else {
        Some(digits / ten.checked_pow(U256::from(down - up))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_prices() {
        let e = |n: u32| U256::from(10u8).pow(U256::from(n));
        assert_eq!(scale_price("1", 6, 6), Some(e(36)));
        assert_eq!(scale_price("1000", 6, 6), Some(U256::from(1000u16) * e(36)), "beyond u128");
        assert_eq!(scale_price("2.5", 6, 18), Some(U256::from(25u8) * e(23)));
        assert_eq!(scale_price(".5", 0, 0), Some(U256::from(5u8) * e(35)));
        for bad in ["", ".", "1.2.3", "-1", "1e3", "abc"] {
            assert_eq!(scale_price(bad, 6, 6), None, "{bad}");
        }
        assert_eq!(PRICE_SCALE, e(36));
        assert_eq!(parse_wad("0.8"), Some(8 * WAD / 10));
        assert_eq!(parse_wad("12.5%"), Some(WAD / 8));
        assert_eq!(format_wad(WAD / 8, 4), "0.1250");
    }
}
