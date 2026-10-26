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

// MODIFIED from risc0-circuit-rv32im 4.0.4 (2026-06-11): adds the hybrid
// Metal proving lane for Apple Silicon, selected at runtime behind a GPU
// capability probe (Tier-2 argument buffers) with a loud CPU fallback, and
// exports `metal_lane_selected()` as the single source of truth for lane
// reporting. See repository NOTICE and patches/.

mod hal;
#[cfg(test)]
mod tests;
mod witgen;

use anyhow::Result;
use cfg_if::cfg_if;
use risc0_core::scope;

use crate::execute::segment::Segment;

pub use witgen::PreflightResults;

const GLOBAL_MIX: usize = 0;
const GLOBAL_OUT: usize = 1;

pub type Seal = Vec<u32>;

pub trait SegmentProver {
    fn prove(&self, segment: &Segment) -> Result<Seal> {
        scope!("prove");
        let results = self.preflight(segment)?;
        self.prove_core(results)
    }

    fn preflight(&self, segment: &Segment) -> Result<PreflightResults>;

    fn prove_core(&self, preflight_results: PreflightResults) -> Result<Seal>;
}

/// True when `R0_DISABLE_METAL` is set to a non-empty value other than "0".
#[cfg(all(
    feature = "prove",
    not(feature = "cuda"),
    target_os = "macos",
    target_arch = "aarch64"
))]
fn metal_disabled_by_env() -> bool {
    std::env::var("R0_DISABLE_METAL").is_ok_and(|v| v != "0" && !v.is_empty())
}

/// Runtime probe for the two *host-variable* preconditions `MetalHal::new()`
/// asserts: the presence of a system Metal device and Tier-2 argument-buffer
/// support. (The remaining `new()` preconditions — embedded-metallib load and
/// per-kernel lookup — are validated at build time and only fail under driver
/// or OS corruption, so they are not probed here.) Probing turns what would
/// otherwise be a panic on hosts without a suitable GPU (VMs, hosted CI
/// runners) into an observable CPU fallback.
#[cfg(all(
    feature = "prove",
    not(feature = "cuda"),
    target_os = "macos",
    target_arch = "aarch64"
))]
fn metal_runtime_available() -> bool {
    match metal::Device::system_default() {
        Some(device) => device.argument_buffers_support() == metal::MTLArgumentBuffersTier::Tier2,
        None => false,
    }
}

/// Single source of truth for hybrid-lane selection: compile target, the
/// `R0_DISABLE_METAL` opt-out, and the runtime GPU capability probe. Hosts
/// that want to report the active lane should call this instead of
/// re-deriving it from the environment.
pub fn metal_lane_selected() -> bool {
    cfg_if! {
        if #[cfg(all(
            feature = "prove",
            not(feature = "cuda"),
            target_os = "macos",
            target_arch = "aarch64"
        ))] {
            if metal_disabled_by_env() {
                false
            } else if !metal_runtime_available() {
                // Surface the fallback once (not per segment). Emit to BOTH
                // tracing (for structured-log consumers) and stderr, because
                // the common `EnvFilter::from_default_env()` host setup
                // defaults to the ERROR level and would otherwise swallow a
                // warn-level event — the operator must always learn why the
                // GPU lane was skipped.
                static FALLBACK_WARNED: std::sync::Once = std::sync::Once::new();
                FALLBACK_WARNED.call_once(|| {
                    let msg = "risc0-metal-hybrid: no Tier-2 Metal GPU on this host; \
                               falling back to the CPU proving lane";
                    tracing::warn!("{msg}");
                    eprintln!("{msg}");
                });
                false
            } else {
                true
            }
        } else {
            false
        }
    }
}

/// Reset the per-phase profile counters (see [`phase_profile_ns`]).
pub fn phase_profile_reset() {
    use std::sync::atomic::Ordering;
    self::hal::PROFILE_WITGEN_NS.store(0, Ordering::Relaxed);
    self::hal::PROFILE_ACCUM_NS.store(0, Ordering::Relaxed);
    self::hal::PROFILE_EVALCHECK_NS.store(0, Ordering::Relaxed);
}

/// Wall-time in nanoseconds spent in the three circuit-specific CPU kernels --
/// `[witgen, accumulate, eval_check]` -- during proofs run with the `R0_PROFILE`
/// environment variable set, accumulated since the last [`phase_profile_reset`].
///
/// These kernels run on the CPU in both lanes (the hybrid moves only the
/// generic STARK ops to the GPU), so their sum is the proof's Amdahl floor.
/// Both the CPU HAL and the Metal HAL feed these counters, so the floor can be
/// measured directly on either lane.
pub fn phase_profile_ns() -> [u64; 3] {
    use std::sync::atomic::Ordering;
    [
        self::hal::PROFILE_WITGEN_NS.load(Ordering::Relaxed),
        self::hal::PROFILE_ACCUM_NS.load(Ordering::Relaxed),
        self::hal::PROFILE_EVALCHECK_NS.load(Ordering::Relaxed),
    ]
}

pub fn segment_prover() -> Result<Box<dyn SegmentProver>> {
    cfg_if! {
        if #[cfg(feature = "cuda")] {
            self::hal::cuda::segment_prover()
        } else if #[cfg(all(feature = "prove", target_os = "macos", target_arch = "aarch64"))] {
            // Apple Silicon: generic STARK ops on the Metal GPU, circuit kernels
            // on CPU. Opt out with R0_DISABLE_METAL=1. Hosts without a Tier-2
            // Metal GPU fall back to the CPU lane (with a one-time warning)
            // instead of panicking inside MetalHal::new().
            if metal_lane_selected() {
                self::hal::metal::segment_prover()
            } else {
                self::hal::cpu::segment_prover()
            }
        } else {
            self::hal::cpu::segment_prover()
        }
    }
}
