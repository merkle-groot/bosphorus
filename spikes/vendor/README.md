# GPU proving on Apple Silicon (vendored risc0 crates)

Status 2026-10-25 (late evening), **uncommitted**, on top of `d2a5d27`. First goal: a private op
proven in under a minute on the M1 Air (8 GB); then the user asked for under 30 s, and after the
floor analysis below, for the ~40–45 s that was reachable. A Codex session started this (Metal
lanes, first GPU constraint kernels); Claude took it from 128.6 s. Every change is host-side: the
LEZ circuit and the receipt format are untouched, and every kernel below matches risc0's CPU code
bit for bit (checked with `metal_eval_bench` or against the CPU kernels in a real proof). **But our
own guest images did change: see "Guest image ids drifted".**

## Result

All proofs verified and settled in-process. Times with Metal kernels already compiled (macOS caches
them; the first run after a build pays 30–130 s of kernel compilation once).

| Step | S4 tx (spike guest, native token) | Real borrow (lending + token guests) |
|---|---:|---:|
| CPU only, in-process (morning baseline; the borrow is P4's `r0vm` run) | 382.8 s | 618 s |
| Codex: Metal for NTT/Merkle/FRI ("hybrid") | 237.0 s | |
| Codex: GPU constraint kernels (`R0_METAL_FULL=1`) | 128.6 s | |
| Constraint kernels regenerated: flat sum, 400-op kernels, shared values stored | 88.9 s | |
| po2-19 segments (peak memory 9.8 → 5.2 GB) | 70.9 s | |
| Fused NTT (fewer memory passes) | 56.1 s | 79.0 s |
| Poseidon2 row hashing: register-resident sponge, ×2/×4 as adds; storage threshold 16 | 55.2 s | 75.8 s |
| 3-multiply Montgomery product for risc0's Metal `Fp` | 52.5 s | 72.6 s |
| NTT low passes without threadgroup memory, bigger constraint tiles | 49.0 s | 67.6–68.4 s |
| Same build, re-measured with the stage timeline (`R0_TL=1`) | | 65.7 s |
| All-zero groups committed without NTTs or row hashing; recursion control group cached per program | | 64.2 s |
| `eval_u` with ~5× fewer multiplies, `zk_shift` from tables, witness dropped once committed, half the constraint scratch | | 61.8 s |
| NEON constraint evaluator for the CPU's share of the domain | 41.5 s | ~62 s |
| rv32im accumulate on the GPU (ported from risc0's CUDA) | 38.6 s | 54.6 s |
| Our proving schedule (`bosphorus-prover`): execute everything, then prove on two lanes | 36.0 s | 47.8 s |
| Shoup products for Poseidon2's diagonal; NEON split rates per po2, CPU share off beside another proof | **35.9 s** | **46.9–47.1 s** |

Checked after the last row:
- **A stock verifier accepts the proofs.** `spikes/stockverify` (its own workspace, crates.io
  risc0-zkvm 3.0.5 with stock risc0-zkp 3.0.5 and risc0-circuit-recursion 4.0.5, none of our
  patches) verifies an S4 proof from this build against lee v0.3.1's circuit image id and rejects
  the same proof with one journal bit flipped.
- **Same receipt format as LEZ's path:** same seal (55,667 words for S4), and the final claim holds
  the journal unpruned as risc0's `prove_with_opts` leaves it: tx 226,559 (S4) and 227,845
  (borrow) bytes, as through `lee::execute_and_prove`.
- The essential tests pass in dev mode (`RISC0_DEV_MODE=1 cargo test --release -p bosphorus-core
  -p bosphorus-methods`: 1 + 13 core, 10 lending, 1 oracle, 4 private, 5 token).
- Not done: the `bos` wallet path still goes through LEZ's wallet, i.e. LEZ's
  `execute_and_prove` and its serial schedule (see "Resume here").

## Guest image ids drifted

The workspace `[patch.crates-io]` entries for `risc0-zkp` and `risc0-circuit-recursion` (added by
Codex earlier on 2026-10-25) also apply to our guests, which are workspace members and compile both
crates. With no change to the token guest's source, its image id went from the deployed
`9226815c…f734` (`DEPLOYMENTS.md`, build of 2026-10-21) to `3945d780…2aea`; lending changed too
(E9 had changed it already). In-process tests don't care; proofs for the deployed testnet programs
do. Before deploying or proving on the testnet, build the guests without the patches: guests out of
the workspace with their own lockfile, or `programs/methods/build.rs` embedding pinned ELFs (e.g.
`.testnet/elfs/`), or a deploy build with the patch entries removed. **Done 2026-10-26: pinned
ELFs** for token and oracle (`programs/methods/deployed/`, ids match `DEPLOYMENTS.md`); lending
is built (E9 changed it) and needs a redeploy. LEZ's circuit is unaffected:
its guest builds in LEZ's own workspace, and its image id is the same in our build and in
`~/Work/logos-execution-zone/target` (checked).

## The floor

The proof's shape is fixed by LEZ: 3 guest proofs (lending plan po2 17, token plan and private
apply po2 16), 3 circuit segments at po2 19, a keccak proof and 13 recursion proofs at po2 18 (lift
×7, join ×2, resolve ×4: one lift and one resolve per guest and for keccak). Two stages are bound by
integer multiplies, and both run close to the GPU's limit:

| Stage | Work per borrow | At the GPU's peak (~42G Montgomery products/s) | Measured (serial run) |
|---|---|---:|---:|
| Constraint evaluation | rv32im ~64k products per point × 7.3M points, recursion ~14k × 13.6M | 16 s | 20.6 s |
| Poseidon2 Merkle trees | ~400M permutations × 1,356 products | 13 s | ~14 s |

So 30 s was out of reach: these two alone need ≥ 29 s, before NTTs (~7 s, memory-bound), the small
kernels and the CPU-only phases. The schedule now hides almost all CPU-only time (the borrow's GPU
work is ~46 s and the run takes ~47 s), so what's left is making GPU work cheaper.

## How `bosphorus-prover` proves (crate `prover/`)

`bosphorus_prover::execute_and_prove` is a drop-in for `lee::execute_and_prove` (same inputs, same
`(PrivacyPreservingCircuitOutput, Proof)`), used by the real-proof tests and the S4 harness under
their `prove` features. Dev mode and `BOSPHORUS_LEZ_PROVE=1` go to LEZ's.

1. **Execute everything first.** LEZ's backend runs with each guest *executed* (`ExecutorImpl`), not
   proven: its claim goes into the circuit's env as an unresolved assumption with control root
   zero, which is all `env::verify` asks for (the executor matches assumptions by claim digest).
   The circuit then executes with a coprocessor callback that collects its keccak request
   (`Session::pending_keccaks` is crate-private).
2. **One dependency graph.** Per segment: preflight (CPU), proof, lift; joins left to right per
   session; then resolves in the order the circuit's claim lists its assumptions (guest receipts by
   claim digest, keccak through `ProverServer::prove_keccak`). The last segment of each session
   gets the session output in its claim (journal digest and assumptions), as risc0's
   `prove_session` merges it; the final claim gets the journal itself.
