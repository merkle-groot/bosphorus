//! GPU evaluation of the unchanged RISC Zero 3.0.5 constraint polynomials.
//!
//! `generate.py` flattens each verifier polynomial into a sum of terms and splits it into
//! kernels ("stages") that recompute what they need from the trace columns. The stages share
//! one extension-field partial sum per point and nothing else.

use std::{ffi::c_void, sync::{atomic::{AtomicUsize, Ordering}, Mutex, OnceLock}, time::Instant};
use metal::{CommandQueue, ComputePipelineState, Device, MTLCommandBufferStatus, MTLResourceOptions, MTLSize};
use risc0_zkp::{
    field::{baby_bear::{BabyBearElem as Val, BabyBearExtElem as ExtVal}, Elem, RootsOfUnity},
    hal::{metal::{BufferImpl, KernelArg}, Buffer},
};

mod metadata { include!(concat!(env!("OUT_DIR"), "/metadata.rs")); }

#[cfg(test)]
mod tests;

/// Points per command buffer at most. Bounds the partial-sum buffer (16 bytes a point) and the
/// length of one GPU submission.
const TILE: usize = 1 << 20;
/// Bound on the scratch buffer (values stored for later stages, 4 bytes a slot and point).
const SCRATCH_BYTES: usize = 256 << 20;

#[derive(Clone, Copy, Debug)]
pub enum Circuit { Rv32im, Recursion, Keccak }

/// The generated NEON evaluator of a circuit: points `start..end` (multiples of 4) of `check`.
type NeonEval = unsafe extern "C" fn(g0: *const u32, g1: *const u32, g2: *const u32, gl0: *const u32,
    gl1: *const u32, pm: *const u32, pmu: *const u32, divisors: *const u32, check: *mut u32, domain: u32,
    start: u32, end: u32);

extern "C" {
    fn rv32im_neon(g0: *const u32, g1: *const u32, g2: *const u32, gl0: *const u32, gl1: *const u32,
        pm: *const u32, pmu: *const u32, divisors: *const u32, check: *mut u32, domain: u32, start: u32, end: u32);
    fn recursion_neon(g0: *const u32, g1: *const u32, g2: *const u32, gl0: *const u32, gl1: *const u32,
        pm: *const u32, pmu: *const u32, divisors: *const u32, check: *mut u32, domain: u32, start: u32, end: u32);
    fn keccak_neon(g0: *const u32, g1: *const u32, g2: *const u32, gl0: *const u32, gl1: *const u32,
        pm: *const u32, pmu: *const u32, divisors: *const u32, check: *mut u32, domain: u32, start: u32, end: u32);
}

/// Points per CPU work item.
const CPU_CHUNK: usize = 2048;

struct Engine {
    device: Device,
    queue: CommandQueue,
    pipelines: Vec<ComputePipelineState>,
    slots: usize,
    max_power: usize,
    neon: NeonEval,
    /// Starting guesses of (GPU, CPU) seconds per point.
    guess: (f64, f64),
    /// Measured (GPU, CPU) seconds per point by po2, for the next split of the domain: the CPU's
    /// rate falls with the domain size (caches), so rates from small proofs don't carry over.
    rates: Mutex<std::collections::HashMap<usize, (f64, f64)>>,
}

impl Engine {
    fn new(name: &str, library: &[u8], (count, slots, max_power): (usize, usize, usize), neon: NeonEval,
           rates: (f64, f64)) -> Self {
        let started = Instant::now();
        let device = Device::system_default().expect("Metal device");
        let queue = device.new_command_queue();
        // Pipeline creation compiles each kernel for this GPU unless macOS has it cached, which
        // takes minutes cold. Spread it over the cores.
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let mut pipelines: Vec<Option<ComputePipelineState>> = (0..count).map(|_| None).collect();
        std::thread::scope(|scope| {
            for (t, chunk) in pipelines.chunks_mut(count.div_ceil(threads)).enumerate() {
                let device = &device;
                scope.spawn(move || {
                    let library = device.new_library_with_data(library).expect("constraint metallib");
                    for (j, slot) in chunk.iter_mut().enumerate() {
                        let i = t * count.div_ceil(threads) + j;
                        let function = library.get_function(&format!("{name}_{i}"), None).expect("constraint kernel");
                        *slot = Some(device.new_compute_pipeline_state_with_function(&function).expect("constraint pipeline"));
                    }
                });
            }
        });
        let pipelines: Vec<_> = pipelines.into_iter().map(Option::unwrap).collect();
        eprintln!("[metal] {name}: {count} constraint kernels initialized in {:.3}s", started.elapsed().as_secs_f64());
        if std::env::var_os("R0_METAL_STATS").is_some() {
            let limits: Vec<_> = pipelines.iter().map(|p| p.max_total_threads_per_threadgroup()).collect();
            eprintln!("[metal] {name}: max threads per threadgroup by stage: {limits:?}");
        }
        Self { device, queue, pipelines, slots, max_power, neon, guess: rates, rates: Mutex::new(Default::default()) }
    }
}

