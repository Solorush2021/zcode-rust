#!/bin/zsh
# Per-target release tarballs: stage full payload via package-rs.sh, swap binary per target.
set -eu
PORTDIR="$(cd "$(dirname "$0")/../.." && pwd)"
RS="$PORTDIR/zcode-rs"
VER="0.16.9"
"$RS/scripts/package-rs.sh"
STAGE="$RS/dist/zcode-rs-$VER"
[ -d "$STAGE" ] || { echo "stage missing after package-rs.sh"; exit 1; }
OUT="$RS/release"; rm -rf "$OUT"; mkdir -p "$OUT"
for target in aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu x86_64-pc-windows-gnu; do
  bin="$RS/dist-rs/$target/zcode"
  [ -f "$bin" ] || bin="$RS/dist-rs/$target/zcode.exe"
  [ -f "$bin" ] || { echo "missing $target"; continue }
  D="$OUT/zcode-rust-$VER-$target"
  rm -rf "$D"
  cp -R "$STAGE" "$D"
  rm -f "$D/bin/zcode"
  cp "$bin" "$D/bin/"
  chmod +x "$D/bin/"*
  if [ -f "$D/sha256sums.txt" ]; then
    (cd "$D" && rm -f sha256sums.txt && shasum -a 256 $(find . -type f ! -name sha256sums.txt | sed 's|^\./||') > sha256sums.txt)
  fi
  tar -czf "$OUT/zcode-rust-$VER-$target.tar.gz" -C "$OUT" "zcode-rust-$VER-$target"
  rm -rf "$D"
done
(cd "$OUT" && shasum -a 256 *.tar.gz > SHA256SUMS.txt)
ls "$OUT"
