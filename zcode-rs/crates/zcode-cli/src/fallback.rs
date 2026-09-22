//! Exec the Node bundle (dist/zcode.cjs) with the original argv, byte-passthrough.
//! Used for every agent/LLM/TUI/locale-dependent path the Rust core doesn't own.
//!
//! V8 heap-flag tuning (batch-3 G, 2026-09-22, `zcode -p hi` fail-fast agent path,
//! hyperfine -i --warmup 3 --runs 12 / /usr/bin/time -l, n=3 per RSS cell; host had
//! 10 concurrent batch-3 builds so time σ is large, RSS spread ~1%):
//!
//! | config                      | time (ms)        | peak RSS (MB) |
//! |-----------------------------|------------------|---------------|
//! | none (NODE_COMPILE_CACHE)   | 743–1030 (noisy) | 340.4         |
//! | +semi-space 16              | ~noise vs base   | 316.5 (−7.0%) |
//! | +semi-space 32              | ~noise vs base   | 340.5 (±0%)   |
//! | +semi-space 64              | ~noise vs base   | 338.5 (−0.6%) |
//!
//! Time showed no reproducible regression (order-controlled runs: apparent ±10–20%
//! swings flip with benchmark position under machine contention; order-corrected
//! delta ≈ ±1.5%). RSS: only semi16 clears the ≥5% bar, reproducibly (−7.0%).
//! Decision per rule: KEEP `--max-semi-space-size=16`; reject 32/64. Native
//! `skills list` RSS is ~3.4 MB and unaffected either way (flag only reaches the
//! node fallback child). Injected ONLY when the user has not set NODE_OPTIONS.

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

pub fn node_bundle_path() -> PathBuf {
    if let Ok(p) = std::env::var("ZCODE_NODE_BUNDLE") {
        return PathBuf::from(p);
    }
    if let Ok(p) = std::env::var("ZCODE_PORT_DIR") {
        let candidate = PathBuf::from(p).join("ZCode/apps/zcode-cli/packages/cli/dist/zcode.cjs");
        if candidate.exists() {
            return candidate;
        }
    }
    // Compile-time default for this port workspace.
    let candidate = PathBuf::from(env!("ZCODE_PORT_DIR_DEFAULT"))
        .join("ZCode/apps/zcode-cli/packages/cli/dist/zcode.cjs");
    if candidate.exists() {
        return candidate;
    }
    // Next to the binary (SEA-style install layout).
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.parent().map(|d| d.join("zcode.cjs"));
        if let Some(s) = sibling.filter(|p| p.exists()) {
            return s;
        }
    }
    candidate
}

pub fn exec_fallback(argv: &[String]) -> i32 {
    let bundle = node_bundle_path();
    let node = std::env::var("ZCODE_NODE").unwrap_or_else(|_| "node".to_string());
    let mut cmd = Command::new(&node);
    // V8 compile cache: cuts ~119ms (33%) of per-dispatch bundle load. Measured in
    // analysis/T1-T8-spawn-report.md; safe to leave enabled (Node manages the cache dir).
    if std::env::var_os("NODE_COMPILE_CACHE").is_none() {
        let port_dir = std::env::var("ZCODE_PORT_DIR")
            .unwrap_or_else(|_| env!("ZCODE_PORT_DIR_DEFAULT").to_string());
        let cache = PathBuf::from(port_dir).join(".node-compile-cache");
        cmd.env("NODE_COMPILE_CACHE", &cache);
    }
    // Measured (see module header): smaller scavenger semi-space cuts peak RSS 7%
    // on the agent path at no time cost. Never override a user-set NODE_OPTIONS.
    if std::env::var_os("NODE_OPTIONS").is_none() {
        cmd.env("NODE_OPTIONS", "--max-semi-space-size=16");
    }
    cmd.arg(&bundle).args(argv);
    match cmd.status() {
        Ok(status) => {
            if let Some(code) = status.code() {
                code
            } else {
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    if let Some(sig) = status.signal() {
                        return 128 + sig;
                    }
                }
                1
            }
        }
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "Error: failed to launch node runtime: {e}");
            1
        }
    }
}