3. **Two lanes sharing the GPU.** A po2-19 segment (~4 GB) runs with at most a smaller segment
   beside it, never with a recursion proof (~1.4 GB); any other job runs in pairs; a free lane takes
   the ready job with the longest estimated chain behind it. Each lane has its own `ProverServer`,
   and the Metal HAL's process-wide lock is gone (`R0_METAL_SERIAL=1` brings it back).

The borrow's trace (`R0_TL=1` prints `[sched start length] lane N: job`): 0–26.8 s the three po2-19
segments one after another, the three guest segments beside the first; 26.8–46.7 s the 13
recursion proofs and keccak on two lanes, ~2.4–3.3 s each when paired (1.9 s alone), the resolve
chain last. Three lanes were slower (50.8 s): two recursion proofs already fill the GPU.

Limits: one keccak proof at most (more need risc0's union tree; the prover bails with a message).
A tx with more private accounts (the wallet pads to 7 slots) needs that first.

## rv32im accumulate on the GPU

`risc0-metal/tools/port_accum.py` takes `step_TopAccum` and the 28 functions it reaches (44k lines;
the only extern is a no-op assert) from risc0-circuit-rv32im-sys 4.0.2's CUDA (`steps.cu`), with
the value types, layout types and layout constants, rewrites the CUDA-isms (address spaces,
`cuarray` for `cuda::std::array`) and writes `kernels/rv32im_accum.metal` (2.7 MB, kept in the tree
so the build doesn't need the registry path). `kernels/accum.h` has the Metal helpers: one non-virtual
buffer type, layouts in the constant address space, `cuarray` with address-space-qualified
accessors, the mixed `Fp`/`FpExt` operators of risc0's CUDA `fpext.h`. Phase 1 (one thread per
cycle) runs on the GPU, phases 2–3 (4-column prefix sums) on the CPU. `R0_ACCUM_CHECK=1` also runs
risc0's CPU accumulate on a copy and asserts equality (bit-exact at po2 16 and 19);
`R0_METAL_ACCUM=0` uses the CPU. 1.0–1.4 → ~0.3 s per po2-19 segment. The first run compiles the
pipeline (16 s, then cached by macOS).

## Resume here

**2026-10-26: items 1–3 are done.** The flags are defaults (`R0_METAL_FULL` on unless `=0`, segment
po2 19, store 16; plain `cargo build` is enough). Token and oracle ship their deployed ELFs. The
wallet path goes through `bosphorus_prover` (vendored `lez-wallet`, `set_private_prover`), with
risc0's keccak union tree (`Job::Union`, the same MMR shape as `prover_impl.rs`) and keccak proofs
up to po2 15 allowed beside a po2-19 segment. Real proofs on the localnet: 42.5–54.0 s for
single-note ops, 75.9 s for a borrow to a second private account (5 po2-19 segments, 2 keccak + 1
union; GPU-bound: ~45 s of exclusive segments, then the resolve chain). `[metal] … constraints`
lines only print with `R0_TL=1`.

1. Build with these exact flags (they aren't the defaults yet; baking them in is a to-do):
   `R0_SEGMENT_PO2=19 R0_METAL_BUDGET=400 R0_METAL_STORE=16 cargo build --release -p spike-harness --features prove`
   Without `R0_SEGMENT_PO2=19` segments go back to po2 20 and the 8 GB machine compresses memory.
   The build compiles three generated NEON files and the accumulate kernel (~1 min).
2. **Fix the guest image drift** (above) before anything touches the testnet.
3. **The wallet path.** `bos`/`bos-app` prove private txs inside LEZ's wallet
   (`lez/wallet/src/account_manager.rs`), which calls LEZ's `execute_and_prove`: route it through
   `bosphorus_prover` (wrap or replicate the wallet's call), add risc0's keccak union tree
   (`host/server/prove/union_peak.rs` and its MMR in risc0-zkvm 3.0.5) for its ~6 keccak proofs,
   then time it with `--features prove`.
4. Remaining speed, each worth about a second: NTT high passes run at ~27 GB/s (3.5 GB per pass in
   132 ms, about half the memory bandwidth; wider accesses or a two-pass four-step NTT); preparing
   the next po2-19 segment's witness during the current one (~0.45 s each); the NEON share in the
   paired recursion phase (the CPU is mostly idle there); recursion accumulate on the GPU
   (`risc0-circuit-recursion-sys-4.0.2/kernels/metal/step_*_accum.metal`, unchecked against
   `AccumContext`); `combos_divide` (CPU).
5. Docs: `docs/spikes/S4-proving.md`, `docs/design/proving-flow.md`, a decision-log row.

## Build and run

Runtime flags for every real proof: `env -u RISC0_DEV_MODE R0_METAL_FULL=1 RISC0_KECCAK_PO2=14`.

| What | Command (after the build above) |
|---|---|
| S4 tx | `S4_ONLY_A=1 R0_TL=1 ./target/release/s2_private` (`S4_CLAIM=1` prints the final claim, `S4_DUMP=DIR` writes `proof.bin`/`journal.bin`) |
| Real borrow | `LOG=/tmp/b.log spikes/vendor/risc0-metal/tools/run_borrow.sh` (builds the test, runs `real_proof_borrow` with the timeline; extra env as arguments, e.g. `R0_LANES=1`). By hand: `R0_SEGMENT_PO2=19 R0_METAL_BUDGET=400 R0_METAL_STORE=16 cargo test -p bosphorus-methods --release --features prove --test private -- --ignored --nocapture real_proof_borrow`; the test binary's hash changes with dependencies, so don't rerun an old `target/release/deps/private-*` |
| Stock verifier | `cd spikes/stockverify && cargo run --release --offline -- data` (after an `S4_DUMP=spikes/stockverify/data` run) |
| Stage timeline | `R0_TL=1`: `[tl start length] stage` per proof stage, `[sched …]` per scheduled job; `risc0-metal/tools/tl_summary.py LOG...` sums stages (with two lanes the sums double-count overlap) |
| Kernel benches | `./target/release/metal_eval_bench rv32im 17 3`, `recursion 18 3`, `keccak 14 2`, `ntt 19 211 2`, `hash 21 211 3`, `small 19 600 2` (`batch_evaluate_any`, `zk_shift`, `mix_poly_coeffs`); `CPU_RATE=1` adds risc0's scalar C++ evaluator's rate, `R0_NEON_SHARE=1` checks the NEON evaluator alone |
| Profile | `sample PID 70 1 -mayDie -file s.txt`, `rustfilt -i s.txt -o s.dm.txt`, then `risc0-metal/tools/sample_ops.py` (share per operation) or `sample_tree.py` |
| Accumulate port | `python3 spikes/vendor/risc0-metal/tools/port_accum.py` regenerates `kernels/rv32im_accum.metal` |

`metal_eval_bench` runs each kernel on random data, times it, and asserts it equals risc0's CPU
code; use it instead of full proofs when tuning. `cargo test -p risc0-metal` can't work:
`spikes/vendor` is outside the workspace.

Knobs: `R0_METAL_FULL` unset = Codex's hybrid lane (CPU constraints). `R0_LANES` (2) proving lanes.
`BOSPHORUS_LEZ_PROVE=1` = LEZ's serial schedule. `R0_METAL_SERIAL=1` = the old one-HAL-at-a-time
lock. `R0_METAL_ACCUM=0` = CPU accumulate, `R0_ACCUM_CHECK=1` = compare with it. `R0_NEON_SHARE`
(0–1) = the CPU's share of constraint points (default: from rates measured per po2 on earlier
calls, 0 while another proof holds a HAL), `R0_NEON_THREADS`, `R0_NEON_BUDGET` (NEON stage size at
build time, 1500). `R0_METAL_NTT_ORIG=1` = risc0's original NTT. `R0_NTT_PROFILE=1` = time each NTT
pass. `R0_NTT_LOW_TG=1` = threadgroup-memory low passes. `R0_METAL_TILE` = max points per
constraint command buffer. `R0_METAL_STATS=1` = threads per threadgroup by kernel.
`R0_METAL_EXP=nocols|nopm` at build time = constraint kernels without trace or mix-power loads
(wrong results, for bottleneck timing only). `R0_DISABLE_METAL=1` = CPU lane.

## What each crate changes

| Crate | From | Changes |
|---|---|---|
| `prover` (`bosphorus-prover`, new) | Claude | `execute_and_prove` on our schedule (above): LEZ's backend with guests executed, the circuit with a keccak coprocessor callback, the job graph and two lanes |
| `risc0-circuit-rv32im` | 4.0.4 via [risc0-metal-hybrid](https://github.com/AnubisQuantumCipher/risc0-metal-hybrid) (see its `NOTICE`, `metal-hybrid.diff`), by Codex | Metal lane for generic STARK ops. `R0_METAL_FULL=1` sends `eval_check` to `risc0-metal`. Claude: `DEFAULT_SEGMENT_LIMIT_PO2` from `R0_SEGMENT_PO2` at compile time (`src/execute/mod.rs`); accumulate phase 1 on the GPU and phases 2–3 here (`src/prove/hal/metal.rs`); the witness and trace dropped after the accum commit (−660 MB at po2 19); the group cache cleared per segment; timeline spans |
| `risc0-circuit-recursion` 4.0.4, `risc0-circuit-keccak` 4.0.5 | Codex | `metal_hybrid.rs` lanes, same `R0_METAL_FULL` switch. Claude: the control group through `commit_group_cached`, keyed by an FNV hash of the program's code; timeline spans |
| `risc0-metal` | Codex's crate, generator rewritten by Claude | `generate.py` turns each verifier polynomial (`poly_ext.rs`) into Metal kernels: the nested `AndEqz`/`AndCond` sum flattened into terms `c·x·mix^e`, split into ~400-op kernels that recompute what they need, storing a value in scratch only if recomputing it costs ≥ `R0_METAL_STORE` ops. The same terms become `NAME_neon.c` (4 points per vector, bigger stages, every cross-stage value stored; `kernels/neon.h`), built with `cc`. `src/lib.rs` runs the GPU tiles without waiting while CPU threads take the top of the domain in 2,048-point chunks; `rv32im_accum` runs the ported accumulate kernel (`kernels/rv32im_accum.metal`, `kernels/accum.h`, `tools/port_accum.py`). `generate_v1.py` is Codex's original, kept for reference: it no longer matches `lib.rs`. `tools/` has the profile, timeline and run scripts |
| `risc0-zkp` 3.0.4 | crates.io copy, by Claude | Metal HAL: fused NTT (`fast_expand_evaluate`, `fast_interpolate`), one kernel library, pipeline cache and NTT twiddle tables per process (`shared()`), `zk_shift` from tables, the process-wide HAL lock opt-in, `live_hals()`. Prover: narrow all-zero groups committed through `PolyGroup::zeros` and `MerkleTreeProver::uniform` (one hash per tree layer, no node buffer), `commit_group_cached`/`clear_group_cache` (thread-local). `timeline.rs` (`R0_TL`) |
| `risc0-sys` 1.5.0 | already vendored (Xcode 27 fix) | `ntt_fast.metal` (new), `poseidon2.metal` (sponge loader with constant indices, ×2/×4 as additions; Claude, late evening: Shoup products for the internal diagonal with compile-time constants), `fp.h`/`fpext.h` shadowing risc0-build-kernel's with a 3-multiply Montgomery product (declared as deps in `build.rs` so the kernel cache notices). Claude, evening: `multi_poly_eval` sums `c(256j+t)·y^n` with a threadgroup table of `y^n` (y = x^256), ~4.3 products per coefficient instead of ~24; `zk_shift_table` |

Elsewhere: Codex added `prove` features to `client`, `cli`, `app`, `programs/methods` (in-process
proving, `client/src/proving.rs` sets `RISC0_PROVER=local`), `real_proof_borrow` in
`programs/methods/tests/private.rs`, and S4 harness tweaks (`R0_PROFILE`, `S4_PROFILE_ONLY`).
Claude added `spikes/harness/src/bin/metal_eval_bench.rs`, `spikes/stockverify`, the `prove`-feature
switch to `bosphorus_prover` in `programs/methods/tests/common/private.rs` and
`spikes/harness/src/bin/s2_private.rs`, and the S4 harness's `S4_CLAIM`/`S4_DUMP`.

## Where the time goes now (real borrow)

GPU-bound: ~46 s of GPU work in a ~47 s run. From the serial timeline before the schedule (61.8 s
run; the GPU-side numbers still hold): constraint evaluation 20.6 s (now ~3.1 s per lone po2-19
segment with the NEON share), data commits 14.4 s (Merkle 8.2, NTTs ~5.8), accum commits 6.0,
check commits 2.3, `eval_u` 1.2, control-group commits 1.0, FRI 0.9, `mix_poly_coeffs` 0.8.
Hashing has since lost 10% (Shoup diagonal).

Per-kernel numbers (bench): rv32im constraints 1.70–1.80 µs/point (Codex's kernels ~7.9),
recursion 0.47, keccak 3.5; NEON evaluator on all 8 cores: rv32im 5.1, recursion 1.2, keccak 10.5
µs/point. Data-group NTT 211 × 2^19 → 2^21: forward 0.50 s (risc0's 2.33 s), inverse 0.14 s
(0.55–0.75 s). Row hashing 211 × 2^21: 0.935 s (risc0's 1.50 s). `batch_evaluate_any` 600 × 2^19:
0.033 s.

## Findings worth keeping

- **Register pressure decides GPU constraint speed.** 2,400-op kernels ran at 13.7 µs/point, 600-op
  at 2.6, 400-op at 2.3, despite 3× more recomputation. Metal capped the big ones at 384 threads per
  threadgroup. Storing values that cost ≥ 16 ops to recompute cut emitted ops ~3× but gained ~15%.
- **The constraint kernels are ALU-bound, not memory-bound**: with every trace load replaced by
  arithmetic they ran 15% faster, without the mix-power loads 4% faster.
- **Montgomery products already cost one `mul` and two `mulhi`**: the compiler turns the
  multiplies by `0x88000001` and P into shifts. Shoup products (precomputed `floor(c·2^32/P)`) were
  slower in the constraint kernels (2.86 vs 1.80 µs/point): the second constant load per term costs
  more than the `mulhi` saved. 64-bit lazy accumulators were slower still (3.17): twice the
  registers. With compile-time constants (Poseidon2's diagonal) Shoup wins: row hashing −10%.
- **po2-20 segments don't fit 8 GB** (9.8 GB footprint). po2 19 gives the same total rows for this
  circuit plus one lift and one join, and halves peak memory: net 88.9 → 70.9 s.
- **Keep GPU memory down rather than pooling it.** The first GPU write to a fresh shared buffer
  costs ~0.16 s/GB in a microbenchmark but ~0.03 s per recursion proof in practice. A buffer pool
  made the borrow slower (70.9 s): it held ~5 GB, pushing the machine into memory compression.
  `recommendedMaxWorkingSetSize` is 5,461 MB, about a po2-19 segment's old peak.
- **Two proofs on the GPU, not three.** Paired recursion proofs take ~3 s each (1.9 s alone), i.e.
  1.27× throughput; three take ~4.9 s each, no better, and they delay the resolve chain.
- **The CPU's multiply throughput is ~60% of the GPU's** (NEON, 8 threads: 25–30G Montgomery
  products/s against 42G), but the NEON evaluator reaches 26–31% of the GPU's rate (IPC ~1.7, 521k
  instructions per 4 points for rv32im) and, run beside the GPU, the CPU loses 38% and the GPU 8%
  (memory traffic). Its rate also falls with the domain size, so split rates are kept per po2: one
  learned on po2-16 guests gave po2-19 segments a CPU share that ran 1.7–2.4 s past the GPU.
- **risc0 receipts from `prove_with_opts` keep the journal unpruned in the final claim** (1.7 KB);
  a receipt with it pruned verifies the same (same claim digest) but is smaller than LEZ's.
- **Patched crates reach our guests**: see "Guest image ids drifted".
- **No thermal throttling seen** in ~60 s windows, even with all 8 cores burning NEON multiplies
  beside the GPU. But background GPU users skew timings: the Aerial wallpaper/screensaver
  (`VTDecoderXPCService`, `WallpaperAerialsExtension`) made runs 5–20% slower. Check `ps aux -r`
  before timing; `caffeinate -dimsu` keeps the display awake.
- **The macOS Metal shader cache works for command-line tools**: 130 s cold, 0.04 s warm. Any kernel
  change means one slow first run (it can make a timing look 15 s worse).
- If `risc0-circuit-recursion`'s build script fails with a hash mismatch, its S3 download was cut
  short (it happens whenever a new feature set rebuilds it): copy an intact `recursion_zkr.zip`
  (`744b999f…`, 59,768,781 bytes) from another `target/release/build/risc0-circuit-recursion-*/out/`.
- zsh doesn't word-split `$var`; parameter sweeps in zsh silently ran one config (use `bash -c` or
  `${=var}`).
