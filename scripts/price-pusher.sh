#!/usr/bin/env bash
# Dev price pusher: publishes a price to a feed every INTERVAL seconds.
#
#   scripts/price-pusher.sh <oracle> <publisher> <feed name> [start price]
#
# By default the price takes a ±1% random walk from the start price. Set PRICE_CMD to any command
# that prints a decimal price to use a real source instead, e.g.
#   PRICE_CMD='curl -s "https://api.coingecko.com/api/v3/simple/price?ids=ethereum&vs_currencies=usd" | jq .ethereum.usd'
# LOAN_DECIMALS / COLLATERAL_DECIMALS (default 6/6) convert it to base units. NETWORK=testnet uses
# the Phase 5 testnet wallet (scripts/net.sh); with no arguments it pushes to the deployed feed.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/net.sh
if [[ $# -eq 0 && -f "$DEPLOY_ENV" ]]; then
  # shellcheck source=/dev/null
  source "$DEPLOY_ENV"
  set -- "$ORACLE" "$PAYER" TKA/TKB
fi

ORACLE=$1 PUBLISHER=$2 NAME=$3 PRICE=${4:-2.5}
BOS=$BOS_BIN
INTERVAL=${INTERVAL:-30}

while true; do
  if [[ -n "${PRICE_CMD:-}" ]]; then
    PRICE=$(bash -c "$PRICE_CMD")
  else
    PRICE=$(awk -v p="$PRICE" -v r="$RANDOM" 'BEGIN { printf "%.6f", p * (0.99 + 0.02 * r / 32767) }')
  fi
  "$BOS" oracle publish --oracle "$ORACLE" --publisher "$PUBLISHER" --name "$NAME" --price "$PRICE" \
    --loan-decimals "${LOAN_DECIMALS:-6}" --collateral-decimals "${COLLATERAL_DECIMALS:-6}" \
    2> >(grep '^\[measure\]' | sed "s/^/$(date +%s) /" >> "$MEASURE") \
    || echo "publish failed; retrying next round" >&2
  sleep "$INTERVAL"
done
