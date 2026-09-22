//! Hidden invocations `__zcode-plugin-host` / `__zcode-dwf-child`
//! (packages/cli/src/plugin-host-command.ts + dwf-child-command.ts).
//!
//! Both commands are runtime-heavy at their core — `__zcode-plugin-host`
//! dynamic-imports a plugin server module in-process after restoring captured
//! CUA broker credentials, and `__zcode-dwf-child` imports a generated ESM
//! entry and injects node vm/readline/stdio into it — so only the argv
//! predicates (`isPluginHostInvocation` / `isDwfChildInvocation`) and the
//! pure-output early paths (usage lines, missing-server-file rejection) are
//! ported natively; everything that needs the module loader execs the Node
//! bundle for byte-identical output.
//!
//! Note: run.rs's pre-wired stub arms route the bare names "plugin-host" /
//! "dwf-child" here; node has no such commands (parseArgs → "Unknown
//! command"), so those inputs fall through to the bundle untouched. This
//! module keys on the real `__zcode-*` names, matching run.ts's dispatch
//! (`ctx.argv.slice(1)` is passed to the TS handlers).

use crate::fallback::exec_fallback;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

const HOST_COMMAND: &str = "__zcode-plugin-host";
const DWF_CHILD_COMMAND: &str = "__zcode-dwf-child";
const HOST_USAGE: &str = "__zcode-plugin-host <server-path> [-- <server-arg>...]";
const CHILD_USAGE: &str = "__zcode-dwf-child <entry path>";

pub fn run(argv: &[String]) -> i32 {
    match argv.first().map(|s| s.as_str()) {
        Some(HOST_COMMAND) => run_plugin_host(argv),
        Some(DWF_CHILD_COMMAND) => run_dwf_child(argv),
        _ => exec_fallback(argv),
    }
}

/// runPluginHostCommand: local paths are (1) the empty-argv usage line and
/// (2) the existsSync rejection, both printed before any credential or module
/// work happens; a server file that exists needs the Node runtime.
fn run_plugin_host(argv: &[String]) -> i32 {
    let rest = &argv[1..];
    if rest.is_empty() {
        let _ = writeln!(std::io::stderr(), "Usage: {HOST_USAGE}");
        return 1;
    }
    let server_path = node_path_resolve(&rest[0]);
    if !server_path.exists() {
        // existsSync failure is thrown inside the try block and surfaces as
        // "Plugin host failed: <message>" via the catch.
        let _ = writeln!(
            std::io::stderr(),
            "Plugin host failed: Plugin server file does not exist."
        );
        return 1;
    }
    exec_fallback(argv)
}

/// runDwfChildCommand: only the empty-argv usage line is pure output; any
/// entry path requires import()ing the generated ESM file plus the
/// vm/readline/stdio injection, which stays in the Node bundle.
fn run_dwf_child(argv: &[String]) -> i32 {
    let rest = &argv[1..];
    if rest.is_empty() {
        let _ = writeln!(std::io::stderr(), "Usage: {CHILD_USAGE}");
        return 1;
    }
    exec_fallback(argv)
}

/// node:path.resolve for one segment: absolute paths normalize lexically,
/// relative paths resolve against the cwd. Only used for the existence probe,
/// where resolve("")→cwd matters (existsSync("") is false, cwd is not).
fn node_path_resolve(raw: &str) -> PathBuf {
    let start = if Path::new(raw).is_absolute() {
        PathBuf::from("/")
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
    };
    let mut out = start;
    for comp in Path::new(raw).components() {
        match comp {
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(c) => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_usage_on_empty_rest() {
        assert_eq!(run_plugin_host(&[HOST_COMMAND.into()]), 1);
    }

    #[test]
    fn host_missing_server_file_rejected() {
        assert_eq!(
            run_plugin_host(&[HOST_COMMAND.into(), "/nonexistent/zcode-server.js".into()]),
            1
        );
    }

    #[test]
    fn child_usage_on_empty_rest() {
        assert_eq!(run_dwf_child(&[DWF_CHILD_COMMAND.into()]), 1);
    }

    #[test]
    fn resolve_empty_is_cwd() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(node_path_resolve(""), cwd);
    }

    #[test]
    fn resolve_lexical_parent() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(node_path_resolve("foo/../bar"), cwd.join("bar"));
        assert_eq!(node_path_resolve("/a/b/../c"), PathBuf::from("/a/c"));
    }
}
