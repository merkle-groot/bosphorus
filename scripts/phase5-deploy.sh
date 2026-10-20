#!/usr/bin/env bash
# Phase 5.1: deploy Bosphorus to a network, idempotently.
#   TKA (collateral) and TKB (loan, on TKA's bytecode), the oracle with a TKA/TKB feed, lending
#   (LLTV 80%, a 2–112% APR kink IRM, the token bytecode enabled) and one TKB/TKA market.
#
#   NETWORK=localnet scripts/phase5-deploy.sh    # rehearsal on the dev-mode localnet
#   NETWORK=testnet  scripts/phase5-deploy.sh    # https://testnet.lez.logos.co
#
# Ids go to .$NETWORK/deploy.env as each step finishes; a rerun skips what's recorded, so it
# resumes after a failure or a stall. Delete deploy.env to deploy afresh. On testnet the first run
# creates the wallet and a payer account, then stops until that account holds native tokens.
# Needs release builds of `bos` and `bos-keeper` and LEZ's `wallet`.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/net.sh

# Native the payer needs before deploying: one tx reserves gas_limit (2M) × 8 on top of ~8 per
# byte uploaded, and the deploy uploads ~1 MB of bytecode. Measured on the localnet rehearsal.
MIN_NATIVE=${MIN_NATIVE:-100000000}

step "$NETWORK: wallet $LEE_WALLET_HOME_DIR"
rpc getLastBlockId > /dev/null || { echo "$RPC isn't answering" >&2; exit 1; }
if [[ ! -f "$LEE_WALLET_HOME_DIR/wallet_config.json" ]]; then
  mkdir -p "$LEE_WALLET_HOME_DIR"
  cp "scripts/$NETWORK/wallet_config.json" "$LEE_WALLET_HOME_DIR/"
fi
touch "$DEPLOY_ENV"
# shellcheck source=/dev/null
source "$DEPLOY_ENV"
[[ -n "${DEPLOY_BLOCK:-}" ]] || record_cmd DEPLOY_BLOCK last_block

if [[ -z "${PAYER:-}" ]]; then
  if [[ $NETWORK == localnet ]]; then
    # Genesis account CbgR6t… (scripts/localnet.md).
    "$W" account import public --private-key 7f273098f25b71e6c005a9519f2678da8d1c7f01f6a27778e2d9948abdf901fb < /dev/null > /dev/null
    record PAYER Public/CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r
  else
    record_cmd PAYER new_account public Public
  fi
fi
BALANCE=$(native_balance "$PAYER")
echo "payer $PAYER holds $BALANCE native" >&2
if [[ "$BALANCE" -lt "$MIN_NATIVE" ]]; then
  cat >&2 <<MSG
The payer needs at least $MIN_NATIVE native. Fund it, then rerun, e.g. from the bridge account:
  LEE_WALLET_HOME_DIR=\$HOME/.lee/wallet $W auth-transfer send \\
    --from Public/6rCD2yrwyEchmoRvLpJBcDREjEJveJccVoPe7nv2vMQn --to $PAYER --amount <native>
MSG
  exit 2
fi

step "pre-flight: the sequencer's privacy circuit is the one this build proves with"
bos check > /dev/null

IRM=(--base 2% --slope1 10% --slope2 100% --kink 80%)
if [[ -z "${TKA:-}" ]]; then
  step "TKA: upload the token bytecode, init (6 decimals, payer mints)"
  record_cmd TKA bos token create --decimals 6 --mint-authority "$PAYER" --payer "$PAYER"
fi
if [[ -z "${TKB:-}" ]]; then
  step "TKB: a second header on TKA's segments, init"
  record_cmd TKB bos token create --decimals 6 --mint-authority "$PAYER" --segments-of "$TKA" --payer "$PAYER"
fi
if [[ -z "${ORACLE:-}" ]]; then
  step "oracle: upload"
  record_cmd ORACLE bos oracle deploy --payer "$PAYER"
fi
if [[ -z "${FEED:-}" ]]; then
  step "oracle: TKA/TKB feed, published by the payer; price 2.5"
  record_cmd FEED bos oracle create-feed --oracle "$ORACLE" --publisher "$PAYER" --name TKA/TKB --payer "$PAYER"
  bos oracle publish --oracle "$ORACLE" --publisher "$PAYER" --name TKA/TKB --price 2.5 \
    --loan-decimals 6 --collateral-decimals 6 --payer "$PAYER" > /dev/null
fi
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
  grep -q '^asset bytecode' <<< "$CONFIG" || bos lending enable-asset --lending "$LENDING" --admin "$PAYER" --of "$TKA" --payer "$PAYER" > /dev/null
  record LENDING_CONFIGURED 1
fi
if [[ -z "${MARKET:-}" ]]; then
  step "market: TKB loan, TKA collateral, TKA/TKB feed, max price age 1 h"
  record_cmd MARKET bos market create --lending "$LENDING" --loan "$TKB" --collateral "$TKA" --oracle "$ORACLE" \
    --feed "$FEED" --lltv 80% "${IRM[@]}" --max-price-age 3600 --payer "$PAYER"
fi

step "check: every program runs this build, behind an immutable header"
bos check --lending "$LENDING" --oracle "$ORACLE" --token "$TKA" --token "$TKB" > /dev/null
"$BOS_BIN" programs --write-dir "$STATE/elfs" > "$STATE/programs.txt"
SPENT=$((BALANCE - $(native_balance "$PAYER")))

cat > "$STATE/DEPLOYMENTS.md" <<MD
## $NETWORK ($(date -u +%Y-%m-%d), commit $(git rev-parse --short HEAD)$(git diff --quiet || echo "+dirty"))

| What | Id |
|---|---|
| TKA token (collateral, 6 decimals) | \`$TKA\` |
| TKB token (loan, 6 decimals) | \`$TKB\` |
| oracle | \`$ORACLE\` |
| TKA/TKB feed (publisher \`$PAYER\`) | \`$FEED\` |
| lending | \`$LENDING\` |
| TKB/TKA market (LLTV 80%, 2–112% APR kink at 80%, max price age 1 h) | \`$MARKET\` |
| admin, fee recipient, mint authority | \`$PAYER\` |

Image ids (\`bos programs\`; ELFs in \`.$NETWORK/elfs/\`):

\`\`\`
$(cat "$STATE/programs.txt")
\`\`\`
MD
step "deployed on $NETWORK (this run spent $SPENT native); ids in $DEPLOY_ENV, table in $STATE/DEPLOYMENTS.md"
