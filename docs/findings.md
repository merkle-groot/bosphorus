# Review findings (2026-10-20)

A full review of code and docs at `28daacc` (Phase 4), before Phase 5.

## Status (updated 2026-10-30)

| Item | Status |
|---|---|
| F1 malicious markets vs the keeper | **Fixed.** On-chain asset allowlist (the user chose it over client-only checks): `Config.enabled_assets`, `EnableAsset`, `create_market` inspects asset headers (test `markets_only_take_enabled_immutable_asset_bytecode`). Keeper: trusts a market only if lending, assets and oracle run its build and the feed publisher is in `--publisher`; refuses unprofitable liquidations. `bos` checks lending and asset image ids on public txs too. |
| F2 one bad market stops the keeper | **Fixed.** Per-market errors are logged and the market is re-checked after 60 rounds. |
| F3 windows vs testnet stalls | **Fixed.** Oracle `PUBLISH_WINDOW_MS` 10 min; client public window and keeper window 10 min (testnet has 5–10 min gaps, see constraints §5); publishes stamped `now − 2 s`. Wallet polling for testnet in `scripts/testnet/wallet_config.json` (5 s × 360). |
| F4 image ids on other machines | **Partly.** `bos programs --write-dir` exports the ELFs; the deploy script keeps them in `.$NETWORK/elfs/` with the image ids. Not done: a client option to load published ELFs, or Docker builds. **New:** a doc-comment edit changed the oracle's image id, so deploy from a committed tree and leave guest sources alone until the scenarios are done. |
| F5 private ops proved against a price that will be stale | **Fixed** (2026-10-21, committed in `7d41494`). Scenario script publishes before each private borrow; `client` refuses a priced op whose feed is stale at the window's end (`stale_price`), before proving. |
| F6 LEZ: rolled-back txs lose their hash index | **Reported upstream** (sent to the Logos team 2026-10-21; issue [logos-execution-zone#997](https://github.com/logos-blockchain/logos-execution-zone/issues/997)). Since resolved on testnet: on 2026-10-21 (tip 48476) both example hashes resolve to block 44212. Client impact: a reported inclusion block isn't final; `[measure]` lines can point at replaced blocks. |
| F7 keeper below the fee reserve | **Fixed** (2026-10-21, committed in `7d41494`). `client` checks every public tx's payer against `gas_limit × base_fee_exec` first and says so (`payer_cannot_fund`); the keeper logs that instead of the raw RPC error. |
| F8 held shares: a full withdrawal burns exactly the supplied shares | **Open** (E9, 2026-10-21, privacy). Client mitigation proposed (round-amount withdrawals), not built. |
| F9 any account can receive a supply; since E9 its owner can withdraw it | **Accepted** (E9, 2026-10-21, info). Was a stuck gift before E9; now an ordinary gift. |
| F10–F20 the app under failure (2026-10-27) | **Fixed** (F20 partly), committed in `690f4ca`. Found by fault injection (`basecamp/dev/faultproxy.py`, `faults.py`) and code reading; see "App findings" below. |
| F21 block scans skip blocks the sequencer doesn't return | **Fixed** for lending scans (2026-10-29, uncommitted): `Scanner.missing`; disclosure requests refuse a gap, the keeper logs it. **Open** for the pool's ASP scan (`client/src/pool.rs`: a deposit in a missing block stays pending; ragequit still works) and the wallet's own sync. See "App findings". |
| S1–S17, P1–P8 self-review (2026-10-30) | **Open** (S15 accepted). Five high: S1 liquidation stalled by dust top-ups, S2 attacker market as the app's default, S3 keeper trust bypass (F1 is weaker than stated), S4 RPC operator links accounts to private positions, S5 E11 cover set narrowable. Plus 8 pitch overclaims. Report and remediation order: `docs/self-review-2026-10-30.md`; details `docs/experiments/stress-test-2026-10-30.md`. |
| D1–D3 STATUS/CLAUDE.md stale lines | Fixed in STATUS.md and CLAUDE.md. |
| D4, D5, D7, D8 design doc | Fixed. |
| D6 vpk in the private test's scan | Fixed (`identifying_bytes` includes it; test passes). |
| D9 CLI `public()` message | Fixed. |
| D10 landing test runtime note | Fixed (~35 min). |
| D11 `print/approach.pdf` | Still out of date. |
| clippy `clone_on_copy` | Fixed; clippy clean. |

The sections below are the original findings, unchanged.

## What was verified

- `RISC0_DEV_MODE=1 cargo test --release -p bosphorus-core -p bosphorus-methods`: 31 pass (14 core,
  9 lending, 5 token, 1 oracle, 2 private). `RISC0_SKIP_BUILD=1 cargo clippy --release --workspace
  --all-targets`: clean apart from two `clone_on_copy` warnings in test/spike code
  (`programs/methods/tests/common/private.rs:81`, `spikes/harness/src/lib.rs:95`).
- Cycle table re-run reproduces design §5 exactly (borrow with accrual 122,144; liquidate 156,878 /
  158,789; shards: market 322 B, position 81 B).
- Lending maths matches Morpho Blue (accrual, fee shares, `zeroFloorSub` on repay, LIF, both
  liquidation modes, bad-debt socialisation); every bounds check in the guest uses the protocol-side
  end of each claim.
- Probed `claims::full_liquidation` for a rounding gap where neither repay-all nor seize-all fits:
  none in ~150M cases (fresh and accrued ratios, prices 1, 0.37, 1.7, 3e-9). Not a finding.
- Added the viewing public key to the private test's byte scan temporarily: it's absent from every
  tx and touched public account, so P4's claim holds (see D6: the test doesn't keep the check).
- LEZ v0.3.1 facts spot-checked in `~/Work/logos-execution-zone` (10 chained calls, 32M cycle
  budget, gas caps, `PROOF_BYTES`, private-tx events dropped, devnet 20 tx / 10 s): all match the
  constraints doc.
- All four commits are GPG-signed (`git log --format=%G?` → `G`). Phases 2–4 (3 commits) are
  not pushed.

## Platform fact the design didn't record (LEZ v0.3.1)

**A public tx's signer is authorised in every call at every depth, PDA grants pass down to every
descendant call, and any call may touch any shard of any account listed in the tx.**

- `lee/state_machine/core/src/execution_state.rs:247`: a public account's `is_authorized` is
  `authorized_accounts.contains(id)` (the tx signers), checked the same way for every chained call.
- `execution_state.rs:466`: chained calls inherit the caller's `grants` (`grants.clone()`), so a PDA
  authorised by `pda_seeds` stays authorised in the callee's own chained calls.
- `lee/state_machine/src/validated_state_diff/public_backend.rs:135`: `public_shard` returns any
  shard of any listed account; shards aren't declared per program. (This is also why a liquidation
  can pay seized collateral to the keeper's TKA shard while the tx only selects its loan-token shard.)

Consequence: signing a tx that calls an untrusted program hands it every token and native balance
on the signing account, payable to any account in the tx. Add this to constraints §1/§2.

## Code findings

### F1 (high): the keeper can be drained through attacker-created markets

Markets are permissionless, and `bos-keeper` liquidates in every market it discovers.

1. **Malicious oracle, honest tokens.** The attacker creates a market with their own oracle/feed and
   junk collateral (an honest token instance they mint), supplies TKB, and borrows it back at an
   inflated price. They then nudge the price so the position is barely unhealthy at a still-inflated
   price. The keeper repays the debt with its real TKB (which lands in the market, where the
   attacker withdraws it as supplier) and receives almost nothing of value. There's no
   profitability check (design §7: "reference quality").
2. **Malicious token program.** The market's loan or collateral asset is a program the attacker
   wrote. `lending` chain-calls it during the liquidation with the keeper as signer, so (by the
   platform fact above) it can chain-call TKB, TKA or the native token and move the keeper's
   balances to any account in the tx. `params.oracle_feed` is attacker-chosen and is in the tx, so
   it can be the attacker's own account. The keeper's pre-check
   (`client.token_balance(loan_asset, liquidator)`) is trivially satisfied by minting the fake token
   to the keeper. The attacker controls the oracle, so the position is liquidatable whenever they want.

Honest markets' vaults aren't exposed: each op touches one market, and the only programs called are
that market's assets.

Fix:
- Keeper: only touch markets whose loan and collateral assets are native or run the Bosphorus token
  bytecode (`Client::header(asset).image_id == Program::new(TOKEN_ELF).id()`, as `program_with`
  already does), and whose `(oracle_program, oracle_feed)` is on an allowlist (CLI flag or file).
  Add a profitability check: value seized collateral against an independent price, not the
  market's own feed.
- `bos` public path: `send_lending` (`client/src/lending.rs:311`) does no asset check; the private
  path does (`program_with`, `client/src/lending.rs:328`). Check assets on the public path too, or at
  least warn.
- **Decision for the user:** also enforce it on-chain? `create_market` could inspect each asset's
  header account (program-loader shard) and require its image id to be in an admin-set allowlist
  in `Config` (inspect works on any listed shard, constraints §1). That makes permissionless
  markets safe against malicious tokens. A malicious-oracle market can't be stopped this way:
  lenders and keepers vet oracles, as on Morpho. Record whichever choice is made in the decision log.

### F2 (medium): one bad market stops every keeper round

`keeper/src/main.rs:116-117` uses `?` on `client.market(..)` and `client.feed(..)`, so an error
aborts the whole round, and markets are visited in `BTreeMap` order by id. `SupplyCollateral` doesn't
touch the market (`programs/lending/guest/src/bin/lending.rs:147`), so anyone can create a position in
a market that doesn't exist, with any token (even one they deployed), and grind the params (e.g.
`max_price_age`) so its id sorts first. Every round then fails before reaching any real market: no
liquidations, so bad debt builds up. A market whose feed account is empty does the same.

