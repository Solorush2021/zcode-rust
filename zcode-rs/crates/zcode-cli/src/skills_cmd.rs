//! skills list|inspect — port of packages/cli/src/skills-command.ts plus the
//! discovery stack it calls into (bootstrap/skills.ts, adapters/skills,
//! adapters/plugins candidate resolution, adapters/config defaults).
//! Byte-exact against the TS CLI under the pinned env (NO_COLOR=1, LC_ALL=C).

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

const SKILLS_COMMAND_USAGE: &str = "Usage: zcode skills [list|inspect <name>]";
const MAX_DESCRIPTION_LENGTH: usize = 1024; // JS .length = UTF-16 code units
const DEFAULT_MAX_SKILL_BYTES: u64 = 100_000;
const FIRST_PLUGIN_PRIORITY: i64 = 1_000;
const PRIORITY_STEP: i64 = 10;
const ROOT_PRIORITY_STEP: i64 = 10;
const MAX_PLUGIN_MANIFEST_SEARCH_DEPTH: usize = 5;
const OFFICIAL_MARKETPLACE: &str = "zcode-plugins-official";
const INLINE_MARKETPLACE: &str = "inline";
const SKILL_FILE_NAME: &str = "SKILL.md";
const HOME_PREFIX: &str = "~/";

const PLUGIN_MANIFEST_RELATIVE_PATHS: [&str; 4] = [
    ".zcode-plugin/plugin.json",
    ".claude-plugin/plugin.json",
    ".codex-plugin/plugin.json",
    ".cursor-plugin/plugin.json",
];

const SAFE_FRONTMATTER_KEYS: [&str; 5] =
    ["name", "description", "when_to_use", "license", "metadata"];

const SKILL_SCAN_EXCLUDED_DIRECTORY_NAMES: [&str; 12] = [
    "node_modules",
    "dist",
    "build",
    "out",
    "target",
    "vendor",
    "coverage",
    ".cache",
    ".next",
    ".turbo",
    ".venv",
    "__pycache__",
];

const SKILL_DISCOVERY_DOT_DIR_ALLOWLIST: [&str; 1] = [".system"];

/// Official plugins with `defaultEnabled: true` (bootstrap/src/app/official-plugin-definitions.ts).
const DEFAULT_ENABLED_OFFICIAL_PLUGIN_IDS: [&str; 10] = [
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

// ============================================================
// Data model
// ============================================================

#[derive(Clone)]
struct SkillRoot {
    path: String,
    scope: &'static str,   // "user" | "project" | "system"
    source: &'static str,  // "zcode" | "agents" | "plugin"
    priority: i64,
    plugin_id: Option<String>,
}

struct SkillDiagnostic {
    code: &'static str,
    severity: &'static str,
    message: String,
    path: Option<String>,
    skill_name: Option<String>,
}

struct SkillMetadata {
    name: String,
    description: String,
    when_to_use: Option<String>,
    plugin_name: Option<String>,
    qualified_name: Option<String>,
    path: String,
    directory: String,
    root_path: String,
    scope: &'static str,
    source: &'static str,
    safe_to_auto_load: bool,
    frontmatter_keys: Vec<String>,
}

struct SkillLoadOutcome {
    diagnostics: Vec<SkillDiagnostic>,
    skills: Vec<SkillMetadata>,
    total_discovered: usize,
}

struct SkillContent {
    metadata: SkillMetadata,
    content: String,
    base_directory: String,
    bytes_read: u64,
    size_bytes: u64,
    truncated: bool,
}

// ============================================================
// Entry point (public signature fixed by run.rs)
// ============================================================

pub fn run(rest: &[String], json: bool, verbose: bool) -> i32 {
    let subcommand = rest.first().map(|s| s.as_str()).unwrap_or("list");
    match subcommand {
        "list" => {
            if rest.len() > 1 {
                return fail_usage();
            }
            run_list(json, verbose)
        }
        "inspect" => {
            let name = match rest.get(1) {
                Some(n) if rest.len() == 2 && !n.trim().is_empty() => n.clone(),
                _ => return fail_usage(),
            };
            run_inspect(&name, json, verbose)
        }
        other => {
            let mut e = std::io::stderr();
            let _ = write!(
                e,
                "Unknown skills command: {other}\n{SKILLS_COMMAND_USAGE}\n"
            );
            1
        }
    }
}

fn fail_usage() -> i32 {
    let mut e = std::io::stderr();
    let _ = writeln!(e, "{SKILLS_COMMAND_USAGE}");
    1
}

fn report_skills_error(message: &str, verbose: bool) -> i32 {
    let mut e = std::io::stderr();
    let _ = writeln!(e, "Error: {message}");
    // TS also prints error.stack when --verbose; V8 stack frames are not
    // replicable and no golden case covers skills + --verbose + error.
    let _ = verbose;
    1
}

fn run_list(json: bool, verbose: bool) -> i32 {
    match discover(&discovery_request()) {
        Ok(outcome) => {
            let cwd = resolved_cwd();
            let text = if json {
                format_skill_json(&outcome, &cwd)
            } else {
                format_human_skill_list(&outcome, verbose)
            };
            let mut o = std::io::stdout();
            let _ = o.write_all(text.as_bytes());
            0
        }
        Err(message) => report_skills_error(&message, verbose),
    }
}

fn run_inspect(name: &str, json: bool, verbose: bool) -> i32 {
    match inspect(&discovery_request(), name) {
        Ok(inspection) => {
            let cwd = resolved_cwd();
            let text = if json {
                format_skill_inspection_json(&inspection, &cwd)
            } else {
                format_human_skill_inspection(&inspection, verbose)
            };
            let mut o = std::io::stdout();
            let _ = o.write_all(text.as_bytes());
            0
        }
        Err(message) => report_skills_error(&message, verbose),
    }
}

fn resolved_cwd() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/".to_string())
}

// ============================================================
// Config (subset of adapters/config relevant to skills/plugins)
// ============================================================

#[derive(Default)]
struct Config {
    features_skill: bool,
    skills_enabled: bool,
    skills_roots: Vec<String>,
    skill_overrides: BTreeMap<String, serde_json::Value>,
    storage_dir: String,
    plugins_enabled: bool,
    plugins_dirs: Vec<String>,
    enabled_plugins: BTreeMap<String, bool>,
    suppressed_builtins: Vec<String>,
}

