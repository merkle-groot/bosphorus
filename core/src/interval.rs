//! Claimed intervals and validity windows (the bounds pattern, constraints §6 and §4).

use borsh::{BorshDeserialize, BorshSerialize};

use crate::{Error, MAX_WINDOW_MS, Result};

/// A claim that a value living in another shard is in `[lo, hi]`. `plan` uses whichever end is
/// worse for the user; the shard's own `apply` asserts the real value is inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Interval<T> {
    pub lo: T,
    pub hi: T,
}

impl<T: PartialOrd + Copy> Interval<T> {
    pub fn new(lo: T, hi: T) -> Result<Self> {
        if lo > hi {
            return Err(Error::EmptyInterval);
        }
        Ok(Self { lo, hi })
    }

    pub const fn exact(x: T) -> Self {
        Self { lo: x, hi: x }
    }

    pub fn contains(&self, x: T) -> bool {
        self.lo <= x && x <= self.hi
    }

    pub fn validate(&self) -> Result<()> {
        if self.lo > self.hi { Err(Error::EmptyInterval) } else { Ok(()) }
    }
}

/// The claimed time `t` (ms) and the window `[t, t + w)` that `plan` sets. Settlement rejects the
/// tx outside the window, so block time is ≥ `t`: accruing to `t` never over-accrues.
///
/// `w` is capped: a backdated `t` lets a supplier or repayer skip up to `w` of interest (the market
/// accrues only to `t`), so the gain is bounded by `rate × w`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Window {
    pub t: u64,
    pub w: u64,
}

impl Window {
    /// Exclusive end, `t + w`.
    pub fn end(&self) -> Result<u64> {
        if self.w == 0 || self.w > MAX_WINDOW_MS {
            return Err(Error::BadWindow);
        }
        self.t.checked_add(self.w).ok_or(Error::BadWindow)
    }
}
