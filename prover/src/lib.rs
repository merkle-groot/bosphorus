//! Proves a private LEE transaction as LEZ v0.3.1's `execute_and_prove` does (same circuit,
//! same guest images, same succinct receipt), on a schedule of our own.
//!
//! LEZ proves each guest the moment it runs, then the circuit's segments, its keccak proof and 13
//! recursion proofs one after another, so the GPU waits through every CPU-only phase. Here every
//! guest and the circuit execute first (the circuit's `env::verify` only needs the guests'
//! claims), then all proofs run from one dependency graph on two threads that share the GPU: a
//! po2-19 segment runs alone or beside a small segment (it needs most of the 8 GB), anything else
//! in pairs, the job with the longest chain behind it first.

use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::{Condvar, Mutex},
    time::Instant,
};

use anyhow::{Context as _, anyhow, bail, ensure};
use lee::{
    AccountId, Cycles, PRIVACY_PRESERVING_CIRCUIT_ELF,
    error::{InvalidProgramBehaviorError, LeeError},
    native_token::{self, NATIVE_TOKEN_PROGRAM_ID},
    privacy_preserving_transaction::circuit::{
        Dependency, ProgramKind, ProgramWithDependencies, Proof, ProvingInput,
    },
    program::Program,
};
use lee_core::{
    PrivacyPreservingCircuitInput, PrivacyPreservingCircuitOutput, ProgramImageWitness, ProvenCall,
    ShadowProgramWitness,
    execution_state::{Backend, DeferPublicEffects, ExecutionState, RootCall},
    from_frame,
    program::{ApplyInput, ApplyOutput, GuestOutput, PlanInput, PlanOutput, ProgramEvent},
    to_frame,
};
use risc0_zkvm::{
    Assumption, AssumptionReceipt, CoprocessorCallback, ExecutorEnv, ExecutorEnvBuilder, ExecutorImpl,
    ExitCode, InnerReceipt, MaybePruned, Output, PreflightResults, ProveKeccakRequest, ProverOpts,
    Receipt, ReceiptClaim, Segment, SegmentReceipt, Session, SuccinctReceipt, Unknown,
    VerifierContext, get_prover_server,
    sha::{Digest, Digestible},
};

/// Drop-in for `lee::execute_and_prove`. Dev mode (fake proofs) goes straight to LEZ's.
pub fn execute_and_prove(
    input: ProvingInput,
    program_with_dependencies: &ProgramWithDependencies,
) -> Result<(PrivacyPreservingCircuitOutput, Proof), LeeError> {
    // `BOSPHORUS_LEZ_PROVE=1`: LEZ's own schedule, for comparisons.
    if ProverOpts::default().dev_mode() || std::env::var("BOSPHORUS_LEZ_PROVE").is_ok_and(|v| v == "1") {
        return lee::execute_and_prove(input, program_with_dependencies);
    }
    let ProvingInput { shard_selectors, signers, private_witnesses, instruction_data, dummy_inputs, ciphertext_padding } = input;
    let ProgramWithDependencies { self_account_id: initial_account_id, programs } = program_with_dependencies;

    let root = RootCall {
        program_account_id: *initial_account_id,
        shard_selectors,
        instruction_data,
        authorized_accounts: signers.into_iter().collect(),
    };
    let mut backend = Executor { programs, env_builder: ExecutorEnv::builder(), calls: Vec::new(), guests: Vec::new() };
    ExecutionState::initialize(root.clone(), &private_witnesses)?.run(&mut backend)?;
    let Executor { mut env_builder, calls, guests, .. } = backend;

    // As LEZ: every program actually invoked, claimed against its real bytecode identity.
    let mut program_image_witnesses = Vec::new();
    let mut shadow_program_witnesses = Vec::new();
    #[expect(clippy::iter_over_hash_type, reason = "witness order is not significant, as in LEZ")]
    for (account_id, Dependency { program, kind }) in programs {
        match kind {
            ProgramKind::Disclosed => program_image_witnesses
                .push(ProgramImageWitness::Disclosed { account_id: *account_id, image_id: program.id() }),
            ProgramKind::Undisclosed { program_header, membership_proof } => {
                program_image_witnesses.push(ProgramImageWitness::Undisclosed {
                    account_id: *account_id,
                    program_header: *program_header,
                    membership_proof: membership_proof.clone(),
                })
            }
            ProgramKind::Shadow => shadow_program_witnesses.push(ShadowProgramWitness { image_id: program.id() }),
        }
    }
    let circuit_input = PrivacyPreservingCircuitInput {
        root,
        private_witnesses,
        dummy_inputs,
        ciphertext_padding,
        program_image_witnesses,
        shadow_program_witnesses,
        calls,
    };
    env_builder.write_slice(&to_frame(&borsh::to_vec(&circuit_input)?));
    // The circuit's keccak proofs come here instead of into the session.
    let keccaks = Rc::new(RefCell::new(Keccaks::default()));
    env_builder.coprocessor_callback_ref(keccaks.clone());
    let env = env_builder.build().map_err(|e| LeeError::CircuitProvingError(e.to_string()))?;
    let circuit = ExecutorImpl::from_elf(env, PRIVACY_PRESERVING_CIRCUIT_ELF)
        .and_then(|mut executor| executor.run())
        .map_err(|e| LeeError::CircuitProvingError(e.to_string()))?;
    let requests = std::mem::take(&mut keccaks.borrow_mut().0);

    let receipt = prove(circuit, guests, requests).map_err(|e| LeeError::CircuitProvingError(format!("{e:#}")))?;
    let proof = Proof::from_inner(borsh::to_vec(&receipt.inner)?);
    let circuit_output: PrivacyPreservingCircuitOutput = borsh::from_slice(
        from_frame(&receipt.journal.bytes).ok_or_else(|| {
            LeeError::CircuitOutputDeserializationError("malformed circuit journal frame".to_owned())
        })?,
    )
    .map_err(|e| LeeError::CircuitOutputDeserializationError(e.to_string()))?;
    Ok((circuit_output, proof))
}

