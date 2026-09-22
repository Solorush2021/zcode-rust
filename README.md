<div align="center">

# ⚡ zcode-rust

**A byte-identical Rust front-end for [ZCode](https://github.com/zai-org/ZCode) — 252x faster, 224x lighter.**

[![Release](https://img.shields.io/github/v/release/Solorush2021/zcode-rust?color=blue&label=release)](https://github.com/Solorush2021/zcode-rust/releases)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux%20%7C%20Windows-green)](#install)
[![Parity](https://img.shields.io/badge/golden%20parity-216%2F216-brightgreen)](#verification)
[![License](https://img.shields.io/badge/license-Apache--2.0-lightgrey)](#license)

`curl -fsSL https://raw.githubusercontent.com/Solorush2021/zcode-rust/main/install.sh | sh`

</div>

---

## What is this?

ZCode is a 281k-line TypeScript coding agent whose every CLI invocation pays ~380ms and ~370MB just to boot its Node bundle. **zcode-rust is a 674KB native binary that replaces that front-end** — byte-identical output on a 216-case golden harness — while transparently `exec`-ing the Node bundle for agent/LLM paths. Same commands, same bytes, same exit codes. 252x faster.

| command | Node | zcode-rust | speedup | RAM: Node → Rust |
|---|---|---|---|---|
| `--version` | 369.6ms | 1.5ms | **252x** | 367MB → **1MB** |
| `--help` | 378.0ms | 1.6ms | **239x** | 367MB → 1MB |
| `doctor` | 365.8ms | 1.7ms | **220x** | 367MB → 1MB |
| `plugins list` | 371.6ms | 1.5ms | **250x** | 371MB → 1MB |
| `skills list` | 375.9ms | 2.0ms | **187x** | 371MB → 2MB |
| app-server session (warm pool) | 243ms | **5ms** | **~50x** | — |

## Install (one line)

```sh
curl -fsSL https://raw.githubusercontent.com/Solorush2021/zcode-rust/main/install.sh | sh
export PATH="$HOME/.zcode/rust-bin/bin:$PATH"
zcode --version
```

macOS arm64/x64 · Linux arm64/x64 · Windows (Git-Bash/MSYS). Installs the Rust binary + the Node sidecar (agent/LLM runtime) to `~/.zcode/rust-bin`. Existing `~/.zcode` config, plugins, skills, and sessions are used as-is.

## What runs native vs. fallback

```
┌───────────────────── zcode (Rust, 674KB) ─────────────────────┐
│  NATIVE (~1–2ms): version · help · doctor · commands ·        │
│  skills · plugins · hooks trust · __internal-search · zh-CN   │
│  locale · arg parsing (exact parseArgs semantics) · warm      │
│  app-server pool (daemon + unix socket)                       │
│                                                               │
│  FALLBACK (exec node sidecar, byte-passthrough):              │
│  prompt · tui · app/agent-server cold · login · dwf ·         │
│  plugin install/marketplace · agent runtime                   │
└───────────────────────────────────────────────────────────────┘
```

Plus Node-side wins shipped in the sidecar: agent dispatch **−34%** (V8 compile cache + lazy-loading a 9MB `typescript.js` chain → bundle 31→21MB), MCP pools no longer re-spawn per session, session writes batched.

## Verification

```sh
git clone https://github.com/Solorush2021/zcode-rust && cd zcode-rust/zcode-rs
cargo test --release parity     # replays all golden matrices vs the TS build
./bench/all.sh                  # build + 6 parity gates + benchmarks
```

216 golden cases across 6 matrices (argv adversarial matrix, error paths, zh-CN locale, hooks trust, internal-search incl. vendored-ugrep mode) — every one byte-identical in stdout, stderr, and exit code. See [`PORT_LOG.md`](PORT_LOG.md) for the 31-iteration engineering ledger and [`LESSONS-RUST-PORT.md`](LESSONS-RUST-PORT.md) for what we learned.

## Repo layout

| path | what |
|---|---|
| [`zcode-rs/`](zcode-rs/) | the Rust crate, packaging scripts, docs |
| [`bench/`](bench/) | golden-harness (parity.mjs + 6 matrices + benchmarks) |
| [`analysis/`](analysis/) | bottleneck research reports (T1–T8) |
| [`PORT_REPORT.md`](PORT_REPORT.md) · [`PORT_LOG.md`](PORT_LOG.md) | final report · append-only ledger |
| [`OPTIMIZATION_CONTEXT.md`](OPTIMIZATION_CONTEXT.md) | multi-agent campaign coordination |

## License & attribution

Apache-2.0. Derived work based on [zai-org/ZCode](https://github.com/zai-org/ZCode) (Apache-2.0) — huge thanks to the Z.AI team for open-sourcing it. Node sidecar (`zcode.cjs`) is built from that source and remains theirs.
