# zcode-rs quickstart

Rust port of the ZCode CLI: local commands run natively (~200x faster, ~1MB RSS); the rest exec the Node bundle via a fallback.

```zsh
# 0) in-repo toolchain (run from repo root)
PORTDIR=$PWD; export CARGO_HOME="$PORTDIR/.cargo" RUSTUP_HOME="$PORTDIR/.rustup" PATH="$PORTDIR/.cargo/bin:$PATH"
# 1) build → zcode-rs/target/release/zcode (~426KB)
cd zcode-rs && cargo build --release && cd ..
# 2) one-shot verify: rebuild + every parity gate + hyperfine bench
./bench/all.sh                  # ZCODE_SKIP_BENCH=1 ./bench/all.sh to skip the bench
```

- **Gates (cargo)**: `cd zcode-rs && cargo test --release parity -- --nocapture` — replays every `bench/golden-args*/manifest.jsonl` matrix via `node bench/parity.mjs <bin> <dir>`; each gate must print `parity: N pass, 0 fail` (161/161 over 3 matrices today). Auto-includes new golden-args4/5/6… matrices; set `ZCODE_PARITY_SKIP=1` to bypass on machines without node.
- **Single gate**: `node bench/parity.mjs zcode-rs/target/release/zcode bench/golden-args2`.
- **Bench**: `./bench/final.sh` → `bench/FINAL.md` (hyperfine head-to-head vs Node); baseline history in `bench/BASELINE.md` / `bench/FINAL.md`.
- **Packaging**: `zcode-rs/scripts/package-rs.sh` + `zcode-rs/README` (batch-3); campaign context in `OPTIMIZATION_CONTEXT.md`.
