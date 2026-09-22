# T5 / T6 / T7 runtime analysis — edit matchers, session storage, agent-startup discovery

Batch-1 read-only analysis. All measurements on this M-series Mac (APFS, warm cache), Node v26.8.1, zcode.cjs 31,060,962 B. Date: 2026-09-22. Bench scripts in `/tmp/t567-bench/`.

---

## T5 — Edit/diff matchers (`packages/core/src/tool/edit-matchers.ts` 411 ln, `diff.ts` 47 ln)

### Algorithms, per Edit tool call (handler: `packages/core/src/tool/handlers/edit.ts`)

Exactly **1 x `findEditMatch`** per Edit call + **1 x `createStructuredPatch`** (jsdiff `structuredPatch`, `diff@^9.0.0`, Myers O(ND), context=3, timeout=5s) per successful edit. `write.ts` also calls `createStructuredPatch`.

Strategy cascade in `findEditMatch` (first hit wins; not-found runs ALL 7):

| strategy | algorithm | complexity |
|---|---|---|
| exact / escape / unicode-escape / line-number-stripped | `String.indexOf` loop (native memmem) | ~O(n+m) |
| quote_normalized | normalize both strings (4 replaceAll) then indexOf | O(n) + scan |
| line_trimmed | split lines, sliding window over every L−S+1 block, per-line `.trim()` compare | O(L·S·len) |
| indentation_flexible | sliding window + `removeCommonIndent` (regex `^[\t ]*` per line) per block | O(L·S·len), regex per line |
| block_anchor | sliding window + **Levenshtein** per middle line (threshold 0.8) | O(L·S·maxLen²) worst |

Input sizes: content = whole file (cap 1 GB, realistically 1–500 KB / 40–20k lines); search = old_string (1–50 lines).