fn engine(circuit: Circuit) -> &'static Engine {
    static RV32IM: OnceLock<Engine> = OnceLock::new();
    static RECURSION: OnceLock<Engine> = OnceLock::new();
    static KECCAK: OnceLock<Engine> = OnceLock::new();
    match circuit {
        // Starting guesses of (GPU, CPU) seconds per point on an M1, on the CPU's slow side (a CPU
        // share that's too big costs more than one that's too small); refined per call.
        Circuit::Rv32im => RV32IM.get_or_init(|| Engine::new("rv32im", include_bytes!(concat!(env!("OUT_DIR"), "/rv32im.metallib")),
            metadata::RV32IM, rv32im_neon, (1.8e-6, 9.0e-6))),
        Circuit::Recursion => RECURSION.get_or_init(|| Engine::new("recursion", include_bytes!(concat!(env!("OUT_DIR"), "/recursion.metallib")),
            metadata::RECURSION, recursion_neon, (0.47e-6, 1.8e-6))),
        Circuit::Keccak => KECCAK.get_or_init(|| Engine::new("keccak", include_bytes!(concat!(env!("OUT_DIR"), "/keccak.metallib")),
            metadata::KECCAK, keccak_neon, (3.5e-6, 15.0e-6))),
    }
}

/// Whether constraint evaluation (and rv32im accumulate) runs on these kernels: on by default,
/// `R0_METAL_FULL=0` keeps Codex's hybrid lane (constraints on the CPU).
pub fn full() -> bool {
    static FULL: OnceLock<bool> = OnceLock::new();
    *FULL.get_or_init(|| std::env::var("R0_METAL_FULL").map_or(true, |v| v != "0"))
}