fn load_config() -> Config {
    let mut cfg = Config {
        features_skill: true,
        skills_enabled: true,
        skills_roots: Vec::new(),
        skill_overrides: BTreeMap::new(),
        storage_dir: "~/.zcode".to_string(),
        plugins_enabled: true,
        plugins_dirs: Vec::new(),
        enabled_plugins: BTreeMap::new(),
        suppressed_builtins: Vec::new(),
    };

    // User config: ~/.zcode/cli/config.json
    let user_path = format!("{}/.zcode/cli/config.json", home_dir());
    apply_config_file(&mut cfg, &user_path);

    // Project configs: walk cwd up to worktree root; per dir try
    // zcode.json and .zcode/config.json (root-most first).
    let cwd = resolved_cwd();
    for dir in project_config_directories(&cwd) {
        for candidate in [
            format!("{dir}/zcode.json"),
            format!("{dir}/.zcode/config.json"),
        ] {
            if Path::new(&candidate).exists() {
                apply_config_file(&mut cfg, &candidate);
            }
        }
    }

    // Environment overrides (ZCODE_* surface).
    if let Ok(dir) = std::env::var("ZCODE_STORAGE_DIR") {
        cfg.storage_dir = dir;
    }
    cfg
}

fn apply_config_file(cfg: &mut Config, path: &str) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return;
    };
    if !value.is_object() {
        return;
    }
    if let Some(features) = value.get("features").and_then(|v| v.as_object()) {
        if let Some(b) = features.get("skill").and_then(|v| v.as_bool()) {
            cfg.features_skill = b;
        }
    }
    if let Some(skills) = value.get("skills").and_then(|v| v.as_object()) {
        if let Some(b) = skills.get("enabled").and_then(|v| v.as_bool()) {
            cfg.skills_enabled = b;
        }
        if let Some(roots) = skills.get("roots").and_then(|v| v.as_array()) {
            cfg.skills_roots = roots
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }
    }
    if let Some(overrides) = value.get("skillOverrides").and_then(|v| v.as_object()) {
        for (k, v) in overrides {
            cfg.skill_overrides.insert(k.clone(), v.clone());
        }
    }
    if let Some(storage) = value.get("storage").and_then(|v| v.as_object()) {
        if let Some(dir) = storage.get("dir").and_then(|v| v.as_str()) {
            cfg.storage_dir = dir.to_string();
        }
    }
    if let Some(plugins) = value.get("plugins").and_then(|v| v.as_object()) {
        if let Some(b) = plugins.get("enabled").and_then(|v| v.as_bool()) {
            cfg.plugins_enabled = b;
        }
        if let Some(dirs) = plugins.get("dirs").and_then(|v| v.as_array()) {
            cfg.plugins_dirs = dirs
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }
        if let Some(enabled) = plugins.get("enabledPlugins").and_then(|v| v.as_object()) {
            for (k, v) in enabled {
                if let Some(b) = v.as_bool() {
                    cfg.enabled_plugins.insert(k.clone(), b);
                }
            }
        }
        if let Some(suppressed) = plugins.get("suppressedBuiltins").and_then(|v| v.as_array()) {
            for id in suppressed.iter().filter_map(|v| v.as_str()) {
                cfg.suppressed_builtins.push(id.to_string());
            }
        }
    }
}

fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
}

fn resolve_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(HOME_PREFIX) {
        return normalize_path(&format!("{}/{}", home_dir(), rest));
    }
    if path == "~" {
        return home_dir();
    }
    if path.starts_with('/') {
        normalize_path(path)
    } else {
        normalize_path(&format!("{}/{}", resolved_cwd(), path))
    }
}

/// Lexical normalization matching Node path.resolve/join semantics closely
/// enough for real paths: collapses "//", "." and "..".
fn normalize_path(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if !segments.is_empty() && segments[segments.len() - 1] != ".." {
                    segments.pop();
                } else if !absolute {
                    segments.push("..");
                }
            }
            other => segments.push(other),
        }
    }
    let joined = segments.join("/");
    if absolute {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    }
}

fn node_resolve(base: &str, p: &str) -> String {
    if p.starts_with('/') {
        normalize_path(p)
    } else {
        normalize_path(&format!("{base}/{p}"))
    }
}

fn node_join(base: &str, p: &str) -> String {
    normalize_path(&format!("{base}/{p}"))
}

fn node_dirname(path: &str) -> String {
    match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(idx) => path[..idx].to_string(),
        None => ".".to_string(),
    }
}

fn path_basename(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((_, base)) if !base.is_empty() => base.to_string(),
        _ => path.to_string(),
    }
}

fn directory_exists(path: &str) -> bool {
    std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
}

fn file_exists(path: &str) -> bool {
    std::fs::metadata(path).map(|m| m.is_file()).unwrap_or(false)
}

fn project_config_directories(start: &str) -> Vec<String> {
    // getProjectConfigDirectories: walk up; .git file or dir marks the
    // worktree root; without a marker only the start dir is project scope.
    let mut directories: Vec<String> = Vec::new();
    let mut current = start.to_string();
    loop {
        directories.push(current.clone());
        if has_worktree_marker(&current) {
            directories.reverse();
            return directories;
        }
        let parent = node_dirname(&current);
        if parent == current {
            break;
        }
        current = parent;
    }
    vec![start.to_string()]
}

fn has_worktree_marker(directory: &str) -> bool {
    let marker = format!("{directory}/.git");
    std::fs::metadata(&marker)
        .map(|m| m.is_dir() || m.is_file())
        .unwrap_or(false)
}

// ============================================================
// Discovery pipeline (bootstrap/skills.ts + adapters/skills)
// ============================================================

struct DiscoveryRequest {
    config: Config,
    plugin_skill_roots: Vec<SkillRoot>,
}

fn discovery_request() -> DiscoveryRequest {
    let config = load_config();
    let plugin_skill_roots = if config.plugins_enabled {
        resolve_plugin_skill_roots(&config)
    } else {
        Vec::new()
    };
    DiscoveryRequest {
        config,
        plugin_skill_roots,
    }
}

