//! GPU constraint evaluation (`risc0-metal`) on random trace data: times it and checks sample
//! points against risc0's own CPU kernels. Much faster to iterate on than a full S4 proof.
//!
//! `cargo run --release -p spike-harness --features prove --bin metal_eval_bench -- rv32im 18 2`

use risc0_metal::{evaluate, Circuit};
use risc0_zkp::{
    field::{baby_bear::{BabyBearElem as Val, BabyBearExtElem as ExtVal}, map_pow, Elem, ExtElem, RootsOfUnity},
    hal::{metal::MetalHalPoseidon2, Buffer, Hal},
};
use std::time::Instant;

fn number(text: &str, key: &str) -> usize {
    let line = text.lines().find(|l| l.contains(key)).unwrap();
    line.split('=').last().unwrap().trim().trim_end_matches(';').parse().unwrap()
}

fn powers(text: &str) -> Vec<usize> {
    text.split("pub const POLY_MIX_POWERS:").nth(1).unwrap().split("= &[").nth(1).unwrap().split(']').next().unwrap()
        .split(',').filter_map(|s| { let s = s.trim(); (!s.is_empty()).then(|| s.parse().unwrap()) }).collect()
}

fn groups(text: &str) -> [usize; 3] {
    let mut sizes = [1; 3];
    for tap in text.split("TapData {").skip(1) {
        let tap = tap.split('}').next().unwrap();
        let field = |name: &str| -> usize { tap.split(name).nth(1).unwrap().split(',').next().unwrap().trim().parse().unwrap() };
        sizes[field("group:")] = sizes[field("group:")].max(field("offset:") + 1);
    }
    sizes
}

/// `ntt LOG_IN COUNT RUNS`: the Metal HAL's NTTs on COUNT columns of 2^LOG_IN coefficients
/// (expanded 4x, as for a commit), checked against the CPU HAL on a few columns.
fn ntt(args: &[String]) {
    use risc0_zkp::{core::hash::poseidon2::Poseidon2HashSuite, hal::cpu::CpuHal};
    let log_in: usize = args.get(2).map_or(19, |s| s.parse().unwrap());
    let count: usize = args.get(3).map_or(211, |s| s.parse().unwrap());
    let runs: usize = args.get(4).map_or(2, |s| s.parse().unwrap());
    let (in_size, expand_bits) = (1 << log_in, 2);
    let out_size = in_size << expand_bits;
    let mut random = 0x5eed_u64;
    let input: Vec<Val> = (0..count * in_size).map(|_| {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        Val::new((random >> 32) as u32 % 2013265921)
    }).collect();
    let hal = MetalHalPoseidon2::new();
    let gpu_in = hal.copy_from_elem("in", &input);
    let gpu_out = hal.alloc_elem("out", count * out_size);
    let gpu_io = hal.copy_from_elem("io", &input);
    for run in 0..runs {
        let started = Instant::now();
        hal.batch_expand_into_evaluate_ntt(&gpu_out, &gpu_in, count, expand_bits);
        let expand = started.elapsed().as_secs_f64();
        let started = Instant::now();
        hal.batch_interpolate_ntt(&gpu_io, count);
        let interpolate = started.elapsed().as_secs_f64();
        println!("ntt {count} x 2^{log_in} run {run}: expand+evaluate {expand:.3}s, interpolate {interpolate:.3}s");
    }
    // Reference: the CPU HAL on the first columns (each run interpolated gpu_io once more).
    let check = count.min(4);
    let cpu = CpuHal::new(Poseidon2HashSuite::new_suite());
    let cpu_in = cpu.copy_from_elem("in", &input[..check * in_size]);
    let cpu_out = cpu.alloc_elem("out", check * out_size);
    cpu.batch_expand_into_evaluate_ntt(&cpu_out, &cpu_in, check, expand_bits);
    let cpu_io = cpu.copy_from_elem("io", &input[..check * in_size]);
    for _ in 0..runs {
        cpu.batch_interpolate_ntt(&cpu_io, check);
    }
    let (mut want, mut got) = (Vec::new(), Vec::new());
    cpu_out.view(|s| want.extend_from_slice(s));
    gpu_out.view(|s| got.extend_from_slice(&s[..check * out_size]));
    assert!(want == got, "expand+evaluate differs from the CPU HAL");
    let (mut want, mut got) = (Vec::new(), Vec::new());
    cpu_io.view(|s| want.extend_from_slice(s));
    gpu_io.view(|s| got.extend_from_slice(&s[..check * in_size]));
    assert!(want == got, "interpolate differs from the CPU HAL");
    println!("ntt: GPU matches the CPU HAL on {check} columns");
}

