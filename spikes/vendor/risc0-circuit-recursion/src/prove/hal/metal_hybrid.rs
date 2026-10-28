// Modified for Bosphorus (2026-10-25): hybrid Metal proving. See NOTICE.
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

use std::rc::Rc;

use anyhow::Result;
use risc0_circuit_recursion_sys::{
    risc0_circuit_recursion_cpu_accum, risc0_circuit_recursion_cpu_eval_check,
    risc0_circuit_recursion_cpu_witgen, RawAccumBuffers, RawExecBuffers, RawPreflightTrace,
    StepMode,
};
use risc0_sys::ffi_wrap;
use risc0_zkp::{
    core::log2_ceil,
    field::{
        baby_bear::{BabyBearElem, BabyBearExtElem},
        map_pow, RootsOfUnity as _,
    },
    hal::{metal::{BufferImpl as MetalBuffer, MetalHalPoseidon2}, AccumPreflight, Buffer, CircuitHal},
    INV_RATE,
};

use crate::{
    prove::{RecursionProver, RecursionProverImpl},
    GLOBAL_MIX, GLOBAL_OUT, REGISTER_GROUP_ACCUM, REGISTER_GROUP_CTRL, REGISTER_GROUP_DATA,
};

use super::{CircuitAccumulator, CircuitWitnessGenerator};

// Added for Bosphorus: reuse the unchanged CPU circuit kernels over Metal shared memory.
// risc0-zkp is pinned to 3.0.4: every dispatch waits for GPU completion before returning.
// Its as_ptr() ignores slice offsets, so require base allocations at every CPU handoff.
fn checked_base_ptr(buf: &MetalBuffer<BabyBearElem>) -> *const BabyBearElem {
    let base = buf.as_ptr() as *const BabyBearElem;
    buf.view(|slice| assert_eq!(base, slice.as_ptr(), "hybrid recursion requires an unsliced buffer"));
    base
}

type MetalHal = MetalHalPoseidon2;

#[derive(Default)]
pub(crate) struct MetalCircuitHal;

impl CircuitWitnessGenerator<MetalHal> for MetalCircuitHal {
    fn generate_witness(
        &self,
        mode: StepMode,
        total_cycles: u32,
        preflight: &RawPreflightTrace,
        ctrl: &MetalBuffer<BabyBearElem>,
        data: &MetalBuffer<BabyBearElem>,
        global: &MetalBuffer<BabyBearElem>,
    ) -> Result<()> {
        let ctrl = checked_base_ptr(ctrl);
        let data = checked_base_ptr(data);
        let global = checked_base_ptr(global);
        let buffers = RawExecBuffers {
            ctrl,
            data,
            global,
        };
        ffi_wrap(|| unsafe {
            risc0_circuit_recursion_cpu_witgen(mode, &buffers, preflight, total_cycles)
        })
    }
}

impl CircuitAccumulator<MetalHal> for MetalCircuitHal {
    fn accumulate(
        &self,
        work_cycles: u32,
        total_cycles: u32,
        ctrl: &MetalBuffer<BabyBearElem>,
        global: &MetalBuffer<BabyBearElem>,
        data: &MetalBuffer<BabyBearElem>,
        mix: &MetalBuffer<BabyBearElem>,
        accum: &MetalBuffer<BabyBearElem>,
    ) -> Result<()> {
        let ctrl = checked_base_ptr(ctrl);
        let global = checked_base_ptr(global);
        let data = checked_base_ptr(data);
        let mix = checked_base_ptr(mix);
        let accum = checked_base_ptr(accum);
        let buffers = RawAccumBuffers {
            ctrl,
            global,
            data,
            mix,
            accum,
        };
        ffi_wrap(|| unsafe {
            risc0_circuit_recursion_cpu_accum(&buffers, work_cycles, total_cycles)
        })
    }
}

impl CircuitHal<MetalHal> for MetalCircuitHal {
    fn eval_check(
        &self,
        check: &MetalBuffer<BabyBearElem>,
        groups: &[&MetalBuffer<BabyBearElem>],
        globals: &[&MetalBuffer<BabyBearElem>],
        poly_mix: BabyBearExtElem,
        po2: usize,
        steps: usize,
    ) {
        if risc0_metal::full() {
            risc0_metal::evaluate(risc0_metal::Circuit::Recursion, check, groups, globals, poly_mix, po2, steps);
            return;
        }

        const EXP_PO2: usize = log2_ceil(INV_RATE);
        let domain = steps * INV_RATE;

        let ctrl = checked_base_ptr(groups[REGISTER_GROUP_CTRL]);
        let data = checked_base_ptr(groups[REGISTER_GROUP_DATA]);
        let accum = checked_base_ptr(groups[REGISTER_GROUP_ACCUM]);
        let mix = checked_base_ptr(globals[GLOBAL_MIX]);
        let global = checked_base_ptr(globals[GLOBAL_OUT]);
        let check = checked_base_ptr(check);

        let poly_mix_pows = map_pow(poly_mix, crate::info::POLY_MIX_POWERS);
        let poly_mix_pows = poly_mix_pows.as_slice();

        let rou = BabyBearElem::ROU_FWD[po2 + EXP_PO2];

        let buffers = RawAccumBuffers {
            ctrl,
            global,
            data,
            mix,
            accum,
        };

        ffi_wrap(|| unsafe {
            risc0_circuit_recursion_cpu_eval_check(
                &buffers,
                poly_mix_pows.as_ptr(),
                check,
                rou,
                po2 as u32,
                domain as u32,
            )
        })
        .unwrap();
    }

    #[allow(unused)]
    fn accumulate(
        &self,
        _preflight: &AccumPreflight,
        ctrl: &MetalBuffer<BabyBearElem>,
        io: &MetalBuffer<BabyBearElem>,
        data: &MetalBuffer<BabyBearElem>,
        mix: &MetalBuffer<BabyBearElem>,
        accum: &MetalBuffer<BabyBearElem>,
        steps: usize,
    ) {
        unimplemented!()
    }
}

pub(crate) fn recursion_prover() -> Result<Box<dyn RecursionProver>> {
    tracing::debug!("recursion: Metal generic operations, CPU circuit kernels");
    let hal = Rc::new(MetalHalPoseidon2::new());
    let circuit_hal = Rc::new(MetalCircuitHal);
    Ok(Box::new(RecursionProverImpl::new(hal, circuit_hal)))
}