Fix: on a per-market error, log it and `continue` (as is already done per position). Also skip
positions whose market isn't allowlisted (F1): spam positions cost one RPC read each per 5 s round.

### F3 (Phase 5): testnet stalls vs short validity windows

Observed 2026-10-20 ~05:00 UTC (`getBlock` header timestamps): blocks every 10.0 s, but one 60 s gap
(40607 → 40608) and one of ~2.5 min (40609 → 40610). Our clock matched the testnet's to ±0.1 s
(`sntp`, HTTP `Date`). A public tx included after its window fails with `OutOfValidityWindow`, which
is **charged** (S3). Current windows:

| What | Window | Where |
|---|---|---|
| oracle `Publish` | 60 s, **a program constant** (immutable once deployed) | `programs/oracle/core/src/lib.rs:13` (`PUBLISH_WINDOW_MS`) |
| keeper liquidations | 60 s | `keeper/src/main.rs:43` |
| client public default | 120 s | `client/src/lending.rs:135` |
| client private | 20 min | `client/src/lending.rs:121` (fine) |

Decide `PUBLISH_WINDOW_MS` before deploying the oracle to testnet (e.g. 5–10 min). Widen the keeper
and public defaults, or make them network-dependent; `MAX_WINDOW_MS` (30 min) is the cap.
Also: `feed_publish` stamps `timestamp = now` with no lag (`client/src/lib.rs:258`), while lending
ops use `t = now − 2 s`. Low risk at the measured skew, but give it the same lag.

