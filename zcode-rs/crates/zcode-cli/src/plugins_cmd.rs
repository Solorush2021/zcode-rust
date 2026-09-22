//! plugins <command> — native port of packages/cli/src/plugins-command.ts.
//!
//! Owns the LOCAL surface: `list` (default subcommand, human + --json), the
//! usage/error outputs (PluginsUsageError -> message + usage, exit 1), and the
//! scope/arity validation errors for the mutating subcommands. Everything that
//! mutates state or needs the network (install/uninstall/enable/disable/update/
//! validate <path>/marketplace ...) execs the Node bundle for byte passthrough.
//!
//! `list` reads live state the same way listZCodePlugins does:
//!   - user config ~/.zcode/cli/config.json (plugins.enabledPlugins etc.)
//!   - project config candidates (zcode.json / .zcode/config.json, cwd -> root)
//!   - bundled official partition ~/.zcode/cli/plugins/marketplaces/zcode-plugins-official/
//!     bundled-marketplace.json (authoritative cachePath list) with a plain
//!     cache-dir scan as fallback
//!   - installed_plugins.json records (rarely present)
//! and re-derives skill/command/hook/mcp counts like adapters/plugins/index.ts.

use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

const PLUGINS_COMMAND_USAGE: &str = r#"Usage: zcode plugins <command> [options]

Commands:
  list [--json] [--available]                  List installed plugins; --available also lists the marketplace catalog
  install <plugin>[@marketplace] [-s <scope>]  Install a plugin from a known marketplace
  uninstall <plugin> [-s <scope>] [--keep-data] [--force]
                                               Uninstall a plugin (--keep-data keeps its data directory)
  enable <plugin> [-s <scope>]                 Enable a plugin
  disable [plugin] [-a|--all] [-s <scope>]     Disable a plugin, or every enabled plugin with --all
  update <plugin> [-s <scope>]                 Update a plugin to the latest marketplace version
  validate <path>                              Validate a plugin or marketplace manifest
  marketplace add <source> [--scope <scope>] [--sparse <path>]
                                               Add a marketplace from a URL, path, or GitHub repo
  marketplace list [--json]                    List configured marketplaces
  marketplace remove <name>                    Remove a configured marketplace
  marketplace update [name]                    Refresh one marketplace, or all when omitted

Scopes: user (default), project. `zcode plugin` is an alias of `zcode plugins`."#;

const ZCODE_OFFICIAL_MARKETPLACE: &str = "zcode-plugins-official";
const ZCODE_INLINE_MARKETPLACE: &str = "inline";
const LEGACY_CUA_PLUGIN_ID: &str = "zcode-cua@zcode-plugins-official";
const CANONICAL_CUA_PLUGIN_ID: &str = "computer-use@zcode-plugins-official";
const DEFAULT_VERSION: &str = "0.0.0";

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

const HOOK_EVENT_NAMES: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PostToolUseFailure",
    "Stop",
];

const SKILL_SCAN_EXCLUDED_DIRS: &[&str] = &[
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

pub fn run(rest: &[String], json: bool, all: bool, available: bool, scope: Option<&str>) -> i32 {
    let (subcommand, rest): (&str, &[String]) = match rest.first() {
        Some(s) => (s.as_str(), &rest[1..]),
        None => ("list", rest),
    };
    match subcommand {
        "list" => {
            if !rest.is_empty() {
                return usage_error("");
            }
            run_list(json, available)
        }
        // Mutating / network subcommands: validate arity + scope exactly like the
        // TS dispatch (usage errors are local, byte-identical), then exec Node.
        "install" | "uninstall" | "enable" | "update" => {
            if require_one_error(rest) {
                return usage_error("");
            }
            if let Err(msg) = resolve_scope(scope) {
                return usage_error(&msg);
            }
            fallback()
        }
        "disable" => {
            if all && !rest.is_empty() {
                return usage_error("Cannot use --all with a specific plugin");
            }
            if !all {
                if rest.is_empty() {
                    return usage_error("Please specify a plugin name or use --all to disable all plugins");
                }
                if require_one_error(rest) {
                    return usage_error("");
                }
                if let Err(msg) = resolve_scope(scope) {
                    return usage_error(&msg);
                }
                return fallback();
            }
            if scope.is_some() {
                return usage_error("Cannot use --scope with --all");
            }
            fallback()
        }
        "validate" => {
            if require_one_error(rest) {
                return usage_error("");
            }
            fallback()
        }
        "marketplace" => fallback(),
        other => usage_error(&format!("Unknown plugins command: {other}")),
    }
}

fn fallback() -> i32 {
    crate::fallback::exec_fallback(&std::env::args().skip(1).collect::<Vec<_>>())
}

/// plugins-command-shared.ts requireOne(): throws a message-less usage error.
fn require_one_error(rest: &[String]) -> bool {
    match rest.first() {
        Some(v) => rest.len() != 1 || v.is_empty() || v.trim().is_empty(),
        None => true,
    }
}

fn resolve_scope(value: Option<&str>) -> Result<(), String> {
    match value {
        None => Ok(()),
        Some("user") => Ok(()),
        Some("project") => Ok(()),
        Some("local") => Err("Scope 'local' is not supported by zcode. Use: user, project".into()),
        Some(other) => Err(format!("Invalid scope '{other}'. Use: user, project")),
    }
}

fn usage_error(message: &str) -> i32 {
    let mut e = std::io::stderr();
    if !message.is_empty() {
        let _ = write!(e, "{message}\n");
    }
    let _ = writeln!(e, "{PLUGINS_COMMAND_USAGE}");
    1
}

// ---------------------------------------------------------------------------
// plugins list
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Diagnostic {
    code: String,
    message: String,
    severity: String,
    path: Option<String>,
    plugin_id: Option<String>,
}

impl Diagnostic {
    fn new(code: &str, severity: &str, message: String, path: Option<String>, plugin_id: Option<String>) -> Self {
        Diagnostic { code: code.to_string(), severity: severity.to_string(), message, path, plugin_id }
    }
    fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("code".into(), Value::String(self.code.clone()));
        m.insert("message".into(), Value::String(self.message.clone()));
        m.insert("severity".into(), Value::String(self.severity.clone()));
        if let Some(p) = &self.path {
            m.insert("path".into(), Value::String(p.clone()));
        }
        if let Some(p) = &self.plugin_id {
            m.insert("pluginId".into(), Value::String(p.clone()));
        }
        Value::Object(m)
    }
    fn line(&self) -> String {
        let location = self.path.as_ref().map(|p| format!(" ({p})")).unwrap_or_default();
        let plugin = self.plugin_id.as_ref().map(|p| format!(" {p}")).unwrap_or_default();
        format!("- [{}]{} {}: {}{}", self.severity, plugin, self.code, self.message, location)
    }
}

