// Copyright 2023 Risc0, Inc.
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

#include "fp.h"
#include "sha256.h"

using namespace metal;

#define CELLS 24
#define ROUNDS_FULL 8
#define ROUNDS_HALF_FULL (ROUNDS_FULL / 2)
#define ROUNDS_PARTIAL 21
#define ROW_SIZE (CELLS + ROUNDS_PARTIAL)
#define CELLS_RATE 16
#define CELLS_OUT 8

namespace {

void add_round_constants_full(const device Fp* ROUND_CONSTANTS, thread Fp* cells, uint round) {
  for (uint i = 0; i < CELLS; i++) {
    cells[i] += ROUND_CONSTANTS[round * CELLS + i];
  }
}

void add_round_constants_partial(const device Fp* ROUND_CONSTANTS, thread Fp* cells, uint round) {
  cells[0] += ROUND_CONSTANTS[round * CELLS];
}

Fp sbox(Fp x) {
  Fp x2 = x * x;
  Fp x4 = x2 * x2;
  Fp x6 = x4 * x2;
  return x6 * x;
}

void do_full_sboxes(thread Fp* cells) {
  for (uint i = 0; i < CELLS; i++) {
    cells[i] = sbox(cells[i]);
  }
}

void do_partial_sboxes(thread Fp* cells) {
  cells[0] = sbox(cells[0]);
}

// Bosphorus: M_INT_DIAG_HZN (risc0-zkp poseidon2/consts.rs) in plain form, and the Shoup factors
// floor(d * 2^32 / P). x * d - q * P with q = hi(x * s) is x * d mod P plus 0 or P; for x in
// Montgomery form it is the Montgomery form of the product, the value `M_INT_DIAG[i] * x` gives.
constant uint32_t M_INT_DIAG_PLAIN[CELLS] = {0x409133f0, 0x1667a8a1, 0x06a6c7b6, 0x6f53160e, 0x273b11d1, 0x03176c5d, 0x72f9bbf9, 0x73ceba91, 0x5cdef81d, 0x01393285, 0x46daee06, 0x065d7ba6, 0x52d72d6f, 0x05dd05e0, 0x3bab4b63, 0x6ada3842, 0x2fc5fbec, 0x770d61b0, 0x5715aae9, 0x03ef0e90, 0x75b6c770, 0x242adf5f, 0x00d0ca4c, 0x36c0e388};
constant uint32_t M_INT_DIAG_SHOUP[CELLS] = {0x89be4ca9, 0x2fcc1268, 0x0e3098fb, 0xed7e0ce8, 0x53b13712, 0x06985ea4, 0xf547f766, 0xf70e5acc, 0xc61fef2b, 0x029c2782, 0x9728622d, 0x0d943afb, 0xb0b9fa84, 0x0c822eaa, 0x7f4b4b7c, 0xe3f3ab35, 0x65eaa1f6, 0xfdfa69fd, 0xb9c7d300, 0x08646355, 0xfb1f8753, 0x4d2842ec, 0x01bd6b4c, 0x74ceb232};

inline Fp mul_diag(Fp x, uint i) {
  uint32_t a = x.asRaw();
  uint32_t r = a * M_INT_DIAG_PLAIN[i] - metal::mulhi(a, M_INT_DIAG_SHOUP[i]) * Fp::P;
  return Fp::fromRaw(r >= Fp::P ? r - Fp::P : r);
}

void multiply_by_m_int(const device Fp* M_INT_DIAG, thread Fp* cells) {
  // Exploit the fact that off-diagonal entries of M_INT are all 1.
  Fp sum = 0;
  for (uint i = 0; i < CELLS; i++) {
    sum += cells[i];
  }
  for (uint i = 0; i < CELLS; i++) {
    cells[i] = sum + mul_diag(cells[i], i);
  }
}

void multiply_by_4x4_circulant(thread Fp* x) {
  // See appendix B of Poseidon2 paper.
  Fp t0 = x[0] + x[1];
  Fp t1 = x[2] + x[3];
  // Bosphorus: 2x and 4x as additions (Montgomery form is linear), not Montgomery products.
  Fp t2 = (x[1] + x[1]) + t1;
  Fp t3 = (x[3] + x[3]) + t0;
  Fp t1x2 = t1 + t1;
  Fp t0x2 = t0 + t0;
  Fp t4 = (t1x2 + t1x2) + t3;
  Fp t5 = (t0x2 + t0x2) + t2;
  Fp t6 = t3 + t5;
  Fp t7 = t2 + t4;
  x[0] = t6;
  x[1] = t5;
  x[2] = t7;
  x[3] = t4;
}

void multiply_by_m_ext(thread Fp* old_cells) {
  // Optimized method for multiplication by M_EXT.
  // See appendix B of Poseidon2 paper for additional details.
  Fp cells[CELLS];
  for (uint i = 0; i < CELLS; i++) {
    cells[0] = Fp(0);
  }
  Fp tmp_sums[4];
  for (uint i = 0; i < 4; i++) {
    tmp_sums[i] = Fp(0);
  }
  for (uint i = 0; i < CELLS / 4; i++) {
    multiply_by_4x4_circulant(old_cells + i * 4);
    for (uint j = 0; j < 4; j++) {
      Fp to_add = old_cells[i * 4 + j];
      tmp_sums[j] += to_add;
      cells[i * 4 + j] += to_add;
    }
  }
  for (uint i = 0; i < CELLS; i++) {
    old_cells[i] = cells[i] + tmp_sums[i % 4];
  }
}

void full_round(const device Fp* ROUND_CONSTANTS, thread Fp* cells, uint round) {
  add_round_constants_full(ROUND_CONSTANTS, cells, round);
  do_full_sboxes(cells);
  multiply_by_m_ext(cells);
}

void partial_round(const device Fp* ROUND_CONSTANTS,
                   const device Fp* M_INT_DIAG,
                   thread Fp* cells,
                   uint round) {
  add_round_constants_partial(ROUND_CONSTANTS, cells, round);
  do_partial_sboxes(cells);
  multiply_by_m_int(M_INT_DIAG, cells);
}

void poseidon2_mix(const device Fp* ROUND_CONSTANTS,
                   const device Fp* M_INT_DIAG,
                   thread Fp* cells) {
  uint round = 0;

  // First linear layer.
  multiply_by_m_ext(cells);

  // Do initial full rounds
  for (uint i = 0; i < ROUNDS_HALF_FULL; i++) {
    full_round(ROUND_CONSTANTS, cells, round);
    round++;
  }
  // Do partial rounds
  for (uint i = 0; i < ROUNDS_PARTIAL; i++) {
    partial_round(ROUND_CONSTANTS, M_INT_DIAG, cells, round);
    round++;
  }
  // Do remaining full rounds
  for (uint i = 0; i < ROUNDS_HALF_FULL; i++) {
    full_round(ROUND_CONSTANTS, cells, round);
    round++;
  }
}

} // namespace

