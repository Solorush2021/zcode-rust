# T3 (app-server protocol + model streaming) & T4 (MCP client) — Analysis Report

Agent: protocol-mcp-analyst | Date: 2026-09-22 | READ-ONLY batch-1 analysis.
All paths relative to `~/Documents/zai/zcode-port/ZCode/apps/zcode-cli/` unless noted.

## Headline

Both T3/T4 steady-state hypotheses are **falsified or negligible**. The wire-level parse/serialize cost per token is sub-microsecond JS work. The dominant cost of the protocol path is **process startup**: ~378 ms warm + ~375 MB RSS before the app-server answers its first request — the same T8 bundle-load floor that dominates every Node invocation. MCP is already pooled with a 30 s idle grace; connections are per-session-lease, not per tool call. Remaining real (but smaller) findings: O(n²) framing buffer in the NDJSON transport, double zod validation per inbound protocol message, per-`mcp/list` full config+plugin disk rescan, and a pool-keying quirk that prevents same-session lease reuse.

## Measurements (handshake: spawn `node zcode.cjs app-server`, send `{"id":"1","method":"runtime/capabilities","params":{}}` over stdin, await response)

| metric | value | method |
|---|---|---|
| time-to-ready (warm, median of 10) | **378 ms** (371–383 warm; 827/963 ms first runs = cold FS cache) | driver `/tmp/t3-handshake.mjs`, 2×5 runs |
| spawn overhead itself | 0.3–2.3 ms (i.e. ~all 378 ms is bundle boot + runtime init) | same |
| peak RSS after handshake, idle 5 s | **392,790,016 B ≈ 375 MB** | `/usr/bin/time -l` |
| CPU to ready | 0.44 s user / 0.04 s sys for 5 s idle session | `/usr/bin/time` |
| JSON.parse per SSE event (143 B text delta) | **0.20 µs** | micro-bench, 200k iters, repo zod |
| + 2 suspect-proto regexes (AI SDK `safeParseJSON`) | 0.23 µs (+17%) | same |
| + zod union `safeParse` (≈ transport `decodeLine`) | 0.27 µs (+37% vs bare parse) | same |
| outgoing `JSON.stringify`+`\n` per notification | 0.17 µs | same |
| `JSON.parse(JSON.stringify(...))` deep clones in T3/T4 hot files | **0** (8 in whole repo, none on these paths) | grep |
| `structuredClone` in hot files | 0 | grep |