/// `hash LOG_ROWS COUNT RUNS`: Poseidon2 row hashing of a COUNT-column matrix (as a commit does),
/// checked against the CPU HAL on a small matrix.
fn hash(args: &[String]) {
    use risc0_zkp::{core::{digest::Digest, hash::poseidon2::Poseidon2HashSuite}, hal::cpu::CpuHal};
    let log_rows: usize = args.get(2).map_or(21, |s| s.parse().unwrap());
    let count: usize = args.get(3).map_or(211, |s| s.parse().unwrap());
    let runs: usize = args.get(4).map_or(3, |s| s.parse().unwrap());
    let mut random = 0xfeed_u64;
    let mut values = |n: usize| -> Vec<Val> {
        (0..n).map(|_| {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            Val::new((random >> 32) as u32 % 2013265921)
        }).collect()
    };
    let hal = MetalHalPoseidon2::new();
    let rows = 1 << log_rows;
    let matrix = hal.copy_from_elem("matrix", &values(rows * count));
    let digests = hal.alloc_digest("digests", rows);
    for run in 0..runs {
        let started = Instant::now();
        hal.hash_rows(&digests, &matrix);
        println!("hash {count} x 2^{log_rows} run {run}: {:.3}s", started.elapsed().as_secs_f64());
    }
    let small_rows = 1 << 12;
    let small = values(small_rows * count);
    let (gpu_out, cpu) = (hal.alloc_digest("d", small_rows), CpuHal::new(Poseidon2HashSuite::new_suite()));
    hal.hash_rows(&gpu_out, &hal.copy_from_elem("m", &small));
    let cpu_out = cpu.alloc_digest("d", small_rows);
    let cpu_matrix = cpu.copy_from_elem("m", &small);
    let started = Instant::now();
    cpu.hash_rows(&cpu_out, &cpu_matrix);
    println!("CPU HAL hash_rows {count} x 2^12: {:.3}s ({:.2} M rows/s)", started.elapsed().as_secs_f64(),
        small_rows as f64 / started.elapsed().as_secs_f64() / 1e6);
    let (mut want, mut got): (Vec<Digest>, Vec<Digest>) = (Vec::new(), Vec::new());
    cpu_out.view(|s| want.extend_from_slice(s));
    gpu_out.view(|s| got.extend_from_slice(s));
    assert!(want == got, "hash_rows differs from the CPU HAL");
    println!("hash: GPU matches the CPU HAL on {count} x 2^12");
}