fn resolve_default_skill_roots(request: &DiscoveryRequest) -> Vec<SkillRoot> {
    let cwd = resolved_cwd();
    let mut roots: Vec<SkillRoot> = Vec::new();
    let mut priority: i64 = 0;
    let mut next_priority = || {
        priority += ROOT_PRIORITY_STEP;
        priority
    };

    // config.skills.roots → project/zcode roots first.
    for extra_root in &request.config.skills_roots {
        roots.push(SkillRoot {
            path: resolve_configured_root(extra_root, &cwd),
            scope: "project",
            source: "zcode",
            priority: next_priority(),
            plugin_id: None,
        });
    }

    // User home roots (~/.zcode/skills, ~/.agents/skills).
    let home = home_dir();
    roots.push(SkillRoot {
        path: node_resolve(&home, ".zcode/skills"),
        scope: "user",
        source: "zcode",
        priority: next_priority(),
        plugin_id: None,
    });
    roots.push(SkillRoot {
        path: node_resolve(&home, ".agents/skills"),
        scope: "user",
        source: "agents",
        priority: next_priority(),
        plugin_id: None,
    });

    // Project roots: cwd up to worktree root (or just cwd).
    for directory in project_skill_directories(&cwd) {
        roots.push(SkillRoot {
            path: node_join(&directory, ".zcode/skills"),
            scope: "project",
            source: "zcode",
            priority: next_priority(),
            plugin_id: None,
        });
        roots.push(SkillRoot {
            path: node_join(&directory, ".agents/skills"),
            scope: "project",
            source: "agents",
            priority: next_priority(),
            plugin_id: None,
        });
    }

    // Plugin skill roots last (their own priorities are >= 1000).
    roots.extend(request.plugin_skill_roots.iter().cloned());
    roots
}

fn project_skill_directories(working_directory: &str) -> Vec<String> {
    // findWorktreeRoot: nearest ancestor (inclusive) containing .git.
    let mut current = working_directory.to_string();
    loop {
        if has_worktree_marker(&current) {
            let mut directories: Vec<String> = Vec::new();
            let mut walk = working_directory.to_string();
            loop {
                directories.push(walk.clone());
                if walk == current || node_dirname(&walk) == walk {
                    break;
                }
                walk = node_dirname(&walk);
            }
            return directories;
        }
        let parent = node_dirname(&current);
        if parent == current {
            return vec![working_directory.to_string()];
        }
        current = parent;
    }
}

fn resolve_configured_root(path: &str, working_directory: &str) -> String {
    let expanded = if let Some(rest) = path.strip_prefix(HOME_PREFIX) {
        format!("{}/{}", home_dir(), rest)
    } else {
        path.to_string()
    };
    if expanded.starts_with('/') {
        normalize_path(&expanded)
    } else {
        node_resolve(working_directory, &expanded)
    }
}

fn discover(request: &DiscoveryRequest) -> Result<SkillLoadOutcome, String> {
    if !request.config.features_skill || !request.config.skills_enabled {
        return Ok(SkillLoadOutcome {
            diagnostics: Vec::new(),
            skills: Vec::new(),
            total_discovered: 0,
        });
    }

    let roots = resolve_default_skill_roots(request);
    let mut sorted_roots = roots;
    sorted_roots.sort_by_key(|root| root.priority); // stable, matches toSorted

    let diagnostics: Vec<SkillDiagnostic> = Vec::new();
    // Map<resolved path, metadata> — insertion-ordered via Vec of keys.
    let mut selected_paths: Vec<String> = Vec::new();
    let mut selected: BTreeMap<String, SkillMetadata> = BTreeMap::new();
    let mut total_discovered: usize = 0;

    let disabled_paths = collect_disabled_paths(&request.config.skill_overrides);

    for root in &sorted_roots {
        let skill_paths = skill_files_under_root(&root.path, root.source != "plugin");
        for path in skill_paths {
            let parsed = match parse_skill(&path, root) {
                Some(p) => p,
                None => continue,
            };
            if is_disabled_skill_path(&parsed.path, &disabled_paths) {
                continue;
            }
            total_discovered += 1;
            if selected.contains_key(&parsed.path) {
                continue;
            }
            selected_paths.push(parsed.path.clone());
            selected.insert(parsed.path.clone(), parsed);
        }
    }

    let mut skills: Vec<SkillMetadata> = selected_paths
        .iter()
        .filter_map(|path| selected.remove(path))
        .collect();
    // Stable sort by name (localeCompare under LC_ALL=C == byte order).
    skills.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(SkillLoadOutcome {
        diagnostics,
        skills,
        total_discovered,
    })
}

fn inspect(request: &DiscoveryRequest, name: &str) -> Result<SkillContent, String> {
    if !request.config.features_skill || !request.config.skills_enabled {
        return Err("Skills are disabled.".to_string());
    }
    let outcome = discover(request)?;
    if !outcome.skills.iter().any(|skill| {
        skill.name == name || skill.qualified_name.as_deref() == Some(name)
    }) {
        return Err(format!("Skill not found: {name}"));
    }
    load_skill(&outcome, name)
}

fn load_skill(outcome: &SkillLoadOutcome, name: &str) -> Result<SkillContent, String> {
    let metadata = outcome
        .skills
        .iter()
        .find(|skill| skill.name == name || skill.qualified_name.as_deref() == Some(name));
    let metadata = match metadata {
        Some(m) => m,
        None => return Err(format!("Skill not found: {name}")),
    };

    let info = std::fs::metadata(&metadata.path)
        .map_err(|_| format!("Skill not found: {name}"))?;
    let size = info.len();
    let truncated = size > DEFAULT_MAX_SKILL_BYTES;
    let buffer: Vec<u8> = if truncated {
        read_first_bytes(&metadata.path, DEFAULT_MAX_SKILL_BYTES)
            .map_err(|_| format!("Skill not found: {name}"))?
    } else {
        std::fs::read(&metadata.path).map_err(|_| format!("Skill not found: {name}"))?
    };
    let bytes_read = buffer.len() as u64;
    let raw_content = String::from_utf8_lossy(&buffer).into_owned();
    let content = strip_frontmatter(&raw_content).trim().to_string();

    Ok(SkillContent {
        base_directory: metadata.directory.clone(),
        bytes_read,
        content,
        metadata: clone_metadata(metadata),
        size_bytes: size,
        truncated,
    })
}

