//! Port of packages/cli/src/commands-command.ts plus the discovery engine it
//! calls into (bootstrap/custom-commands.ts, adapters/commands/index.ts,
//! adapters/commands/roots.ts and the plugins half of bootstrap/plugins.ts +
//! adapters/plugins/index.ts). Byte-exact for the reachable states of this
//! machine: project/user .zcode + .agents command dirs, enabled-plugin command
//! roots (official cache + installed records + inline dirs), frontmatter
//! parsing, diagnostics, JSON shape and error lines.
//!
//! Known gaps (not reachable here): Node-style OS error strings for exotic
//! scan/read failures, and `--verbose` error stacks (JS stack traces are not
//! portable). Plugin-pipeline diagnostics (plugin_* codes) are dropped by the
//! TS commands path exactly as here — only command scan diagnostics surface.

use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

const COMMANDS_COMMAND_USAGE: &str = "Usage: zcode commands [list|inspect <name>]";
const COMMAND_EXTENSION: &str = ".md";
const DEFAULT_MAX_COMMAND_BYTES: u64 = 100_000;
const MAX_DESCRIPTION_LENGTH: usize = 1024; // UTF-16 code units, like JS .length/.slice
const MAX_SCAN_DEPTH: usize = 12;

const SAFE_FRONTMATTER_KEYS: [&str; 6] = [
    "allowed-tools",
    "argument-hint",
    "description",
    "disable-noninteractive",
    "model",
    "skills",
];

pub fn run(rest: &[String], json: bool, verbose: bool) -> i32 {
    let subcommand = rest.first().map(|s| s.as_str()).unwrap_or("list");
    if subcommand == "list" {
        if rest.len() > 1 {
            return fail_usage();
        }
        return run_list(json, verbose);
    }
    if subcommand == "inspect" {
        // args.length !== 2 || args[1]?.trim().length === 0
        if rest.len() != 2 || js_trim(&rest[1]).is_empty() {
            return fail_usage();
        }
        return run_inspect(&rest[1], json, verbose);
    }
    let mut e = std::io::stderr();
    let _ = write!(e, "Unknown commands command: {subcommand}\n{COMMANDS_COMMAND_USAGE}\n");
    1
}

fn fail_usage() -> i32 {
    let _ = writeln!(std::io::stderr(), "{COMMANDS_COMMAND_USAGE}");
    1
}

// ============================================================
// Metadata model
// ============================================================

struct Metadata {
    allowed_tools: Vec<String>,
    argument_hint: Option<String>,
    description: String,
    disable_non_interactive: bool,
    frontmatter_keys: Vec<String>,
    model: Option<String>,
    name: String,
    path: String,
    root_path: String,
    scope: &'static str,
    skills: Vec<String>,
    source: &'static str,
}

struct Diagnostic {
    code: &'static str,
    message: String,
    command_name: Option<String>,
    path: Option<String>,
    severity: &'static str,
}

impl Diagnostic {
    fn to_json(&self) -> Value {
        let mut o = json!({
            "code": self.code,
            "message": self.message,
        });
        let map = o.as_object_mut().unwrap();
        if let Some(cn) = &self.command_name {
            map.insert("commandName".into(), json!(cn));
        }
        if let Some(p) = &self.path {
            map.insert("path".into(), json!(p));
        }
        map.insert("severity".into(), json!(self.severity));
        o
    }
}

impl Metadata {
    fn to_json(&self) -> Value {
        let mut o = json!({
            "allowedTools": self.allowed_tools,
            "description": self.description,
            "disableNonInteractive": self.disable_non_interactive,
            "frontmatterKeys": self.frontmatter_keys,
            "name": self.name,
            "path": self.path,
            "rootPath": self.root_path,
            "scope": self.scope,
            "skills": self.skills,
            "source": self.source,
        });
        let map = o.as_object_mut().unwrap();
        if let Some(h) = &self.argument_hint {
            map.insert("argumentHint".into(), json!(h));
        }
        if let Some(m) = &self.model {
            map.insert("model".into(), json!(m));
        }
        o
    }
}

struct Outcome {
    commands: Vec<Metadata>,
    diagnostics: Vec<Diagnostic>,
    total_discovered: usize,
}

// ============================================================
// Subcommands
// ============================================================

fn run_list(json: bool, verbose: bool) -> i32 {
    let cwd = current_dir_string();
    match discover(&cwd) {
        Ok(outcome) => {
            let mut w = std::io::stdout();
            if json {
                let payload = json!({
                    "commands": outcome.commands.iter().map(|c| c.to_json()).collect::<Vec<_>>(),
                    "cwd": cwd,
                    "diagnostics": outcome.diagnostics.iter().map(|d| d.to_json()).collect::<Vec<_>>(),
                    "totalDiscovered": outcome.total_discovered,
                });
                let _ = writeln!(w, "{}", serde_json::to_string_pretty(&payload).unwrap_or_default());
            } else {
                let _ = write!(w, "{}", format_human_command_list(&outcome, verbose));
            }
            0
        }
        Err(msg) => report_error(&msg, verbose),
    }
}

fn run_inspect(name: &str, json: bool, verbose: bool) -> i32 {
    let cwd = current_dir_string();
    // bootstrap.inspectZCodeCustomCommand: discover, then fail with the ORIGINAL
    // (un-normalized) name in the message before loadCommand re-resolves it.
    let normalized = normalize_command_name(name);
    let outcome = match discover(&cwd) {
        Ok(o) => o,
        Err(msg) => return report_error(&msg, verbose),
    };
    let found = outcome.commands.iter().find(|c| c.name == normalized);
    let metadata = match found {
        Some(m) => m,
        None => return report_error(&format!("Custom command not found: {name}"), verbose),
    };

    // loadCommand: stat, truncate at 100_000 bytes, strip frontmatter, trim.
    let max_bytes = DEFAULT_MAX_COMMAND_BYTES;
    let (bytes_read, size_bytes, truncated, content) = match read_command_body(&metadata.path, max_bytes) {
        Ok(v) => v,
        Err(msg) => return report_error(&msg, verbose),
    };

    if json {
        let payload = json!({
            "command": {
                "bytesRead": bytes_read,
                "content": content,
                "metadata": metadata.to_json(),
                "sizeBytes": size_bytes,
                "truncated": truncated,
            },
            "cwd": cwd,
            "diagnostics": outcome.diagnostics.iter().map(|d| d.to_json()).collect::<Vec<_>>(),
        });
        let _ = writeln!(
            std::io::stdout(),
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        );
    } else {
        let _ = write!(
            std::io::stdout(),
            "{}",
            format_human_command_inspection(metadata, bytes_read, size_bytes, truncated, &content, verbose, &outcome)
        );
    }
    0
}

