# ZCode → Rust Port Report

**Date:** 2026-09-22 · **Workdir:** `~/Documents/zai/zcode-port/` · **Machine:** macOS arm64 (M-series), Node 26.8.1, Rust 1.98.1 (all toolchains installed in-folder)

## What was built

A Rust binary `zcode` (426KB) that reproduces the TypeScript CLI (`apps/zcode-cli`, 281k lines TS, 31MB esbuild bundle) **byte-identically** for every local deterministic command, and transparently `exec`s the Node bundle for agent/LLM/TUI/network paths. Dispatch table:

| path | native Rust | exec Node bundle |
|---|---|---|
| arg parsing (strict parseArgs replica incl. all error formats) | ✅ | — |
| `--version` `-v` `version` | ✅ | — |
| `--help` `-h` `help` | ✅ | — |
| `doctor` (+`--json` `--verbose`) | ✅* | — |
| `commands list/inspect` | ✅ | — |
| `skills list/inspect` (+`--json`) | ✅ | — |
| `plugins list` (+`--json`), usage errors | ✅ | — |
| plugins install/uninstall/marketplace/validate | — | ✅ passthrough |
| `-p/--prompt`, `--target`, `tui`, `app-server`, `agent-server`, `login/logout`, `hooks`, `__internal-search`, plugin-host, dwf-child | — | ✅ passthrough |
| `--locale zh-CN` / zh env locale | — | ✅ passthrough (zh copy tables) |

\* doctor deviates on 3 runtime-dependent lines by design (`node:`, `default artifact:`, `execPath:` — the Rust binary reports its true runtime); masked in the parity gate. Everything else is byte-exact.

## Verification (G2 gate)

- 148 golden cases captured from the TS build (2 adversarial argv matrices: unknown options, combined shorts, equals-forms, ambiguous-value 3-line errors, empty/unicode args, `--` terminator, duplicate options, error-order conflicts).
- Gate: `node bench/parity.mjs <bin> [goldenDir]` — byte-compares stdout+stderr+exit code per case.
- **Result: 148/148 pass.**

## Performance (G1/G3 gates, hyperfine n=20 warmup 3 + /usr/bin/time -l, 2026-09-22 01:58)

| command | node_ms | rust_ms | speedup | node_MB | rust_MB | mem reduction |
|---|---|---|---|---|---|---|
| `--version` | 369.6 | 1.5 | **252x** | 367 | 1 | **226x** |
| `--help` | 378.0 | 1.6 | **239x** | 367 | 1 | — |
| `doctor` | 365.8 | 1.7 | **220x** | 367 | 1 | **224x** |
| `commands list` | 376.1 | 1.7 | **216x** | 369 | 1 | — |
| `skills list` | 375.9 | 2.0 | **187x** | 371 | 2 | **142x** |
| `plugins list` | 371.6 | 1.5 | **250x** | 371 | 1 | **191x** |

Why: every Node invocation paid ~360ms module-load + ~330MB heap for a 31MB bundle before doing ~1ms of actual work. The Rust binary does the same work in a 426KB native process.

## How to use

```bash
# The Rust binary (drop-in: same argv, byte-identical local output)
~/Documents/zai/zcode-port/zcode-rs/target/release/zcode --version
# Agent/TUI paths exec the Node bundle transparently (override via ZCODE_NODE / ZCODE_NODE_BUNDLE).

# Gates
cd ~/Documents/zai/zcode-port
node bench/parity.mjs zcode-rs/target/release/zcode            # 111/111
node bench/parity.mjs zcode-rs/target/release/zcode bench/golden-args2  # 37/37
./bench/final.sh                                                 # benchmark table
./bench/argmatrix.zsh                                            # regenerate goldens after disk-state changes
```

## Loop ledger

See PORT_LOG.md (append-only, pre-registered gates, falsified approaches).

## Known limitations / next phases

- zh-CN locale copy tables → fallback (correct, slower) until i18n tables are ported.
- `hooks` command and `__internal-search` (spawns ripgrep) are passthrough; porting them native is mechanical next work.
- skills/plugins/commands read live disk state; goldens regenerate via `bench/argmatrix.zsh` when state changes.
- The agent runtime (210k lines: core/bootstrap/adapters) stays on Node — this is the fish-shell pattern: port the surface + leaves first, keep behavior locked, grow inward.