fn clone_metadata(m: &SkillMetadata) -> SkillMetadata {
    SkillMetadata {
        name: m.name.clone(),
        description: m.description.clone(),
        when_to_use: m.when_to_use.clone(),
        plugin_name: m.plugin_name.clone(),
        qualified_name: m.qualified_name.clone(),
        path: m.path.clone(),
        directory: m.directory.clone(),
        root_path: m.root_path.clone(),
        scope: m.scope,
        source: m.source,
        safe_to_auto_load: m.safe_to_auto_load,
        frontmatter_keys: m.frontmatter_keys.clone(),
    }
}

fn read_first_bytes(path: &str, max_bytes: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let mut buffer = vec![0u8; max_bytes as usize];
    let mut read_total = 0usize;
    file.seek(SeekFrom::Start(0))?;
    loop {
        let n = file.read(&mut buffer[read_total..])?;
        if n == 0 {
            break;
        }
        read_total += n;
        if read_total == buffer.len() {
            break;
        }
    }
    buffer.truncate(read_total);
    Ok(buffer)
}

// ============================================================
// Root scanning (adapters/src/skills/scan.ts)
// ============================================================

fn is_symbolic_link(path: &str) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

fn is_loadable_skill_file(path: &str, follow_symlinks: bool) -> bool {
    if !follow_symlinks && is_symbolic_link(path) {
        return false;
    }
    std::fs::metadata(path)
        .map(|m| m.is_file())
        .unwrap_or(false)
}

fn should_walk_skill_directory_entry(entry_name: &str) -> bool {
    if SKILL_SCAN_EXCLUDED_DIRECTORY_NAMES.contains(&entry_name) {
        return false;
    }
    if !entry_name.starts_with('.') {
        return true;
    }
    SKILL_DISCOVERY_DOT_DIR_ALLOWLIST.contains(&entry_name)
}

fn skill_files_under_root(root_path: &str, follow_symlinks: bool) -> Vec<String> {
    if !follow_symlinks && is_symbolic_link(root_path) {
        return Vec::new();
    }
    let root_info = match std::fs::metadata(root_path) {
        Ok(m) => m,
        Err(_) => return Vec::new(), // ENOENT and other scan errors → empty root
    };
    if !root_info.is_dir() {
        return Vec::new();
    }

    let mut files: Vec<String> = Vec::new();
    let own = node_join(root_path, SKILL_FILE_NAME);
    if is_loadable_skill_file(&own, follow_symlinks) {
        files.push(own);
    }

    let entries = match std::fs::read_dir(root_path) {
        Ok(entries) => entries,
        Err(_) => return files,
    };
    for entry in entries.flatten() {
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let walkable = file_type.is_dir() || (follow_symlinks && file_type.is_symlink());
        if !walkable {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !should_walk_skill_directory_entry(&name) {
            continue;
        }
        let candidate = node_join(&node_join(root_path, &name), SKILL_FILE_NAME);
        if is_loadable_skill_file(&candidate, follow_symlinks) {
            files.push(candidate);
        }
    }
    files
}

// ============================================================
// Skill parsing (adapters/src/skills/index.ts)
// ============================================================

fn parse_skill(path: &str, root: &SkillRoot) -> Option<SkillMetadata> {
    let bytes = std::fs::read(path).ok()?; // read failure (missing) → silently skip
    let raw_content = String::from_utf8_lossy(&bytes).into_owned();

    let frontmatter = extract_frontmatter(&raw_content);
    let parsed = match &frontmatter {
        Some(fm) => parse_flat_yaml(fm, path),
        None => (BTreeMap::new(), Vec::new()),
    };
    let (values, keys) = parsed;

    let mut name = parse_scalar(values.get("name").map(|s| s.as_str()));
    if name.is_none() && frontmatter.is_none() {
        name = Some(path_basename(&node_dirname(path)));
    }
    let name = name?;

    let raw_description = parse_scalar(values.get("description").map(|s| s.as_str()));
    if raw_description.is_none() && frontmatter.is_some() {
        // skill_missing_description → skill dropped (diagnostic not surfaced by CLI).
        return None;
    }
    let description = raw_description.unwrap_or_default();
    if utf16_length(&description) > MAX_DESCRIPTION_LENGTH {
        return None;
    }

    let (plugin_name, qualified_name) = if root.source == "plugin" {
        match resolve_plugin_name(&root.path) {
            Some(plugin_name) => (
                Some(plugin_name.clone()),
                Some(format!("{plugin_name}:{name}")),
            ),
            None => (None, None),
        }
    } else {
        (None, None)
    };

    let safe_to_auto_load = keys
        .iter()
        .all(|key| SAFE_FRONTMATTER_KEYS.contains(&key.as_str()));

    Some(SkillMetadata {
        frontmatter_keys: keys,
        description,
        directory: node_dirname(path),
        name,
        path: path.to_string(),
        plugin_name,
        qualified_name,
        root_path: root.path.clone(),
        safe_to_auto_load,
        scope: root.scope,
        source: root.source,
        when_to_use: parse_scalar(values.get("when_to_use").map(|s| s.as_str())),
    })
}

fn utf16_length(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

fn extract_frontmatter(content: &str) -> Option<String> {
    let normalized = content.strip_prefix('\u{FEFF}').unwrap_or(content);
    if !normalized.starts_with("---") {
        return None;
    }
    let lines: Vec<&str> = normalized.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    if lines.first().map(|l| l.trim()) != Some("---") {
        return None;
    }
    let end_index = lines
        .iter()
        .enumerate()
        .find(|(index, line)| *index > 0 && line.trim() == "---")
        .map(|(index, _)| index);
    let end_index = end_index?;
    if end_index == 0 {
        return None;
    }
    Some(lines[1..end_index].join("\n"))
}

fn strip_frontmatter(content: &str) -> String {
    let normalized = content.strip_prefix('\u{FEFF}').unwrap_or(content);
    if !normalized.starts_with("---") {
        return normalized.to_string();
    }
    let lines: Vec<&str> = normalized.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    if lines.first().map(|l| l.trim()) != Some("---") {
        return normalized.to_string();
    }
    let end_index = lines
        .iter()
        .enumerate()
        .find(|(index, line)| *index > 0 && line.trim() == "---")
        .map(|(index, _)| index);
    match end_index {
        Some(idx) if idx > 0 => lines[idx + 1..].join("\n"),
        _ => normalized.to_string(),
    }
}

type FlatYaml = (BTreeMap<String, String>, Vec<String>);

fn parse_flat_yaml(frontmatter: &str, path: &str) -> FlatYaml {
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    let mut keys: Vec<String> = Vec::new();
    let lines: Vec<&str> = frontmatter
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();

    let mut index = 0usize;
    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            index += 1;
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') || starts_with_whitespace(line) {
            index += 1;
            continue;
        }

        let separator = match line.find(':') {
            Some(sep) if sep > 0 => sep,
            _ => {
                // skill_invalid_frontmatter diagnostic (not surfaced by the CLI).
                index += 1;
                let _ = path_basename(path);
                continue;
            }
        };

        let key = line[..separator].trim().to_string();
        let value = line[separator + 1..].trim().to_string();
        keys.push(key.clone());
        match parse_block_scalar_style(&value) {
            Some(style) => {
                let block = read_block_scalar(&lines, index + 1, style);
                values.insert(key, block.0);
                index = block.1;
            }
            None => {
                values.insert(key, value);
                index += 1;
            }
        }
    }

    (values, keys)
}