### Regex backtracking
5 regexes total: `/^\d+: (.*)$/`, `/^\d+\t(.*)$/`, `/\\([ntr"'`\\$])/g`, `/(\\\\)|\\u([0-9a-fA-F]{4})/g`, `/\p{L}/u`, `^[\t ]*`. All anchored or single-char classes, **no nested quantifiers → no catastrophic backtracking**.

### Measured (`/tmp/t567-bench/bench.mjs`, exact code imported via node type-stripping)

| case | ms/op |
|---|---|
| exact hit, 5-line snippet in 2k-line/60KB file | **0.012** |
| exact hit, 10k-line file | 0.031 |
| replaceAll=true | 0.012 |
| line_trimmed fallback hit (2k lines) | 0.851 |
| block_anchor hit (levenshtein, 2k lines) | 2.155 |
| NOT FOUND → full 7-strategy fallback, 2k lines | 2.05 |
| NOT FOUND full fallback, 10k lines | 10.3 |
| jsdiff structuredPatch, 1-line change in 2k lines | 0.451 |
| jsdiff structuredPatch, 2k-line **reversed** file (pathological) | **322** (5 s cap) |

Frequency: once per Edit call — a heavy session does tens of Edits; each sits between an fs read+write (~1–10 ms) and a multi-second LLM round-trip.

### Verdict: NOT a NAPI-RS candidate
Gain = call frequency (tens/session) x per-call cost (0.01–2 ms typical, 10 ms worst) → **<0.1% of Edit wall time, unmeasurable end-to-end**. Same conclusion as oxc-pattern ports generally: port only when the same function is hit per-token, not per-tool-call.
Two cheap JS-only fixes if we want them:
1. Not-found path: prefilter `block_anchor`/`line_trimmed` with `content.indexOf(firstTrimmedLine)` before the O(L·S) window scan — kills the 2–10 ms not-found tail.
2. jsdiff pathological tail (322 ms): trim common prefix/suffix lines before `structuredPatch` (typical edits are local; reversed-file case is rare but real after "reorganize file" prompts).

---

## T6 — Session storage (`packages/adapters/src/storage/`, `session-store/` 2,659 ln)

### Engine
**`node:sqlite` `DatabaseSync`** (built-in, synchronous) — NOT better-sqlite3, NOT JSONL. DB at `getDefaultSessionDbPath()`; opened with `busy_timeout` 5 s.
Migration runner sets: `foreign_keys = on`, `journal_mode = wal` (enforced, retried 10→200 ms backoff). **`synchronous` is never set → SQLite default `FULL`** → every commit fsyncs the WAL.

### Write path anatomy (repositories/messages.ts, sessions.ts)
- `savePart` = 1 upsert (with `select coalesce(max(sequence),-1)+1` correlated subquery — indexed by `part_message_id_id_idx`, OK) **+** `touchSession` UPDATE.
- Both run in **autocommit → 2 separate transactions → 2 WAL fsyncs per part save**. Same for `saveMessage`, `saveSessionEntry`, dwf journal inserts.
- Batching exists ONLY in fork/shared-context bundles (`begin immediate` ... `commit`), which is the right pattern already in-tree.
- Frequency: per part-state change + per message during a turn ≈ 10–50 writes/turn; dwf journal can emit hundreds per workflow run. All synchronous → event-loop blocking 0.5 ms each.

### Measured (`/tmp/t567-bench/t6.mjs`, replica schema, 10,448-byte JSON part row)

| mode | ms/event |
|---|---|
| as-shipped: WAL + FULL, 2 autocommits | **0.528** |
| WAL + `synchronous = normal` | 0.479 |
| WAL + FULL, both statements in 1 explicit txn | **0.073** (7.2x) |

APFS fsync is cheap (~0.2 ms/commit), so FULL≈NORMAL here; on Linux ext4 the FULL-per-commit penalty is typically 1–10 ms — the fix matters more there.

### Fixes ranked
1. `pragma synchronous = normal` after WAL (1 line in migration-runner; standard WAL practice, loses only latest commits on OS crash). Gain: 7x on batched path, large on Linux.
2. Wrap per-turn persist bursts in one `begin immediate` txn (pattern exists in `commitForkBundle`; needs a small txn-scoped API on the store). Gain: ~0.45 ms per write saved → ~20–25 ms per 50-write turn today, likely 0.5–5 s/turn on spinning/ext4 disks.
3. Later: rusqlite via NAPI-RS only if Node stays the runtime — the JS side is not the bottleneck, the fsync pattern is.

---

## T7 — Config/skill/plugin discovery inside the agent runtime (`packages/bootstrap/src`, 63k ln)

### What the fail-fast agent startup actually does
Measured with `node dist/zcode.cjs --prompt "hi"` (dies at model creation, no API key) — **wall 587 ms**, CPU-profiled (`/tmp/t567-bench/prof`) + fs-call counted via `NODE_OPTIONS=--require` preload:

| phase | cost |
|---|---|
| CJS bundle compile (`wrapSafe`) | **179 ms** |
| bundle init/require chain | ~100 ms (T8 floor, total ~300–360 ms) |
| discovery+config fs primitives (self CPU) | **~15 ms** (readFileUtf8 10.6, readdir 1.5, zod parse 7.9, createConfig 1.1) |
| model-creation failure | rest |

**fs syscall count per agent startup: ~430** — existsSync 219, stat+statSync 89, readFileSync+readFile 66, readdir+readdirSync 26, lstat 16, realpathSync 4.

### Scan roots enumerated from code
- **Skills** (`adapters/src/skills/roots.ts`): walk cwd→git root (`stat(.git)` per level); per base dir (home + every project dir): `~/.zcode/skills`, `~/.agents/skills` (+project equivalents); per root: stat root, stat SKILL.md, readdir, stat per subdir candidate (scan.ts: 2–3 stats/candidate). This machine: 17 skills found natively.
- **Plugins** (`bootstrap/plugins.ts` → `adapters/plugins/` 7,019 ln): `discoverNodePluginsSync` over plugin cache (`~/.zcode/cli/plugins/cache`, 13 plugins here) + marketplace manifests (`loadKnownMarketplacesSync`, `ensureDefaultPluginMarketplaces`), all **synchronous** fs.
- **Config chain** (`adapters/config/`): user config.json, project config, settings merge + zod validation.
- Plus custom commands, session DB open + migrations (T6).

### vs Rust CLI
`zcode-rs` does skills/plugins/commands discovery natively in **~2 ms total wall** (measured: `/usr/bin/time` real 0.00 s; orchestrator's 187–252x headline covers it).

### Estimated savings if agent runtime delegated discovery to Rust
~15–20 ms of 587 ms ≈ **3% of agent startup** — and the JS side would still JSON.parse + zod-validate results unless the embedded lib returns typed structs (then up to ~25 ms incl. zod). **Not worth doing standalone.** Real leverage is combined with T1: every dwf/subagent spawn repeats the full ~590 ms (bundle floor 360 ms + discovery 15–20 ms), so discovery only pays off once the per-spawn bundle floor (T8) is fixed. Priority: strictly below T8 and T1.

---

## Ranked fixes (all three targets)

| # | fix | target | effort | est. gain |
|---|---|---|---|---|
| 1 | `pragma synchronous = normal` | T6 | 1 line | 7x on write path w/ batching; ~20–25 ms/turn here, seconds/turn possible on Linux |
| 2 | Batch per-turn persists in one txn (reuse `begin immediate` pattern) | T6 | small refactor | 7.2x measured on replica (0.528→0.073 ms/event) |
| 3 | Not-found early-exit prefilter + common prefix/suffix trim before jsdiff | T5 | ~10 lines JS | kills rare 2–10 ms and 322 ms tails; no steady-state gain |
| 4 | Rust discovery for agent runtime | T7 | medium | ~15–20 ms per agent/subagent start (~3%) — only bundle with T1/T8 work |

**Explicit recommendation: do NOT port T5 to NAPI-RS** — measured 0.012 ms typical / 10 ms absolute worst per Edit call makes the gain unmeasurable; the oxc-pattern applies only to per-token workloads.

Bench artifacts: `/tmp/t567-bench/{bench.mjs,t6.mjs,countfs.cjs,profsum.mjs,prof/*.cpuprofile,fscounts.txt}`.
