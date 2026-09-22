//! `hooks trust ...` — native read-only port of packages/cli/src/hooks-trust-command.ts
//! plus the minimal read path of bootstrap/src/workspace-hook-trust-cli.ts
//! (discovery + declaration/bundle digests + trust-store load).
//!
//! status/review run natively; grant/revoke still fall back to the Node bundle
//! because they mutate the persistent trust store.

use serde_json::{json, Map, Value};
use std::io::Write;
use std::path::Path;

const USAGE: &str = "Usage:\n  zcode hooks trust status [--workspace <path-or-identity>] [--json]\n  zcode hooks trust review [--workspace <path-or-identity>] [--json]\n  zcode hooks trust grant --workspace <path-or-identity> --hook-digest <sha256> [--hook-digest <sha256> ...]\n  zcode hooks trust grant --workspace <path-or-identity> --all-current --bundle-digest <sha256>\n  zcode hooks trust revoke --workspace <path-or-identity> [--hook-digest <sha256> ... | --all]\n";

const EVENT_NAMES: [&str; 7] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PostToolUseFailure",
    "Stop",
];

const DEFAULT_HOOK_TIMEOUT_MS: f64 = 60_000.0;
const DEFAULT_HOOK_MAX_OUTPUT_BYTES: f64 = 32_768.0;

struct TrustArgs {
    workspace: Option<String>,
    json: bool,
    help: bool,
    positionals: Vec<String>,
}

fn fail(message: &str) -> i32 {
    let mut e = std::io::stderr();
    let _ = write!(e, "{message}\n{USAGE}");
    1
}

pub fn run(argv: &[String]) -> i32 {
    // run.rs routes argv[0] == "hooks" here; keep the "hooks" token for fallback execs.
    let args: &[String] = if argv.first().map(|s| s.as_str()) == Some("hooks") {
        &argv[1..]
    } else {
        argv
    };
    if args.first().map(|s| s.as_str()) != Some("trust") {
        return fail(&format!(
            "Unknown hooks command: {}",
            args.first().map(|s| s.as_str()).unwrap_or("")
        ));
    }
    let parsed = match parse_trust_args(&args[1..]) {
        Ok(p) => p,
        Err(message) => return fail(&message),
    };
    if parsed.help {
        let _ = write!(std::io::stdout(), "{USAGE}");
        return 0;
    }
    let action = parsed.positionals.first().cloned().unwrap_or_else(|| "status".into());
    if !matches!(action.as_str(), "status" | "review" | "grant" | "revoke")
        || parsed.positionals.len() > 1
    {
        return fail(&format!("Unknown hooks trust command: {action}"));
    }
    if action == "grant" || action == "revoke" {
        return crate::fallback::exec_fallback(argv);
    }
    match inspect_status(&parsed, &action) {
        Ok(output) => {
            let _ = write!(std::io::stdout(), "{output}");
            0
        }
        Err(reason) => {
            if parsed.json {
                let mut obj = Map::new();
                obj.insert("accepted".into(), Value::Bool(false));
                obj.insert("reasonCode".into(), Value::String(reason));
                let _ = writeln!(std::io::stdout(), "{}", pretty(&Value::Object(obj)));
            } else {
                let mut e = std::io::stderr();
                let _ = writeln!(e, "Error: {reason}");
            }
            1
        }
    }
}

// ---- argv parsing (mirrors node:util parseArgs strict mode for this schema) ----

