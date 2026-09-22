//! Native `__internal-search` — port of cli/src/internal-search/embedded-search-cli.ts.
//! Strategy: spawn the system binaries and pass bytes through (do NOT reimplement
//! grep semantics). Spec: analysis/T2-search-report.md §1.
//!
//! Vendored-tool env overrides mirror the TS backend selector
//! (bootstrap/src/app/embedded-search-backend.ts + adapters/src/exec/embedded-search-prelude.ts,
//! `native-binaries` kind): when ZCODE_UGREP_BINARY / ZCODE_BFS_BINARY
//! (packages/shared/src/runtime-tool-runtime.ts `binaryEnvVar`) name an existing
//! executable, that binary is spawned with the prelude's default args. Grep bypass
//! args always fall back to system grep unmodified (ugrep -z/-Z semantics diverge).

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const GREP_DEFAULT_ARGS: &[&str] = &[
    "-G",
    "-I",
    "--exclude-dir=.git",
    "--exclude-dir=.svn",
    "--exclude-dir=.hg",
    "--exclude-dir=.bzr",
    "--exclude-dir=.jj",
    "--exclude-dir=.sl",
];

/// embedded-search-prelude.ts UGREP_DEFAULT_ARGS (native-binaries grep prelude).
const UGREP_DEFAULT_ARGS: &[&str] = &[
    "-G",
    "--ignore-files",
    "--hidden",
    "-I",
    "--exclude-dir=.git",
    "--exclude-dir=.svn",
    "--exclude-dir=.hg",
    "--exclude-dir=.bzr",
    "--exclude-dir=.jj",
    "--exclude-dir=.sl",
];

/// embedded-search-prelude.ts BFS_DEFAULT_ARGS (native-binaries find prelude).
const BFS_DEFAULT_ARGS: &[&str] = &["-S", "dfs", "-regextype", "findutils-default"];

/// Byte-faithful hand-rolled ports of the 12 bypass regexes (embedded-search-cli.ts:23-37).
fn is_bypass_arg(arg: &str) -> bool {
    let starts_dash = arg.starts_with('-');
    if !starts_dash {
        return false;
    }
    // /^---.*$/
    if arg.starts_with("---") {
        return true;
    }
    // /^-@.*$/
    if arg.starts_with("-@") {
        return true;
    }
    // /^-[Zz].*$/
    let second = arg.chars().nth(1);
    if matches!(second, Some('Z') | Some('z')) {
        return true;
    }
    // /^-[^-].*[Zz].*$/
    if second.is_some() && second != Some('-') && arg.contains(['Z', 'z']) {
        return true;
    }
    // /^-.*-filter.*$/, pager, view, format-open, config, save-config
    arg.contains("-filter")
        || arg.contains("-pager")
        || arg.contains("-view")
        || arg.contains("-format-open")
        || arg.contains("-save-config")
        || arg.contains("-config")
        || arg == "--null"
        || arg == "--null-data"
}

pub fn run(args: &[String]) -> i32 {
    let Some(cmd) = args.first() else {
        let _ = writeln!(std::io::stderr(), "unsupported embedded search command: ");
        return 2;
    };
    let rest = &args[1..];
    match cmd.as_str() {
        "find" => match env_binary("ZCODE_BFS_BINARY") {
            // native-binaries find prelude: `command <bfs> -S dfs -regextype findutils-default "$@"`.
            Some(binary) => spawn_passthrough(
                &binary,
                "find",
                &prepend(BFS_DEFAULT_ARGS, rest),
            ),
            None => spawn_passthrough("find", "find", rest),
        },
        "grep" => {
            if rest.iter().any(|a| is_bypass_arg(a)) {
                // Bypass always returns to system grep with args unmodified — the TS
                // prelude does `command grep "$@"` (ugrep -z/-Z semantics diverge).
                spawn_passthrough("grep", "grep", rest)
            } else if let Some(binary) = env_binary("ZCODE_UGREP_BINARY") {
                // native-binaries grep prelude: `command <ugrep> -G --ignore-files --hidden -I … "$@"`.
                spawn_passthrough(&binary, "grep", &prepend(UGREP_DEFAULT_ARGS, rest))
            } else {
                let full = prepend(GREP_DEFAULT_ARGS, rest);
                spawn_passthrough("grep", "grep", &full)
            }
        }
        other => {
            let _ = writeln!(std::io::stderr(), "unsupported embedded search command: {other}");
            2
        }
    }
}

fn prepend(defaults: &[&str], rest: &[String]) -> Vec<String> {
    let mut full: Vec<String> = defaults.iter().map(|s| s.to_string()).collect();
    full.extend(rest.iter().cloned());
    full
}

/// Mirror of the TS `env[binaryEnvVar]?.trim() || "default"` + shell `command -v` check:
/// non-empty (trimmed) value that resolves to an existing executable; anything else
/// (unset, whitespace, missing/non-executable path, name not on PATH) yields None.
fn env_binary(env_name: &str) -> Option<String> {
    let value = std::env::var(env_name).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains('/') {
        if is_executable_file(Path::new(trimmed)) {
            Some(trimmed.to_string())
        } else {
            None
        }
    } else {
        lookup_on_path(trimmed)
    }
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

/// `command -v <name>` equivalent: first executable `<name>` on PATH.
#[cfg(unix)]
fn lookup_on_path(name: &str) -> Option<String> {
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable_file(candidate))
        .and_then(|candidate| candidate.into_os_string().into_string().ok())
}

#[cfg(not(unix))]
fn lookup_on_path(name: &str) -> Option<String> {
    let path_var = std::env::var("PATH").ok()?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

/// `label` keeps the TS error-message shape (`failed to run embedded grep: …`)
/// even when the spawned program is a vendored binary path.
fn spawn_passthrough(program: &str, label: &str, args: &[String]) -> i32 {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    match cmd.status() {
        Ok(status) => {
            if let Some(code) = status.code() {
                return code;
            }
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                if let Some(sig) = status.signal() {
                    // Downstream-closed pipe (grep … | head) settles 0 in the TS wrapper;
                    // with inherited stdio the child dies by SIGPIPE directly.
                    if sig == 13 {
                        return 0;
                    }
                    return 128 + sig;
                }
            }
            1
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let _ = writeln!(
                std::io::stderr(),
                "failed to run embedded {label}: spawn {label} ENOENT"
            );
            127
        }
        Err(_) => 1,
    }
}
