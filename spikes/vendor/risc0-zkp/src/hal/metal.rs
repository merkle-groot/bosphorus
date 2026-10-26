// Copyright 2025 RISC Zero, Inc.
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

use std::{
    collections::HashMap, ffi::c_void, fmt::Debug, marker::PhantomData, mem, slice,
    sync::{Mutex, OnceLock},
};

use metal::{
    Buffer as MetalBuffer, CommandBufferRef, CommandQueue, ComputeCommandEncoderRef,
    ComputePipelineDescriptor, ComputePipelineState, Device, Library, MTLArgumentBuffersTier,
    MTLResourceOptions, MTLSize,
};
use parking_lot::{ReentrantMutex, ReentrantMutexGuard};
use rayon::prelude::*;
use risc0_core::{
    field::{
        baby_bear::{BabyBear, BabyBearElem, BabyBearExtElem},
        Elem, ExtElem, RootsOfUnity,
    },
    scope,
};

use super::{tracker, Buffer, Hal};
use crate::{
    core::{
        digest::Digest,
        hash::{
            poseidon2::{self, Poseidon2HashSuite},
            sha::Sha256HashSuite,
            HashSuite,
        },
        log2_ceil,
    },
    FRI_FOLD,
};

const METAL_LIB: &[u8] = include_bytes!(env!("ZKP_METAL_PATH"));

const KERNEL_NAMES: &[&str] = &[
    "batch_expand",
    "eltwise_add_fp",
    "eltwise_copy_fp",
    "eltwise_mul_factor_fp",
    "eltwise_sum_fpext",
    "eltwise_zeroize_fp",
    "eltwise_zeroize_fpext",
    "fri_fold",
    "gather_sample",
    "mix_poly_coeffs",
    "multi_bit_reverse",
    "multi_ntt_fwd_step",
    "multi_ntt_rev_step",
    "multi_poly_eval",
    "poseidon2_fold",
    "poseidon2_rows",
    "sha_fold",
    "sha_rows",
    "zk_shift",
    "zk_shift_table",
    // ntt_fast.metal (Bosphorus): fused NTT passes, see fast_expand_evaluate.
    "ntt_fwd_low_0_6",
    "ntt_fwd_low_1_6",
    "ntt_fwd_low_2_6",
    "ntt_fwd_low_3_6",
    "ntt_fwd_low_4_6",
    "ntt_rev_low_6",
    "ntt_rev_lowd_6",
    "ntt_fwd_lowd_0_6",
    "ntt_fwd_lowd_1_6",
    "ntt_fwd_lowd_2_6",
    "ntt_fwd_lowd_3_6",
    "ntt_fwd_lowd_4_6",
    "ntt_fwd_high_1",
    "ntt_fwd_high_2",
    "ntt_fwd_high_3",
    "ntt_fwd_high_4",
    "ntt_fwd_high_5",
    "ntt_rev_high_1",
    "ntt_rev_high_2",
    "ntt_rev_high_3",
    "ntt_rev_high_4",
    "ntt_rev_high_5",
];

fn ntt_group() -> u64 {
    std::env::var("R0_NTT_GROUP").ok().and_then(|g| g.parse().ok()).unwrap_or(256)
}

/// Fused NTT: stages 1..NTT_LOW_BITS run in threadgroup memory, the rest NTT_MAX_R at a time.
const NTT_LOW_BITS: usize = 6;
const NTT_LOW_GROUP: u64 = 64;
const NTT_MAX_R: usize = 5;

/// Added for Bosphorus: Metal state shared by every HAL in the process (each recursion proof
/// makes a new HAL): the kernel library, compiled pipelines and NTT twiddle tables.
struct Shared {
    device: Device,
    library: Library,
    pipelines: Mutex<HashMap<String, ComputePipelineState>>,
    ntt_tables: Mutex<HashMap<(bool, usize), [MetalBuffer; 3]>>,
    /// 3^k for k < 1024 and 3^(1024 k) for k < 4096, for zk_shift_table.
    pow3: OnceLock<[MetalBuffer; 2]>,
}

fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(|| {
        let device = Device::system_default().expect("no device found");
        let library = device.new_library_with_data(METAL_LIB).unwrap();
        Shared {
            device,
            library,
            pipelines: Mutex::new(HashMap::new()),
            ntt_tables: Mutex::new(HashMap::new()),
            pow3: OnceLock::new(),
        }
    })
}

fn shared_pipeline(name: &str) -> ComputePipelineState {
    let shared = shared();
    let mut pipelines = shared.pipelines.lock().unwrap();
    if let Some(pipeline) = pipelines.get(name) {
        return pipeline.clone();
    }
    let function = shared.library.get_function(name, None).unwrap();
    let pipeline = shared.device.new_compute_pipeline_state_with_function(&function).unwrap();
    pipelines.insert(name.to_string(), pipeline.clone());
    pipeline
}

/// Added for Bosphorus: HALs alive in the process, i.e. proofs in flight.
static LIVE_HALS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many proofs hold a Metal HAL right now.
pub fn live_hals() -> usize {
    LIVE_HALS.load(std::sync::atomic::Ordering::Relaxed)
}

// The GPU becomes unstable as the number of concurrent provers grow.
fn singleton() -> &'static ReentrantMutex<()> {
    static ONCE: OnceLock<ReentrantMutex<()>> = OnceLock::new();
    ONCE.get_or_init(|| ReentrantMutex::new(()))
}

pub trait MetalHash {
    /// Create a hash implementation
    fn new(hal: &MetalHal<Self>) -> Self;

    /// Run the hash_fold function
    fn hash_fold(&self, hal: &MetalHal<Self>, io: &BufferImpl<Digest>, output_size: usize);

    /// Run the hash_rows function
    fn hash_rows(
        &self,
        hal: &MetalHal<Self>,
        output: &BufferImpl<Digest>,
        matrix: &BufferImpl<BabyBearElem>,
    );