### F4 (Phase 5): other users can't prove private txs with their own builds

A private tx must be proven with the exact deployed bytecode (the client checks image ids, and so
does the circuit). Guests are built without Docker (`risc0_build::embed_methods`), so another
machine's build gets different image ids and private ops fail with "runs different bytecode than this
client's build". Either publish the deployed ELFs next to `DEPLOYMENTS.md` (and let the client load
them), or build with Docker for reproducible ids. STATUS mentions Docker only for reproducibility,
not this consequence.

### F5 (Phase 5, testnet): private ops proved against a price that will be stale

The first testnet private run (2026-10-21) lost its index-1 borrow: proved for 632 s, sent at
01:21:46 IST, never included. The only publish (2.5) was at 23:50:50, the market's `max_price_age`
is 1 h, and freshness is checked at the window's end, so the program rejected it; a rejected
private tx is dropped for free, and the wallet gave up after its 30-min poll. The localnet rehearsal
took 4 min, so it never hit this. Two fixes: (1) done: `scripts/phase5-private.sh` publishes before
each private borrow; (2) not done: `bos` should refuse to start proving a priced op when the feed's
age at the claim window's end exceeds `max_price_age` (and say so), instead of spending ~11 min on a
tx that can't land. Same check would help public ops (they are charged when rejected).

