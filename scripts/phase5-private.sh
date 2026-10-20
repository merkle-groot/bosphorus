#!/usr/bin/env bash
# Phase 5.2: the private scenario on a deployed market (scripts/phase5-deploy.sh), every op measured.
#   A private owner (funded by shielding) opens a salted position, borrows to itself, repays and
#   withdraws, each tx passing `bos tx-report --check`; then a second position (salt index 1) goes
#   under water and the public keeper liquidates it. SKIP_LIQUIDATION=1 stops after the lifecycle.
#
#   NETWORK=testnet scripts/phase5-private.sh    (default NETWORK=localnet)
#
# On testnet every private tx is a real proof, ~10–11 min each on the M1 (S4, P4): ~80 min with
# the liquidation part. Close heavy apps first. Reports go to .$NETWORK/logs/private-reports.txt.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/net.sh
# shellcheck source=/dev/null
source "$DEPLOY_ENV"
[[ -n "${MARKET:-}" ]] || { echo "deploy first: NETWORK=$NETWORK scripts/phase5-deploy.sh" >&2; exit 1; }

REPORTS=$LOGS/private-reports.txt
KEEPER_LOG=$LOGS/keeper-private.log
KEEPER_WAIT=${KEEPER_WAIT:-600}
KEEPER_NATIVE=${KEEPER_NATIVE:-50000000}
ON=(--lending "$LENDING" --market "$MARKET" --payer "$PAYER")
publish() {
  bos oracle publish --oracle "$ORACLE" --publisher "$PAYER" --name TKA/TKB --price "$1" \
    --loan-decimals 6 --collateral-decimals 6 --payer "$PAYER" > /dev/null
}
# A private op: prints its result, measures it, and fails on a leak (tx-report --check).
# $1: salt index of the position, rest: the bos command.
private_op() {
  local index=$1 err out tx
  shift
  err=$(mktemp)
  out=$("$BOS_BIN" "$@" 2> "$err" | tail -1) || { cat "$err" >&2; echo "FAIL: $*" >&2; exit 1; }
  grep '^\[measure\]' "$err" | sed "s/^/$(date +%s) /" >> "$MEASURE" || true
  grep -q '^private tx ' "$err" || { echo "FAIL: not a private tx: $*" >&2; cat "$err" >&2; exit 1; }
  tx=$(grep -o 'tx [0-9a-f]\{64\} included' "$err" | tail -1 | cut -d' ' -f2)
  { echo "### $*"; "$BOS_BIN" tx-report --tx "$tx" --owner "$OWNER" --market "$MARKET" --index "$index" --check; echo; } >> "$REPORTS" 2>&1 \
    || { echo "FAIL: tx-report on $tx (leak?)" >&2; tail -20 "$REPORTS" >&2; exit 1; }
  rm -f "$err"
  echo "$out"
}
# The block timestamp of the last [measure] line for op $1 in file $2.
measured_ts() { grep "op=$1 " "$2" | tail -1 | grep -o 'block_ts_ms=[0-9]*' | cut -d= -f2; }

START=$(last_block)
echo "phase5-private on $NETWORK from block $START" >> "$MEASURE"
: > "$REPORTS"

step "a public lender supplies 2,000,000 TKB; price 2.5"
LENDER=$(new_account public Public)
bos token mint --token "$TKB" --authority "$PAYER" --to "$LENDER" --amount 2000000000000 --payer "$PAYER" > /dev/null
bos market supply "${ON[@]}" --from "$LENDER" --assets 2000000000000 > /dev/null
publish 2.5

step "a private owner, funded by shielding 500,000 TKA and 50,000 TKB (two private txs)"
OWNER=$(new_account private Private)
STAGE=$(new_account public Public)
bos token mint --token "$TKA" --authority "$PAYER" --to "$STAGE" --amount 500000000000 --payer "$PAYER" > /dev/null
bos token mint --token "$TKB" --authority "$PAYER" --to "$STAGE" --amount 50000000000 --payer "$PAYER" > /dev/null
# The stage account signs the shields, so it pays nothing (private txs are fee-exempt).
expect "$(bos token shield --token "$TKA" --from "$STAGE" --to "$OWNER" --amount 500000000000)" 500000000000 "shielded TKA"
expect "$(bos token shield --token "$TKB" --from "$STAGE" --to "$OWNER" --amount 50000000000)" 50000000000 "shielded TKB"
echo "owner $OWNER" >&2