struct HookDetail {
    command: String,
    event: String,
    runnable: bool,
    source_path: String,
    kind: String,
    args: Option<Vec<String>>,
    is_async: Option<bool>,
    matcher: Option<String>,
    shell: Option<Value>,
    status_message: Option<String>,
    timeout: Option<f64>,
    timeout_ms: Option<f64>,
}

impl HookDetail {
    fn to_json(&self) -> Value {
        // plugins-command-format.ts formatHookDetailJson key order.
        let mut m = Map::new();
        m.insert("command".into(), Value::String(self.command.clone()));
        m.insert("event".into(), Value::String(self.event.clone()));
        m.insert("runnable".into(), Value::Bool(self.runnable));
        m.insert("sourcePath".into(), Value::String(self.source_path.clone()));
        m.insert("type".into(), Value::String(self.kind.clone()));
        if let Some(a) = &self.args {
            m.insert("args".into(), Value::Array(a.iter().cloned().map(Value::String).collect()));
        }
        if let Some(a) = self.is_async {
            m.insert("async".into(), Value::Bool(a));
        }
        if let Some(v) = &self.matcher {
            m.insert("matcher".into(), Value::String(v.clone()));
        }
        if let Some(v) = &self.shell {
            m.insert("shell".into(), v.clone());
        }
        if let Some(v) = &self.status_message {
            m.insert("statusMessage".into(), Value::String(v.clone()));
        }
        if let Some(v) = self.timeout {
            m.insert("timeout".into(), num(v));
        }
        if let Some(v) = self.timeout_ms {
            m.insert("timeoutMs".into(), num(v));
        }
        Value::Object(m)
    }
}

struct PluginMeta {
    command_root_count: usize,
    data_path: String,
    enabled: bool,
    id: String,
    manifest_path: String,
    marketplace: String,
    declared_mcp_server_names: Vec<String>,
    mcp_server_names: Vec<String>,
    hook_details: Vec<HookDetail>,
    name: String,
    root_path: String,
    skill_count: usize,
    skill_root_count: usize,
    source: String,
    description: Option<String>,
    version: Option<String>,
}

impl PluginMeta {
    /// plugins-command-format.ts formatPluginJson key order.
    fn to_json(&self, diagnostics: &[Diagnostic]) -> Value {
        let mut m = Map::new();
        m.insert("commandRootCount".into(), Value::from(self.command_root_count as u64));
        m.insert("dataPath".into(), Value::String(self.data_path.clone()));
        m.insert("enabled".into(), Value::Bool(self.enabled));
        m.insert("id".into(), Value::String(self.id.clone()));
        m.insert("manifestPath".into(), Value::String(self.manifest_path.clone()));
        m.insert("marketplace".into(), Value::String(self.marketplace.clone()));
        m.insert(
            "declaredMcpServerNames".into(),
            Value::Array(self.declared_mcp_server_names.iter().cloned().map(Value::String).collect()),
        );
        m.insert(
            "mcpServerNames".into(),
            Value::Array(self.mcp_server_names.iter().cloned().map(Value::String).collect()),
        );
        m.insert(
            "hookDetails".into(),
            Value::Array(self.hook_details.iter().map(|h| h.to_json()).collect()),
        );
        m.insert("name".into(), Value::String(self.name.clone()));
        m.insert("rootPath".into(), Value::String(self.root_path.clone()));
        m.insert("skillCount".into(), Value::from(self.skill_count as u64));
        m.insert("skillRootCount".into(), Value::from(self.skill_root_count as u64));
        m.insert("source".into(), Value::String(self.source.clone()));
        m.insert(
            "diagnostics".into(),
            Value::Array(
                diagnostics
                    .iter()
                    .filter(|d| d.plugin_id.as_deref() == Some(self.id.as_str()))
                    .map(|d| d.to_json())
                    .collect(),
            ),
        );
        if let Some(d) = &self.description {
            m.insert("description".into(), Value::String(d.clone()));
        }
        if let Some(v) = &self.version {
            m.insert("version".into(), Value::String(v.clone()));
        }
        Value::Object(m)
    }
}

fn num(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < 9.007_199_254_740_992e15 {
        Value::from(v as i64)
    } else {
        serde_json::Number::from_f64(v).map(Value::Number).unwrap_or(Value::Null)
    }
}