### F6 (Phase 5, testnet): rolled-back txs lose their hash index (LEZ)

2026-10-20 ~17:48–17:50 UTC: old blocks 44212–44224 were replaced by one new 44212 (15 txs, the
same txs in order); `getTransaction` returns null for the 12 that moved, though `getBlock` has them
and their effects are in state. Bosphorus state was unaffected, but inclusion blocks reported by the
wallet aren't final. Details and the upstream message: STATUS ("Testnet anomaly").

### F8 (E9, privacy): a full withdrawal of held shares burns exactly the supplied shares

A private op's deferred market effect carries `assets` and `shares` verbatim (P4). With held shares
(E9) there's no position to link a lender's supply and withdrawal, but the share count does: the
localnet run (2026-10-21) minted `99999891059218681` shares and `withdraw --all` burnt the same
17-digit number. Sums of several supplies can be matched the same way. Proposed client fix: for
held shares, withdraw round asset amounts (shares derived from them) in more than one tx and leave
the dust; make `--all` warn. Belongs with E8 (amount bucketing). Details:
[`experiments/E9-held-lender-shares.md`](experiments/E9-held-lender-shares.md).

### F9 (E9, info): any account can receive a supply, and its owner can now withdraw it

`Supply`, `SupplyCollateral` and `Repay` never checked that `position` is a position PDA (they act
"on behalf of any position", as in Morpho). Supplying into an arbitrary account writes a `Position`
there. Before E9 those shares were stuck (only a salted PDA could withdraw); since E9 the account's
owner can withdraw them (`position == owner`). That's a gift by the supplier, like a transfer, and
nobody else gains: a lending PDA (config, market, vaults) is only authorised by `lending` itself,
which never calls itself, and a market or config account fails the shard-type check. Accepted.

### F7 (Phase 5, testnet, low): a keeper below the fee reserve retries forever

The sequencer screens every charged tx before the mempool: the payer must hold the fee reserve
`gas_limit × base_fee_exec + gas_stor × base_fee_stor + tip` (`lez/programs/fee/core/src/assess.rs`
`fee_reserve`), i.e. ≥ 16M native at gas limit 2M and base fee 8, or the RPC answers
`InvalidParams "Incorrect fee"` (the wallet FFI maps it to `PayerCannotFund`; nothing is charged).
On 2026-10-21 the private scenario's keeper was given 15M (`KEEPER_NATIVE=15000000`, an operator
override; the default is 50M) and every liquidation was refused; the keeper logged the raw error and
backed off. Fix: the keeper (and `bos`) should compare the payer's balance with the reserve at the
current base fee before sending and fail with a clear message; scripts should never fund a payer
below ~20M.

## App findings (2026-10-27)

A pass over the app (`app/`, `ffi/`, the Basecamp module and view) for failures and waiting,
driven through `bos-app` against the dev-mode localnet behind a fault-injecting proxy
(`basecamp/dev/faultproxy.py`: down, hang, a sent tx's reply dropped, a send answered 502; the
scenarios in `basecamp/dev/faults.py`), the C ABI (stop/close during a real proof), and the view
probe (offline, interrupted, dropped, refused, over-balance screens). All fixed in the same pass
unless said otherwise.

- **F10 (high, money): a lost reply turned a landed tx into "didn't go through".** Any error from
  the send RPC failed the job ("Nothing moved"), even after the sequencer had taken the tx; a retry
  then repeats it (a second borrow). Reproduced: tx `00f8cbb8…` landed, the job said failed. Fix:
  the tx hash and the notes' secrets are saved right before the tx is handed over (a task-local
  `BEFORE_SEND` hook in the vendored wallet); only a refusal (`ClientError::Call` → `rejected`) or a
  failure before the hook fails the job, anything else is watched by hash and position. Checked:
  public and private (held) supplies with the reply dropped end `done`; a 502 is watched, not failed.