/// `small LOG_SIZE COUNT RUNS`: the per-proof small kernels on COUNT polynomials of 2^LOG_SIZE
/// coefficients (batch_evaluate_any at one point per polynomial, zk_shift, mix_poly_coeffs),
/// timed and checked against the CPU HAL.
fn small(args: &[String]) {
    use risc0_zkp::{core::hash::poseidon2::Poseidon2HashSuite, hal::cpu::CpuHal};
    let log_size: usize = args.get(2).map_or(19, |s| s.parse().unwrap());
    let count: usize = args.get(3).map_or(211, |s| s.parse().unwrap());
    let runs: usize = args.get(4).map_or(2, |s| s.parse().unwrap());
    let size = 1 << log_size;
    let mut random = 0xabcd_u64;
    let mut values = |n: usize| -> Vec<Val> {
        (0..n).map(|_| {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            Val::new((random >> 32) as u32 % 2013265921)
        }).collect()
    };
    let coeffs = values(count * size);
    let which: Vec<u32> = (0..count as u32).collect();
    let xs: Vec<ExtVal> = (0..count).map(|_| ExtVal::from_subelems(values(4))).collect();
    let hal = MetalHalPoseidon2::new();
    let cpu = CpuHal::new(Poseidon2HashSuite::new_suite());
    let read = |b: &risc0_zkp::hal::metal::BufferImpl<ExtVal>| { let mut v = Vec::new(); b.view(|s| v.extend_from_slice(s)); v };
    let (g_coeffs, g_which, g_xs) = (hal.copy_from_elem("c", &coeffs), hal.copy_from_u32("w", &which), hal.copy_from_extelem("x", &xs));
    let g_out = hal.alloc_extelem("o", count);
    for run in 0..runs {
        let started = Instant::now();
        hal.batch_evaluate_any(&g_coeffs, count, &g_which, &g_xs, &g_out);
        println!("batch_evaluate_any {count} x 2^{log_size} run {run}: {:.3}s", started.elapsed().as_secs_f64());
    }
    let (c_coeffs, c_which, c_xs) = (cpu.copy_from_elem("c", &coeffs), cpu.copy_from_u32("w", &which), cpu.copy_from_extelem("x", &xs));
    let c_out = cpu.alloc_extelem("o", count);
    cpu.batch_evaluate_any(&c_coeffs, count, &c_which, &c_xs, &c_out);
    let mut want = Vec::new();
    c_out.view(|s| want.extend_from_slice(s));
    assert!(read(&g_out) == want, "batch_evaluate_any differs from the CPU HAL");
    for run in 0..runs {
        let io = hal.copy_from_elem("io", &coeffs);
        let started = Instant::now();
        hal.zk_shift(&io, count);
        println!("zk_shift {count} x 2^{log_size} run {run}: {:.3}s", started.elapsed().as_secs_f64());
        if run == 0 {
            let c_io = cpu.copy_from_elem("io", &coeffs);
            cpu.zk_shift(&c_io, count);
            let (mut want, mut got) = (Vec::new(), Vec::new());
            c_io.view(|s| want.extend_from_slice(s));
            io.view(|s| got.extend_from_slice(s));
            assert!(want == got, "zk_shift differs from the CPU HAL");
        }
    }
    let combo_ids: Vec<u32> = (0..count as u32).map(|i| i % 5).collect();
    let (mix_start, mix) = (ExtVal::from_subelems(values(4)), ExtVal::from_subelems(values(4)));
    let g_ids = hal.copy_from_u32("ids", &combo_ids);
    let g_combos = hal.alloc_extelem_zeroed("combos", size * 6);
    for run in 0..runs {
        let started = Instant::now();
        hal.mix_poly_coeffs(&g_combos, &mix_start, &mix, &g_coeffs, &g_ids, count, size);
        println!("mix_poly_coeffs {count} x 2^{log_size} run {run}: {:.3}s", started.elapsed().as_secs_f64());
    }
    let c_combos = cpu.alloc_extelem_zeroed("combos", size * 6);
    let c_ids = cpu.copy_from_u32("ids", &combo_ids);
    for _ in 0..runs {
        cpu.mix_poly_coeffs(&c_combos, &mix_start, &mix, &c_coeffs, &c_ids, count, size);
    }
    let mut want = Vec::new();
    c_combos.view(|s| want.extend_from_slice(s));
    assert!(read(&g_combos) == want, "mix_poly_coeffs differs from the CPU HAL");
    println!("small kernels: GPU matches the CPU HAL");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "small") {
        return small(&args);
    }
    if args.get(1).is_some_and(|a| a == "ntt") {
        return ntt(&args);
    }
    if args.get(1).is_some_and(|a| a == "hash") {
        return hash(&args);
    }
    let circuit = match args.get(1).map_or("rv32im", String::as_str) {
        "rv32im" => Circuit::Rv32im,
        "recursion" => Circuit::Recursion,
        "keccak" => Circuit::Keccak,
        other => panic!("unknown circuit {other}"),
    };
    let po2: usize = args.get(2).map_or(18, |s| s.parse().unwrap());
    let runs: usize = args.get(3).map_or(2, |s| s.parse().unwrap());
    let (info, taps) = match circuit {
        Circuit::Rv32im => (include_str!("../../../vendor/risc0-circuit-rv32im/src/zirgen/info.rs"), include_str!("../../../vendor/risc0-circuit-rv32im/src/zirgen/taps.rs")),
        Circuit::Recursion => (include_str!("../../../vendor/risc0-circuit-recursion/src/info.rs"), include_str!("../../../vendor/risc0-circuit-recursion/src/taps.rs")),
        Circuit::Keccak => (include_str!("../../../vendor/risc0-circuit-keccak/src/zirgen/info.rs"), include_str!("../../../vendor/risc0-circuit-keccak/src/zirgen/taps.rs")),
    };
    let steps = 1 << po2;
    let domain = steps * 4;
    let mut random = 0x5a17_u64;
    let mut values = |count: usize| -> Vec<Val> {
        (0..count).map(|_| {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            Val::new((random >> 32) as u32 % 2013265921)
        }).collect()
    };
    let hal = MetalHalPoseidon2::new();
    let group_data: Vec<_> = groups(taps).into_iter().map(|n| values(n * domain)).collect();
    let out = values(number(info, "const OUTPUT_SIZE:"));
    let mix = values(number(info, "const MIX_SIZE:"));
    let poly_mix = ExtVal::from_subelems(values(4));
    let mix_powers = map_pow(poly_mix, &powers(info));
    let buffers: Vec<_> = group_data.iter().map(|v| hal.copy_from_elem("group", v)).collect();
    let out_buffer = hal.copy_from_elem("out", &out);
    let mix_buffer = hal.copy_from_elem("mix", &mix);
    let check = hal.alloc_elem("check", domain * 4);
    for run in 0..runs {
        let started = Instant::now();
        evaluate(circuit, &check, &buffers.iter().collect::<Vec<_>>(), &[&mix_buffer, &out_buffer], poly_mix, po2, steps);
        let seconds = started.elapsed().as_secs_f64();
        println!("{circuit:?} po2={po2} run {run}: {seconds:.3}s, {:.2} µs/point", seconds * 1e6 / domain as f64);
    }
    let got = { let mut v = Vec::new(); check.view(|s| v.extend_from_slice(s)); v };
    let rou = Val::ROU_FWD[po2 + 2];
    let samples: Vec<usize> = (0..64).chain((0..64).map(|i| (i * 7919 + 13) % domain)).chain([domain - 2, domain - 1]).collect();
    if matches!(circuit, Circuit::Recursion) {
        use risc0_circuit_recursion_sys::*;
        let input = RawAccumBuffers { ctrl: group_data[1].as_ptr(), data: group_data[2].as_ptr(),
            accum: group_data[0].as_ptr(), global: out.as_ptr(), mix: mix.as_ptr() };
        let mut expected = vec![Val::ZERO; domain * 4];
        let err = unsafe { risc0_circuit_recursion_cpu_eval_check(&input, mix_powers.as_ptr(), expected.as_mut_ptr(), rou, po2 as u32, domain as u32) };
        assert!(err.is_null());
        assert_eq!(got, expected, "{circuit:?} po2={po2}");
    } else {
        for cycle in samples {
            let mut total = ExtVal::ZERO;
            let err = unsafe {
                match circuit {
                    Circuit::Rv32im => {
                        let args = [group_data[0].as_ptr(), group_data[2].as_ptr(), out.as_ptr(), mix.as_ptr()];
                        risc0_circuit_rv32im_sys::risc0_circuit_rv32im_cpu_poly_fp(cycle, domain, mix_powers.as_ptr(), args.as_ptr(), &mut total)
                    }
                    Circuit::Keccak => {
                        let args = [group_data[2].as_ptr(), out.as_ptr()];
                        risc0_circuit_keccak_sys::risc0_circuit_keccak_cpu_poly_fp(cycle, domain, mix_powers.as_ptr(), args.as_ptr(), &mut total)
                    }
                    Circuit::Recursion => unreachable!(),
                }
            };
            assert!(err.is_null());
            let expected = total * ((Val::new(3) * rou.pow(cycle)).pow(steps) - Val::ONE).inv();
            for i in 0..4 {
                assert_eq!(got[i * domain + cycle], expected.elems()[i], "{circuit:?} po2={po2} cycle={cycle} limb={i}");
            }
        }
    }
    println!("{circuit:?} po2={po2}: GPU matches the CPU kernels");
    if std::env::var_os("CPU_RATE").is_some() {
        // risc0's scalar C++ evaluator over a slice of points on every core, for comparison.
        let points = domain.min(1 << 16);
        let threads = std::thread::available_parallelism().map_or(8, |n| n.get());
        let started = Instant::now();
        std::thread::scope(|scope| {
            for t in 0..threads {
                let (group_data, out, mix, mix_powers) = (&group_data, &out, &mix, &mix_powers);
                scope.spawn(move || {
                    for cycle in (t..points).step_by(threads) {
                        let mut total = ExtVal::ZERO;
                        unsafe {
                            match circuit {
                                Circuit::Rv32im => {
                                    let args = [group_data[0].as_ptr(), group_data[2].as_ptr(), out.as_ptr(), mix.as_ptr()];
                                    risc0_circuit_rv32im_sys::risc0_circuit_rv32im_cpu_poly_fp(cycle, domain, mix_powers.as_ptr(), args.as_ptr(), &mut total);
                                }
                                Circuit::Keccak => {
                                    let args = [group_data[2].as_ptr(), out.as_ptr()];
                                    risc0_circuit_keccak_sys::risc0_circuit_keccak_cpu_poly_fp(cycle, domain, mix_powers.as_ptr(), args.as_ptr(), &mut total);
                                }
                                Circuit::Recursion => {}
                            }
                        }
                        std::hint::black_box(total);
                    }
                });
            }
        });
        let seconds = started.elapsed().as_secs_f64();
        println!("CPU (all cores): {points} points in {seconds:.3}s, {:.2} µs/point", seconds * 1e6 / points as f64);
    }
}
