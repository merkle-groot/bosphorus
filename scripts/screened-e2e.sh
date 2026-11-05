#!/usr/bin/env bash
# Screened markets (design/screened-markets.md) on the dev-mode localnet.
#   Needs scripts/phase5-deploy.sh in the same STATE (TKA, TKB, oracle, lending, payer). Deploys a
#   pool (ASP = payer), enables its bytecode in lending, registers lending with it and creates a
#   screened TKB/TKA market (ids in $STATE/screened.env). A public depositor shields TKB into a
#   private lender's lots and TKA into a private borrower's; the ASP approves them. The lender
#   lends from its lots (held shares), the borrower posts collateral from its lots and borrows into
#   its market lot, repays from it, and takes the rest out of the pool to a public account; the
#   lender withdraws into its market lot. Direct and unapproved payments are refused.
#
#   A separate localnet, so the onboarding one on :3040 stays as it is:
#   RPC=http://127.0.0.1:3050 STATE=$PWD/.localnet/screened LEE_WALLET_HOME_DIR=$PWD/.localnet/screened/wallet \
#     scripts/phase5-deploy.sh && (same env) scripts/screened-e2e.sh
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/net.sh
# shellcheck source=/dev/null
source "$DEPLOY_ENV"
[[ -n "${LENDING_CONFIGURED:-}" ]] || { echo "deploy first: scripts/phase5-deploy.sh (same env)" >&2; exit 1; }
DEPLOY_ENV=$STATE/screened.env
touch "$DEPLOY_ENV"
# shellcheck source=/dev/null
source "$DEPLOY_ENV"
REPORTS=$LOGS/screened-reports.txt
: > "$REPORTS"
IRM=(--base 2% --slope1 10% --slope2 100% --kink 80%)

if [[ -z "${POOL:-}" ]]; then
  step "pool (this build): upload, init with the payer as ASP"
  record_cmd POOL bos pool deploy --asp "$PAYER" --payer "$PAYER"
fi
P=(--pool "$POOL")
if [[ -z "${POOL_WIRED:-}" ]]; then
  step "lending trusts the pool's bytecode; the pool lets lending pay into market lots"
  bos lending enable-asset --lending "$LENDING" --admin "$PAYER" --of "$POOL" --payer "$PAYER" > /dev/null
  bos pool set-market "${P[@]}" --asp "$PAYER" --lending "$LENDING" --payer "$PAYER" > /dev/null
  record POOL_WIRED 1
fi
"$BOS_BIN" pool info "${P[@]}" | grep -q "^market $LENDING$" || { echo "FAIL: lending isn't registered" >&2; exit 1; }
if [[ -z "${SMARKET:-}" ]]; then
  step "a screened TKB/TKA market"
  record_cmd SMARKET bos market create --lending "$LENDING" --loan "$TKB" --collateral "$TKA" --oracle "$ORACLE" \
    --feed "$FEED" --lltv 80% "${IRM[@]}" --max-price-age 3600 --pool "$POOL" --payer "$PAYER"
fi
ON=(--lending "$LENDING" --market "$SMARKET")
bos oracle publish --oracle "$ORACLE" --publisher "$PAYER" --name TKA/TKB --price 2.5 \
  --loan-decimals 6 --collateral-decimals 6 --payer "$PAYER" > /dev/null

fails() { if "$BOS_BIN" "$@" > /dev/null 2>&1; then echo "FAIL: should have been refused: $*" >&2; exit 1; fi; }
lot() { "$BOS_BIN" pool lots "${P[@]}" --holder "$1" | awk -v a="${2#Public/}" -v s="$3" '$2 == a && $4 == s { t += $3 } END { print t + 0 }'; }
# A private op: its result; fails unless private and nothing in it names `$1` (a private account).
private_op() {
  local who=$1 err out tx report
  shift
  err=$(mktemp)
  out=$("$BOS_BIN" "$@" 2> "$err" | tail -1) || { cat "$err" >&2; echo "FAIL: $*" >&2; exit 1; }
  grep '^\[measure\]' "$err" | sed "s/^/$(date +%s) /" >> "$MEASURE" || true
  tx=$(grep -o 'tx [0-9a-f]\{64\} included' "$err" | tail -1 | cut -d' ' -f2)
  grep -q 'kind=private' "$err" || { cat "$err" >&2; echo "FAIL: not a private tx: $*" >&2; exit 1; }
  report=$("$BOS_BIN" tx-report --tx "$tx" --owner "$who" --check) \
    || { echo "$report" >&2; echo "FAIL: tx-report on $tx (leak?)" >&2; exit 1; }
  { echo "### $*"; echo "$report"; echo; } >> "$REPORTS"
  rm -f "$err"
  echo "$out"
}