step "private open: 400,000 TKA of collateral into the salted position (index 0)"
expect "$(private_op 0 market supply-collateral "${ON[@]}" --from "$OWNER" --assets 400000000000)" 400000000000 "collateral"
step "private borrow: 300,000 TKB to the owner's private account"
more "$(private_op 0 market borrow "${ON[@]}" --owner "$OWNER" --assets 300000000000)" 0 "debt shares"
expect "$(bos token balance --token "$TKB" --account "$OWNER")" 350000000000 "private TKB after the borrow"
step "private repay of everything, from the private account"
PAID=$(private_op 0 market repay "${ON[@]}" --from "$OWNER" --all)
more "$PAID" 300000000000 "repaid with interest"
step "private withdrawal of all the collateral to the private account"
expect "$(private_op 0 market withdraw-collateral "${ON[@]}" --owner "$OWNER" --all)" 0 "collateral left"
expect "$(bos token balance --token "$TKA" --account "$OWNER")" 500000000000 "private TKA back"

step "recovery: the owner's positions in this market, by salt index"
expect "$("$BOS_BIN" market positions --lending "$LENDING" --market "$MARKET" --owner "$OWNER" 2> /dev/null | wc -l | tr -d ' ')" 1 "positions recovered"

if [[ -z "${SKIP_LIQUIDATION:-}" ]]; then
  step "a second position (index 1): 100,000 TKA, borrow 180,000 TKB (HF 1.11)"
  private_op 1 market supply-collateral "${ON[@]}" --from "$OWNER" --index 1 --assets 100000000000 > /dev/null
  private_op 1 market borrow "${ON[@]}" --owner "$OWNER" --index 1 --assets 180000000000 > /dev/null
  P1=$("$BOS_BIN" market positions --lending "$LENDING" --market "$MARKET" --owner "$OWNER" 2> /dev/null | awk '$1 == 1 { print $2 }')
  [[ -n "$P1" ]] || { echo "FAIL: position 1 not found" >&2; exit 1; }

  step "the public keeper (knows nothing of the owner) watches from block $START"
  KEEPER=$(new_account public Public)
  bos token mint --token "$TKB" --authority "$PAYER" --to "$KEEPER" --amount 1000000000000 --payer "$PAYER" > /dev/null
  "$W" auth-transfer send --from "$PAYER" --to "$KEEPER" --amount "$KEEPER_NATIVE" < /dev/null > /dev/null
  "$KEEPER_BIN" --lending "$LENDING" --liquidator "$KEEPER" --publisher "$PAYER" --interval 5 --from-block "$START" \
    > "$KEEPER_LOG" 2>&1 &
  KEEPER_PID=$!
  disown $KEEPER_PID
  trap 'kill $KEEPER_PID 2> /dev/null || true' EXIT
  for _ in $(seq $((KEEPER_WAIT / 5))); do
    grep -q "found position $P1" "$KEEPER_LOG" && break
    sleep 5
  done
  grep -q "found position $P1" "$KEEPER_LOG" || { echo "FAIL: keeper didn't find the private position" >&2; cat "$KEEPER_LOG" >&2; exit 1; }

  step "price drops to 2.2: position 1 is under water; the keeper liquidates it"
  publish 2.2
  DROP_TS=$(measured_ts oracle.publish "$MEASURE")
  for _ in $(seq $((KEEPER_WAIT / 5))); do
    grep -q "liquidated $P1" "$KEEPER_LOG" && break
    sleep 5
  done
  grep "liquidated $P1" "$KEEPER_LOG" >&2 || { echo "FAIL: keeper didn't liquidate $P1" >&2; tail "$KEEPER_LOG" >&2; exit 1; }
  LIQ_TS=$(measured_ts lending.liquidate "$KEEPER_LOG")
  echo "$(date +%s) [measure] op=keeper.latency kind=private-position price_block_ts_ms=$DROP_TS liquidation_block_ts_ms=$LIQ_TS latency_ms=$((LIQ_TS - DROP_TS))" >> "$MEASURE"
  grep '^\[measure\]' "$KEEPER_LOG" | sed "s/^/$(date +%s) /" >> "$MEASURE" || true
  kill $KEEPER_PID 2> /dev/null || true
fi

END=$(last_block)
"$BOS_BIN" blocks --from "$START" --to "$END" --summary 2> /dev/null | tee -a "$MEASURE" >&2
step "Phase 5 private scenario on $NETWORK: PASS (blocks $START..$END, reports in $REPORTS)"
