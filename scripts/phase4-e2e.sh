#!/usr/bin/env bash
# Phase 4 exit gate: private mode end to end on a dev-mode localnet (fake proofs, same tx shapes).
#   A private owner (a wallet private account, funded by shielding) opens a salted position,
#   borrows to its private account, repays from it, and withdraws the collateral back to it.
#   Every private tx goes through `bos tx-report --check` (no owner id, keys or salt published).
#   Then a second position of the same owner (salt index 1) goes under water and the public keeper,
#   which knows nothing about the owner, finds it in the private txs and liquidates it.
#
# Reuses Phase 1's wallet, tokens and feed (.localnet/phase1.env); run scripts/phase1-e2e.sh first
# on a fresh chain. Deploys its own lending instance. Needs release builds of `bos` and `bos-keeper`.
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck source=/dev/null
source .localnet/phase1.env

W=${WALLET:-$HOME/Work/logos-execution-zone/target/release/wallet}
BOS=${BOS:-target/release/bos}
KEEPER_BIN=${KEEPER_BIN:-target/release/bos-keeper}
export RISC0_DEV_MODE=1 SUPPRESS_VERBOSE_PRINTS=1
export LEE_WALLET_HOME_DIR=$PWD/.localnet/e2e-wallet
PAYER=$PUBLISHER
LOGS=.localnet/logs
mkdir -p $LOGS
REPORTS=$LOGS/phase4-reports.txt
KEEPER_LOG=$LOGS/keeper-phase4.log
: > "$REPORTS"

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
# A private op: prints bos's result, then reports its tx (appended to $REPORTS) and fails on a leak.
# $1: salt index of the position, rest: the bos command.
private_op() {
  local index=$1 err
  shift
  err=$(mktemp)
  "$BOS" "$@" 2> "$err" | tail -1
  grep -q '^private tx ' "$err" || { echo "FAIL: not a private tx: $*" >&2; cat "$err" >&2; exit 1; }
  local tx
  tx=$(grep -o 'tx [0-9a-f]\{64\} included' "$err" | tail -1 | cut -d' ' -f2)
  { echo "### $*"; "$BOS" tx-report --tx "$tx" --owner "$OWNER" --market "$MARKET" --index "$index" --check; echo; } >> "$REPORTS" \
    || { echo "FAIL: tx-report on $tx (leak?)" >&2; tail -20 "$REPORTS" >&2; exit 1; }
  rm -f "$err"
}

START=$(last_block)
IRM=(--base 2% --slope1 10% --slope2 100% --kink 80%)
step "lending: deploy, init, enable LLTV 80% and the IRM; create the market"
LENDING=$(bos lending deploy --payer $PAYER)
bos lending init --lending "$LENDING" --admin $PAYER --fee-recipient $PAYER --payer $PAYER > /dev/null
bos lending enable-lltv --lending "$LENDING" --admin $PAYER --lltv 80% > /dev/null
bos lending enable-irm --lending "$LENDING" --admin $PAYER "${IRM[@]}" > /dev/null
MARKET=$(bos market create --lending "$LENDING" --loan "$TKB" --collateral "$TKA" --oracle "$ORACLE" \
  --feed "$FEED" --lltv 80% "${IRM[@]}" --max-price-age 3600 --payer $PAYER)
ON=(--lending "$LENDING" --market "$MARKET" --payer $PAYER)
echo "lending $LENDING, market $MARKET" >&2

step "a public lender supplies 2,000,000 TKB; price 2.5"
LENDER=$(new_account public Public)
bos token mint --token "$TKB" --authority $PAYER --to "$LENDER" --amount 2000000000000 > /dev/null
bos market supply "${ON[@]}" --from "$LENDER" --assets 2000000000000 > /dev/null
publish 2.5

step "a private owner, funded by shielding 500,000 TKA and 50,000 TKB"
OWNER=$(new_account private Private)
STAGE=$(new_account public Public)
bos token mint --token "$TKA" --authority $PAYER --to "$STAGE" --amount 500000000000 > /dev/null
bos token mint --token "$TKB" --authority $PAYER --to "$STAGE" --amount 50000000000 > /dev/null
expect "$(bos token shield --token "$TKA" --from "$STAGE" --to "$OWNER" --amount 500000000000)" 500000000000 "shielded TKA"
expect "$(bos token shield --token "$TKB" --from "$STAGE" --to "$OWNER" --amount 50000000000)" 50000000000 "shielded TKB"
echo "owner $OWNER" >&2

