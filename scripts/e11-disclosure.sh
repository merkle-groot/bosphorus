#!/usr/bin/env bash
# E11 on the dev-mode localnet: solvency proofs ("my position has health >= 1.5 and >= 50,000 TKA
# of collateral", without saying which position).
#   A fresh lending instance and TKB/TKA market (reuses phase5-deploy's tokens and feed). Private
#   owners A (two positions), B and C and a public borrower open positions at various health. An
#   auditor makes a request; A proves; the auditor verifies. Then the attacks, each of which must
#   fail: a position that doesn't meet the claims, another owner's proof passed off as A's, a proof
#   replayed for another request, a narrowed cover set, a moved price, claims only two positions
#   meet, a stale snapshot, a dev-mode receipt checked without dev mode, a tampered receipt.
#   Last, a real (GPU) proof, verified without dev mode. Logs in .localnet/logs/e11-*.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/net.sh
# shellcheck source=/dev/null
source "$DEPLOY_ENV"
[[ -n "${TKB:-}" ]] || { echo "deploy first: NETWORK=$NETWORK scripts/phase5-deploy.sh" >&2; exit 1; }
D=$STATE/e11
rm -rf "$D" && mkdir -p "$D"
REAL=${REAL:-1}
START=$(last_block)

# Must fail, and its stderr must mention $1.
fails() {
  local want=$1 err
  shift
  err=$(mktemp)
  if "$BOS_BIN" "$@" > /dev/null 2> "$err"; then echo "FAIL: should have failed: $*" >&2; exit 1; fi
  grep -q -- "$want" "$err" || { echo "FAIL: wrong error for $* (wanted '$want'):" >&2; cat "$err" >&2; exit 1; }
  echo "  refused as expected: $(grep -o -- "$want.*" "$err" | head -1 | cut -c1-110)" >&2
  rm -f "$err"
}
# Python helpers on the borsh files (layouts in disclosure/src/lib.rs).
py() { python3 - "$@"; }

IRM=(--base 2% --slope1 10% --slope2 100% --kink 80%)
step "lending (this build) and a TKB/TKA market; price 2.5, LLTV 80%"
bos oracle publish --oracle "$ORACLE" --publisher "$PAYER" --name TKA/TKB --price 2.5 --loan-decimals 6 --collateral-decimals 6 > /dev/null
LENDING=$(bos lending deploy --payer "$PAYER")
bos lending init --lending "$LENDING" --admin "$PAYER" --fee-recipient "$PAYER" --payer "$PAYER" > /dev/null
bos lending enable-lltv --lending "$LENDING" --admin "$PAYER" --lltv 80% > /dev/null
bos lending enable-irm --lending "$LENDING" --admin "$PAYER" "${IRM[@]}" > /dev/null
bos lending enable-asset --lending "$LENDING" --admin "$PAYER" --of "$TKA" > /dev/null
MARKET=$(bos market create --lending "$LENDING" --loan "$TKB" --collateral "$TKA" --oracle "$ORACLE" \
  --feed "$FEED" --lltv 80% "${IRM[@]}" --max-price-age 3600 --payer "$PAYER")
ON=(--lending "$LENDING" --market "$MARKET" --payer "$PAYER")
echo "LENDING=$LENDING MARKET=$MARKET" | tee "$D/env" >&2

step "a public lender supplies 1,000,000 TKB"
LENDER=$(new_account public Public)
bos token mint --token "$TKB" --authority "$PAYER" --to "$LENDER" --amount 1000000000000 > /dev/null
bos market supply "${ON[@]}" --from "$LENDER" --assets 1000000000000 > /dev/null