fn run_list(json: bool, available: bool) -> i32 {
    let verbose = std::env::args().any(|a| a == "--verbose");
    let (plugins_root, plugins_cfg) = load_plugin_context();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    let mut plugins: Vec<PluginMeta> = Vec::new();
    if plugins_cfg.enabled {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let candidates = collect_candidates(&plugins_root, &plugins_cfg, &mut diagnostics);
        let mut seen: HashSet<String> = HashSet::new();
        for candidate in candidates {
            let loaded = match load_plugin(&candidate, &mut diagnostics) {
                Some(l) => l,
                None => continue,
            };
            if loaded.source == "official"
                && plugins_cfg.suppressed_builtins.iter().any(|s| *s == loaded.id)
            {
                continue;
            }
            if seen.contains(&loaded.id) {
                diagnostics.push(Diagnostic::new(
                    "plugin_duplicate_id",
                    "warning",
                    format!("Duplicate plugin ignored: {}", loaded.id),
                    Some(loaded.root_path.clone()),
                    Some(loaded.id.clone()),
                ));
                continue;
            }
            seen.insert(loaded.id.clone());
            for key in ["channels", "lspServers", "outputStyles", "settings"] {
                if loaded.manifest.raw.contains_key(key) {
                    diagnostics.push(Diagnostic::new(
                        "plugin_unsupported_component",
                        "warning",
                        format!("Plugin component is diagnostic-only in this ZCode runtime: {key}"),
                        Some(loaded.manifest_path.clone()),
                        Some(loaded.id.clone()),
                    ));
                }
            }

            let default_enabled = candidate.default_enabled
                || DEFAULT_ENABLED_OFFICIAL_IDS.contains(&loaded.id.as_str());
            let enabled = plugins_cfg
                .enabled_plugins
                .get(&loaded.id)
                .copied()
                .unwrap_or(default_enabled);
            let data_path = plugins_root.join("data").join(sanitize_plugin_id(&loaded.id));

            // declaredMcpServerNames are computed for every plugin (enabled or not).
            let declared_mcp = load_plugin_mcp_definitions(&loaded, &mut diagnostics);
            let hook_details = inspect_plugin_hooks(&loaded, &data_path.to_string_lossy(), &mut diagnostics);

            let (command_root_count, skill_root_count, skill_count, mcp_server_names) = if enabled {
                let _ = std::fs::create_dir_all(&data_path);
                let skill_roots = resolve_component_roots("skills", &loaded, &mut diagnostics);
                let command_roots = resolve_component_roots("commands", &loaded, &mut diagnostics);
                let skill_count = count_skill_files(&skill_roots);
                let mcp_names = mcp_names_from_definitions(&loaded, &declared_mcp, &mut diagnostics);
                (command_roots.len(), skill_roots.len(), skill_count, mcp_names)
            } else {
                (0, 0, 0, Vec::new())
            };

            plugins.push(PluginMeta {
                command_root_count,
                data_path: data_path.to_string_lossy().into_owned(),
                enabled,
                id: loaded.id,
                manifest_path: loaded.manifest_path,
                marketplace: loaded.marketplace,
                declared_mcp_server_names: declared_mcp.iter().map(|(k, _)| k.clone()).collect(),
                mcp_server_names,
                hook_details,
                name: loaded.manifest.name,
                root_path: loaded.root_path,
                skill_count,
                skill_root_count,
                source: loaded.source,
                description: loaded.manifest.description,
                version: loaded.manifest.version,
            });
        }
        let _ = cwd;
    }

    if !available {
        let out = if json {
            serde_json::to_string_pretty(&Value::Array(
                plugins.iter().map(|p| p.to_json(&diagnostics)).collect(),
            ))
            .unwrap_or_default()
        } else {
            format_human_plugin_list(&plugins, &diagnostics, verbose)
        };
        // Human output already ends with \n; --json needs its trailing newline.
        if json {
            let _ = writeln!(std::io::stdout(), "{out}");
        } else {
            let _ = write!(std::io::stdout(), "{out}");
        }
        // Marketplace refresh failures carry a marketplace id (or none) as pluginId,
        // so error-level diagnostics that match no listed entry must not be dropped.
        let known: HashSet<&str> = plugins.iter().map(|p| p.id.as_str()).collect();
        let orphaned: Vec<&Diagnostic> = diagnostics
            .iter()
            .filter(|d| {
                d.severity == "error"
                    && match &d.plugin_id {
                        None => true,
                        Some(id) => !known.contains(id.as_str()),
                    }
            })
            .collect();
        if orphaned.is_empty() {
            return 0;
        }
        let mut e = std::io::stderr();
        for d in &orphaned {
            let _ = writeln!(e, "{}", d.line());
        }
        return 1;
    }

    // --available: overview joins marketplaces catalog + installed records.
    let known_marketplaces = load_known_marketplaces(&plugins_root);
    let mut overview_diagnostics = diagnostics.clone();
    for record in &known_marketplaces {
        if let Some(failure) = &record.last_refresh_failure {
            overview_diagnostics.push(Diagnostic::new(
                &failure.code,
                "error",
                failure.message.clone(),
                None,
                Some(record.id.clone()),
            ));
        }
    }
    let installed_ids: HashSet<&str> = plugins.iter().map(|p| p.id.as_str()).collect();
    let available_plugins = list_available_plugins(&plugins_root, &known_marketplaces, &installed_ids);

    if json {
        let mut m = Map::new();
        m.insert(
            "installed".into(),
            Value::Array(plugins.iter().map(|p| p.to_json(&diagnostics)).collect()),
        );
        m.insert(
            "available".into(),
            Value::Array(available_plugins.iter().map(|p| p.to_json()).collect()),
        );
        m.insert(
            "diagnostics".into(),
            Value::Array(overview_diagnostics.iter().map(|d| d.to_json()).collect()),
        );
        let s = serde_json::to_string_pretty(&Value::Object(m)).unwrap_or_default();
        let _ = writeln!(std::io::stdout(), "{s}");
    } else {
        let mut out = format_human_plugin_list(&plugins, &diagnostics, verbose);
        out.push('\n');
        out.push_str(&format_human_available_list(&available_plugins));
        let _ = write!(std::io::stdout(), "{out}");
    }
    0
}

fn format_human_plugin_list(plugins: &[PluginMeta], diagnostics: &[Diagnostic], verbose: bool) -> String {
    if plugins.is_empty() {
        return "No plugins found.\n".into();
    }
    let mut lines = vec![format!("Plugins ({})", plugins.len())];
    for plugin in plugins {
        let state = if plugin.enabled { "enabled" } else { "disabled" };
        let mcp = if plugin.mcp_server_names.is_empty() {
            "none".to_string()
        } else {
            plugin.mcp_server_names.join(", ")
        };
        lines.push(format!("- {} [{}]", plugin.id, state));
        lines.push(format!("  {}/{}: {}", plugin.source, plugin.marketplace, plugin.root_path));
        lines.push(format!(
            "  skills: {}, commands: {}, hooks: {}, mcp: {}",
            plugin.skill_count,
            plugin.command_root_count,
            plugin.hook_details.len(),
            mcp
        ));
    }
    if verbose && !diagnostics.is_empty() {
        lines.push(String::new());
        lines.push(format!("Diagnostics ({})", diagnostics.len()));
        for d in diagnostics {
            lines.push(d.line());
        }
    }
    format!("{}\n", lines.join("\n"))
}

struct AvailablePlugin {
    id: String,
    installed: bool,
    marketplace: String,
    name: String,
    component_types: Vec<String>,
    description: Option<String>,
    version: Option<String>,
}

impl AvailablePlugin {
    fn to_json(&self) -> Value {
        // formatAvailablePluginJson key order.
        let mut m = Map::new();
        m.insert("id".into(), Value::String(self.id.clone()));
        m.insert("installed".into(), Value::Bool(self.installed));
        m.insert("marketplace".into(), Value::String(self.marketplace.clone()));
        m.insert("name".into(), Value::String(self.name.clone()));
        if !self.component_types.is_empty() {
            m.insert(
                "componentTypes".into(),
                Value::Array(self.component_types.iter().cloned().map(Value::String).collect()),
            );
        }
        if let Some(d) = &self.description {
            m.insert("description".into(), Value::String(d.clone()));
        }
        if let Some(v) = &self.version {
            m.insert("version".into(), Value::String(v.clone()));
        }
        Value::Object(m)
    }
}