fn parse_trust_args(args: &[String]) -> Result<TrustArgs, String> {
    let mut out = TrustArgs {
        workspace: None,
        json: false,
        help: false,
        positionals: vec![],
    };
    let mut hook_digests: Vec<String> = vec![];
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        i += 1;
        if arg == "--" {
            out.positionals.extend_from_slice(&args[i..]);
            break;
        }
        if let Some(name) = arg.strip_prefix("--") {
            let (name, inline) = match name.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (name, None),
            };
            match name {
                "workspace" | "bundle-digest" => {
                    let value = match inline {
                        Some(v) => v,
                        None => match args.get(i) {
                            Some(v) => {
                                i += 1;
                                v.clone()
                            }
                            None => {
                                return Err(format!(
                                    "Option '--{name} <value>' argument missing"
                                ))
                            }
                        },
                    };
                    if name == "workspace" {
                        out.workspace = Some(value);
                    }
                }
                "hook-digest" => {
                    let value = match inline {
                        Some(v) => v,
                        None => match args.get(i) {
                            Some(v) => {
                                i += 1;
                                v.clone()
                            }
                            None => {
                                return Err(format!(
                                    "Option '--{name} <value>' argument missing"
                                ))
                            }
                        },
                    };
                    hook_digests.push(value);
                }
                "all-current" | "all" | "json" | "help" => {
                    if inline.is_some() {
                        return Err(format!("Option '--{name}' does not take an argument"));
                    }
                    match name {
                        "json" => out.json = true,
                        "help" => out.help = true,
                        _ => {}
                    }
                }
                _ => {
                    return Err(format!(
                        "Unknown option '--{name}'. To specify a positional argument starting with a '-', place it at the end of the command after '--', as in '-- \"--{name}\""
                    ))
                }
            }
            continue;
        }
        if arg.len() > 1 && arg.starts_with('-') {
            // Only -h is defined; any other short option is unknown.
            if arg == "-h" {
                out.help = true;
                continue;
            }
            return Err(format!(
                "Unknown option '{arg}'. To specify a positional argument starting with a '-', place it at the end of the command after '--', as in '-- \"{arg}\""
            ));
        }
        out.positionals.push(arg.to_string());
    }
    Ok(out)
}

// ---- node path semantics (resolve/relative without symlink resolution) ----

fn node_normalize(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut parts: Vec<&str> = vec![];
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    if absolute {
        format!("/{}", parts.join("/"))
    } else {
        parts.join("/")
    }
}

fn node_resolve(base: &str, p: &str) -> String {
    if p.starts_with('/') {
        node_normalize(p)
    } else if p.is_empty() {
        node_normalize(base)
    } else {
        node_normalize(&format!("{base}/{p}"))
    }
}

fn node_dirname(path: &str) -> String {
    match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(i) => path[..i].to_string(),
        None => ".".to_string(),
    }
}

fn node_basename(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[i + 1..].to_string(),
        None => path.to_string(),
    }
}

fn node_relative(from: &str, to: &str) -> String {
    let f: Vec<&str> = from.split('/').filter(|s| !s.is_empty()).collect();
    let t: Vec<&str> = to.split('/').filter(|s| !s.is_empty()).collect();
    let mut common = 0;
    while common < f.len() && common < t.len() && f[common] == t[common] {
        common += 1;
    }
    let mut parts: Vec<String> = vec![];
    for _ in common..f.len() {
        parts.push("..".to_string());
    }
    for seg in &t[common..] {
        parts.push((*seg).to_string());
    }
    parts.join("/")
}

// ---- hooks config validation (zod schemas in workspace-hook-config.ts) ----

fn pos_num(v: &Value) -> bool {
    v.as_f64().map(|n| n.is_finite() && n > 0.0).unwrap_or(false)
}

fn validate_hook_def(h: &Value) -> bool {
    let Some(obj) = h.as_object() else { return false };
    match obj.get("type").and_then(|t| t.as_str()) {
        Some("process") => {
            if obj.get("command").and_then(|c| c.as_str()).map(|c| c.is_empty()).unwrap_or(true) {
                return false;
            }
            if let Some(e) = obj.get("enabled") {
                if !e.is_boolean() {
                    return false;
                }
            }
            if let Some(a) = obj.get("args") {
                let ok = a.as_array().map(|arr| arr.iter().all(|x| x.is_string()));
                if ok != Some(true) {
                    return false;
                }
            }
            if let Some(t) = obj.get("timeoutMs") {
                if !pos_num(t) {
                    return false;
                }
            }
            if let Some(s) = obj.get("statusMessage") {
                if s.as_str().map(|s| s.is_empty()).unwrap_or(true) {
                    return false;
                }
            }
            true
        }
        Some("command") => {
            if obj.get("command").and_then(|c| c.as_str()).map(|c| c.is_empty()).unwrap_or(true) {
                return false;
            }
            if let Some(e) = obj.get("enabled") {
                if !e.is_boolean() {
                    return false;
                }
            }
            if let Some(a) = obj.get("async") {
                if !a.is_boolean() {
                    return false;
                }
            }
            if let Some(s) = obj.get("shell") {
                let ok = s.as_str().map(|x| !x.is_empty()).unwrap_or_else(|| s == &Value::Bool(true));
                if !ok {
                    return false;
                }
            }
            if let Some(t) = obj.get("timeout") {
                if !pos_num(t) {
                    return false;
                }
            }
            if let Some(t) = obj.get("timeoutMs") {
                if !pos_num(t) {
                    return false;
                }
            }
            if let Some(s) = obj.get("statusMessage") {
                if s.as_str().map(|s| s.is_empty()).unwrap_or(true) {
                    return false;
                }
            }
            true
        }
        _ => false,
    }
}

