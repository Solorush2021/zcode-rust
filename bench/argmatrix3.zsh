#!/bin/zsh
# __internal-search goldens (modeled on argmatrix.zsh)
CLI="/Users/vipulkumar/Documents/zai/zcode-port/ZCode/apps/zcode-cli/packages/cli/dist/zcode.cjs"
OUT="/Users/vipulkumar/Documents/zai/zcode-port/bench/golden-args3"
export NO_COLOR=1 FORCE_COLOR=0 CI=1 TERM=dumb LC_ALL=C LANG=C TZ=UTC
mkdir -p "$OUT"; : > "$OUT/manifest.tsv"
REPO="/Users/vipulkumar/Documents/zai/zcode-port/ZCode/apps/zcode-cli/packages/cli/src"
i=0
cap() {
  i=$((i+1))
  local name="scase$(printf '%03d' $i)"
  { (cd "$REPO" && node "$CLI" "$@") >"$OUT/$name.out" 2>"$OUT/$name.err"; echo $? > "$OUT/$name.exit"; } </dev/null
  local joined=$(printf '%s\x1f' "$@")
  echo -e "$name\t$joined\tcheck" >> "$OUT/manifest.tsv"
}
cap __internal-search
cap __internal-search bogus
cap __internal-search grep --version
cap __internal-search find . -maxdepth 1 -name 'help.ts'
cap __internal-search grep -c parseArgs arguments.ts
cap __internal-search grep -n parseArgs arguments.ts
cap __internal-search grep -rln runDoctor --include='*.ts' .
cap __internal-search grep -Z -n parseArgs arguments.ts
cap __internal-search grep --null -n parseArgs arguments.ts
cap __internal-search grep -rln 'zzz-no-such-token-zzz' --include='*.ts' .
cap __internal-search grep -n --config x parseArgs arguments.ts
cap __internal-search find . -name 'nosuchfile.xyz'
cap __internal-search grep
echo "matrix3: $i cases"
