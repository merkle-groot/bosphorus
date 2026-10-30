#!/usr/bin/env bash
# E10 on the dev-mode localnet: screened deposits (Privacy Pools on LEZ).
#   Reuses TKB from scripts/phase5-deploy.sh; deploys the pool with the payer as ASP (id in
#   .localnet/e10.env). A public depositor makes screened shields of native LEZ and TKB into a
#   private holder, and a denied depositor one of TKB. The ASP's scan admits all three; it approves
#   all but the denied one. The holder withdraws part of each approved lot to a fresh public
#   account (each withdrawal passes `bos tx-report --check`: no label, record or holder). The denied
#   deposit and one remainder go back by ragequit; a removed label stops withdrawing.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/net.sh
# shellcheck source=/dev/null
source "$DEPLOY_ENV"
[[ -n "${TKB:-}" ]] || { echo "deploy first: NETWORK=$NETWORK scripts/phase5-deploy.sh" >&2; exit 1; }
DEPLOY_ENV=$STATE/e10.env
touch "$DEPLOY_ENV"
# shellcheck source=/dev/null
source "$DEPLOY_ENV"
REPORTS=$LOGS/e10-reports.txt
START=$(last_block)
: > "$REPORTS"

if [[ -z "${POOL:-}" ]]; then
  step "pool (this build): upload, init with the payer as ASP"
  record_cmd POOL bos pool deploy --asp "$PAYER" --payer "$PAYER"
fi
P=(--pool "$POOL")
bos pool info "${P[@]}" > /dev/null

# A pool withdrawal: its result; fails unless private and nothing in it names the holder or labels.
private_op() {
  local labels=$1 err out tx report
  shift
  err=$(mktemp)
  out=$("$BOS_BIN" "$@" 2> "$err" | tail -1) || { cat "$err" >&2; echo "FAIL: $*" >&2; exit 1; }
  grep '^\[measure\]' "$err" | sed "s/^/$(date +%s) /" >> "$MEASURE" || true
  tx=$(grep -o 'tx [0-9a-f]\{64\} included' "$err" | tail -1 | cut -d' ' -f2)
  grep -q 'kind=private' "$err" || { cat "$err" >&2; echo "FAIL: not a private tx: $*" >&2; exit 1; }
  local scan=(--owner "$HOLDER")
  for l in $labels; do scan+=(--label "$l"); done
  report=$("$BOS_BIN" tx-report --tx "$tx" --pool "$POOL" "${scan[@]}" --check) \
    || { echo "$report" >&2; echo "FAIL: tx-report on $tx (leak?)" >&2; exit 1; }
  { echo "### $*"; echo "$report"; echo; } >> "$REPORTS"
  rm -f "$err"
  echo "$out"
}
fails() { if "$BOS_BIN" "$@" > /dev/null 2>&1; then echo "FAIL: should have been refused: $*" >&2; exit 1; fi; }

step "a depositor (native and TKB), a denied depositor (TKB), a private holder, a recipient"
DEPOSITOR=$(new_account public Public)
DENIED=$(new_account public Public)
HOLDER=$(new_account private Private)
RECIPIENT=$(new_account public Public)
"$W" auth-transfer send --from "$PAYER" --to "$DEPOSITOR" --amount 30000000 < /dev/null > /dev/null
bos token mint --token "$TKB" --authority "$PAYER" --to "$DEPOSITOR" --amount 5000000000 --payer "$PAYER" > /dev/null
bos token mint --token "$TKB" --authority "$PAYER" --to "$DENIED" --amount 1000000000 --payer "$PAYER" > /dev/null

step "screened shields: 20,000,000 native and 5,000 TKB from the depositor, 1,000 TKB from the denied one"
L_NATIVE=$(bos pool deposit "${P[@]}" --from "$DEPOSITOR" --to "$HOLDER" --amount 20000000)
L_TKB=$(bos pool deposit "${P[@]}" --token "$TKB" --from "$DEPOSITOR" --to "$HOLDER" --amount 5000000000)
L_DENIED=$(bos pool deposit "${P[@]}" --token "$TKB" --from "$DENIED" --to "$HOLDER" --amount 1000000000)
expect "$("$BOS_BIN" pool lots "${P[@]}" --holder "$HOLDER" | grep -c ' pending$')" 3 "pending lots"
fails pool deposit "${P[@]}" --token "$TKB" --from "$HOLDER" --to "$HOLDER" --amount 1
fails pool withdraw "${P[@]}" --from "$HOLDER" --to "$RECIPIENT" --amount 1

step "the ASP: scan, then approve what is admitted, except the denied depositor"
SCAN=$("$BOS_BIN" pool scan "${P[@]}" --from-block "$START")
echo "$SCAN" >&2
expect "$(grep -c ' admitted pending$' <<< "$SCAN")" 3 "admitted deposits"
DENY=$(mktemp)
echo "$DENIED" > "$DENY"
APPROVED=$("$BOS_BIN" pool approve "${P[@]}" --asp "$PAYER" --admitted --from-block "$START" --deny "$DENY" --payer "$PAYER")
rm -f "$DENY"
expect "$(sort <<< "$APPROVED" | tr '\n' ' ')" "$(printf '%s\n' "$L_NATIVE" "$L_TKB" | sort | tr '\n' ' ')" "approved labels"

step "withdraw 8,000,000 native and 2,000 TKB to a fresh public account (two private txs)"
expect "$(private_op "$L_NATIVE" pool withdraw "${P[@]}" --from "$HOLDER" --to "$RECIPIENT" --amount 8000000)" 8000000 "recipient native"
expect "$(private_op "$L_TKB" pool withdraw "${P[@]}" --token "$TKB" --from "$HOLDER" --to "$RECIPIENT" --amount 2000000000)" 2000000000 "recipient TKB"
fails pool withdraw "${P[@]}" --token "$TKB" --from "$HOLDER" --to "$RECIPIENT" --amount 3000000001

step "ragequit: the denied deposit back to its depositor, then the rest of the TKB lot"
expect "$(bos pool ragequit "${P[@]}" --holder "$HOLDER" --label "$L_DENIED")" 1000000000 "denied deposit returned"
expect "$(bos token balance --token "$TKB" --account "$DENIED")" 1000000000 "denied depositor's TKB"
expect "$(bos pool ragequit "${P[@]}" --holder "$HOLDER" --label "$L_TKB")" 3000000000 "TKB remainder returned"
fails pool ragequit "${P[@]}" --holder "$HOLDER" --label "$L_TKB"

step "the ASP removes the native label: its remainder can't be withdrawn, only taken back"
bos pool remove "${P[@]}" --asp "$PAYER" --payer "$PAYER" "$L_NATIVE" > /dev/null
expect "$("$BOS_BIN" pool lots "${P[@]}" --holder "$HOLDER")" "$L_NATIVE ${NATIVE_ID:-11111111111111111111111111111111} 12000000 pending" "native lot after removal"
fails pool withdraw "${P[@]}" --from "$HOLDER" --to "$RECIPIENT" --amount 1
expect "$(bos pool ragequit "${P[@]}" --holder "$HOLDER" --label "$L_NATIVE")" 12000000 "native remainder returned"
expect "$("$BOS_BIN" pool lots "${P[@]}" --holder "$HOLDER" | wc -l | tr -d ' ')" 0 "lots left"

echo "E10 OK (pool $POOL; tx reports in $REPORTS)" >&2