fn validate_matcher(m: &Value) -> bool {
    let Some(obj) = m.as_object() else { return false };
    for k in obj.keys() {
        if k != "matcher" && k != "hooks" {
            return false;
        }
    }
    if let Some(v) = obj.get("matcher") {
        if v.as_str().map(|s| s.is_empty()).unwrap_or(true) {
            return false;
        }
    }
    match obj.get("hooks") {
        Some(Value::Array(arr)) if !arr.is_empty() => arr.iter().all(validate_hook_def),
        _ => false,
    }
}

fn validate_hooks_config(v: &Value) -> bool {
    let Some(obj) = v.as_object() else { return false };
    for k in obj.keys() {
        if k != "enabled" && k != "timeoutMs" && k != "maxOutputBytes" && k != "events" {
            return false;
        }
    }
    if let Some(e) = obj.get("enabled") {
        if !e.is_boolean() {
            return false;
        }
    }
    if let Some(t) = obj.get("timeoutMs") {
        if !pos_num(t) {
            return false;
        }
    }
    if let Some(m) = obj.get("maxOutputBytes") {
        if !pos_num(m) {
            return false;
        }
    }
    if let Some(ev) = obj.get("events") {
        let Some(eo) = ev.as_object() else { return false };
        for (k, val) in eo {
            if !EVENT_NAMES.contains(&k.as_str()) {
                return false;
            }
            let Some(arr) = val.as_array() else { return false };
            if !arr.iter().all(validate_matcher) {
                return false;
            }
        }
    }
    true
}

// ---- discovery (readWorkspaceHookProjectSources + bundle snapshot) ----

struct HookSource {
    canonical_path: String,
    discovery_order: usize,
    kind: &'static str,
    explicit: bool,
    hooks: Value,
}

fn has_git_marker(dir: &str) -> bool {
    let marker = Path::new(dir).join(".git");
    match std::fs::metadata(&marker) {
        Ok(m) => m.is_dir() || m.is_file(),
        Err(_) => false,
    }
}

fn project_config_dirs(start: &str) -> Vec<String> {
    let mut dirs: Vec<String> = vec![];
    let mut current = start.to_string();
    loop {
        dirs.push(current.clone());
        if has_git_marker(&current) {
            dirs.reverse();
            return dirs;
        }
        let parent = node_dirname(&current);
        if parent == current {
            break;
        }
        current = parent;
    }
    vec![start.to_string()]
}

fn is_record(v: &Value) -> bool {
    v.is_object()
}

fn discover_sources(workspace_path: &str) -> Result<Vec<HookSource>, String> {
    let mut dirs = project_config_dirs(workspace_path);
    dirs.dedup();
    let mut refs: Vec<(String, bool)> = vec![]; // (resolved path, explicit)
    let mut seen: Vec<String> = vec![];
    for dir in &dirs {
        for candidate in [
            format!("{dir}/zcode.json"),
            format!("{dir}/.zcode/config.json"),
        ] {
            let resolved = node_normalize(&candidate);
            if std::fs::metadata(&resolved).is_err() {
                continue;
            }
            if seen.contains(&resolved) {
                continue;
            }
            seen.push(resolved.clone());
            refs.push((resolved, false));
        }
    }
    let mut sources: Vec<HookSource> = vec![];
    for (order, (path, _explicit)) in refs.iter().enumerate() {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Err(path.clone()),
        };
        let value: Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => return Err(path.clone()),
        };
        if !is_record(&value) || value.get("hooks").is_none() {
            continue;
        }
        let hooks = value["hooks"].clone();
        if !validate_hooks_config(&hooks) {
            return Err(path.clone());
        }
        sources.push(HookSource {
            canonical_path: path.clone(),
            discovery_order: order,
            kind: if node_basename(path) == "zcode.json" {
                "zcode.json"
            } else {
                ".zcode/config.json"
            },
            explicit: false,
            hooks,
        });
    }
    Ok(sources)
}