/// LEZ's proving backend, with each guest executed instead of proven: its claim goes to the
/// circuit as an assumption and its session is kept for the schedule.
struct Executor<'programs> {
    programs: &'programs HashMap<AccountId, Dependency>,
    env_builder: ExecutorEnvBuilder<'static>,
    calls: Vec<ProvenCall>,
    guests: Vec<Session>,
}

impl<'programs> Backend for Executor<'programs> {
    type Call = Option<(&'programs Program, ProvenCall)>;
    type Error = LeeError;
    type PublicEffects = DeferPublicEffects;

    fn plan(&mut self, input: &PlanInput, _execution: &ExecutionState<'_>) -> Result<(PlanOutput, Self::Call), LeeError> {
        let self_account_id = input.self_account_id;
        if self_account_id == NATIVE_TOKEN_PROGRAM_ID {
            let plan = native_token::plan(input.caller_account_id, &input.accounts, &input.instruction_data)
                .map_err(InvalidProgramBehaviorError::NativeTransferFailed)?;
            return Ok((plan, None));
        }
        let program = &self
            .programs
            .get(&self_account_id)
            .ok_or(InvalidProgramBehaviorError::UndeclaredProgramDependency { program_account_id: self_account_id })?
            .program;
        let journal = self.execute(program, |env| Program::write_plan_inputs(input, env))?;
        let plan = match decode(&journal)? {
            GuestOutput::Plan(plan) => plan,
            GuestOutput::Apply(_) => return Err(wrong_entrypoint("plan", "an apply")),
        };
        let proven = ProvenCall { plan: plan.clone(), private_apply_outputs: Vec::new() };
        Ok((plan, Some((program, proven))))
    }

    fn apply(&mut self, call: &mut Self::Call, input: &ApplyInput) -> Result<ApplyOutput, LeeError> {
        let Some((program, proven)) = call else {
            return Ok(native_token::apply_output(input).map_err(InvalidProgramBehaviorError::NativeTransferFailed)?);
        };
        let journal = self.execute(program, |env| Program::write_apply_inputs(input, env))?;
        let output = match decode(&journal)? {
            GuestOutput::Apply(output) => output,
            GuestOutput::Plan(_) => return Err(wrong_entrypoint("apply", "a plan")),
        };
        proven.private_apply_outputs.push(output.clone());
        Ok(output)
    }

    fn complete(&mut self, call: Self::Call, _events: Vec<ProgramEvent>, _execution: &ExecutionState<'_>) -> Result<(), LeeError> {
        self.calls.extend(call.map(|(_, proven)| proven));
        Ok(())
    }
}

impl Executor<'_> {
    fn execute(
        &mut self,
        program: &Program,
        write: impl FnOnce(&mut ExecutorEnvBuilder) -> Result<(), LeeError>,
    ) -> Result<Vec<u8>, LeeError> {
        let mut env_builder = ExecutorEnv::builder();
        write(&mut env_builder)?;
        let env = env_builder.build().map_err(|e| LeeError::ProgramProveFailed(e.to_string()))?;
        let session = ExecutorImpl::from_elf(env, program.elf())
            .and_then(|mut executor| executor.run())
            .map_err(|e| LeeError::ProgramProveFailed(e.to_string()))?;
        check_exit_code(session.exit_code, session.user_cycles)?;
        let claim = session.claim().map_err(|e| LeeError::ProgramProveFailed(e.to_string()))?;
        // What `add_assumption(receipt)` gives `env::verify`: the claim, with any control root.
        self.env_builder.add_assumption(AssumptionReceipt::Unresolved(Assumption {
            claim: claim.digest(),
            control_root: Digest::ZERO,
        }));
        let journal = session.journal.as_ref().map(|journal| journal.bytes.clone()).unwrap_or_default();
        self.guests.push(session);
        Ok(journal)
    }
}

