# T2 — `__internal-search` + Grep/Glob tool handlers (READ-ONLY analysis)

Agent: T2-search-agent (GLM-5.3-Flash) · 2026-09-22 · Repo: `~/Documents/zai/zcode-port/ZCode` @ apps/zcode-cli

---

## 0. Headline answer to the orchestrator's key question

**Neither of the two hypotheses is the default path.** Measured wiring:

- **Grep tool** (`packages/core/src/tool/handlers/grep.ts`) never spawns rg and never calls `__internal-search`. It calls `FileSystemPort.searchText()` → `searchTextWithRipgrep()` → `runBundledRipgrep()` = **ripgrep compiled to WASI WASM** (`ripgrep@0.3.1`, 716KB `_rg.wasm.mjs`), run in a **fresh `worker_threads` Worker per call** (`adapters/src/fs/index.ts:1047-1131`), with pure-JS fallback on `RipgrepRuntimeFailure`.
- **Glob tool** (`handlers/glob.ts`) = pure-JS recursive `walkFiles()` + mtime sort. No external process at all.
- **Both tools are normally UNREGISTERED**: `core/src/embedded-search/capability.ts` hardcodes `ENABLE_EMBEDDED_SEARCH_BRANCH = true`; when Bash is available, `refreshBranchAwareBuiltInTools()` (`runtime/methods/embedded-search-branch.ts:21-46`) unregisters Grep and Glob and the model is funneled to **Bash `grep`/`find`/`rg`**, which a shell-function prelude rewrites to the embedded-search backend.
- The prelude backend is `native-binaries` by default (`bootstrap/src/app/embedded-search-backend.ts:20-28`): direct `command bfs/ugrep/rg` spawn — **zero Node, zero `__internal-search`** on a standard install (SEA/desktop extract vendored binaries and set `ZCODE_BFS_BINARY`/`ZCODE_UGREP_BINARY`/`ZCODE_RG_BINARY`; on this machine: `/Applications/ZCode.app/Contents/Resources/tools/{bfs,ugrep}` + homebrew rg).
- **`zcode __internal-search find|grep ...` (the internal-cli backend) is only used when `ZCODE_EMBEDDED_SEARCH_COMMAND` is set** — an operator/deployment override that is *read* in exactly one file and *set nowhere* in the repo (remote/managed sessions with no vendored tools). `argv0-dispatch` is typed in contracts and handled in the prelude but constructed nowhere in-repo (reserved for external launchers).

**So the "~380ms Node boot per Grep tool call" hypothesis is FALSE for default installs** — there is no per-search Node process on the happy path. It is TRUE (372ms measured) only for internal-cli deployments on every Bash grep/find. Under our Rust binary that path costs 372ms instead of 361ms (Rust exec wrapper ≈ +11ms, inside noise) — no regression, but also no win until `__internal-search` is ported natively.

---

## 1. Exact argv grammar of `__internal-search` (for a byte-identical port)

Entry: `run.ts:287-294` — `argv[0] === "__internal-search"` → `runEmbeddedSearchCli(argv.slice(1), {cwd, stdin, stdout, stderr})` (file `cli/src/internal-search/embedded-search-cli.ts`, 338 ln).

```
zcode __internal-search <cmd> [args...]
  cmd := "find" | "grep"        (anything else: stderr "unsupported embedded search command: <cmd>\n", exit 2)
```

- **`find`**: passthrough — `spawn("find", args)` verbatim, cwd = io.cwd.
- **`grep`**: unless any arg matches a bypass pattern, prepend exactly:
  `["-G","-I","--exclude-dir=.git","--exclude-dir=.svn","--exclude-dir=.hg","--exclude-dir=.bzr","--exclude-dir=.jj","--exclude-dir=.sl"]`
  (bypass = pass args unmodified; patterns, `embedded-search-cli.ts:23-37`):
  `/^-.*-filter.*$/`, `/^-.*-pager.*$/`, `/^-.*-view.*$/`, `/^-.*-format-open.*$/`, `/^-.*-config.*$/`, `/^---.*$/`, `/^-@.*$/`, `/^-.*-save-config.*$/`, `/^-[Zz].*$/`, `/^-[^-].*[Zz].*$/`, `/^--null$/`, `/^--null-data$/`
  (the `-Z`/`--null` family diverges between ugrep and GNU grep and must fall back to system grep).
