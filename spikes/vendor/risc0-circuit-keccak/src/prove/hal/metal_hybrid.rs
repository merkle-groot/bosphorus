// Modified for Bosphorus (2026-10-25): hybrid Metal proving. See NOTICE.
// Copyright 2024 RISC Zero, Inc.
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

use std::rc::Rc;

use anyhow::Result;
use rayon::{iter::IntoParallelIterator, prelude::*};
use risc0_circuit_keccak_sys::{
    risc0_circuit_keccak_cpu_poly_fp, risc0_circuit_keccak_cpu_witgen, RawBuffer, RawExecBuffers,
    RawPreflightTrace, ScatterInfo,
};
use risc0_core::scope;
use risc0_sys::ffi_wrap;
use risc0_zkp::{
    core::log2_ceil,
    field::{map_pow, Elem as _, ExtElem as _, RootsOfUnity as _},
    hal::{
        metal::{BufferImpl as MetalBuffer, MetalHalPoseidon2},
        AccumPreflight, Buffer, CircuitHal,
    },
    INV_RATE,
};

use crate::{
    prove::{KeccakProver, KeccakProverImpl, GLOBAL_MIX, GLOBAL_OUT},
    zirgen::{
        circuit::{ExtVal, Val, REGISTER_GROUP_ACCUM, REGISTER_GROUP_DATA},
        info::POLY_MIX_POWERS,
    },
};

use super::{
    CircuitWitnessGenerator, ForwardPreflightOrder, MetaBuffer, PreflightCycleOrder,
    PreflightTrace, StepMode,
};

// Added for Bosphorus. risc0-zkp 3.0.4 uses synchronous dispatch and shared buffers.
// as_ptr() ignores offsets: reject slices before passing a buffer to the CPU kernels.
fn checked_base_ptr(buf: &MetalBuffer<Val>) -> *const Val {
    let base = buf.as_ptr() as *const Val;
    buf.view(|slice| assert_eq!(base, slice.as_ptr(), "hybrid keccak requires an unsliced buffer"));
    base
}

fn host_slice(buf: &MetalBuffer<Val>) -> &[Val] {
    // SAFETY: the allocation is shared CPU/GPU memory and remains alive for this borrow.
    // Every Metal dispatch waits for completion, and the caller only reads this slice.
    unsafe { std::slice::from_raw_parts(checked_base_ptr(buf), buf.size()) }
}

#[derive(Default)]
pub struct MetalCircuitHal;

impl CircuitWitnessGenerator<MetalHalPoseidon2> for MetalCircuitHal {
    type PreferredPreflightOrder = ForwardPreflightOrder;

    fn scatter_preflight(
        &self,
        into: &MetaBuffer<MetalHalPoseidon2>,
        infos: &[ScatterInfo],
        data: &[u32],
    ) -> Result<()> {
        scope!("scatter");
        into.buf.view_mut(|into_slice| {
        for info in infos {
            let inner_count = 32 / info.bits;
            let mask: u32 = (1 << info.bits) - 1;
            for i in 0..info.count as u32 {
                let from_idx = info.offset + (i / inner_count);
                let word = data[from_idx as usize];
                let j = i % inner_count;
                let val = (word >> (j * info.bits)) & mask;
                let col = info.col as u32 + i;
                let into_idx = col as usize * into.rows + info.row as usize;
                into_slice[into_idx] = val.into();
            }
        }
        });
        Ok(())
    }

    fn generate_witness<O: PreflightCycleOrder>(
        &self,
        mode: StepMode,
        preflight: &PreflightTrace<O>,
        global: &MetaBuffer<MetalHalPoseidon2>,
        data: &MetaBuffer<MetalHalPoseidon2>,
    ) -> Result<()> {
        scope!("witgen");

        let cycles = preflight.cycle;
        assert_eq!(cycles, data.rows);
        tracing::debug!("witgen: {cycles}");

        let buffers = RawExecBuffers {
            global: RawBuffer {
                buf: checked_base_ptr(&global.buf),
                rows: global.rows,
                cols: global.cols,
                checked_reads: global.checked_reads,
            },
            data: RawBuffer {
                buf: checked_base_ptr(&data.buf),
                rows: data.rows,
                cols: data.cols,
                checked_reads: data.checked_reads,
            },
        };
        let run_order: Vec<u32> = preflight
            .cycles
            .values()
            .flatten()
            .map(|c| c.cycle)
            .collect();
        let preimage_idx: Vec<u32> = preflight
            .cycles
            .values()
            .flatten()
            .map(|c| c.preimage_idx)
            .collect();
        let preflight = RawPreflightTrace {
            preimages: preflight.preimages.as_ptr(),
            preimages_count: preflight.preimages.len() as u32,
            preimage_idxs: preimage_idx.as_ptr(),
            run_order: run_order.as_ptr(),
        };
        ffi_wrap(|| unsafe {
            risc0_circuit_keccak_cpu_witgen(mode as u32, &buffers, &preflight, cycles as u32)
        })
    }
}