// LEZ's `check_exit_code`, `decode_guest_output` and `wrong_entrypoint` (crate-private there).
fn check_exit_code(exit_code: ExitCode, cycles: Cycles) -> Result<(), LeeError> {
    match exit_code {
        ExitCode::Halted(0) => Ok(()),
        ExitCode::Halted(code) => Err(LeeError::ProgramExitedWithCode { code, cycles }),
        ExitCode::Paused(code) => Err(LeeError::ProgramProveFailed(format!("program paused with code {code}"))),
        _ => Err(LeeError::ProgramProveFailed(format!("unexpected exit {exit_code:?}"))),
    }
}

fn decode(journal: &[u8]) -> Result<GuestOutput, LeeError> {
    let payload = from_frame(journal)
        .ok_or_else(|| LeeError::ProgramExecutionFailed("malformed program journal frame".to_owned()))?;
    borsh::from_slice(payload).map_err(|e| LeeError::ProgramExecutionFailed(e.to_string()))
}

fn wrong_entrypoint(scheduled: &str, returned: &str) -> LeeError {
    LeeError::ProgramExecutionFailed(format!("scheduled a {scheduled} call but the program returned {returned} output"))
}

#[derive(Default)]
struct Keccaks(Vec<ProveKeccakRequest>);

impl CoprocessorCallback for Keccaks {
    fn prove_keccak(&mut self, request: ProveKeccakRequest) -> anyhow::Result<()> {
        self.0.push(request);
        Ok(())
    }
}

// The schedule.

enum Job {
    /// CPU only: the segment's trace.
    Preflight(usize),
    /// The segment proof (witness, commits, constraints, FRI), from its preflight.
    Segment(usize),
    Keccak(usize),
    Lift,
    Join,
    Resolve,
}

enum Product {
    Preflight(PreflightResults),
    Segment(SegmentReceipt),
    Claim(SuccinctReceipt<ReceiptClaim>),
    Unknown(SuccinctReceipt<Unknown>),
}

struct Node {
    job: Job,
    deps: Vec<usize>,
    /// Estimated seconds, for the critical-path order.
    cost: f64,
    /// Runs with no other GPU job but light ones (a po2-19 segment).
    exclusive: bool,
    /// Small enough to run beside an exclusive job (a segment up to po2 17, ~1 GB).
    light: bool,
    cpu_only: bool,
}

struct Work {
    segments: Vec<Segment>,
    /// For a session's last segment: the output its claim carries (journal, assumptions).
    outputs: Vec<Option<Output>>,
    requests: Vec<ProveKeccakRequest>,
    nodes: Vec<Node>,
}

impl Work {
    fn add(&mut self, job: Job, deps: Vec<usize>, cost: f64, exclusive: bool, cpu_only: bool) -> usize {
        let light = matches!(job, Job::Segment(_)) && !exclusive;
        self.nodes.push(Node { job, deps, cost, exclusive, light, cpu_only });
        self.nodes.len() - 1
    }

    /// Preflight, prove and lift each segment of `session`, joined left to right; returns the
    /// node with the session's succinct receipt.
    fn session(&mut self, session: &Session, assumptions: Vec<Assumption>) -> anyhow::Result<usize> {
        let count = session.segments.len();
        ensure!(count > 0, "session is empty");
        let mut joined = None;
        for (i, segment_ref) in session.segments.iter().enumerate() {
            let segment = segment_ref.resolve()?;
            // A po2-19 segment proof peaks at ~4 GB of GPU memory, a recursion proof at ~1.4 GB.
            let (po2, s) = (segment.po2(), self.segments.len());
            let output = (i + 1 == count).then(|| Output {
                journal: MaybePruned::Pruned(session.journal.as_ref().map(|j| j.digest()).unwrap_or(Digest::ZERO)),
                assumptions: assumptions.clone().into(),
            });
            self.segments.push(segment);
            self.outputs.push(output);
            let scale = (1u64 << po2) as f64 / (1u64 << 19) as f64;
            let preflight = self.add(Job::Preflight(s), vec![], 0.1 * scale, false, true);
            let proof = self.add(Job::Segment(s), vec![preflight], 7.5 * scale + 0.3, po2 >= 19, false);
            let lift = self.add(Job::Lift, vec![proof], 1.9, false, false);
            joined = Some(match joined {
                None => lift,
                Some(left) => self.add(Job::Join, vec![left, lift], 1.9, false, false),
            });
        }
        Ok(joined.unwrap())
    }
}

