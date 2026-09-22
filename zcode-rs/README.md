# zcode-rs

Byte-parity Rust front-end for the ZCode CLI. A ~426KB native binary that reproduces
the TypeScript CLI (`ZCode/apps/zcode-cli`, 281k lines TS / 21MB esbuild bundle)
**byte-identically** for every local deterministic command, and transparently `exec`s
the Node bundle for agent/LLM/TUI/network paths (the fish-shell pattern: port the
surface, keep behavior locked, grow inward).

Headline numbers (macOS arm64, `PORT_REPORT.md`): **187–252x faster** on local
commands (e.g. `--version` 369.6 → 1.5 ms) and **367–371 MB → 1–2 MB peak RSS**.

| path | native Rust | Node fallback (exec `zcode.cjs`) |
|---|---|---|
| arg parsing, `--version/-v`, `--help/-h`, `doctor`*, `commands`, `skills`, `plugins list` | yes | — |
| `-p/--prompt`, `tui`, `app-server`/`agent-server`, `login`, `hooks`, `plugins install/uninstall/marketplace/validate`, `__internal-search` (native spawn-passthrough, FIX-D), plugin-host, dwf, zh-CN locale | — | yes |

\* `doctor` deviates on 3 runtime-dependent lines by design (see Deviations).

## Layout

```
zcode-rs/
├── Cargo.toml            workspace (in-folder .cargo/.rustup toolchain lives in the port root)
├── crates/zcode-cli/     the binary: main/args/run/help/doctor/fallback + batch-3 modules
│                         (warm_pool, hooks_cmd, i18n_zh, plugin_dwf, internal_search)
├── scripts/package-rs.sh build release + assemble dist/zcode-rs-<ver>/ payload + tar.gz
├── scripts/install.sh    install payload into a prefix (default ~/.zcode/rust-bin)
├── dist/                 packaging output (gitignore-able)
└── target/release/zcode  the built binary
```

Packaged payload (`scripts/package-rs.sh`):

```
zcode-rs-<ver>/
├── zcode              portable launcher (sources zcode.env, resolves ZCODE_NODE_BUNDLE, execs bin/zcode)
├── bin/zcode          raw native binary
├── runtime/zcode.cjs  Node fallback sidecar (copy of the TS CLI bundle)
├── zcode.env          install defaults; ZCODE_NODE_BUNDLE=../runtime/zcode.cjs (bin-relative)
├── install.sh         self-contained installer
├── sha256sums.txt     checksums over every payload file
└── README             this file
```

Install layout (`scripts/install.sh [PREFIX]`, default `~/.zcode/rust-bin`):
`<prefix>/bin/zcode` = generated wrapper (exports `ZCODE_NODE_BUNDLE=<prefix>/runtime/zcode.cjs`,
execs the binary), `<prefix>/libexec/zcode` = binary, plus `runtime/`, `zcode.env`,
`README`, and a `.zcode-rs-files` manifest. Idempotent; `--uninstall` removes exactly
the manifest-listed files. PATH hint is printed after install.

## Build

In-folder toolchain, no global Rust required:

```bash
PORTDIR=~/Documents/zai/zcode-port
export CARGO_HOME="$PORTDIR/.cargo" RUSTUP_HOME="$PORTDIR/.rustup" PATH="$PORTDIR/.cargo/bin:$PATH"
cd "$PORTDIR/zcode-rs" && cargo build --release
```

Package + install:

```bash
"$PORTDIR/zcode-rs/scripts/package-rs.sh"            # or --skip-build to repackage only
"$PORTDIR/zcode-rs/scripts/install.sh" [PREFIX]      # auto-picks newest dist/zcode-rs-*
"$PORTDIR/zcode-rs/scripts/install.sh --uninstall [PREFIX]"
```

## Parity gates (all 4 must pass — 161/161 total)