- **F11 (high, keys): the wallet file was rewritten in place.** LEZ's `save_to_path` truncates
  `storage.json` (every key, in the clear) and writes it again on every save, including every synced
  block; a crash or kill mid-write leaves it truncated. Fix (vendored wallet): write a temp file
  (0600), fsync, rename, fsync the directory.
- **F12 (medium): a job stopped mid-proof waited ~20 min, and the view said it carries on.** On
  restart a `proving` job watched its position until its window closed, though nothing had been
  sent; "You can close Bosphorus; it carries on when you're back" was false (the proof is dropped).
  Fix: with the hash saved before sending, a job stopped without one is `interrupted` at once
  (measured: 0 s after restart, was 20+ min); the copy says closing stops the dive and nothing is sent.
- **F13 (medium): a dropped transfer held the app for up to an hour.** A transfer the sequencer
  skips (seen: a token shield over the balance, `failed settlement: insufficient balance`) left the
  call polling for `seq_tx_poll_max_blocks` (200–360 blocks) with the wallet locked. Fix: balances
  are checked first (`insufficient_balance`); not included 8 blocks after sending → `not_included`;
  a chain that makes too few blocks to tell → `unconfirmed` after 10 min; a lost reply is resolved by
  the saved hash.
- **F14 (medium): reads froze during a proof.** The job held the wallet's read lock while proving;
  a sync, new account or transfer queued for the write lock, and tokio's fair lock then held every
  read (balances, quotes) behind it until the proof ended. Fix: a `heavy` lock serialises the slow
  and wallet-changing work, writers take it first, reads only take the wallet briefly; sync goes in
  100-block steps. Measured: reads ≤ 0.1 s during a proof, a sync waits its turn.
- **F15 (medium, UX): no notion of the network being away.** With the sequencer down the status
  said "Calm · up to date", background refreshes toasted raw `client error (SendRequest)` every
  30 s, `info` failed; with it hanging, each call took jsonrpsee's 60 s. Fix: `info` answers within
  10 s with `network_problem`; errors carry `network`; RPC timeout 30 s; the view shows "No
  connection" / "Network stalled" (no block for 2 min), disables sending, polls every 5 s and
  refreshes when it's back; `markets` serves the last answer marked `cached`.
- **F16 (medium): ops paying more than the account holds were sent.** A public one lands as a
  failure and still pays its fee; a private one is dropped and the job waited out its window
  (~20 min). Fix: supply, add security and repay check the paying account's token balance first
  (`insufficient_balance`; private: as of the last sync), and the view says so before quoting.
