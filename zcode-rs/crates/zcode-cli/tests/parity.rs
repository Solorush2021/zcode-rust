// parity.rs — cargo-test wrapper around bench/parity.mjs (G2 golden gates).
// Replays every bench/golden-args*/manifest.jsonl matrix against the release
// binary and asserts each gate exits 0. Auto-includes matrices created by
// later agents (golden-args4/5/6, ...). Skips (passes, with a notice) when
// node or fixtures are absent so `cargo test` works on bare machines.
//
// Run: cd zcode-rs && cargo test --release parity -- --nocapture
// Binary under test: $ZCODE_BIN, else <repo>/target/release/zcode.

use std::path::PathBuf;
use std::process::Command;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn port_dir() -> PathBuf {
    // crates/zcode-cli -> zcode-rs -> repo root (where bench/ lives)
    manifest_dir().join("../../..").canonicalize().unwrap_or_else(|_| manifest_dir().join("../../.."))
}

fn binary_path() -> PathBuf {
    match std::env::var_os("ZCODE_BIN") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        // zcode-rs/crates/zcode-cli -> zcode-rs/target/release/zcode
        _ => manifest_dir().join("../../target/release/zcode"),
    }
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn find_matrices() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let bench = port_dir().join("bench");
    let Ok(entries) = std::fs::read_dir(&bench) else {
        return out;
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for d in dirs {
        let m = d.join("manifest.jsonl");
        if m.is_file() {
            out.push(d);
        }
    }
    out
}

#[test]
fn parity_all_matrices() {
    if std::env::var("ZCODE_PARITY_SKIP").as_deref() == Ok("1") {
        eprintln!("parity: SKIPPED (ZCODE_PARITY_SKIP=1)");
        return;
    }
    if !node_available() {
        eprintln!("parity: SKIPPED (node not found on PATH)");
        return;
    }
    let port = port_dir().canonicalize().unwrap_or_else(|_| port_dir());
    let script = port.join("bench/parity.mjs");
    let bin = binary_path();
    if !bin.is_file() {
        panic!(
            "parity: binary not found at {} — build it first: cd zcode-rs && cargo build --release (or set ZCODE_BIN)",
            bin.display()
        );
    }
    let matrices = find_matrices();
    if matrices.is_empty() {
        eprintln!(
            "parity: SKIPPED (no bench/golden-args*/manifest.jsonl fixtures under {})",
            port.join("bench").display()
        );
        return;
    }
    let mut failed: Vec<String> = Vec::new();
    for dir in &matrices {
        let name = dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| dir.display().to_string());
        println!("--- parity gate: {} ---", name);
        let out = Command::new("node")
            .arg(&script)
            .arg(&bin)
            .arg(dir)
            .output()
            .unwrap_or_else(|e| panic!("parity: failed to spawn node for gate {}: {}", name, e));
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            println!("{}", line);
        }
        let err = String::from_utf8_lossy(&out.stderr);
        if !err.trim().is_empty() {
            eprintln!("{}", err.trim_end());
        }
        if !out.status.success() {
            failed.push(name);
        }
    }
    assert!(
        failed.is_empty(),
        "parity: {} gate(s) FAILED: {}",
        failed.len(),
        failed.join(", ")
    );
}
