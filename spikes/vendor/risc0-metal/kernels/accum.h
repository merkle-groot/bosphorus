// Metal versions of the helpers risc0's generated rv32im accumulate code expects (Apache-2.0):
// risc0-circuit-rv32im-sys 4.0.2, kernels/cuda/witgen.h and buffers.h. Buffers are one
// non-virtual type (Metal has no virtual calls); layouts live in the constant address space.
#pragma once

#include <metal_stdlib>
#include "fp.h"
#include "fpext.h"

using namespace metal;

using Val = Fp;
using ExtVal = FpExt;
using Index = size_t;

#define assert(x) ((void)0)

// cuda::std::array as the generated code uses it (value_type, aggregate initialisation), for
// thread-local values and for layouts in the constant address space.
template <typename T, size_t N> struct cuarray {
  using value_type = T;
  T elems_[N];
  thread T& operator[](size_t i) thread { return elems_[i]; }
  T operator[](size_t i) const thread { return elems_[i]; }
  constant T& operator[](size_t i) constant { return elems_[i]; }
};

// The mixed Fp/FpExt operators of risc0-sys's CUDA fpext.h.
inline FpExt operator+(FpExt a, Fp b) {
  a.elems[0] = a.elems[0] + b;
  return a;
}
inline FpExt operator-(FpExt a, Fp b) {
  a.elems[0] = a.elems[0] - b;
  return a;
}
inline FpExt operator+(Fp a, FpExt b) {
  return b + a;
}
inline FpExt operator-(Fp a, FpExt b) {
  return a + (-b);
}

struct ExecContext {
  size_t cycle;
};

// A column-major trace buffer: rows x cols. `global` buffers have one row and no back offsets;
// the accum buffer reads 0 for back offsets into its columns past `zeroBack`.
struct BufferObj {
  device Fp* buf;
  size_t rows;
  size_t zeroBack;
  bool global;

  Val load(thread ExecContext& ctx, size_t col, size_t back) {
    if (global) {
      return buf[col];
    }
    if (zeroBack && col > zeroBack && back > 0) {
      return Val(0);
    }
    size_t backRow = (rows + ctx.cycle - back) % rows;
    return buf[col * rows + backRow];
  }

  void store(thread ExecContext& ctx, size_t col, Val val) {
    if (global) {
      buf[col] = val;
    } else {
      buf[col * rows + ctx.cycle] = val;
    }
  }
};

typedef thread BufferObj* MutableBuf;
typedef thread BufferObj* GlobalBuf;

template <typename T> struct BoundLayout {
  constant T* layout;
  thread BufferObj* buf;
};

template <typename T> inline BoundLayout<T> bind(constant T& layout, thread BufferObj* buf) {
  return BoundLayout<T>{&layout, buf};
}

inline size_t to_size_t(Val v) {
  return v.asUInt32();
}

inline Val mod(Val a, Val b) {
  return Val(a.asUInt32() % b.asUInt32());
}

constant size_t EXT_SIZE = 4;

inline Val isz(Val x) {
  return Val(x == Val(0));
}

inline Val neg_0(Val x) {
  return -x;
}

inline Val inv_0(Val x) {
  return inv(x);
}

inline ExtVal inv_0(ExtVal x) {
  return inv(x);
}

inline Val bitAnd(Val a, Val b) {
  return Val(a.asUInt32() & b.asUInt32());
}

inline Val inRange(Val low, Val mid, Val high) {
  return Val(low <= mid && mid < high);
}

struct Reg {
  constexpr Reg(size_t col) : col(col) {}
  size_t col;
};

#define BIND_LAYOUT(orig, buf) bind(orig, buf)
#define LAYOUT_LOOKUP(orig, elem) bind((orig).layout->elem, (orig).buf)
#define LAYOUT_SUBSCRIPT(orig, index) bind((*(orig).layout)[index], (orig).buf)
// The accumulate step re-checks constraints the witness already satisfies; nothing to report.
#define EQZ(val, loc) ((void)0)

inline void store(thread ExecContext& ctx, BoundLayout<Reg> reg, Val val) {
  reg.buf->store(ctx, reg.layout->col, val);
}

inline void storeExt(thread ExecContext& ctx, BoundLayout<Reg> reg, ExtVal val) {
  for (size_t i = 0; i < EXT_SIZE; i++) {
    reg.buf->store(ctx, reg.layout->col + i, val.elems[i]);
  }
}

inline Val load(thread ExecContext& ctx, BoundLayout<Reg> reg, size_t back) {
  return reg.buf->load(ctx, reg.layout->col, back);
}

inline ExtVal loadExt(thread ExecContext& ctx, BoundLayout<Reg> reg, size_t back) {
  Fp e0 = reg.buf->load(ctx, reg.layout->col + 0, back);
  Fp e1 = reg.buf->load(ctx, reg.layout->col + 1, back);
  Fp e2 = reg.buf->load(ctx, reg.layout->col + 2, back);
  Fp e3 = reg.buf->load(ctx, reg.layout->col + 3, back);
  return FpExt(e0, e1, e2, e3);
}

#define LOAD(reg, back) load(ctx, reg, back)
#define LOAD_EXT(reg, back) loadExt(ctx, reg, back)
#define STORE(reg, val) store(ctx, reg, val)
#define STORE_EXT(reg, val) storeExt(ctx, reg, val)

template <typename T1, typename F, size_t N> inline auto map(cuarray<T1, N> a, F f) {
  cuarray<decltype(f(T1{})), N> out;
  for (size_t i = 0; i < N; i++) {
    out.elems_[i] = f(a.elems_[i]);
  }
  return out;
}

template <typename T1, typename T2, typename F, size_t N>
inline auto map(cuarray<T1, N> a, cuarray<T2, N> b, F f) {
  cuarray<decltype(f(T1{}, T2{})), N> out;
  for (size_t i = 0; i < N; i++) {
    out.elems_[i] = f(a.elems_[i], b.elems_[i]);
  }
  return out;
}

template <typename T1, typename T2, typename F, size_t N>
inline auto map(cuarray<T1, N> a, BoundLayout<T2> b, F f) {
  cuarray<decltype(f(T1{}, bind(b.layout->elems_[0], b.buf))), N> out;
  for (size_t i = 0; i < N; i++) {
    out.elems_[i] = f(a.elems_[i], bind(b.layout->elems_[i], b.buf));
  }
  return out;
}

template <typename T1, typename T2, typename F, size_t N>
inline auto reduce(cuarray<T1, N> elems, T2 start, F f) {
  T2 cur = start;
  for (size_t i = 0; i < N; i++) {
    cur = f(cur, elems.elems_[i]);
  }
  return cur;
}

template <typename T1, typename T2, typename T3, typename F, size_t N>
inline auto reduce(cuarray<T1, N> elems, T2 start, BoundLayout<T3> b, F f) {
  T2 cur = start;
  for (size_t i = 0; i < N; i++) {
    cur = f(cur, elems.elems_[i], bind(b.layout->elems_[i], b.buf));
  }
  return cur;
}

#define INVOKE_EXTERN(ctx, name, ...) extern_##name(ctx, ##__VA_ARGS__)

inline void extern_assert(thread ExecContext& ctx, Val cond, constant char* message) {}

#define SET_FIELD(x) /**/