fn report_error(message: &str, _verbose: bool) -> i32 {
    // TS also prints error.stack under --verbose; JS stacks are not portable.
    let _ = writeln!(std::io::stderr(), "Error: {message}");
    1
}

fn current_dir_string() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

// ============================================================
// Human formatting
// ============================================================

fn format_human_command_list(outcome: &Outcome, verbose: bool) -> String {
    if outcome.commands.is_empty() {
        return "No custom commands found.\n".to_string();
    }
    let mut lines = vec![format!("Custom commands ({})", outcome.commands.len())];
    for command in &outcome.commands {
        let hint = command
            .argument_hint
            .as_ref()
            .map(|h| format!(" {h}"))
            .unwrap_or_default();
        lines.push(format!("- /{}{}", command.name, hint));
        let suffix = if command.disable_non_interactive { " (interactive only)" } else { "" };
        lines.push(format!("  {}{}", command.description, suffix));
        lines.push(format!("  {}/{}: {}", command.scope, command.source, command.path));
    }
    append_diagnostics(&mut lines, &outcome.diagnostics, verbose);
    format!("{}\n", lines.join("\n"))
}

fn format_human_command_inspection(
    metadata: &Metadata,
    bytes_read: usize,
    size_bytes: u64,
    truncated: bool,
    content: &str,
    verbose: bool,
    outcome: &Outcome,
) -> String {
    let mut lines = vec![
        format!("Command: /{}", metadata.name),
        format!("scope/source: {}/{}", metadata.scope, metadata.source),
        format!("path: {}", metadata.path),
        format!("description: {}", metadata.description),
    ];
    if let Some(h) = &metadata.argument_hint {
        lines.push(format!("argumentHint: {h}"));
    }
    if !metadata.allowed_tools.is_empty() {
        lines.push(format!("allowedTools: {}", metadata.allowed_tools.join(", ")));
    }
    if !metadata.skills.is_empty() {
        lines.push(format!("skills: {}", metadata.skills.join(", ")));
    }
    if let Some(m) = &metadata.model {
        lines.push(format!("model: {m}"));
    }
    lines.push(format!(
        "size: {bytes_read}/{size_bytes} bytes{}",
        if truncated { " (truncated)" } else { "" }
    ));
    if verbose {
        lines.push(String::new());
        lines.push("Content".to_string());
        lines.push(if content.is_empty() { "(empty)".to_string() } else { content.to_string() });
    }
    append_diagnostics(&mut lines, &outcome.diagnostics, verbose);
    format!("{}\n", lines.join("\n"))
}

fn append_diagnostics(lines: &mut Vec<String>, diagnostics: &[Diagnostic], verbose: bool) {
    if !verbose || diagnostics.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push(format!("Diagnostics ({})", diagnostics.len()));
    for d in diagnostics {
        let location = d.path.as_ref().map(|p| format!(" ({p})")).unwrap_or_default();
        lines.push(format!("- [{}] {}: {}{}", d.severity, d.code, d.message, location));
    }
}

// ============================================================
// Discovery engine (adapters/commands/index.ts + roots.ts)
// ============================================================

struct Root {
    path: PathBuf,
    scope: &'static str,
    source: &'static str,
    priority: usize,
}

fn discover(working_directory: &str) -> Result<Outcome, String> {
    let roots = resolve_roots(working_directory);
    let disabled_paths = collect_disabled_paths(working_directory);

    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut selected: Vec<(String, Metadata)> = Vec::new();
    let mut selected_names: BTreeSet<String> = BTreeSet::new();
    let mut total_discovered = 0usize;

    let mut sorted = roots;
    sorted.sort_by_key(|r| r.priority);
    for root in &sorted {
        for path in command_files_under_root(&root.path, &mut diagnostics) {
            let parsed = match parse_command(&path, root, &mut diagnostics) {
                Some(p) => p,
                None => continue,
            };
            // config-disabled commands never enter the usable set (or the total).
            if disabled_paths.contains(&parsed.path) {
                continue;
            }
            total_discovered += 1;
            if selected_names.contains(&parsed.name) {
                diagnostics.push(Diagnostic {
                    code: "custom_command_duplicate_name",
                    command_name: Some(parsed.name.clone()),
                    message: format!("Duplicate custom command ignored: {}", parsed.name),
                    path: Some(parsed.path.clone()),
                    severity: "warning",
                });
                continue;
            }
            selected_names.insert(parsed.name.clone());
            selected.push((parsed.name.clone(), parsed));
        }
    }

    // toSorted((a, b) => a.name.localeCompare(b.name)); valid names are
    // ASCII [a-z0-9_:-] and ICU root collation orders them `_ < - < : <
    // digits < letters`, shorter prefix first — encoded below.
    selected.sort_by(|a, b| icul_compare_name(&a.0, &b.0));

    Ok(Outcome {
        commands: selected.into_iter().map(|(_, m)| m).collect(),
        diagnostics,
        total_discovered,
    })
}

fn icul_compare_name(a: &str, b: &str) -> std::cmp::Ordering {
    let rank = |c: char| -> u8 {
        match c {
            '_' => 0,
            '-' => 1,
            ':' => 2,
            '0'..='9' => 3,
            _ => 4, // letters
        }
    };
    let ac: Vec<char> = a.chars().collect();
    let bc: Vec<char> = b.chars().collect();
    let n = ac.len().min(bc.len());
    for i in 0..n {
        let (x, y) = (ac[i], bc[i]);
        if x == y {
            continue;
        }
        let (rx, ry) = (rank(x), rank(y));
        if rx != ry {
            return rx.cmp(&ry);
        }
        return x.cmp(&y);
    }
    ac.len().cmp(&bc.len())
}

fn resolve_roots(working_directory: &str) -> Vec<Root> {
    let mut roots: Vec<Root> = Vec::new();
    let mut priority = 0usize;
    let mut next_priority = || {
        priority += 10;
        priority
    };

    // User scope (~/.zcode/commands, ~/.agents/commands).
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    for (dir, source) in [(".zcode", "zcode"), (".agents", "agents")] {
        roots.push(Root {
            path: home.join(dir).join("commands"),
            scope: "user",
            source,
            priority: next_priority(),
        });
    }

    // Project scope: cwd up to the worktree root (inclusive); without a .git
    // marker anywhere, just the cwd.
    let dirs = project_directories(Path::new(working_directory));
    for dir in dirs {
        roots.push(Root {
            path: dir.join(".zcode").join("commands"),
            scope: "project",
            source: "zcode",
            priority: next_priority(),
        });
        roots.push(Root {
            path: dir.join(".agents").join("commands"),
            scope: "project",
            source: "agents",
            priority: next_priority(),
        });
    }
    // extraResolvedRoots (plugins): enabled plugins contribute command roots
    // after the user/project roots; priorities (1000+) place them last in the
    // priority sort, matching resolveDefaultCustomCommandRoots + toSorted.
    roots.extend(plugin_command_roots(working_directory));
    roots
}