fn read_user_hooks(user_config_path: &str) -> Result<Option<Value>, String> {
    let content = match std::fs::read_to_string(user_config_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{e}")),
    };
    let value: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => return Err(format!("{e}")),
    };
    if !is_record(&value) || value.get("hooks").is_none() {
        return Ok(None);
    }
    let hooks = value["hooks"].clone();
    if !validate_hooks_config(&hooks) {
        return Err("Invalid hooks configuration in user config".to_string());
    }
    Ok(Some(hooks))
}

struct RuntimeRoot {
    enabled: bool,
    timeout_ms: f64,
    max_output_bytes: f64,
}

fn resolve_runtime_root(roots: &[Option<&Value>]) -> RuntimeRoot {
    let mut enabled = false;
    let mut timeout_ms = DEFAULT_HOOK_TIMEOUT_MS;
    let mut max_output_bytes = DEFAULT_HOOK_MAX_OUTPUT_BYTES;
    for root in roots.iter().flatten() {
        if root.get("enabled") == Some(&Value::Bool(true)) {
            enabled = true;
        }
        if let Some(t) = root.get("timeoutMs").and_then(|v| v.as_f64()) {
            timeout_ms = t;
        }
        if let Some(m) = root.get("maxOutputBytes").and_then(|v| v.as_f64()) {
            max_output_bytes = m;
        }
    }
    RuntimeRoot {
        enabled,
        timeout_ms: (timeout_ms.round()).max(1.0),
        max_output_bytes: (max_output_bytes.round()).max(1.0),
    }
}

// ---- sha256 (pure Rust to avoid new crate deps) ----

fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

fn json_stringify(value: &Value) -> String {
    value.to_string()
}