- **F17 (low, money): a double click submitted twice** (Position's button stayed live until the
  ticket answered; Activity's "Dive again" too). Fix: guarded; "Dive again" hides once a newer job
  with the same request exists. Quotes are re-asked when the position moves or the offer runs out.
- **F18 (low): closing Basecamp waited for a running proof** (1 min GPU, 10 on a CPU: the
  runtime's drop waits for blocking tasks). Fix: `bos_app_stop` (calls in flight answer
  `stopping`), then `bos_app_close` shuts the runtime down within 3 s. Measured over the C ABI
  during a real shield: answer 0.1 s after stop, close 3.0 s, reopen at once, nothing sent.
- **F19 (low): job events at open were lost.** The worker resumed jobs before anyone subscribed,
  so the first subscriber missed them (app-e2e.sh's restart step was flaky). Fix: the first
  receiver is made with the channel.
- **F20 (info, partly): restoring.** A restored wallet didn't sync (private balances 0 until
  Update), holds its first accounts only, and a "new" private account whose keys were used on
  another copy of the wallet has its txs dropped ("Commitment already seen"; hit by the test
  harness, now reported as `not_included` within 8 blocks). Done: the view syncs after a restore
  and says to add further accounts in order. Not done: account discovery (a gap limit).

- **F21 (medium, 2026-10-29): block scans silently skip missing blocks.** The localnet's disk
  filled during E11; the sequencer kept applying blocks 6544–6589 to state but lost them from its
  block store. `getBlockRange` over a range that includes them returns the blocks before the gap
  and nothing after it, with no error. `Scanner::scan` took the reply as complete, so
  `bos market positions` found 2 of the market's 6 positions, and a disclosure request built a
  cover set without the 4 private ones (the owner's check agreed, as it uses the same scan). The
  keeper would never have seen those positions either. Fix: the scanner compares returned block
  ids with the range asked and records gaps; `market_positions` (disclosure) refuses them, the
  keeper logs them. Not fixed: the pool's ASP scan and the wallet's sync use the same RPC.

## Doc findings

- **D1.** CLAUDE.md and STATUS.md say Phase 4 is "not yet committed": it's `28daacc` (signed).
  Phases 2–4 (3 commits) are unpushed. STATUS open thread 1 becomes "push Phases 2–4".
- **D2.** STATUS machine notes: the "Repo" row (head `9a49b15`, "Phase 2 uncommitted") is stale; the
  localnet row says **running**, but it isn't (`pgrep -f sequencer_service` finds nothing).
- **D3.** STATUS open threads 2 and 6, plan risks, S6, spikes README: testnet "stalled at block
  36316". On 2026-10-20 it's live (head ~40,614 at 05:04 UTC) but stalls for minutes at a time (F3).
- **D4.** Design §2.3: `MarketParams` lists `oracle_feed` before `oracle_program`; the code
  (`programs/lending/core/src/lib.rs:30-42`) has `oracle_program` first. The order matters because
  `market_id = SHA256(borsh(params))`.
- **D5.** Design §2.1: the token's `Mint { to, amount }` is really `Mint { amount }` with accounts
  `[authority, config, to]`.
- **D6.** P4 report ("What can't leak") says the in-process scan covers `vpk (prefix)`.
  `identifying_bytes` (`programs/methods/tests/common/private.rs:55-63`) doesn't include it; only
  `bos tx-report` scans it, and only in the tx bytes. Add
  `("viewing public key (first 32 bytes)", self.vpk().to_bytes()[..32].to_vec())`: verified to pass.
- **D7.** Design §4 still has the superseded client rule
  `salt = HMAC(wallet_secret, "mm-position" ‖ market_id ‖ index)` above the `[built, Phase 4]` one.
- **D8.** Design §3 says "withdrawers and borrowers only lose by backdating". A backdated `t` also
  values existing debt at the older ratio in the `borrow` and `withdraw_collateral` health checks, so
  the check can be optimistic by up to `rate × w` of the debt (≤ 0.57% at the 10,000% APR cap with a
  30-min window; ~6e-6 at 10% APR). Nothing is lost (the next accrual catches up), but the sentence
  is wrong.
- **D9.** `cli/src/lending.rs:19-23`: the `public()` helper's comment and error still say private
  accounts are "Phase 4". It's now only for admin and liquidator roles.
- **D10.** `programs/methods/tests/landing.rs:144` says "a few minutes"; STATUS says ~35 min.
- **D11.** `print/approach.pdf` is out of date on proving (already noted in STATUS).

## Next steps, in order

As of the review (2026-10-20); items 1–4 are done (F4 only partly). The current list is in [`STATUS.md`](STATUS.md) "Open threads".

1. Fix F1 and F2 (keeper asset/oracle checks, profitability check, per-market error isolation;
   public-path asset check in `bos`). Decide the on-chain allowlist question (F1).
2. Decide `PUBLISH_WINDOW_MS` and the keeper/public default windows for testnet (F3); add a lag to
   `feed_publish`.
3. Apply D1–D10; add the platform fact above to constraints §1/§2 and a decision-log row for each
   decision taken. Then the user pushes Phases 2–4 (GPG-signed, from their terminal; no Claude
   trailer). Whether to push before or after the keeper fix is the user's call.
4. Phase 5 (plan): a network switch for the scripts (sequencer URL `https://testnet.lez.logos.co`,
   wallet home, per-network ids file instead of `.localnet/*.env`); resolve F4; fund testnet
   accounts; deploy tokens, oracle, lending; `DEPLOYMENTS.md`; run the price pusher and keeper; run
   the public and private scenarios; write the measurement report (gas per op, proving, landing rate,
   inclusion delays **including stalls**, keeper latency).
5. Then: E7 follow-ups (repay overpayment refund, re-tune widths to testnet block times); tell the
   LEZ team about the ML-KEM profile, the testnet stalls, and the tx-wide signer authority; re-measure
   proving on a CUDA box.
