#!/bin/bash
# Builds the real-borrow test and runs it with the stage timeline into $LOG (extra env as args):
#   LOG=/tmp/b.log spikes/vendor/risc0-metal/tools/run_borrow.sh R0_LANES=1
cd /Users/0x1379/Work/money-market
BIN=$(R0_SEGMENT_PO2=19 R0_METAL_BUDGET=400 R0_METAL_STORE=16 cargo test -p bosphorus-methods --release --features prove --test private --no-run 2>&1 | grep -o 'target/release/deps/private-[0-9a-f]*' | tail -1)
[ -z "$BIN" ] && { echo "build failed"; exit 1; }
echo "binary: $BIN"
cd programs/methods
env -u RISC0_DEV_MODE R0_METAL_FULL=1 RISC0_KECCAK_PO2=14 R0_TL=1 "$@" ../../$BIN --ignored --nocapture real_proof_borrow > "$LOG" 2>&1
grep -E "\| borrow|test result|panicked" "$LOG" | head -5
