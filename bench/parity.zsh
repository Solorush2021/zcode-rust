#!/bin/zsh
# parity.zsh — G2 gate: replay every golden case against a binary and byte-compare.
# Usage: parity.zsh <binary> [golden-dir]
# Exits nonzero if ANY case mismatches. Prints one line per case.

set -u
BIN="$1"
GDIR="${2:-/Users/vipulkumar/Documents/zai/zcode-port/bench/golden-args}"
PORTDIR=/Users/vipulkumar/Documents/zai/zcode-port
MANIFEST="$GDIR/manifest.tsv"

if [ ! -f "$MANIFEST" ]; then
  echo "manifest missing: $MANIFEST (regenerate with matrix scripts)" >&2
  exit 2
fi

export NO_COLOR=1 FORCE_COLOR=0 CI=1 TERM=dumb LC_ALL=C LANG=C TZ=UTC
pass=0; fail=0
while IFS=$'\t' read -r name argv_str mode; do
  [ -z "$name" ] && continue
  # argv_str: args separated by \x1f (unit separator) to survive spaces/empties
  args=("${(@ps/$'\x1f'/)argv_str}")
  tmpd=$(mktemp -d)
  "$BIN" "${args[@]}" >"$tmpd/out" 2>"$tmpd/err" </dev/null
  code=$?
  ok=1
  [ "$code" != "$(cat "$GDIR/$name.exit")" ] && ok=0
  cmp -s "$tmpd/out" "$GDIR/$name.out" || ok=0
  if [ "$mode" = "mask" ]; then
    # byte-parity except masked (runtime-dependent) lines on stderr/stdout
    diff <(grep -vE '^(node|execPath|cwd):' "$tmpd/out") <(grep -vE '^(node|execPath|cwd):' "$GDIR/$name.out") >/dev/null || ok=0
  else
    cmp -s "$tmpd/err" "$GDIR/$name.err" || ok=0
  fi
  if [ $ok = 1 ]; then pass=$((pass+1)); else fail=$((fail+1)); echo "FAIL $name (exit=$code)"; fi
  rm -rf "$tmpd"
done < "$MANIFEST"
echo "parity: $pass pass, $fail fail / $((pass+fail))"
[ $fail = 0 ]
