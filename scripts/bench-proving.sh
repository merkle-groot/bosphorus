#!/usr/bin/env bash
# Proving benchmark: proves S4's private tx (spike-harness's `s2_private`) once, on the GPU if this
# machine has a usable one, else on the CPU, and reports the real wall-clock proving time next to the
# machine it ran on.
#
# GPU: Apple Silicon via the Metal-hybrid spike (spikes/vendor/README.md), or NVIDIA via risc0's own
# stock CUDA HAL (no vendored code involved — `--features cuda` just turns on risc0-zkvm/cuda, same as
# upstream risc0). GPU numbers are this-machine-only, not cross-laptop-comparable.
#
# CPU (no GPU detected, the GPU build or run failed, or BENCH_CPU=1): the portable path through
# `r0vm`, no Metal/CUDA build required, so numbers from different laptops are comparable. Also runs
# the portable CPU kernel rates (`cpu_kernel_bench`: NTT, Poseidon2 row hashing via risc0-zkp's
# reference CpuHal) as a cheap secondary data point.
#
#   scripts/bench-proving.sh               # GPU if available, else CPU
#   BENCH_CPU=1 scripts/bench-proving.sh   # CPU only (the cross-laptop number)
#
# Needs: RISC0_DEV_MODE unset (dev mode fakes the proof and times nothing); for the CPU path, the
# risc0 toolchain's `r0vm` on PATH (`rzup install r0vm 3.0.5`). On an M1 8-core/8 GB: ~1–3 min on
# Metal (incl. the first run's shader compile), ~10 min on CPU. Close the browser and other heavy apps
# first, they skew real-proof timings (docs/spikes/S4-proving.md).
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "${RISC0_DEV_MODE:-}" == "1" || "${RISC0_DEV_MODE:-}" == "true" ]]; then
  echo "RISC0_DEV_MODE is set: unset it first — dev-mode proofs are fake and time nothing." >&2
  exit 1
fi

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
if command -v r0vm > /dev/null; then
  echo "r0vm:  $(r0vm --version)"
fi
echo

# Each GPU path returns non-zero if its build or run fails, and the CPU path below takes over.
# (`set -e` doesn't apply inside a function called from `&&`, hence the explicit returns.)
metal() {
  echo "=== S4: one private tx, real proof on the GPU (Metal, Apple Silicon only — not cross-laptop-comparable) ==="
  echo "building s2_private with the Metal-hybrid in-process prover (spikes/vendor/README.md)..."
  R0_SEGMENT_PO2=19 R0_METAL_BUDGET=400 R0_METAL_STORE=16 \
    cargo build --release -p spike-harness --features prove --bin s2_private \
    || { echo "Metal build failed." >&2; return 1; }
  echo "(first run after a build also pays Metal's one-time shader-cache compile, ~30-130s)"
  env -u RISC0_DEV_MODE R0_METAL_FULL=1 RISC0_KECCAK_PO2=14 S4_ONLY_A=1 ./target/release/s2_private \
    || { echo "Metal run failed." >&2; return 1; }
}

cuda() {
  echo "=== S4: one private tx, real proof on the GPU (CUDA, NVIDIA only — not cross-laptop-comparable) ==="
  echo "building s2_private with risc0's stock CUDA HAL (no vendored Metal code involved)..."
  cargo build --release -p spike-harness --features cuda --bin s2_private \
    || { echo "CUDA build failed." >&2; return 1; }
  env -u RISC0_DEV_MODE RISC0_PROVER=local S4_ONLY_A=1 ./target/release/s2_private \
    || { echo "CUDA run failed." >&2; return 1; }
}

if [[ "${BENCH_CPU:-}" == "1" ]]; then
  echo "BENCH_CPU=1: skipping the GPU paths."
elif [[ "$(uname)" == Darwin && "$(uname -m)" == arm64 ]] && command -v xcrun > /dev/null \
     && xcrun -sdk macosx metal --version > /dev/null 2>&1; then
  metal && exit 0
  echo "falling back to the CPU path."
elif command -v nvidia-smi > /dev/null && command -v nvcc > /dev/null; then
  cuda && exit 0
  echo "falling back to the CPU path."
else
  echo "no usable GPU (Apple Silicon + Xcode Metal toolchain, or NVIDIA + CUDA toolkit): CPU only."
fi
echo

command -v r0vm > /dev/null || { echo "r0vm not found on PATH (rzup install r0vm 3.0.5)" >&2; exit 1; }
echo "building s2_private (release, portable path: no --features prove, real proofs via r0vm)..."
cargo build --release -p spike-harness --bin s2_private
echo
echo "=== S4: one private tx, real proof on the CPU via r0vm ==="
S4_ONLY_A=1 ./target/release/s2_private

echo
echo "building cpu_kernel_bench (release, portable: risc0-zkp's reference CpuHal, no GPU)..."
cargo build --release -p spike-harness --features cpu-bench --bin cpu_kernel_bench
echo
echo "=== CPU kernel rates (secondary: NTT + Poseidon2 row hashing) ==="
./target/release/cpu_kernel_bench