fn project_directories(working_directory: &Path) -> Vec<PathBuf> {
    // findWorktreeRoot: nearest ancestor (inclusive) containing ".git".
    let mut current = working_directory.to_path_buf();
    let mut worktree_root: Option<PathBuf> = None;
    loop {
        if current.join(".git").exists() {
            worktree_root = Some(current.clone());
            break;
        }
        match current.parent() {
            Some(p) => current = p.to_path_buf(),
            None => break,
        }
    }
    let Some(root) = worktree_root else {
        return vec![working_directory.to_path_buf()];
    };
    let mut directories: Vec<PathBuf> = Vec::new();
    let mut current = working_directory.to_path_buf();
    loop {
        directories.push(current.clone());
        if current == root || current.parent().is_none() {
            break;
        }
        current = current.parent().unwrap().to_path_buf();
    }
    directories
}

fn command_files_under_root(root: &Path, diagnostics: &mut Vec<Diagnostic>) -> Vec<String> {
    let meta = match std::fs::metadata(root) {
        Ok(m) => m,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                diagnostics.push(Diagnostic {
                    code: "custom_command_scan_failed",
                    command_name: None,
                    message: node_stat_error(e, &root.to_string_lossy()),
                    path: Some(root.to_string_lossy().to_string()),
                    severity: "warning",
                });
            }
            return Vec::new();
        }
    };
    if !meta.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    scan_markdown_files(root, 0, &mut out, diagnostics);
    out
}

fn scan_markdown_files(
    directory: &Path,
    depth: usize,
    results: &mut Vec<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    let entries_raw = match std::fs::read_dir(directory) {
        Ok(e) => e,
        Err(e) => {
            diagnostics.push(Diagnostic {
                code: "custom_command_scan_failed",
                command_name: None,
                message: node_readdir_error(e, &directory.to_string_lossy()),
                path: Some(directory.to_string_lossy().to_string()),
                severity: "warning",
            });
            return;
        }
    };
    let mut entries: Vec<(std::ffi::OsString, bool, PathBuf)> = Vec::new();
    for entry in entries_raw {
        let Ok(entry) = entry else { continue };
        // Node's readdir on this platform surfaces names sorted; raw getdents
        // order differs, so sort bytewise to keep scan (and diagnostic) order.
        let is_symlink = entry.file_type().map(|t| t.is_symlink()).unwrap_or(false);
        entries.push((entry.file_name(), is_symlink, entry.path()));
    }
    entries.sort_by(|a, b| a.0.as_encoded_bytes().cmp(b.0.as_encoded_bytes()));
    for (name, is_symlink, path) in entries {
        let name_str = name.to_string_lossy();
        // Symlinks: stat the target to classify; broken link → skip.
        let meta = if is_symlink {
            match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            }
        } else {
            match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            }
        };
        if meta.is_dir() {
            scan_markdown_files(&path, depth + 1, results, diagnostics);
        } else if meta.is_file() && name_str.to_lowercase().ends_with(COMMAND_EXTENSION) {
            results.push(path.to_string_lossy().to_string());
        }
    }
}

fn parse_command(path: &str, root: &Root, diagnostics: &mut Vec<Diagnostic>) -> Option<Metadata> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return None;
            }
            diagnostics.push(Diagnostic {
                code: "custom_command_read_failed",
                command_name: None,
                message: node_read_error(e, path),
                path: Some(path.to_string()),
                severity: "warning",
            });
            return None;
        }
    };
    let raw_content = String::from_utf8_lossy(&bytes).into_owned();

    let name = command_name_from_path(path, &root.path.to_string_lossy());
    if !is_valid_command_name(&name) {
        diagnostics.push(Diagnostic {
            code: "custom_command_invalid_name",
            command_name: Some(name.clone()),
            message: format!("Invalid custom command name: {name}"),
            path: Some(path.to_string()),
            severity: "error",
        });
        return None;
    }

    let (frontmatter, body) = split_frontmatter(&raw_content);
    let (keys, values) = match &frontmatter {
        Some(fm) => parse_flat_yaml(fm, path, diagnostics),
        None => (Vec::new(), std::collections::HashMap::new()),
    };
    let description = match scalar(&keys, &values, "description") {
        Some(d) => Some(d),
        None => extract_description(&body),
    };
    let Some(description) = description else {
        diagnostics.push(Diagnostic {
            code: "custom_command_invalid_frontmatter",
            command_name: Some(name.clone()),
            message: format!("Custom command must include a description or non-empty body: {path}"),
            path: Some(path.to_string()),
            severity: "error",
        });
        return None;
    };

    for key in &keys {
        if SAFE_FRONTMATTER_KEYS.contains(&key.as_str()) {
            continue;
        }
        diagnostics.push(Diagnostic {
            code: "custom_command_unknown_frontmatter",
            command_name: Some(name.clone()),
            message: format!("Unknown custom command frontmatter key: {key}"),
            path: Some(path.to_string()),
            severity: "warning",
        });
    }

    Some(Metadata {
        allowed_tools: parse_list(&keys, &values, "allowed-tools"),
        argument_hint: scalar(&keys, &values, "argument-hint"),
        description: truncate_utf16(&description, MAX_DESCRIPTION_LENGTH),
        disable_non_interactive: {
            let v = scalar(&keys, &values, "disable-noninteractive");
            matches!(v.as_deref().map(|s| s.to_lowercase()).as_deref(), Some("true") | Some("yes"))
        },
        model: scalar(&keys, &values, "model"),
        name,
        path: path.to_string(),
        root_path: root.path.to_string_lossy().to_string(),
        scope: root.scope,
        skills: parse_list(&keys, &values, "skills"),
        source: root.source,
        frontmatter_keys: keys,
    })
}

// ============================================================
// Load (read body)
// ============================================================

