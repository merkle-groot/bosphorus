#!/usr/bin/env bash
# E9 on the dev-mode localnet: a private lender holds its supply shares in its own private account
# (`--held`), so its supply and withdraw publish no position at all.
#   Reuses the tokens and oracle from scripts/phase5-deploy.sh (their code is unchanged), deploys a
#   fresh lending and market (the guest changed; ids in .localnet/e9.env), then: a public lender
#   supplies, a private lender supplies into its own account, a public borrower borrows, the
#   private lender withdraws everything. Each private tx passes `bos tx-report --check` and
#   touches only the market and the loan vault.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/net.sh
# shellcheck source=/dev/null
source "$DEPLOY_ENV"
[[ -n "${FEED:-}" ]] || { echo "deploy first: NETWORK=$NETWORK scripts/phase5-deploy.sh" >&2; exit 1; }
DEPLOY_ENV=$STATE/e9.env
touch "$DEPLOY_ENV"
# shellcheck source=/dev/null
source "$DEPLOY_ENV"
REPORTS=$LOGS/e9-reports.txt
IRM=(--base 2% --slope1 10% --slope2 100% --kink 80%)
publish() {
  bos oracle publish --oracle "$ORACLE" --publisher "$PAYER" --name TKA/TKB --price "$1" \
    --loan-decimals 6 --collateral-decimals 6 --payer "$PAYER" > /dev/null
}

if [[ -z "${E9_LENDING:-}" ]]; then
  step "lending (this build): upload, init, LLTV 80%, the IRM, the token bytecode"
  record_cmd E9_LENDING bos lending deploy --payer "$PAYER"
  bos lending init --lending "$E9_LENDING" --admin "$PAYER" --fee-recipient "$PAYER" --payer "$PAYER" > /dev/null
  bos lending enable-lltv --lending "$E9_LENDING" --admin "$PAYER" --lltv 80% --payer "$PAYER" > /dev/null
  bos lending enable-irm --lending "$E9_LENDING" --admin "$PAYER" "${IRM[@]}" --payer "$PAYER" > /dev/null
  bos lending enable-asset --lending "$E9_LENDING" --admin "$PAYER" --of "$TKA" --payer "$PAYER" > /dev/null
fi
if [[ -z "${E9_MARKET:-}" ]]; then
  step "market: TKB loan, TKA collateral"
  record_cmd E9_MARKET bos market create --lending "$E9_LENDING" --loan "$TKB" --collateral "$TKA" --oracle "$ORACLE" \
    --feed "$FEED" --lltv 80% "${IRM[@]}" --max-price-age 3600 --payer "$PAYER"
fi
bos check --lending "$E9_LENDING" --oracle "$ORACLE" --token "$TKA" --token "$TKB" > /dev/null
ON=(--lending "$E9_LENDING" --market "$E9_MARKET" --payer "$PAYER")
publish 2.5
: > "$REPORTS"

step "a public lender supplies 1,000,000 TKB (liquidity for the full withdraw)"
PUBLIC_LENDER=$(new_account public Public)
bos token mint --token "$TKB" --authority "$PAYER" --to "$PUBLIC_LENDER" --amount 1000000000000 --payer "$PAYER" > /dev/null
bos market supply "${ON[@]}" --from "$PUBLIC_LENDER" --assets 1000000000000 > /dev/null

step "a private lender, funded by shielding 100,000 TKB"
LENDER=$(new_account private Private)
STAGE=$(new_account public Public)
bos token mint --token "$TKB" --authority "$PAYER" --to "$STAGE" --amount 100000000000 --payer "$PAYER" > /dev/null
expect "$(bos token shield --token "$TKB" --from "$STAGE" --to "$LENDER" --amount 100000000000)" 100000000000 "shielded TKB"

# A held private op: its result; fails on a leak or on any public account but the market's.
held_op() {
  local err out tx report
  err=$(mktemp)
  out=$("$BOS_BIN" "$@" 2> "$err" | tail -1) || { cat "$err" >&2; echo "FAIL: $*" >&2; exit 1; }
  grep '^\[measure\]' "$err" | sed "s/^/$(date +%s) /" >> "$MEASURE" || true
  tx=$(grep -o 'tx [0-9a-f]\{64\} included' "$err" | tail -1 | cut -d' ' -f2)
  [[ -n "$tx" ]] && grep -q '^private tx ' "$err" || { echo "FAIL: not a private tx: $*" >&2; cat "$err" >&2; exit 1; }
  report=$("$BOS_BIN" tx-report --tx "$tx" --owner "$LENDER" --market "$E9_MARKET" --check) \
    || { echo "$report" >&2; echo "FAIL: tx-report on $tx (leak?)" >&2; exit 1; }
  { echo "### $*"; echo "$report"; echo; } >> "$REPORTS"
  grep -q '^  public accounts: 2$' <<< "$report" || { echo "$report" >&2; echo "FAIL: $tx touched more than the market and its vault" >&2; exit 1; }
  rm -f "$err"
  echo "$out"
}

step "held supply: 100,000 TKB into the lender's own private account"
SHARES=$(held_op market supply "${ON[@]}" --from "$LENDER" --held --assets 100000000000)
more "$SHARES" 0 "held shares"
"$BOS_BIN" status --lending "$E9_LENDING" --market "$E9_MARKET" --owner "$LENDER" --held >&2
expect "$(bos token balance --token "$TKB" --account "$LENDER")" 0 "private TKB after the supply"

step "a public borrower: 100,000 TKA of collateral, borrows 50,000 TKB"
BORROWER=$(new_account public Public)
bos token mint --token "$TKA" --authority "$PAYER" --to "$BORROWER" --amount 100000000000 --payer "$PAYER" > /dev/null
bos market supply-collateral "${ON[@]}" --from "$BORROWER" --assets 100000000000 > /dev/null
bos market borrow "${ON[@]}" --owner "$BORROWER" --assets 50000000000 > /dev/null
sleep 30
publish 2.5

step "held withdraw of everything, back to the lender's private account"
ASSETS=$(held_op market withdraw "${ON[@]}" --owner "$LENDER" --held --all)
expect "$(bos token balance --token "$TKB" --account "$LENDER")" "$ASSETS" "private TKB after the withdraw"
echo "supplied 100000000000, withdrew $ASSETS" >&2
step "E9 held scenario passed; reports in $REPORTS"
