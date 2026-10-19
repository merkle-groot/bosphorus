//! Positions, health and what the position's `apply` does (design §3, §5).
//!
//! The position's `apply` owns the health invariant: it checks it at the bounds that the oracle's
//! and the market's applies verify in the same tx (constraints §6, step 4).

use borsh::{BorshDeserialize, BorshSerialize};

use crate::{
    Error, PRICE_SCALE, Result, U256, WAD,
    math::{div_down, div_up, mul_div_down, u256},
};

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Position {
    pub market_id: [u8; 32],
    pub supply_shares: u128,
    pub borrow_shares: u128,
    /// Collateral-asset base units.
    pub collateral: u128,
}

/// The conservative ends a health check uses: the lowest price and the highest borrow ratio the
/// tx's other applies have verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Health {
    /// Oracle price (1e36 scale), the claim's `lo`.
    pub price: U256,
    /// Borrow ratio (WAD), the claim's `hi`.
    pub rb: u128,
    pub lltv: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum PositionOp {
    Supply { shares: u128 },
    Withdraw { shares: u128 },
    SupplyCollateral { assets: u128 },
    /// `health: None` skips the oracle and the market, and requires the position to hold no debt.
    WithdrawCollateral { assets: u128, health: Option<Health> },
    Borrow { shares: u128, health: Health },
    Repay { shares: u128 },
}

impl Position {
    /// Applies `op`, or leaves the position untouched on error.
    pub fn apply(&mut self, op: &PositionOp) -> Result<()> {
        let mut next = self.clone();
        next.apply_in_place(op)?;
        *self = next;
        Ok(())
    }

    fn apply_in_place(&mut self, op: &PositionOp) -> Result<()> {
        let add = |x: u128, y: u128| x.checked_add(y).ok_or(Error::Overflow);
        match *op {
            PositionOp::Supply { shares } => self.supply_shares = add(self.supply_shares, shares)?,
            PositionOp::Withdraw { shares } => {
                self.supply_shares = self.supply_shares.checked_sub(shares).ok_or(Error::InsufficientShares)?;
            }
            PositionOp::SupplyCollateral { assets } => self.collateral = add(self.collateral, assets)?,
            PositionOp::WithdrawCollateral { assets, health } => {
                self.collateral = self.collateral.checked_sub(assets).ok_or(Error::InsufficientCollateral)?;
                match health {
                    Some(health) => self.check_health(&health)?,
                    None if self.borrow_shares != 0 => return Err(Error::Unhealthy),
                    None => {}
                }
            }
            PositionOp::Borrow { shares, health } => {
                self.borrow_shares = add(self.borrow_shares, shares)?;
                self.check_health(&health)?;
            }
            PositionOp::Repay { shares } => {
                self.borrow_shares = self.borrow_shares.checked_sub(shares).ok_or(Error::InsufficientShares)?;
            }
        }
        Ok(())
    }

    pub fn check_health(&self, health: &Health) -> Result<()> {
        if is_healthy(self.collateral, self.borrow_shares, health)? { Ok(()) } else { Err(Error::Unhealthy) }
    }
}

/// `⌈borrow_shares · rb⌉`: debt at a claimed ratio, rounded up.
pub fn debt(borrow_shares: u128, rb: u128) -> Result<U256> {
    div_up(u256(borrow_shares) * u256(rb), u256(WAD))
}

/// `⌊⌊collateral · price / 1e36⌋ · lltv⌋` (Morpho's `maxBorrow`), in 256 bits.
pub fn max_borrow(collateral: u128, price: U256, lltv: u128) -> Result<U256> {
    let value = u256(collateral).checked_mul(price).ok_or(Error::Overflow)?;
    let value = div_down(value, PRICE_SCALE)?;
    div_down(value.checked_mul(u256(lltv)).ok_or(Error::Overflow)?, u256(WAD))
}

/// Healthy iff `debt ≤ max_borrow` at the given bounds. A position without debt is always healthy.
pub fn is_healthy(collateral: u128, borrow_shares: u128, health: &Health) -> Result<bool> {
    if borrow_shares == 0 {
        return Ok(true);
    }
    Ok(debt(borrow_shares, health.rb)? <= max_borrow(collateral, health.price, health.lltv)?)
}

/// Morpho's liquidation incentive factor: `min(1.15, 1 / (1 − 0.3·(1 − lltv)))`, WAD.
pub fn liquidation_incentive_factor(lltv: u128) -> Result<u128> {
    const MAX_LIF: u128 = 1_150_000_000_000_000_000;
    const CURSOR: u128 = 300_000_000_000_000_000;
    let denominator = WAD - mul_div_down(CURSOR, WAD - lltv.min(WAD), WAD)?;
    Ok(mul_div_down(WAD, WAD, denominator)?.min(MAX_LIF))
}

/// `max_borrow / debt` (WAD) at the given bounds; `None` without debt.
pub fn health_factor(collateral: u128, borrow_shares: u128, health: &Health) -> Result<Option<U256>> {
    if borrow_shares == 0 {
        return Ok(None);
    }
    let max = max_borrow(collateral, health.price, health.lltv)?;
    Ok(Some(div_down(max.checked_mul(u256(WAD)).ok_or(Error::Overflow)?, debt(borrow_shares, health.rb)?)?))
}