fn prove(circuit: Session, guests: Vec<Session>, requests: Vec<ProveKeccakRequest>) -> anyhow::Result<Receipt> {
    let journal = circuit.journal.clone().ok_or_else(|| anyhow!("circuit has no journal: {:?}", circuit.exit_code))?;
    ensure!(circuit.exit_code == ExitCode::Halted(0), "circuit exited with {:?}", circuit.exit_code);
    let mut work = Work { segments: vec![], outputs: vec![], requests, nodes: vec![] };
    let assumptions: Vec<Assumption> = circuit.assumptions.iter().map(|(a, _)| a.clone()).collect();
    let conditional = work.session(&circuit, assumptions.clone())?;
    let mut by_claim = HashMap::new();
    for guest in &guests {
        let claim = guest.claim()?.digest();
        let node = work.session(guest, vec![])?;
        by_claim.insert(claim, node);
    }
    ensure!(work.requests.len() <= 1, "{} keccak proofs need a union tree, not supported yet", work.requests.len());
    let keccak = (!work.requests.is_empty()).then(|| work.add(Job::Keccak(0), vec![], 2.0, false, false));
    // Resolve the assumptions in the order the circuit's claim lists them, as risc0 does.
    let mut resolved = conditional;
    for assumption in &assumptions {
        let receipt = match (by_claim.get(&assumption.claim), keccak) {
            (Some(&node), _) => node,
            (None, Some(node)) if work.requests[0].claim_digest == assumption.claim => node,
            _ => bail!("no proof for the circuit's assumption {assumption:?}"),
        };
        resolved = work.add(Job::Resolve, vec![resolved, receipt], 1.9, false, false);
    }
    let lanes = std::env::var("R0_LANES").ok().and_then(|v| v.parse().ok()).unwrap_or(2usize).max(1);
    let Product::Claim(mut final_receipt) = run(&work, lanes, resolved)? else { unreachable!() };
    // As LEZ's receipts (risc0's prove_with_opts): the final claim holds the journal itself.
    if let MaybePruned::Value(claim) = &mut final_receipt.claim
        && let MaybePruned::Value(Some(output)) = &mut claim.output
    {
        let full = MaybePruned::Value(journal.bytes.clone());
        ensure!(full.digest() == output.journal.digest(), "final claim's journal mismatch");
        output.journal = full;
    }
    Ok(Receipt::new(InnerReceipt::Succinct(final_receipt), journal.bytes))
}

struct State {
    started: Vec<bool>,
    products: Vec<Option<Product>>,
    finished: Vec<bool>,
    gpu_running: usize,
    /// GPU jobs running that aren't light.
    heavy_running: usize,
    exclusive_running: bool,
    error: Option<anyhow::Error>,
}