fn read_command_body(path: &str, max_bytes: u64) -> Result<(usize, u64, bool, String), String> {
    let meta = std::fs::metadata(path).map_err(|e| node_stat_error(e, path))?;
    let size_bytes = meta.len();
    let truncated = size_bytes > max_bytes;
    let raw = if truncated {
        use std::io::Read;
        let mut f = std::fs::File::open(path).map_err(|e| node_open_error(e, path))?;
        let mut buf = vec![0u8; max_bytes as usize];
        let mut read_total = 0usize;
        loop {
            let n = f.read(&mut buf[read_total..]).map_err(|e| node_read_error(e, path))?;
            if n == 0 {
                break;
            }
            read_total += n;
            if read_total == buf.len() {
                break;
            }
        }
        buf.truncate(read_total);
        buf
    } else {
        std::fs::read(path).map_err(|e| node_read_error(e, path))?
    };
    let bytes_read = raw.len();
    let content = js_trim(&strip_frontmatter(&String::from_utf8_lossy(&raw)));
    Ok((bytes_read, size_bytes, truncated, content))
}

// ============================================================
// Naming
// ============================================================

fn normalize_command_name(name: &str) -> String {
    let trimmed = js_trim(name);
    let stripped = trimmed.trim_start_matches('/');
    stripped.to_lowercase()
}

fn command_name_from_path(path: &str, root_path: &str) -> String {
    // relative(rootPath, path): paths are built from rootPath, so strip prefix.
    let rel = path
        .strip_prefix(root_path)
        .unwrap_or(path)
        .trim_start_matches('/');
    let without_extension = &rel[..rel.len().saturating_sub(COMMAND_EXTENSION.len())];
    let joined = split_path_separators(without_extension);
    normalize_command_name(&joined)
}

fn split_path_separators(s: &str) -> String {
    // split(/[\\/]+/).join(":")
    let mut out = String::new();
    let mut prev_sep = true; // also collapses a leading separator run
    for c in s.chars() {
        if c == '/' || c == '\\' {
            if !prev_sep {
                out.push(':');
                prev_sep = true;
            }
        } else {
            out.push(c);
            prev_sep = false;
        }
    }
    out
}

fn is_valid_command_name(name: &str) -> bool {
    // ^[a-z0-9][a-z0-9_:-]{0,63}$
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    let rest: Vec<char> = chars.collect();
    if rest.len() > 63 {
        return false;
    }
    rest.iter()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_' || *c == ':' || *c == '-')
}

// ============================================================
// Frontmatter (extract / strip / flat YAML)
// ============================================================

fn split_frontmatter(content: &str) -> (Option<String>, String) {
    // Returns (frontmatter | None, body-after-frontmatter-or-original).
    let normalized = content.strip_prefix('\u{feff}').unwrap_or(content);
    if !normalized.starts_with("---") {
        // TS returns the ORIGINAL content here, BOM included.
        return (None, content.to_string());
    }
    let lines = js_lines(normalized);
    if js_trim(lines.first().unwrap_or(&String::new())) != "---" {
        return (None, content.to_string());
    }
    let end_index = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, l)| js_trim(l) == "---")
        .map(|(i, _)| i);
    match end_index {
        Some(e) if e > 0 => (
            Some(lines[1..e].join("\n")),
            lines[e + 1..].join("\n"),
        ),
        _ => (None, content.to_string()),
    }
}

fn strip_frontmatter(content: &str) -> String {
    split_frontmatter(content).1
}

fn parse_flat_yaml(
    frontmatter: &str,
    path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Vec<String>, std::collections::HashMap<String, String>) {
    let mut values = std::collections::HashMap::new();
    let mut keys: Vec<String> = Vec::new();
    for (index, line) in js_lines(frontmatter).into_iter().enumerate() {
        let t = js_trim(&line);
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if line.chars().next().is_some_and(is_js_ws) {
            continue; // /^\s/ — indented lines are skipped
        }
        let separator = line.find(':');
        match separator {
            None | Some(0) => {
                let base = Path::new(path)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default();
                diagnostics.push(Diagnostic {
                    code: "custom_command_invalid_frontmatter",
                    command_name: None,
                    message: format!("Invalid frontmatter line {} in {base}", index + 1),
                    path: Some(path.to_string()),
                    severity: "warning",
                });
            }
            Some(sep) => {
                let key = js_trim(&line[..sep]);
                keys.push(key.clone());
                values.insert(key, js_trim(&line[sep + 1..]));
            }
        }
    }
    (keys, values)
}

fn scalar(keys: &[String], values: &std::collections::HashMap<String, String>, key: &str) -> Option<String> {
    keys.iter().find(|k| k.as_str() == key).and_then(|_| values.get(key)).and_then(|v| parse_scalar(v))
}

fn parse_scalar(value: &str) -> Option<String> {
    let trimmed = js_trim(value);
    if trimmed.is_empty() {
        return None;
    }
    let bytes: Vec<char> = trimmed.chars().collect();
    if bytes.len() >= 1 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == '"' || first == '\'') && first == last {
            // JS slice(1, -1) on a 1-char string yields "".
            let inner: String = if bytes.len() >= 2 {
                bytes[1..bytes.len() - 1].iter().collect()
            } else {
                String::new()
            };
            return Some(js_trim(&inner));
        }
    }
    Some(trimmed)
}

fn parse_list(keys: &[String], values: &std::collections::HashMap<String, String>, key: &str) -> Vec<String> {
    let Some(scalar) = scalar(keys, values, key) else {
        return Vec::new();
    };
    // replace(/^\[/,"").replace(/\]$/,"") — one leading '[' and one trailing ']'.
    let stripped = scalar.strip_prefix('[').unwrap_or(&scalar);
    let stripped = stripped.strip_suffix(']').unwrap_or(stripped);
    stripped
        .split(',')
        .map(js_trim)
        .filter(|s| !s.is_empty())
        .collect()
}

fn extract_description(body: &str) -> Option<String> {
    for line in js_lines(body) {
        // replace(/^#+\s*/, "") — only fires when the line STARTS with '#'.
        let mut candidate = if line.starts_with('#') {
            line.trim_start_matches('#').trim_start_matches(is_js_ws).to_string()
        } else {
            line.clone()
        };
        // replace(/^[-*]\s*/, "") — only when the (possibly still indented)
        // result starts with '-' or '*'.
        if candidate.starts_with('-') || candidate.starts_with('*') {
            candidate = candidate[1..].trim_start_matches(is_js_ws).to_string();
        }
        candidate = js_trim(&candidate);
        if !candidate.is_empty() {
            return Some(truncate_utf16(&candidate, MAX_DESCRIPTION_LENGTH));
        }
    }
    None
}

fn truncate_utf16(value: &str, max_units: usize) -> String {
    let mut units = 0usize;
    let mut out = String::new();
    for c in value.chars() {
        let len = if (c as u32) > 0xFFFF { 2 } else { 1 };
        if units + len > max_units {
            break;
        }
        units += len;
        out.push(c);
    }
    out
}

