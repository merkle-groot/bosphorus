//! Market state and what the market's `apply` does with each operation (design §3, §5).

use borsh::{BorshDeserialize, BorshSerialize};

use crate::{
    Error, Interval, MAX_FEE, Result, WAD,
    irm::{IrmParams, utilization},
    math::{narrow, taylor3, u256, w_mul_down},
    shares::{check_ratio, exact_to_shares_down},
};

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct MarketState {
    pub total_supply_assets: u128,
    /// Includes `pending_fee_shares`.
    pub total_supply_shares: u128,
    pub total_borrow_assets: u128,
    pub total_borrow_shares: u128,
    /// ms.
    pub last_update: u64,
    /// WAD, ≤ [`MAX_FEE`].
    pub fee: u128,
    /// Fee shares accrued but not yet moved to the fee recipient's position (`claim_fees`).
    pub pending_fee_shares: u128,
}

/// What `plan` asks the market's `apply` to do, after accruing to the claimed `t`. Each carries
/// the claim it was derived from; the apply checks the claim, then applies the derived deltas.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum MarketOp {
    Supply { rs: Interval<u128>, assets: u128, shares: u128 },
    Withdraw { rs: Interval<u128>, shares: u128, assets: u128 },
    Borrow { rb: Interval<u128>, assets: u128, shares: u128 },
    Repay { rb: Interval<u128>, shares: u128, assets: u128 },
    /// Accrue and check `rb`, for a health check elsewhere (`withdraw_collateral`).
    Check { rb: Interval<u128> },
    SetFee { fee: u128 },
}

impl MarketState {
    pub fn new(t: u64) -> Self {
        Self { last_update: t, ..Self::default() }
    }

    /// Accrues interest up to `t` (ms) and returns it. A no-op if `t ≤ last_update`.
    pub fn accrue(&mut self, irm: &IrmParams, t: u64) -> Result<u128> {
        if t <= self.last_update {
            return Ok(0);
        }
        let dt = t - self.last_update;
        self.last_update = t;
        if self.total_borrow_assets == 0 {
            return Ok(0);
        }
        let rate = irm.rate(utilization(self.total_borrow_assets, self.total_supply_assets)?)?;
        // rate is per second, dt in ms.
        let x = u256(rate) * crate::U256::from(dt) / crate::U256::from(1000u16);
        let interest = narrow(u256(self.total_borrow_assets) * taylor3(x) / u256(WAD))?;
        self.total_borrow_assets = self.total_borrow_assets.checked_add(interest).ok_or(Error::Overflow)?;
        self.total_supply_assets = self.total_supply_assets.checked_add(interest).ok_or(Error::Overflow)?;
        if self.fee != 0 {
            let fee_amount = w_mul_down(interest, self.fee)?;
            let fee_shares =
                exact_to_shares_down(fee_amount, self.total_supply_assets - fee_amount, self.total_supply_shares)?;
            self.total_supply_shares = self.total_supply_shares.checked_add(fee_shares).ok_or(Error::Overflow)?;
            self.pending_fee_shares += fee_shares;
        }
        Ok(interest)
    }

    /// The market's `apply`: accrue to `t`, check the op's claim against the accrued ratio, then
    /// apply its deltas. Leaves the state untouched on error.
    pub fn apply(&mut self, irm: &IrmParams, t: u64, op: &MarketOp) -> Result<()> {
        let mut next = self.clone();
        next.apply_in_place(irm, t, op)?;
        *self = next;
        Ok(())
    }

    fn apply_in_place(&mut self, irm: &IrmParams, t: u64, op: &MarketOp) -> Result<()> {
        self.accrue(irm, t)?;
        let add = |x: u128, y: u128| x.checked_add(y).ok_or(Error::Overflow);
        let sub = |x: u128, y: u128, e: Error| x.checked_sub(y).ok_or(e);
        match *op {
            MarketOp::Supply { rs, assets, shares } => {
                check_ratio(&rs, self.total_supply_assets, self.total_supply_shares)?;
                self.total_supply_assets = add(self.total_supply_assets, assets)?;
                self.total_supply_shares = add(self.total_supply_shares, shares)?;
            }
            MarketOp::Withdraw { rs, shares, assets } => {
                check_ratio(&rs, self.total_supply_assets, self.total_supply_shares)?;
                self.total_supply_shares = sub(self.total_supply_shares, shares, Error::InsufficientShares)?;
                self.total_supply_assets = sub(self.total_supply_assets, assets, Error::InsufficientLiquidity)?;
                self.check_liquidity()?;
            }
            MarketOp::Borrow { rb, assets, shares } => {
                check_ratio(&rb, self.total_borrow_assets, self.total_borrow_shares)?;
                self.total_borrow_assets = add(self.total_borrow_assets, assets)?;
                self.total_borrow_shares = add(self.total_borrow_shares, shares)?;
                self.check_liquidity()?;
            }
            MarketOp::Repay { rb, shares, assets } => {
                check_ratio(&rb, self.total_borrow_assets, self.total_borrow_shares)?;
                self.total_borrow_shares = sub(self.total_borrow_shares, shares, Error::InsufficientShares)?;
                // Rounding (and the claim's width) can make `assets` exceed what's left.
                self.total_borrow_assets = self.total_borrow_assets.saturating_sub(assets);
            }
            MarketOp::Check { rb } => check_ratio(&rb, self.total_borrow_assets, self.total_borrow_shares)?,
            MarketOp::SetFee { fee } => {
                if fee > MAX_FEE {
                    return Err(Error::FeeTooHigh);
                }
                self.fee = fee;
            }
        }
        Ok(())
    }

    fn check_liquidity(&self) -> Result<()> {
        if self.total_borrow_assets > self.total_supply_assets {
            return Err(Error::InsufficientLiquidity);
        }
        Ok(())
    }
}