fn format_human_available_list(plugins: &[AvailablePlugin]) -> String {
    if plugins.is_empty() {
        return "No plugins available from configured marketplaces.\n".into();
    }
    let mut lines = vec![format!("Available plugins ({})", plugins.len())];
    for p in plugins {
        let state = if p.installed { "installed" } else { "not installed" };
        match &p.version {
            Some(v) if !v.is_empty() => lines.push(format!("- {} {} [{}]", p.id, v, state)),
            _ => lines.push(format!("- {} [{}]", p.id, state)),
        }
        if let Some(d) = &p.description {
            if !d.is_empty() {
                lines.push(format!("  {d}"));
            }
        }
    }
    format!("{}\n", lines.join("\n"))
}

// ---------------------------------------------------------------------------
// Config layer (createConfig reduced to the plugins + storage surface)
// ---------------------------------------------------------------------------

struct PluginConfig {
    enabled: bool,
    dirs: Vec<String>,
    enabled_plugins: HashMap<String, bool>,
    suppressed_builtins: Vec<String>,
}

struct MarketplaceRecord {
    id: String,
    last_refresh_failure: Option<Diagnostic>,
}

fn json_str(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(|s| s.to_string())
}

fn canonicalize_plugin_id(id: &str) -> String {
    if id == LEGACY_CUA_PLUGIN_ID { CANONICAL_CUA_PLUGIN_ID.to_string() } else { id.to_string() }
}

fn parse_file_config(path: &Path) -> Option<(Map<String, Value>)> {
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
fn project_config_directories(start: &Path) -> Vec<PathBuf> {
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

fn load_plugin_context() -> (PathBuf, PluginConfig) {
    let home = std::env::var("HOME").unwrap_or_default();
    let user_config_path = PathBuf::from(&home).join(".zcode/cli/config.json");

    let mut cfg = PluginConfig {
        enabled: true,
        dirs: Vec::new(),
        enabled_plugins: HashMap::new(),
        suppressed_builtins: Vec::new(),
    };

    // Storage dir: user/project config, then ZCODE_STORAGE_DIR env, then ~/.zcode.
    let mut storage_dir: Option<String> = None;

    let user_file = parse_file_config(&user_config_path);
    if let Some(user) = &user_file {
        if let Some(patch) = extract_plugin_patch(user) {
            merge_plugin_patch(&mut cfg, &patch);
        }
        storage_dir = json_str(user.get("storage").and_then(|s| s.get("dir")));
    }

    // Project configs: nearest-cwd wins (walk is root-most first, merged in order).
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for directory in project_config_directories(&cwd) {
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
                if let Some(dir) = json_str(file.get("storage").and_then(|s| s.get("dir"))) {
                    storage_dir = Some(dir);
                }
            }
        }
    }

    if let Some(dir) = std::env::var("ZCODE_STORAGE_DIR").ok() {
        storage_dir = Some(dir);
    }

    let raw = storage_dir.unwrap_or_else(|| "~/.zcode".to_string());
    let storage_root = resolve_path(&raw, &home);
    let cli_root = if storage_root.file_name().and_then(|s| s.to_str()) == Some("cli") {
        storage_root
    } else {
        storage_root.join("cli")
    };
    (cli_root.join("plugins"), cfg)
}

// ---------------------------------------------------------------------------
// Candidate discovery (adapters/plugins/index.ts resolveCandidates + scan)
// ---------------------------------------------------------------------------

struct Candidate {
    root_path: PathBuf,
    marketplace: String,
    source: String,
    default_enabled: bool,
}

