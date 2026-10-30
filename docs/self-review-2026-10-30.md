# Bosphorus self-review, 2026-10-30

A hostile-auditor review of Bosphorus as it stands at `690f4ca` plus the uncommitted E10 (screened
pool) and E11 (solvency proofs) work. It is the "self-review" in the Adoption milestone
(`docs/pitch/talk-10min.md` §08). Detailed write-up, with the scenario, numbers and suggested fix for
each finding: `docs/experiments/stress-test-2026-10-30.md`. Earlier findings F1–F21 are in
`docs/findings.md`; this review numbers its own S1–S17 and P1–P8.

## Scope and method

- **Code:** `core/`, the lending, oracle, token and pool guests, `client/`, `keeper/`, `app/`,
  `ffi/`, the Basecamp view, E9, E10 and E11. LEZ behaviour checked in `~/Work/logos-execution-zone`
  at `v0.3.1`.
- **Claims:** `README.md`, `docs/pitch/talk-10min.md`, `docs/pitch/pitch-2min.md`.
- **Approach:** read each path as an attacker (borrower, liquidator, market creator, RPC operator,
  auditor, screener) and as a sceptical investor checking the pitch against the code.
- **Executed:** `cargo test --release -p bosphorus-core` in a scratch copy: 1 unit test and 13
  essential tests pass. Five proof-of-concept tests pin the numbers for S1, S3, S12, S13 and S15; they
  are kept in `docs/experiments/stress-test-2026-10-30-poc.rs` (drop into `core/tests/` to run).
- **Not executed:** the guest tests (`bosphorus-methods`), because of disk space (10 GB free with
  `target/` at 23 GB, see F21). Findings marked *traced* were followed through the code, not run.
  S1, S2, S3, S4 and S12 were spot-checked a second time against the cited lines.

## Results

5 high, 6 medium, 5 low, 1 info, and 8 places where the pitch claims more than the code does.

| # | Severity | Area | Finding | Evidence | Status |
|---|---|---|---|---|---|
| S1 | **High** | liquidation | Anyone can stall a liquidation by adding 1 unit of collateral each block; the liquidation must match the position exactly, and private top-ups are free and can be proved in advance | PoC; ordering against the keeper not tested | Open |
| S2 | **High** | app | A market whose id is ground to sort first becomes the app's default, with the real token names and the attacker's price feed; nothing in the UI tells it apart | traced | Open |
| S3 | **High** | keeper | The F1 trust check can be bypassed: a trusted publisher's feed is accepted for any asset pair, and oracle headers aren't checked for immutability while trust is cached | PoC (a); traced (b) | Open |
| S4 | **High** | privacy | Every refresh reads each account's position addresses by id over RPC, so the RPC operator (the sequencer on testnet) links a public account to all its private positions | traced | Open |
| S5 | **High** | privacy, E11 | A solvency proof can be narrowed to one position: public positions count toward the minimum cover set, and the auditor can add sybil positions; the pseudonym is also transferable | traced | Open |
| S6 | Medium | keeper | Liquidates one position at a time, waits for inclusion, closes only in full, and needs the whole repay on hand: crashes, whales and spam stall it | traced | Open |
| S7 | Medium | oracle | One hot publisher key, no price bounds, and a 1 h staleness allowance checked at the window's end (~50 min old prices accepted) | traced | Open |
| S8 | Medium | product | Private borrowers can borrow to HF ≈ 1.02 but need 60–75 s to rescue a position, against a keeper acting in 1–2 blocks | measured timings + traced | Open |
| S9 | Medium | app | A landed op is reported as "didn't apply" if anyone touched the position first, inviting a double borrow (F10-class) | traced | Open |
| S10 | Medium | product | Private lenders exit last in a bad-debt event | traced | Open |
| S11 | Medium | E10 | The screener can freeze approved withdrawals by churning roots; the pool can approve only 2,048 deposits in its lifetime | traced | Open |
| S12 | Low | money | An overpaid repay can zero total borrow assets while other borrowers keep shares: their debt is wiped and borrowing is bricked | PoC, spot-checked | Open |
| S13 | Low | money | Accrual overflows `u128` at extreme sizes and bricks the market | PoC | Open |
| S14 | Low | privacy | The price and ratio claims reveal the proving start that rounding `t` was meant to hide | traced | Open |
| S15 | Info | money | Interest rounds down at each accrual; a small market touched every block accrues nothing | PoC | Accepted (Morpho has the same) |
| S16 | Low | security | Keys and job secrets are plaintext on disk (inherited from LEZ v0.3.1) | traced | Open |
| S17 | Low | docs | Design §8 and §6 and the plan's experiment ids are out of date | read | Open |