    /// Return the HashSuite
    fn get_hash_suite(&self) -> &HashSuite<BabyBear>;
}

pub struct MetalHashSha256 {
    suite: HashSuite<BabyBear>,
}

impl MetalHash for MetalHashSha256 {
    fn new(_hal: &MetalHal<Self>) -> Self {
        MetalHashSha256 {
            suite: Sha256HashSuite::new_suite(),
        }
    }

    fn hash_fold(&self, hal: &MetalHal<Self>, io: &BufferImpl<Digest>, output_size: usize) {
        let args = &[
            io.as_arg_with_offset(output_size),
            io.as_arg_with_offset(output_size * 2),
        ];
        hal.dispatch_by_name("sha_fold", args, output_size as u64);
    }

    fn hash_rows(
        &self,
        hal: &MetalHal<Self>,
        output: &BufferImpl<Digest>,
        matrix: &BufferImpl<BabyBearElem>,
    ) {
        let row_size = output.size();
        let col_size = matrix.size() / output.size();
        assert_eq!(matrix.size(), col_size * row_size);
        let args = &[
            output.as_arg(),
            matrix.as_arg(),
            KernelArg::Integer(row_size as u32),
            KernelArg::Integer(col_size as u32),
        ];
        hal.dispatch_by_name("sha_rows", args, row_size as u64);
    }

    fn get_hash_suite(&self) -> &HashSuite<BabyBear> {
        &self.suite
    }
}

pub struct MetalHashPoseidon2 {
    suite: HashSuite<BabyBear>,
    round_constants: BufferImpl<BabyBearElem>,
    m_int_diag: BufferImpl<BabyBearElem>,
}

impl MetalHash for MetalHashPoseidon2 {
    fn new(hal: &MetalHal<Self>) -> Self {
        let round_constants =
            hal.copy_from_elem("round_constants", poseidon2::consts::ROUND_CONSTANTS);
        let m_int_diag = hal.copy_from_elem("m_int_diag", poseidon2::consts::M_INT_DIAG_HZN);
        MetalHashPoseidon2 {
            suite: Poseidon2HashSuite::new_suite(),
            round_constants,
            m_int_diag,
        }
    }

    fn hash_fold(&self, hal: &MetalHal<Self>, io: &BufferImpl<Digest>, output_size: usize) {
        let args = &[
            self.round_constants.as_arg(),
            self.m_int_diag.as_arg(),
            io.as_arg_with_offset(output_size),
            io.as_arg_with_offset(output_size * 2),
        ];
        hal.dispatch_by_name("poseidon2_fold", args, output_size as u64);
    }

    fn hash_rows(
        &self,
        hal: &MetalHal<Self>,
        output: &BufferImpl<Digest>,
        matrix: &BufferImpl<BabyBearElem>,
    ) {
        let row_size = output.size();
        let col_size = matrix.size() / output.size();
        assert_eq!(matrix.size(), col_size * row_size);
        let args = &[
            self.round_constants.as_arg(),
            self.m_int_diag.as_arg(),
            output.as_arg(),
            matrix.as_arg(),
            KernelArg::Integer(row_size as u32),
            KernelArg::Integer(col_size as u32),
        ];
        hal.dispatch_by_name("poseidon2_rows", args, row_size as u64);
    }

    fn get_hash_suite(&self) -> &HashSuite<BabyBear> {
        &self.suite
    }
}

#[derive(Debug)]
pub struct MetalHal<Hash: MetalHash + ?Sized> {
    pub device: Device,
    pub cmd_queue: CommandQueue,
    kernels: HashMap<String, ComputePipelineDescriptor>,
    hash: Option<Box<Hash>>,
    /// Bosphorus: held only with `R0_METAL_SERIAL=1`. Two provers may share the GPU (each HAL has
    /// its own command queue; the shared state above is locked).
    _lock: Option<ReentrantMutexGuard<'static, ()>>,
}

pub type MetalHalSha256 = MetalHal<MetalHashSha256>;
pub type MetalHalPoseidon2 = MetalHal<MetalHashPoseidon2>;

#[derive(Clone, Debug)]
struct TrackedBuffer(MetalBuffer);

impl TrackedBuffer {
    pub fn new(buffer: MetalBuffer) -> Self {
        tracker().lock().unwrap().alloc(buffer.length() as usize);
        Self(buffer)
    }
}

impl Drop for TrackedBuffer {
    fn drop(&mut self) {
        tracker().lock().unwrap().free(self.0.length() as usize);
    }
}

#[derive(Clone, Debug)]
pub struct BufferImpl<T> {
    name: &'static str,
    cmd_queue: CommandQueue,
    buffer: TrackedBuffer,
    offset: usize,
    size: usize,
    marker: PhantomData<T>,
}

/// An NTT table, shared by every HAL.
struct TableArg(MetalBuffer);

impl TableArg {
    fn as_arg(&self) -> KernelArg<'_> {
        KernelArg::Buffer { buffer: &self.0, offset: 0 }
    }
}

pub enum KernelArg<'a> {
    Null,
    Buffer {
        buffer: &'a MetalBuffer,
        offset: u64,
    },
    Integer(u32),
}

impl<T> BufferImpl<T> {
    pub fn new(name: &'static str, device: &Device, cmd_queue: CommandQueue, size: usize) -> Self {
        let bytes_len = size * mem::size_of::<T>();
        // TODO: detect if device supports shared mode
        let options = MTLResourceOptions::StorageModeShared;
        let buffer = device.new_buffer(bytes_len as u64, options);
        Self {
            name,
            cmd_queue,
            buffer: TrackedBuffer::new(buffer),
            offset: 0,
            size,
            marker: PhantomData,
        }
    }

