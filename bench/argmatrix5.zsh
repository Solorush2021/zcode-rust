#!/bin/zsh
# zh-CN locale goldens (modeled on argmatrix3.zsh).
# Pins the zh env at capture; manifest rows carry the same env object so
# parity.mjs merges it over its pinned base env (backward-compatible feature).
# Case selection documents the upstream i18n audit:
#   - zh help ONLY via explicit --locale zh-CN or --locale auto (+zh env);
#     bare zh env keeps EN help (resolveLocale(undefined,...) -> en-US quirk).
#   - all pre-dispatch validation errors are EN even under zh (hardcoded in
#     run.ts); --locale xx error is EN too (getZCodeCopy() called w/o locale).
#   - unknown command: EN prefix + locale-aware help on stderr.
#   - doctor = locale-independent runtime facts (mask mode: node:/default
#     artifact: lines differ by design between node and the rust binary).
#   - skills/commands/-c (tui)/-p "" go through the node fallback (zh copy
#     lives in Node) — byte-identical by construction.
CLI="/Users/vipulkumar/Documents/zai/zcode-port/ZCode/apps/zcode-cli/packages/cli/dist/zcode.cjs"
OUT="/Users/vipulkumar/Documents/zai/zcode-port/bench/golden-args5"
REPO="/Users/vipulkumar/Documents/zai/zcode-port/ZCode"
ENVJSON='{"LANG":"zh_CN.UTF-8","LC_ALL":"zh_CN.UTF-8"}'
export NO_COLOR=1 FORCE_COLOR=0 CI=1 TERM=dumb TZ=UTC LANG=zh_CN.UTF-8 LC_ALL=zh_CN.UTF-8
mkdir -p "$OUT"
CWDJSON=$(python3 -c "import json,sys; print(json.dumps(sys.argv[1]))" "$REPO")
i=0
cap() {
  i=$((i+1))
  local name="zcase$(printf '%03d' $i)"
  local mode="check"
  for a in "$@"; do [[ "$a" == "doctor" ]] && mode="mask"; done
  { (cd "$REPO" && node "$CLI" "$@") >"$OUT/$name.out" 2>"$OUT/$name.err"; echo $? > "$OUT/$name.exit"; } </dev/null
  local argvjson
  argvjson=$(node -e 'process.stdout.write(JSON.stringify(process.argv.slice(1)))' -- "$@")
  printf '{"name":"%s","argv":%s,"mode":"%s","cwd":%s,"env":%s}\n' "$name" "$argvjson" "$mode" "$CWDJSON" "$ENVJSON" >> "$OUT/manifest.jsonl"
}
# help rendering: explicit zh / bare zh env (EN quirk) / auto (env-detected zh) / explicit en
cap --help
cap --locale zh-CN --help
cap --locale zh-CN -h
cap --locale zh-CN help
cap --locale auto --help
cap --locale en-US --help
# version (locale-independent)
cap --locale zh-CN --version
cap --locale zh-CN version
# unknown command: locale-aware help after EN prefix
cap --locale zh-CN bogus_cmd
cap bogus_cmd
# --locale xx: EN error even under zh env (verified quirk)
cap --locale xx --version
# pre-dispatch validation errors: hardcoded EN upstream
cap --locale zh-CN --mode bogus
cap --locale zh-CN --surface bogus
cap --locale zh-CN --output-format bogus
cap --locale zh-CN --target "  "
cap --locale zh-CN --target-replace
cap --locale zh-CN --continue --resume x
cap --locale zh-CN --memory-bench
cap --locale zh-CN --browser-use bogus
cap --locale zh-CN --browser-executable /tmp/x
cap --locale zh-CN --force-mcs
cap --locale zh-CN --target x --prompt y
cap --locale zh-CN --cwd /nonexistent-zh-zcase
cap --locale zh-CN --cwd=
cap --locale zh-CN --cwd /etc/hosts
# parse-arg error: msg + EN help even under zh (writeHelp w/o locale)
cap --locale zh-CN --foo
# doctor: locale-independent (mask mode)
cap --locale zh-CN doctor
cap --locale zh-CN --json doctor
# node fallback paths (zh copy lives in Node)
cap --locale zh-CN skills
cap --locale zh-CN commands
cap --locale zh-CN -c
cap --locale zh-CN -p ""
echo "matrix5: $i cases"
