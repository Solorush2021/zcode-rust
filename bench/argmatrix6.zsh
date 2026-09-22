#!/bin/zsh
# argmatrix6.zsh — __internal-search vendored-tool env goldens (ZCODE_UGREP_BINARY/ZCODE_BFS_BINARY).
#
# TS source of truth (mirrored by zcode-rs internal_search.rs):
#   - env names: packages/shared/src/runtime-tool-runtime.ts binaryEnvVar
#   - vendored-mode args: apps/zcode-cli/packages/adapters/src/exec/embedded-search-prelude.ts
#     UGREP_DEFAULT_ARGS = -G --ignore-files --hidden -I --exclude-dir=<6 VCS dirs>
#     BFS_DEFAULT_ARGS   = -S dfs -regextype findutils-default
#   - bypass args -> system grep, unmodified (prelude `command grep "$@"`)
#
# LIMITATION: node's `__internal-search` (embedded-search-cli.ts) NEVER reads these
# env vars — in TS they are consumed only by the shell prelude backend selector
# (embedded-search-backend.ts, native-binaries kind). So vendored-mode goldens are
# captured by running the env-named binary with exactly the TS prelude args (the
# observable TS behavior in that mode; ugrep/bfs error strings also pin WHICH
# binary ran). Fallback cases (env whitespace / nonexistent path) ARE captured
# from the node bundle, since node's behavior there (system grep/find +
# GREP_DEFAULT_ARGS, env ignored) is exactly what the rust fallback must produce.
CLI="/Users/vipulkumar/Documents/zai/zcode-port/ZCode/apps/zcode-cli/packages/cli/dist/zcode.cjs"
OUT="/Users/vipulkumar/Documents/zai/zcode-port/bench/golden-args6"
BIN="$OUT/bin"
APP="/Applications/ZCode.app/Contents/Resources/tools"
export NO_COLOR=1 FORCE_COLOR=0 CI=1 TERM=dumb LC_ALL=C LANG=C TZ=UTC
REPO="/Users/vipulkumar/Documents/zai/zcode-port/ZCode/apps/zcode-cli/packages/cli/src"

# Vendored tools: reuse copies in bin/, else copy from the desktop app bundle.
mkdir -p "$BIN"
if [[ ! -x "$BIN/ugrep" ]]; then cp "$APP/ugrep/ugrep" "$BIN/ugrep" 2>/dev/null || UGREP_MISSING=1; fi
if [[ ! -x "$BIN/bfs" ]]; then cp "$APP/bfs/bfs" "$BIN/bfs" 2>/dev/null || BFS_MISSING=1; fi
UGREP_BIN="$BIN/ugrep"
BFS_BIN="$BIN/bfs"
if [[ -n "$UGREP_MISSING" || ! -x "$UGREP_BIN" ]]; then
  UGREP_BIN="/usr/bin/grep"   # LIMITED MODE: pins env-dispatch only, not ugrep args
  BFS_BIN="/usr/bin/find"
  echo "WARN: vendored binaries unavailable; using /usr/bin equivalents"
fi

# TS prelude args (embedded-search-prelude.ts) — keep in sync with the source.
UGREP_DEFAULT_ARGS=(-G --ignore-files --hidden -I --exclude-dir=.git --exclude-dir=.svn --exclude-dir=.hg --exclude-dir=.bzr --exclude-dir=.jj --exclude-dir=.sl)
BFS_DEFAULT_ARGS=(-S dfs -regextype findutils-default)

mkdir -p "$OUT"; : > "$OUT/manifest.jsonl"
i=0

# Capture wrappers; each captures bytes then appends the manifest row.
# caprun runs an argv with optional env overrides applied via `env`.
caprun() { # caprun <name> <ug-env> <bf-env> <cmd...>
  local name="$1" ug="$2" bf="$3"; shift 3
  local -a envs=()
  [[ -n "$ug" ]] && envs+=("ZCODE_UGREP_BINARY=$ug")
  [[ -n "$bf" ]] && envs+=("ZCODE_BFS_BINARY=$bf")
  { (cd "$REPO" && env "${envs[@]}" "$@") >"$OUT/$name.out" 2>"$OUT/$name.err"; echo $? > "$OUT/$name.exit"; } </dev/null
}

