#!/usr/bin/env bash
# Cross-laptop proving benchmark: builds and runs S4's private tx (spike-harness's `s2_private`)
# through `r0vm` — the portable CPU path, no Metal/CUDA build required — and reports the real
# wall-clock proving time next to the machine it ran on, so numbers from different laptops are
# comparable. Also runs the portable CPU kernel rates (`cpu_kernel_bench`: NTT, Poseidon2 row
# hashing via risc0-zkp's reference CpuHal) as a cheap secondary data point.
#
# If this machine has a usable GPU, two more (this-machine-only, not cross-laptop-comparable)
# numbers follow: Apple Silicon via the Metal-hybrid spike (spikes/vendor/README.md), or NVIDIA
# via risc0's own stock CUDA HAL (no vendored code involved — `--features cuda` just turns on
# risc0-zkvm/cuda, same as upstream risc0). Either is skipped with a one-line note if the matching
# hardware/toolchain isn't detected; a GPU build or run failing doesn't abort the portable numbers
# above it.
#
#   scripts/bench-proving.sh
#
# Needs: the risc0 toolchain's `r0vm` on PATH (`rzup install r0vm 3.0.5`), RISC0_DEV_MODE unset
# (dev mode fakes the proof and times nothing). ~10 min on an M1 8-core/8 GB; close the browser
# and other heavy apps first, they skew real-proof timings (docs/spikes/S4-proving.md).
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "${RISC0_DEV_MODE:-}" == "1" || "${RISC0_DEV_MODE:-}" == "true" ]]; then
  echo "RISC0_DEV_MODE is set: unset it first — dev-mode proofs are fake and time nothing." >&2
  exit 1
fi
command -v r0vm > /dev/null || { echo "r0vm not found on PATH (rzup install r0vm 3.0.5)" >&2; exit 1; }

echo "=== machine ==="
echo "host:  $(hostname)"
echo "date:  $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "os:    $(uname -srm)"
if [[ "$(uname)" == Darwin ]]; then
  echo "cpu:   $(sysctl -n machdep.cpu.brand_string) ($(sysctl -n hw.physicalcpu) cores, $(sysctl -n hw.logicalcpu) threads)"
  echo "ram:   $(($(sysctl -n hw.memsize) / 1024 / 1024 / 1024)) GB"
else
  echo "cpu:   $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | sed 's/^ *//') ($(nproc) threads)"
  echo "ram:   $(($(grep MemTotal /proc/meminfo | awk '{print $2}') / 1024 / 1024)) GB"
fi
echo "rustc: $(rustc --version)"
echo "r0vm:  $(r0vm --version)"
echo

echo "building s2_private (release, portable path: no --features prove, real proofs via r0vm)..."
cargo build --release -p spike-harness --bin s2_private
echo
echo "=== S4: one private tx, real proof via r0vm ==="
S4_ONLY_A=1 ./target/release/s2_private

echo
echo "building cpu_kernel_bench (release, portable: risc0-zkp's reference CpuHal, no GPU)..."
cargo build --release -p spike-harness --features cpu-bench --bin cpu_kernel_bench
echo
echo "=== CPU kernel rates (secondary: NTT + Poseidon2 row hashing) ==="
./target/release/cpu_kernel_bench

echo
if [[ "$(uname)" == Darwin && "$(uname -m)" == arm64 ]] && command -v xcrun > /dev/null \
   && xcrun -sdk macosx metal --version > /dev/null 2>&1; then
  echo "=== GPU (Metal, Apple Silicon only — not cross-laptop-comparable) ==="
  echo "building s2_private with the Metal-hybrid in-process prover (spikes/vendor/README.md)..."
  if R0_SEGMENT_PO2=19 R0_METAL_BUDGET=400 R0_METAL_STORE=16 \
       cargo build --release -p spike-harness --features prove --bin s2_private; then
    echo "(first run after a build also pays Metal's one-time shader-cache compile, ~30-130s)"
    if env -u RISC0_DEV_MODE R0_METAL_FULL=1 RISC0_KECCAK_PO2=14 S4_ONLY_A=1 ./target/release/s2_private; then
      :
    else
      echo "Metal run failed — skipping (R0_DISABLE_METAL=1 falls back to CPU if this errors oddly)." >&2
    fi
  else
    echo "Metal build failed — skipping." >&2
  fi
else
  echo "no Apple Silicon + Xcode Metal toolchain detected: skipping the Metal GPU path."
fi

echo
if command -v nvidia-smi > /dev/null && command -v nvcc > /dev/null; then
  echo "=== GPU (CUDA, NVIDIA only — not cross-laptop-comparable) ==="
  echo "building s2_private with risc0's stock CUDA HAL (no vendored Metal code involved)..."
  if cargo build --release -p spike-harness --features cuda --bin s2_private; then
    if env -u RISC0_DEV_MODE RISC0_PROVER=local S4_ONLY_A=1 ./target/release/s2_private; then
      :
    else
      echo "CUDA run failed — skipping." >&2
    fi
  else
    echo "CUDA build failed — skipping." >&2
  fi
else
  echo "no NVIDIA GPU + CUDA toolkit (nvidia-smi, nvcc) detected: skipping the CUDA GPU path."
fi