// ============================================================
// JS string/regex semantics helpers
// ============================================================

fn is_js_ws(c: char) -> bool {
    // JS \s = Unicode White_Space + U+FEFF.
    c.is_whitespace() || c == '\u{feff}'
}

fn js_trim(s: &str) -> String {
    s.trim_matches(is_js_ws).to_string()
}

fn js_lines(s: &str) -> Vec<String> {
    // split(/\r?\n/)
    s.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).map(String::from).collect()
}

// ============================================================
// Node-style OS error strings (best effort, diagnostics only)
// ============================================================

fn errno_code(e: &std::io::Error) -> (&'static str, String) {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => ("EACCES", "permission denied".into()),
        std::io::ErrorKind::NotFound => ("ENOENT", "no such file or directory".into()),
        std::io::ErrorKind::AlreadyExists => ("EEXIST", "file already exists".into()),
        std::io::ErrorKind::InvalidInput => ("EINVAL", "invalid argument".into()),
        _ => ("EIO", "input/output error".into()),
    }
}

fn node_stat_error(e: std::io::Error, path: &str) -> String {
    let (code, msg) = errno_code(&e);
    format!("{code}: {msg}, stat '{path}'")
}

fn node_readdir_error(e: std::io::Error, path: &str) -> String {
    let (code, msg) = errno_code(&e);
    format!("{code}: {msg}, scandir '{path}'")
}

fn node_read_error(e: std::io::Error, path: &str) -> String {
    let (code, msg) = errno_code(&e);
    format!("{code}: {msg}, open '{path}'")
}

fn node_open_error(e: std::io::Error, path: &str) -> String {
    let (code, msg) = errno_code(&e);
    format!("{code}: {msg}, open '{path}'")
}

// ============================================================
// Disabled paths (config command overrides)
// ============================================================

fn collect_disabled_paths(working_directory: &str) -> BTreeSet<String> {
    // Merge order (lowest → highest): user config, project configs root→cwd.
    let mut overrides: Vec<(String, bool)> = Vec::new();
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() {
        read_command_overrides(&PathBuf::from(&home).join(".zcode").join("cli").join("config.json"), &mut overrides);
    }
    let mut current = PathBuf::from(working_directory);
    let mut dirs: Vec<PathBuf> = Vec::new();
    loop {
        dirs.push(current.clone());
        if current.join(".git").exists() || current.parent().is_none() {
            break;
        }
        current = current.parent().unwrap().to_path_buf();
    }
    dirs.reverse();
    for dir in dirs {
        read_command_overrides(&dir.join("zcode.json"), &mut overrides);
        read_command_overrides(&dir.join(".zcode").join("config.json"), &mut overrides);
    }
    overrides
        .into_iter()
        .filter(|(_, enable)| !enable)
        .map(|(path, _)| resolve_against(working_directory, &path))
        .collect()
}

fn read_command_overrides(path: &Path, overrides: &mut Vec<(String, bool)>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    let Some(command) = value.get("command").and_then(|v| v.as_object()) else {
        return;
    };
    for (p, spec) in command {
        let enable = spec.get("enable").and_then(|v| v.as_bool());
        if let Some(enable) = enable {
            overrides.push((p.clone(), enable));
        }
    }
}

fn resolve_against(base: &str, path: &str) -> String {
    if Path::new(path).is_absolute() {
        return path.to_string();
    }
    PathBuf::from(base).join(path).to_string_lossy().to_string()
}

// ============================================================
// Plugin command roots (bootstrap/custom-commands.ts extraResolvedRoots)
// ============================================================
// Mirror of the plugins half of bootstrap/plugins.ts + plugins_cmd.rs (kept
// self-contained: this module must not edit plugins_cmd.rs), reduced to the
// surfaces the commands path consumes: enabled-plugin set and command roots.
// Plugin-pipeline diagnostics are dropped by the TS commands path, so the
// candidate/manifest steps here skip silently on failure, exactly like TS.

const ZCODE_OFFICIAL_MARKETPLACE: &str = "zcode-plugins-official";
const ZCODE_INLINE_MARKETPLACE: &str = "inline";
const LEGACY_CUA_PLUGIN_ID: &str = "zcode-cua@zcode-plugins-official";
const CANONICAL_CUA_PLUGIN_ID: &str = "computer-use@zcode-plugins-official";
const PLUGIN_DEFAULT_VERSION: &str = "0.0.0";
const FIRST_PLUGIN_PRIORITY: usize = 1_000;
const PLUGIN_PRIORITY_STEP: usize = 10;

/// bootstrap/app/official-plugin-definitions.ts: definitions with defaultEnabled.
const DEFAULT_ENABLED_OFFICIAL_IDS: &[&str] = &[
    "node-repl-host@zcode-plugins-official",
    "browser-use@zcode-plugins-official",
    "documents@zcode-plugins-official",
    "pdf@zcode-plugins-official",
    "presentations@zcode-plugins-official",
    "spreadsheets@zcode-plugins-official",
    "image-search@zcode-plugins-official",
    "plugin-creator@zcode-plugins-official",
    "skill-creator@zcode-plugins-official",
    "zcode-guide@zcode-plugins-official",
];

struct PluginConfig {
    enabled: bool,
    dirs: Vec<String>,
    enabled_plugins: std::collections::HashMap<String, bool>,
    suppressed_builtins: Vec<String>,
}

fn plugin_json_str(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(|s| s.to_string())
}

fn canonicalize_plugin_id(id: &str) -> String {
    if id == LEGACY_CUA_PLUGIN_ID {
        CANONICAL_CUA_PLUGIN_ID.to_string()
    } else {
        id.to_string()
    }
}

fn parse_file_config(path: &Path) -> Option<Map<String, Value>> {
    let text = std::fs::read_to_string(path).ok()?;
    let parsed = serde_json::from_str::<Value>(&text).ok()?;
    parsed.as_object().cloned()
}