step "private open: 400,000 TKA of collateral into the salted position (index 0)"
expect "$(private_op 0 market supply-collateral "${ON[@]}" --from "$OWNER" --assets 400000000000)" 400000000000 "collateral"
expect "$(bos token balance --token "$TKA" --account "$OWNER")" 100000000000 "private TKA left"

step "private borrow: 300,000 TKB to the owner's private account"
more "$(private_op 0 market borrow "${ON[@]}" --owner "$OWNER" --assets 300000000000)" 0 "debt shares"
expect "$(bos token balance --token "$TKB" --account "$OWNER")" 350000000000 "private TKB after the borrow"

step "private repay of everything, from the private account"
sleep 20
PAID=$(private_op 0 market repay "${ON[@]}" --from "$OWNER" --all)
more "$PAID" 300000000000 "repaid with interest"
expect "$(bos token balance --token "$TKB" --account "$OWNER")" $((350000000000 - PAID)) "private TKB after the repay"

step "private withdrawal of all the collateral to the private account"
expect "$(private_op 0 market withdraw-collateral "${ON[@]}" --owner "$OWNER" --all)" 0 "collateral left"
expect "$(bos token balance --token "$TKA" --account "$OWNER")" 500000000000 "private TKA back"

step "recovery: the owner's positions in this market, by salt index"
"$BOS" market positions --lending "$LENDING" --market "$MARKET" --owner "$OWNER" >&2
expect "$("$BOS" market positions --lending "$LENDING" --market "$MARKET" --owner "$OWNER" | wc -l | tr -d ' ')" 1 "positions recovered"

step "a second position (index 1): 100,000 TKA, borrow 180,000 TKB (HF 1.11)"
private_op 1 market supply-collateral "${ON[@]}" --from "$OWNER" --index 1 --assets 100000000000 > /dev/null
private_op 1 market borrow "${ON[@]}" --owner "$OWNER" --index 1 --assets 180000000000 > /dev/null
P0=$("$BOS" market positions --lending "$LENDING" --market "$MARKET" --owner "$OWNER" | awk '$1 == 0 { print $2 }')
P1=$("$BOS" market positions --lending "$LENDING" --market "$MARKET" --owner "$OWNER" | awk '$1 == 1 { print $2 }')
[[ -n "$P0" && -n "$P1" && "$P0" != "$P1" ]] || { echo "FAIL: expected two distinct positions" >&2; exit 1; }
echo "positions: index 0 $P0, index 1 $P1" >&2

step "the public keeper (knows nothing of the owner) watches from block $START"
KEEPER=$(new_account public Public)
bos token mint --token "$TKB" --authority $PAYER --to "$KEEPER" --amount 1000000000000 > /dev/null
"$W" auth-transfer send --from $PAYER --to "$KEEPER" --amount 1000000000 < /dev/null > /dev/null
"$KEEPER_BIN" --lending "$LENDING" --liquidator "$KEEPER" --interval 5 --from-block "$START" > "$KEEPER_LOG" 2>&1 &
KEEPER_PID=$!
trap 'kill $KEEPER_PID 2> /dev/null || true' EXIT
sleep 20
grep -q "found position $P1" "$KEEPER_LOG" || { echo "FAIL: keeper didn't find the private position" >&2; cat "$KEEPER_LOG" >&2; exit 1; }

step "price drops to 2.2: position 1 is under water; the keeper liquidates it"
publish 2.2
for _ in $(seq 48); do
  grep -q "liquidated $P1" "$KEEPER_LOG" && break
  sleep 5
done
grep "liquidated $P1" "$KEEPER_LOG" >&2 || { echo "FAIL: keeper didn't liquidate $P1" >&2; tail "$KEEPER_LOG" >&2; exit 1; }
"$BOS" market positions --lending "$LENDING" --market "$MARKET" --owner "$OWNER" >&2
kill $KEEPER_PID 2> /dev/null || true

cat > .localnet/phase4.env <<ENV
LENDING4=$LENDING
MARKET4=$MARKET
OWNER=$OWNER
ENV
step "Phase 4 e2e: PASS (tx reports in $REPORTS)"
