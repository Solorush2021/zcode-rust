#!/bin/zsh
# capture_golden.sh — G2 parity fixtures: capture stdout/stderr/exit for each command.
# Usage: capture_golden.sh <binary-invocation...> --outdir <dir>
#   e.g. capture_golden.sh node /path/dist/zcode.cjs --outdir ./golden
# Env pinned for determinism (both sides use the same env at capture & replay).

set -u
OUT=""
ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --outdir) OUT="$2"; shift 2 ;;
    *) ARGS+=("$1"); shift ;;
  esac
done
[ -z "$OUT" ] && { echo "usage: capture_golden.sh <cmd...> --outdir DIR"; exit 2; }
mkdir -p "$OUT"
BIN="${ARGS[1]}"
export NO_COLOR=1 FORCE_COLOR=0 CI=1 TERM=dumb LC_ALL=C LANG=C TZ=UTC

run_case() {
  local name="$1"; shift
  { "${ARGS[@]}" "$@" >"$OUT/$name.out" 2>"$OUT/$name.err"; echo $? > "$OUT/$name.exit"; } </dev/null
  printf '%s\n' "$name: exit=$(cat "$OUT/$name.exit") out=$(wc -c < "$OUT/$name.out" | tr -d ' ')B err=$(wc -c < "$OUT/$name.err" | tr -d ' ')B"
}

run_case version --version
run_case version_short -v
run_case help --help
run_case help_short -h
run_case doctor doctor
run_case commands_list commands list
run_case commands_inspect commands inspect
run_case skills_list skills list
run_case skills_inspect skills inspect
run_case plugins_list plugins list
run_case plugins_validate plugins validate
run_case no_args_help "" 2>/dev/null || true
run_case bogus_flag --definitely-not-a-flag
run_case bogus_cmd definitely-not-a-command
echo "captured -> $OUT"