/// JS-style number: integral f64 must serialize as "5000", not "5000.0"
/// (JSON.stringify never emits a trailing .0; digest parity depends on it).
fn js_num(x: f64) -> Value {
    if x.fract() == 0.0 && x.is_finite() && x.abs() < 9.007_199_254_740_992e15 {
        Value::Number(serde_json::Number::from(x as i64))
    } else {
        serde_json::Number::from_f64(x)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

// ---- declaration/bundle digests (workspace-hook-digest.ts) ----

fn resolved_timeout_ms(hook: &Value, default_timeout_ms: f64) -> f64 {
    let explicit = hook.get("timeoutMs").and_then(|v| v.as_f64());
    let timeout = match explicit {
        Some(t) => t,
        None => {
            let command_timeout = hook.get("type").and_then(|t| t.as_str()) == Some("command")
                && hook.get("timeout").is_some();
            if command_timeout {
                hook["timeout"].as_f64().unwrap_or(0.0) * 1000.0
            } else {
                default_timeout_ms
            }
        }
    };
    (timeout.round()).max(1.0)
}

fn canonical_optional(v: Option<&Value>) -> Value {
    match v {
        None => json!(["unset"]),
        Some(v) => json!(["set", v]),
    }
}

struct HookEntry {
    review_item_id: String,
    event: String,
    matcher: Value,
    display_command: String,
    source_relative_path: String,
    configured_enabled: bool,
    digest: String,
    source_root_enabled: bool,
    declaration_enabled: bool,
}


// ---- trust store load (read-only; file schema from workspace-hook-trust-store-file.ts) ----

fn hex64(v: &Value) -> bool {
    match v.as_str() {
        Some(s) => s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        None => false,
    }
}

fn nonneg_int(v: &Value) -> bool {
    v.as_u64().is_some()
}

fn validate_trust_record(r: &Value) -> bool {
    let Some(obj) = r.as_object() else { return false };
    for k in obj.keys() {
        if !matches!(
            k.as_str(),
            "workspaceIdentity"
                | "hookDeclarationDigest"
                | "digestAlgorithm"
                | "decision"
                | "grantedAt"
                | "lastUsedAt"
                | "bundleDigestAtGrant"
                | "eventAtGrant"
                | "displayCommandAtGrant"
                | "sourcePathAtGrant"
                | "sourceDiscoveryOrderAtGrant"
                | "matcherAtGrant"
                | "matcherIndexAtGrant"
                | "hookIndexAtGrant"
                | "appVersionAtGrant"
        ) {
            return false;
        }
    }
    if !nonempty_str(obj.get("workspaceIdentity")) {
        return false;
    }
    if !hex64(obj.get("hookDeclarationDigest").unwrap_or(&Value::Null)) {
        return false;
    }
    if obj.get("digestAlgorithm").and_then(|v| v.as_str()) != Some("sha256") {
        return false;
    }
    if obj.get("decision").and_then(|v| v.as_str()) != Some("trusted") {
        return false;
    }
    if !iso_datetime(obj.get("grantedAt").unwrap_or(&Value::Null)) {
        return false;
    }
    if let Some(v) = obj.get("lastUsedAt") {
        if !iso_datetime(v) {
            return false;
        }
    }
    if let Some(v) = obj.get("bundleDigestAtGrant") {
        if !hex64(v) {
            return false;
        }
    }
    if !EVENT_NAMES.contains(&obj.get("eventAtGrant").and_then(|v| v.as_str()).unwrap_or("")) {
        return false;
    }
    if !nonempty_str(obj.get("displayCommandAtGrant")) || !nonempty_str(obj.get("sourcePathAtGrant")) {
        return false;
    }
    if let Some(v) = obj.get("sourceDiscoveryOrderAtGrant") {
        if !nonneg_int(v) {
            return false;
        }
    }
    if let Some(v) = obj.get("matcherAtGrant") {
        if !v.is_null() && !v.is_string() {
            return false;
        }
    }
    if let Some(v) = obj.get("matcherIndexAtGrant") {
        if !nonneg_int(v) {
            return false;
        }
    }
    if let Some(v) = obj.get("hookIndexAtGrant") {
        if !nonneg_int(v) {
            return false;
        }
    }
    if let Some(v) = obj.get("appVersionAtGrant") {
        if !nonempty_str(Some(v)) {
            return false;
        }
    }
    true
}

fn nonempty_str(v: Option<&Value>) -> bool {
    match v.and_then(|v| v.as_str()) {
        Some(s) => !s.trim().is_empty(),
        None => false,
    }
}

fn iso_datetime(v: &Value) -> bool {
    let Some(s) = v.as_str() else { return false };
    let bytes = s.as_bytes();
    // z.string().datetime() default: YYYY-MM-DDTHH:MM:SS[.sss]Z
    if bytes.len() < 20 {
        return false;
    }
    let date = &s[..10];
    let date_ok = date.len() == 10
        && date.as_bytes()[4] == b'-'
        && date.as_bytes()[7] == b'-'
        && date.bytes().enumerate().all(|(i, b)| {
            i == 4 || i == 7 || b.is_ascii_digit()
        });
    if !date_ok || &s[10..11] != "T" {
        return false;
    }
    if !s.ends_with('Z') {
        return false;
    }
    let time = &s[11..s.len() - 1];
    let main = time.split('.').next().unwrap_or("");
    let main_ok = main.len() == 8
        && main.as_bytes()[2] == b':'
        && main.as_bytes()[5] == b':'
        && main.bytes().enumerate().all(|(i, b)| {
            i == 2 || i == 5 || b.is_ascii_digit()
        });
    if !main_ok {
        return false;
    }
    if let Some(frac) = time.split_once('.').map(|(_, f)| f) {
        if frac.is_empty() || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
    }
    true
}

/// Read-only load: Ok(digests) when store missing/valid, Err(()) when corrupt.
fn load_trusted_digests(store_path: &str, workspace_identity: &str) -> Result<Vec<String>, ()> {
    let content = match std::fs::read_to_string(store_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(_) => return Ok(vec![]),
    };
    let value: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Err(()),
    };
    let Some(obj) = value.as_object() else { return Err(()) };
    if obj.len() != 2 || obj.get("schemaVersion") != Some(&json!(1)) {
        return Err(());
    }
    let Some(records) = obj.get("records").and_then(|v| v.as_array()) else {
        return Err(());
    };
    let mut keys: Vec<String> = vec![];
    let mut trusted = vec![];
    for r in records {
        if !validate_trust_record(r) {
            return Err(());
        }
        let identity = r["workspaceIdentity"].as_str().unwrap_or("").trim().to_string();
        let digest = r["hookDeclarationDigest"].as_str().unwrap_or("").to_string();
        let key = format!("{identity}\u{0}{digest}");
        if keys.contains(&key) {
            return Err(());
        }
        keys.push(key);
        if identity == workspace_identity {
            trusted.push(digest);
        }
    }
    Ok(trusted)
}

fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_default()
}

