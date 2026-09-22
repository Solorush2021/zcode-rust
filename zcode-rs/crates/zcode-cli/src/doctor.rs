//! Port of runDoctor (run.ts:188). Two lines deviate from the Node golden by
//! design and are masked in the parity test: `node:` (the Rust binary's
//! runtime, not Node's) and `default artifact:` (rust binary + node fallback).
//! Field order and formatting are byte-exact.

use crate::run::VERSION;
use serde::Serialize;
use std::io::Write;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DoctorPayload {
    cli: CliInfo,
    runtime: RuntimeInfo,
    packaging: Packaging,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CliInfo {
    name: &'static str,
    process_name: &'static str,
    version: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeInfo {
    arch: String,
    cwd: String,
    exec_path: String,
    node: String,
    platform: String,
    process_title: &'static str,
    sea: bool,
}

#[derive(Serialize)]
struct Packaging {
    default: &'static str,
    sea: &'static str,
}

fn runtime_info() -> RuntimeInfo {
    let exec_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    // process.platform / process.arch naming (darwin/arm64), not Rust's.
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    RuntimeInfo {
        arch: arch.to_string(),
        cwd: std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        exec_path,
        node: format!("rust-binary (rustc {})", rustc_version()),
        platform: platform.to_string(),
        process_title: "zcode-cli",
        sea: false,
    }
}

fn rustc_version() -> &'static str {
    env!("ZCODE_RUSTC_VERSION")
}

pub fn run(json: bool, verbose: bool) -> i32 {
    let payload = DoctorPayload {
        cli: CliInfo { name: "zcode", process_name: "zcode-cli", version: VERSION },
        runtime: runtime_info(),
        packaging: Packaging { default: "rust-binary (node fallback)", sea: "optional" },
    };
    let out = std::io::stdout();
    let mut w = out.lock();
    if json {
        // formatJson = JSON.stringify(_, null, 2) + trailing newline.
        let _ = writeln!(w, "{}", serde_json::to_string_pretty(&payload).unwrap_or_default());
        return 0;
    }
    let _ = writeln!(w, "zcode doctor");
    let _ = writeln!(w, "version: {}", payload.cli.version);
    let _ = writeln!(w, "process: {}", payload.runtime.process_title);
    let _ = writeln!(w, "node: {}", payload.runtime.node);
    let _ = writeln!(
        w,
        "platform: {}/{}",
        payload.runtime.platform, payload.runtime.arch
    );
    let _ = writeln!(w, "sea: {} ({})", if payload.runtime.sea { "yes" } else { "no" }, payload.packaging.sea);
    let _ = writeln!(w, "default artifact: {}", payload.packaging.default);
    if verbose {
        let _ = writeln!(w, "execPath: {}", payload.runtime.exec_path);
        let _ = writeln!(w, "cwd: {}", payload.runtime.cwd);
    }
    0
}
