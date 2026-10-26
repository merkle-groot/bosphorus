use super::*;
use risc0_zkp::{field::{ExtElem, map_pow}, hal::{Hal, metal::MetalHalPoseidon2}};

fn number(line: &str) -> usize {
    line.split('=').last().unwrap().trim().trim_end_matches(';').parse().unwrap()
}

fn info(text: &str) -> (usize, usize, Vec<usize>) {
    let output = number(text.lines().find(|l| l.contains("const OUTPUT_SIZE:")).unwrap());
    let mix = number(text.lines().find(|l| l.contains("const MIX_SIZE:")).unwrap());
    let powers = text.split("pub const POLY_MIX_POWERS:").nth(1).unwrap().split("= &[").nth(1).unwrap().split(']').next().unwrap()
        .split(',').filter_map(|s| { let s = s.trim(); (!s.is_empty()).then(|| s.parse().unwrap()) }).collect();
    (output, mix, powers)
}

fn groups(text: &str) -> [usize; 3] {
    let mut sizes = [1; 3];
    for tap in text.split("TapData {").skip(1) {
        let tap = tap.split('}').next().unwrap();
        let field = |name: &str| -> usize { tap.split(name).nth(1).unwrap().split(',').next().unwrap().trim().parse().unwrap() };
        let group = field("group:");
        sizes[group] = sizes[group].max(field("offset:") + 1);
    }
    sizes
}

#[test]
fn gpu_constraints_match_original_cpu_kernels() {
    let cases = [
        (Circuit::Rv32im, include_str!("../../risc0-circuit-rv32im/src/zirgen/info.rs"), include_str!("../../risc0-circuit-rv32im/src/zirgen/taps.rs")),
        (Circuit::Recursion, include_str!("../../risc0-circuit-recursion/src/info.rs"), include_str!("../../risc0-circuit-recursion/src/taps.rs")),
        (Circuit::Keccak, include_str!("../../risc0-circuit-keccak/src/zirgen/info.rs"), include_str!("../../risc0-circuit-keccak/src/zirgen/taps.rs")),
    ];
    let hal = MetalHalPoseidon2::new();
    let mut random = 0x5a17_u64;
    for (circuit, source_info, source_taps) in cases {
        for po2 in [6, 13] { // Includes both sides of the GPU tile boundary.
            let steps = 1 << po2;
            let domain = steps * 4;
            let mut values = |count: usize| -> Vec<Val> {
                (0..count).map(|_| {
                    random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                    Val::new((random >> 32) as u32 % 2013265921)
                }).collect()
            };
            let (output_size, mix_size, exponents) = info(source_info);
            let group_data: Vec<_> = groups(source_taps).into_iter().map(|n| values(n * domain)).collect();
            let out = values(output_size);
            let mix = values(mix_size);
            let seed = values(4);
            let poly_mix = ExtVal::from_subelems(seed);
            let powers = map_pow(poly_mix, &exponents);
            let buffers: Vec<_> = group_data.iter().map(|v| hal.copy_from_elem("group", v)).collect();
            let out_buffer = hal.copy_from_elem("out", &out);
            let mix_buffer = hal.copy_from_elem("mix", &mix);
            let check = hal.alloc_elem("check", domain * 4);
            evaluate(circuit, &check, &buffers.iter().collect::<Vec<_>>(), &[&mix_buffer, &out_buffer], poly_mix, po2, steps);
            let got = { let mut v = Vec::new(); check.view(|s| v.extend_from_slice(s)); v };
            let samples: Vec<_> = if po2 == 6 { (0..domain).collect() } else {
                (0..32).chain([16_382,16_383,16_384,16_385,domain-2,domain-1]).collect()
            };
            let rou = Val::ROU_FWD[po2 + 2];
            if matches!(circuit, Circuit::Recursion) {
                use risc0_circuit_recursion_sys::*;
                let input = RawAccumBuffers { ctrl: group_data[1].as_ptr(), data: group_data[2].as_ptr(),
                    accum: group_data[0].as_ptr(), global: out.as_ptr(), mix: mix.as_ptr() };
                let mut expected = vec![Val::ZERO; domain * 4];
                let err = unsafe { risc0_circuit_recursion_cpu_eval_check(&input, powers.as_ptr(), expected.as_mut_ptr(), rou, po2 as u32, domain as u32) };
                assert!(err.is_null());
                assert_eq!(got, expected, "{circuit:?} po2={po2}");
            } else {
                for cycle in samples {
                    let mut total = ExtVal::ZERO;
                    let err = unsafe {
                        match circuit {
                            Circuit::Rv32im => {
                                let args = [group_data[0].as_ptr(), group_data[2].as_ptr(), out.as_ptr(), mix.as_ptr()];
                                risc0_circuit_rv32im_sys::risc0_circuit_rv32im_cpu_poly_fp(cycle, domain, powers.as_ptr(), args.as_ptr(), &mut total)
                            }
                            Circuit::Keccak => {
                                let args = [group_data[2].as_ptr(), out.as_ptr()];
                                risc0_circuit_keccak_sys::risc0_circuit_keccak_cpu_poly_fp(cycle, domain, powers.as_ptr(), args.as_ptr(), &mut total)
                            }
                            Circuit::Recursion => unreachable!(),
                        }
                    };
                    assert!(err.is_null());
                    let expected = total * ((Val::new(3) * rou.pow(cycle)).pow(steps) - Val::ONE).inv();
                    for i in 0..4 { assert_eq!(got[i*domain+cycle], expected.elems()[i], "{circuit:?} po2={po2} cycle={cycle} limb={i}"); }
                }
            }
            eprintln!("CPU/GPU parity passed: {circuit:?} po2={po2}");
        }
    }
}