```bash
cd ~/Documents/zai/zcode-port
node bench/parity.mjs zcode-rs/target/release/zcode                      # 111/111
node bench/parity.mjs zcode-rs/target/release/zcode bench/golden-args2   # 37/37
node bench/parity.mjs zcode-rs/target/release/zcode bench/golden-args3   # 13/13 (FIX-D __internal-search)
node bench/parity.mjs zcode-rs/dist/zcode-rs-<ver>/bin/zcode             # 111/111, the packaged artifact
```

## Bench

```bash
export NO_COLOR=1 FORCE_COLOR=0
cd ~/Documents/zai/zcode-port
./bench/final.sh                                    # node-vs-rust comparison table
"$PORTDIR/.cargo/bin/hyperfine" -w 3 -n 10 \
  'zcode-rs/target/release/zcode --version' \
  'node ZCode/apps/zcode-cli/packages/cli/dist/zcode.cjs --version'
/usr/bin/time -l zcode-rs/target/release/zcode --version 2>&1 >/dev/null | grep maximum   # peak RSS (bytes, macOS)
```

## Environment variables

Fallback-bundle resolution order (see `crates/zcode-cli/src/fallback.rs`):
`ZCODE_NODE_BUNDLE` > `$ZCODE_PORT_DIR/ZCode/apps/zcode-cli/packages/cli/dist/zcode.cjs` >
compile-time port path > `zcode.cjs` next to the binary.

| var | meaning |
|---|---|
| `ZCODE_NODE` | node runtime used for fallback dispatch (default: `node` on PATH). |
| `ZCODE_NODE_BUNDLE` | path to the sidecar bundle. Relative paths (as shipped in `zcode.env`: `../runtime/zcode.cjs`) are resolved by the wrapper against the binary's directory, so installs are relocatable; absolute paths used as-is. Wins over everything — including a value in `zcode.env`, which only fills the variable when unset (`${VAR:-default}`). |
| `ZCODE_PORT_DIR` | port workspace root; alternate bundle resolver + parent of the compile-cache dir. Leave unset for wrapper installs. |
| `ZCODE_WARM_POOL` | reserved for the warm app-server pool (batch-3 target A). Accepted today, no-op while `warm_pool.rs` is a passthrough stub. |
| `NODE_COMPILE_CACHE` | If unset, the fallback sets it to `<portdir>/.node-compile-cache` (FIX-A): **−119 ms / −33%** per Node dispatch. Set it yourself to override location; the binary never unsets it. |

## Cross-compile

Platform payloads other than the local one are built in `dist-rs/` (batch-3 target E:
`aarch64/x86_64-apple-darwin`, `x86_64/aarch64-unknown-linux-gnu`, windows-msvc planned).
Both scripts accept `--from <payload-dir>` to package/install any of those payloads;
binaries are OS/arch-specific, so install the payload matching the host.

## Known deviations (by design)

- **doctor**: 3 runtime-dependent lines differ (`node:`, `default artifact:`, `execPath:` —
  the Rust binary reports its true runtime). Masked (`mode=mask`) in the parity gate;
  everything else is byte-exact.
- **SEA caveat (FIX-E)**: a single-file SEA binary has no `node_modules`, so the first
  DWF workflow compile inside SEA throws until a `sea-ts-assets` collector (mirror of
  `sea-tui-assets.mjs`) exists; non-SEA CLIs are unaffected. Same applies to this
  package's `runtime/zcode.cjs` sidecar: DWF workflow *compilation* needs `typescript`
  resolvable from `runtime/` (the port workspace's hoisted `node_modules` provides it;
  for relocated installs `export NODE_PATH=$PORTDIR/ZCode/node_modules`). DWF listing
  and all non-compile paths work from the bare sidecar.
- **SIGTERM forwarding (FIX-D)**: a programmatic (non-terminal) SIGTERM sent to the
  Rust wrapper only is not forwarded to the fallback child; terminal signals reach the
  child via the process group. Documented, accepted.
- **zh-CN locale**: served via the Node fallback with ported copy tables — correct,
  slower, until i18n is fully native.
