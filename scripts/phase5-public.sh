#!/usr/bin/env bash
# Phase 5.2: the public scenario on a deployed market (scripts/phase5-deploy.sh), every op measured.
#   A lender supplies; alice and bob borrow; bob withdraws a little collateral while in debt;
#   the price drops and the keeper liquidates alice on its own; bob repays and leaves; fees are
#   claimed; the lender withdraws.
#
#   NETWORK=testnet scripts/phase5-public.sh     (default NETWORK=localnet)
#
# [measure] lines go to .$NETWORK/logs/measure.log (summary: scripts/measure-summary.sh), the
# keeper log to .$NETWORK/logs/keeper-public.log. Every public tx is paid by the deploy payer;
# the keeper pays its own fees.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/net.sh
# shellcheck source=/dev/null
source "$DEPLOY_ENV"
[[ -n "${MARKET:-}" ]] || { echo "deploy first: NETWORK=$NETWORK scripts/phase5-deploy.sh" >&2; exit 1; }

KEEPER_LOG=$LOGS/keeper-public.log
# How long to wait for the keeper (10 s blocks, plus the testnet's stalls).
KEEPER_WAIT=${KEEPER_WAIT:-600}
KEEPER_NATIVE=${KEEPER_NATIVE:-50000000}
ON=(--lending "$LENDING" --market "$MARKET" --payer "$PAYER")
publish() {
  bos oracle publish --oracle "$ORACLE" --publisher "$PAYER" --name TKA/TKB --price "$1" \
    --loan-decimals 6 --collateral-decimals 6 --payer "$PAYER" > /dev/null
}
debt() { bos status --lending "$LENDING" --market "$MARKET" --owner "$1"; }
position_of() {
  "$BOS_BIN" status --lending "$LENDING" --market "$MARKET" --owner "$1" 2> /dev/null \
    | grep -o '^position [1-9A-HJ-NP-Za-km-z]*' | cut -d' ' -f2
}
# The block timestamp of the last [measure] line for op $1 in file $2.
measured_ts() { grep "op=$1 " "$2" | tail -1 | grep -o 'block_ts_ms=[0-9]*' | cut -d= -f2; }

START=$(last_block)
echo "phase5-public on $NETWORK from block $START" >> "$MEASURE"

step "accounts: a lender, alice, bob and the keeper (funded with native for its own fees)"
LENDER=$(new_account public Public)
ALICE=$(new_account public Public)
BOB=$(new_account public Public)
KEEPER=$(new_account public Public)
bos token mint --token "$TKB" --authority "$PAYER" --to "$LENDER" --amount 2000000000000 --payer "$PAYER" > /dev/null
bos token mint --token "$TKA" --authority "$PAYER" --to "$ALICE" --amount 400000000000 --payer "$PAYER" > /dev/null
bos token mint --token "$TKA" --authority "$PAYER" --to "$BOB" --amount 100000000000 --payer "$PAYER" > /dev/null
bos token mint --token "$TKB" --authority "$PAYER" --to "$BOB" --amount 10000000000 --payer "$PAYER" > /dev/null
bos token mint --token "$TKB" --authority "$PAYER" --to "$KEEPER" --amount 1000000000000 --payer "$PAYER" > /dev/null
"$W" auth-transfer send --from "$PAYER" --to "$KEEPER" --amount "$KEEPER_NATIVE" < /dev/null > /dev/null
echo "lender $LENDER, alice $ALICE, bob $BOB, keeper $KEEPER" >&2

step "10% fee; lender supplies 2,000,000 TKB; price 2.5"
bos lending set-fee --lending "$LENDING" --admin "$PAYER" --market "$MARKET" --fee 10% --payer "$PAYER" > /dev/null
more "$(bos market supply "${ON[@]}" --from "$LENDER" --assets 2000000000000)" 0 "supply shares"
publish 2.5

step "alice: 400,000 TKA (worth 1,000,000), borrows 750,000 (HF 1.07)"
expect "$(bos market supply-collateral "${ON[@]}" --from "$ALICE" --assets 400000000000)" 400000000000 "alice's collateral"
more "$(bos market borrow "${ON[@]}" --owner "$ALICE" --assets 750000000000)" 0 "alice's debt shares"
step "bob: 100,000 TKA, borrows 150,000 (HF 1.33), withdraws 10,000 TKA while in debt (HF 1.2)"
bos market supply-collateral "${ON[@]}" --from "$BOB" --assets 100000000000 > /dev/null
bos market borrow "${ON[@]}" --owner "$BOB" --assets 150000000000 > /dev/null
expect "$(bos market withdraw-collateral "${ON[@]}" --owner "$BOB" --assets 10000000000)" 90000000000 "bob's collateral"