Context: 50 tok/s stream ⇒ ~14 µs/s parse CPU. Steady-state streaming/GC is noise; only startup matters. (Consistent with orchestrator's 367–371 MB `doctor` baseline; protocol init adds ~10–20 MB over bare bundle load.)

## 1. Protocol framing (run.ts → bootstrap entrypoint → transport)

Call chain: `run.ts:527` `case "app-server"` → `run.ts:233 runZCodeProtocolCommand` (dotenv + env sanitize, then) → `packages/bootstrap/src/zcode-protocol-entrypoint.ts:82 runZCodeProtocolAgent` (SQLite open → provider-registry worker runtime → telemetry → MCP pool → server → connection) → `packages/bootstrap/src/zcode-protocol/transport.ts:41 ZCodeProtocolNdjsonConnection`.

- **NDJSON over stdin, manual buffer, no readline**: `transport.ts:83-95 onData` — `this.buffer += chunk.toString("utf8")`, then loop `indexOf("\n")` + `this.buffer = this.buffer.slice(newlineIndex + 1)`.
  - Allocation churn per chunk: 1 string decode + 1 concat; per line: 1 slice + 1 `trim()` (transport.ts:88) = 2–3 strings/message. Fine at protocol rates.
  - **O(n²) hazard**: if a client pipelines N messages in one chunk, each line slice copies the remaining tail — O(N·bytes). Only matters for bulk replays (session history restore), not normal traffic.
- **Parse per line**: `JSON.parse` at transport.ts:201, then **zod re-validation** `zcodeProtocolMessageSchema.safeParse` at transport.ts:213 (union of 4 strict schemas) — second full traversal + fresh object allocation per inbound message. Measured +37% over bare parse; negligible per-message, but it is a redundant double-walk.
- **Serialize per outbound message**: single choke point `transport.ts:77` `output.write(\`${JSON.stringify(message)}\n\`)` — one stringify + one template-string alloc per message. Count across hot path: transport 1, server.ts 0 — clean.
- Ordering machinery (transport.ts:156-196): 2 promises + closures per queued request — fine.

## 2. runner-stream (model streaming)

`packages/adapters/src/model/runner-stream.ts` (1,655 ln) does **not** parse SSE bytes. It consumes the Vercel AI SDK's `fullStream`:

- Wire parse stack: `runner-stream.ts:310-312` `runtime.streamText(...)` → AI SDK `ai@6.0.193` → `@ai-sdk/provider-utils/dist/index.mjs:2286-2296 parseJsonEventStream` = `TextDecoderStream → EventSourceParserStream (eventsource-parser@3.1.0) → safeParseJSON`.
- **eventsource-parser is a good incremental parser**: hand-rolled `indexOf`-based line scanner over decoded string chunks (`node_modules/eventsource-parser/dist/index.js:26-75`), no regex-per-chunk, no unbounded re-scan. **No regex-based SSE splitting found.**
- Per-event cost: `safeParseJSON` (`provider-utils/dist/index.mjs:817-838 _parse`) = `JSON.parse` + **2 suspect-proto/constructor regexes over the full event text** + zod schema validate of every SSE event (provider schema). Measured 0.23–0.27 µs/event — real but immaterial.
- **Where tokens get re-serialized**: only once, at the transport choke point (transport.ts:77) when a delta leaves as a protocol notification. **There is no per-token JSON.stringify inside runner-stream; zero stringify/parse call sites in runner-stream.ts, runner-normalization.ts, streaming-tool-call-assembler.ts.** Normalization (`runner-normalization.ts:62 toModelStreamEvent`) is a switch → 1 small object per delta. Tool-input deltas accumulate via assembler without intermediate serialization.
- The heavy work per attempt is status plumbing (spread-cloned status objects, ISO timestamps) — but only on request start/finish/retry, not per token (milestones guarded at runner-stream.ts:341, 206-236).

## 3. MCP client (T4)

- **Pooled, not per-call**: process-level pool created once at `zcode-protocol-entrypoint.ts:227-242 createMcpAdapterConnectionPool`; lease per app at `:273-278` (`mcpPortFactory` → `acquireLease({leaseId: sessionId})`), invoked once per ZCodeApp (`app/create-app.ts:375-379`). `adapters/src/mcp/pool.ts:57-418` keeps entries in a Map, idle-close after **30 s grace** (`pool.ts:16`); `callTool` reuses the live client (`mcp/index.ts:438-530 → requireEntry().adapter.callTool`, reconnect-once on dead stdio at `mcp/index.ts:477-489`). **No spawn per tool call.**
- **Session isolation prevents cross-lease reuse**: `pool.ts:164` `leaseId = ${++leaseSequence}:...` (globally unique per acquireLease) and `connectionKey` uses that raw leaseId as scope for session-isolated servers (`pool.ts:441-453`). Two leases for the *same* sessionId therefore never share an entry → each new app/session re-spawns stdio MCP children. Subagents are fine: they borrow the parent port (`core/src/subagent/borrowed-mcp-port.ts:38-70`, no new connections).
- **Discovery is cached**: tools cached in `McpServerRecord.tools` at connect (`mcp/index.ts:1066-1073`); `listTools()` returns the cache (`mcp/index.ts:434-436`). Rescan only on explicit `revalidate` (settings-page `mcp/list` with `revalidate: true`, `zcode-protocol/mcp.ts:94-104`) which pings every server (5 s timeout, `mcp/index.ts:95`).
- **Config parse frequency — the real finding**: every `mcp/list` request runs `createConfig(...)` (full config parse) + `resolveStartupPlugins(...)` (plugin disk scan) (`zcode-protocol/mcp.ts:36-58`). Settings-page refreshes re-read config + scan plugin dirs each time.
- Session startup connects once per runtime with a guard (`core/src/runtime/methods/mcp.ts:46-50 mcpInitialized`) — good.

## 4. GC pressure evidence

- String concat in loop: transport.ts:85 `this.buffer += ...` + per-line `slice` (transport.ts:89) — the only unbounded-shape accumulation on the protocol path.
- `toString("utf8")` per chunk: transport.ts:85.
- No `JSON.parse(JSON.stringify(...))` (0 hits), no `structuredClone` (0 hits) in runner-stream / mcp / transport / server / normalization.
- AI SDK internals: per-event zod object rebuilds (fresh objects per SSE event) — unavoidable alloc churn, sub-µs each.
- Per-attempt spread clones of statusContext (runner-stream.ts:671-675 etc.) — per retry, not per token.

## 5. Ranked fixes (batch-2 candidates)

| # | fix | where | est. gain |
|---|---|---|---|
| 1 | **Attack startup, not streaming** (T8 synergy): lazy-require the 31 MB bundle / NODE_COMPILE_CACHE / minify / split, or keep a warm app-server pool in our zcode proxy so clients reuse one booted process instead of re-paying 378 ms + 375 MB per session | run.ts bootstrap → bundle build (`packages/cli/scripts/build.mjs`); Rust-side connection mux in zcode-rs | **378 ms + ~375 MB per protocol session/subagent eliminated or amortized to ~0** — the only HIGH-value item here |
| 2 | **Protocol front-end in zcode binary** (proxyNDJSON): our Rust binary accepts the client socket, and keeps one long-lived `node ... app-server` child alive across client reconnects (epoch tag already exists in transport) | zcode-rs fallback path | hides full boot on every reconnect/desktop restart; pairs with #1 |
| 3 | **Drop zod double-walk on inbound messages**: trust `JSON.parse` + shape-check discriminator, keep zod only for admin/config methods | transport.ts:213 | ~37% of inbound parse CPU; matters only for bulk replays (session history) — LOW today |
| 4 | **Fix framing buffer**: scan chunk in place (`indexOf` on chunk with carry of partial line) instead of `buffer +=`/`slice` | transport.ts:83-95 | removes O(n²) tail-copy risk; only visible on pipelined/bulk frames — LOW-MED |
| 5 | **Stable lease key for same-session reuse**: key session-isolated pool entries on `(sessionId, serverName, config)` instead of sequence-prefixed leaseId | pool.ts:164, 441-453 | avoids re-spawning stdio MCP children when an app re-acquires; per-session MCP boot (initialize+tools/list, typically 0.3–2 s × servers) saved on app recreation/resume — MED |
| 6 | **Cache `createConfig` + `resolveStartupPlugins`** keyed on (cwd, mtime) for `mcp/list` | zcode-protocol/mcp.ts:36-58 | tens of ms + disk scans per settings refresh — LOW-MED |
| 7 | **Incremental SSE parser / Rust NAPI parse layer**: measured per-event parse is 0.2–0.3 µs; a Rust SSE+zod replacement saves <1 µs/token ⇒ **<0.1 ms per 100-token reply**. Do NOT invest here | provider-utils/eventsource-parser | ~0 (falsified) |
| 8 | **Avoid double-serialize of deltas**: already single-serialize (transport.ts:77). Nothing to do | — | 0 (already clean) |

## Verdict for orchestrator

- T3: steady-state streaming is **not** a bottleneck (numbers above). The protocol path's cost is 100% boot (T8). Merge T3 remediation into T8/warm-process work.
- T4: pooling exists and is sound for the tool-call path; the actionable defects are lease-key reuse (#5) and per-request config rescan (#6). Rescan is settings-page-only; #5 benefits session resume/subagent churn.

## Repro artifacts

- Handshake driver: `/tmp/t3-handshake.mjs` (spawn → runtime/capabilities → ready latency).
- Parse micro-bench: `/tmp/t3-parsebench.mjs`.
- Raw outputs: `/tmp/t3-time.txt`, `/tmp/t3-out.txt`.
