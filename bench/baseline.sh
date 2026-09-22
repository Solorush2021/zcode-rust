#!/bin/zsh
set -u
PORTDIR=/Users/vipulkumar/Documents/zai/zcode-port
CLI="$PORTDIR/ZCode/apps/zcode-cli/packages/cli"
B="$PORTDIR/bench/bench.sh"; chmod +x "$B" "$PORTDIR/bench/capture_golden.sh"
OUT="$PORTDIR/bench/BASELINE.md"
export NO_COLOR=1 FORCE_COLOR=0
echo "# Node baseline ($(date '+%Y-%m-%d %H:%M'), node $(node --version), n=20 hyperfine + 5x time -l" > "$OUT"
echo "| command | mean_ms | stddev_ms | peak_rss_mb | user_s | sys_s |" >> "$OUT"
echo "|---|---|---|---|---|---|" >> "$OUT"
"$B" "floor: node -e 1" 20 node -e 1 >> "$OUT"
"$B" "version" 20 node "$CLI/dist/zcode.cjs" --version >> "$OUT"
"$B" "help" 20 node "$CLI/dist/zcode.cjs" --help >> "$OUT"
"$B" "doctor" 20 node "$CLI/dist/zcode.cjs" doctor >> "$OUT"
"$B" "commands list" 20 node "$CLI/dist/zcode.cjs" commands list >> "$OUT"
"$B" "skills list" 20 node "$CLI/dist/zcode.cjs" skills list >> "$OUT"
"$B" "plugins list" 20 node "$CLI/dist/zcode.cjs" plugins list >> "$OUT"
cat "$OUT"