### What held up

- **Rounding:** every conversion rounds in the protocol's favour from the right end of each claim,
  and ratio claims are checked exactly. Property tests pass.
- **Vault solvency:** the loan vault always covers `TSA − TBA` and the collateral vault covers all
  collateral, on every path. Donations can't move ratios, and the first-depositor attack is defeated
  by the share offsets.
- **Cross-shard binding:** `inspect` is tied to the right program's shard, positions check their
  market, and shard tags stop one kind of account posing as another.
- **Private settlement:** deferred effects, price checks included, are re-checked against live state
  at settlement.
- **Authorisation and replay:** owner checks, salt derivation, signer nonces and nullifiers.
- **Assets, pool and oracle:** the on-chain asset allowlist, the pool's money safety (ragequit can't
  double-spend), and oracle replay protection.
- **C ABI:** panics are caught, memory ownership is clear, and null handles are refused.

## The pitch against the code

| # | Claim | What's true |
|---|---|---|
| P1 | The talk opens with whale hunting (Egorov, Solend, Celsius) | Option 1 positions show collateral, debt, health and liquidation price publicly. Bosphorus hides who owns a position, not the position, so hunting isn't prevented. Hiding contents is Option 2 |
| P2 | "Nobody else sees your numbers" (2-min) | Everyone sees the numbers |
| P3 | "Your position never leaves the machine" (2-min) | The position is on chain in the clear |
| P4 | "No one can tell it's yours" | The demo dives 0.5 WBTC and adds the same amount as collateral a minute later, on a near-empty chain. The RPC operator links it anyway (S4) |
| P5 | "Risk controls taken from Aave v3" | Only the interest-rate curve's shape. Borrow LTV below LLTV (E2), caps (E3) and close factor (E5) aren't built |
| P6 | "Liquidations work exactly as on a public market; lenders take no extra risk" | S1, S6, S7, S8 |
| P7 | Solvency proofs: "they learn nothing about which position is yours" | S5 |
| P8 | "You can prove none of it came from a sanctioned address"; "about thirty-six seconds" | Lending doesn't check provenance yet. Real ops take 60–75 s end to end |

The honest pitch is still strong: **nobody knows the position is yours**. That covers the
identity half of the backup incidents (Egorov was hunted *because* everyone knew it was him; Celsius
was exposed before it said anything), but not the position half.

## Remediation plan

In order. Each fix goes through the usual tests (money safety and authorisation) and the phase's
e2e script. Fixes in `core` change the lending image id, so they need a redeploy.

1. **S1:** liquidate against lower-bound claims (`collateral ≥ c`, `borrow_shares ≥ S`), keeping the
   exact claim only for bad-debt realisation; the keeper retries `PositionNotAsClaimed` at once.
2. **S3:** trusted feeds configured per pair and bound to the market's assets; `check_program`
   requires an immutable header; trust re-checked on a timer.
3. **S2:** the app lists allowlisted markets only, shows the publisher, and never defaults to
   `markets[0]`.
4. **S12, S9:** cap repay assets at what the shares owe; `finish` decides from the tx's result.
5. **S4:** read positions from the local block scan instead of by id over RPC.
6. **Pitch:** rewrite the talk and the 2-min pitch around "nobody knows it's yours" (P1–P8), and use
   unrelated amounts in the demo.
7. **S5, S8, S6, S7, S11:** design work, one experiment each. S8 (minimum HF for private borrows) is
   the cheapest.
8. **S13, S14, S16, S17:** when the code is next touched.
