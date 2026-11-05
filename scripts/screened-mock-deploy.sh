#!/usr/bin/env bash
# A screened mock market for Basecamp, next to the onboarding deploy (scripts/onboard-deploy.sh,
# same STATE): a new lending instance (this build), a pool (ASP = payer), lending registered with
# it, and a screened USDC/WBTC market seeded with 100,000 USDC lent from screened lots. Ids in
# $STATE/screened-mock.env, Basecamp settings in $STATE/basecamp-settings-screened.json.
#   STATE=$PWD/.localnet/onboard scripts/screened-mock-deploy.sh
set -euo pipefail
cd "$(dirname "$0")/.."
STATE=${STATE:-$PWD/.localnet/onboard}
source scripts/net.sh
# shellcheck source=/dev/null
source "$DEPLOY_ENV"
# shellcheck source=/dev/null
source "$STATE/onboard.env"
DEPLOY_ENV=$STATE/screened-mock.env
touch "$DEPLOY_ENV"
# shellcheck source=/dev/null
source "$DEPLOY_ENV"
IRM=(--base 2% --slope1 10% --slope2 100% --kink 80%)
USDC_DROP=25000000000
SEED_CLAIMS=4

[[ -n "${S_BLOCK:-}" ]] || record_cmd S_BLOCK last_block
if [[ -z "${S_LENDING:-}" ]]; then
  step "lending (this build): upload"
  record_cmd S_LENDING bos lending deploy --payer "$PAYER"
fi
if [[ -z "${S_POOL:-}" ]]; then
  step "pool (this build): upload, init with the payer as ASP"
  record_cmd S_POOL bos pool deploy --asp "$PAYER" --payer "$PAYER"
fi
if [[ -z "${S_CONFIGURED:-}" ]]; then
  step "lending: init, LLTV 80%, the IRM, token and pool bytecode; the pool registers lending"
  bos lending init --lending "$S_LENDING" --admin "$PAYER" --fee-recipient "$PAYER" --payer "$PAYER" > /dev/null
  bos lending enable-lltv --lending "$S_LENDING" --admin "$PAYER" --lltv 80% --payer "$PAYER" > /dev/null
  bos lending enable-irm --lending "$S_LENDING" --admin "$PAYER" "${IRM[@]}" --payer "$PAYER" > /dev/null
  bos lending enable-asset --lending "$S_LENDING" --admin "$PAYER" --of "$USDC" --payer "$PAYER" > /dev/null
  bos lending enable-asset --lending "$S_LENDING" --admin "$PAYER" --of "$S_POOL" --payer "$PAYER" > /dev/null
  bos pool set-market --pool "$S_POOL" --asp "$PAYER" --lending "$S_LENDING" --payer "$PAYER" > /dev/null
  record S_CONFIGURED 1
fi
if [[ -z "${S_MARKET:-}" ]]; then
  step "a screened market: borrow USDC against WBTC"
  record_cmd S_MARKET bos market create --lending "$S_LENDING" --loan "$USDC" --collateral "$WBTC" --oracle "$ORACLE" \
    --feed "$WBTC_FEED" --lltv 80% "${IRM[@]}" --max-price-age 7200 --pool "$S_POOL" --payer "$PAYER"
fi
if [[ -z "${S_SEEDED:-}" ]]; then
  step "liquidity: $SEED_CLAIMS USDC claims to the payer, shielded through the pool into a private seeder, lent from its lots"
  [[ -n "${S_SEEDER:-}" ]] || record_cmd S_SEEDER new_account private Private
  for ((i = 0; i < SEED_CLAIMS; i++)); do
    claimer=$(new_account public Public)
    bos faucet claim --faucet "$FAUCET" --asset "$USDC" --claimer "$claimer" --to "$PAYER" --payer "$PAYER" > /dev/null
  done
  from=$(last_block)
  bos pool deposit --pool "$S_POOL" --token "$USDC" --from "$PAYER" --to "$S_SEEDER" --amount $((SEED_CLAIMS * USDC_DROP)) > /dev/null
  bos pool approve --pool "$S_POOL" --asp "$PAYER" --admitted --from-block "$from" --payer "$PAYER" > /dev/null
  bos market supply --lending "$S_LENDING" --market "$S_MARKET" --from "$S_SEEDER" --held --assets $((SEED_CLAIMS * USDC_DROP)) > /dev/null
  record S_SEEDED 1
fi

# An unscreened market next to it, same lending: faucet money (private, not through the pool)
# borrows and lends here, so the onboarding's guided borrow and lend work on this instance too.
if [[ -z "${S_PLAIN_MARKET:-}" ]]; then
  step "an unscreened market: borrow USDC against WBTC"
  record_cmd S_PLAIN_MARKET bos market create --lending "$S_LENDING" --loan "$USDC" --collateral "$WBTC" --oracle "$ORACLE" \
    --feed "$WBTC_FEED" --lltv 80% "${IRM[@]}" --max-price-age 7200 --payer "$PAYER"
fi
if [[ -z "${S_PLAIN_SEEDED:-}" ]]; then
  step "liquidity: $SEED_CLAIMS USDC claims to the payer, lent in public"
  for ((i = 0; i < SEED_CLAIMS; i++)); do
    claimer=$(new_account public Public)
    bos faucet claim --faucet "$FAUCET" --asset "$USDC" --claimer "$claimer" --to "$PAYER" --payer "$PAYER" > /dev/null
  done
  bos market supply --lending "$S_LENDING" --market "$S_PLAIN_MARKET" --from "$PAYER" --assets $((SEED_CLAIMS * USDC_DROP)) > /dev/null
  record S_PLAIN_SEEDED 1
fi

SETTINGS=$STATE/basecamp-settings-screened.json
cat > "$SETTINGS" <<JSON
{
  "sequencer": "$RPC",
  "lending": "$S_LENDING",
  "pool": "$S_POOL",
  "faucet": "$FAUCET",
  "scan_from": $S_BLOCK,
  "tokens": { "$USDC": "USDC", "$WETH": "WETH", "$WBTC": "WBTC" },
  "markets": ["$S_PLAIN_MARKET", "$S_MARKET"]
}
JSON
step "screened market $S_MARKET and unscreened $S_PLAIN_MARKET under $S_LENDING; Basecamp settings in $SETTINGS"