/// schema.ts normalizePluginConfig: canonicalize CUA ids in one loaded file.
fn extract_plugin_patch(file: &Map<String, Value>) -> Option<Map<String, Value>> {
    let plugins = file.get("plugins")?.as_object()?.clone();
    let mut plugins = plugins;
    if let Some(Value::Object(enabled)) = plugins.get_mut("enabledPlugins") {
        if let Some(v) = enabled.remove(LEGACY_CUA_PLUGIN_ID) {
            enabled
                .entry(CANONICAL_CUA_PLUGIN_ID.to_string())
                .or_insert(v);
        }
    }
    if let Some(Value::Array(suppressed)) = plugins.get_mut("suppressedBuiltins") {
        let mut seen: Vec<String> = Vec::new();
        let mut out: Vec<Value> = Vec::new();
        for item in suppressed.iter() {
            let Some(s) = item.as_str() else { continue };
            let canonical = canonicalize_plugin_id(s);
            if canonical == CANONICAL_CUA_PLUGIN_ID && seen.contains(&canonical) {
                continue;
            }
            seen.push(canonical.clone());
            out.push(Value::String(canonical));
        }
        *suppressed = out;
    }
    Some(plugins)
}

fn merge_plugin_patch(base: &mut PluginConfig, patch: &Map<String, Value>) {
    if let Some(v) = patch.get("enabled").and_then(Value::as_bool) {
        base.enabled = v;
    }
    if let Some(Value::Array(dirs)) = patch.get("dirs") {
        for d in dirs {
            if let Some(s) = d.as_str() {
                if !base.dirs.iter().any(|x| x == s) {
                    base.dirs.push(s.to_string());
                }
            }
        }
    }
    if let Some(Value::Object(enabled)) = patch.get("enabledPlugins") {
        for (k, v) in enabled {
            if let Some(b) = v.as_bool() {
                base.enabled_plugins.insert(k.clone(), b);
            }
        }
    }
    // suppressedBuiltins is replaced wholesale by the higher-priority layer.
    if let Some(Value::Array(suppressed)) = patch.get("suppressedBuiltins") {
        base.suppressed_builtins = suppressed
            .iter()
            .filter_map(|v| v.as_str().map(|s| canonicalize_plugin_id(s)))
            .collect();
    }
}

/// Directories walked by shared/workspace-hook-config.ts getProjectConfigDirectories.
fn plugin_project_config_directories(start: &Path) -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = Vec::new();
    let mut current = start.to_path_buf();
    loop {
        directories.push(current.clone());
        if current.join(".git").exists() {
            directories.reverse();
            return directories;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    vec![start.to_path_buf()]
}

fn plugin_normalize_abs(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    if p.is_absolute() {
        out.push("/");
    } else {
        out = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    }
    for comp in p.components() {
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

fn plugin_resolve_path(raw: &str, home: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        plugin_normalize_abs(&PathBuf::from(home).join(rest))
    } else if raw == "~" {
        PathBuf::from(home)
    } else {
        plugin_normalize_abs(Path::new(raw))
    }
}

/// plugins_cmd.rs load_plugin_context, reduced to (plugins root, plugin config).
fn load_plugin_context(working_directory: &str) -> (PathBuf, PluginConfig) {
    let home = std::env::var("HOME").unwrap_or_default();
    let user_config_path = PathBuf::from(&home).join(".zcode/cli/config.json");

    let mut cfg = PluginConfig {
        enabled: true,
        dirs: Vec::new(),
        enabled_plugins: std::collections::HashMap::new(),
        suppressed_builtins: Vec::new(),
    };

    let mut storage_dir: Option<String> = None;

    let user_file = parse_file_config(&user_config_path);
    if let Some(user) = &user_file {
        if let Some(patch) = extract_plugin_patch(user) {
            merge_plugin_patch(&mut cfg, &patch);
        }
        storage_dir = plugin_json_str(user.get("storage").and_then(|s| s.get("dir")));
    }

    // Project configs: nearest-cwd wins (walk is root-most first, merged in order).
    for directory in plugin_project_config_directories(Path::new(working_directory)) {
        for candidate in [
            directory.join("zcode.json"),
            directory.join(".zcode").join("config.json"),
        ] {
            if !candidate.exists() {
                continue;
            }
            if let Some(file) = parse_file_config(&candidate) {
                let mut patch = extract_plugin_patch(&file).unwrap_or_default();
                // Project layer cannot declare extraKnownMarketplaces (unused here).
                patch.remove("extraKnownMarketplaces");
                merge_plugin_patch(&mut cfg, &patch);
                if let Some(dir) = plugin_json_str(file.get("storage").and_then(|s| s.get("dir"))) {
                    storage_dir = Some(dir);
                }
            }
        }
    }

    if let Ok(dir) = std::env::var("ZCODE_STORAGE_DIR") {
        storage_dir = Some(dir);
    }

    let raw = storage_dir.unwrap_or_else(|| "~/.zcode".to_string());
    let storage_root = plugin_resolve_path(&raw, &home);
    let cli_root = if storage_root.file_name().and_then(|s| s.to_str()) == Some("cli") {
        storage_root
    } else {
        storage_root.join("cli")
    };
    (cli_root.join("plugins"), cfg)
}

struct PluginCandidate {
    root_path: PathBuf,
    marketplace: String,
    source: &'static str,
    default_enabled: bool,
}

/// adapters/plugins/index.ts resolveCandidates (officialPluginRoots seeding is
/// Node-bundle-owned; normal installs contribute zero such roots — same as
/// plugins_cmd.rs collect_candidates).
fn collect_plugin_candidates(
    plugins_root: &Path,
    cfg: &PluginConfig,
) -> Vec<PluginCandidate> {
    let mut candidates: Vec<PluginCandidate> = Vec::new();
    let home = std::env::var("HOME").unwrap_or_default();
    for dir in &cfg.dirs {
        candidates.push(PluginCandidate {
            root_path: plugin_resolve_path(dir, &home),
            marketplace: ZCODE_INLINE_MARKETPLACE.into(),
            source: "inline",
            default_enabled: true,
        });
    }
    candidates.extend(scan_official_cache(plugins_root).into_iter().map(|root| PluginCandidate {
        root_path: root,
        marketplace: ZCODE_OFFICIAL_MARKETPLACE.into(),
        source: "official",
        default_enabled: false,
    }));
    for record in list_installed_records(plugins_root) {
        let root = match &record.install_path {
            Some(p) => PathBuf::from(p),
            None => plugins_root
                .join("cache")
                .join(sanitize_plugin_id(&record.marketplace))
                .join(sanitize_plugin_id(&record.name))
                .join(sanitize_plugin_id(&record.version)),
        };
        candidates.push(PluginCandidate {
            root_path: root,
            marketplace: record.marketplace.clone(),
            source: "cache",
            default_enabled: false,
        });
    }
    candidates
}

fn scan_official_cache(plugins_root: &Path) -> Vec<PathBuf> {
    // bundled-marketplace.json is the authoritative list when present.
    let partition = plugins_root
        .join("marketplaces")
        .join(ZCODE_OFFICIAL_MARKETPLACE)
        .join("bundled-marketplace.json");
    if let Ok(text) = std::fs::read_to_string(&partition) {
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            let obj = value.as_object();
            let version_ok = obj.and_then(|o| o.get("version")).and_then(Value::as_u64) == Some(1);
            let manifest = obj.and_then(|o| o.get("manifest")).and_then(Value::as_object);
            if version_ok {
                if let Some(manifest) = manifest {
                    let official_cache_root =
                        plugins_root.join("cache").join(ZCODE_OFFICIAL_MARKETPLACE);
                    let mut roots = Vec::new();
                    if let Some(Value::Array(plugins)) = manifest.get("plugins") {
                        for plugin in plugins {
                            let Some(p) = plugin.as_object() else { continue };
                            let (Some(_name), Some(cache_path)) =
                                (plugin_json_str(p.get("name")), plugin_json_str(p.get("cachePath")))
                            else {
                                continue;
                            };
                            let plugin_cache_root =
                                official_cache_root.join(_name);
                            let resolved = PathBuf::from(&cache_path);
                            if !is_strict_descendant(&official_cache_root, &plugin_cache_root)
                                || !is_strict_descendant(&plugin_cache_root, &resolved)
                            {
                                continue;
                            }
                            roots.push(resolved);
                        }
                    }
                    return roots;
                }
            }
        }
    }

    let cache_root = plugins_root.join("cache").join(ZCODE_OFFICIAL_MARKETPLACE);
    let mut roots = Vec::new();
    let Ok(entries) = std::fs::read_dir(&cache_root) else {
        return roots;
    };
    for plugin_entry in entries.flatten() {
        if !plugin_entry.path().is_dir() {
            continue;
        }
        let Ok(versions) = std::fs::read_dir(plugin_entry.path()) else {
            continue;
        };
        for version_entry in versions.flatten() {
            if version_entry.path().is_dir() {
                roots.push(version_entry.path());
            }
        }
    }
    roots
}

fn is_strict_descendant(parent: &Path, child: &Path) -> bool {
    match child.strip_prefix(parent) {
        Ok(rel) => rel.components().next().is_some(),
        Err(_) => false,
    }
}

struct InstalledRecord {
    name: String,
    marketplace: String,
    version: String,
    install_path: Option<String>,
}

fn list_installed_records(plugins_root: &Path) -> Vec<InstalledRecord> {
    let mut records = Vec::new();
    let path = plugins_root.join("installed_plugins.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return records;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return records;
    };
    let Some(Value::Array(plugins)) = value.get("plugins") else {
        return records;
    };
    for entry in plugins {
        let Some(obj) = entry.as_object() else { continue };
        let (Some(_id), Some(name), Some(marketplace), Some(version)) = (
            plugin_json_str(obj.get("id")),
            plugin_json_str(obj.get("name")),
            plugin_json_str(obj.get("marketplace")),
            plugin_json_str(obj.get("version")),
        ) else {
            continue;
        };
        records.push(InstalledRecord {
            name,
            marketplace,
            version,
            install_path: plugin_json_str(obj.get("installPath")),
        });
    }
    records
}

fn sanitize_plugin_id(plugin_id: &str) -> String {
    plugin_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '@' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

struct LoadedPlugin {
    id: String,
    /// Raw manifest object (insertion-ordered via serde_json preserve_order).
    raw: Map<String, Value>,
    root_path: PathBuf,
    source: &'static str,
}

fn plugin_load(candidate: &PluginCandidate) -> Option<LoadedPlugin> {
    let root = &candidate.root_path;
    if !root.is_dir() {
        return None;
    }
    let manifest_path = [
        root.join(".zcode-plugin").join("plugin.json"),
        root.join(".claude-plugin").join("plugin.json"),
        root.join(".codex-plugin").join("plugin.json"),
    ]
    .into_iter()
    .find(|p| p.is_file())?;
    let Ok(text) = std::fs::read_to_string(&manifest_path) else {
        return None;
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&text) else {
        return None;
    };
    let Some(obj) = parsed.as_object() else {
        return None;
    };
    let name = plugin_json_str(obj.get("name")).unwrap_or_default();
    let name = name.trim().to_string();
    if !is_valid_plugin_name(&name) {
        return None;
    }
    Some(LoadedPlugin {
        id: format!("{}@{}", name, candidate.marketplace),
        raw: obj.clone(),
        root_path: root.clone(),
        source: candidate.source,
    })
}

fn is_valid_plugin_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 128 {
        return false;
    }
    let first = bytes[0];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
}

fn plugin_parse_path_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(items)) => {
            items.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
        }
        _ => Vec::new(),
    }
}

