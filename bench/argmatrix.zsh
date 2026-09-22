#!/bin/zsh
# Captures golden fixtures + manifest.tsv (name \t \x1f-joined argv \t mode)
CLI="/Users/vipulkumar/Documents/zai/zcode-port/ZCode/apps/zcode-cli/packages/cli/dist/zcode.cjs"
OUT="/Users/vipulkumar/Documents/zai/zcode-port/bench/golden-args"
export NO_COLOR=1 FORCE_COLOR=0 CI=1 TERM=dumb LC_ALL=C LANG=C TZ=UTC
mkdir -p "$OUT"; : > "$OUT/manifest.tsv"
i=0
cap() {
  local mode="${1:-check}"; shift
  i=$((i+1))
  local name="case$(printf '%03d' $i)"
  { node "$CLI" "$@" >"$OUT/$name.out" 2>"$OUT/$name.err"; echo $? > "$OUT/$name.exit"; } </dev/null
  local joined=$(printf '%s\x1f' "$@")
  echo -e "$name\t$joined\t$mode" >> "$OUT/manifest.tsv"
}
cap check --version
cap check -v
cap check --help
cap check -h
cap check help
cap check version
cap check --json --version
cap check --version --json
cap mask --json doctor
cap check --json --help
cap check --no-color --version
cap check --verbose --version
cap mask --verbose doctor
cap mask --verbose --json doctor
cap check --foo
cap check -x
cap check --version=true
cap check --help=false
cap check -vX
cap check -hv
cap check --version extra
cap check extra --version
cap check -- --version
cap check --locale en --version
cap check --locale xx --version
cap check --locale
cap check --mode build --version
cap check --mode bogus --version
cap check --browser-use headless --version
cap check --browser-use bogus
cap check --browser-executable /tmp/x --version
cap check --surface desktop --version
cap check --surface bogus
cap check --output-format json --version
cap check --output-format bogus
cap check --target "  "
cap check --target-replace
cap check --target x --prompt y
cap check --continue --resume abc --version
cap check --memory-bench
cap check -p
cap check --prompt
cap check --cwd
cap check --cwd /tmp --version
cap check --force-mcs --version
cap check --browser-use headless --version
cap check --all --version
cap check -a --version
cap check --sparse a --sparse b --version
cap check --locale en --locale zh --version
cap check -vv
cap check --version --version
cap check --Prompt x
cap check --VERSION
cap check -- --
cap check ""
cap check " "
cap check --disallowed-tools WebSearch --version
cap check --disallowed-tools
cap check --disallowed-tools=WebSearch,Grep --version
cap check skills
cap check skills list
cap check skills inspect
cap check skills inspect agent-browser
cap check skills bogus
cap check commands
cap check commands list
cap check commands inspect
cap check commands bogus
cap check plugins
cap check plugins list
cap check plugins bogus
cap mask doctor extra
cap check version --json
cap check -pfoo --version
cap check -p=foo --version
cap check --locale --version
cap check --locale -- --version
cap check --json=x --version
cap check --memory-bench --version
cap check --cwd /definitely/not/here --version
cap check --foo=bar
cap check --all=1 --version
cap check --scope
cap check -s
cap check -s x --version
cap check --attach
cap check --attach a --version
cap check --prompt hello --version
cap check -- -- --version
cap check --version --
cap skip -p hello
cap skip --target abc
cap check --browser-use headless
cap check --surface terminal --version
cap check --output-format stream-json --version
cap check --no-browser --version
cap check --force --version
cap check --stdio --version
cap check --prepare-storage --version
cap check --keep-data --version
cap check --available --version
cap check --verbose --json skills list
cap check --json skills list
cap check --json commands list
cap check --json plugins list
cap check --json skills inspect nosuch
cap check --json commands inspect nosuch
cap check skills inspect arc-agi3-agent
cap check commands inspect /review:code
cap check -s -x
cap check --scope -x
cap check hooks
echo "manifest: $(wc -l < $OUT/manifest.tsv | tr -d ' ') cases"
