#!/usr/bin/env bash
# Onboarding deploy: USDC (loan), WETH and WBTC (collateral), all minted by the faucet; the faucet
# (daily drops of each, and gas); the oracle with WETH/USDC and WBTC/USDC feeds at live prices;
# lending with USDC/WETH and USDC/WBTC markets; USDC liquidity in both, claimed from the faucet.
# Writes the Basecamp module's settings.json for the network.
#
#   NETWORK=localnet scripts/onboard-deploy.sh
#   NETWORK=testnet  scripts/onboard-deploy.sh
#
# Ids go to .$NETWORK/onboard.env as each step finishes; a rerun skips what's recorded. On testnet
# it reuses the deployed token bytecode and oracle (deploy.env), and the payer of deploy.env.
# Needs release builds of `bos` and LEZ's `wallet`.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/net.sh
ENV=$STATE/onboard.env

# Drops per claim (base units) and the cooldown. 25,000 USDC; 5 WETH; 0.15 WBTC; 15M native gas.
USDC_DROP=${USDC_DROP:-25000000000}
WETH_DROP=${WETH_DROP:-5000000000000000000}
WBTC_DROP=${WBTC_DROP:-15000000}
GAS_DROP=${GAS_DROP:-15000000}
COOLDOWN=${COOLDOWN:-86400}
# Native for the faucet's gas custody (10 newcomers' first gas), and USDC liquidity per market (in
# claims of USDC_DROP).
CUSTODY_NATIVE=${CUSTODY_NATIVE:-150000000}
SEED_CLAIMS=${SEED_CLAIMS:-4}
# Markets refuse prices older than this; an hourly pusher needs headroom (a private op checks at the
# end of its 20 min window).
MAX_PRICE_AGE=${MAX_PRICE_AGE:-7200}

record() { echo "$1=$2" >> "$ENV"; eval "$1=\$2"; }
record_cmd() {
  local var=$1 out
  shift
  out=$("$@") || { echo "FAIL: $var: $*" >&2; exit 1; }
  [[ -n "$out" ]] || { echo "FAIL: $var: no output from $*" >&2; exit 1; }
  record "$var" "$out"
}
# A live USD price from Coinbase (ETH, BTC); FALLBACK if it can't be read.
spot() {
  local p
  p=$(curl -s -m 10 "https://api.coinbase.com/v2/prices/$1-USD/spot" | grep -o '"amount":"[0-9.]*"' | cut -d'"' -f4)
  [[ $p =~ ^[0-9]+(\.[0-9]+)?$ ]] && echo "$p" || echo "$2"
}

step "$NETWORK: wallet $LEE_WALLET_HOME_DIR, ids in $ENV"
rpc getLastBlockId > /dev/null || { echo "$RPC isn't answering" >&2; exit 1; }
if [[ ! -f "$LEE_WALLET_HOME_DIR/wallet_config.json" ]]; then
  mkdir -p "$LEE_WALLET_HOME_DIR"
  cp "scripts/$NETWORK/wallet_config.json" "$LEE_WALLET_HOME_DIR/"
fi
touch "$DEPLOY_ENV" "$ENV"
# From the earlier deploy, only the payer, the token bytecode and the oracle (its lending is old).
for var in PAYER TKA ORACLE; do
  val=$(sed -n "s/^$var=//p" "$DEPLOY_ENV" | tail -1)
  if [[ -n $val ]]; then eval "$var=\$val"; fi
done
# shellcheck source=/dev/null
source "$ENV"
[[ -n "${DEPLOY_BLOCK_ONBOARD:-}" ]] || record_cmd DEPLOY_BLOCK_ONBOARD last_block

if [[ -z "${PAYER:-}" ]]; then
  [[ $NETWORK == localnet ]] || { echo "no PAYER in $DEPLOY_ENV" >&2; exit 1; }
  # Genesis account CbgR6t… (scripts/localnet.md).
  "$W" account import public --private-key 7f273098f25b71e6c005a9519f2678da8d1c7f01f6a27778e2d9948abdf901fb < /dev/null > /dev/null
  echo "PAYER=Public/CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r" >> "$DEPLOY_ENV"
  PAYER=Public/CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r
fi
BALANCE=$(native_balance "$PAYER")
echo "payer $PAYER holds $BALANCE native" >&2

step "pre-flight: the sequencer's privacy circuit is the one this build proves with"
bos check > /dev/null

if [[ -z "${FAUCET:-}" ]]; then
  step "faucet: upload"
  record_cmd FAUCET bos faucet deploy --payer "$PAYER"