fn trust_store_path() -> Result<String, String> {
    let home = home_dir();
    let user_config_path = node_resolve(&home, ".zcode/cli/config.json");
    let mut storage_dir: Option<String> = None;
    if let Ok(content) = std::fs::read_to_string(&user_config_path) {
        let value: Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => {
                return Err(format!(
                    "Unable to read trusted user config for Workspace Hook Trust store: {user_config_path}"
                ))
            }
        };
        if is_record(&value) {
            if let Some(dir) = value.get("storage").and_then(|s| s.get("dir")) {
                if let Some(s) = dir.as_str() {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() {
                        storage_dir = Some(trimmed.to_string());
                    }
                }
            }
        }
    }
    let root = match storage_dir {
        Some(configured) => {
            if let Some(rest) = configured.strip_prefix("~/") {
                node_normalize(&format!("{home}/{rest}"))
            } else if configured.starts_with('/') {
                node_normalize(&configured)
            } else {
                node_resolve(&home, &configured)
            }
        }
        None => node_resolve(&home, ".zcode"),
    };
    Ok(format!("{root}/security/workspace-hook-trust-v1.json"))
}

// ---- inspect (workspace-hook-trust-cli.ts inspectWorkspaceHookTrust) ----

fn inspect_status(parsed: &TrustArgs, action: &str) -> Result<String, String> {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string());
    let selected = parsed.workspace.as_deref().map(str::trim).unwrap_or("");
    let looks_identity = selected.starts_with("local:")
        || selected.starts_with("remote:")
        || selected.starts_with("ssh:")
        || selected.starts_with("container:")
        || selected.starts_with("wsl:");
    let identity_input = if !selected.is_empty() && looks_identity {
        Some(selected.to_string())
    } else {
        None
    };
    let workspace_path = if identity_input.is_some() {
        node_normalize(&cwd)
    } else if !selected.is_empty() {
        node_resolve(&cwd, selected)
    } else {
        node_normalize(&cwd)
    };
    let workspace_identity = match &identity_input {
        Some(id) => id.clone(),
        None => workspace_path.clone(),
    };

    // discovery; errors from readWorkspaceHookProjectSources/readUserHooks surface as strings
    let sources = discover_sources(&workspace_path).map_err(|path| {
        format!("Unable to read Workspace Hook config: {path}")
    })?;
    let home = home_dir();
    let user_config_path = node_resolve(&home, ".zcode/cli/config.json");
    let user_hooks = read_user_hooks(&user_config_path)?;

    let default_hooks = json!({
        "enabled": false,
        "maxOutputBytes": DEFAULT_HOOK_MAX_OUTPUT_BYTES,
        "timeoutMs": DEFAULT_HOOK_TIMEOUT_MS,
    });
    let mut roots: Vec<Option<&Value>> = vec![Some(&default_hooks)];
    roots.push(user_hooks.as_ref());
    let source_roots: Vec<&Value> = sources.iter().map(|s| &s.hooks).collect();
    for r in &source_roots {
        roots.push(Some(r));
    }
    let runtime = resolve_runtime_root(&roots);

    // bundle snapshot: entries + digest; none when no declarations
    struct Snapshot {
        bundle_digest: Option<String>,
        entries: Vec<HookEntry>,
    }
    // Build entries (needs workspace_path for relative paths).
    let entries = {
        let mut entries = vec![];
        for (sfi, source) in sources.iter().enumerate() {
            let rel = {
                let value = node_relative(&workspace_path, &source.canonical_path);
                let value = if value.is_empty() {
                    node_basename(&source.canonical_path)
                } else {
                    value
                };
                value.replace('\\', "/")
            };
            for event in EVENT_NAMES {
                let Some(matchers) = source
                    .hooks
                    .get("events")
                    .and_then(|e| e.get(event))
                    .and_then(|v| v.as_array())
                else {
                    continue;
                };
                for (mi, matcher) in matchers.iter().enumerate() {
                    let hooks = matcher["hooks"].as_array().cloned().unwrap_or_default();
                    for (hi, hook) in hooks.iter().enumerate() {
                        entries.push(build_entry(
                            sfi, source, event, mi, hi, hook, matcher, &rel, &runtime,
                        ));
                    }
                }
            }
        }
        entries
    };
    let snapshot = if entries.is_empty() {
        None
    } else {
        let source_payload: Vec<Value> = sources
            .iter()
            .map(|s| {
                let rel = {
                    let value = node_relative(&workspace_path, &s.canonical_path);
                    let value = if value.is_empty() {
                        node_basename(&s.canonical_path)
                    } else {
                        value
                    };
                    value.replace('\\', "/")
                };
                json!([
                    rel,
                    s.discovery_order,
                    s.kind,
                    s.explicit,
                    canonical_optional(s.hooks.get("enabled")),
                    canonical_optional(s.hooks.get("timeoutMs")),
                    canonical_optional(s.hooks.get("maxOutputBytes")),
                ])
            })
            .collect();
        let hooks_payload: Vec<Value> = entries
            .iter()
            .map(|e| {
                json!([
                    e.digest,
                    e.source_root_enabled,
                    e.declaration_enabled,
                    runtime.enabled,
                    e.configured_enabled,
                ])
            })
            .collect();
        let bundle_payload = json!([
            "workspace-hook-bundle",
            1,
            source_payload,
            hooks_payload,
        ]);
        Some(Snapshot {
            bundle_digest: Some(sha256_hex(json_stringify(&bundle_payload).as_bytes())),
            entries,
        })
    };

    let store_path = trust_store_path()?;
    let loaded = load_trusted_digests(&store_path, &workspace_identity);
    let corrupt = loaded.is_err();
    let trusted: Vec<String> = loaded.unwrap_or_default();

    let (reason_code, items) = if corrupt {
        let items = snapshot
            .as_ref()
            .map(|s| {
                s.entries
                    .iter()
                    .map(|e| item_value(e, "pending_trust"))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        ("workspace_hooks_trust_store_corrupt", items)
    } else {
        match &snapshot {
            None => ("workspace_hooks_not_applicable", vec![]),
            Some(snap) => {
                let items: Vec<Value> = snap
                    .entries
                    .iter()
                    .map(|e| {
                        let state = if trusted.contains(&e.digest) {
                            "trusted_persistent"
                        } else {
                            "pending_trust"
                        };
                        item_value(e, state)
                    })
                    .collect();
                let enabled: Vec<&Value> =
                    items.iter().filter(|i| i["configuredEnabled"] == json!(true)).collect();
                let reason = if enabled.is_empty() {
                    "workspace_hooks_no_enabled_hooks"
                } else if enabled.iter().all(|i| {
                    i["trustState"] == json!("trusted_persistent")
                }) {
                    "workspace_hooks_trusted_persistent"
                } else {
                    "workspace_hooks_pending_trust"
                };
                (reason, items)
            }
        }
    };

    let bundle_digest = snapshot.as_ref().and_then(|s| s.bundle_digest.clone());
    let mut status = Map::new();
    status.insert("workspacePath".into(), json!(workspace_path));
    status.insert("workspaceIdentity".into(), json!(workspace_identity));
    status.insert(
        "bundleDigest".into(),
        bundle_digest.clone().map(Value::String).unwrap_or(Value::Null),
    );
    status.insert("reasonCode".into(), json!(reason_code));
    status.insert("items".into(), Value::Array(items.clone()));

    Ok(if parsed.json {
        format!("{}\n", pretty(&Value::Object(status)))
    } else {
        format!("{}\n", format_human(action, &status))
    })
}

#[allow(clippy::too_many_arguments)]
fn build_entry(
    sfi: usize,
    source: &HookSource,
    event: &str,
    mi: usize,
    hi: usize,
    hook: &Value,
    matcher: &Value,
    rel: &str,
    runtime: &RuntimeRoot,
) -> HookEntry {
    let source_root_enabled = source.hooks.get("enabled") != Some(&Value::Bool(false));
    let declaration_enabled = hook.get("enabled") != Some(&Value::Bool(false));
    let configured_enabled = source_root_enabled && declaration_enabled && runtime.enabled;
    let rtm = resolved_timeout_ms(hook, runtime.timeout_ms);
    let hook_type = hook["type"].as_str().unwrap_or("");
    let command = hook["command"].as_str().unwrap_or("").to_string();
    let display_command = if hook_type == "process" {
        match hook.get("args").and_then(|a| a.as_array()) {
            Some(args) if !args.is_empty() => {
                let mut parts = vec![command.clone()];
                parts.extend(args.iter().map(|a| a.as_str().unwrap_or("").to_string()));
                parts.join(" ")
            }
            _ => command.clone(),
        }
    } else {
        command.clone()
    };
    let execution = if hook_type == "process" {
        let args: Vec<Value> = hook
            .get("args")
            .and_then(|a| a.as_array())
            .cloned()
            .unwrap_or_default();
        json!(["process", command.clone(), args])
    } else {
        let async_flag = hook.get("async") == Some(&Value::Bool(true));
        let shell = match hook.get("shell") {
            None => json!(["unset"]),
            Some(Value::Bool(true)) => json!(["true"]),
            Some(Value::String(s)) => json!(["string", s.clone()]),
            Some(_) => json!(["unset"]),
        };
        json!(["command", command.clone(), async_flag, shell])
    };
    let payload = json!([
        "workspace-hook-declaration",
        1,
        rel,
        source.discovery_order,
        event,
        matcher.get("matcher").cloned().unwrap_or(Value::Null),
        mi,
        hi,
        execution,
        js_num(rtm),
        js_num(runtime.max_output_bytes),
    ]);
    HookEntry {
        review_item_id: format!("workspace-hook-{sfi}-{event}-{mi}-{hi}"),
        event: event.to_string(),
        matcher: matcher.get("matcher").cloned().unwrap_or(Value::Null),
        display_command,
        source_relative_path: rel.to_string(),
        configured_enabled,
        digest: sha256_hex(json_stringify(&payload).as_bytes()),
        source_root_enabled,
        declaration_enabled,
    }
}

fn item_value(e: &HookEntry, trust_state: &str) -> Value {
    let mut obj = Map::new();
    obj.insert("reviewItemId".into(), json!(e.review_item_id));
    obj.insert("event".into(), json!(e.event));
    obj.insert("matcher".into(), e.matcher.clone());
    obj.insert("displayCommand".into(), json!(e.display_command));
    obj.insert("sourcePath".into(), json!(e.source_relative_path));
    obj.insert("configuredEnabled".into(), json!(e.configured_enabled));
    obj.insert("hookDeclarationDigest".into(), json!(e.digest));
    obj.insert("trustState".into(), json!(trust_state));
    Value::Object(obj)
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

fn quote(value: &str) -> String {
    json!(value).to_string()
}

fn format_human(action: &str, status: &Map<String, Value>) -> String {
    let mut lines = vec![
        format!("Workspace Hook Trust ({action})"),
        format!("workspace: {}", status["workspaceIdentity"].as_str().unwrap_or("")),
        format!("path: {}", status["workspacePath"].as_str().unwrap_or("")),
        format!(
            "bundle: {}",
            status["bundleDigest"]
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "none".to_string())
        ),
        format!("state: {}", status["reasonCode"].as_str().unwrap_or("")),
    ];
    let items = status["items"].as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        lines.push("No Workspace Hook declarations found.".to_string());
    }
    for (index, item) in items.iter().enumerate() {
        let matcher = item["matcher"].as_str().unwrap_or("");
        lines.push(format!(
            "{}. [{}] [{}] {}{}",
            index + 1,
            item["trustState"].as_str().unwrap_or(""),
            if item["configuredEnabled"] == json!(true) { "enabled" } else { "disabled" },
            item["event"].as_str().unwrap_or(""),
            if matcher.is_empty() {
                String::new()
            } else {
                format!(" / {matcher}")
            }
        ));
        lines.push(format!("   {}", item["displayCommand"].as_str().unwrap_or("")));
        lines.push(format!("   source: {}", item["sourcePath"].as_str().unwrap_or("")));
        lines.push(format!(
            "   digest: {}",
            item["hookDeclarationDigest"].as_str().unwrap_or("")
        ));
    }
    if status["reasonCode"] == json!("workspace_hooks_pending_trust") {
        if let Some(bundle) = status["bundleDigest"].as_str() {
            let identity = status["workspaceIdentity"].as_str().unwrap_or("");
            lines.push("Pretrust exact declarations with:".to_string());
            lines.push(format!(
                "  zcode hooks trust grant --workspace {} --hook-digest <sha256>",
                quote(identity)
            ));
            lines.push(
                "Or trust every currently enabled declaration in this exact bundle with:"
                    .to_string(),
            );
            lines.push(format!(
                "  zcode hooks trust grant --workspace {} --all-current --bundle-digest {bundle}",
                quote(identity)
            ));
        }
    }
    if status["reasonCode"] == json!("workspace_hooks_trust_store_corrupt") {
        lines.push(
            "The persistent trust store is corrupt; grant/revoke are rejected until it is fixed."
                .to_string(),
        );
        lines.push(
            "The corrupted file was moved aside as workspace-hook-trust-v1.json.corrupt-<timestamp>."
                .to_string(),
        );
        lines.push(
            "Recovery: restore it from backup, or remove the leftover *.corrupt-* file so a fresh store is created, then re-run grant."
                .to_string(),
        );
    }
    lines.join("\n")
}
