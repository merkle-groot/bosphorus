//! Interest-rate models. Rates are WAD **per second** (S3: per-ms rates lose precision).

use borsh::{BorshDeserialize, BorshSerialize};

use crate::{Error, Result, SECONDS_PER_YEAR, WAD, math::mul_div_down};

/// Highest borrow rate any IRM may return: 10,000% APR.
pub const MAX_RATE_PER_SECOND: u128 = 100 * WAD / SECONDS_PER_YEAR as u128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum IrmParams {
    /// Aave-style: `base + slope1·U/u_opt` up to the kink, then `+ slope2·(U − u_opt)/(1 − u_opt)`.
    /// All WAD; `base`/`slope*` per second.
    LinearKink { base: u128, slope1: u128, slope2: u128, u_opt: u128 },
}

impl IrmParams {
    /// From yearly rates (WAD per year), rounding each per-second rate down.
    pub fn linear_kink_apr(base: u128, slope1: u128, slope2: u128, u_opt: u128) -> Self {
        let per_second = |x: u128| x / SECONDS_PER_YEAR as u128;
        Self::LinearKink { base: per_second(base), slope1: per_second(slope1), slope2: per_second(slope2), u_opt }
    }

    pub fn validate(&self) -> Result<()> {
        let Self::LinearKink { base, slope1, slope2, u_opt } = *self;
        let max = base.checked_add(slope1).and_then(|x| x.checked_add(slope2)).ok_or(Error::BadIrm)?;
        if u_opt == 0 || u_opt >= WAD || max > MAX_RATE_PER_SECOND {
            return Err(Error::BadIrm);
        }
        Ok(())
    }

    /// Borrow rate at utilisation `u` (WAD, clamped to 1).
    pub fn rate(&self, u: u128) -> Result<u128> {
        let Self::LinearKink { base, slope1, slope2, u_opt } = *self;
        let u = u.min(WAD);
        if u <= u_opt {
            Ok(base + mul_div_down(slope1, u, u_opt)?)
        } else {
            Ok(base + slope1 + mul_div_down(slope2, u - u_opt, WAD - u_opt)?)
        }
    }
}

/// `TBA / TSA` in WAD (0 on an empty market).
pub fn utilization(total_borrow_assets: u128, total_supply_assets: u128) -> Result<u128> {
    if total_supply_assets == 0 {
        return Ok(0);
    }
    Ok(mul_div_down(total_borrow_assets, WAD, total_supply_assets)?.min(WAD))
}

/// Per-second rate → APR (WAD, simple, not compounded).
pub fn apr(rate_per_second: u128) -> u128 {
    rate_per_second.saturating_mul(SECONDS_PER_YEAR as u128)
}
