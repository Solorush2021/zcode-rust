#!/bin/zsh
# final.sh — G6 head-to-head: Node bundle vs Rust binary, same machine, same n.
# Writes bench/FINAL.md with the comparison table.

set -u
PORTDIR=/Users/vipulkumar/Documents/zai/zcode-port
CLI="$PORTDIR/ZCode/apps/zcode-cli/packages/cli/dist/zcode.cjs"
Z="$PORTDIR/zcode-rs/target/release/zcode"
HF="$PORTDIR/.cargo/bin/hyperfine"
OUT="$PORTDIR/bench/FINAL.md"
export NO_COLOR=1 FORCE_COLOR=0

row() {
  local label="$1"; shift
  local n="$1"; shift
  local ts_cmd="$1"; shift
  local rs_cmd="$1"; shift
  "$HF" --warmup 3 --runs "$n" --style basic --export-json /tmp/hf-tmp.json "$ts_cmd" "$rs_cmd" >/dev/null 2>&1
  node -e "
const r=require('/tmp/hf-tmp.json').results;
const [t,s]=r;
const ts=(t.mean*1000).toFixed(1), rs=(s.mean*1000).toFixed(1);
const speed=(t.mean/s.mean).toFixed(0);
console.log('| $label | '+ts+' | '+rs+' | '+speed+'x |');
" 
}

rss() {
  local label="$1"; local cmd="$2"; local cmd2="$3"
  local r1=0 r2=0
  for i in 1 2 3; do
    R=$(/usr/bin/time -l ${=cmd} 2>&1 >/dev/null | awk '/maximum resident/{print $1}'); [ -n "$R" ] && [ "$R" -gt "$r1" ] && r1=$R
    R=$(/usr/bin/time -l ${=cmd2} 2>&1 >/dev/null | awk '/maximum resident/{print $1}'); [ -n "$R" ] && [ "$R" -gt "$r2" ] && r2=$R
  done
  echo "| $label | $((r1/1048576)) | $((r2/1048576)) | $(echo "scale=0; $r1/$r2" | bc)x |"
}

{
echo "# ZCode Node vs Rust — final comparison ($(date '+%Y-%m-%d %H:%M'), M-series, n=20+"
echo ""
echo "## Latency (mean ms, lower is better)"
echo "| command | node_ms | rust_ms | speedup |"
echo "|---|---|---|---|"
row "--version" 20 "node $CLI --version" "$Z --version"
row "--help" 20 "node $CLI --help" "$Z --help"
row "doctor" 20 "node $CLI doctor" "$Z doctor"
row "commands list" 20 "node $CLI commands list" "$Z commands list"
row "skills list" 20 "node $CLI skills list" "$Z skills list"
row "plugins list" 20 "node $CLI plugins list" "$Z plugins list"
echo ""
echo "## Peak RSS (MB, lower is better)"
echo "| command | node_mb | rust_mb | reduction |"
echo "|---|---|---|---|"
rss "--version" "node $CLI --version" "$Z --version"
rss "doctor" "node $CLI doctor" "$Z doctor"
rss "skills list" "node $CLI skills list" "$Z skills list"
rss "plugins list" "node $CLI plugins list" "$Z plugins list"
} > "$OUT"
cat "$OUT"
