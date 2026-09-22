#!/bin/zsh
# Second adversarial matrix (core paths only; separate dir to avoid disturbing port agents).
CLI="/Users/vipulkumar/Documents/zai/zcode-port/ZCode/apps/zcode-cli/packages/cli/dist/zcode.cjs"
OUT="/Users/vipulkumar/Documents/zai/zcode-port/bench/golden-args2"
export NO_COLOR=1 FORCE_COLOR=0 CI=1 TERM=dumb LC_ALL=C LANG=C TZ=UTC
mkdir -p "$OUT"; : > "$OUT/manifest.tsv"
i=0
cap() {
  local mode="${1:-check}"; shift
  i=$((i+1))
  local name="xcase$(printf '%03d' $i)"
  { node "$CLI" "$@" >"$OUT/$name.out" 2>"$OUT/$name.err"; echo $? > "$OUT/$name.exit"; } </dev/null
  local joined=$(printf '%s\x1f' "$@")
  echo -e "$name\t$joined\t$mode" >> "$OUT/manifest.tsv"
}
cap check --target=x --version
cap check --target=
cap check --output-format=json --version
cap check --browser-use=headless --version
cap check --locale=zh-CN --version
cap check --cwd= --version
cap check -vh
cap check -hv
cap check -c
cap check -f
cap check -cf --version
cap check -fa --version
cap check -h=true
cap check --verbose=x
cap check --no-color=1
cap check --héllo
cap check -é
cap check -- -h
cap check -- --help
cap check help --json
cap check version --json
cap check --help extra
cap check doctor --json extra
cap check --json
cap check --verbose
cap check --attach=
cap check --sparse=
cap check -p ""
cap check --prompt=
cap check -- "" ""
cap check "---"
cap check "-",
cap check --mode=BUILD --version
cap check --mode=Yolo --version
cap check --browser-use=HEADLESS --version
cap check --surface=DESKTOP --version
cap check --output-format=JSON --version
echo "matrix2: $i cases"
