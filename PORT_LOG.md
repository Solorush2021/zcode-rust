# ZCode → Rust Port Campaign Ledger (append-only)

Mission: port zcode CLI behavior to Rust, byte-identical for local deterministic commands; breakthrough = large startup/memory/CPU wins vs Node baseline, all artifacts inside zcode-port/.

## Pre-registered gates (set BEFORE work, per competition-agent-loop)
- G1 Baseline: Node CLI measured (startup ms p50/p95, peak RSS MB, CPU ms) for ≥5 commands, n≥20 runs, hyperfine. No optimization claims before this exists.
- G2 Parity: golden fixtures captured from TS build; Rust must byte-match stdout+stderr+exit code for every ported command. Any mismatch = FAIL, no partial credit.
- G3 Breakthrough bar: Rust binary ≥3x faster startup AND ≥50% less peak RSS on ≥3 commands, measured, else document root cause and close.
- G4 Loop discipline: each optimization iteration logged below (hypothesis → change → gate result → keep/revert). Max 2 iterations past a failed gate, then close with root cause.
- G5 Timebox: baseline+parity harness ≤ 1 session segment; each port phase ≤ 45 min wall before reassess.
- G6 Ablation: every claimed win must re-run the same bench command as baseline (same n, same machine state).

## Iteration ledger

| # | phase | hypothesis/change | gate | result | verdict |
|---|-------|-------------------|------|--------|---------|
| 0 | setup | clone, rustup in-folder, pnpm 10.33.2 in-folder, hyperfine | — | done | ✅ |
| 1 | research | 4 subagents: case studies (fish/Biome/Rspack/oxc), repo analysis, integration strategy, skills | — | architecture = Rust-first dispatch + Node exec fallback; fish methodology: goldens BEFORE port, leaf-first; gotcha list: float fmt, color/TTY, ICU, stream order, exit codes | ✅ |
| 2 | baseline-build | pnpm install (frozen) + turbo build CLI | build green | dist/zcode.cjs 31MB; zod pinned 4.6.5 | ✅ |
| 3 | G1 baseline | hyperfine n=20 + /usr/bin/time -l ×5 | table | version 384ms/368MB, help 397ms/367MB, doctor 376ms/368MB, skills 389ms/371MB, plugins 390ms/371MB; floor node -e 1 = 20ms/36MB | ✅ |
| 4 | G2 goldens | 113-case argv matrix + manifest.jsonl | parity.mjs gate | captured incl. error paths, ambiguous-arg 3-line errors, doctor mask rules | ✅ |
| 5 | rust core | args.rs (parseArgs replica) + run.rs dispatch + help + doctor + fallback | 111/111 parity | 4 bug-fix iterations: unbalanced-quote hint, ambiguous 3-line msg w/ short-form rule, darwin/arm64 naming, camelCase JSON | ✅ |
| 6 | first bench | native Rust vs Node | G3 BREAKTHROUGH | --version 361.9→1.1ms (326x), --help 363.7→1.2ms (328x), doctor 365.3→1.2ms (330x); RSS 368MB→1.6MB (236x) | ✅ breakthrough |
| 7 | native commands | 3 subagents porting skills/plugins/commands | parity 148/148 (both matrices) | skills native 1.9ms, plugins native 686µs, commands native; verified with ZCODE_NODE=/nonexistent | ✅ |
| 8 | hardening matrix | 37-case matrix2: equals-forms, unicode, empty values, case-folding | 37/37 | one harness labeling bug (doctor mask), zero code bugs | ✅ |
| 9 | G6 final bench | same commands, same n as baseline | FINAL.md | 187x–252x faster; RSS 367–371MB → 1–2MB (142x–226x) | ✅ breakthrough confirmed |
| 10 | perf loop | hypothesis: tune 1.5–2.0ms native commands (parallel dir scan, buffers) | diminishing returns | absolute floor is fs scan; tuning unjustified — loop closed at root cause | ✅ closed |

