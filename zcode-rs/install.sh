#!/bin/sh
# zcode-rust one-click installer (macOS arm64/x64, Linux arm64/x64, Windows via MSYS/Git-Bash)
# Usage: curl -fsSL https://raw.githubusercontent.com/<owner>/zcode-rust/main/install.sh | sh
# Env: GH_REPO="owner/zcode-rust" (default below), PREFIX=~/.zcode/rust-bin
set -eu

GH_REPO="${GH_REPO:-OWNER_PLACEHOLDER/zcode-rust}"
PREFIX="${PREFIX:-$HOME/.zcode/rust-bin}"
VER="0.16.9"

case "$(uname -s)/$(uname -m)" in
  Darwin/arm64)  target="aarch64-apple-darwin" ;;
  Darwin/x86_64) target="x86_64-apple-darwin" ;;
  Darwin/*)      echo "Unsupported Mac arch: $(uname -m)"; exit 1 ;;
  Linux/aarch64) target="aarch64-unknown-linux-gnu" ;;
  Linux/x86_64)  target="x86_64-unknown-linux-gnu" ;;
  MINGW*/x86_64|MSYS*/x86_64) target="x86_64-pc-windows-gnu" ;;
  *) echo "Unsupported platform: $(uname -s) $(uname -m)"; exit 1 ;;
esac

asset="zcode-rust-$VER-$target.tar.gz"
base="https://github.com/$GH_REPO/releases/download/v$VER"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT

echo ">> downloading $asset"
curl -fSL "$base/$asset" -o "$tmp/$asset"
curl -fsSL "$base/SHA256SUMS.txt" -o "$tmp/SHA256SUMS.txt" || true
if [ -s "$tmp/SHA256SUMS.txt" ]; then
  (cd "$tmp" && grep " $asset\$" SHA256SUMS.txt | shasum -a 256 -c -) || { echo "checksum FAILED"; exit 1; }
fi
tar -xzf "$tmp/$asset" -C "$tmp"
sh "$tmp/zcode-rust-$VER-$target/install.sh" "$PREFIX" >/dev/null

echo ">> installed. restart your shell or:"
echo "   export PATH=\"$PREFIX/bin:\$PATH\""
echo "   zcode --version"
