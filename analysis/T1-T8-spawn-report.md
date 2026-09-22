# T1 + T8 — Subagent spawn path & bundle load cost (READ-ONLY analysis)

Agent: T1/T8 analyst · 2026-09-22 · Node v26.8.1 arm64, macOS darwin 25.5.0
Binary measured: `node /Users/vipulkumar/Documents/zai/zcode-port/ZCode/apps/zcode-cli/packages/cli/dist/zcode.cjs` (31,060,962 bytes, unminified CJS + sourcemap).
All scratch builds/artifacts in /tmp only (`/tmp/t8-scratch`, `/tmp/cc`). No source edited.

---

## 1. HEADLINE NUMBERS (hyperfine n≥12, /usr/bin/time -l peak RSS)

| scenario | wall (mean ± σ) | peak RSS | delta vs baseline |
|---|---|---|---|
| bare `node -e ""` | 19.2 ± 0.7 ms | — | floor of any spawn |
| `node --check dist/zcode.cjs` (parse-only floor) | 163.5 ± 1.7 ms | — | parse alone = 45% |
| **baseline** `node dist/zcode.cjs --version` | **359.9 ± 10.5 ms** (solo n=10; 381.5 ± 9.4 in 3-way) | **381,288,448 B (381 MB)** | — |
| `NODE_COMPILE_CACHE=/tmp/cc` warm (n=12) | **240.5 ± 7.0 ms** | 389,709,824 B | **−119 ms (−33%)**, RSS unchanged |
| NODE_COMPILE_CACHE cold (cache-miss write, n=3) | 0.35–0.37 s | — | ~free (lazy write) |
| scratch minified bundle +keepNames (14.0 MB) | 435.4 ± 4.7 ms | — | **WORSE +55 ms** |
| scratch minified bundle, no keepNames (13.2 MB) | 408.5 ± 4.5 ms | 343,703,552 B | **WORSE +27 ms**, RSS −37 MB |
| scratch plain rebuild (27.7 MB, no banner) | 376.1 ± 6.4 ms | — | ≈ baseline |
| **real agent path:** `zcode -p hi` (fails fast at Model creation, exit 1) | **639 ms** cold / **548 ms** warm-cache | — | −91 ms with cache |

Scratch builds used the repo's own `scripts/build.mjs` exports (same esbuild opts, same zod-dedupe plugin), outfile redirected to /tmp. Official dist is NOT minified (readable identifiers verified); `scripts/build.mjs:130` `minify: desktopAgent && !e2eCoverage`, default `minify = false` (:207).

## 2. T8b — WHERE THE 360 ms GOES (--cpu-prof, 500 µs interval, total profiled 374.9 ms)

| bucket | self time | % |
|---|---|---|
| `wrapSafe` (CJS compile of the bundle, `cjs/loader:1823`) | 155.2 ms | 41.4% |
| esbuild module top-level exec (`__init*` wrappers) | 59.0 ms | 15.7% |
| esbuild `__require` plumbing | 15.8 ms | 4.2% |
| GC | 16.1 ms | 4.3% |
| `readFileUtf8` (reading the 31 MB file) | 9.8 ms | 2.6% |
| everything else (i18n `resolveIntlLocale2` 7.5 ms, `setCliProcessTitle` 5.3 ms, bash-command-registry init 3.8 ms, …) | ~119 ms | ~32% |
| node process baseline | ~19 ms | 5% |

Cross-check: parse-only floor (`node --check`) 163.5 ms ≈ `wrapSafe` 155 ms → **~45% of startup is V8 compiling one 31 MB file; ~16% is executing 2,500 module initializers; no single JS module dominates execution**. Cache 6.6 MB on disk.

## 3. T1 — SUBAGENT/TASK SPAWN TRACE (correction to the T1 hypothesis)

**Subagents are IN-PROCESS. No fresh `node dist/zcode.cjs`, no `child_process.fork`, no tsx.** The 360 ms bundle load is paid ONCE per top-level CLI dispatch; an N-agent DWF fan-out does NOT multiply it.