- **Output format: none** — raw byte passthrough of child stdout/stderr (no JSON envelope, no framing). Exit code = child exit code. Stdin piped through if provided, else closed.
- **Process semantics to replicate:**
  - spawn error ENOENT → stderr `failed to run embedded grep: <msg>\n`, exit **127**; other spawn errors → exit **1**.
  - stdout write error in {EPIPE, EIO, ENXIO, EBADF} → treat as downstream-closed (`grep … | head`): kill child, settle exit **0**, swallow.
  - SIGTERM → 750ms later SIGKILL (only the direct child; child stays in caller's process group).
  - Abort/parent-signal exit codes: SIGHUP **129**, SIGINT **130**, SIGTERM **143**, default abort **130**.
  - `windowsHide: true`; win32 kill = `taskkill /pid <pid> /T /F`.
  - Listens process-globally for SIGINT/SIGTERM/SIGHUP during the call and aborts the child.

Call sites of the string `__internal-search` in-repo (all 3): `cli/src/run.ts:287` (dispatch), `bootstrap/src/app/embedded-search-backend.ts:5` (backend `args`), and generated shell prelude text. **Frequency: 1 process per Bash-tool grep/find on internal-cli deployments** (every compound command can spawn several). The prelude itself is injected into *every* POSIX Bash call (`bash.ts:394-405` → `buildEmbeddedSearchPreludeContent`), defining `grep()`, `find()`, and a conditional `rg()` fallback:

```sh
grep() { …bypass check…; command <zcode> __internal-search grep "$@"; }   # internal-cli
grep() { command <ugrep-path> -G --ignore-files --hidden -I --exclude-dir=… "$@"; }  # native-binaries (default)
find() { command <bfs-path> -S dfs -regextype findutils-default "$@"; }
```

Native-binaries defaults: `bfs -S dfs -regextype findutils-default`, `ugrep -G --ignore-files --hidden -I --exclude-dir=<6 VCS dirs>`, `rg` passthrough (only wrapped when PATH lacks rg).

---

## 2. Grep-tool WASM engine details (the registered-tools path)

- `createRipgrepSearchPlan` (`fs/index.ts:924-979`) builds rg args: `--no-config --hidden --color never --no-heading --with-filename --max-columns 500` + `--glob !<vcs> --glob !**/<vcs>/**` ×6 + mode flags (`--json` for content / `-c` for count+files modes) + `-e <pattern> -- <target>`. **No node_modules exclusion — but WASM rg honors `.gitignore` at runtime**, so on real repos it skips vendored dirs (measured below).
- `runBundledRipgrepWorker` spawns a **new Worker per search** (worker source is an inline string that `await import("ripgrep")` and runs WASI with `buffer:true, env:{}, nodeWasi:false, preopens:{".":searchRoot}`); main thread never blocks; abort = `worker.terminate()`; hard timeout `DEFAULT_RIPGREP_TIMEOUT_MS` (test-overridable); any non-abort failure → `RipgrepRuntimeFailure` → pure-JS `searchTextWithJavaScript` fallback (line 576+).
- `textSearchEngine: "javascript"` adapter option forces the JS engine (single-threaded, reads every candidate file).

---

## 3. Measurements (macOS 25.5.0 arm64; hyperfine ≥12 runs, warmup ≥2; peak RSS `/usr/bin/time -l`)

Query (model-realistic, run in `ZCode/`): `grep -rln resolveDefaultEmbeddedSearchBackend --include='*.ts' .`

| # | Path | mean | σ | min…max | peak RSS |
|---|---|---|---|---|---|
| 1 | **Rust binary** `zcode __internal-search grep …` (fallback→node) | **3.142 s** | 0.111 | 3.00–3.27 s | **379.7 MB** |
| 2 | **Node bundle direct** `node dist/zcode.cjs __internal-search grep …` | **3.124 s** | 0.108 | 3.00–3.28 s | **380.5 MB** |
| 3 | System `/usr/bin/grep` with exact GREP_DEFAULT_ARGS (what the handler spawns) | **2.697 s** | 0.075 | 2.64–2.93 s | **5.8 MB** |
| 4 | **Vendored ugrep** (native-binaries default backend) | **35.8 ms** | 0.6 | 34.8–38.8 ms | **7.9 MB** |
| 5 | `rg` with `--no-ignore-vcs` (worst-case vendored-rg arg mix) | 663.8 ms | 50.6 | 628–814 ms | 16.3 MB |
| 6 | `rg` natural (gitignore-aware; what models actually type) | **51.4 ms** | 1.1 | 48.8–54.0 ms | 16.3 MB |

Boot-cost isolation (`__internal-search grep --version` — same Node boot, no search):

| Path | mean | σ |
|---|---|---|
| Rust binary wrapper → node | **372.2 ms** | 37.1 |
| Node bundle direct | **361.5 ms** | 7.2 |

**Deltas:**
- Node bundle boot = **~361 ms** per `__internal-search` call; the Rust exec wrapper adds **~11 ms** (σ-overlapping → ≈ free).
- 3.142 vs 2.697 s ⇒ Node+wrapper overhead on a real query = **~445 ms** (= 361 ms boot + ~11 ms + output-piping through Node streams).
- WASM Grep-tool engine (isolated probe replicating the worker: same args, `preopens` = repo root): **479 ms cold** (WASM compile) / **209 ms warm** per search, vs **51 ms** native rg on the identical tree → WASM path is **~4× slower** than a native binary, plus worker spawn + main-thread message marshalling. Output check: rows 1–3 byte-identical stdout.

### Per-100-calls session impact (grep-shaped Bash calls)

| Deployment | Today (Rust binary) | Native Rust `__internal-search` | Saving /100 calls |
|---|---|---|---|
| internal-cli (`ZCODE_EMBEDDED_SEARCH_COMMAND` set) | 100 × (372 ms boot + 380 MB peak + child search 36 ms…2.7 s) | 100 × (~3 ms + child) | **~37 s wall, −370 MB peak per call** |
| Default native-binaries (local desktop/CLI) | 0 Node boots — direct ugrep/bfs/rg | no change | 0 |
| Grep tool registered (Bash unavailable; rare) | 100 × 209 ms warm WASM (+479 ms first) | 100 × ~50 ms | **~16 s** |

Note the hidden second-order cost in internal-cli mode: the handler spawns *system* grep, which walks `node_modules` (only VCS dirs excluded → 2.7 s child on this repo) — the child dominates, not the boot. ugrep's `--ignore-files` (native-binaries default) is 75× faster on the same repo.

---

## 4. Root cause summary

- T2's assumed "per-search Node fallback" exists **only** behind `ZCODE_EMBEDDED_SEARCH_COMMAND`; default installs already avoid Node per search by design (shell prelude → vendored native binaries; Grep tool → in-process WASM). The real Node-boot tax in an agent session is paid by the **agent runtime itself** (T1/T8), not by searches.
- Where the fallback *is* deployed (remote/managed internal-cli), every model grep pays 361 ms Node boot + 380 MB RSS before a child grep even starts, and the child itself is un-bounded system `grep -r` (no gitignore).

## 5. Ranked fix proposals

1. **Port `__internal-search` natively into the Rust binary (spawn, not reimplement).** ~150 lines: argv grammar per §1, `std::process::Command` for system `grep`/`find` with the same prepend/bypass logic, EPIPE→0, signal exit codes, SIGTERM→SIGKILL ladder. Removes the 361 ms boot + 380 MB RSS **per Bash grep/find on internal-cli deployments** (≈37 s/100 calls) with byte-identical I/O semantics; zero effect on default installs (dead path there). Cheapest, safest, unblocks removing `__internal-search` from the fallback list in `zcode-rs/crates/zcode-cli/src/run.rs:53`.
2. **Same native implementation should prefer vendored tools when present** (honor `ZCODE_UGREP_BINARY`/`ZCODE_BFS_BINARY`, like `resolveDefaultEmbeddedSearchBackend` does) so internal-cli mode gets ugrep-class speed (36 ms) instead of system-grep-with-node_modules walks (2.7 s) — 75× on the child itself. Careful: ugrep/bfs flag defaults differ from system grep, so keep system grep as the strict byte-compat default and only upgrade when the deployment env names the binary (mirrors the existing prelude semantics).
3. **Library-embed (`ignore` + `regex`/`grep-searcher` crates) is NOT recommended for `__internal-search`** — traversal order/regex dialect (BRE `-G`) differences make byte-identical output risky, and the process-spawn version already captures 99% of the win. Reserve crates for a *future* native Grep-tool engine (item 4).
4. **Grep tool WASM worker (rare path):** if/when the Node agent runtime shrinks (T8), replace per-call Worker+WASI ripgrep (209 ms warm/479 ms cold, 4× slower than native) either with the vendored `rg` binary (51 ms, spawn once per call) or a Rust-side search service. Only matters when Bash is unavailable, so schedule behind T1/T8.
5. **Do not touch the default happy path** — it is already optimal (vendored ugrep 36 ms, 7.9 MB, no Node). Any "optimization" there would be regression risk for zero gain.

### Verification hooks for batch-2
- Parity: golden `__internal-search` cases (grep defaults, bypass flags, `unsupported` exit 2, ENOENT 127) can be added to `bench/parity.mjs`; I/O must stay byte-identical (rows 1–3 identical today).
- Gates: `node bench/parity.mjs zcode-rs/target/release/zcode` (111/111) + golden-args2 (37/37) after any run.rs change.

## 6. Files touched (read-only) / evidence index

- `ZCode/apps/zcode-cli/packages/cli/src/internal-search/embedded-search-cli.ts` (grammar, bypass, exit codes)
- `ZCode/apps/zcode-cli/packages/cli/src/run.ts:287` (dispatch, pre-parseArgs)
- `ZCode/apps/zcode-cli/packages/bootstrap/src/app/embedded-search-backend.ts` (backend selection; env var)
- `ZCode/apps/zcode-cli/packages/core/src/embedded-search/{capability,shell}.ts` (branch + prelude hardcoded ON)
- `ZCode/apps/zcode-cli/packages/core/src/runtime/methods/embedded-search-branch.ts` (Grep/Glob unregister)
- `ZCode/apps/zcode-cli/packages/core/src/tool/handlers/{grep,glob,bash}.ts` (tool handlers, prelude attach)
- `ZCode/apps/zcode-cli/packages/core/src/tool/executor/call-runner.ts:401`
- `ZCode/apps/zcode-cli/packages/adapters/src/fs/index.ts` (WASM rg worker, JS fallback, search plan)
- `ZCode/apps/zcode-cli/packages/adapters/src/exec/embedded-search-prelude.ts` (shell rewrite; 3 backend kinds)
- `ZCode/apps/zcode-cli/packages/cli/src/sea-runtime-tools.ts`, `packages/shared/src/runtime-tool-runtime.ts` (vendored tool extraction/env)
- `ZCode/apps/zcode-cli/packages/cli/scripts/*native-search-tools*.mjs`, `ZCode/third-party/native-search` (vendoring pipeline)
- `zcode-rs/crates/zcode-cli/src/run.rs:53`, `fallback.rs` (current fallback + node exec ≈11 ms)
- Probes: `/tmp/t2-wasm-probe.mjs` (WASM engine timing); hyperfine at `~/.cargo/bin` under port dir.
