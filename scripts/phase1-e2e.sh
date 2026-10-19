#!/usr/bin/env bash
# Phase 1 exit gate: create TKA and TKB (TKB on TKA's uploaded bytecode), mint both, shield TKA into
# a private account, publish a TKA/TKB price.
#
# Needs a running dev-mode localnet (scripts/localnet.md) and release builds of `bos`
# (`cargo build --release -p bosphorus-cli`) and LEZ's `wallet`. Uses a fresh wallet each run.
set -euo pipefail
cd "$(dirname "$0")/.."

W=${WALLET:-$HOME/Work/logos-execution-zone/target/release/wallet}
BOS=${BOS:-target/release/bos}
export RISC0_DEV_MODE=1 SUPPRESS_VERBOSE_PRINTS=1
export LEE_WALLET_HOME_DIR=$PWD/.localnet/e2e-wallet
GENESIS_KEY=7f273098f25b71e6c005a9519f2678da8d1c7f01f6a27778e2d9948abdf901fb
PAYER=Public/CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r

step() { printf '\n== %s\n' "$*" >&2; }
expect() { [[ "$1" == "$2" ]] || { echo "FAIL: $3: expected $2, got $1" >&2; exit 1; }; }
# $1: public|private, $2: Public|Private (macOS bash 3.2 has no ${1^}).
# The wallet library prints progress to stdout; `bos` prints its result last.
bos() { "$BOS" "$@" | tail -1; }
new_account() { "$W" account new "$1" < /dev/null | grep -o "$2/[1-9A-HJ-NP-Za-km-z]*" | head -1; }

curl -sf -X POST http://127.0.0.1:3040 -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getLastBlockId","params":[]}' > /dev/null \
  || { echo "localnet isn't running (see scripts/localnet.md)" >&2; exit 1; }

step "fresh wallet"
rm -rf "$LEE_WALLET_HOME_DIR" && mkdir -p "$LEE_WALLET_HOME_DIR"
cp scripts/localnet/wallet_config.json "$LEE_WALLET_HOME_DIR/"
"$W" account import public --private-key $GENESIS_KEY < /dev/null > /dev/null
ALICE=$(new_account public Public)
SHIELDED=$(new_account private Private)
echo "alice $ALICE, private $SHIELDED" >&2

step "TKA: upload + init"
TKA=$(bos token create --decimals 6 --mint-authority $PAYER --payer $PAYER)
step "TKB: new header on TKA's segments + init"
TKB=$(bos token create --decimals 6 --mint-authority $PAYER --segments-of "$TKA" --payer $PAYER)
echo "TKA $TKA, TKB $TKB" >&2

step "mint"
expect "$(bos token mint --token "$TKA" --authority $PAYER --to "$ALICE" --amount 1000000)" 1000000 "TKA minted"
expect "$(bos token mint --token "$TKB" --authority $PAYER --to "$ALICE" --amount 5000000)" 5000000 "TKB minted"

step "shield 400000 TKA into $SHIELDED (private tx, dev-mode proof)"
expect "$(bos token shield --token "$TKA" --from "$ALICE" --to "$SHIELDED" --amount 400000)" 400000 "private TKA"
expect "$(bos token balance --token "$TKA" --account "$ALICE")" 600000 "public TKA left"

step "oracle: deploy, create feed, publish TKA/TKB = 2.5"
ORACLE=$(bos oracle deploy --payer $PAYER)
FEED=$(bos oracle create-feed --oracle "$ORACLE" --publisher $PAYER --name TKA/TKB)
bos oracle publish --oracle "$ORACLE" --publisher $PAYER --name TKA/TKB --price 2.5 \
  --loan-decimals 6 --collateral-decimals 6

cat > .localnet/phase1.env <<EOF
TKA=$TKA
TKB=$TKB
ORACLE=$ORACLE
FEED=$FEED
PUBLISHER=$PAYER
SHIELDED=$SHIELDED
EOF
step "Phase 1 e2e: PASS (ids in .localnet/phase1.env)"