Trace:
- Entry: `packages/cli/src/main.ts:14` `void main()` → `main.ts:77` `await import("./run.js")`.
- Dispatch: `packages/cli/src/run.ts:487,503` → `runPrompt` (prompt mode, headless `-p`).
- Bootstrap: `packages/cli/src/prompt-command.ts:166` `await loadBootstrapModule()` → `packages/cli/src/bootstrap-loader.ts:6` `import("@zcode/bootstrap")` (the 63k-line package, in-bundle; memoized).
- Task = Agent alias: `packages/core/src/tool/handlers/agent.ts:287-300` (`taskToolEntry` spreads `agentToolEntry`, name = `TASK_TOOL_NAME`).
- Handler: `agent.ts:174-222` → `context.subagentPort.launch(...)` (`agent.ts:211`).
- Port wiring: `packages/core/src/runtime/agent-runtime.ts:291` `this.subagentPort = deps.subagentPort ?? runtime.createDefaultSubagentPort(deps)`.
- Port impl: `packages/core/src/subagent/runner.ts:131` `createExploreSubagentPort` → foreground launch awaits `runner.ts:1131` `options.runExploreAgent(...)`.
- **The spawn**: `packages/core/src/runtime/methods/subagent.ts:93` `runExploreAgent:` → `subagent.ts:239` `const childRuntime = new AgentRuntime(...)` **in the same process**, then `childRuntime.executeTurn(request.prompt, …)` (:~410). Child gets its own session id, model factory, tool allowlist, MCP access, and mirrored events — but zero new processes.

The only real child process in the agent path is the **DWF sandbox, one per WORKFLOW RUN (not per agent ask)**:
- `packages/dynamic-workflow-runtime/src/harness.ts:238-253` `spawn(process.execPath, ["--max-old-space-size=256", <entry.mjs>])`.
- The child loads a **self-contained ~entry ESM file** written to `.zcode/workflow-runs/<runId>.mjs` (`child-source.ts`) — it does NOT import the 31 MB bundle; `agent()` asks round-trip over NDJSON stdio to the parent (in-process subagent runtime). Cost ≈ bare node 19 ms + file.
- The full-bundle re-exec variant (`argsPrefix: [__zcode-dwf-child]`, `packages/bootstrap/src/app/dynamic-workflow-run-launch.ts:355`) fires **only when `isSea`** — i.e. the desktop SEA binary. Under plain node AND under the Rust fallback (`node dist/zcode.cjs`), the DWF child stays cheap. `dwf-child-command.ts:44` is the SEA-side receiver only.

Where the ×N multiplication ACTUALLY is: every Rust→node fallback dispatch (prompt, hooks per hook-event, `__internal-search` per query — T2, `plugin-host`, `dwf-child`) re-pays 360 ms + 381 MB. Per-AGENT that's ~0; per-DISPATCH it's ~100%.

Minimal real agent invocation floor: `node dist/zcode.cjs -p hi` reaches Model creation and fails (no network/creds configured) in **639 ms** — i.e. ~360 ms bundle + ~280 ms bootstrap/config/provider-registry/model-factory teardown. That ~280 ms non-bundle boot is the next target after the bundle itself.

## 4. T8 IMPORT-CHAIN WEIGHT (esbuild metafile, scratch build in /tmp, 2,500 inputs → 29.1 MB output)

Top packages by bytes-in-output:

