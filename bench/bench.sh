#!/bin/zsh
# bench.sh — G1/G6-compliant benchmark harness.
# Usage: bench.sh <label> <n_runs> <command...>
# Prints: label | mean_ms | stddev_ms | peak_rss_mb | user_s | sys_s
# Method: 3 warmups; hyperfine for wall time (n runs); /usr/bin/time -l (5 runs, max) for RSS/CPU.

set -u
LABEL="$1"; N="$2"; shift 2
CMD=("$@")
PORTDIR=/Users/vipulkumar/Documents/zai/zcode-port
HF="$PORTDIR/.cargo/bin/hyperfine"
TMP="$PORTDIR/bench/.tmp"; mkdir -p "$TMP"

# warmup
for i in 1 2 3; do "${CMD[@]}" >/dev/null 2>&1; done

# wall time via hyperfine (JSON)
HFJSON="$TMP/hf.json"
"$HF" --runs "$N" --export-json "$HFJSON" --style basic \
  --command-name "$LABEL" "${CMD[*]}" >/dev/null 2>&1
MEAN=$(node -e "const d=require('$HFJSON').results[0];console.log(d.mean*1000)")
STD=$(node -e "const d=require('$HFJSON').results[0];console.log(d.stddev*1000)")

# peak RSS + CPU via /usr/bin/time -l, 5 runs, take max RSS / mean CPU
RSS=0; U=0; S=0
for i in 1 2 3 4 5; do
  OUT=$(/usr/bin/time -l "${CMD[@]}" 2>&1 >/dev/null)
  R=$(echo "$OUT" | awk '/maximum resident set size/{print $1}')
  UU=$(echo "$OUT" | awk '/^ *[0-9.]+ user/{for(i=1;i<=NF;i++) if($i=="user") print $(i-1)}')
  SS=$(echo "$OUT" | awk '/^ *[0-9.]+ sys/{for(i=1;i<=NF;i++) if($i=="sys") print $(i-1)}')
  [ -n "$R" ] && [ "$R" -gt "$RSS" ] && RSS=$R
  U=$(node -e "console.log((${UU:-0})+$U)")
  S=$(node -e "console.log((${SS:-0})+$S)")
done
UM=$(node -e "console.log(($U/5).toFixed(3))")
SM=$(node -e "console.log(($S/5).toFixed(3))")
RSSMB=$(node -e "console.log(($RSS/1048576).toFixed(1))")
printf "%s | %.1f | %.1f | %s | %s | %s\n" "$LABEL" "$MEAN" "$STD" "$RSSMB" "$UM" "$SM"
