#!/usr/bin/env bash
# The C ABI (`ffi/`, `bosphorus_ffi.h`) from C, read-only (no tx is sent): builds libbosphorus_ffi,
# compiles scripts/ffi-smoke.c against the header and the dylib, and runs it on a copy of the
# network's wallet: open, a second open refused (`wallet_in_use`), `info`, an unknown method, bad
# input, events, close, open again.
#   NETWORK=localnet (default) needs the localnet up; NETWORK=testnet reads https://testnet.lez.logos.co.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/net.sh
# shellcheck source=/dev/null
source "$DEPLOY_ENV"
[[ -n "${LENDING:-}" ]] || { echo "no LENDING in $DEPLOY_ENV: deploy first" >&2; exit 1; }

step "build libbosphorus_ffi"
cargo build --release -p bosphorus-ffi
OUT=$(mktemp -d)
trap 'rm -rf "$OUT"' EXIT
cc -std=c11 -Wall -Wextra -Werror -I ffi scripts/ffi-smoke.c -L target/release -lbosphorus_ffi \
  -Wl,-rpath,"$PWD/target/release" -o "$OUT/ffi-smoke"

# A copy (plain JSON keys, so 0600), so nothing here can write the network's wallet; passed by
# path, not LEE_WALLET_HOME_DIR.
mkdir "$OUT/wallet"
cp "$LEE_WALLET_HOME_DIR/wallet_config.json" "$LEE_WALLET_HOME_DIR/storage.json" "$OUT/wallet/"
chmod 600 "$OUT"/wallet/*
step "run on $NETWORK"
env -u LEE_WALLET_HOME_DIR "$OUT/ffi-smoke" \
  "{\"wallet_home\":\"$OUT/wallet\",\"lending\":\"$LENDING\",\"data_dir\":\"$OUT/data\"}"