step "the keeper starts (trusting the payer's feed) and must leave both positions alone"
"$KEEPER_BIN" --lending "$LENDING" --liquidator "$KEEPER" --publisher "$PAYER" --interval 5 --from-block "$START" \
  > "$KEEPER_LOG" 2>&1 &
KEEPER_PID=$!
disown $KEEPER_PID
trap 'kill $KEEPER_PID 2> /dev/null || true' EXIT
for _ in $(seq $((KEEPER_WAIT / 5))); do
  [[ $(grep -c "found position" "$KEEPER_LOG" || true) -ge 2 ]] && break
  sleep 5
done
[[ $(grep -c "found position" "$KEEPER_LOG" || true) -ge 2 ]] || { echo "FAIL: keeper didn't find both positions" >&2; cat "$KEEPER_LOG" >&2; exit 1; }
if grep -q "liquidated" "$KEEPER_LOG"; then echo "FAIL: keeper liquidated a healthy position" >&2; exit 1; fi

step "price drops to 2.2: alice's HF 0.94, bob's 1.06; the keeper should liquidate alice only"
P_ALICE=$(position_of "$ALICE")
publish 2.2
DROP_TS=$(measured_ts oracle.publish "$MEASURE")
for _ in $(seq $((KEEPER_WAIT / 5))); do
  grep -q "liquidated $P_ALICE" "$KEEPER_LOG" && break
  sleep 5
done
grep "liquidated $P_ALICE" "$KEEPER_LOG" >&2 || { echo "FAIL: keeper didn't liquidate alice" >&2; tail -20 "$KEEPER_LOG" >&2; exit 1; }
LIQ_TS=$(measured_ts lending.liquidate "$KEEPER_LOG")
echo "$(date +%s) [measure] op=keeper.latency kind=public price_block_ts_ms=$DROP_TS liquidation_block_ts_ms=$LIQ_TS latency_ms=$((LIQ_TS - DROP_TS))" >> "$MEASURE"
echo "keeper latency: $(((LIQ_TS - DROP_TS) / 1000)) s from the price block to the liquidation block" >&2
grep '^\[measure\]' "$KEEPER_LOG" | sed "s/^/$(date +%s) /" >> "$MEASURE" || true
expect "$(debt "$ALICE")" 0 "alice's debt after the liquidation"
more "$(debt "$BOB")" 0 "bob untouched"
kill $KEEPER_PID 2> /dev/null || true

step "alice takes back what the keeper left her (no debt, no price needed)"
expect "$(bos market withdraw-collateral "${ON[@]}" --owner "$ALICE" --all)" 0 "alice's collateral left"
more "$(bos token balance --token "$TKA" --account "$ALICE")" 0 "alice kept part of her collateral"

step "bob repays everything and withdraws his collateral"
# Not necessarily more than he borrowed: the liquidator's overpayment (the claim's `ahead` of
# projected interest) lowered the market's borrow assets, which only bob still owes (design §5).
echo "bob repaid $(bos market repay "${ON[@]}" --from "$BOB" --all) for 150000000000" >&2
expect "$(debt "$BOB")" 0 "bob's debt after repaying"
expect "$(bos market withdraw-collateral "${ON[@]}" --owner "$BOB" --all)" 0 "bob's collateral left"

step "fees are claimed; the lender withdraws everything"
more "$(bos market claim-fees "${ON[@]}")" 0 "fee shares claimed"
GOT=$(bos market withdraw "${ON[@]}" --owner "$LENDER" --all)
more "$GOT" 2000000000000 "the lender earned interest"
echo "lender got $GOT for 2000000000000" >&2

END=$(last_block)
"$BOS_BIN" blocks --from "$START" --to "$END" --summary 2> /dev/null | tee -a "$MEASURE" >&2
step "Phase 5 public scenario on $NETWORK: PASS (blocks $START..$END)"