# A private owner with `n` x 100,000 TKA shielded.
private_owner() {
  local owner stage
  owner=$(new_account private Private)
  stage=$(new_account public Public)
  bos token mint --token "$TKA" --authority "$PAYER" --to "$stage" --amount $(($1 * 100000000000)) > /dev/null
  bos token shield --token "$TKA" --from "$stage" --to "$owner" --amount $(($1 * 100000000000)) > /dev/null
  echo "$owner"
}
# position: owner index borrow (100,000 TKA collateral; max borrow 200,000 TKB)
open() {
  bos market supply-collateral "${ON[@]}" --from "$1" --index "$2" --assets 100000000000 > /dev/null
  bos market borrow "${ON[@]}" --owner "$1" --index "$2" --assets "$3" > /dev/null
}
step "positions: A#0 HF 2.0, A#1 HF 1.11, B HF 1.67, C HF 1.6 (private), PUB HF 1.82 (public)"
A=$(private_owner 2); B=$(private_owner 1); C=$(private_owner 1)
open "$A" 0 100000000000
open "$A" 1 180000000000
open "$B" 0 120000000000
open "$C" 0 125000000000
PUB=$(new_account public Public)
bos token mint --token "$TKA" --authority "$PAYER" --to "$PUB" --amount 100000000000 > /dev/null
open "$PUB" 0 110000000000
echo "A=$A B=$B C=$C PUB=$PUB" | tee -a "$D/env" >&2

step "disclosure ids (A's is stable, B's differs)"
ID_A=$(bos disclose id --holder "$A"); ID_B=$(bos disclose id --holder "$B")
expect "$(bos disclose id --holder "$A")" "$ID_A" "A's id is stable"
[[ "$ID_A" != "$ID_B" ]] || { echo "FAIL: A and B share an id" >&2; exit 1; }
echo "A: $ID_A" >&2

CLAIMS=(--min-health 1.5 --min-collateral 50000000000)
step "auditor: request health >= 1.5 and collateral >= 50,000 TKA"
"$BOS_BIN" disclose request --from-block "$START" --lending "$LENDING" --market "$MARKET" --label "Acme credit desk, Q4 review" "${CLAIMS[@]}" --out "$D/r1.bin" >&2
"$BOS_BIN" disclose show --from-block "$START" --request "$D/r1.bin" | tail -1 | grep -q "matches the chain: 4 of 6" || { echo "FAIL: expected 4 of 6 qualifying" >&2; exit 1; }

step "A proves (dev mode) and the auditor verifies"
"$BOS_BIN" disclose prove --from-block "$START" --request "$D/r1.bin" --holder "$A" --out "$D/a1.bin" >&2
"$BOS_BIN" disclose verify --request "$D/r1.bin" --attestation "$D/a1.bin" --expect-id "$ID_A" | tee "$D/verify-a1.txt" >&2
grep -q "^VERIFIED" "$D/verify-a1.txt"

step "attack: A proves with the position that fails the claims (A#1, HF 1.11)"
fails "no position in this market that meets the claims" disclose prove --from-block "$START" --request "$D/r1.bin" --holder "$A" --index 1 --out "$D/x.bin"
step "attack: B's valid proof passed off as A's"
"$BOS_BIN" disclose prove --from-block "$START" --request "$D/r1.bin" --holder "$B" --out "$D/b1.bin" >&2
fails "not $ID_A" disclose verify --request "$D/r1.bin" --attestation "$D/b1.bin" --expect-id "$ID_A"
step "attack: an owner with no position in the market"
NOBODY=$(new_account private Private)
fails "no position in this market" disclose prove --from-block "$START" --request "$D/r1.bin" --holder "$NOBODY" --out "$D/x.bin"

step "attack: A's proof replayed for a later request (new nonce)"
"$BOS_BIN" disclose request --from-block "$START" --lending "$LENDING" --market "$MARKET" "${CLAIMS[@]}" --out "$D/r2.bin" > /dev/null
fails "answers a different request" disclose verify --request "$D/r2.bin" --attestation "$D/a1.bin"
# Splice: request 2 with A's receipt for request 1.
py "$D/r1.bin" "$D/r2.bin" "$D/a1.bin" "$D/spliced.bin" <<'EOF'
import sys
r1, r2, a1 = (open(p, 'rb').read() for p in sys.argv[1:4])
assert a1.startswith(r1)
open(sys.argv[4], 'wb').write(r2 + a1[len(r1):])
EOF
fails "proof is for another request" disclose verify --request "$D/r2.bin" --attestation "$D/spliced.bin"

