#!/usr/bin/env bash
# Phase 3 exit gate: the keeper liquidates on its own.
#   A. price drop: alice's position goes under water; the keeper repays all her debt for part of
#      her collateral.
#   B. crash: bob's collateral is worth less than his debt; the keeper seizes all of it and the rest
#      of the debt is written off against the lender (bad debt lowers the supply side).
# Then the fee recipient's pending fees are claimed and the lender withdraws what's left.
#
# Runs scripts/phase2-e2e.sh first (which runs Phase 1); REUSE_PHASE2=1 reuses .localnet/phase{1,2}.env
# and the e2e wallet instead. Deploys its own lending instance either way. Needs a running dev-mode localnet and release builds of `bos` and
# `bos-keeper` (`cargo build --release -p bosphorus-cli -p bosphorus-keeper`).
set -euo pipefail
cd "$(dirname "$0")/.."

[[ -n "${REUSE_PHASE2:-}" ]] || scripts/phase2-e2e.sh
# shellcheck source=/dev/null
source .localnet/phase1.env
# shellcheck source=/dev/null
source .localnet/phase2.env

W=${WALLET:-$HOME/Work/logos-execution-zone/target/release/wallet}
BOS=${BOS:-target/release/bos}
KEEPER_BIN=${KEEPER_BIN:-target/release/bos-keeper}
export RISC0_DEV_MODE=1 SUPPRESS_VERBOSE_PRINTS=1
export LEE_WALLET_HOME_DIR=$PWD/.localnet/e2e-wallet
PAYER=$PUBLISHER
mkdir -p .localnet/logs
KEEPER_LOG=.localnet/logs/keeper.log

step() { printf '\n== %s\n' "$*" >&2; }
expect() { [[ "$1" == "$2" ]] || { echo "FAIL: $3: expected $2, got $1" >&2; exit 1; }; }
more() { [[ "$1" -gt "$2" ]] || { echo "FAIL: $3: expected more than $2, got $1" >&2; exit 1; }; }
bos() { "$BOS" "$@" | tail -1; }
new_account() { "$W" account new "$1" < /dev/null | grep -o "$2/[1-9A-HJ-NP-Za-km-z]*" | head -1; }
last_block() {
  curl -sf -X POST http://127.0.0.1:3040 -H 'content-type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getLastBlockId","params":[]}' | grep -o '"result":[0-9]*' | cut -d: -f2
}
publish() {
  bos oracle publish --oracle "$ORACLE" --publisher $PAYER --name TKA/TKB --price "$1" \
    --loan-decimals 6 --collateral-decimals 6 > /dev/null
}
# Debt of a position (last line of `bos status --owner`).
debt() { bos status --lending "$LENDING" --market "$MARKET" --owner "$1"; }
supplied() { "$BOS" status --lending "$LENDING" --market "$MARKET" | grep -o 'supplied [0-9]*' | cut -d' ' -f2; }
# Waits up to 4 min for the keeper to clear $1's debt (position $2) and log it.
await_liquidation() {
  for _ in $(seq 48); do
    [[ "$(debt "$1")" == 0 ]] && grep -q "liquidated $2" "$KEEPER_LOG" && return 0
    sleep 5
  done
  echo "FAIL: the keeper didn't liquidate $1; keeper log:" >&2
  tail -20 "$KEEPER_LOG" >&2
  exit 1
}

START=$(last_block)
IRM=(--base 2% --slope1 10% --slope2 100% --kink 80%)
# Programs are immutable: Phase 2's instance predates `Liquidate`, so deploy this build's.
step "lending: deploy, init, enable LLTV 80% and the IRM"
LENDING=$(bos lending deploy --payer $PAYER)
bos lending init --lending "$LENDING" --admin $PAYER --fee-recipient $PAYER --payer $PAYER > /dev/null
bos lending enable-lltv --lending "$LENDING" --admin $PAYER --lltv 80% > /dev/null
bos lending enable-irm --lending "$LENDING" --admin $PAYER "${IRM[@]}" > /dev/null
echo "lending $LENDING" >&2

step "new market (TKB loan, TKA collateral, LLTV 80%, 2–112% APR, 10% fee)"
MARKET=$(bos market create --lending "$LENDING" --loan "$TKB" --collateral "$TKA" --oracle "$ORACLE" \
  --feed "$FEED" --lltv 80% "${IRM[@]}" --max-price-age 3600 --payer $PAYER)
bos lending set-fee --lending "$LENDING" --admin $PAYER --market "$MARKET" --fee 10% > /dev/null
echo "market $MARKET" >&2
ON=(--lending "$LENDING" --market "$MARKET" --payer $PAYER)
position_of() { "$BOS" status --lending "$LENDING" --market "$MARKET" --owner "$1" | grep -o '^position [1-9A-HJ-NP-Za-km-z]*' | cut -d' ' -f2; }