## Phase 2: internals optimization campaign (2026-09-22)
| # | phase | hypothesis/change | gate | result | verdict |
| 11 | strategy | language/platform decision: universal Rust core, 5 target matrix, Node stays as embedded agent runtime | — | OPTIMIZATION_CONTEXT.md [ORCHESTRATOR] section | ✅ locked |
| 12 | bottleneck ID | batch-1: 4 read-only agents T1-T8 (spawn/bundle, search, protocol/MCP, edit/storage/discovery) → analysis/*.md | reports with numbers | running | ⏳ |
| 13 | fixes | batch-2 fix agents (launched after 12) | parity 148/148 + build green + measured delta | pending | ⏳ |

## Loop conclusion (G3/G4, phase 1)
Breakthrough bar exceeded 60–80x over requirement (3x). Optimization loop closed after iteration 10: remaining latency is filesystem-scan bound (~1–2ms absolute); next gains come from porting more surface (hooks trust, __internal-search, zh-CN i18n, app-server protocol), not from tuning.

## Decisions (locked)
- D1 Architecture: single Rust binary `zcode` — deterministic commands native in Rust; agent commands `exec` the Node bundle (dist/zcode.cjs). Rationale: Node floor measured 20ms/38MB; NAPI can't remove it.
- D2 Parity gate: golden fixtures from TS build (NO_COLOR=1, env -i, TERM=dumb), byte-compare stdout/stderr/exit. Ported command list phase 1: --version, --help, doctor, commands list/inspect, skills list/inspect, plugins list.
- D3 Bench: hyperfine (n≥20, 3 warmups) + /usr/bin/time -l peak RSS (5 runs, max), same commands both sides.
- D4 fish lesson: expect mid-port dip; mmap may LOSE to buffered IO (ripgrep data); don't FFI-per-call.

## Falsified approaches (do not retry without new evidence)
- pnpm install --no-frozen-lockfile on this repo: re-resolves zod to incompatible v4 minor → @zcode/contracts build fails (ZodEffects missing). Always --frozen-lockfile.

## Phase 2 results (final, 2026-09-22 03:4x)
| 14 | FIX-A | NODE_COMPILE_CACHE in fallback.rs exec | parity green | dispatch 565→448ms (−117ms) | ✅ |
| 15 | FIX-B | storage: synchronous=normal + savePart single txn (adapters/src/storage/session-store/) | build+parity green | 0.130→0.094ms/event (1.3-1.4x APFS, more on ext4); report's 7.2x was mis-instrumented replica | ✅ |
| 16 | FIX-C | transport bounded-offset framing (defensive; O(n²) falsified at runtime), MCP stable connection keys (no same-session re-spawn), mcp/list config cache (cwd+mtime LRU 8) | build+parity green | MCP child re-spawn eliminated; mcp/list skips config+plugin walk; framing equivalence 300/300 + E2E 1001/1001 | ✅ |
| 17 | FIX-E | lazy-load typescript.js chain (esbuild lazy-CJS shim + 4 module-scope consts) | parity green + metafile proof | -p hi 560→445ms (−20.5%); bundle 31→21MB; --version RSS 381→262MB; SEA needs ts-assets collector (documented) | ✅ |
| 18 | FIX-D | native __internal-search (internal_search.rs) | golden-args3 13/13, all gates 161/161 | wrapper boot 267→2.5ms (107x); repo grep 6.5ms | ✅ |
| 19 | combined | FIX-A+E on real agent dispatch | hyperfine | 565→372ms (−34%) through rust binary; node RSS 368→250MB | ✅ |
| 20 | golden drift | skills/plugins goldens stale (session seeded skills 17→32) | regenerate + re-verify | both sides agree on fresh state; drift protocol worked as designed | ✅ |

## Batch-3 (Rust expansion, 10 parallel agents) — final 2026-09-22
| 21 | A warm pool | daemon + unix socket, warm handoff, stale/race/env-fallback | all gates | cold 243ms → warm 5–34ms; replenish+idle-exit; 2 real bugs fixed (accept O_NONBLOCK, stdin-drain race) | ✅ |
| 22 | B hooks native | status/review + trust store read, grant/revoke fallback | args4 11/11 | 2.5ms vs 502ms (~200x) | ✅ |
| 23 | C zh-CN native | zh copy table + run_zh mirror; full detectLocale semantics; cwd-error bug found+fixed | args5 32/32 | zh --help 2.7ms vs 510.9ms (189x) | ✅ |
| 24 | D plugin-dwf + commands | enabled-plugin command roots in commands_cmd (fixes 111/111); plugin-host/dwf-child: predicates native, runtime stays fallback (verified runtime-heavy); run.rs arm names corrected (__zcode-*) | 111/111 | commands list --json 4.1ms vs ~370ms (~90x) | ✅ |
| 25 | E cross-compile | 5 targets incl. windows-gnu first try; dist-rs/ + checksums | compile+file | arm64 776K, x64 mac 931K, linux 702K/984K, win 1.0M | ✅ |
| 26 | F size | opt-level=z kept; panic=abort rejected (exit-code semantics), fat-lto rejected | gates | 875K → 674K (−23%) | ✅ |
| 27 | G V8 flags | --max-semi-space-size=16 injected when NODE_OPTIONS absent | gates+bench | agent RSS −7% (340→316MB) | ✅ |
| 28 | H ugrep pref | ZCODE_UGREP/BFS_BINARY vendored spawn w/ prelude args; bypass preserved; parity.mjs per-case env + ZCODE_*_BINARY strip | args6 12/12 | 13.1→8.5ms (1.54x), RSS 3.8→1.8MB | ✅ |
| 29 | I packaging | dist/zcode-rs-0.16.9 (bin+sidecar+wrapper+install.sh, tar.gz 4.1M), /tmp install verified | e2e | install/uninstall idempotent | ✅ |
| 30 | J gates | cargo test parity (auto-discovers matrices) + bench/all.sh + QUICKSTART.md | cargo test | one-command verification | ✅ |
| 31 | final merge | rebuild + regenerate drifted goldens + all 6 gates | 216/216 | golden-args 111, args2 37, args3 13, args4 11, args5 32, args6 12 | ✅ |