fn collect_candidates(
    plugins_root: &Path,
    cfg: &PluginConfig,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<Candidate> {
    let mut candidates: Vec<Candidate> = Vec::new();
    for dir in &cfg.dirs {
        candidates.push(Candidate {
            root_path: resolve_path(dir, &std::env::var("HOME").unwrap_or_default()),
            marketplace: ZCODE_INLINE_MARKETPLACE.into(),
            source: "inline".into(),
            default_enabled: true,
        });
    }
    // officialPluginRoots: seed fallback roots only; seeding itself is owned by
    // the Node bundle, so a fresh unseeded install needs Node once. Normal
    // state (seeded cache) contributes zero roots here — matches TS.
    candidates.extend(scan_official_cache(plugins_root, diagnostics).into_iter().map(|root| Candidate {
        root_path: root,
        marketplace: ZCODE_OFFICIAL_MARKETPLACE.into(),
        source: "official".into(),
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
        candidates.push(Candidate {
            root_path: root,
            marketplace: record.marketplace.clone(),
            source: "cache".into(),
            default_enabled: false,
        });
    }
    candidates
}

fn scan_official_cache(plugins_root: &Path, diagnostics: &mut Vec<Diagnostic>) -> Vec<PathBuf> {
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
                            let (Some(name), Some(cache_path)) =
                                (json_str(p.get("name")), json_str(p.get("cachePath")))
                            else {
                                continue;
                            };
                            let plugin_cache_root = official_cache_root.join(&name);
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
    let entries = match std::fs::read_dir(&cache_root) {
        Ok(entries) => entries,
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                diagnostics.push(Diagnostic::new(
                    "plugin_root_not_found",
                    "warning",
                    format!("Failed to scan {}: {err}", cache_root.display()),
                    Some(cache_root.to_string_lossy().into_owned()),
                    None,
                ));
            }
            return roots;
        }
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
    id: String,
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
        let (Some(id), Some(name), Some(marketplace), Some(version)) = (
            json_str(obj.get("id")),
            json_str(obj.get("name")),
            json_str(obj.get("marketplace")),
            json_str(obj.get("version")),
        ) else {
            continue;
        };
        records.push(InstalledRecord {
            id,
            name,
            marketplace,
            version,
            install_path: json_str(obj.get("installPath")),
        });
    }
    records
}

// ---------------------------------------------------------------------------
// Plugin loading (loadPlugin + component resolution)
// ---------------------------------------------------------------------------

struct LoadedPlugin {
    id: String,
    manifest: Manifest,
    manifest_path: String,
    marketplace: String,
    root_path: String,
    source: String,
}

struct Manifest {
    raw: Map<String, Value>,
    name: String,
    version: Option<String>,
    description: Option<String>,
}

fn load_plugin(candidate: &Candidate, diagnostics: &mut Vec<Diagnostic>) -> Option<LoadedPlugin> {
    let root = &candidate.root_path;
    if !root.is_dir() {
        diagnostics.push(Diagnostic::new(
            "plugin_root_not_found",
            "warning",
            format!("Plugin root does not exist: {}", root.display()),
            Some(root.to_string_lossy().into_owned()),
            None,
        ));
        return None;
    }
    let manifest_path = [
        root.join(".zcode-plugin").join("plugin.json"),
        root.join(".claude-plugin").join("plugin.json"),
        root.join(".codex-plugin").join("plugin.json"),
    ]
    .into_iter()
    .find(|p| p.is_file());
    let manifest_path = match manifest_path {
        Some(p) => p,
        None => {
            diagnostics.push(Diagnostic::new(
                "plugin_manifest_not_found",
                "error",
                format!("Plugin manifest not found: {}", root.display()),
                Some(root.to_string_lossy().into_owned()),
                None,
            ));
            return None;
        }
    };
    let manifest = match read_manifest(&manifest_path, diagnostics) {
        Some(m) => m,
        None => return None,
    };
    Some(LoadedPlugin {
        id: format!("{}@{}", manifest.name, candidate.marketplace),
        manifest_path: manifest_path.to_string_lossy().into_owned(),
        marketplace: candidate.marketplace.clone(),
        root_path: root.to_string_lossy().into_owned(),
        source: candidate.source.clone(),
        manifest,
    })
}

fn read_manifest(path: &Path, diagnostics: &mut Vec<Diagnostic>) -> Option<Manifest> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(err) => {
            diagnostics.push(Diagnostic::new(
                "plugin_manifest_invalid",
                "error",
                format!("{err}"),
                Some(path.to_string_lossy().into_owned()),
                None,
            ));
            return None;
        }
    };
    let parsed: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(err) => {
            diagnostics.push(Diagnostic::new(
                "plugin_manifest_invalid",
                "error",
                format!("{err}"),
                Some(path.to_string_lossy().into_owned()),
                None,
            ));
            return None;
        }
    };
    let Some(obj) = parsed.as_object() else {
        diagnostics.push(Diagnostic::new(
            "plugin_manifest_invalid",
            "error",
            "Manifest must be a JSON object".into(),
            Some(path.to_string_lossy().into_owned()),
            None,
        ));
        return None;
    };
    let name = json_str(obj.get("name")).unwrap_or_default();
    let name = name.trim().to_string();
    if !is_valid_plugin_name(&name) {
        diagnostics.push(Diagnostic::new(
            "plugin_manifest_invalid",
            "error",
            format!("Invalid plugin name: {name}"),
            Some(path.to_string_lossy().into_owned()),
            None,
        ));
        return None;
    }
    let version = json_str(obj.get("version"));
    let description = json_str(obj.get("description"));
    Some(Manifest {
        raw: obj.clone(),
        name,
        version: Some(version.unwrap_or_else(|| DEFAULT_VERSION.into())),
        description,
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

fn parse_path_list(value: Option<&Value>) -> Vec<String> {
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

fn is_missing_path(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(_) => false,
        Err(err) => matches!(err.kind(), std::io::ErrorKind::NotFound),
    }
}

/// helpers.ts resolveInside: reject absolute paths and lexical escapes.
fn resolve_inside(root: &Path, raw: &str) -> Option<PathBuf> {
    let raw_path = Path::new(raw);
    if raw_path.is_absolute() {
        return None;
    }
    let resolved = normalize_abs(&root.join(raw_path));
    let rel = path_relative(root, &resolved);
    if rel.is_empty() || (!rel.starts_with("..") && !rel.contains("../")) {
        Some(resolved)
    } else {
        None
    }
}

fn normalize_abs(p: &Path) -> PathBuf {
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

fn path_relative(from: &Path, to: &Path) -> String {
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

fn resolve_path(raw: &str, home: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        normalize_abs(&PathBuf::from(home).join(rest))
    } else if raw == "~" {
        PathBuf::from(home)
    } else {
        normalize_abs(Path::new(raw))
    }
}

/// adapters/plugins/index.ts resolveComponentRoots (skills | commands).
fn resolve_component_roots(
    key: &str,
    loaded: &LoadedPlugin,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<PathBuf> {
    let root = Path::new(&loaded.root_path);
    let mut paths = parse_path_list(loaded.manifest.raw.get(key));
    let default_path = root.join(key);
    if directory_exists(&default_path) {
        paths.insert(0, key.to_string());
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for raw_path in paths {
        let Some(path) = resolve_inside(root, &raw_path) else {
            diagnostics.push(Diagnostic::new(
                "plugin_component_path_invalid",
                "error",
                format!("Plugin {key} path escapes plugin root: {raw_path}"),
                Some(loaded.manifest_path.clone()),
                Some(loaded.id.clone()),
            ));
            continue;
        };
        let key_str = path.to_string_lossy().into_owned();
        if seen.contains(&key_str) {
            continue;
        }
        seen.insert(key_str);
        roots.push(path);
    }
    roots
}

/// warnEmptyDeclaredSkillRoots: diagnostics for declared-but-empty skill paths.
fn warn_empty_declared_skill_roots(
    loaded: &LoadedPlugin,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let declared = parse_path_list(loaded.manifest.raw.get("skills"));
    if declared.is_empty() {
        return;
    }
    let root = Path::new(&loaded.root_path);
    let mut seen: HashSet<String> = HashSet::new();
    for raw_path in &declared {
        let Some(resolved) = resolve_inside(root, raw_path) else { continue };
        let key = resolved.to_string_lossy().into_owned();
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);
        if is_missing_path(&resolved) {
            diagnostics.push(Diagnostic::new(
                "plugin_skill_root_empty",
                "warning",
                format!("Plugin skills path does not exist: {raw_path}"),
                Some(resolved.to_string_lossy().into_owned()),
                Some(loaded.id.clone()),
            ));
            continue;
        }
        let Ok(files) = scan_skill_files_under_root(&resolved, false) else {
            continue;
        };
        if !files.is_empty() {
            continue;
        }
        diagnostics.push(Diagnostic::new(
            "plugin_skill_root_empty",
            "warning",
            format!("Plugin skills path does not contain any skills: {raw_path}"),
            Some(resolved.to_string_lossy().into_owned()),
            Some(loaded.id.clone()),
        ));
    }
}

/// skills/scan.ts scanSkillFilesUnderRootSync (plugin scope: never follow links).
/// Err = unexpected fs error (EACCES etc.), matching the TS error contract.
fn scan_skill_files_under_root(root: &Path, follow_symlinks: bool) -> std::io::Result<Vec<PathBuf>> {
    if !follow_symlinks && is_symbolic_link(root) {
        return Ok(Vec::new());
    }
    let meta = std::fs::metadata(root)?;
    if !meta.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    let own = root.join("SKILL.md");
    if is_loadable_skill_file(&own, follow_symlinks) {
        files.push(own);
    }
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let entry_type = entry.file_type()?;
        let walkable = entry_type.is_dir() || (follow_symlinks && entry_type.is_symlink());
        if !walkable || !should_walk_skill_directory_entry(&name) {
            continue;
        }
        let candidate = entry.path().join("SKILL.md");
        if is_loadable_skill_file(&candidate, follow_symlinks) {
            files.push(candidate);
        }
    }
    Ok(files)
}

fn should_walk_skill_directory_entry(name: &str) -> bool {
    if SKILL_SCAN_EXCLUDED_DIRS.contains(&name) {
        return false;
    }
    if !name.starts_with('.') {
        return true;
    }
    name == ".system"
}

fn is_symbolic_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).map(|m| m.file_type().is_symlink()).unwrap_or(false)
}

fn is_loadable_skill_file(path: &Path, follow_symlinks: bool) -> bool {
    if !follow_symlinks && is_symbolic_link(path) {
        return false;
    }
    std::fs::metadata(path).map(|m| m.is_file()).unwrap_or(false)
}

fn count_skill_files(skill_roots: &[PathBuf]) -> usize {
    let mut seen: HashSet<String> = HashSet::new();
    for root in skill_roots {
        let Ok(files) = scan_skill_files_under_root(root, false) else {
            continue;
        };
        for file in files {
            seen.insert(file.to_string_lossy().into_owned());
        }
    }
    seen.len()
}

// ---------------------------------------------------------------------------
// Hooks (hook-sources.ts + parsePluginHookEvents, details only)
// ---------------------------------------------------------------------------

fn inspect_plugin_hooks(
    loaded: &LoadedPlugin,
    data_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<HookDetail> {
    let mut details = Vec::new();
    for source in list_plugin_hook_sources(loaded, diagnostics) {
        parse_plugin_hook_events(loaded, data_path, &source, &mut details, diagnostics);
    }
    details
}

struct HookSource {
    raw: Value,
    source_path: String,
    wrapper: bool,
}

fn list_plugin_hook_sources(
    loaded: &LoadedPlugin,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<HookSource> {
    let root = Path::new(&loaded.root_path);
    let mut sources: Vec<HookSource> = Vec::new();
    let mut loaded_hook_paths: HashSet<String> = HashSet::new();
    let standard = root.join("hooks").join("hooks.json");
    if standard.is_file() {
        if let Some(source) = load_hook_file(&standard, loaded, diagnostics) {
            loaded_hook_paths.insert(realpath_or_self(&standard).to_string_lossy().into_owned());
            sources.push(source);
        }
    }
    let Some(manifest_hooks) = loaded.manifest.raw.get("hooks") else {
        return sources;
    };
    let specs: Vec<Value> = match manifest_hooks {
        Value::Array(items) => items.clone(),
        other => vec![other.clone()],
    };
    for spec in specs {
        match spec {
            Value::String(hook_spec) => {
                let Some(hook_file) = resolve_inside(root, &hook_spec) else {
                    diagnostics.push(Diagnostic::new(
                        "plugin_component_path_invalid",
                        "error",
                        format!("Plugin hooks path escapes plugin root: {hook_spec}"),
                        Some(loaded.manifest_path.clone()),
                        Some(loaded.id.clone()),
                    ));
                    continue;
                };
                if !hook_file.is_file() {
                    diagnostics.push(Diagnostic::new(
                        "plugin_hook_read_failed",
                        "error",
                        format!("Plugin hooks file not found: {hook_spec}"),
                        Some(hook_file.to_string_lossy().into_owned()),
                        Some(loaded.id.clone()),
                    ));
                    continue;
                }
                let real = realpath_or_self(&hook_file).to_string_lossy().into_owned();
                if loaded_hook_paths.contains(&real) {
                    diagnostics.push(Diagnostic::new(
                        "plugin_hook_invalid",
                        "warning",
                        format!("Duplicate plugin hooks file ignored: {hook_spec}"),
                        Some(hook_file.to_string_lossy().into_owned()),
                        Some(loaded.id.clone()),
                    ));
                    continue;
                }
                if let Some(source) = load_hook_file(&hook_file, loaded, diagnostics) {
                    loaded_hook_paths.insert(real);
                    sources.push(source);
                }
            }
            other => sources.push(HookSource {
                raw: other,
                source_path: loaded.manifest_path.clone(),
                wrapper: false,
            }),
        }
    }
    sources
}

fn load_hook_file(
    path: &Path,
    loaded: &LoadedPlugin,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<HookSource> {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(value) => Some(HookSource {
                raw: value,
                source_path: path.to_string_lossy().into_owned(),
                wrapper: true,
            }),
            Err(_) => {
                diagnostics.push(Diagnostic::new(
                    "plugin_hook_read_failed",
                    "error",
                    format!("Failed to read plugin hooks: {}", path.display()),
                    Some(path.to_string_lossy().into_owned()),
                    Some(loaded.id.clone()),
                ));
                None
            }
        },
        Err(_) => {
            diagnostics.push(Diagnostic::new(
                "plugin_hook_read_failed",
                "error",
                format!("Failed to read plugin hooks: {}", path.display()),
                Some(path.to_string_lossy().into_owned()),
                Some(loaded.id.clone()),
            ));
            None
        }
    }
}

fn realpath_or_self(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn parse_plugin_hook_events(
    loaded: &LoadedPlugin,
    data_path: &str,
    source: &HookSource,
    details: &mut Vec<HookDetail>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let hooks_root = if source.wrapper {
        source.raw.as_object().and_then(|o| o.get("hooks")).cloned().unwrap_or(Value::Null)
    } else {
        source.raw.clone()
    };
    let Some(event_map) = hooks_root.as_object() else {
        diagnostics.push(Diagnostic::new(
            "plugin_hook_invalid",
            "error",
            if source.wrapper {
                "Plugin hooks file must contain a hooks object".into()
            } else {
                "Plugin manifest hooks entry must be an object, a path, or an array".into()
            },
            Some(source.source_path.clone()),
            Some(loaded.id.clone()),
        ));
        return;
    };
    for (event_name, matcher_configs) in event_map {
        if !HOOK_EVENT_NAMES.contains(&event_name.as_str()) {
            diagnostics.push(Diagnostic::new(
                "plugin_hook_unsupported_event",
                "warning",
                format!("Plugin hook event is not supported by this ZCode runtime: {event_name}"),
                Some(source.source_path.clone()),
                Some(loaded.id.clone()),
            ));
            continue;
        }
        let Some(configs) = matcher_configs.as_array() else {
            diagnostics.push(Diagnostic::new(
                "plugin_hook_invalid",
                "error",
                format!("Plugin hook event must be an array: {event_name}"),
                Some(source.source_path.clone()),
                Some(loaded.id.clone()),
            ));
            continue;
        };
        for config in configs {
            let Some(matcher_obj) = config.as_object() else {
                diagnostics.push(Diagnostic::new(
                    "plugin_hook_invalid",
                    "error",
                    format!(
                        "Invalid plugin hook matcher for {event_name}: {}: Required",
                        if config.is_object() { "hooks" } else { "" }
                    ),
                    Some(source.source_path.clone()),
                    Some(loaded.id.clone()),
                ));
                continue;
            };
            let matcher = json_str(matcher_obj.get("matcher"));
            let Some(Value::Array(hooks)) = matcher_obj.get("hooks") else {
                diagnostics.push(Diagnostic::new(
                    "plugin_hook_invalid",
                    "error",
                    format!("Invalid plugin hook matcher for {event_name}: hooks: Required"),
                    Some(source.source_path.clone()),
                    Some(loaded.id.clone()),
                ));
                continue;
            };
            if hooks.is_empty() {
                diagnostics.push(Diagnostic::new(
                    "plugin_hook_invalid",
                    "error",
                    format!(
                        "Invalid plugin hook matcher for {event_name}: hooks: Array must contain at least 1 element(s)"
                    ),
                    Some(source.source_path.clone()),
                    Some(loaded.id.clone()),
                ));
                continue;
            }
            for hook in hooks {
                let Some(hook_obj) = hook.as_object() else {
                    diagnostics.push(Diagnostic::new(
                        "plugin_hook_invalid",
                        "error",
                        format!("Invalid plugin hook matcher for {event_name}: hooks: Required"),
                        Some(source.source_path.clone()),
                        Some(loaded.id.clone()),
                    ));
                    continue;
                };
                let kind = json_str(hook_obj.get("type")).unwrap_or_default();
                if kind != "process" && kind != "command" {
                    diagnostics.push(Diagnostic::new(
                        "plugin_hook_invalid",
                        "error",
                        format!(
                            "Invalid plugin hook matcher for {event_name}: type: Invalid discriminator value"
                        ),
                        Some(source.source_path.clone()),
                        Some(loaded.id.clone()),
                    ));
                    continue;
                }
                let command = json_str(hook_obj.get("command"));
                let Some(command) = command else {
                    diagnostics.push(Diagnostic::new(
                        "plugin_hook_invalid",
                        "error",
                        format!("Invalid plugin hook matcher for {event_name}: command: Required"),
                        Some(source.source_path.clone()),
                        Some(loaded.id.clone()),
                    ));
                    continue;
                };
                if command.is_empty() {
                    diagnostics.push(Diagnostic::new(
                        "plugin_hook_invalid",
                        "error",
                        format!(
                            "Invalid plugin hook matcher for {event_name}: command: String must contain at least 1 character(s)"
                        ),
                        Some(source.source_path.clone()),
                        Some(loaded.id.clone()),
                    ));
                    continue;
                }
                let args = if kind == "process" {
                    hook_obj.get("args").and_then(|v| v.as_array()).map(|items| {
                        items.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect::<Vec<_>>()
                    })
                } else {
                    None
                };
                let is_async = if kind == "command" {
                    hook_obj.get("async").and_then(Value::as_bool)
                } else {
                    None
                };
                let shell = if kind == "command" {
                    hook_obj.get("shell").and_then(|v| {
                        if v.as_bool() == Some(true) || v.is_string() {
                            Some(v.clone())
                        } else {
                            None
                        }
                    })
                } else {
                    None
                };
                let timeout = if kind == "command" {
                    hook_obj.get("timeout").and_then(Value::as_f64)
                } else {
                    None
                };
                let timeout_ms = hook_obj.get("timeoutMs").and_then(Value::as_f64);
                let status_message = json_str(hook_obj.get("statusMessage"));
                details.push(HookDetail {
                    command,
                    event: event_name.clone(),
                    runnable: true,
                    source_path: source.source_path.clone(),
                    kind,
                    args,
                    is_async,
                    matcher: matcher.clone(),
                    shell,
                    status_message,
                    timeout,
                    timeout_ms,
                });
            }
        }
    }
    let _ = data_path;
}

// ---------------------------------------------------------------------------
// MCP declared servers (mcp.ts, names only)
// ---------------------------------------------------------------------------

fn load_plugin_mcp_definitions(
    loaded: &LoadedPlugin,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<(String, Value)> {
    let mut merged: Vec<(String, Value)> = Vec::new();
    let root = Path::new(&loaded.root_path);
    let from_file = load_mcp_file(&root.join(".mcp.json"), loaded, diagnostics);
    let from_manifest = match loaded.manifest.raw.get("mcpServers") {
        None => Vec::new(),
        Some(Value::String(spec)) => {
            match resolve_inside(root, spec) {
                Some(path) => load_mcp_file(&path, loaded, diagnostics),
                None => {
                    diagnostics.push(Diagnostic::new(
                        "plugin_component_path_invalid",
                        "error",
                        format!("Plugin mcpServers path escapes plugin root: {spec}"),
                        Some(loaded.manifest_path.clone()),
                        Some(loaded.id.clone()),
                    ));
                    Vec::new()
                }
            }
        }
        Some(Value::Array(items)) => {
            let mut out = Vec::new();
            for item in items {
                match item {
                    Value::String(spec) => match resolve_inside(root, spec) {
                        Some(path) => out.extend(load_mcp_file(&path, loaded, diagnostics)),
                        None => {
                            diagnostics.push(Diagnostic::new(
                                "plugin_component_path_invalid",
                                "error",
                                format!("Plugin mcpServers path escapes plugin root: {spec}"),
                                Some(loaded.manifest_path.clone()),
                                Some(loaded.id.clone()),
                            ));
                        }
                    },
                    other => out.extend(normalize_mcp_shape(other.clone(), loaded, diagnostics)),
                }
            }
            out
        }
        Some(other) => normalize_mcp_shape(other.clone(), loaded, diagnostics),
    };
    // {...fromFile, ...fromManifest}: manifest overrides file, key order is
    // first-insertion for shared keys.
    for (key, value) in from_file.into_iter().chain(from_manifest) {
        if let Some(slot) = merged.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = value;
        } else {
            merged.push((key, value));
        }
    }
    merged
}

fn load_mcp_file(
    path: &Path,
    loaded: &LoadedPlugin,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<(String, Value)> {
    match std::fs::read_to_string(path) {
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                diagnostics.push(Diagnostic::new(
                    "plugin_mcp_read_failed",
                    "error",
                    format!("Failed to read MCP config: {}", path.display()),
                    Some(path.to_string_lossy().into_owned()),
                    Some(loaded.id.clone()),
                ));
            }
            Vec::new()
        }
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Err(_) => {
                diagnostics.push(Diagnostic::new(
                    "plugin_mcp_read_failed",
                    "error",
                    format!("Failed to read MCP config: {}", path.display()),
                    Some(path.to_string_lossy().into_owned()),
                    Some(loaded.id.clone()),
                ));
                Vec::new()
            }
            Ok(value) => normalize_mcp_shape(value, loaded, diagnostics),
        },
    }
}

fn normalize_mcp_shape(
    value: Value,
    loaded: &LoadedPlugin,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<(String, Value)> {
    let Some(obj) = value.as_object() else {
        diagnostics.push(Diagnostic::new(
            "plugin_mcp_invalid",
            "error",
            "Plugin MCP config must be an object".into(),
            Some(loaded.manifest_path.clone()),
            Some(loaded.id.clone()),
        ));
        return Vec::new();
    };
    let servers = obj.get("mcpServers").and_then(Value::as_object).unwrap_or(obj);
    servers
        .iter()
        .filter(|(_, v)| v.is_object())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// resolvePluginMcpServers, reduced to the resulting (namespaced) key names.
fn mcp_names_from_definitions(
    loaded: &LoadedPlugin,
    definitions: &[(String, Value)],
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<String> {
    let mut names = Vec::new();
    for (name, server) in definitions {
        let Some(obj) = server.as_object() else {
            continue;
        };
        let kind = match json_str(obj.get("type")) {
            Some(t) => t,
            None => {
                if obj.get("command").and_then(Value::as_str).is_some() {
                    "stdio".to_string()
                } else {
                    "http".to_string()
                }
            }
        };
        if !matches!(kind.as_str(), "stdio" | "http" | "sse") {
            diagnostics.push(Diagnostic::new(
                "plugin_mcp_server_disabled",
                "error",
                format!("Unsupported MCP transport: {kind}"),
                Some(loaded.manifest_path.clone()),
                Some(loaded.id.clone()),
            ));
            continue;
        }
        if kind == "stdio" && json_str(obj.get("command")).map(|c| c.is_empty()).unwrap_or(true) {
            diagnostics.push(Diagnostic::new(
                "plugin_mcp_server_disabled",
                "error",
                "stdio MCP server requires command".into(),
                Some(loaded.manifest_path.clone()),
                Some(loaded.id.clone()),
            ));
            continue;
        }
        names.push(format!("plugin:{}:{}", loaded.manifest.name, name));
    }
    names
}

// ---------------------------------------------------------------------------
// Marketplaces (--available support)
// ---------------------------------------------------------------------------

struct KnownMarketplace {
    id: String,
    last_refresh_failure: Option<Diagnostic>,
}

fn load_known_marketplaces(plugins_root: &Path) -> Vec<KnownMarketplace> {
    let mut records = Vec::new();
    let path = plugins_root.join("known_marketplaces.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return records;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return records;
    };
    let Some(Value::Array(list)) = value.get("marketplaces") else {
        return records;
    };
    for entry in list {
        let Some(obj) = entry.as_object() else { continue };
        let (Some(id), Some(name), Some(_count)) = (
            json_str(obj.get("id")),
            json_str(obj.get("name")),
            obj.get("pluginCount").and_then(Value::as_f64),
        ) else {
            continue;
        };
        if !obj.get("source").map(Value::is_object).unwrap_or(false) {
            continue;
        }
        let _ = name;
        let last_refresh_failure = obj.get("lastRefreshFailure").and_then(|f| {
            let f = f.as_object()?;
            Some(Diagnostic::new(
                &json_str(f.get("code")).unwrap_or_default(),
                "error",
                json_str(f.get("message")).unwrap_or_default(),
                None,
                Some(id.clone()),
            ))
        });
        records.push(KnownMarketplace { id, last_refresh_failure });
    }
    records
}

fn list_available_plugins(
    plugins_root: &Path,
    known: &[KnownMarketplace],
    installed_ids: &HashSet<&str>,
) -> Vec<AvailablePlugin> {
    let mut available = Vec::new();
    for record in known {
        let manifest_path = plugins_root
            .join("marketplaces")
            .join(&record.id)
            .join("marketplace.json");
        let Ok(text) = std::fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(obj) = value.as_object() else { continue };
        let Some(Value::Array(entries)) = obj.get("plugins") else { continue };
        for entry in entries {
            let Some(entry_obj) = entry.as_object() else { continue };
            let Some(name) = json_str(entry_obj.get("name")).map(|n| n.trim().to_string()) else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            let id = format!("{name}@{}", record.id);
            let mut component_types = Vec::new();
            for (key, kind) in [
                ("agents", "agent"),
                ("commands", "command"),
                ("skills", "skill"),
                ("hooks", "hook"),
                ("mcpServers", "mcp"),
                ("lspServers", "lsp"),
            ] {
                if entry_obj.contains_key(key) {
                    component_types.push(kind.to_string());
                }
            }
            let description = json_str(entry_obj.get("description"));
            let version = json_str(entry_obj.get("version"));
            available.push(AvailablePlugin {
                installed: installed_ids.contains(id.as_str()),
                component_types,
                id,
                name,
                marketplace: record.id.clone(),
                description: description.filter(|d| !d.is_empty()),
                version: version.filter(|v| !v.is_empty()),
            });
        }
    }
    available
}