step "fund a lender, two borrowers and the keeper (which pays its own fees)"
LENDER=$(new_account public Public)
ALICE=$(new_account public Public)
BOB=$(new_account public Public)
KEEPER=$(new_account public Public)
bos token mint --token "$TKB" --authority $PAYER --to "$LENDER" --amount 2000000000000 > /dev/null
bos token mint --token "$TKA" --authority $PAYER --to "$ALICE" --amount 400000000000 > /dev/null
bos token mint --token "$TKA" --authority $PAYER --to "$BOB" --amount 100000000000 > /dev/null
bos token mint --token "$TKB" --authority $PAYER --to "$KEEPER" --amount 2000000000000 > /dev/null
"$W" auth-transfer send --from $PAYER --to "$KEEPER" --amount 1000000000 < /dev/null > /dev/null
echo "lender $LENDER, alice $ALICE, bob $BOB, keeper $KEEPER" >&2

step "lender supplies 2,000,000 TKB; price 2.5"
bos market supply "${ON[@]}" --from "$LENDER" --assets 2000000000000 > /dev/null
publish 2.5

step "alice: 400,000 TKA (worth 1,000,000), borrows 750,000 (HF 1.07)"
bos market supply-collateral "${ON[@]}" --from "$ALICE" --assets 400000000000 > /dev/null
bos market borrow "${ON[@]}" --owner "$ALICE" --assets 750000000000 > /dev/null
step "bob: 100,000 TKA (worth 250,000), borrows 150,000 (HF 1.33)"
bos market supply-collateral "${ON[@]}" --from "$BOB" --assets 100000000000 > /dev/null
bos market borrow "${ON[@]}" --owner "$BOB" --assets 150000000000 > /dev/null

step "start the keeper (scanning from block $START)"
"$KEEPER_BIN" --lending "$LENDING" --liquidator "$KEEPER" --interval 5 --from-block "$START" > "$KEEPER_LOG" 2>&1 &
KEEPER_PID=$!
trap 'kill $KEEPER_PID 2> /dev/null || true' EXIT
sleep 30
grep -q "found position" "$KEEPER_LOG" || { echo "FAIL: keeper found no positions" >&2; cat "$KEEPER_LOG" >&2; exit 1; }
if grep -q "liquidated" "$KEEPER_LOG"; then echo "FAIL: keeper liquidated a healthy position" >&2; exit 1; fi

step "A. price drops to 2.2: alice's HF 0.94, bob's 1.17; the keeper should liquidate alice only"
publish 2.2
await_liquidation "$ALICE" "$(position_of "$ALICE")"
LEFT=$(bos market withdraw-collateral "${ON[@]}" --owner "$ALICE" --all)
expect "$LEFT" 0 "alice withdrew what the keeper left her"
more "$(bos token balance --token "$TKA" --account "$ALICE")" 0 "alice kept part of her collateral"
more "$(debt "$BOB")" 0 "bob untouched"
grep "liquidated" "$KEEPER_LOG" >&2
echo "keeper: $(grep -c "failed" "$KEEPER_LOG" || true) failed attempts" >&2

step "B. price crashes to 1.2: bob's 100,000 TKA is worth 120,000 against 150,000 of debt"
BEFORE=$(supplied)
publish 1.2
await_liquidation "$BOB" "$(position_of "$BOB")"
AFTER=$(supplied)
echo "supplied before $BEFORE, after $AFTER" >&2
more "$BEFORE" "$AFTER" "bad debt written off against the lender"
grep "bad-debt shares" "$KEEPER_LOG" | tail -1 >&2
"$BOS" status --lending "$LENDING" --market "$MARKET" --owner "$BOB" | tail -4 >&2
"$BOS" status --lending "$LENDING" --market "$MARKET" --owner "$BOB" | grep -q "collateral 0  borrow shares 0 " \
  || { echo "FAIL: bob's position should be empty" >&2; exit 1; }

step "claim the fees; the lender withdraws what's left"
more "$(bos market claim-fees "${ON[@]}")" 0 "fee shares claimed"
GOT=$(bos market withdraw "${ON[@]}" --owner "$LENDER" --all)
echo "lender got $GOT for 2000000000000" >&2
[[ "$GOT" -lt 2000000000000 ]] || { echo "FAIL: lender should have lost to bad debt" >&2; exit 1; }

kill $KEEPER_PID 2> /dev/null || true
cat > .localnet/phase3.env <<ENV
LENDING3=$LENDING
MARKET3=$MARKET
KEEPER=$KEEPER
ENV
step "Phase 3 e2e: PASS (keeper log in $KEEPER_LOG)"