fn directory_exists(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
}

fn file_exists(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.is_file()).unwrap_or(false)
}

/// helpers.ts resolveInside: reject absolute paths and lexical escapes.
fn plugin_resolve_inside(root: &Path, raw: &str) -> Option<PathBuf> {
    let raw_path = Path::new(raw);
    if raw_path.is_absolute() {
        return None;
    }
    let resolved = plugin_normalize_abs(&root.join(raw_path));
    let rel = plugin_path_relative(root, &resolved);
    if rel.is_empty() || (!rel.starts_with("..") && !rel.contains("../")) {
        Some(resolved)
    } else {
        None
    }
}

fn plugin_path_relative(from: &Path, to: &Path) -> String {
    let from_parts: Vec<_> = from.components().filter(|c| matches!(c, Component::Normal(_))).collect();
    let to_parts: Vec<_> = to.components().filter(|c| matches!(c, Component::Normal(_))).collect();
    let mut common = 0;
    while common < from_parts.len() && common < to_parts.len() && from_parts[common] == to_parts[common] {
        common += 1;
    }
    let mut segments: Vec<String> = Vec::new();
    for _ in common..from_parts.len() {
        segments.push("..".into());
    }
    for part in &to_parts[common..] {
        segments.push(part.as_os_str().to_string_lossy().into_owned());
    }
    segments.join("/")
}

/// resolveComponentRoots("commands") + materializeCommandMetadataRoot for one
/// enabled plugin, at the plugin's priority slot.
fn plugin_command_roots_for(
    loaded: &LoadedPlugin,
    data_path: &Path,
    priority: usize,
) -> Vec<Root> {
    let scope: &'static str = if loaded.source == "official" { "system" } else { "user" };
    let mut roots: Vec<Root> = Vec::new();

    // resolveComponentRoots("commands")
    let mut paths = plugin_parse_path_list(loaded.raw.get("commands"));
    let default_path = loaded.root_path.join("commands");
    if directory_exists(&default_path) {
        paths.insert(0, "commands".to_string());
    }
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for raw_path in paths {
        let Some(path) = plugin_resolve_inside(&loaded.root_path, &raw_path) else {
            continue;
        };
        let key = path.to_string_lossy().into_owned();
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);
        roots.push(Root {
            path,
            scope,
            source: "plugin",
            priority,
        });
    }

    // materializeCommandMetadataRoot (manifest.commands as metadata record).
    if let Some(root) = materialize_command_metadata_root(loaded, data_path, priority + 2, scope) {
        roots.push(root);
    }
    roots
}

