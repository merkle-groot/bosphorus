#!/usr/bin/env bash
# Price bot: publishes a feed's price every INTERVAL seconds (default 300) and keeps going through
# failures. Each round gets a price, publishes it, then reads the feed back to check the round
# moved; a failed round is retried with backoff and never stops the bot.
#
#   scripts/price-pusher.sh                                  # WBTC/USDC on localnet p6 (usdc-wbtc.env)
#   scripts/price-pusher.sh <oracle> <publisher> <feed name> [start price]
#
# Price source (SOURCE):
#   hold     (default) re-publish the same price, so markets never see it go stale
#   walk     a ±WALK_PCT% (default 1) random walk from the start price
#   cmd      PRICE_CMD prints a decimal price, e.g.
#            PRICE_CMD='curl -s "https://api.coingecko.com/api/v3/simple/price?ids=bitcoin&vs_currencies=usd" | jq .bitcoin.usd'
#            a failed or insane reading (not a positive number, or more than MAX_JUMP_PCT, default 20,
#            from the last price) is dropped and the last good price is re-published instead.
# Other knobs: LOAN_DECIMALS / COLLATERAL_DECIMALS (default 6/8), ATTEMPTS (3), TIMEOUT (120 s per
# bos call), BACKOFF ("10 30 60" s), ALERT_AFTER (3 failed rounds in a row → a loud line in the log).
# Logs to .$NETWORK/logs/price-bot.log. One bot per feed (a lock in .$NETWORK/). Ctrl-C stops it.
set -uo pipefail
cd "$(dirname "$0")/.."
export LEE_WALLET_HOME_DIR=${LEE_WALLET_HOME_DIR:-$PWD/.localnet/p6-wallet}
source scripts/net.sh

