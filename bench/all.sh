#!/bin/zsh
# all.sh — one-shot verification: rebuild → all parity gates → bench.
#   ./bench/all.sh                      full: build + parity + hyperfine bench
#   ZCODE_SKIP_BENCH=1 ./bench/all.sh   build + parity only (fast)
# Fail-fast: stops at the first failing stage. Exit 0 = all green.
set -u
PORTDIR=${0:a:h:h}

# In-folder cargo toolchain (see OPTIMIZATION_CONTEXT.md)
export CARGO_HOME="$PORTDIR/.cargo" RUSTUP_HOME="$PORTDIR/.rustup"
export PATH="$PORTDIR/.cargo/bin:$PATH"
export NO_COLOR=1 FORCE_COLOR=0

BIN="$PORTDIR/zcode-rs/target/release/zcode"
BUILD_LOG=/tmp/zcode-all-build.log
BENCH_LOG=/tmp/zcode-all-bench.log
typeset -a rows
fail=0

echo "== [1/3] rebuild (cargo build --release) =="
if (cd "$PORTDIR/zcode-rs" && cargo build --release >"$BUILD_LOG" 2>&1); then
  rows+=("build|release|OK ($(wc -c < "$BIN" | tr -d ' ') bytes)")
  echo "   OK ($(wc -c < "$BIN" | tr -d ' ') bytes)"
else
  rows+=("build|release|FAIL (log: $BUILD_LOG)")
  echo "   FAIL — last 20 lines of $BUILD_LOG:"
  tail -20 "$BUILD_LOG"
  fail=1
fi

echo "== [2/3] parity gates (bench/golden-args*/ via parity.mjs) =="
if (( fail )); then
  rows+=("parity|skipped|build failed")
elif ! command -v node >/dev/null 2>&1; then
  rows+=("parity|all gates|SKIP (no node on PATH)")
  echo "   SKIP — node not found"
else
  for m in "$PORTDIR"/bench/golden-args*/manifest.jsonl(N); do
    gate=${m:h:t}
    summary=$(node "$PORTDIR/bench/parity.mjs" "$BIN" "${m:h}" 2>&1)
    if (( $? == 0 )); then
      rows+=("parity|$gate|$summary")
      echo "   $gate: $summary"
    else
      rows+=("parity|$gate|FAIL — $summary")
      echo "   $gate: FAIL — $summary"
      fail=1
      break
    fi
  done
  (( ${#rows} )) || { rows+=("parity|none|SKIP (no golden-args*/manifest.jsonl found)"); echo "   SKIP — no matrices found"; }
fi

echo "== [3/3] bench (bench/final.sh) =="
if (( fail )); then
  rows+=("bench|final.sh|skipped (earlier stage failed)")
  echo "   skipped (earlier stage failed)"
elif [[ "${ZCODE_SKIP_BENCH:-0}" == "1" ]]; then
  rows+=("bench|final.sh|SKIP (ZCODE_SKIP_BENCH=1)")
  echo "   SKIP (ZCODE_SKIP_BENCH=1)"
elif zsh "$PORTDIR/bench/final.sh" >"$BENCH_LOG" 2>&1; then
  rows+=("bench|final.sh|OK → bench/FINAL.md (log: $BENCH_LOG)")
  echo "   OK → bench/FINAL.md (log: $BENCH_LOG)"
else
  rows+=("bench|final.sh|FAIL (log: $BENCH_LOG)")
  echo "   FAIL — last 20 lines of $BENCH_LOG:"
  tail -20 "$BENCH_LOG"
  fail=1
fi

echo
echo "=============== summary ==============="
printf '  %-7s | %-11s | %s\n' stage gate result
printf '  -------+-------------+-------------------\n'
for r in "${rows[@]}"; do
  IFS='|' read -r stage gate result <<< "$r"
  printf '  %-7s | %-11s | %s\n' "$stage" "$gate" "$result"
done
echo "======================================="
exit "$fail"