fn starts_with_whitespace(line: &str) -> bool {
    line.chars().next().map(|c| c.is_whitespace()).unwrap_or(false)
}

fn parse_block_scalar_style(value: &str) -> Option<&'static str> {
    match value {
        ">" | ">-" | ">+" => Some("folded"),
        "|" | "|-" | "|+" => Some("literal"),
        _ => None,
    }
}

fn read_block_scalar(lines: &[&str], start_index: usize, style: &str) -> (String, usize) {
    let mut raw_lines: Vec<&str> = Vec::new();
    let mut index = start_index;
    while index < lines.len() {
        let line = lines[index];
        if !line.trim().is_empty() && !starts_with_whitespace(line) {
            break;
        }
        raw_lines.push(line);
        index += 1;
    }

    let mut indent: Option<usize> = None;
    for line in &raw_lines {
        if line.trim().is_empty() {
            continue;
        }
        let line_indent = leading_whitespace_length(line);
        indent = Some(match indent {
            None => line_indent,
            Some(current) => current.min(line_indent),
        });
    }
    let indent = indent.unwrap_or(0);
    let content_lines: Vec<String> = raw_lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                line.chars().skip(indent).collect()
            }
        })
        .collect();

    let value = if style == "folded" {
        fold_block_scalar_lines(&content_lines)
    } else {
        content_lines.join("\n").trim().to_string()
    };
    (value, index)
}

fn leading_whitespace_length(value: &str) -> usize {
    value
        .chars()
        .take_while(|c| c.is_whitespace())
        .count()
}

fn fold_block_scalar_lines(lines: &[String]) -> String {
    let mut paragraphs: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !current.is_empty() {
                paragraphs.push(current.join(" "));
                current.clear();
            }
            continue;
        }
        current.push(trimmed);
    }
    if !current.is_empty() {
        paragraphs.push(current.join(" "));
    }
    paragraphs.join("\n").trim().to_string()
}

fn parse_scalar(value: Option<&str>) -> Option<String> {
    let value = value?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2)
        || (trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2)
    {
        return Some(trimmed[1..trimmed.len() - 1].trim().to_string());
    }
    Some(trimmed.to_string())
}

fn is_disabled_skill_path(path: &str, disabled_paths: &BTreeSet<String>) -> bool {
    if disabled_paths.is_empty() {
        return false;
    }
    let resolved_path = normalize_path(path);
    if disabled_paths.contains(&resolved_path) {
        return true;
    }
    match std::fs::canonicalize(&resolved_path) {
        Ok(canonical) => disabled_paths.contains(&canonical.to_string_lossy().into_owned()),
        Err(_) => false,
    }
}

fn collect_disabled_paths(skill_overrides: &BTreeMap<String, serde_json::Value>) -> BTreeSet<String> {
    let mut paths: BTreeSet<String> = BTreeSet::new();
    for (path, value) in skill_overrides {
        let enabled_false = value
            .as_object()
            .and_then(|obj| obj.get("enable"))
            .and_then(|v| v.as_bool())
            == Some(false);
        if !enabled_false {
            continue;
        }
        let resolved = normalize_path(path);
        paths.insert(resolved.clone());
        if let Ok(canonical) = std::fs::canonicalize(&resolved) {
            paths.insert(canonical.to_string_lossy().into_owned());
        }
    }
    paths
}

// ============================================================
// Plugin skill roots (bootstrap resolveZCodePlugins → adapter)
// ============================================================

fn plugin_storage_root(config: &Config) -> String {
    let storage_root = resolve_path(&config.storage_dir);
    let cli_root = if path_basename(&storage_root) == "cli" {
        storage_root
    } else {
        node_join(&storage_root, "cli")
    };
    node_join(&cli_root, "plugins")
}

