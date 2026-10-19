//! Pure maths and types for Bosphorus, a lending market on LEZ: shares, accrual, IRM, health, LIF,
//! intervals.
//!
//! No `lee_core` types here (constraints §9). Filled in from Phase 2.

pub use ruint::aliases::U256;

pub const WAD: u128 = 1_000_000_000_000_000_000;

/// Oracle prices are loan-asset base units per collateral-asset base unit × 1e36 (Morpho's
/// `ORACLE_PRICE_SCALE`). They need 256 bits: with equal decimals, a u128 would cap the price at ~340.
pub const PRICE_SCALE_DECIMALS: u32 = 36;

/// Scales a human price ("1 collateral token = `price` loan tokens") to the oracle's fixed point,
/// rounding down. `None` on a malformed number or overflow.
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
    }
}