fi
MINT=$("$BOS_BIN" faucet accounts --faucet "$FAUCET" 2> /dev/null | sed -n 's/^mint_authority //p')
CUSTODY=$("$BOS_BIN" faucet accounts --faucet "$FAUCET" 2> /dev/null | sed -n 's/^custody //p')

# Token bytecode: the deployed token's segments if there is one (testnet: TKA's), else upload.
SEGMENTS=(${TKA:+--segments-of "$TKA"})
if [[ -z "${USDC:-}" ]]; then
  step "USDC: 6 decimals, minted by the faucet"
  record_cmd USDC bos token create --decimals 6 --mint-authority "$MINT" ${SEGMENTS[@]+"${SEGMENTS[@]}"} --payer "$PAYER"
fi
SEGMENTS=(--segments-of "$USDC")
if [[ -z "${WETH:-}" ]]; then
  step "WETH: 18 decimals, minted by the faucet"
  record_cmd WETH bos token create --decimals 18 --mint-authority "$MINT" ${SEGMENTS[@]+"${SEGMENTS[@]}"} --payer "$PAYER"
fi
if [[ -z "${WBTC:-}" ]]; then
  step "WBTC: 8 decimals, minted by the faucet"
  record_cmd WBTC bos token create --decimals 8 --mint-authority "$MINT" ${SEGMENTS[@]+"${SEGMENTS[@]}"} --payer "$PAYER"
fi
if [[ -z "${FAUCET_CONFIGURED:-}" ]]; then
  step "faucet: init (USDC $USDC_DROP, WETH $WETH_DROP, WBTC $WBTC_DROP, gas $GAS_DROP; cooldown ${COOLDOWN}s)"
  "$BOS_BIN" faucet info --faucet "$FAUCET" > /dev/null 2>&1 \
    || bos faucet init --faucet "$FAUCET" --drop "$USDC=$USDC_DROP" "$WETH=$WETH_DROP" "$WBTC=$WBTC_DROP" "native=$GAS_DROP" \
      --cooldown-secs "$COOLDOWN" --payer "$PAYER" > /dev/null
  record FAUCET_CONFIGURED 1
fi
if [[ -z "${CUSTODY_FUNDED:-}" ]]; then
  step "faucet: $CUSTODY_NATIVE native into the gas custody $CUSTODY"
  "$W" auth-transfer send --from "$PAYER" --to "$CUSTODY" --amount "$CUSTODY_NATIVE" < /dev/null > /dev/null
  record CUSTODY_FUNDED 1
fi

if [[ -z "${ORACLE:-}" ]]; then
  step "oracle: upload"
  record_cmd ORACLE bos oracle deploy --payer "$PAYER"
  echo "ORACLE=$ORACLE" >> "$DEPLOY_ENV"
fi
ETH_USD=$(spot ETH 2300)
BTC_USD=$(spot BTC 77000)
# The feeds' publisher: the payer, or a publish-only key in its own wallet (PUBLISHER and
# PUBLISHER_WALLET; testnet's is on the Contabo price pusher). It pays its own fees.
PUBLISHER=${PUBLISHER:-$PAYER}
PUBLISHER_WALLET=${PUBLISHER_WALLET:-$LEE_WALLET_HOME_DIR}
as_publisher() { LEE_WALLET_HOME_DIR=$PUBLISHER_WALLET bos "$@"; }
if [[ -z "${WETH_FEED:-}" ]]; then
  step "oracle: WETH/USDC feed at $ETH_USD"
  record_cmd WETH_FEED as_publisher oracle create-feed --oracle "$ORACLE" --publisher "$PUBLISHER" --name WETH/USDC --payer "$PUBLISHER"
fi
if [[ -z "${WBTC_FEED:-}" ]]; then
  step "oracle: WBTC/USDC feed at $BTC_USD"
  record_cmd WBTC_FEED as_publisher oracle create-feed --oracle "$ORACLE" --publisher "$PUBLISHER" --name WBTC/USDC --payer "$PUBLISHER"
fi
# Once a price pusher owns the publisher key (PUSHER=1), the script stays out of its nonce.
if [[ -z "${PUSHER:-}" ]]; then
  step "oracle: publish WETH/USDC $ETH_USD, WBTC/USDC $BTC_USD"
  as_publisher oracle publish --oracle "$ORACLE" --publisher "$PUBLISHER" --name WETH/USDC --price "$ETH_USD" \
    --loan-decimals 6 --collateral-decimals 18 --payer "$PUBLISHER" > /dev/null
  as_publisher oracle publish --oracle "$ORACLE" --publisher "$PUBLISHER" --name WBTC/USDC --price "$BTC_USD" \
    --loan-decimals 6 --collateral-decimals 8 --payer "$PUBLISHER" > /dev/null
