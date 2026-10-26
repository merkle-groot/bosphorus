// BabyBear arithmetic on 4 points at once (NEON), matching kernels/field.h (Apache-2.0).
#include <arm_neon.h>
#include <stdint.h>

typedef uint32x4_t V;
typedef struct { V c[4]; } E;

#define NEON_P 2013265921u

static inline V add1(V a, V b) {
    V s = vaddq_u32(a, b);
    return vminq_u32(s, vsubq_u32(s, vdupq_n_u32(NEON_P)));
}
static inline V sub1(V a, V b) {
    V d = vsubq_u32(a, b);
    return vminq_u32(d, vaddq_u32(d, vdupq_n_u32(NEON_P)));
}
// Montgomery product a * b / 2^32 mod P, given mu_b = b * P^-1 mod 2^32 (as in Plonky3).
static inline V mulk(V a, V b, V mu_b) {
    int32x4_t sa = vreinterpretq_s32_u32(a);
    int32x4_t c_hi = vqdmulhq_s32(sa, vreinterpretq_s32_u32(b));
    int32x4_t q = vmulq_s32(sa, vreinterpretq_s32_u32(mu_b));
    int32x4_t qp_hi = vqdmulhq_s32(q, vdupq_n_s32((int32_t)NEON_P));
    // d is in (-P, P); as unsigned, min(d, d + P) is d mod P.
    uint32x4_t d = vreinterpretq_u32_s32(vhsubq_s32(c_hi, qp_hi));
    return vminq_u32(d, vaddq_u32(d, vdupq_n_u32(NEON_P)));
}
static inline V mul1(V a, V b) { return mulk(a, b, vmulq_u32(b, vdupq_n_u32(0x88000001u))); }

static inline E zero4(void) { E r = {{vdupq_n_u32(0), vdupq_n_u32(0), vdupq_n_u32(0), vdupq_n_u32(0)}}; return r; }
static inline E ext1(V a) { E r = {{a, vdupq_n_u32(0), vdupq_n_u32(0), vdupq_n_u32(0)}}; return r; }
static inline E ext4(uint32_t a, uint32_t b, uint32_t c, uint32_t d) {
    E r = {{vdupq_n_u32(a), vdupq_n_u32(b), vdupq_n_u32(c), vdupq_n_u32(d)}};
    return r;
}
static inline E loadpm(const uint32_t* m) { return ext4(m[0], m[1], m[2], m[3]); }
static inline E add4(E a, E b) {
    E r = {{add1(a.c[0], b.c[0]), add1(a.c[1], b.c[1]), add1(a.c[2], b.c[2]), add1(a.c[3], b.c[3])}};
    return r;
}
static inline E sub4(E a, E b) {
    E r = {{sub1(a.c[0], b.c[0]), sub1(a.c[1], b.c[1]), sub1(a.c[2], b.c[2]), sub1(a.c[3], b.c[3])}};
    return r;
}
static inline E mul41(E a, V b) {
    V mu = vmulq_u32(b, vdupq_n_u32(0x88000001u));
    E r = {{mulk(a.c[0], b, mu), mulk(a.c[1], b, mu), mulk(a.c[2], b, mu), mulk(a.c[3], b, mu)}};
    return r;
}
// Shoup product x * c mod P for a constant c in plain form, with s = floor(c * 2^31 / P): the
// quotient estimate q = floor(x * s / 2^31) is floor(x * c / P) or one less, so x * c - q * P is
// in [0, 2P). For x in Montgomery form the result is the Montgomery form of the product.
#define SHOUP(x, c, s, k) ({ \
    V r_ = vmlsq_n_u32(vmulq_laneq_u32(x, c, k), \
        vreinterpretq_u32_s32(vqdmulhq_laneq_s32(vreinterpretq_s32_u32(x), vreinterpretq_s32_u32(s), k)), NEON_P); \
    vminq_u32(r_, vsubq_u32(r_, vdupq_n_u32(NEON_P))); })
// acc += x * m, for a constant power m given as 8 words: its plain form and Shoup factors.
static inline E addpm(E acc, V x, const uint32_t* cs) {
    uint32x4_t c = vld1q_u32(cs), s = vld1q_u32(cs + 4);
    E r = {{add1(acc.c[0], SHOUP(x, c, s, 0)), add1(acc.c[1], SHOUP(x, c, s, 1)),
            add1(acc.c[2], SHOUP(x, c, s, 2)), add1(acc.c[3], SHOUP(x, c, s, 3))}};
    return r;
}
// The extension relation is X^4 = -11, as in kernels/field.h.
static inline E mul4(E a, E b) {
    E n = mul41(a, vdupq_n_u32(1073741848u));
    E r1 = {{n.c[3], a.c[0], a.c[1], a.c[2]}};
    E r2 = {{n.c[2], n.c[3], a.c[0], a.c[1]}};
    E r3 = {{n.c[1], n.c[2], n.c[3], a.c[0]}};
    return add4(add4(mul41(a, b.c[0]), mul41(r1, b.c[1])), add4(mul41(r2, b.c[2]), mul41(r3, b.c[3])));
}
