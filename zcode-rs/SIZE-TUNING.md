# SIZE-TUNING — [profile.release] experiments (agent F, Batch-3, 2026-09-22)

Method: one variant at a time; after each, full rebuild + all 3 parity gates
(`bench/parity.mjs` × golden-args, golden-args2, golden-args3) + hyperfine (n≥15, `-N`)
on `--version` and `skills list`. Sizes are `stat -f %z` of `target/release/zcode`.

Baseline profile: `opt-level=3, lto="thin", codegen-units=1, strip=true` → **875552 B**,
perf 2.2 ms (`--version`) / 5.3 ms (`skills list`).

## Results

| Variant | Size (B) | Δ size | Gates vs contemporaneous baseline | Perf (head-to-head vs orig) | Verdict |
|---|---|---|---|---|---|
| baseline (3/thin/cgu1/strip) | 875552 | — | 106/5 · 37/0 · 13/0 (5 pre-existing fails: case066/067/103/104/105) | — | reference |
| 1. `opt-level="z"` | 657504 | **−24.9%** | identical all 3 | `--version` tie (opt-z 1.01× faster); `skills list` +3.8% (5.2→5.4 ms, orig 1.05× ± 0.10, σ overlaps) — within 5% budget | **KEPT** |
| 2. `lto="fat"` + opt3 | 817712 | −6.6% | identical all 3 | ~tie (noisy; parallel builds) | rejected: small cut, slower build (41 s vs 21 s), no win over z |
| 3. `panic="abort"` (+opt3/thin) | 775920 | −11.4% | identical all 3 (no golden case panics) | tie | **REJECTED-with-reason**: 11 panic-reachable `unwrap/expect` sites in shipped modules (commands_cmd, plugins_cmd, skills_cmd, args); on any real-world panic, designed behavior (hook prints `Error: …`, unwind → exit 101) becomes SIGABRT/exit 134 + macOS crash-reporter dialog. Any doubt → rejected per directive |
| 4. `opt-level="s"` | 768288 | −12.2% | gate3 first read 10/3 — proven NOT the profile: control rebuild of the ORIGINAL profile on the same (concurrently edited) source fails the same scase003/007/013 (agent H's in-flight internal_search.rs drift) | too noisy to defend | rejected: cut less than half of opt-z's, no advantage |

## Final head-to-head (same hyperfine session, latest source)

- orig profile: 875584 B · opt-z: **674144 B (−23.0%)**
- `--version`: tie (1.01× in opt-z's favor) · `skills list`: orig 1.05× faster (+3.8%, ≤5% budget)
- Gates on final opt-z build: 108/3 · 37/0 · 10/3 — byte-identical to orig-profile control
  (108/3 · 37/0 · 10/3). Remaining fails are other agents' in-flight modules, present on
  BOTH profiles; not profile-related.

## Kept

```toml
[profile.release]
opt-level = "z"
lto = "thin"
codegen-units = 1
strip = true
```

≈ **−23% binary size for ≤5% (statistically marginal) runtime cost.** Caveat: `opt-level="z"`
optimizes for size; if hot paths grow (e.g. TUI/render loops), re-run the head-to-head.