impl CircuitHal<MetalHalPoseidon2> for MetalCircuitHal {
    fn accumulate(
        &self,
        _preflight: &AccumPreflight,
        _ctrl: &MetalBuffer<Val>,
        _io: &MetalBuffer<Val>,
        _data: &MetalBuffer<Val>,
        _mix: &MetalBuffer<Val>,
        _accum: &MetalBuffer<Val>,
        _steps: usize,
    ) {
    }

    fn eval_check(
        &self,
        check: &MetalBuffer<Val>,
        groups: &[&MetalBuffer<Val>],
        globals: &[&MetalBuffer<Val>],
        poly_mix: ExtVal,
        po2: usize,
        steps: usize,
    ) {
        if risc0_metal::full() {
            risc0_metal::evaluate(risc0_metal::Circuit::Keccak, check, groups, globals, poly_mix, po2, steps);
            return;
        }

        scope!("eval_check");

        let check_size = check.size();
        let check_ptr = checked_base_ptr(check) as usize;
        let accum = host_slice(groups[REGISTER_GROUP_ACCUM]);
        let data = host_slice(groups[REGISTER_GROUP_DATA]);
        let mix = host_slice(globals[GLOBAL_MIX]);
        let out = host_slice(globals[GLOBAL_OUT]);

        tracing::debug!(
            "check: {}, data: {}, accum: {}, mix: {} out: {}",
            check_size,
            data.len(),
            accum.len(),
            mix.len(),
            out.len()
        );

        const EXP_PO2: usize = log2_ceil(INV_RATE);
        let domain = steps * INV_RATE;
        let poly_mix_pows = map_pow(poly_mix, POLY_MIX_POWERS);

        // SAFETY: Convert a borrow of a cell into a raw const slice so that we can pass
        // it over the thread boundary. This should be safe because the scope of the
        // usage is within this function and each thread access will not overlap with
        // each other.

        let data = unsafe { std::slice::from_raw_parts(data.as_ptr(), data.len()) };
        let out = unsafe { std::slice::from_raw_parts(out.as_ptr(), out.len()) };
        let poly_mix_pows = poly_mix_pows.as_slice();

        (0..domain).into_par_iter().for_each(|cycle| {
            let args = &[data.as_ptr(), out.as_ptr()];
            let mut tot = ExtVal::ZERO;
            unsafe {
                risc0_circuit_keccak_cpu_poly_fp(
                    cycle,
                    domain,
                    poly_mix_pows.as_ptr(),
                    args.as_ptr(),
                    &mut tot,
                )
            };
            let x = Val::ROU_FWD[po2 + EXP_PO2].pow(cycle);
            // TODO: what is this magic number 3?
            let y = (Val::new(3) * x).pow(1 << po2);
            let ret = tot * (y - Val::new(1)).inv();

            // SAFETY: GPU commands have completed, buffers stay alive, and each cycle
            // writes only its own four indices. No overlapping mutable Rust slices are made.
            for i in 0..ExtVal::EXT_SIZE {
                unsafe { (check_ptr as *mut Val).add(i * domain + cycle).write(ret.elems()[i]); }
            }
        });
    }
}

pub fn keccak_prover() -> Result<Box<dyn KeccakProver>> {
    tracing::debug!("keccak: Metal generic operations, CPU circuit kernels");
    let hal = Rc::new(MetalHalPoseidon2::new());
    let circuit_hal = Rc::new(MetalCircuitHal);
    Ok(Box::new(KeccakProverImpl { hal, circuit_hal }))
}
