# Sourced by the Phase 5 scripts: picks the network and the state that goes with it.
#
#   NETWORK=localnet (default)  dev-mode localnet (scripts/localnet.md), fake proofs
#   NETWORK=testnet             https://testnet.lez.logos.co, real proofs (~10 min per private op)
#
# State lives in .$NETWORK/ (git-ignored): the wallet (phase5-wallet/), deployed ids (deploy.env),
# logs (logs/). Override the wallet with LEE_WALLET_HOME_DIR.

NETWORK=${NETWORK:-localnet}
case $NETWORK in
  localnet)
    RPC=http://127.0.0.1:3040
    export RISC0_DEV_MODE=1
    ;;
  testnet)
    RPC=https://testnet.lez.logos.co
    unset RISC0_DEV_MODE
    ;;
  *) echo "NETWORK must be localnet or testnet, got $NETWORK" >&2; exit 1 ;;
esac
STATE=$PWD/.$NETWORK
export LEE_WALLET_HOME_DIR=${LEE_WALLET_HOME_DIR:-$STATE/phase5-wallet}
export SUPPRESS_VERBOSE_PRINTS=1
DEPLOY_ENV=$STATE/deploy.env
LOGS=$STATE/logs
MEASURE=$LOGS/measure.log
mkdir -p "$LOGS"

W=${WALLET:-$HOME/Work/logos-execution-zone/target/release/wallet}
BOS_BIN=${BOS:-$PWD/target/release/bos}
KEEPER_BIN=${KEEPER_BIN:-$PWD/target/release/bos-keeper}

step() { printf '\n== %s\n' "$*" >&2; }
expect() { [[ "$1" == "$2" ]] || { echo "FAIL: $3: expected $2, got $1" >&2; exit 1; }; }
more() { [[ "$1" -gt "$2" ]] || { echo "FAIL: $3: expected more than $2, got $1" >&2; exit 1; }; }
# `bos`, keeping its last stdout line (the result) and appending its [measure] lines to $MEASURE.
bos() {
  local err rc=0
  err=$(mktemp)
  "$BOS_BIN" "$@" 2> "$err" | tail -1 || rc=$?
  grep '^\[measure\]' "$err" | sed "s/^/$(date +%s) /" >> "$MEASURE" || true
  grep -v '^\[measure\]' "$err" >&2 || true
  rm -f "$err"
  return $rc
}
new_account() { "$W" account new "$1" < /dev/null | grep -o "$2/[1-9A-HJ-NP-Za-km-z]*" | head -1; }
rpc() {
  curl -sf -X POST "$RPC" -H 'content-type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":${2:-[]}}"
}
last_block() { rpc getLastBlockId | grep -o '"result":[0-9]*' | cut -d: -f2; }
native_balance() { rpc getAccountBalance "[\"${1#Public/}\"]" | grep -o '"result":[0-9]*' | cut -d: -f2; }
# Records a key in $DEPLOY_ENV (and the current shell).
record() { echo "$1=$2" >> "$DEPLOY_ENV"; eval "$1=\$2"; }
# Runs a command and records its output under $1; stops the script if it fails or prints nothing.
record_cmd() {
  local var=$1 out
  shift
  out=$("$@") || { echo "FAIL: $var: $*" >&2; exit 1; }
  [[ -n "$out" ]] || { echo "FAIL: $var: no output from $*" >&2; exit 1; }
  record "$var" "$out"
}
