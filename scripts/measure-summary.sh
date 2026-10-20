#!/usr/bin/env bash
# Phase 5.3: summarises the [measure] lines the Phase 5 scripts collect (.$NETWORK/logs/measure.log)
# into the report's tables: per op, inclusion delay (block timestamp − submission), proving time
# and fee; deploy costs; keeper latency; and the block summaries the scenarios recorded.
#
#   NETWORK=testnet scripts/measure-summary.sh [measure.log]
set -euo pipefail
cd "$(dirname "$0")/.."
NETWORK=${NETWORK:-localnet}
LOG=${1:-.$NETWORK/logs/measure.log}
[[ -f "$LOG" ]] || { echo "no $LOG" >&2; exit 1; }

echo "### Ops ($LOG)"
echo
echo "| op | kind | n | inclusion p50 (s) | inclusion max (s) | proving p50 (s) | fee p50 (native) |"
echo "|---|---|---:|---:|---:|---:|---:|"
grep ' \[measure\] ' "$LOG" | grep -v 'op=deploy \|op=keeper.latency ' | awk '
  function kv(k,   i, a) { for (i = 1; i <= NF; i++) { split($i, a, "="); if (a[1] == k) return a[2] } return "?" }
  { key = kv("op") " | " kv("kind"); n[key]++
    v = kv("inclusion_ms"); if (v != "?") inc[key] = inc[key] " " v
    v = kv("proving_ms"); if (v != "?") prov[key] = prov[key] " " v
    v = kv("fee"); if (v != "?") fee[key] = fee[key] " " v }
  function sorted(s, out,   m, i, j, t) { m = split(s, out, " "); for (i = 2; i <= m; i++) for (j = i; j > 1 && out[j-1] + 0 > out[j] + 0; j--) { t = out[j]; out[j] = out[j-1]; out[j-1] = t } return m }
  function p50(s,   a, m) { m = sorted(s, a); return m ? a[int((m + 1) / 2)] : "" }
  function max(s,   a, m) { m = sorted(s, a); return m ? a[m] : "" }
  function sec(x) { return x == "" ? "–" : sprintf("%.1f", x / 1000) }
  END { for (k in n) printf "| %s | %d | %s | %s | %s | %s |\n", k, n[k], sec(p50(inc[k])), sec(max(inc[k])), sec(p50(prov[k])), (p50(fee[k]) == "" ? "–" : p50(fee[k])) }
' | sort
echo
echo "### Deploys"
echo
grep ' \[measure\] op=deploy ' "$LOG" | sed 's/.*\[measure\] //' || echo "(none)"
echo
echo "### Keeper latency (price block → liquidation block)"
echo
grep ' \[measure\] op=keeper.latency ' "$LOG" | sed 's/.*\[measure\] //' || echo "(none)"
echo
echo "### Blocks"
echo
grep '^blocks ' "$LOG" || echo "(none)"