/// Runs every node on `lanes` threads; returns the product of `last`.
fn run(work: &Work, lanes: usize, last: usize) -> anyhow::Result<Product> {
    let n = work.nodes.len();
    // Longest estimated chain from each node to the end (nodes are added in dependency order).
    let mut height = vec![0.0f64; n];
    for i in (0..n).rev() {
        height[i] += work.nodes[i].cost;
        for &d in &work.nodes[i].deps {
            height[d] = height[d].max(height[i]);
        }
    }
    let state = Mutex::new(State {
        started: vec![false; n],
        products: (0..n).map(|_| None).collect(),
        finished: vec![false; n],
        gpu_running: 0,
        heavy_running: 0,
        exclusive_running: false,
        error: None,
    });
    let wake = Condvar::new();
    let origin = Instant::now();
    let timeline = std::env::var("R0_TL").is_ok_and(|v| v == "1");
    std::thread::scope(|scope| {
        for lane in 0..lanes {
            let (state, wake, height) = (&state, &wake, &height);
            scope.spawn(move || {
                let prover = match get_prover_server(&ProverOpts::succinct()) {
                    Ok(prover) => prover,
                    Err(e) => {
                        state.lock().unwrap().error.get_or_insert(e);
                        return wake.notify_all();
                    }
                };
                let ctx = VerifierContext::default();
                loop {
                    let mut s = state.lock().unwrap();
                    let (index, inputs) = loop {
                        if s.error.is_some() || s.finished.iter().all(|f| *f) {
                            return wake.notify_all();
                        }
                        let ready = (0..n)
                            .filter(|&i| !s.started[i] && work.nodes[i].deps.iter().all(|&d| s.finished[d]))
                            .filter(|&i| {
                                let node = &work.nodes[i];
                                node.cpu_only
                                    || (node.exclusive && s.heavy_running == 0 && s.gpu_running < lanes)
                                    || (node.light && s.gpu_running < lanes)
                                    || (!node.exclusive && !s.exclusive_running && s.gpu_running < lanes)
                            })
                            .max_by(|&a, &b| height[a].total_cmp(&height[b]));
                        match ready {
                            Some(i) => {
                                s.started[i] = true;
                                let node = &work.nodes[i];
                                if !node.cpu_only {
                                    s.gpu_running += 1;
                                    s.heavy_running += usize::from(!node.light);
                                    s.exclusive_running |= node.exclusive;
                                }
                                let inputs: Vec<Product> = node.deps.iter().map(|&d| s.products[d].take().unwrap()).collect();
                                break (i, inputs);
                            }
                            None => s = wake.wait(s).unwrap(),
                        }
                    };
                    drop(s);
                    let started = origin.elapsed().as_secs_f64();
                    let result = execute(work, prover.as_ref(), &ctx, index, inputs);
                    if timeline {
                        eprintln!("[sched {started:8.3} {:7.3}] lane {lane}: {}", origin.elapsed().as_secs_f64() - started,
                            describe(work, index));
                    }
                    let mut s = state.lock().unwrap();
                    let node = &work.nodes[index];
                    if !node.cpu_only {
                        s.gpu_running -= 1;
                        s.heavy_running -= usize::from(!node.light);
                        s.exclusive_running &= !node.exclusive;
                    }
                    match result {
                        Ok(product) => {
                            s.products[index] = Some(product);
                            s.finished[index] = true;
                        }
                        Err(e) => {
                            s.error.get_or_insert(e.context(describe(work, index)));
                        }
                    }
                    drop(s);
                    wake.notify_all();
                }
            });
        }
    });
    let mut s = state.into_inner().unwrap();
    if let Some(e) = s.error.take() {
        return Err(e);
    }
    Ok(s.products[last].take().unwrap())
}

fn describe(work: &Work, index: usize) -> String {
    match &work.nodes[index].job {
        Job::Preflight(s) => format!("preflight segment {s} (po2 {})", work.segments[*s].po2()),
        Job::Segment(s) => format!("prove segment {s} (po2 {})", work.segments[*s].po2()),
        Job::Keccak(r) => format!("keccak {r} (po2 {})", work.requests[*r].po2),
        Job::Lift => "lift".into(),
        Job::Join => "join".into(),
        Job::Resolve => "resolve".into(),
    }
}

fn execute(
    work: &Work,
    prover: &dyn risc0_zkvm::ProverServer,
    ctx: &VerifierContext,
    index: usize,
    mut inputs: Vec<Product>,
) -> anyhow::Result<Product> {
    let mut take = || inputs.remove(0);
    Ok(match &work.nodes[index].job {
        Job::Preflight(s) => Product::Preflight(prover.segment_preflight(&work.segments[*s])?),
        Job::Segment(s) => {
            let Product::Preflight(preflight) = take() else { unreachable!() };
            let mut receipt = prover.prove_segment_core(ctx, preflight)?;
            if let Some(output) = &work.outputs[*s] {
                // As risc0's prove_session: the last segment's claim carries the session output.
                let full = MaybePruned::Value(Some(output.clone()));
                ensure!(receipt.claim.output.digest() == full.digest(), "segment {s}: session output mismatch");
                receipt.claim.output = full;
            }
            Product::Segment(receipt)
        }
        Job::Keccak(r) => Product::Unknown(prover.prove_keccak(&work.requests[*r]).context("prove keccak")?),
        Job::Lift => {
            let Product::Segment(receipt) = take() else { unreachable!() };
            Product::Claim(prover.lift(&receipt)?)
        }
        Job::Join => {
            let (Product::Claim(left), Product::Claim(right)) = (take(), take()) else { unreachable!() };
            Product::Claim(prover.join(&left, &right)?)
        }
        Job::Resolve => {
            let Product::Claim(conditional) = take() else { unreachable!() };
            let assumption = match take() {
                Product::Claim(receipt) => receipt.into_unknown(),
                Product::Unknown(receipt) => receipt,
                _ => unreachable!(),
            };
            Product::Claim(prover.resolve(&conditional, &assumption)?)
        }
    })
}
