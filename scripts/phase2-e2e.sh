#!/usr/bin/env bash
# Phase 2 exit gate: the public lifecycle on one market (TKB loan, TKA collateral, TKA/TKB feed):
# create market → supply → supply collateral → borrow → wait → repay → withdraw.
#
# Runs scripts/phase1-e2e.sh first (fresh wallet, tokens, oracle, feed); REUSE_PHASE1=1 reuses its
# wallet and .localnet/phase1.env instead. Needs a running dev-mode localnet (scripts/localnet.md)
# and a release build of `bos` (`cargo build --release -p bosphorus-cli`).
set -euo pipefail
cd "$(dirname "$0")/.."

[[ -n "${REUSE_PHASE1:-}" ]] || scripts/phase1-e2e.sh
# shellcheck source=/dev/null
source .localnet/phase1.env

W=${WALLET:-$HOME/Work/logos-execution-zone/target/release/wallet}
BOS=${BOS:-target/release/bos}
export RISC0_DEV_MODE=1 SUPPRESS_VERBOSE_PRINTS=1
export LEE_WALLET_HOME_DIR=$PWD/.localnet/e2e-wallet
PAYER=$PUBLISHER # genesis account: mint authority, publisher, deployer, admin, fee payer

step() { printf '\n== %s\n' "$*" >&2; }
expect() { [[ "$1" == "$2" ]] || { echo "FAIL: $3: expected $2, got $1" >&2; exit 1; }; }
more() { [[ "$1" -gt "$2" ]] || { echo "FAIL: $3: expected more than $2, got $1" >&2; exit 1; }; }
bos() { "$BOS" "$@" | tail -1; }
new_account() { "$W" account new "$1" < /dev/null | grep -o "$2/[1-9A-HJ-NP-Za-km-z]*" | head -1; }

IRM=(--base 100% --slope1 400% --slope2 3000% --kink 80%) # steep, so 30 s of interest shows

step "lending: deploy, init, enable LLTV 80% and the IRM"
LENDING=$(bos lending deploy --payer $PAYER)
bos lending init --lending "$LENDING" --admin $PAYER --fee-recipient $PAYER --payer $PAYER > /dev/null
bos lending enable-lltv --lending "$LENDING" --admin $PAYER --lltv 80% > /dev/null
bos lending enable-irm --lending "$LENDING" --admin $PAYER "${IRM[@]}" > /dev/null
echo "lending $LENDING" >&2

step "create market: TKB loan, TKA collateral, TKA/TKB feed"
MARKET=$(bos market create --lending "$LENDING" --loan "$TKB" --collateral "$TKA" --oracle "$ORACLE" \
  --feed "$FEED" --lltv 80% "${IRM[@]}" --max-price-age 3600 --payer $PAYER)
echo "market $MARKET" >&2
"$BOS" market list --lending "$LENDING" | grep -q "^$MARKET" || { echo "FAIL: market not listed" >&2; exit 1; }

step "fund a lender and a borrower"
LENDER=$(new_account public Public)
BORROWER=$(new_account public Public)
echo "lender $LENDER, borrower $BORROWER" >&2
bos token mint --token "$TKB" --authority $PAYER --to "$LENDER" --amount 500000000000 > /dev/null
bos token mint --token "$TKA" --authority $PAYER --to "$BORROWER" --amount 400000000000 > /dev/null
bos token mint --token "$TKB" --authority $PAYER --to "$BORROWER" --amount 10000000000 > /dev/null # for interest
ON=(--lending "$LENDING" --market "$MARKET" --payer $PAYER)

step "lender supplies 500,000 TKB"
SHARES=$(bos market supply "${ON[@]}" --from "$LENDER" --assets 500000000000)
more "$SHARES" 0 "supply shares"
expect "$(bos token balance --token "$TKB" --account "$LENDER")" 0 "lender's TKB in the market"

step "borrower supplies 400,000 TKA of collateral (worth 1,000,000 TKB at 2.5)"
expect "$(bos market supply-collateral "${ON[@]}" --from "$BORROWER" --assets 400000000000)" 400000000000 "collateral"

step "borrower borrows 300,000 TKB"
more "$(bos market borrow "${ON[@]}" --owner "$BORROWER" --assets 300000000000)" 0 "debt shares"
expect "$(bos token balance --token "$TKB" --account "$BORROWER")" 310000000000 "borrowed TKB"

step "wait 30 s for interest; refresh the price"
sleep 30
bos oracle publish --oracle "$ORACLE" --publisher $PAYER --name TKA/TKB --price 2.5 \
  --loan-decimals 6 --collateral-decimals 6 > /dev/null
"$BOS" status --lending "$LENDING" --market "$MARKET" --owner "$BORROWER" | tail -9 >&2
more "$(bos status --lending "$LENDING" --market "$MARKET" --owner "$BORROWER")" 300000000000 "debt with interest"

step "borrower repays everything"
PAID=$(bos market repay "${ON[@]}" --from "$BORROWER" --all)
more "$PAID" 300000000000 "repaid with interest"
echo "repaid $PAID" >&2

step "lender withdraws everything"
GOT=$(bos market withdraw "${ON[@]}" --owner "$LENDER" --all)
more "$GOT" 500000000000 "withdrawn with interest"
expect "$(bos token balance --token "$TKB" --account "$LENDER")" "$GOT" "lender's TKB back"
echo "lender got $GOT for 500000000000" >&2

step "borrower withdraws all collateral (no debt, so no price needed)"
expect "$(bos market withdraw-collateral "${ON[@]}" --owner "$BORROWER" --all)" 0 "collateral left"
expect "$(bos token balance --token "$TKA" --account "$BORROWER")" 400000000000 "TKA back"

"$BOS" status --lending "$LENDING" --market "$MARKET" | tail -7 >&2
cat > .localnet/phase2.env <<ENV
LENDING=$LENDING
MARKET=$MARKET
LENDER=$LENDER
BORROWER=$BORROWER
ENV
step "Phase 2 e2e: PASS (ids in .localnet/phase2.env)"
