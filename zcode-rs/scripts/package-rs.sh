#!/bin/sh
# package-rs.sh — build the release binary (in-folder cargo toolchain) and
# assemble a distributable zcode-rs-<version>/ payload:
#
#   zcode-rs-<ver>/
#     zcode              portable launcher (resolves sidecar, execs bin/zcode)
#     bin/zcode          raw native binary
#     runtime/zcode.cjs  Node fallback sidecar (copy of the TS CLI esbuild bundle)
#     zcode.env          install defaults (ZCODE_NODE_BUNDLE, bin-relative)
#     install.sh         self-contained installer (see scripts/install.sh)
#     README             full project README
#     sha256sums.txt     checksums over every payload file
#
# Emits dist/zcode-rs-<ver>/ , dist/zcode-rs-<ver>.tar.gz and its .sha256.
#
# Usage: scripts/package-rs.sh [--skip-build]
set -eu

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
PORTDIR=$(CDPATH='' cd -- "$SCRIPT_DIR/../.." && pwd -P)  # zcode-port root
RSDIR="$PORTDIR/zcode-rs"
BUNDLE_SRC="$PORTDIR/ZCode/apps/zcode-cli/packages/cli/dist/zcode.cjs"

SKIP_BUILD=0
[ "${1:-}" = "--skip-build" ] && SKIP_BUILD=1

# sha256 tool (macOS ships shasum, Linux sha256sum); must be a real binary for xargs.
if command -v sha256sum >/dev/null 2>&1; then SHASUM=sha256sum; else SHASUM="shasum -a 256"; fi

# In-folder cargo/rustup toolchain (OPTIMIZATION_CONTEXT.md "Build").
export CARGO_HOME="$PORTDIR/.cargo"
export RUSTUP_HOME="$PORTDIR/.rustup"
export PATH="$CARGO_HOME/bin:$PATH"

if [ "$SKIP_BUILD" -eq 0 ]; then
  echo "==> cargo build --release (in-folder toolchain)"
  (cd "$RSDIR" && cargo build --release)
fi

BIN="$RSDIR/target/release/zcode"
[ -x "$BIN" ] || { echo "error: $BIN not found (build first)" >&2; exit 1; }
[ -f "$BUNDLE_SRC" ] || {
  echo "error: sidecar bundle missing: $BUNDLE_SRC" >&2
  echo "  build it first: cd $PORTDIR/ZCode/apps/zcode-cli && pnpm build" >&2
  exit 1
}

# Version = what the binary reports (build.rs derives it from the TS package.json).
VER=$("$BIN" --version | head -n1 | tr -dc '0-9A-Za-z._-')
[ -n "$VER" ] || VER=$(sed -n 's/^version = "\(.*\)"/\1/p' "$RSDIR/crates/zcode-cli/Cargo.toml" | head -n1)
[ -n "$VER" ] || { echo "error: cannot determine version" >&2; exit 1; }

STAGE="$RSDIR/dist/zcode-rs-$VER"
echo "==> assembling $STAGE"
rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/runtime"

cp "$BIN" "$STAGE/bin/zcode"
chmod 755 "$STAGE/bin/zcode"
cp "$BUNDLE_SRC" "$STAGE/runtime/zcode.cjs"
cp "$SCRIPT_DIR/install.sh" "$STAGE/install.sh"
chmod 755 "$STAGE/install.sh"
cp "$RSDIR/README.md" "$STAGE/README"

cat > "$STAGE/zcode.env" <<'EOF'
# zcode.env — zcode-rs install defaults. Sourced by the zcode wrapper.
# ZCODE_NODE_BUNDLE: Node fallback sidecar (zcode.cjs) used for agent/LLM/TUI
# paths. Relative paths resolve against the directory containing the native
# binary, so the shipped "../runtime/zcode.cjs" survives prefix relocation;
# absolute paths are used as-is. Takes precedence over ZCODE_PORT_DIR and the
# compiled-in port path. The :- default means a value already set in the
# caller's environment always wins over this file.
export ZCODE_NODE_BUNDLE="${ZCODE_NODE_BUNDLE:-../runtime/zcode.cjs}"

# ZCODE_NODE: node runtime for fallback dispatch (default: `node` on PATH).
# export ZCODE_NODE=node

# ZCODE_PORT_DIR: port workspace root; fallback bundle resolver and the parent
# of .node-compile-cache. Leave unset for wrapper installs (ZCODE_NODE_BUNDLE
# above already pins the bundle).

# ZCODE_WARM_POOL: reserved for the warm app-server pool (batch-3 target A).
# Accepted today but a no-op while warm_pool.rs is a passthrough stub.
# export ZCODE_WARM_POOL=1
EOF

cat > "$STAGE/zcode" <<'EOF'
#!/bin/sh
# Portable launcher for an unpacked zcode-rs-<ver> directory (no install
# needed). Resolves ZCODE_NODE_BUNDLE against bin/ and execs the native
# binary; for a system install use ./install.sh instead.
ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
[ -f "$ROOT/zcode.env" ] && . "$ROOT/zcode.env"
case "${ZCODE_NODE_BUNDLE:-}" in
  "") ZCODE_NODE_BUNDLE="$ROOT/runtime/zcode.cjs" ;;
  /*) ;;
  *)  ZCODE_NODE_BUNDLE="$ROOT/bin/$ZCODE_NODE_BUNDLE" ;;
esac
export ZCODE_NODE_BUNDLE
exec "$ROOT/bin/zcode" "$@"
EOF
chmod 755 "$STAGE/zcode"

echo "==> sha256sums"
PAYLOAD="README install.sh zcode zcode.env bin/zcode runtime/zcode.cjs"
( cd "$STAGE" && for f in $PAYLOAD; do printf '%s\n' "$f"; done | LC_ALL=C sort | xargs $SHASUM ) \
  > "$STAGE/sha256sums.txt"

TARBALL="$RSDIR/dist/zcode-rs-$VER.tar.gz"
echo "==> $TARBALL"
COPYFILE_DISABLE=1 tar -czf "$TARBALL" -C "$RSDIR/dist" "zcode-rs-$VER"
( cd "$RSDIR/dist" && $SHASUM "zcode-rs-$VER.tar.gz" ) > "$TARBALL.sha256"

echo "==> done"
ls -lh "$STAGE/bin/zcode" "$STAGE/runtime/zcode.cjs" "$TARBALL" | awk '{print $9, $5}'
echo "    payload: $STAGE  (verify: shasum -a 256 -c $STAGE/sha256sums.txt)"