| bytes | % | pkg |
|---|---|---|
| 10.35 MB | 35.6% | `[workspace]/dynamic-workflow` (87 files) — **includes `typescript/lib/typescript.js` 8.9 MB (30.6% of the whole bundle!)** + `compiler/libs.generated.js` 472 KB + meriyah 320 KB + esprima 277 KB + escodegen 95 KB |
| 5.34 MB | 18.4% | `[workspace]/core` (488 files) — incl. generated `bash-command-registry.js` **2.07 MB** |
| 8.32 MB | 28.6% | node_modules: ai SDK 435 KB, MCP client trio ~800 KB, @opentelemetry/otlp-transformer 838 KB + semantic-conventions 139 KB, @ai-sdk/anthropic 216 KB + openai 247 KB, **zod v3 types ×2 copies (128 KB each, dedupe plugin misses zod/v3 subpaths)**, image stack (@jimp, image-q ×2, utif2) |
| 2.33 MB | 8.0% | `[workspace]/bootstrap` (protocol v4 gateway/projection) |
| 1.94 MB | 6.7% | `[workspace]/adapters` |
| 1.43 MB | 4.9% | `[workspace]/shared` (zcode-protocol 140 KB) |

Single biggest chain: **the DWF workflow compiler statically embeds the entire TypeScript compiler (~30% of bundle bytes)**, pulled in via `packages/dynamic-workflow` into every process including `--version`, hooks, and every search dispatch.

## 5. FIXES RANKED BY (GAIN × CHEAPNESS)

1. **Bake `NODE_COMPILE_CACHE` into the Rust fallback exec** — measured **−119 ms (−33%), 360→240 ms** per dispatch, zero RSS change, cache auto-invalidated by V8 (hash-keyed, 6.6 MB, e.g. under `~/.cache/zcode/compile-cache`). Rust side: set env var before exec'ing node in fallback; optionally pre-warm once at install. Cheapest win, applies to EVERY fallback dispatch incl. hooks/search (multiplies across a session). Risk: near-zero. [T8a, measured]
2. **Lazy-load the DWF workflow compiler chain (typescript.js 8.9 MB + parser libs)** out of the static graph (dynamic `import()` at the CreateWorkflow/Eval compile sites). Bundle 31→~21 MB; est. **−60 to −90 ms** (≈36% of parse 155 ms + read + init) and **−120 to −150 MB RSS**. Caveat: CJS + `format:"cjs"` means esbuild can't code-split; needs the chunk kept as a separate on-disk file (SEA asset) or `external` + runtime resolution — build surgery, medium effort. [metafile-derived estimate]
3. **Do NOT minify** (negative result): minified 14 MB is **slower** (408–435 ms vs 381 ms baseline) — token count, not byte count, drives parse; keepNames adds ~27 ms more. Minify only cuts RSS −37 MB. Skip. [T8c, measured]
4. **Generated `bash-command-registry.js` (2.07 MB) → lazy require or data file**: est. −10 to −20 ms parse + heap. Cheap, contained. [estimate from metafile]
5. **V8 startup snapshot: NOT feasible today** (all tested empirically on Node 26.8.1): `--build-snapshot` on the bundle dies with `SharedArrayBuffer is not defined`; `requireForUserSnapshot` cannot load ANY user file (even a 1-line /tmp module — "Cannot find module" for an existing absolute path); node itself warns `node:async_hooks`/`node:sqlite` are unverified in snapshot builders. If ever unblocked, ceiling ≈ 360→~60-100 ms, but that's a Node-upstream wait, not our fix. [T8d, tested]
6. **Agent pool / pre-forked subagents: unnecessary** — T1 finding: subagents already run in-process; there is nothing per-agent to pre-fork. The real per-dispatch tax is the Rust fallback re-exec (fix #1) and, for long sessions, moving hooks/`__internal-search` dispatches onto a persistent `agent-server`-style warm node process (removes the remaining ~240 ms × N dispatches; architectural, do after #1/#2).

## 6. Raw artifacts
- /tmp/t8-baseline.json (hyperfine), /tmp/t8prof/*.cpuprofile, /tmp/t8-scratch/{min,plain}-meta.json, /tmp/t8-scratch/zcode-{min,min-nokn,plain}.cjs, /tmp/cc (6.6 MB compile cache), /tmp/t8-measure.mjs (bounded runner).