emit() { # emit <name> <ug-env> <bf-env> <full CLI argv...> (must start with __internal-search)
  local name="$1" ug="$2" bf="$3"; shift 3
  NAME="$name" UGREP_ENV="$ug" BFS_ENV="$bf" OUT="$OUT" REPO="$REPO" \
    python3 -c '
import json, os, sys
row = {
    "name": os.environ["NAME"],
    "argv": sys.argv[1:],
    "mode": "check",
    "cwd": os.environ["REPO"],
}
env = {}
if os.environ.get("UGREP_ENV"): env["ZCODE_UGREP_BINARY"] = os.environ["UGREP_ENV"]
if os.environ.get("BFS_ENV"):   env["ZCODE_BFS_BINARY"] = os.environ["BFS_ENV"]
if env: row["env"] = env
with open(os.path.join(os.environ["OUT"], "manifest.jsonl"), "a") as f:
    f.write(json.dumps(row) + "\n")
' "$@"
}

capug()  { i=$((i+1)); local n="vcase$(printf '%03d' $i)"
  { (cd "$REPO" && "$UGREP_BIN" $UGREP_DEFAULT_ARGS "$@") >"$OUT/$n.out" 2>"$OUT/$n.err"; echo $? >"$OUT/$n.exit"; } </dev/null
  emit "$n" "$UGREP_BIN" "" __internal-search grep "$@"; }
capbfs() { i=$((i+1)); local n="vcase$(printf '%03d' $i)"
  { (cd "$REPO" && "$BFS_BIN" $BFS_DEFAULT_ARGS "$@") >"$OUT/$n.out" 2>"$OUT/$n.err"; echo $? >"$OUT/$n.exit"; } </dev/null
  emit "$n" "" "$BFS_BIN" __internal-search find "$@"; }
capsys() { i=$((i+1)); local n="vcase$(printf '%03d' $i)"   # bypass: system grep, args unmodified
  { (cd "$REPO" && command grep "$@") >"$OUT/$n.out" 2>"$OUT/$n.err"; echo $? >"$OUT/$n.exit"; } </dev/null
  emit "$n" "$UGREP_BIN" "" __internal-search grep "$@"; }
capnode() { local ug="$1" bf="$2"; shift 2; i=$((i+1)); local n="vcase$(printf '%03d' $i)"
  caprun "$n" "$ug" "$bf" node "$CLI" "$@"
  emit "$n" "$ug" "$bf" "$@"; }

# --- vendored-mode grep (env-named ugrep + TS UGREP_DEFAULT_ARGS) ---
capug -c parseArgs arguments.ts
capug -n parseArgs arguments.ts
capug -rln runDoctor --include='*.ts' .
capug -c zzz-no-such-token-zzz arguments.ts          # no match -> exit 1
capug -n parseArgs nosuchfile.xyz                    # stderr "ugrep: ..." pins binary identity
# --- bypass with env set -> system grep, unmodified (no prepend) ---
capsys -Z -n parseArgs arguments.ts
# --- vendored-mode find (env-named bfs + TS BFS_DEFAULT_ARGS) ---
capbfs . -maxdepth 1 -name '*.ts'
capbfs --bogus-option-xyz                            # stderr "bfs: ..." pins binary identity
# --- unsupported command with both envs set (env-independent; node capture) ---
# NOTE: capnode passes CLI argv verbatim -> must include "__internal-search" itself.
capnode "$UGREP_BIN" "$BFS_BIN" __internal-search bogus
# --- fallback: env whitespace / nonexistent path -> system grep + GREP_DEFAULT_ARGS ---
capnode "/nonexistent/ugrep" "" __internal-search grep -c parseArgs arguments.ts
capnode " " "" __internal-search grep -c parseArgs arguments.ts
capnode "" "/nonexistent/bfs" __internal-search find . -name nosuchfile.xyz
echo "matrix6: $i cases (ugrep=$UGREP_BIN bfs=$BFS_BIN)"