/// All buffers must be quiescent before entry. Every command submitted here completes before
/// return, so the surrounding synchronous Metal HAL and the CPU kernels can safely reuse them.
pub fn evaluate(
    circuit: Circuit,
    check: &BufferImpl<Val>,
    groups: &[&BufferImpl<Val>],
    globals: &[&BufferImpl<Val>], // [mix, out], the HAL convention
    poly_mix: ExtVal,
    po2: usize,
    steps: usize,
) {
    let e = engine(circuit);
    let started = Instant::now();
    let domain = steps * 4;
    assert_eq!(steps, 1 << po2);
    assert_eq!(check.size(), domain * 4);
    let fit = (SCRATCH_BYTES / (4 * e.slots.max(1))).max(1024);
    let max_tile = std::env::var("R0_METAL_TILE").ok().and_then(|t| t.parse().ok()).unwrap_or(TILE);
    let tile = domain.min(max_tile).min(1 << fit.ilog2());
    let acc = e.device.new_buffer((tile * 16) as u64, MTLResourceOptions::StorageModePrivate);
    let scratch = e.device.new_buffer((e.slots.max(1) * tile * 4) as u64, MTLResourceOptions::StorageModePrivate);
    let options = MTLResourceOptions::StorageModeShared;
    let mut power = ExtVal::ONE;
    let poly_mix_pows: Vec<_> = (0..=e.max_power).map(|_| { let p = power; power *= poly_mix; p }).collect();
    let powers = e.device.new_buffer_with_data(poly_mix_pows.as_ptr() as *const c_void,
        std::mem::size_of_val(poly_mix_pows.as_slice()) as u64, options);
    let rou = Val::ROU_FWD[po2 + 2];
    let divisor_values: [Val; 4] = std::array::from_fn(|i| ((Val::new(3) * rou.pow(i)).pow(steps) - Val::ONE).inv());
    let divisors = e.device.new_buffer_with_data(divisor_values.as_ptr() as *const c_void, 16, options);
    // Buffer indices as in the generated kernels: 1-3 groups, 4 out, 5 mix, 7 check.
    let args = [(1, groups[0].as_arg()), (2, groups[1].as_arg()), (3, groups[2].as_arg()),
        (4, globals[1].as_arg()), (5, globals[0].as_arg()), (7, check.as_arg())];
    // Bosphorus: the CPU (NEON) evaluates the top of the domain while the GPU does the rest, split
    // by the rates measured on earlier calls. `R0_NEON_SHARE` (0..1) fixes the CPU's share.
    // With another proof in flight (two provers sharing the GPU), the CPU has the other prover's
    // work to do: GPU only.
    let alone = risc0_zkp::hal::metal::live_hals() <= 1;
    let (gpu_rate, cpu_rate) = *e.rates.lock().unwrap().get(&po2).unwrap_or(&e.guess);
    let share = std::env::var("R0_NEON_SHARE").ok().and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(if alone { gpu_rate / (gpu_rate + cpu_rate) } else { 0.0 }).clamp(0.0, 1.0);
    let split = (domain - ((domain as f64 * share) as usize / CPU_CHUNK * CPU_CHUNK).min(domain)).min(domain);
    let mut commands = Vec::new();
    for start in (0..split).step_by(tile) {
        let command = e.queue.new_command_buffer();
        let cfg = [domain as u32, start as u32, tile as u32, (split - start).min(tile) as u32];
        for pipeline in &e.pipelines {
            // Separate encoders order each stage's read-modify-write of `acc` and the scratch
            // values it reads from earlier stages.
            let encoder = command.new_compute_command_encoder();
            encoder.set_compute_pipeline_state(pipeline);
            encoder.set_buffer(0, Some(&acc), 0);
            for (i, arg) in &args {
                let KernelArg::Buffer { buffer, offset } = arg else { unreachable!() };
                encoder.set_buffer(*i, Some(buffer), *offset);
            }
            encoder.set_buffer(6, Some(&powers), 0);
            encoder.set_bytes(8, 16, cfg.as_ptr() as *const c_void);
            encoder.set_buffer(9, Some(&divisors), 0);
            encoder.set_buffer(10, Some(&scratch), 0);
            encoder.dispatch_threads(MTLSize::new(cfg[3] as u64, 1, 1),
                MTLSize::new(256.min(pipeline.max_total_threads_per_threadgroup()), 1, 1));
            encoder.end_encoding();
        }
        // Hazard tracking orders the tiles' uses of `acc` and `scratch`; no wait in between.
        command.commit();
        commands.push(command.to_owned());
    }
    let ptr = |b: &BufferImpl<Val>| { let mut p = std::ptr::null(); b.view(|s| p = s.as_ptr() as *const u32); p as usize };
    let (g0, g1, g2, gl0, gl1) = (ptr(groups[0]), ptr(groups[1]), ptr(groups[2]), ptr(globals[1]), ptr(globals[0]));
    let check_ptr = ptr(check);
    let pm: Vec<u32> = poly_mix_pows.iter().flat_map(|p| p.elems().iter().map(|x| x.as_u32_montgomery())).collect();
    // Per power: its plain-form words and their Shoup factors floor(c * 2^31 / P) (see neon.h).
    let pmu: Vec<u32> = poly_mix_pows.iter().flat_map(|p| {
        let plain: Vec<u32> = p.elems().iter().map(|x| x.as_u32()).collect();
        let shoup: Vec<u32> = plain.iter().map(|&c| (((c as u64) << 31) / 2013265921) as u32).collect();
        plain.into_iter().chain(shoup)
    }).collect();
    let div: Vec<u32> = divisor_values.iter().map(|x| x.as_u32_montgomery()).collect();
    let next = AtomicUsize::new(split);
    let cpu_done = Mutex::new(0.0f64);
    let threads = if split < domain {
        std::env::var("R0_NEON_THREADS").ok().and_then(|v| v.parse().ok())
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(8, |n| n.get()))
    } else {
        0
    };
    let gpu_seconds = std::thread::scope(|scope| {
        for _ in 0..threads {
            let (next, pm, pmu, div, cpu_done) = (&next, &pm, &pmu, &div, &cpu_done);
            scope.spawn(move || {
                loop {
                    let start = next.fetch_add(CPU_CHUNK, Ordering::Relaxed);
                    if start >= domain { break; }
                    let end = (start + CPU_CHUNK).min(domain);
                    // SAFETY: the buffers are quiescent shared memory of the asserted sizes; each work
                    // item writes only its own points of `check`, which the GPU doesn't touch.
                    unsafe {
                        (e.neon)(g0 as *const u32, g1 as *const u32, g2 as *const u32, gl0 as *const u32,
                            gl1 as *const u32, pm.as_ptr(), pmu.as_ptr(), div.as_ptr(), check_ptr as *mut u32,
                            domain as u32, start as u32, end as u32);
                    }
                }
                let mut done = cpu_done.lock().unwrap();
                *done = done.max(started.elapsed().as_secs_f64());
            });
        }
        for command in &commands {
            command.wait_until_completed();
            assert_eq!(command.status(), MTLCommandBufferStatus::Completed, "constraint GPU command failed");
        }
        started.elapsed().as_secs_f64()
    });
    let cpu_seconds = *cpu_done.lock().unwrap();
    let total = started.elapsed().as_secs_f64();
    if alone {
        let mut all = e.rates.lock().unwrap();
        let rates = all.entry(po2).or_insert(e.guess);
        if split > 0 { rates.0 = 0.5 * rates.0 + 0.5 * gpu_seconds / split as f64; }
        if split < domain { rates.1 = 0.5 * rates.1 + 0.5 * cpu_seconds / (domain - split) as f64; }
    }
    if risc0_zkp::timeline::enabled() {
        eprintln!("[metal] {circuit:?} constraints po2={po2}: {total:.3}s (GPU {gpu_seconds:.3}s, CPU {:.0}% of points {cpu_seconds:.3}s)",
            100.0 * (domain - split) as f64 / domain as f64);
    }
}

