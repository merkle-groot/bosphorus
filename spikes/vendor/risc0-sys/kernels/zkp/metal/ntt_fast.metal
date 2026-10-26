// Fused NTT passes for the Metal HAL (Bosphorus, Apache-2.0). Same butterflies, same twiddles
// and same order per element as multi_ntt_fwd_step / multi_ntt_rev_step in ntt.metal, so the
// output is bit-identical; only the grouping into memory passes changes.
//
// Stage s (1-based) pairs i and i + 2^(s-1) inside blocks of 2^s, twiddle root[s]^(i mod 2^(s-1)).
// Forward: a + b*w, a - b*w for s = 1..n.   Inverse: a + b, (a - b)*w for s = n..1.
//
// - *_low: stages 1..L of 2^L-element blocks, through threadgroup memory so global accesses are
//   coalesced. The forward one also expands its input (each coefficient repeated 2^E times, the
//   same values batch_expand writes, whose first E stages are no-ops); the inverse one also
//   multiplies by 1/N (eltwise_mul_factor_fp).
// - *_high: R consecutive stages s..s+R-1 in registers. Thread t takes the 2^R elements
//   b + j * 2^(s-1) where b has zeros in bits s-1 .. s+R-2; for s > L adjacent threads read
//   adjacent addresses. The twiddle of stage s+q at local j is root[s+q]^(b mod 2^(s-1)) times
//   root[q+1]^(j mod 2^q); `table` holds root[n]^x for x < N/2.

#include "fp.h"

using namespace metal;

constant uint32_t T = 64;  // threads per threadgroup in the *_low kernels

template <uint32_t E, uint32_t L>
inline void fwd_low(device Fp* out, const device Fp* in, const device Fp* lowtw, uint32_t nBits,
                    threadgroup uint32_t* tgIn, threadgroup uint32_t* tgOut, uint gid, uint lid) {
  constexpr uint32_t B = 1 << L;           // block (elements per thread)
  constexpr uint32_t K = 1 << (L - E);     // input coefficients per block
  uint32_t nSize = 1 << nBits;
  uint32_t blocksPerCol = nSize >> L;
  uint32_t c = gid / blocksPerCol;
  uint32_t blk0 = (gid % blocksPerCol) - lid;
  const device Fp* src = in + (size_t)c * (nSize >> E) + (size_t)blk0 * K;
  for (uint32_t idx = lid; idx < T * K; idx += T) {
    tgIn[(idx / K) * (K + 1) + idx % K] = src[idx].asRaw();
  }
  threadgroup_barrier(mem_flags::mem_threadgroup);
  Fp x[B];
  for (uint32_t j = 0; j < B; j++) {
    x[j] = Fp::fromRaw(tgIn[lid * (K + 1) + (j >> E)]);
  }
  for (uint32_t s = E + 1; s <= L; s++) {
    uint32_t hs = 1 << (s - 1);
    for (uint32_t jj = 0; jj < hs; jj++) {
      Fp w = lowtw[hs - 1 + jj];
      for (uint32_t j = jj; j < B; j += 2 * hs) {
        Fp a = x[j];
        Fp b = x[j + hs] * w;
        x[j] = a + b;
        x[j + hs] = a - b;
      }
    }
  }
  for (uint32_t j = 0; j < B; j++) {
    tgOut[lid * (B + 1) + j] = x[j].asRaw();
  }
  threadgroup_barrier(mem_flags::mem_threadgroup);
  device Fp* dst = out + (size_t)c * nSize + (size_t)blk0 * B;
  for (uint32_t idx = lid; idx < T * B; idx += T) {
    dst[idx] = Fp::fromRaw(tgOut[(idx >> L) * (B + 1) + (idx & (B - 1))]);
  }
}

#define FWD_LOW(E, L)                                                                           \
  kernel void ntt_fwd_low_##E##_##L(device Fp* out [[buffer(0)]],                               \
                                    const device Fp* in [[buffer(1)]],                          \
                                    const device Fp* lowtw [[buffer(2)]],                       \
                                    const device uint32_t& nBits [[buffer(3)]],                 \
                                    uint gid [[thread_position_in_grid]],                       \
                                    uint lid [[thread_position_in_threadgroup]]) {              \
    threadgroup uint32_t tgIn[T * ((1 << (L - E)) + 1)];                                        \
    threadgroup uint32_t tgOut[T * ((1 << L) + 1)];                                             \
    fwd_low<E, L>(out, in, lowtw, nBits, tgIn, tgOut, gid, lid);                                \
  }