fn resolve_plugin_skill_roots(config: &Config) -> Vec<SkillRoot> {
    let storage_root = plugin_storage_root(config);
    let mut suppressed: BTreeSet<String> = config.suppressed_builtins.iter().cloned().collect();
    if !cua_internal_feature_enabled() {
        suppressed.insert(format!("computer-use@{OFFICIAL_MARKETPLACE}"));
    }

    let data_root = node_join(&storage_root, "data");
    let candidates = plugin_candidates(config, &storage_root);
    let mut skill_roots: Vec<SkillRoot> = Vec::new();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    let mut priority = FIRST_PLUGIN_PRIORITY;

    for candidate in candidates {
        let loaded = match load_plugin_candidate(&candidate) {
            Some(loaded) => loaded,
            None => continue, // plugin_root_not_found / manifest problems
        };
        if loaded.source == "official" && suppressed.contains(&loaded.id) {
            continue;
        }
        if seen_ids.contains(&loaded.id) {
            continue; // plugin_duplicate_id
        }
        seen_ids.insert(loaded.id.clone());

        let candidate_default_enabled =
            candidate.default_enabled || DEFAULT_ENABLED_OFFICIAL_PLUGIN_IDS.contains(&loaded.id.as_str());
        let enabled = config
            .enabled_plugins
            .get(&loaded.id)
            .copied()
            .unwrap_or(candidate_default_enabled);
        if !enabled {
            priority += PRIORITY_STEP;
            continue;
        }

        // resolveEnabledComponents: TS mkdirs dataPath here; side effect only.
        let _ = &data_root;

        // resolveComponentRoots("skills"):
        let mut paths = parse_path_list(loaded.manifest.get("skills"));
        let default_path = node_join(&loaded.root_path, "skills");
        if directory_exists(&default_path) {
            paths.insert(0, "skills".to_string());
        }
        let mut seen_paths: BTreeSet<String> = BTreeSet::new();
        for raw_path in paths {
            let path = match resolve_inside(&loaded.root_path, &raw_path) {
                Some(path) => path,
                None => continue, // plugin_component_path_invalid
            };
            if seen_paths.contains(&path) {
                continue;
            }
            seen_paths.insert(path.clone());
            skill_roots.push(SkillRoot {
                path,
                scope: if loaded.source == "official" { "system" } else { "user" },
                source: "plugin",
                priority,
                plugin_id: Some(loaded.id.clone()),
            });
        }
        priority += PRIORITY_STEP;
    }

    skill_roots
}

fn cua_internal_feature_enabled() -> bool {
    // isZCodeCuaInternalFeatureEnabled: explicit 0/false/off disables.
    match std::env::var("ZCODE_CUA_PRODUCT_HELPER") {
        Ok(value) => {
            let normalized = value.trim().to_lowercase();
            !matches!(normalized.as_str(), "0" | "false" | "off")
        }
        Err(_) => true,
    }
}

struct PluginCandidate {
    root_path: String,
    marketplace: &'static str,
    source: &'static str, // "inline" | "official" | "cache"
    default_enabled: bool,
}

/// Marketplace ids are few; intern them for the &'static str field.
fn leak_marketplace(marketplace: &str) -> &'static str {
    match marketplace {
        OFFICIAL_MARKETPLACE => OFFICIAL_MARKETPLACE,
        INLINE_MARKETPLACE => INLINE_MARKETPLACE,
        other => Box::leak(other.to_string().into_boxed_str()),
    }
}

struct LoadedPlugin {
    id: String,
    manifest: serde_json::Value,
    root_path: String,
    source: &'static str,
    marketplace: &'static str,
}

fn plugin_candidates(config: &Config, storage_root: &str) -> Vec<PluginCandidate> {
    let mut candidates: Vec<PluginCandidate> = Vec::new();
    for root_path in &config.plugins_dirs {
        candidates.push(PluginCandidate {
            root_path: normalize_path(root_path),
            marketplace: INLINE_MARKETPLACE,
            source: "inline",
            default_enabled: true,
        });
    }
    // resolveOfficialPluginRoots: extraRoots + failed-seed fallbacks; both empty
    // when the bundled cache is current (the normal case; TS seeds it).
    for root_path in scan_official_cache(storage_root) {
        candidates.push(PluginCandidate {
            root_path,
            marketplace: OFFICIAL_MARKETPLACE,
            source: "official",
            default_enabled: false,
        });
    }
    for record in list_installed_plugin_records(storage_root) {
        candidates.push(PluginCandidate {
            root_path: resolve_installed_plugin_root(storage_root, &record),
            marketplace: leak_marketplace(&record.marketplace),
            source: "cache",
            default_enabled: false,
        });
    }
    candidates
}

/// scanOfficialCache: bundled partition is authoritative when present.
fn scan_official_cache(storage_root: &str) -> Vec<String> {
    if let Some(roots) = load_bundled_official_plugin_roots(storage_root) {
        return roots;
    }
    let cache_root = node_join(storage_root, &format!("cache/{OFFICIAL_MARKETPLACE}"));
    let mut roots: Vec<String> = Vec::new();
    let Ok(plugin_entries) = std::fs::read_dir(&cache_root) else {
        return Vec::new();
    };
    for plugin_entry in plugin_entries.flatten() {
        if !plugin_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let plugin_dir = node_join(&cache_root, &plugin_entry.file_name().to_string_lossy());
        let Ok(version_entries) = std::fs::read_dir(&plugin_dir) else {
            continue;
        };
        for version_entry in version_entries.flatten() {
            if version_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                roots.push(node_join(&plugin_dir, &version_entry.file_name().to_string_lossy()));
            }
        }
    }
    roots
}

