#!/bin/zsh
# hooks trust goldens (modeled on argmatrix3.zsh); manifest.jsonl incl. cwd field
CLI="/Users/vipulkumar/Documents/zai/zcode-port/ZCode/apps/zcode-cli/packages/cli/dist/zcode.cjs"
OUT="/Users/vipulkumar/Documents/zai/zcode-port/bench/golden-args4"
export NO_COLOR=1 FORCE_COLOR=0 CI=1 TERM=dumb LC_ALL=C LANG=C TZ=UTC
rm -rf "$OUT"; mkdir -p "$OUT"
REPO="/Users/vipulkumar/Documents/zai/zcode-port/ZCode/apps/zcode-cli/packages/cli/src"
i=0
cap() {
  i=$((i+1))
  local name="scase$(printf '%03d' $i)"
  { (cd "$REPO" && node "$CLI" "$@") >"$OUT/$name.out" 2>"$OUT/$name.err"; echo $? > "$OUT/$name.exit"; } </dev/null
  local -a parts
  for a in "$@"; do parts+=("$(printf '%s' "$a" | sed 's/["\\]/\\&/g')"); done
  local jsonargv
  jsonargv="[$(printf '"%s",' "${parts[@]}" | sed 's/,$//')]"
  printf '{"name":"%s","argv":%s,"mode":"check","cwd":"%s"}\n' "$name" "$jsonargv" "$REPO" >> "$OUT/manifest.jsonl"
}
cap hooks
cap hooks trust
cap hooks trust status
cap hooks trust status --json
cap hooks trust review
cap hooks trust review --json
cap hooks trust status --workspace /Users/vipulkumar/Documents/zai/zcode-port/ZCode
cap hooks bogus
cap hooks trust bogus
cap hooks trust grant
cap hooks trust status --workspace /nonexistent
echo "matrix4: $i cases"