/// materializeCommandMetadataRoot: manifest.commands as an object map of
/// {description, argumentHint, model, allowedTools, source|content} entries,
/// written as .md files under <dataPath>/generated-commands (a real side
/// effect of every TS discovery pass).
fn materialize_command_metadata_root(
    loaded: &LoadedPlugin,
    data_path: &Path,
    priority: usize,
    scope: &'static str,
) -> Option<Root> {
    let Some(spec) = loaded.raw.get("commands").and_then(Value::as_object) else {
        return None;
    };

    let generated_root = data_path.join("generated-commands");
    let _ = std::fs::create_dir_all(&generated_root);
    let mut wrote_command = false;

    for (raw_name, raw_metadata) in spec {
        let Some(metadata) = raw_metadata.as_object() else {
            continue;
        };
        let Some(name) = normalize_generated_command_name(raw_name) else {
            continue;
        };
        let source = plugin_json_str(metadata.get("source"));
        let content = plugin_json_str(metadata.get("content"));
        if (source.is_some() && content.is_some()) || (source.is_none() && content.is_none()) {
            continue;
        }

        let markdown = if let Some(source) = &source {
            // trimRelativePrefix: strip one leading "./".
            let trimmed = source.strip_prefix("./").unwrap_or(source);
            let Some(source_path) = plugin_resolve_inside(&loaded.root_path, trimmed) else {
                continue;
            };
            if !file_exists(&source_path) {
                continue;
            }
            match std::fs::read_to_string(&source_path) {
                Ok(m) => m,
                Err(_) => continue,
            }
        } else {
            content.unwrap()
        };

        let out_path = generated_root.join(format!("{name}.md"));
        if std::fs::write(&out_path, apply_command_metadata_frontmatter(markdown, metadata)).is_ok()
        {
            wrote_command = true;
        }
    }

    wrote_command.then(|| Root {
        path: generated_root,
        scope,
        source: "plugin",
        priority,
    })
}

fn normalize_generated_command_name(name: &str) -> Option<String> {
    let normalized = js_trim(name).trim_start_matches('/').to_lowercase();
    let mut chars = normalized.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return None,
    }
    let rest: Vec<char> = chars.collect();
    if rest.len() > 63 {
        return None;
    }
    if !rest.iter()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_' || *c == ':' || *c == '-')
    {
        return None;
    }
    Some(normalized)
}

fn apply_command_metadata_frontmatter(markdown: String, metadata: &Map<String, Value>) -> String {
    let mut frontmatter: Vec<(String, String)> = Vec::new();
    if let Some(d) = plugin_json_str(metadata.get("description")) {
        if !js_trim(&d).is_empty() {
            frontmatter.push(("description".into(), js_trim(&d)));
        }
    }
    if let Some(h) = plugin_json_str(metadata.get("argumentHint")) {
        if !js_trim(&h).is_empty() {
            frontmatter.push(("argument-hint".into(), js_trim(&h)));
        }
    }
    if let Some(m) = plugin_json_str(metadata.get("model")) {
        if !js_trim(&m).is_empty() {
            frontmatter.push(("model".into(), js_trim(&m)));
        }
    }
    if let Some(Value::Array(tools)) = metadata.get("allowedTools") {
        let allowed: Vec<String> = tools
            .iter()
            .filter_map(|t| t.as_str())
            .map(|t| js_trim(t))
            .filter(|t| !t.is_empty())
            .collect();
        if !allowed.is_empty() {
            frontmatter.push(("allowed-tools".into(), allowed.join(", ")));
        }
    }
    if frontmatter.is_empty() {
        return markdown;
    }
    let body = strip_markdown_frontmatter(&markdown);
    let body = body.trim_start_matches(is_js_ws);
    format!(
        "---\n{}\n---\n\n{}",
        frontmatter
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join("\n"),
        body
    )
}

fn strip_markdown_frontmatter(markdown: &str) -> String {
    let normalized = markdown.strip_prefix('\u{feff}').unwrap_or(markdown);
    if !normalized.starts_with("---") {
        return markdown.to_string();
    }
    let lines = js_lines(normalized);
    if js_trim(lines.first().unwrap_or(&String::new())) != "---" {
        return markdown.to_string();
    }
    let end_index = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, l)| js_trim(l) == "---")
        .map(|(i, _)| i);
    match end_index {
        Some(e) if e > 0 => lines[e + 1..].join("\n"),
        _ => markdown.to_string(),
    }
}

/// bootstrap/custom-commands.ts createCustomCommandDiscovery → resolveZCodePlugins
/// → pluginOutcome.commandRoots, reduced to command roots for enabled plugins.
fn plugin_command_roots(working_directory: &str) -> Vec<Root> {
    let (plugins_root, cfg) = load_plugin_context(working_directory);
    if !cfg.enabled {
        return Vec::new();
    }
    let mut roots = Vec::new();
    let candidates = collect_plugin_candidates(&plugins_root, &cfg);
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut priority = FIRST_PLUGIN_PRIORITY;
    for candidate in candidates {
        let Some(loaded) = plugin_load(&candidate) else {
            continue;
        };
        // Suppressed builtins are filtered at the discovery layer by authority id.
        if loaded.source == "official"
            && cfg.suppressed_builtins.iter().any(|s| *s == loaded.id)
        {
            continue;
        }
        if seen.contains(&loaded.id) {
            continue;
        }
        seen.insert(loaded.id.clone());
        let default_enabled =
            candidate.default_enabled || DEFAULT_ENABLED_OFFICIAL_IDS.contains(&loaded.id.as_str());
        let enabled = cfg
            .enabled_plugins
            .get(&loaded.id)
            .copied()
            .unwrap_or(default_enabled);
        if enabled {
            // resolveEnabledComponents creates the data dir for enabled plugins.
            let data_path = plugins_root.join("data").join(sanitize_plugin_id(&loaded.id));
            let _ = std::fs::create_dir_all(&data_path);
            roots.extend(plugin_command_roots_for(&loaded, &data_path, priority + 1));
        }
        priority += PLUGIN_PRIORITY_STEP;
    }
    roots
}
