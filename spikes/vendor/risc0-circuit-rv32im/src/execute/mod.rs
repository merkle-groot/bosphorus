// Copyright 2025 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pub(crate) mod bibc;
pub(crate) mod bigint;
mod executor;
pub mod gdb;
pub(crate) mod pager;
pub mod platform;
pub(crate) mod poseidon2;
pub(crate) mod r0vm;
pub(crate) mod rv32im;
pub(crate) mod segment;
pub(crate) mod sha2;
mod syscall;
#[cfg(test)]
mod tests;
pub mod testutil;

pub use self::{
    bigint::analyze::analyze as bigint_analyze,
    executor::{CycleLimit, EcallMetric, Executor, ExecutorResult, SimpleSession},
    platform::*,
    segment::Segment,
    syscall::{Syscall, SyscallContext},
};

/// Prover-side choice only: segments of any allowed po2 verify the same. Bosphorus builds pick it
/// at compile time with `R0_SEGMENT_PO2`, default 19 (risc0's is 20: an 8 GB Mac pages through a
/// po2-20 segment, and a po2-19 one needs ~4 GB of GPU memory).
pub const DEFAULT_SEGMENT_LIMIT_PO2: usize = match option_env!("R0_SEGMENT_PO2") {
    Some(po2) => parse_po2(po2.as_bytes()),
    None => 19,
};

const fn parse_po2(digits: &[u8]) -> usize {
    let mut value = 0;
    let mut i = 0;
    while i < digits.len() {
        assert!(digits[i].is_ascii_digit(), "R0_SEGMENT_PO2 must be a number");
        value = value * 10 + (digits[i] - b'0') as usize;
        i += 1;
    }
    value
}

pub(crate) fn node_idx(page_idx: u32) -> u32 {
    MEMORY_PAGES as u32 + page_idx
}

#[inline]
#[cold]
fn cold() {}

#[inline]
#[allow(dead_code)]
fn likely(b: bool) -> bool {
    if !b {
        cold()
    }
    b
}

#[inline]
fn unlikely(b: bool) -> bool {
    if b {
        cold()
    }
    b
}