if [[ $# -eq 0 ]]; then
  # shellcheck source=/dev/null
  source "$DEPLOY_ENV"
  # shellcheck source=/dev/null
  [[ -f $STATE/usdc-wbtc.env ]] && source "$STATE/usdc-wbtc.env"
  set -- "$ORACLE" "$PAYER" WBTC/USDC 100000
fi
[[ $# -ge 3 ]] || { sed -n '2,20p' "$0" >&2; exit 2; }

ORACLE=$1 PUBLISHER=$2 NAME=$3 PRICE=${4:-100000}
INTERVAL=${INTERVAL:-300}
SOURCE=${SOURCE:-hold}
WALK_PCT=${WALK_PCT:-1}
MAX_JUMP_PCT=${MAX_JUMP_PCT:-20}
ATTEMPTS=${ATTEMPTS:-3}
TIMEOUT=${TIMEOUT:-120}
read -r -a BACKOFF <<< "${BACKOFF:-10 30 60}"
ALERT_AFTER=${ALERT_AFTER:-3}
LOG=$LOGS/price-bot.log

log() { printf '%s %s\n' "$(date '+%F %T')" "$*" | tee -a "$LOG" >&2; }

# One bot per feed: two would race each other's rounds.
LOCK=$STATE/price-bot.${NAME//\//_}.lock
if ! mkdir "$LOCK" 2> /dev/null; then
  old=$(cat "$LOCK/pid" 2> /dev/null)
  if [[ -n $old ]] && kill -0 "$old" 2> /dev/null; then
    echo "a price bot for $NAME already runs (pid $old)" >&2; exit 1
  fi
  rm -rf "$LOCK"; mkdir "$LOCK"   # stale lock from a crashed bot
fi
echo $$ > "$LOCK/pid"
CHILD=
cleanup() { [[ -n $CHILD ]] && kill "$CHILD" 2> /dev/null; rm -rf "$LOCK"; log "stopped"; exit 0; }
trap cleanup INT TERM
trap 'rm -rf "$LOCK"' EXIT

# Runs bos with a time limit (macOS has no `timeout`; an alarm survives exec and kills bos with
# SIGALRM), so a hung RPC can't freeze the bot.
run_bos() {
  local out=$1 rc=0; shift
  perl -e 'alarm shift; exec @ARGV or die "exec: $!\n"' "$TIMEOUT" "$BOS_BIN" "$@" > "$out" 2>&1 &
  CHILD=$!
  wait "$CHILD" || rc=$?
  CHILD=
  [[ $rc -eq 142 ]] && echo "timed out after ${TIMEOUT}s" >> "$out"
  return "$rc"
}

feed_round() {
  local out; out=$(mktemp)
  run_bos "$out" oracle feed --oracle "$ORACLE" --publisher "$PUBLISHER" --name "$NAME" \
    && sed -n 's/.*round: \([0-9]*\).*/\1/p' "$out" | tail -1
  rm -f "$out"
}

is_price() { [[ $1 =~ ^[0-9]+(\.[0-9]+)?$ ]] && awk -v p="$1" 'BEGIN { exit !(p > 0) }'; }

next_price() {
  case $SOURCE in
    hold) echo "$PRICE" ;;
    walk) awk -v p="$PRICE" -v w="$WALK_PCT" -v r="$RANDOM" 'BEGIN { printf "%.6f", p * (1 + w / 100 * (2 * r / 32767 - 1)) }' ;;
    cmd)
      local p
      p=$(bash -c "${PRICE_CMD:?SOURCE=cmd needs PRICE_CMD}" 2> /dev/null | tr -d '[:space:]')
      if ! is_price "$p"; then log "source: bad reading '${p:0:40}', keeping $PRICE"; echo "$PRICE"; return; fi
      if awk -v p="$p" -v l="$PRICE" -v m="$MAX_JUMP_PCT" 'BEGIN { d = (p - l) / l * 100; exit !(d > m || d < -m) }'; then
        log "source: $p is more than ${MAX_JUMP_PCT}% from $PRICE, keeping $PRICE"; echo "$PRICE"; return
      fi
      echo "$p" ;;
    *) log "SOURCE must be hold, walk or cmd"; exit 2 ;;
  esac
}

# One round: publish, then check the feed's round moved (a send can fail after landing, or
# "succeed" without landing).
publish() {
  local price=$1 before after out rc
  before=$(feed_round)
  out=$(mktemp)
  run_bos "$out" oracle publish --oracle "$ORACLE" --publisher "$PUBLISHER" --name "$NAME" --price "$price" \
    --loan-decimals "${LOAN_DECIMALS:-6}" --collateral-decimals "${COLLATERAL_DECIMALS:-8}"
  rc=$?
  grep '^\[measure\]' "$out" | sed "s/^/$(date +%s) /" >> "$MEASURE"
  local err; err=$(grep -v '^\[measure\]' "$out" | tail -2 | tr '\n' ' ')
  rm -f "$out"
  after=$(feed_round)
  if [[ -n $after && -n $before && $after -gt $before ]]; then
    [[ $rc -ne 0 ]] && log "publish reported failure but landed (round $after): $err"
    return 0
  fi
  if [[ $rc -eq 0 && -z $after ]]; then log "published, but couldn't read the feed back"; return 0; fi
  log "publish failed (rc $rc, round ${before:-?} → ${after:-?}): ${err:-no output}"
  return 1
}

log "started: $NAME on $NETWORK ($RPC), every ${INTERVAL}s, source $SOURCE, start $PRICE, wallet $LEE_WALLET_HOME_DIR"
fails=0
while true; do
  t0=$(date +%s)
  price=$(next_price)
  ok=0
  for ((a = 1; a <= ATTEMPTS; a++)); do
    if publish "$price"; then ok=1; break; fi
    if ((a < ATTEMPTS)); then
      wait_s=${BACKOFF[a - 1]:-${BACKOFF[${#BACKOFF[@]} - 1]}}   # bash 3.2: no [-1]
      log "attempt $a/$ATTEMPTS failed; retrying in ${wait_s}s"
      sleep "$wait_s" & CHILD=$!; wait "$CHILD"; CHILD=
    fi
  done
  if ((ok)); then
    PRICE=$price
    ((fails > 0)) && log "recovered after $fails failed round(s)"
    fails=0
    log "published $NAME = $price"
  else
    fails=$((fails + 1))
    log "round failed after $ATTEMPTS attempts ($fails in a row); trying again next round"
    ((fails >= ALERT_AFTER)) && log "ALERT: $NAME not published for $fails rounds; markets refuse prices past their age limit (is the sequencer up at $RPC?)"
  fi
  # Keep the schedule: sleep what's left of the interval.
  left=$((INTERVAL - ($(date +%s) - t0)))
  ((left > 0)) && { sleep "$left" & CHILD=$!; wait "$CHILD"; CHILD=; }
done
