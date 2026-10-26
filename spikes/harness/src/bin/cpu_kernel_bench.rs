//! Portable proving-kernel rates: NTT and Poseidon2 row hashing on risc0-zkp's reference
//! `CpuHal`, the same ops `r0vm` runs on any machine (no Metal/CUDA). A baseline for laptops
//! without the Metal GPU path (`metal_eval_bench`), or to compare CPU proving speed across them.
//!
//! `cargo run --release -p spike-harness --features cpu-bench --bin cpu_kernel_bench -- [log_size] [count] [runs]`

use std::time::Instant;

use risc0_zkp::{
    core::hash::poseidon2::Poseidon2HashSuite,
    field::baby_bear::BabyBearElem as Val,
    hal::{Hal, cpu::CpuHal},
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let log_size: usize = args.get(1).map_or(19, |s| s.parse().unwrap());
    let count: usize = args.get(2).map_or(211, |s| s.parse().unwrap());
    let runs: usize = args.get(3).map_or(3, |s| s.parse().unwrap());
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    println!("CPU kernel rates: {threads} threads, {count} columns x 2^{log_size}, {runs} runs");

    let mut random = 0x5eed_u64;
    let mut values = |n: usize| -> Vec<Val> {
        (0..n)
            .map(|_| {
                random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                Val::new((random >> 32) as u32 % 2013265921)
            })
            .collect()
    };
    let hal = CpuHal::new(Poseidon2HashSuite::new_suite());

    let (in_size, expand_bits) = (1 << log_size, 2);
    let out_size = in_size << expand_bits;
    let input = values(count * in_size);
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
        println!("ntt {count} x 2^{log_size} run {run}: expand+evaluate {expand:.3}s, interpolate {interpolate:.3}s");
    }

    let rows = 1 << log_size;
    let matrix = hal.copy_from_elem("matrix", &values(rows * count));
    let digests = hal.alloc_digest("digests", rows);
    for run in 0..runs {
        let started = Instant::now();
        hal.hash_rows(&digests, &matrix);
        let seconds = started.elapsed().as_secs_f64();
        println!(
            "hash {count} x 2^{log_size} run {run}: {seconds:.3}s ({:.2} M rows/s)",
            rows as f64 * count as f64 / seconds / 1e6
        );
    }
}