fi

IRM=(--base 2% --slope1 10% --slope2 100% --kink 80%)
if [[ -z "${LENDING:-}" ]]; then
  step "lending: upload"
  record_cmd LENDING bos lending deploy --payer "$PAYER"
fi
if [[ -z "${LENDING_CONFIGURED:-}" ]]; then
  step "lending: init (payer is admin and fee recipient), LLTV 80%, the IRM, the token bytecode"
  CONFIG=$("$BOS_BIN" lending config --lending "$LENDING" 2> /dev/null || true)
  grep -q '^admin' <<< "$CONFIG" \
    || bos lending init --lending "$LENDING" --admin "$PAYER" --fee-recipient "$PAYER" --payer "$PAYER" > /dev/null
  CONFIG=$("$BOS_BIN" lending config --lending "$LENDING" 2> /dev/null)
  grep -q '^lltv' <<< "$CONFIG" || bos lending enable-lltv --lending "$LENDING" --admin "$PAYER" --lltv 80% --payer "$PAYER" > /dev/null
  grep -q '^irm' <<< "$CONFIG" || bos lending enable-irm --lending "$LENDING" --admin "$PAYER" "${IRM[@]}" --payer "$PAYER" > /dev/null
  grep -q '^asset bytecode' <<< "$CONFIG" || bos lending enable-asset --lending "$LENDING" --admin "$PAYER" --of "$USDC" --payer "$PAYER" > /dev/null
  record LENDING_CONFIGURED 1
fi
if [[ -z "${WETH_MARKET:-}" ]]; then
  step "market: borrow USDC against WETH (LLTV 80%, max price age ${MAX_PRICE_AGE} s)"
  record_cmd WETH_MARKET bos market create --lending "$LENDING" --loan "$USDC" --collateral "$WETH" --oracle "$ORACLE" \
    --feed "$WETH_FEED" --lltv 80% "${IRM[@]}" --max-price-age "$MAX_PRICE_AGE" --payer "$PAYER"
fi
if [[ -z "${WBTC_MARKET:-}" ]]; then
  step "market: borrow USDC against WBTC (LLTV 80%, max price age ${MAX_PRICE_AGE} s)"
  record_cmd WBTC_MARKET bos market create --lending "$LENDING" --loan "$USDC" --collateral "$WBTC" --oracle "$ORACLE" \
    --feed "$WBTC_FEED" --lltv 80% "${IRM[@]}" --max-price-age "$MAX_PRICE_AGE" --payer "$PAYER"
fi

# Liquidity: the payer claims USDC through fresh accounts (one drop per claimer per day), paid to
# itself, and supplies it to both markets.
if [[ -z "${SEEDED:-}" ]]; then
  step "liquidity: $SEED_CLAIMS USDC claims per market, supplied by the payer"
  for market in "$WETH_MARKET" "$WBTC_MARKET"; do
    for ((i = 0; i < SEED_CLAIMS; i++)); do
      claimer=$(new_account public Public)
      bos faucet claim --faucet "$FAUCET" --asset "$USDC" --claimer "$claimer" --to "$PAYER" --payer "$PAYER" > /dev/null
    done
    bos market supply --lending "$LENDING" --market "$market" --from "$PAYER" --assets $((SEED_CLAIMS * USDC_DROP)) > /dev/null
  done
  record SEEDED 1
fi

step "check: every program runs this build, behind an immutable header"
bos check --lending "$LENDING" --oracle "$ORACLE" --token "$USDC" --token "$WETH" --token "$WBTC" > /dev/null

SETTINGS=$STATE/basecamp-settings.json
cat > "$SETTINGS" <<JSON
{
  "sequencer": "$RPC",
  "lending": "$LENDING",
  "faucet": "$FAUCET",
  "scan_from": $DEPLOY_BLOCK_ONBOARD,
  "tokens": { "$USDC": "USDC", "$WETH": "WETH", "$WBTC": "WBTC" },
  "markets": ["$WETH_MARKET", "$WBTC_MARKET"]
}
JSON
SPENT=$((BALANCE - $(native_balance "$PAYER")))
step "onboarding deploy on $NETWORK done (spent $SPENT native); ids in $ENV; Basecamp settings in $SETTINGS"