step "a public depositor, a private lender and borrower, a public recipient"
DEPOSITOR=$(new_account public Public)
LENDER=$(new_account private Private)
BORROWER=$(new_account private Private)
RECIPIENT=$(new_account public Public)
"$W" auth-transfer send --from "$PAYER" --to "$DEPOSITOR" --amount 30000000 < /dev/null > /dev/null
bos token mint --token "$TKB" --authority "$PAYER" --to "$DEPOSITOR" --amount 6000000000 --payer "$PAYER" > /dev/null
bos token mint --token "$TKA" --authority "$PAYER" --to "$DEPOSITOR" --amount 2000000000 --payer "$PAYER" > /dev/null
START=$(last_block)

step "screened shields: 5,000 TKB to the lender, 2,000 TKA to the borrower, 100 TKB left pending"
bos pool deposit "${P[@]}" --token "$TKB" --from "$DEPOSITOR" --to "$LENDER" --amount 5000000000 > /dev/null
bos pool deposit "${P[@]}" --token "$TKA" --from "$DEPOSITOR" --to "$BORROWER" --amount 2000000000 > /dev/null
step "refused: lending from lots that aren't approved yet"
fails market supply "${ON[@]}" --from "$LENDER" --held --assets 1000000000
bos pool approve "${P[@]}" --asp "$PAYER" --admitted --from-block "$START" --payer "$PAYER" > /dev/null
PENDING=$(bos pool deposit "${P[@]}" --token "$TKB" --from "$DEPOSITOR" --to "$BORROWER" --amount 100000000)
expect "$(lot "$LENDER" "$TKB" approved)" 5000000000 "lender's approved TKB"

step "refused: a direct payment (the depositor's own TKB) and more than the approved lots"
fails market supply "${ON[@]}" --from "$DEPOSITOR" --assets 1000000
fails market supply "${ON[@]}" --from "$LENDER" --held --assets 5000000001
fails market repay "${ON[@]}" --from "$BORROWER" --shares 1

step "the lender lends 4,000 TKB from its lots, held on its own account"
SHARES=$(private_op "$LENDER" market supply "${ON[@]}" --from "$LENDER" --held --assets 4000000000)
expect "$(lot "$LENDER" "$TKB" approved)" 1000000000 "lender's lots after lending"

step "the borrower posts 2,000 TKA from its lots and borrows 1,000 TKB into its market lot"
private_op "$BORROWER" market supply-collateral "${ON[@]}" --from "$BORROWER" --assets 2000000000 > /dev/null
DEBT=$(private_op "$BORROWER" market borrow "${ON[@]}" --owner "$BORROWER" --assets 1000000000)
expect "$(lot "$BORROWER" "$TKB" market)" 1000000000 "borrower's TKB market lot"
expect "$(lot "$BORROWER" "$TKB" pending)" 100000000 "the pending lot is untouched"

step "repay half from the market lot; take 300 TKB out of the pool to a public account"
private_op "$BORROWER" market repay "${ON[@]}" --from "$BORROWER" --shares $((DEBT / 2)) > /dev/null
private_op "$BORROWER" pool withdraw "${P[@]}" --token "$TKB" --from "$BORROWER" --to "$RECIPIENT" --amount 300000000 > /dev/null
expect "$("$BOS_BIN" token balance --token "$TKB" --account "$RECIPIENT" 2> /dev/null | tail -1)" 300000000 "recipient TKB"

step "the lender withdraws 1,000 TKB of shares into its market lot"
private_op "$LENDER" market withdraw "${ON[@]}" --owner "$LENDER" --held --shares $((SHARES / 4)) > /dev/null
more "$(lot "$LENDER" "$TKB" market)" 0 "lender's TKB market lot"

step "screened markets work end to end; reports in $REPORTS"
echo "pending lot $PENDING stays with the borrower until the ASP approves it (or its depositor ragequits)" >&2