FWD_LOW(0, 6)
FWD_LOW(1, 6)
FWD_LOW(2, 6)
FWD_LOW(3, 6)
FWD_LOW(4, 6)

kernel void ntt_rev_low_6(device Fp* io [[buffer(0)]],
                          const device Fp* lowtw [[buffer(1)]],
                          const device Fp* norm [[buffer(2)]],
                          const device uint32_t& nBits [[buffer(3)]],
                          uint gid [[thread_position_in_grid]],
                          uint lid [[thread_position_in_threadgroup]]) {
  constexpr uint32_t L = 6;
  constexpr uint32_t B = 1 << L;
  threadgroup uint32_t tg[T * (B + 1)];
  uint32_t nSize = 1 << nBits;
  uint32_t blocksPerCol = nSize >> L;
  uint32_t c = gid / blocksPerCol;
  uint32_t blk0 = (gid % blocksPerCol) - lid;
  device Fp* base = io + (size_t)c * nSize + (size_t)blk0 * B;
  for (uint32_t idx = lid; idx < T * B; idx += T) {
    tg[(idx >> L) * (B + 1) + (idx & (B - 1))] = base[idx].asRaw();
  }
  threadgroup_barrier(mem_flags::mem_threadgroup);
  Fp x[B];
  for (uint32_t j = 0; j < B; j++) {
    x[j] = Fp::fromRaw(tg[lid * (B + 1) + j]);
  }
  for (uint32_t s = L; s >= 1; s--) {
    uint32_t hs = 1 << (s - 1);
    for (uint32_t jj = 0; jj < hs; jj++) {
      Fp w = lowtw[hs - 1 + jj];
      for (uint32_t j = jj; j < B; j += 2 * hs) {
        Fp a = x[j];
        Fp b = x[j + hs];
        x[j] = a + b;
        x[j + hs] = (a - b) * w;
      }
    }
  }
  Fp n = norm[0];
  threadgroup_barrier(mem_flags::mem_threadgroup);
  for (uint32_t j = 0; j < B; j++) {
    tg[lid * (B + 1) + j] = (x[j] * n).asRaw();
  }
  threadgroup_barrier(mem_flags::mem_threadgroup);
  for (uint32_t idx = lid; idx < T * B; idx += T) {
    base[idx] = Fp::fromRaw(tg[(idx >> L) * (B + 1) + (idx & (B - 1))]);
  }
}

template <uint32_t R, bool FWD>
inline void high(device Fp* io, const device Fp* table, const device Fp* qroots, uint32_t nBits,
                 uint32_t sBits, uint gid) {
  constexpr uint32_t B = 1 << R;
  uint32_t nSize = 1 << nBits;
  uint32_t perCol = nSize >> R;
  uint32_t c = gid / perCol;
  uint32_t t = gid % perCol;
  uint32_t h = 1 << (sBits - 1);
  uint32_t bLo = t & (h - 1);
  uint32_t b = bLo + ((t >> (sBits - 1)) << (sBits - 1 + R));
  device Fp* p = io + (size_t)c * nSize + b;
  Fp x[B];
  for (uint32_t j = 0; j < B; j++) {
    x[j] = p[(size_t)j * h];
  }
  Fp base[R];
  base[R - 1] = table[bLo << (nBits - sBits - R + 1)];
  for (int q = int(R) - 2; q >= 0; q--) {
    base[q] = base[q + 1] * base[q + 1];
  }
  for (uint32_t qq = 0; qq < R; qq++) {
    uint32_t q = FWD ? qq : R - 1 - qq;
    uint32_t hs = 1 << q;
    Fp w = base[q];
    Fp step = qroots[q];
    for (uint32_t jj = 0; jj < hs; jj++) {
      for (uint32_t j = jj; j < B; j += 2 * hs) {
        Fp a = x[j];
        Fp y = x[j + hs];
        if (FWD) {
          y = y * w;
          x[j] = a + y;
          x[j + hs] = a - y;
        } else {
          x[j] = a + y;
          x[j + hs] = (a - y) * w;
        }
      }
      w = w * step;
    }
  }
  for (uint32_t j = 0; j < B; j++) {
    p[(size_t)j * h] = x[j];
  }
}

#define HIGH(R, DIR, FWD)                                                                       \
  kernel void ntt_##DIR##_high_##R(device Fp* io [[buffer(0)]],                                 \
                                   const device Fp* table [[buffer(1)]],                        \
                                   const device Fp* qroots [[buffer(2)]],                       \
                                   const device uint32_t& nBits [[buffer(3)]],                  \
                                   const device uint32_t& sBits [[buffer(4)]],                  \
                                   uint gid [[thread_position_in_grid]]) {                      \
    high<R, FWD>(io, table, qroots, nBits, sBits, gid);                                         \
  }

