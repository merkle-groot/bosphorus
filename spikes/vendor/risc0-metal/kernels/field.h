// Montgomery BabyBear arithmetic, matching risc0-core and risc0-sys (Apache-2.0).
#include <metal_stdlib>
using namespace metal;
constant uint P = 2013265921u;
inline uint add(uint a, uint b) { uint r = a + b; return r >= P ? r - P : r; }
inline uint sub(uint a, uint b) { uint r = a - b; return a < b ? r + P : r; }
inline uint mul(uint a, uint b) {
    uint low = a * b;
    uint red = (0u - low) * 0x88000001u;
    uint r = mulhi(a, b) + mulhi(red, P) + uint(low != 0u);
    return r >= P ? r - P : r;
}
inline uint4 ext(uint a) { return uint4(a, 0u, 0u, 0u); }
inline uint4 add(uint4 a, uint4 b) { return uint4(add(a.x,b.x),add(a.y,b.y),add(a.z,b.z),add(a.w,b.w)); }
inline uint4 sub(uint4 a, uint4 b) { return uint4(sub(a.x,b.x),sub(a.y,b.y),sub(a.z,b.z),sub(a.w,b.w)); }
inline uint4 mul(uint4 a, uint b) { return uint4(mul(a.x,b),mul(a.y,b),mul(a.z,b),mul(a.w,b)); }
inline uint4 mul(uint a, uint4 b) { return mul(b,a); }
inline uint4 mul(uint4 a, uint4 b) {
    // The extension relation is X^4 = -11, as in risc0-sys/cxx/fpext.h.
    uint nbeta = 1073741848u; // (-11 * 2^32) mod P; checked by the generator.
    uint4 n = mul(a, nbeta);
    return add(add(mul(a,b.x),mul(uint4(n.w,a.x,a.y,a.z),b.y)),
               add(mul(uint4(n.z,n.w,a.x,a.y),b.z),mul(uint4(n.y,n.z,n.w,a.x),b.w)));
}