    pub fn copy_from(
        name: &'static str,
        device: &Device,
        cmd_queue: CommandQueue,
        slice: &[T],
    ) -> Self {
        let bytes_len = mem::size_of_val(slice);
        // TODO: detect if device supports shared mode
        let options = MTLResourceOptions::StorageModeShared;
        let buffer =
            device.new_buffer_with_data(slice.as_ptr() as *const c_void, bytes_len as u64, options);
        Self {
            name,
            cmd_queue,
            buffer: TrackedBuffer::new(buffer),
            offset: 0,
            size: slice.len(),
            marker: PhantomData,
        }
    }

    pub fn as_arg(&self) -> KernelArg<'_> {
        let offset = self.offset * mem::size_of::<T>();
        KernelArg::Buffer {
            buffer: &self.buffer.0,
            offset: offset as u64,
        }
    }

    pub fn as_arg_with_offset(&self, offset: usize) -> KernelArg<'_> {
        let offset = (self.offset + offset) * mem::size_of::<T>();
        KernelArg::Buffer {
            buffer: &self.buffer.0,
            offset: offset as u64,
        }
    }

    // fn sync(&self) {
    //     let cmd_buffer = self.cmd_queue.new_command_buffer();
    //     let blit_encoder = cmd_buffer.new_blit_command_encoder();
    //     blit_encoder.synchronize_resource(&self.buffer.0);
    //     blit_encoder.end_encoding();
    //     cmd_buffer.commit();
    //     cmd_buffer.wait_until_completed();
    // }

    pub fn as_device_ptr(&self) -> *mut c_void {
        self.buffer.0.gpu_address() as *mut c_void
    }

    pub fn as_ptr(&self) -> *mut c_void {
        self.buffer.0.contents()
    }

    pub fn as_buf(&self) -> MetalBuffer {
        self.buffer.0.clone()
    }
}

impl<T: Clone> Buffer<T> for BufferImpl<T> {
    fn name(&self) -> &'static str {
        self.name
    }

    fn size(&self) -> usize {
        self.size
    }

    fn slice(&self, offset: usize, size: usize) -> BufferImpl<T> {
        assert!(offset + size <= self.size());
        BufferImpl {
            name: self.name,
            cmd_queue: self.cmd_queue.clone(),
            buffer: self.buffer.clone(),
            offset: self.offset + offset,
            size,
            marker: PhantomData,
        }
    }

    fn get_at(&self, idx: usize) -> T {
        // TODO: detect if device supports synchronize_resource
        // self.sync();
        let ptr = self.buffer.0.contents() as *const T;
        let len = self.buffer.0.length() as usize / mem::size_of::<T>();
        let slice = unsafe { slice::from_raw_parts(ptr, len) };
        slice[self.offset + idx].clone()
    }

    fn view<F: FnOnce(&[T])>(&self, f: F) {
        // TODO: detect if device supports synchronize_resource
        // self.sync();
        let ptr = self.buffer.0.contents() as *const T;
        let len = self.buffer.0.length() as usize / mem::size_of::<T>();
        let slice = unsafe { slice::from_raw_parts(ptr, len) };
        f(&slice[self.offset..self.offset + self.size]);
    }

    fn view_mut<F: FnOnce(&mut [T])>(&self, f: F) {
        // TODO: detect if device supports synchronize_resource
        // self.sync();
        let ptr = self.buffer.0.contents() as *mut T;
        let len = self.buffer.0.length() as usize / mem::size_of::<T>();
        let slice = unsafe { slice::from_raw_parts_mut(ptr, len) };
        f(&mut slice[self.offset..self.offset + self.size]);
        // TODO: detect if device supports did_modify_range
        // let offset = self.offset * mem::size_of::<T>();
        // let size = self.size * mem::size_of::<T>();
        // self.buffer
        //     .0
        //     .did_modify_range(NSRange::new(offset as u64, size as u64));
    }

    fn to_vec(&self) -> Vec<T> {
        let ptr = self.buffer.0.contents() as *const T;
        let len = self.buffer.0.length() as usize / mem::size_of::<T>();
        let slice = unsafe { slice::from_raw_parts(ptr, len) };
        Vec::from(slice)
    }
}

