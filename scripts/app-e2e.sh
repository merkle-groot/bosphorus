#!/usr/bin/env bash
# The app API (`bos-app`) on the dev-mode localnet, against the E9 lending and market
# (scripts/e9-held.sh deploys them):
#   1. reads (info, markets, market) and a clean stdout (only protocol lines);
#   2. F5: a borrow quote against a stale feed is refused with `stale_price`;
#   3. a public supply, and a private one into a salted position, as jobs;
#   4. the app is killed right after a private withdraw is sent: on restart the job settles from
#      its saved hash and secrets, and the wallet gets the withdrawn tokens;
#   5. the app is killed while a job is proving (stretched with BOS_APP_DELAY_SEND_MS): its hash is
#      saved only right before sending, so on restart the job is `interrupted` at once, nothing is
#      sent, and the position hasn't moved.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/net.sh
# shellcheck source=/dev/null
source "$DEPLOY_ENV"
# shellcheck source=/dev/null
source "$STATE/e9.env"
[[ -n "${E9_LENDING:-}" && -n "${E9_MARKET:-}" ]] || { echo "deploy first: scripts/e9-held.sh" >&2; exit 1; }
APP_BIN=${APP_BIN:-$PWD/target/release/bos-app}
DATA=$(mktemp -d)
RUN=$DATA/run
mkdir -p "$RUN"
APP_PID=
publish() {
  bos oracle publish --oracle "$ORACLE" --publisher "$PAYER" --name TKA/TKB --price "$1" \
    --loan-decimals 6 --collateral-decimals 6 --payer "$PAYER" > /dev/null
}

# Starts bos-app reading requests from a fifo; its stdout (protocol) goes to $RUN/out.
start_app() {
  rm -f "$RUN/in"
  mkfifo "$RUN/in"
  [[ -f "$RUN/out" ]] && cat "$RUN/out" >> "$RUN/all"
  : > "$RUN/out"
  env "$@" "$APP_BIN" --lending "$E9_LENDING" --data-dir "$DATA" < "$RUN/in" > "$RUN/out" 2>> "$LOGS/app-e2e.err" &
  APP_PID=$!
  exec 3> "$RUN/in"
}
stop_app() {
  exec 3>&-
  wait "$APP_PID" || true
}
kill_app() {
  kill -9 "$APP_PID"
  wait "$APP_PID" 2> /dev/null || true
  exec 3>&-
}
trap '[[ -n "$APP_PID" ]] && kill -9 "$APP_PID" 2> /dev/null || true' EXIT
# call METHOD PARAMS: prints the response line (result or error).
call() {
  # Request ids live in a file: `call` runs in command substitutions (subshells).
  N=$(($(cat "$RUN/n" 2> /dev/null || echo 0) + 1))
  echo "$N" > "$RUN/n"
  local params=${2:-'{}'}
  echo "{\"id\":$N,\"method\":\"$1\",\"params\":$params}" >&3
  local line=
  for _ in $(seq 1 6000); do
    line=$(jq -c "select(.id == $N)" "$RUN/out" 2> /dev/null || true)
    [[ -n "$line" ]] && break
    sleep 0.2
  done
  [[ -n "$line" ]] || { echo "FAIL: no response to $1" >&2; exit 1; }
  echo "$line"
}
# result METHOD PARAMS: the result, or fail on an error.
result() {
  local r
  r=$(call "$@")
  jq -e 'has("result")' <<< "$r" > /dev/null || { echo "FAIL: $1: $r" >&2; exit 1; }
  jq -c .result <<< "$r"
}
# Waits for a job event in STATE.
await_state() {
  for _ in $(seq 1 3000); do
    jq -e --arg id "$1" --arg s "$2" 'select(.event == "job" and .job.id == $id and .job.state == $s)' "$RUN/out" > /dev/null 2>&1 && return
    sleep 0.1
  done
  echo "FAIL: job $1 never reached $2" >&2
  exit 1
}
position() {
  result positions "{\"market\":\"$E9_MARKET\",\"owner\":\"$LENDER\"}" | jq -r '.[] | select(.index == 0) | .supply_shares'
}

step "accounts (before the app starts: it holds the wallet, so nothing else may write it meanwhile)"
LENDER=$(new_account private Private)
PUBLIC_LENDER=$(new_account public Public)
STAGE=$(new_account public Public)
bos token mint --token "$TKB" --authority "$PAYER" --to "$PUBLIC_LENDER" --amount 1000000000 --payer "$PAYER" > /dev/null
bos token mint --token "$TKB" --authority "$PAYER" --to "$STAGE" --amount 100000000000 --payer "$PAYER" > /dev/null
expect "$(bos token shield --token "$TKB" --from "$STAGE" --to "$LENDER" --amount 100000000000)" 100000000000 "shielded TKB"