# Request layout: label (u32 + bytes), nonce, lending, market (32 each), price (32), rb, rs, lltv
# (16 each), at_ms (8), 4 x Option<u128>, positions (u32 + n x 112).
REQ_PY='
import struct, sys
def parse(b):
    (l,) = struct.unpack_from("<I", b, 0)
    o = 4 + l + 96
    price_at = o
    o += 32 + 48 + 8
    for _ in range(4):
        o += 1 + (16 if b[o] else 0)
    (n,) = struct.unpack_from("<I", b, o)
    assert len(b) == o + 4 + n * 112, "layout"
    return price_at, o, n
'
step "attack: the auditor narrows the cover set to one position"
py "$D/r1.bin" "$D/narrow.bin" <<EOF
$REQ_PY
b = open(sys.argv[1], 'rb').read()
_, o, n = parse(b)
open(sys.argv[2], 'wb').write(b[:o] + struct.pack("<I", 1) + b[o + 4:o + 4 + 112])
EOF
fails "leaves out" disclose prove --from-block "$START" --request "$D/narrow.bin" --holder "$A" --out "$D/x.bin"
step "attack: the auditor lists a position that doesn't exist"
py "$D/r1.bin" "$D/fake-pos.bin" <<EOF
$REQ_PY
b = open(sys.argv[1], 'rb').read()
_, o, n = parse(b)
entry = bytes([0xff] * 32) + b[o + 4 + 32:o + 4 + 112]
open(sys.argv[2], 'wb').write(b[:o] + struct.pack("<I", n + 1) + b[o + 4:] + entry)
EOF
fails "which isn't a position of this market" disclose prove --from-block "$START" --request "$D/fake-pos.bin" --holder "$A" --out "$D/x.bin"
step "attack: the auditor moves the price 10% up"
py "$D/r1.bin" "$D/price.bin" <<EOF
$REQ_PY
b = bytearray(open(sys.argv[1], 'rb').read())
p, _, _ = parse(b)
price = int.from_bytes(b[p:p + 32], 'little') * 11 // 10
b[p:p + 32] = price.to_bytes(32, 'little')
open(sys.argv[2], 'wb').write(b)
EOF
fails "from the feed" disclose prove --from-block "$START" --request "$D/price.bin" --holder "$A" --out "$D/x.bin"
if [[ $REAL == 1 ]]; then
  step "real proof (no dev mode): fresh request, A proves, verified without dev mode"
  "$BOS_BIN" disclose request --from-block "$START" --lending "$LENDING" --market "$MARKET" --label "real" "${CLAIMS[@]}" --out "$D/r3.bin" > /dev/null
  env -u RISC0_DEV_MODE "$BOS_BIN" disclose prove --from-block "$START" --request "$D/r3.bin" --holder "$A" --out "$D/a3.bin" | tee "$D/real-prove.txt" >&2
  env -u RISC0_DEV_MODE "$BOS_BIN" disclose verify --request "$D/r3.bin" --attestation "$D/a3.bin" --expect-id "$ID_A" | tee "$D/verify-a3.txt" >&2
  grep -qx "VERIFIED" "$D/verify-a3.txt" || { echo "FAIL: real proof not verified" >&2; exit 1; }
  step "attack: tampered real receipts (a seal byte, the last bytes)"
  for where in mid end; do
    py "$D/r3.bin" "$D/a3.bin" "$D/t-$where.bin" "$where" <<'PYEOF'