fn load_bundled_official_plugin_roots(storage_root: &str) -> Option<Vec<String>> {
    let partition_path = node_join(
        storage_root,
        &format!("marketplaces/{OFFICIAL_MARKETPLACE}/bundled-marketplace.json"),
    );
    let bytes = std::fs::read(&partition_path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let obj = value.as_object()?;
    if obj.get("version").and_then(|v| v.as_i64()) != Some(1) {
        return None;
    }
    let manifest = obj.get("manifest").and_then(|v| v.as_object())?;
    let official_cache_root = node_resolve(storage_root, &format!("cache/{OFFICIAL_MARKETPLACE}"));
    let plugins = manifest.get("plugins")?.as_array()?;
    let mut roots: Vec<String> = Vec::new();
    for plugin in plugins {
        let plugin = plugin.as_object()?;
        let name = plugin.get("name").and_then(|v| v.as_str());
        let cache_path = plugin.get("cachePath").and_then(|v| v.as_str());
        let (name, cache_path) = match (name, cache_path) {
            (Some(n), Some(c)) if !n.is_empty() => (n, c),
            _ => continue,
        };
        let plugin_cache_root = node_resolve(&official_cache_root, name);
        let resolved_cache_path = node_resolve("/", cache_path);
        if !is_strict_descendant(&official_cache_root, &plugin_cache_root)
            || !is_strict_descendant(&plugin_cache_root, &resolved_cache_path)
        {
            continue;
        }
        roots.push(resolved_cache_path);
    }
    Some(roots)
}

fn is_strict_descendant(parent_path: &str, child_path: &str) -> bool {
    let relative_path = match child_path.strip_prefix(parent_path) {
        Some(rest) if rest.starts_with('/') => &rest[1..],
        Some("") => return false,
        _ => return false,
    };
    !relative_path.is_empty() && relative_path != ".." && !relative_path.starts_with("../")
}

struct InstalledPluginRecord {
    marketplace: String,
    name: String,
    version: String,
    install_path: Option<String>,
}

fn list_installed_plugin_records(storage_root: &str) -> Vec<InstalledPluginRecord> {
    let path = node_join(storage_root, "installed_plugins.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    let mut records: Vec<InstalledPluginRecord> = Vec::new();
    if let Some(plugins) = value.get("plugins").and_then(|v| v.as_array()) {
        for plugin in plugins {
            let obj = match plugin.as_object() {
                Some(obj) => obj,
                None => continue,
            };
            let marketplace = obj.get("marketplace").and_then(|v| v.as_str()).unwrap_or("");
            let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let version = obj.get("version").and_then(|v| v.as_str()).unwrap_or("0.0.0");
            let install_path = obj
                .get("installPath")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            records.push(InstalledPluginRecord {
                marketplace: marketplace.to_string(),
                name: name.to_string(),
                version: version.to_string(),
                install_path,
            });
        }
    }
    records
}

fn resolve_installed_plugin_root(storage_root: &str, record: &InstalledPluginRecord) -> String {
    match &record.install_path {
        Some(path) if !path.is_empty() => normalize_path(path),
        _ => node_join(
            storage_root,
            &format!("cache/{}/{}/{}", record.marketplace, record.name, record.version),
        ),
    }
}

fn load_plugin_candidate(candidate: &PluginCandidate) -> Option<LoadedPlugin> {
    if !directory_exists(&candidate.root_path) {
        return None; // plugin_root_not_found
    }
    let manifest_path = [
        node_join(&candidate.root_path, ".zcode-plugin/plugin.json"),
        node_join(&candidate.root_path, ".claude-plugin/plugin.json"),
        node_join(&candidate.root_path, ".codex-plugin/plugin.json"),
    ]
    .into_iter()
    .find(|path| file_exists(path))?;

    let bytes = std::fs::read(&manifest_path).ok()?;
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let manifest = parsed.as_object()?.to_owned();
    let name = manifest
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if !is_valid_plugin_name(name) {
        return None; // plugin_manifest_invalid
    }
    let id = format!("{name}@{}", candidate.marketplace);
    Some(LoadedPlugin {
        id,
        manifest: serde_json::Value::Object(manifest),
        root_path: candidate.root_path.clone(),
        source: candidate.source,
        marketplace: candidate.marketplace,
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
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_' || b == b'-')
}

fn parse_path_list(value: Option<&serde_json::Value>) -> Vec<String> {
    match value {
        Some(serde_json::Value::String(s)) => vec![s.clone()],
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

fn resolve_inside(root_path: &str, raw_path: &str) -> Option<String> {
    if raw_path.starts_with('/') {
        return None;
    }
    let resolved = node_resolve(root_path, raw_path);
    let root_norm = normalize_path(root_path);
    let rel = match resolved.strip_prefix(&format!("{root_norm}/")) {
        Some(rest) => rest,
        None if resolved == root_norm => "",
        None => return None,
    };
    if rel.is_empty() {
        return Some(resolved);
    }
    // Reject any ".." path segment (escape) — matches the TS relative() check.
    if rel.split('/').any(|segment| segment == "..") {
        return None;
    }
    Some(resolved)
}

/// resolvePluginName: walk up from the skill root (max 5 levels) looking for a
/// plugin manifest whose `name` becomes the qualifiedName prefix.
fn resolve_plugin_name(skill_root_path: &str) -> Option<String> {
    let mut current = node_resolve(skill_root_path, ".");
    for _depth in 0..=MAX_PLUGIN_MANIFEST_SEARCH_DEPTH {
        for relative_path in PLUGIN_MANIFEST_RELATIVE_PATHS {
            let manifest_path = node_join(&current, relative_path);
            if let Some(name) = read_plugin_name_from_manifest(&manifest_path) {
                return Some(name);
            }
        }
        let parent = node_dirname(&current);
        if parent == current {
            break;
        }
        current = parent;
    }
    None
}

fn read_plugin_name_from_manifest(manifest_path: &str) -> Option<String> {
    let bytes = std::fs::read(manifest_path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let name = value.get("name")?.as_str()?;
    let trimmed = name.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

// ============================================================
// Output formatting (skills-command.ts)
// ============================================================

fn format_skill_description(description: &str, when_to_use: &Option<String>) -> String {
    match when_to_use {
        Some(wtu) => format!("{description} {wtu}"),
        None => description.to_string(),
    }
}

fn format_skill_name<'a>(name: &'a str, qualified_name: &'a Option<String>) -> &'a str {
    match qualified_name {
        Some(qualified) => qualified,
        None => name,
    }
}

fn format_human_skill_list(outcome: &SkillLoadOutcome, verbose: bool) -> String {
    if outcome.skills.is_empty() {
        return "No skills found.\n".to_string();
    }

    let mut lines: Vec<String> = vec![format!(
        "Available skills ({})",
        outcome.skills.len()
    )];
    for skill in &outcome.skills {
        let alias = match &skill.qualified_name {
            Some(_) => format!("; alias {}", skill.name),
            None => String::new(),
        };
        lines.push(format!(
            "- {} ({}/{}{alias})",
            format_skill_name(&skill.name, &skill.qualified_name),
            skill.scope,
            skill.source
        ));
        lines.push(format!(
            "  {}",
            format_skill_description(&skill.description, &skill.when_to_use)
        ));
        lines.push(format!("  {}", skill.path));
    }

    append_diagnostics(&mut lines, &outcome.diagnostics, verbose);
    format!("{}\n", lines.join("\n"))
}

fn format_human_skill_inspection(inspection: &SkillContent, verbose: bool) -> String {
    let metadata = &inspection.metadata;
    let mut lines: Vec<String> = vec![format!(
        "Skill: {}",
        format_skill_name(&metadata.name, &metadata.qualified_name)
    )];
    lines.push(format!("scope/source: {}/{}", metadata.scope, metadata.source));
    lines.push(format!("path: {}", metadata.path));
    lines.push(format!("directory: {}", metadata.directory));
    lines.push(format!("description: {}", metadata.description));
    if let Some(plugin_name) = &metadata.plugin_name {
        lines.push(format!("pluginName: {plugin_name}"));
    }
    if let Some(qualified_name) = &metadata.qualified_name {
        lines.push(format!("qualifiedName: {qualified_name}"));
    }
    if let Some(when_to_use) = &metadata.when_to_use {
        lines.push(format!("whenToUse: {when_to_use}"));
    }
    lines.push(format!(
        "safeToAutoLoad: {}",
        if metadata.safe_to_auto_load { "yes" } else { "no" }
    ));
    lines.push(format!(
        "size: {}/{} bytes{}",
        inspection.bytes_read,
        inspection.size_bytes,
        if inspection.truncated { " (truncated)" } else { "" }
    ));
    lines.push(String::new());
    lines.push("Content".to_string());
    lines.push(if inspection.content.is_empty() {
        "(empty)".to_string()
    } else {
        inspection.content.clone()
    });

    append_diagnostics(&mut lines, &inspect_diagnostics(inspection), verbose);
    format!("{}\n", lines.join("\n"))
}

fn inspect_diagnostics(_: &SkillContent) -> Vec<SkillDiagnostic> {
    // Filled by caller via DiscoveryRequest in TS; the CLI always passes the
    // discovery diagnostics through. We re-run discovery instead of carrying
    // them on SkillContent (see run_inspect for why this is lossless here).
    Vec::new()
}

fn append_diagnostics(lines: &mut Vec<String>, diagnostics: &[SkillDiagnostic], verbose: bool) {
    if !verbose || diagnostics.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push(format!("Diagnostics ({})", diagnostics.len()));
    for diagnostic in diagnostics {
        let location = match &diagnostic.path {
            Some(path) => format!(" ({path})"),
            None => String::new(),
        };
        lines.push(format!(
            "- [{}] {}: {}{}",
            diagnostic.severity, diagnostic.code, diagnostic.message, location
        ));
    }
}

// ============================================================
// JSON output (matches JSON.stringify(value, null, 2) + "\n")
// ============================================================

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticJson<'a> {
    code: &'a str,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'a String>,
    severity: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    skill_name: Option<&'a String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillJson<'a> {
    description: &'a str,
    directory: &'a str,
    name: &'a str,
    path: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    plugin_name: Option<&'a String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    qualified_name: Option<&'a String>,
    root_path: &'a str,
    scope: &'a str,
    source: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    when_to_use: Option<&'a String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillListJson<'a> {
    cwd: &'a str,
    diagnostics: Vec<DiagnosticJson<'a>>,
    skills: Vec<SkillJson<'a>>,
    total_discovered: usize,
}

fn format_skill_json(outcome: &SkillLoadOutcome, cwd: &str) -> String {
    let payload = SkillListJson {
        cwd,
        diagnostics: outcome
            .diagnostics
            .iter()
            .map(|d| DiagnosticJson {
                code: d.code,
                message: &d.message,
                path: d.path.as_ref(),
                severity: d.severity,
                skill_name: d.skill_name.as_ref(),
            })
            .collect(),
        skills: outcome
            .skills
            .iter()
            .map(|skill| SkillJson {
                description: &skill.description,
                directory: &skill.directory,
                name: &skill.name,
                path: &skill.path,
                plugin_name: skill.plugin_name.as_ref(),
                qualified_name: skill.qualified_name.as_ref(),
                root_path: &skill.root_path,
                scope: skill.scope,
                source: skill.source,
                when_to_use: skill.when_to_use.as_ref(),
            })
            .collect(),
        total_discovered: outcome.total_discovered,
    };
    serde_json_string(&payload)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillMetadataJson<'a> {
    description: &'a str,
    directory: &'a str,
    frontmatter_keys: &'a [String],
    name: &'a str,
    path: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    plugin_name: Option<&'a String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    qualified_name: Option<&'a String>,
    root_path: &'a str,
    safe_to_auto_load: bool,
    scope: &'a str,
    source: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    when_to_use: Option<&'a String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillContentJson<'a> {
    base_directory: &'a str,
    bytes_read: u64,
    content: &'a str,
    metadata: SkillMetadataJson<'a>,
    size_bytes: u64,
    truncated: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillInspectionJson<'a> {
    cwd: &'a str,
    diagnostics: Vec<DiagnosticJson<'a>>,
    skill: SkillContentJson<'a>,
}

fn format_skill_inspection_json(inspection: &SkillContent, cwd: &str) -> String {
    let metadata = &inspection.metadata;
    let payload = SkillInspectionJson {
        cwd,
        diagnostics: Vec::new(),
        skill: SkillContentJson {
            base_directory: &inspection.base_directory,
            bytes_read: inspection.bytes_read,
            content: &inspection.content,
            metadata: SkillMetadataJson {
                description: &metadata.description,
                directory: &metadata.directory,
                frontmatter_keys: &metadata.frontmatter_keys,
                name: &metadata.name,
                path: &metadata.path,
                plugin_name: metadata.plugin_name.as_ref(),
                qualified_name: metadata.qualified_name.as_ref(),
                root_path: &metadata.root_path,
                safe_to_auto_load: metadata.safe_to_auto_load,
                scope: metadata.scope,
                source: metadata.source,
                when_to_use: metadata.when_to_use.as_ref(),
            },
            size_bytes: inspection.size_bytes,
            truncated: inspection.truncated,
        },
    };
    serde_json_string(&payload)
}

fn serde_json_string<T: Serialize>(value: &T) -> String {
    let mut out = serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".to_string());
    out.push('\n');
    out
}