HIGH(1, fwd, true)
HIGH(2, fwd, true)
HIGH(3, fwd, true)
HIGH(4, fwd, true)
HIGH(5, fwd, true)
HIGH(1, rev, false)
HIGH(2, rev, false)
HIGH(3, rev, false)
HIGH(4, rev, false)
HIGH(5, rev, false)

// Direct variant of ntt_fwd_low: no threadgroup memory, so occupancy isn't capped. Each thread
// reads its 2^(L-E) coefficients and writes its 2^L outputs contiguously; within a SIMD group
// those accesses stay in a few cache lines.
template <uint32_t E, uint32_t L>
inline void fwd_low_direct(device Fp* out, const device Fp* in, const device Fp* lowtw,
                           uint32_t nBits, uint gid) {
  constexpr uint32_t B = 1 << L;
  constexpr uint32_t K = 1 << (L - E);
  uint32_t nSize = 1 << nBits;
  uint32_t blocksPerCol = nSize >> L;
  uint32_t c = gid / blocksPerCol;
  uint32_t blk = gid % blocksPerCol;
  const device Fp* src = in + (size_t)c * (nSize >> E) + (size_t)blk * K;
  Fp coeff[K];
  for (uint32_t k = 0; k < K; k++) {
    coeff[k] = src[k];
  }
  Fp x[B];
  for (uint32_t j = 0; j < B; j++) {
    x[j] = coeff[j >> E];
  }
  for (uint32_t s = E + 1; s <= L; s++) {
    uint32_t hs = 1 << (s - 1);
    for (uint32_t jj = 0; jj < hs; jj++) {
      Fp w = lowtw[hs - 1 + jj];
      for (uint32_t j = jj; j < B; j += 2 * hs) {
        Fp a = x[j];
        Fp b = x[j + hs] * w;
        x[j] = a + b;
        x[j + hs] = a - b;
      }
    }
  }
  device Fp* dst = out + (size_t)c * nSize + (size_t)blk * B;
  for (uint32_t j = 0; j < B; j++) {
    dst[j] = x[j];
  }
}

#define FWD_LOW_DIRECT(E, L)                                                                    \
  kernel void ntt_fwd_lowd_##E##_##L(device Fp* out [[buffer(0)]],                              \
                                     const device Fp* in [[buffer(1)]],                         \
                                     const device Fp* lowtw [[buffer(2)]],                      \
                                     const device uint32_t& nBits [[buffer(3)]],                \
                                     uint gid [[thread_position_in_grid]]) {                    \
    fwd_low_direct<E, L>(out, in, lowtw, nBits, gid);                                           \
  }

FWD_LOW_DIRECT(0, 6)
FWD_LOW_DIRECT(1, 6)
FWD_LOW_DIRECT(2, 6)
FWD_LOW_DIRECT(3, 6)
FWD_LOW_DIRECT(4, 6)

// Direct variant of ntt_rev_low_6 (no threadgroup memory), in place, with the 1/N scaling.
kernel void ntt_rev_lowd_6(device Fp* io [[buffer(0)]],
                           const device Fp* lowtw [[buffer(1)]],
                           const device Fp* norm [[buffer(2)]],
                           const device uint32_t& nBits [[buffer(3)]],
                           uint gid [[thread_position_in_grid]]) {
  constexpr uint32_t L = 6;
  constexpr uint32_t B = 1 << L;
  uint32_t nSize = 1 << nBits;
  uint32_t blocksPerCol = nSize >> L;
  uint32_t c = gid / blocksPerCol;
  uint32_t blk = gid % blocksPerCol;
  device Fp* base = io + (size_t)c * nSize + (size_t)blk * B;
  Fp x[B];
  for (uint32_t j = 0; j < B; j++) {
    x[j] = base[j];
  }
  for (uint32_t s = L; s >= 1; s--) {
    uint32_t hs = 1 << (s - 1);
    for (uint32_t jj = 0; jj < hs; jj++) {
      Fp w = lowtw[hs - 1 + jj];
      for (uint32_t j = jj; j < B; j += 2 * hs) {
        Fp a = x[j];
        Fp b = x[j + hs];
        x[j] = a + b;
        x[j + hs] = (a - b) * w;
      }
    }
  }
  Fp n = norm[0];
  for (uint32_t j = 0; j < B; j++) {
    base[j] = x[j] * n;
  }
}