kernel void poseidon2_fold(const device Fp* ROUND_CONSTANTS,
                           const device Fp* M_INT_DIAG,
                           device Fp* output,
                           const device Fp* input,
                           device uint32_t& output_size,
                           uint gid [[thread_position_in_grid]]) {
  Fp cells[CELLS];
  for (size_t i = 0; i < CELLS_OUT; i++) {
    cells[i] = input[2 * gid * CELLS_OUT + i];
    cells[CELLS_OUT + i] = input[(2 * gid + 1) * CELLS_OUT + i];
  }
  poseidon2_mix(ROUND_CONSTANTS, M_INT_DIAG, cells);
  for (uint i = 0; i < CELLS_OUT; i++) {
    output[gid * CELLS_OUT + i] = cells[i];
  }
}

kernel void poseidon2_rows(const device Fp* ROUND_CONSTANTS,
                           const device Fp* M_INT_DIAG,
                           device Fp* out,
                           const device Fp* matrix,
                           const device uint32_t& count,
                           const device uint32_t& col_size,
                           uint gid [[thread_position_in_grid]]) {
  Fp cells[CELLS];
  // Bosphorus: the same overwrite-mode sponge as before (each CELLS_RATE inputs overwrite the
  // rate cells, then a mix; a short tail is zero-padded), but every cell index is a compile-time
  // constant so `cells` can stay in registers. `cells[used++]` forced it into memory.
  uint full = col_size / CELLS_RATE;
  for (uint chunk = 0; chunk < full; chunk++) {
    for (uint j = 0; j < CELLS_RATE; j++) {
      cells[j] = matrix[(chunk * CELLS_RATE + j) * count + gid];
    }
    poseidon2_mix(ROUND_CONSTANTS, M_INT_DIAG, cells);
  }
  uint tail = col_size - full * CELLS_RATE;
  if (tail != 0 || count == 0) {
    // Zero pad to get a CELLS_RATE-aligned number of inputs
    for (uint j = 0; j < CELLS_RATE; j++) {
      cells[j] = j < tail ? matrix[(full * CELLS_RATE + j) * count + gid] : Fp(0);
    }
    poseidon2_mix(ROUND_CONSTANTS, M_INT_DIAG, cells);
  }
  for (uint i = 0; i < CELLS_OUT; i++) {
    out[CELLS_OUT * gid + i] = cells[i];
  }
}