struct Accum { queue: CommandQueue, pipeline: ComputePipelineState }

fn accum_kernel() -> &'static Accum {
    static ACCUM: OnceLock<Accum> = OnceLock::new();
    ACCUM.get_or_init(|| {
        let started = Instant::now();
        let device = Device::system_default().expect("Metal device");
        let library = device.new_library_with_data(include_bytes!(concat!(env!("OUT_DIR"), "/rv32im_accum.metallib")))
            .expect("accum metallib");
        let function = library.get_function("rv32im_step_accum", None).expect("accum kernel");
        let pipeline = device.new_compute_pipeline_state_with_function(&function).expect("accum pipeline");
        eprintln!("[metal] rv32im accum kernel initialized in {:.3}s ({} threads per threadgroup)",
            started.elapsed().as_secs_f64(), pipeline.max_total_threads_per_threadgroup());
        Accum { queue: device.new_command_queue(), pipeline }
    })
}

/// Builds every pipeline now: ~60 s on the M1 the first time after a build or an install, ~0.1 s
/// once macOS has cached them. A long-lived host pays the first time before the user's first
/// proof. No-op without a Metal device.
pub fn warm_up() {
    if Device::system_default().is_none() {
        return;
    }
    for circuit in [Circuit::Rv32im, Circuit::Recursion, Circuit::Keccak] {
        engine(circuit);
    }
    accum_kernel();
}

/// Phase 1 of rv32im's accumulate step (risc0_circuit_rv32im_cpu_accum) on the GPU: risc0's
/// generated `step_TopAccum` for cycles `0..cycles`, ported from CUDA (kernels/rv32im_accum.metal).
/// Buffers are column-major with `rows` rows (`global` and `mix` have one). Phases 2 and 3 (the
/// prefix sums) are the caller's.
pub fn rv32im_accum(data: &BufferImpl<Val>, accum: &BufferImpl<Val>, global: &BufferImpl<Val>, mix: &BufferImpl<Val>,
                    rows: usize, cycles: usize) {
    let a = accum_kernel();
    let command = a.queue.new_command_buffer();
    let encoder = command.new_compute_command_encoder();
    encoder.set_compute_pipeline_state(&a.pipeline);
    for (i, buffer) in [data, accum, global, mix].into_iter().enumerate() {
        let KernelArg::Buffer { buffer, offset } = buffer.as_arg() else { unreachable!() };
        encoder.set_buffer(i as u64, Some(buffer), offset);
    }
    let cfg = [rows as u32, cycles as u32];
    encoder.set_bytes(4, 8, cfg.as_ptr() as *const c_void);
    encoder.dispatch_threads(MTLSize::new(cycles as u64, 1, 1),
        MTLSize::new(64.min(a.pipeline.max_total_threads_per_threadgroup()), 1, 1));
    encoder.end_encoding();
    command.commit();
    command.wait_until_completed();
    assert_eq!(command.status(), MTLCommandBufferStatus::Completed, "accum GPU command failed");
}