step "app: reads"
start_app
INFO=$(result info)
expect "$(jq -r '.privacy_circuit_problem, .lending_problem' <<< "$INFO" | sort -u)" null "info problems"
expect "$(result markets | jq -r --arg m "$E9_MARKET" '[.[] | select(.market == $m)] | length')" 1 "markets lists the E9 market"
MARKET=$(result market "{\"market\":\"$E9_MARKET\"}")
jq -c '{supplied, borrowed, utilization, code_problem, feed}' <<< "$MARKET" >&2

step "F5: a borrow quote against a stale feed is refused"
if (( $(jq -r .feed.age_ms <<< "$MARKET") > $(jq -r .max_price_age_ms <<< "$MARKET") )); then
  R=$(call quote "{\"op\":\"borrow\",\"market\":\"$E9_MARKET\",\"owner\":\"$LENDER\",\"amount\":\"1\"}")
  expect "$(jq -r .error.code <<< "$R")" stale_price "borrow quote on a stale feed"
else
  echo "SKIP: the feed is fresh (it goes stale 1 h after the last publish)" >&2
fi
publish 2.5

step "a public supply of 1,000 TKB, as a job"
JOB=$(result submit "{\"op\":\"supply\",\"market\":\"$E9_MARKET\",\"from\":\"$PUBLIC_LENDER\",\"amount\":\"1000000000\",\"payer\":\"$PAYER\"}" | jq -r .id)
DONE=$(result wait "{\"id\":\"$JOB\"}")
expect "$(jq -r .state <<< "$DONE")" done "public supply job"

step "a private supply of 100,000 TKB into a salted position, as a job"
Q=$(result quote "{\"op\":\"supply\",\"market\":\"$E9_MARKET\",\"from\":\"$LENDER\",\"amount\":\"100000000000\"}")
expect "$(jq -r .private <<< "$Q")" true "the quote is private"
JOB=$(result submit "{\"op\":\"supply\",\"market\":\"$E9_MARKET\",\"from\":\"$LENDER\",\"amount\":\"100000000000\"}" | jq -r .id)
DONE=$(result wait "{\"id\":\"$JOB\"}")
expect "$(jq -r .state <<< "$DONE")" done "private supply job"
SHARES=$(position)
expect "$SHARES" "$(jq -r .expect.to <<< "$DONE")" "supply shares as the job expected (a quote is indicative: the job re-reads the market)"

step "killed right after sending a private withdraw: the restart settles it from the saved hash"
HALF=$((SHARES / 2))
JOB=$(result submit "{\"op\":\"withdraw\",\"market\":\"$E9_MARKET\",\"owner\":\"$LENDER\",\"amount\":\"$HALF\"}" | jq -r .id)
await_state "$JOB" submitted
kill_app
start_app
DONE=$(result wait "{\"id\":\"$JOB\"}")
expect "$(jq -r '.state, .note' <<< "$DONE" | paste -sd' ' -)" "done null" "resumed withdraw job (decoded, no F6 note)"
expect "$(position)" $((SHARES - HALF)) "supply shares after the resumed withdraw"
stop_app
# Only the decoded note moves the wallet's private balance.
expect "$(bos token balance --token "$TKB" --account "$LENDER")" "$(jq -r .expect.amount <<< "$DONE")" "private TKB after the resumed withdraw"

step "killed while proving: the restart marks it interrupted and sends nothing"
start_app BOS_APP_DELAY_SEND_MS=60000
JOB=$(result submit "{\"op\":\"withdraw\",\"market\":\"$E9_MARKET\",\"owner\":\"$LENDER\",\"amount\":\"all\"}" | jq -r .id)
await_state "$JOB" proving
kill_app
start_app
await_state "$JOB" interrupted
sleep 20
J=$(result job "{\"id\":\"$JOB\"}")
expect "$(jq -r '.state, .tx' <<< "$J" | paste -sd' ' -)" "interrupted null" "in-flight job after the restart"
expect "$(position)" $((SHARES - HALF)) "supply shares untouched"
stop_app

step "stdout carried only protocol lines"
cat "$RUN/out" >> "$RUN/all"
jq -e . "$RUN/all" > /dev/null || { echo "FAIL: non-JSON on bos-app stdout" >&2; exit 1; }
step "app e2e passed (jobs in $DATA)"