import sys
r, a = open(sys.argv[1], 'rb').read(), bytearray(open(sys.argv[2], 'rb').read())
i = len(r) + 4 + (len(a) - len(r) - 4) // 2 if sys.argv[4] == 'mid' else len(a) - 40
a[i] ^= 1
open(sys.argv[3], 'wb').write(a)
PYEOF
    if env -u RISC0_DEV_MODE "$BOS_BIN" disclose verify --request "$D/r3.bin" --attestation "$D/t-$where.bin" > /dev/null 2>&1; then
      echo "FAIL: tampered receipt ($where) verified" >&2; exit 1
    fi
    echo "  tampered ($where): refused" >&2
  done
fi

step "attack: a dev-mode receipt checked without dev mode"
if env -u RISC0_DEV_MODE "$BOS_BIN" disclose verify --request "$D/r1.bin" --attestation "$D/a1.bin" > /dev/null 2> "$D/devcheck.err"; then
  echo "FAIL: a dev-mode receipt verified without dev mode" >&2; exit 1
fi
echo "  refused as expected: $(head -c 160 "$D/devcheck.err" | tr '\n' ' ')" >&2

step "attack: claims only two positions meet (health >= 1.7: A#0, PUB)"
"$BOS_BIN" disclose request --from-block "$START" --lending "$LENDING" --market "$MARKET" --min-health 1.7 --min-collateral 50000000000 --out "$D/r-narrow.bin" > /dev/null
fails "narrow you down to 2 positions (2 of 6 meet these claims)" disclose prove --from-block "$START" --request "$D/r-narrow.bin" --holder "$A" --out "$D/x.bin"

step "stale snapshot: C repays some debt, then A proves against the old request"
bos market repay "${ON[@]}" --from "$C" --owner "$C" --shares 1000000 > /dev/null
fails "changed since the request" disclose prove --from-block "$START" --request "$D/r1.bin" --holder "$A" --out "$D/x.bin"

step "attack by repetition: B and C drop below 1.5, D and E open at 2.0; a new request's 4 qualifying share only A#0 and PUB with A's earlier proofs"
bos market borrow "${ON[@]}" --owner "$B" --assets 20000000000 > /dev/null
bos market borrow "${ON[@]}" --owner "$C" --assets 20000000000 > /dev/null
for who in D E; do
  acct=$(new_account public Public)
  bos token mint --token "$TKA" --authority "$PAYER" --to "$acct" --amount 100000000000 > /dev/null
  open "$acct" 0 100000000000
done
"$BOS_BIN" disclose request --from-block "$START" --lending "$LENDING" --market "$MARKET" "${CLAIMS[@]}" --out "$D/r4.bin" > /dev/null
"$BOS_BIN" disclose show --from-block "$START" --request "$D/r4.bin" | tail -1 | grep -q "matches the chain: 4 of 8" || { echo "FAIL: expected 4 of 8 qualifying" >&2; "$BOS_BIN" disclose show --from-block "$START" --request "$D/r4.bin" >&2; exit 1; }
fails "narrow you down to 2 positions (4 of 8 meet these claims, fewer counting your earlier proofs" disclose prove --from-block "$START" --request "$D/r4.bin" --holder "$A" --out "$D/x.bin"

step "--force: the owner may prove anyway (fresh request, same claims)"
"$BOS_BIN" disclose request --from-block "$START" --lending "$LENDING" --market "$MARKET" "${CLAIMS[@]}" --out "$D/r5.bin" > /dev/null
"$BOS_BIN" disclose prove --from-block "$START" --request "$D/r5.bin" --holder "$A" --force --out "$D/a-forced.bin" > /dev/null
"$BOS_BIN" disclose verify --request "$D/r5.bin" --attestation "$D/a-forced.bin" --expect-id "$ID_A" | grep -q "^VERIFIED"
echo "  proved and verified" >&2

echo "E11 disclosure: all checks passed" >&2