impl<MH: MetalHash + ?Sized> Drop for MetalHal<MH> {
    fn drop(&mut self) {
        LIVE_HALS.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

impl<MH: MetalHash> Default for MetalHal<MH> {
    fn default() -> Self {
        Self::new()
    }
}

impl<MH: MetalHash> MetalHal<MH> {
    pub fn new() -> Self {
        let lock = std::env::var("R0_METAL_SERIAL").is_ok_and(|v| v == "1").then(|| singleton().lock());
        LIVE_HALS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let device = shared().device.clone();
        assert_eq!(
            device.argument_buffers_support(),
            MTLArgumentBuffersTier::Tier2
        );
        let library = &shared().library;
        let cmd_queue = device.new_command_queue();
        let mut kernels = HashMap::new();
        for name in KERNEL_NAMES {
            let function = library.get_function(name, None).unwrap();
            let pipeline = ComputePipelineDescriptor::new();
            pipeline.set_compute_function(Some(&function));
            kernels.insert(name.to_string(), pipeline);
        }
        let mut hal = Self {
            device,
            cmd_queue,
            kernels,
            hash: None,
            _lock: lock,
        };
        hal.hash = Some(Box::new(MH::new(&hal)));
        hal
    }

    fn pipeline(&self, name: &str) -> ComputePipelineState {
        shared_pipeline(name)
    }

    /// Encodes one dispatch of `threads` threads into `cmd` (its own encoder, so passes order).
    fn encode_threads(&self, cmd: &CommandBufferRef, name: &str, args: &[KernelArg], threads: u64, group: u64) {
        let pipeline = self.pipeline(name);
        let encoder = cmd.new_compute_command_encoder();
        encoder.set_compute_pipeline_state(&pipeline);
        for (index, arg) in args.iter().enumerate() {
            match arg {
                KernelArg::Buffer { buffer, offset } => encoder.set_buffer(index as u64, Some(buffer), *offset),
                KernelArg::Integer(value) => encoder.set_bytes(
                    index as u64,
                    mem::size_of_val(value) as u64,
                    value.to_le_bytes().as_ptr() as *const c_void,
                ),
                KernelArg::Null => encoder.set_buffer(index as u64, None, 0),
            }
        }
        let group = group.min(pipeline.max_total_threads_per_threadgroup());
        encoder.dispatch_threads(MTLSize::new(threads, 1, 1), MTLSize::new(group, 1, 1));
        encoder.end_encoding();
    }

    /// Per (forward, n_bits): root[n]^x for x < N/2, root[s]^j for the low stages, root[q+1].
    fn with_ntt_tables<R>(&self, fwd: bool, n_bits: usize, f: impl FnOnce(&[TableArg; 3]) -> R) -> R {
        let mut tables = shared().ntt_tables.lock().unwrap();
        if !tables.contains_key(&(fwd, n_bits)) {
            let roots = if fwd { BabyBearElem::ROU_FWD } else { BabyBearElem::ROU_REV };
            let half = 1usize << (n_bits - 1);
            let mut table = Vec::with_capacity(half);
            let mut x = BabyBearElem::ONE;
            for _ in 0..half {
                table.push(x);
                x *= roots[n_bits];
            }
            let mut low = Vec::with_capacity((1 << NTT_LOW_BITS) - 1);
            for s in 1..=NTT_LOW_BITS {
                let mut x = BabyBearElem::ONE;
                for _ in 0..1usize << (s - 1) {
                    low.push(x);
                    x *= roots[s];
                }
            }
            let q: Vec<_> = (0..NTT_MAX_R).map(|q| roots[q + 1]).collect();
            let options = MTLResourceOptions::StorageModeShared;
            let buffer = |v: &[BabyBearElem]| self.device.new_buffer_with_data(v.as_ptr() as *const c_void,
                mem::size_of_val(v) as u64, options);
            tables.insert((fwd, n_bits), [buffer(&table), buffer(&low), buffer(&q)]);
        }
        let args = tables[&(fwd, n_bits)].clone().map(TableArg);
        drop(tables);
        f(&args)
    }

    /// The fused path applies to columns of at least 2^12 points (whole threadgroups per column).
    fn fast_ntt(n_bits: usize) -> bool {
        n_bits >= NTT_LOW_BITS + 6 && std::env::var_os("R0_METAL_NTT_ORIG").is_none()
    }

    /// batch_expand + every multi_ntt_fwd_step stage, as ~n/5 passes in one command buffer.
    fn fast_expand_evaluate(&self, output: &BufferImpl<BabyBearElem>, input: &BufferImpl<BabyBearElem>,
                            count: usize, n_bits: usize, expand_bits: usize) {
        self.with_ntt_tables(true, n_bits, |[table, low, q]| {
            let cmd = self.cmd_queue.new_command_buffer();
            let threads = (count << (n_bits - NTT_LOW_BITS)) as u64;
            let low_kernel = if std::env::var_os("R0_NTT_LOW_TG").is_some() { "low" } else { "lowd" };
            self.encode_threads(cmd, &format!("ntt_fwd_{low_kernel}_{expand_bits}_{NTT_LOW_BITS}"),
                &[output.as_arg(), input.as_arg(), low.as_arg(), KernelArg::Integer(n_bits as u32)],
                threads, NTT_LOW_GROUP);
            let profile = std::env::var_os("R0_NTT_PROFILE").is_some();
            let mut cmd = cmd;
            let mut started = std::time::Instant::now();
            let lap = |cmd: &CommandBufferRef, started: std::time::Instant, what: String| -> std::time::Instant {
                if profile {
                    cmd.commit();
                    cmd.wait_until_completed();
                    eprintln!("[ntt] {what}: {:.1} ms", started.elapsed().as_secs_f64() * 1e3);
                }
                std::time::Instant::now()
            };
            if profile {
                started = lap(cmd, started, format!("low E={expand_bits} n={n_bits}"));
                cmd = self.cmd_queue.new_command_buffer();
            }
            let mut s = NTT_LOW_BITS + 1;
            while s <= n_bits {
                let r = NTT_MAX_R.min(n_bits - s + 1);
                self.encode_threads(cmd, &format!("ntt_fwd_high_{r}"),
                    &[output.as_arg(), table.as_arg(), q.as_arg(), KernelArg::Integer(n_bits as u32),
                      KernelArg::Integer(s as u32)],
                    (count << (n_bits - r)) as u64, ntt_group());
                if profile {
                    started = lap(cmd, started, format!("high s={s} r={r}"));
                    cmd = self.cmd_queue.new_command_buffer();
                }
                s += r;
            }
            cmd.commit();
            cmd.wait_until_completed();
        });
    }

    /// Every multi_ntt_rev_step stage plus the 1/N scaling, as ~n/5 passes in one command buffer.
    fn fast_interpolate(&self, io: &BufferImpl<BabyBearElem>, count: usize, n_bits: usize) {
        let norm = self.copy_from_elem("norm", &[BabyBearElem::new(1u32 << n_bits).inv()]);
        self.with_ntt_tables(false, n_bits, |[table, low, q]| {
            let cmd = self.cmd_queue.new_command_buffer();
            let mut top = n_bits;
            while top > NTT_LOW_BITS {
                let r = NTT_MAX_R.min(top - NTT_LOW_BITS);
                let s = top - r + 1;
                self.encode_threads(cmd, &format!("ntt_rev_high_{r}"),
                    &[io.as_arg(), table.as_arg(), q.as_arg(), KernelArg::Integer(n_bits as u32),
                      KernelArg::Integer(s as u32)],
                    (count << (n_bits - r)) as u64, ntt_group());
                top = s - 1;
            }
            let low_kernel = if std::env::var_os("R0_NTT_LOW_TG").is_some() { "low" } else { "lowd" };
            self.encode_threads(cmd, &format!("ntt_rev_{low_kernel}_{NTT_LOW_BITS}"),
                &[io.as_arg(), low.as_arg(), norm.as_arg(), KernelArg::Integer(n_bits as u32)],
                (count << (n_bits - NTT_LOW_BITS)) as u64, NTT_LOW_GROUP);
            cmd.commit();
            cmd.wait_until_completed();
        });
    }

    pub fn dispatch_by_name(&self, name: &str, args: &[KernelArg], count: u64) {
        let kernel = self.kernels.get(name).unwrap();
        self.dispatch(kernel, args, count, None);
    }

    pub fn dispatch(
        &self,
        kernel: &ComputePipelineDescriptor,
        args: &[KernelArg],
        count: u64,
        opts: Option<(MTLSize, MTLSize)>,
    ) {
        self.dispatch_with_resources(kernel, args, count, opts, |_| {});
    }

    pub fn dispatch_with_resources<F: Fn(&ComputeCommandEncoderRef)>(
        &self,
        kernel: &ComputePipelineDescriptor,
        args: &[KernelArg],
        count: u64,
        opts: Option<(MTLSize, MTLSize)>,
        callback: F,
    ) {
        let cmd_buffer = self.cmd_queue.new_command_buffer();
        let cmd_encoder = cmd_buffer.new_compute_command_encoder();

        callback(cmd_encoder);

        let pipeline_state = shared_pipeline(&kernel.compute_function().unwrap().name());

        cmd_encoder.set_compute_pipeline_state(&pipeline_state);
        for (index, arg) in args.iter().enumerate() {
            match arg {
                KernelArg::Buffer { buffer, offset } => {
                    cmd_encoder.set_buffer(index as u64, Some(buffer), *offset);
                }
                KernelArg::Integer(value) => {
                    cmd_encoder.set_bytes(
                        index as u64,
                        mem::size_of_val(value) as u64,
                        value.to_le_bytes().as_ptr() as *const c_void,
                    );
                }
                KernelArg::Null => {
                    cmd_encoder.set_buffer(index as u64, None, 0);
                }
            }
        }

        match opts {
            Some((thread_groups_count, threads_per_threadgroup)) => {
                cmd_encoder.dispatch_thread_groups(thread_groups_count, threads_per_threadgroup);
            }
            None => {
                let threads_per_grid = MTLSize::new(count, 1, 1);
                let threads_per_threadgroup =
                    MTLSize::new(pipeline_state.max_total_threads_per_threadgroup(), 1, 1);
                cmd_encoder.dispatch_threads(threads_per_grid, threads_per_threadgroup);
            }
        };

        cmd_encoder.end_encoding();

        cmd_buffer.commit();
        cmd_buffer.wait_until_completed();
    }
}

#[allow(unused_variables)]
impl<MH: MetalHash> Hal for MetalHal<MH> {
    type Elem = BabyBearElem;
    type ExtElem = BabyBearExtElem;
    type Field = BabyBear;
    type Buffer<T: Clone + Debug + PartialEq> = BufferImpl<T>;

    fn alloc_elem(&self, name: &'static str, size: usize) -> Self::Buffer<Self::Elem> {
        BufferImpl::new(name, &self.device, self.cmd_queue.clone(), size)
    }

    fn copy_from_elem(&self, name: &'static str, slice: &[Self::Elem]) -> Self::Buffer<Self::Elem> {
        BufferImpl::copy_from(name, &self.device, self.cmd_queue.clone(), slice)
    }

    fn alloc_extelem(&self, name: &'static str, size: usize) -> Self::Buffer<Self::ExtElem> {
        BufferImpl::new(name, &self.device, self.cmd_queue.clone(), size)
    }

    fn copy_from_extelem(
        &self,
        name: &'static str,
        slice: &[Self::ExtElem],
    ) -> Self::Buffer<Self::ExtElem> {
        BufferImpl::copy_from(name, &self.device, self.cmd_queue.clone(), slice)
    }

    fn alloc_digest(&self, name: &'static str, size: usize) -> Self::Buffer<Digest> {
        BufferImpl::new(name, &self.device, self.cmd_queue.clone(), size)
    }

    fn copy_from_digest(&self, name: &'static str, slice: &[Digest]) -> Self::Buffer<Digest> {
        BufferImpl::copy_from(name, &self.device, self.cmd_queue.clone(), slice)
    }

    fn alloc_u32(&self, name: &'static str, size: usize) -> Self::Buffer<u32> {
        BufferImpl::new(name, &self.device, self.cmd_queue.clone(), size)
    }

    fn copy_from_u32(&self, name: &'static str, slice: &[u32]) -> Self::Buffer<u32> {
        BufferImpl::copy_from(name, &self.device, self.cmd_queue.clone(), slice)
    }

    fn batch_bit_reverse(&self, io: &Self::Buffer<Self::Elem>, count: usize) {
        let row_size = io.size() / count;
        assert_eq!(row_size * count, io.size());
        let bits = log2_ceil(row_size);
        assert_eq!(row_size, 1 << bits);
        tracing::debug!(
            "io: {}, count: {count}, row_size: {row_size}, bits: {bits}",
            io.size()
        );
        let args = &[io.as_arg(), KernelArg::Integer(bits as u32)];
        self.dispatch_by_name("multi_bit_reverse", args, row_size as u64 * count as u64);
    }

    fn batch_expand_into_evaluate_ntt(
        &self,
        output: &Self::Buffer<Self::Elem>,
        input: &Self::Buffer<Self::Elem>,
        count: usize,
        expand_bits: usize,
    ) {
        scope!("batch_expand", {
            tracing::debug!(
                "output: {}, input: {}, count: {count}",
                output.size(),
                input.size()
            );
            let out_size = output.size() / count;
            let in_size = input.size() / count;
            let expand_bits = log2_ceil(out_size / in_size);
            assert_eq!(output.size(), out_size * count);
            assert_eq!(input.size(), in_size * count);
            assert_eq!(out_size, in_size * (1 << expand_bits));
            let n_bits = log2_ceil(out_size);
            if Self::fast_ntt(n_bits) && expand_bits <= 4 && n_bits < BabyBearElem::MAX_ROU_PO2 {
                return self.fast_expand_evaluate(output, input, count, n_bits, expand_bits);
            }
            let args = &[
                output.as_arg(),
                input.as_arg(),
                KernelArg::Integer(count as u32),
                KernelArg::Integer(out_size as u32),
                KernelArg::Integer(in_size as u32),
                KernelArg::Integer(expand_bits as u32),
            ];
            self.dispatch_by_name("batch_expand", args, out_size as u64);
        });

        scope!("batch_evaluate_ntt", {
            tracing::debug!(
                "output: {}, count: {count}, expand_bits: {expand_bits}",
                output.size()
            );
            let row_size = output.size() / count;
            assert_eq!(row_size * count, output.size());
            let n_bits = log2_ceil(row_size);
            assert_eq!(row_size, 1 << n_bits);
            assert!(n_bits >= expand_bits);
            assert!(n_bits < Self::Elem::MAX_ROU_PO2);
            let rou = self.copy_from_elem("rou", Self::Elem::ROU_FWD);
            let kernel = self.kernels.get("multi_ntt_fwd_step").unwrap();
            for s_bits in 1 + expand_bits..=n_bits {
                let args = &[
                    output.as_arg(),
                    rou.as_arg(),
                    KernelArg::Integer(n_bits as u32),
                    KernelArg::Integer(s_bits as u32),
                    KernelArg::Integer(count as u32),
                ];
                let params = compute_launch_params(n_bits as u32, s_bits as u32, count as u32);
                self.dispatch(kernel, args, count as u64, Some(params));
            }
        });
    }

    fn batch_interpolate_ntt(&self, io: &Self::Buffer<Self::Elem>, count: usize) {
        scope!("batch_interpolate_ntt");
        tracing::debug!("io: {}, count: {count}", io.size());
        let row_size = io.size() / count;
        assert_eq!(row_size * count, io.size());
        let n_bits = log2_ceil(row_size);
        assert_eq!(row_size, 1 << n_bits);
        assert!(n_bits < Self::Elem::MAX_ROU_PO2);
        if Self::fast_ntt(n_bits) {
            return self.fast_interpolate(io, count, n_bits);
        }

        let rou = self.copy_from_elem("rou", Self::Elem::ROU_REV);
        let kernel = self.kernels.get("multi_ntt_rev_step").unwrap();
        for s_bits in (1..=n_bits).rev() {
            let args = &[
                io.as_arg(),
                rou.as_arg(),
                KernelArg::Integer(n_bits as u32),
                KernelArg::Integer(s_bits as u32),
                KernelArg::Integer(count as u32),
            ];
            let params = compute_launch_params(n_bits as u32, s_bits as u32, count as u32);
            self.dispatch(kernel, args, count as u64, Some(params));
        }

        let norm = self.copy_from_elem("norm", &[Self::Elem::new(row_size as u32).inv()]);
        let args = &[io.as_arg(), norm.as_arg()];
        self.dispatch_by_name("eltwise_mul_factor_fp", args, io.size() as u64);
    }

    fn batch_evaluate_any(
        &self,
        coeffs: &Self::Buffer<Self::Elem>,
        poly_count: usize,
        which: &Self::Buffer<u32>,
        xs: &Self::Buffer<Self::ExtElem>,
        out: &Self::Buffer<Self::ExtElem>,
    ) {
        scope!("batch_evaluate_any");
        let po2 = log2_ceil(coeffs.size() / poly_count);
        let count = 1 << po2;
        assert_eq!(poly_count * count, coeffs.size());
        let eval_count = which.size();
        assert_eq!(xs.size(), eval_count);
        assert_eq!(out.size(), eval_count);
        let args = &[
            out.as_arg(),
            coeffs.as_arg(),
            which.as_arg(),
            xs.as_arg(),
            KernelArg::Integer(count as u32),
        ];
        let kernel = self.kernels.get("multi_poly_eval").unwrap();
        let params = simple_launch_params(out.size() as u32 * 256, 256);
        self.dispatch(kernel, args, count as u64, Some(params));
    }

    fn eltwise_add_elem(
        &self,
        output: &Self::Buffer<Self::Elem>,
        input1: &Self::Buffer<Self::Elem>,
        input2: &Self::Buffer<Self::Elem>,
    ) {
        assert_eq!(output.size(), input1.size());
        assert_eq!(output.size(), input2.size());
        let count = output.size() as u64;
        let args = &[output.as_arg(), input1.as_arg(), input2.as_arg()];
        self.dispatch_by_name("eltwise_add_fp", args, count);
    }

    fn eltwise_sum_extelem(
        &self,
        output: &Self::Buffer<Self::Elem>,
        input: &Self::Buffer<Self::ExtElem>,
    ) {
        let count = output.size() / Self::ExtElem::EXT_SIZE;
        let to_add = input.size() / count;
        assert_eq!(output.size(), count * Self::ExtElem::EXT_SIZE);
        assert_eq!(input.size(), count * to_add);
        let args = &[
            output.as_arg(),
            input.as_arg(),
            KernelArg::Integer(count as u32),
            KernelArg::Integer(to_add as u32),
        ];
        self.dispatch_by_name("eltwise_sum_fpext", args, count as u64);
    }

    fn eltwise_copy_elem(
        &self,
        output: &Self::Buffer<Self::Elem>,
        input: &Self::Buffer<Self::Elem>,
    ) {
        assert_eq!(output.size(), input.size());
        let count = output.size() as u64;
        let args = &[output.as_arg(), input.as_arg()];
        self.dispatch_by_name("eltwise_copy_fp", args, count);
    }

    fn fri_fold(
        &self,
        output: &Self::Buffer<Self::Elem>,
        input: &Self::Buffer<Self::Elem>,
        mix: &Self::ExtElem,
    ) {
        let count = output.size() / Self::ExtElem::EXT_SIZE;
        assert_eq!(output.size(), count * Self::ExtElem::EXT_SIZE);
        assert_eq!(input.size(), output.size() * FRI_FOLD);
        let mix = self.copy_from_extelem("mix", &[*mix]);
        let args = &[
            output.as_arg(),
            input.as_arg(),
            mix.as_arg(),
            KernelArg::Integer(count as u32),
        ];
        self.dispatch_by_name("fri_fold", args, count as u64);
    }

    fn mix_poly_coeffs(
        &self,
        output: &Self::Buffer<Self::ExtElem>,
        mix_start: &Self::ExtElem,
        mix: &Self::ExtElem,
        input: &Self::Buffer<Self::Elem>,
        combos: &Self::Buffer<u32>,
        input_size: usize,
        count: usize,
    ) {
        tracing::debug!(
            "output: {}, input: {}, combos: {}, input_size: {input_size}, count: {count}",
            output.size(),
            input.size(),
            combos.size(),
        );

        let mix_start = self.copy_from_extelem("mix_start", &[*mix_start]);
        let mix = self.copy_from_extelem("mix", &[*mix]);
        let args = &[
            output.as_arg(),
            input.as_arg(),
            combos.as_arg(),
            mix_start.as_arg(),
            mix.as_arg(),
            KernelArg::Integer(input_size as u32),
            KernelArg::Integer(count as u32),
        ];
        self.dispatch_by_name("mix_poly_coeffs", args, count as u64);
    }

    fn hash_fold(&self, io: &Self::Buffer<Digest>, input_size: usize, output_size: usize) {
        assert_eq!(input_size, 2 * output_size);
        self.hash.as_ref().unwrap().hash_fold(self, io, output_size);
    }

    fn hash_rows(&self, output: &Self::Buffer<Digest>, matrix: &Self::Buffer<Self::Elem>) {
        self.hash.as_ref().unwrap().hash_rows(self, output, matrix);
    }

    fn zk_shift(&self, io: &Self::Buffer<Self::Elem>, poly_count: usize) {
        let bits = log2_ceil(io.size() / poly_count);
        let count = io.size();
        assert_eq!(io.size(), poly_count * (1 << bits));
        if bits > 22 {
            let args = &[io.as_arg(), KernelArg::Integer(bits as u32)];
            return self.dispatch_by_name("zk_shift", args, count as u64);
        }
        // Bosphorus: 3^k from two tables instead of an exponentiation per element.
        let [lo, hi] = shared().pow3.get_or_init(|| {
            let powers = |base: BabyBearElem, n: usize| {
                let mut x = BabyBearElem::ONE;
                let table: Vec<_> = (0..n).map(|_| { let p = x; x *= base; p }).collect();
                self.device.new_buffer_with_data(table.as_ptr() as *const c_void, mem::size_of_val(table.as_slice()) as u64,
                    MTLResourceOptions::StorageModeShared)
            };
            let three = BabyBearElem::new(3);
            [powers(three, 1024), powers(three.pow(1024), 4096)]
        });
        let args = &[io.as_arg(), KernelArg::Integer(bits as u32), KernelArg::Buffer { buffer: lo, offset: 0 },
            KernelArg::Buffer { buffer: hi, offset: 0 }];
        self.dispatch_by_name("zk_shift_table", args, count as u64);
    }

    fn gather_sample(
        &self,
        dst: &Self::Buffer<Self::Elem>,
        src: &Self::Buffer<Self::Elem>,
        idx: usize,
        size: usize,
        stride: usize,
    ) {
        let args = &[
            dst.as_arg(),
            src.as_arg(),
            KernelArg::Integer(idx as u32),
            KernelArg::Integer(stride as u32),
        ];
        self.dispatch_by_name("gather_sample", args, size as u64);
    }

    fn has_unified_memory(&self) -> bool {
        self.device.has_unified_memory()
    }

    fn get_hash_suite(&self) -> &HashSuite<Self::Field> {
        self.hash.as_ref().unwrap().get_hash_suite()
    }

    #[cfg(feature = "metal_prefix_products")]
    fn prefix_products(&self, io: &Self::Buffer<Self::ExtElem>) {
        let block_size = 256;
        let grain_size = 16;
        let threadgroup_size = block_size * grain_size;
        let io_size = io.size() as u64;
        let threadgroups = (io_size + threadgroup_size - 1) / threadgroup_size;
        let threadgroups_per_grid = MTLSize::new(threadgroups, 1, 1);
        let threads_per_threadgroup = MTLSize::new(block_size, 1, 1);
        let params = Params::ThreadGroups(threadgroups_per_grid, threads_per_threadgroup);

        let partial_products = self.alloc_extelem("partial_products", block_size as usize);

        let kernel = self.kernels.get("reduce").unwrap();
        let args = &[partial_products.as_arg(), io.as_arg()];
        self.dispatch(kernel, args, 0, Some(params.clone()));

        // partial_products.view(|view| {
        //     println!("partial_products");
        //     let items: Vec<_> = view.iter().enumerate().collect();
        //     for (i, x) in items {
        //         println!("{i}: {x:?}");
        //     }
        // });

        let kernel = self.kernels.get("prefix_products_single").unwrap();
        let args = &[partial_products.as_arg(), partial_products.as_arg()];
        self.dispatch(kernel, args, 0, Some(params.clone()));

        // partial_products.view(|view| {
        //     println!("partial_products");
        //     let items: Vec<_> = view.iter().enumerate().collect();
        //     for (i, x) in items {
        //         println!("{i}: {x:?}");
        //     }
        // });

        let kernel = self.kernels.get("prefix_products").unwrap();
        let args = &[io.as_arg(), partial_products.as_arg()];
        self.dispatch(kernel, args, 0, Some(params));
    }

    #[cfg(not(feature = "metal_prefix_products"))]
    fn prefix_products(&self, io: &Self::Buffer<Self::ExtElem>) {
        io.view_mut(|io| {
            for i in 1..io.len() {
                io[i] *= io[i - 1];
            }
        });
    }

    fn eltwise_zeroize_elem(&self, elems: &Self::Buffer<Self::Elem>) {
        elems.view_mut(|slice| {
            slice.par_iter_mut().for_each(|elem| {
                *elem = elem.valid_or_zero();
            });
        })
    }

    fn scatter(
        &self,
        into: &Self::Buffer<Self::Elem>,
        index: &[u32],
        offsets: &[u32],
        values: &[Self::Elem],
    ) {
        into.view_mut(|into| {
            for cycle in 0..index.len() - 1 {
                for idx in index[cycle]..index[cycle + 1] {
                    into[offsets[idx as usize] as usize] = values[idx as usize];
                }
            }
        });
    }

    fn eltwise_copy_elem_slice(
        &self,
        into: &Self::Buffer<Self::Elem>,
        from: &[Self::Elem],
        from_rows: usize,
        from_cols: usize,
        from_offset: usize,
        from_stride: usize,
        into_offset: usize,
        into_stride: usize,
    ) {
        into.view_mut(|into| {
            for row in 0..from_rows {
                for col in 0..from_cols {
                    into[into_offset + row * into_stride + col] =
                        from[from_offset + row * from_stride + col];
                }
            }
        });
    }
}

fn simple_launch_params(count: u32, threads_per_group: u32) -> (MTLSize, MTLSize) {
    let groups = count.div_ceil(threads_per_group);
    (
        MTLSize::new(groups.into(), 1, 1),
        MTLSize::new(threads_per_group.into(), 1, 1),
    )
}

fn compute_launch_params(n_bits: u32, s_bits: u32, c_size: u32) -> (MTLSize, MTLSize) {
    let s_size = 1 << (s_bits - 1);
    let g_size = 1 << (n_bits - s_bits);

    let mut grid = MTLSize::new(1, 1, 1);
    let mut block = MTLSize::new(1, 1, 1);

    let mut threads = 128;
    // First thread over S
    block.width = threads.min(s_size);
    threads /= block.width;
    // Next thread over G
    block.height = threads.min(g_size);
    // Don't bother threading over C
    let mut grids = 32;
    // First grid over S
    grid.width = grids.min(s_size / block.width);
    grids /= grid.width;
    // Next grid over G
    grid.height = grids.min(g_size / block.height);
    grids /= grid.height;
    // Next grid over C
    grid.depth = grids.min(c_size as u64);
    (grid, block)
}

#[cfg(test)]
mod tests {
    use test_log::test;

    use super::{MetalHalPoseidon2, MetalHalSha256};
    use crate::hal::testutil;

    #[test]
    fn batch_bit_reverse() {
        testutil::batch_bit_reverse(MetalHalSha256::new());
    }

    #[test]
    fn batch_evaluate_any() {
        testutil::batch_evaluate_any(MetalHalSha256::new());
    }

    #[test]
    fn batch_expand_into_evaluate_ntt() {
        testutil::batch_expand_into_evaluate_ntt(MetalHalSha256::new());
    }

    #[test]
    fn batch_interpolate_ntt() {
        testutil::batch_interpolate_ntt(MetalHalSha256::new());
    }

    #[test]
    #[should_panic]
    fn check_req() {
        testutil::check_req(MetalHalSha256::new());
    }

    #[test]
    fn eltwise_add_fp() {
        testutil::eltwise_add_elem(MetalHalSha256::new());
    }

    #[test]
    fn eltwise_copy_fp() {
        testutil::eltwise_copy_elem(MetalHalSha256::new());
    }

    #[test]
    fn eltwise_sum_extelem() {
        testutil::eltwise_sum_extelem(MetalHalSha256::new());
    }

    #[test]
    fn fri_fold() {
        testutil::fri_fold(MetalHalSha256::new());
    }

    #[test]
    fn mix_poly_coeffs() {
        testutil::mix_poly_coeffs(MetalHalSha256::new());
    }

    #[test]
    fn hash_fold_sha256() {
        testutil::hash_fold(MetalHalSha256::new());
    }

    #[test]
    fn hash_rows_sha256() {
        testutil::hash_rows(MetalHalSha256::new());
    }

    #[test]
    fn hash_fold_poseidon2() {
        testutil::hash_fold(MetalHalPoseidon2::new());
    }

    #[test]
    fn hash_rows_poseidon2() {
        testutil::hash_rows(MetalHalPoseidon2::new());
    }

    #[test]
    fn slice() {
        testutil::slice(MetalHalSha256::new());
    }

    #[test]
    fn zk_shift() {
        testutil::zk_shift(MetalHalSha256::new());
    }

    #[test]
    fn gather_sample() {
        testutil::gather_sample(MetalHalSha256::new());
    }

    #[test]
    fn prefix_products() {
        use crate::hal::{cpu::CpuHal, dual::DualHal, Hal as _};
        use risc0_core::field::baby_bear::BabyBearExtElem;
        use std::rc::Rc;

        fn test(size: usize) {
            let hal_gpu = MetalHalSha256::new();
            let hal_cpu = CpuHal::new(hal_gpu.get_hash_suite().clone());
            let hal = DualHal::new(Rc::new(hal_cpu), Rc::new(hal_gpu));

            use risc0_core::field::Elem as _;
            let mut rng = rand::rng();
            let io: Vec<_> = (0..size)
                .map(|_| BabyBearExtElem::random(&mut rng))
                .collect();
            let io = hal.copy_from_extelem("io", io.as_slice());

            hal.prefix_products(&io);
        }

        for i in 15..=20 {
            println!("po2: {i}");
            test(1 << i);
        }
    }
}
