//! zh-CN locale path (batch-3 C): native serving of the run.rs-owned surfaces
//! when zh is selected, instead of paying a full Node boot (~360 ms / ~330 MB).
//!
//! Copy-table source: `apps/zcode-cli/packages/i18n/src/locales/zh-CN.ts`
//! (`cli.help`, `cli.errors.localeUnsupported`) — the only two localized strings
//! on the entire run.ts dispatch surface. Upstream behavior audited against
//! `dist/zcode.cjs` under `LANG=zh_CN.UTF-8 LC_ALL=zh_CN.UTF-8`:
//!
//! - `cli.errors.localeUnsupported` is ALWAYS English in practice: run.ts:130
//!   renders it via `getZCodeCopy()` with no locale → en-US catalog (verified).
//! - Help is locale-aware ONLY when `--locale` resolves to zh-CN: explicit
//!   `zh-CN`, or `auto` + zh env. Bare zh env with no flag → en-US help
//!   (`resolveLocale(undefined, …)` falls back to en-US; verified).
//! - The `Unknown command: X\n\n` prefix is hardcoded English; the help
//!   appended after it IS locale-aware.
//! - Every pre-dispatch validation error (mode/browser-use/surface/
//!   output-format/target/resume/memory-bench/force-mcs/cwd and parse-arg
//!   errors) is hardcoded English upstream; parse-arg errors append EN help
//!   (`writeHelp(ctx.stderr)` without a locale).
//! - Command-specific output (skills/plugins/commands lists, tui, prompt runs,
//!   login, protocol servers) is formatted in Node → `exec_fallback`.
//!
//! Therefore `run_zh` mirrors run.rs's validation ORDER exactly with the same
//! English strings (byte-identical to the EN path) and renders the zh help
//! table at the two locale-aware sites (help flag/`help` command; help appended
//! after `Unknown command:`). run.rs is intentionally untouched; this module is
//! invoked from its zh gate (`dispatch_or_fallback`).

use crate::args::{self, Parsed};
use crate::fallback::exec_fallback;
use crate::help::format_help;
use crate::run::VERSION;
use std::io::Write;

// === zh copy table (packages/i18n/src/locales/zh-CN.ts) ====================

/// zh-CN `cli.help` body — everything after the leading `zcode {version}` line.
/// Byte-exact capture of `dist/zcode.cjs --locale zh-CN --help`.
const HELP_ZH_BODY: &str = r#"

用法:
  zcode [command] [options]

不传 command 时，zcode 会打开全屏 TUI。

命令:
  app-server 运行 ZCode Protocol stdio app server
  commands   列出自定义 slash commands（`commands list`）
  doctor     检查运行时和打包假设
  login [zai|bigmodel]  通过浏览器授权登录
  logout     删除共享的 Z.AI 登录凭据
  plugins    管理插件与市场（`plugins list|install|uninstall|enable|disable|update|validate|marketplace ...`；别名 plugin）
  skills     列出本地 skills（`skills list`）
  tui        打开终端 UI
  version    打印 CLI 版本

选项:
  -h, --help       显示帮助
  -v, --version    显示版本
  -p, --prompt <text>  单次运行 prompt，不打开 TUI
  --memory-bench   配合 --prompt 开启自动 Memory 提取并等待后退出（需已开启 Memory）
  --browser-use <mode> 启用 Browser Use backend（当前支持：headless）
  --surface <surface>  设置无头 prompt/app-server 的呈现面：terminal 或 desktop
  --browser-executable <path> headless Browser Use 使用的 Chrome/Chromium 路径
  --attach <path>  给 --prompt 附加本地文件；可重复传入
  --cwd <path>     从指定目录运行命令
  --disallowed-tools, --disallowedTools <tools...>
    从本次 prompt/TUI 的可用工具集中移除整个工具，不修改持久化配置。
    工具名用逗号或空格分隔，例如 "Bash Edit"。
    "Bash(git *)" 也会移除整个 Bash，不支持按命令内容匹配。
  --force-mcs      对 Anthropic provider 强制启用 mid-conversation system 投影
  --locale <locale>  UI 语言：en-US、zh-CN 或 auto
  --mode <mode>    prompt 权限模式：build、edit、plan 或 yolo（--prompt 默认 yolo）
  --resume <sessionId>  按 sessionId 恢复持久化 session（sess_...）
  --target <text>  在 headless 模式运行或设置 session goal
  --target-replace 替换 --target 已存在的 goal
  -c, --continue        恢复当前目录最近的 session
  --json           在支持的命令中输出机器可读 JSON
  --no-browser     不打开浏览器，只打印 OAuth URL
  --no-color       禁用 ANSI 颜色
  --verbose        打印更多诊断信息

Slash Commands:
  /help [command]       显示 slash command 帮助
  /login                使用 Z.AI OAuth 登录
  /logout               删除共享的 Z.AI 登录凭据
  /compact [instructions]  压缩当前对话
  /expert [status|resume|stop|<task>]  运行或管理 expert workflow
  /dwf [list|cancel|resume]  列出、取消或恢复 dynamic workflow run
  /fork [latest|checkpointId]  从 workspace checkpoint 派生新 session
  /mcp [list|status|connect|disconnect]  查看或管理 MCP servers
  /mode [mode]          查看或切换权限模式：build、edit、plan 或 yolo
  /model [id]           查看或切换当前 session 模型
  /new                  在 TUI 中开始新 session
  /resume [sessionId]   按 sessionId 恢复 session；省略时恢复当前 cwd 最新 session
  /rewind [latest|checkpointId]  查看最新 checkpoint 或恢复 workspace 文件
  /skill [name] [task]  列出 skills，或强制下一次 prompt 加载某个 skill
  /goal [action]        查看或设置当前 session goal
"#;

/// zh-CN `cli.errors.localeUnsupported` (zh-CN.ts:7). NOTE: unreachable
/// upstream — run.ts renders this error through `getZCodeCopy()` with no
/// locale argument, which always resolves to the en-US catalog (verified
/// against the bundle: `--locale xx` under zh env prints English). Kept here
/// for table fidelity with the source catalog.
#[allow(dead_code)]
const LOCALE_UNSUPPORTED_ZH_TMPL: &str =
    "不支持的 --locale 值：{value}。支持的语言：en-US、zh-CN、auto。";

pub fn format_help_zh() -> String {
    format!("zcode {VERSION}{}", HELP_ZH_BODY)
}

// === locale detection (mirror of packages/i18n/src/locale.ts detectLocale) ==

const LOCALE_ENV_KEYS: &[&str] = &["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"];

/// True when the environment resolves to the zh-CN catalog. Env-key priority
/// and value normalization mirror `detectLocale`: candidates from LC_ALL,
/// LC_MESSAGES, LANG, then LANGUAGE (`:`-separated list); encoding (`.UTF-8`)
/// and modifier (`@`) suffixes stripped, `_` → `-`; `c`/`posix` ignored;
/// `zh`/`zh-*` → zh-CN, `en`/`en-*` → en-US.
pub fn env_is_zh() -> bool {
    detected_is_zh()
}

fn detected_is_zh() -> bool {
    for key in LOCALE_ENV_KEYS {
        let value = match std::env::var(key) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for candidate in locale_candidates(key, &value) {
            match normalize_locale_tag(&candidate) {
                Some(locale) => return locale == "zh-CN",
                None => continue,
            }
        }
    }
    false
}

fn locale_candidates(key: &str, value: &str) -> Vec<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if key == "LANGUAGE" {
        return trimmed
            .split(':')
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect();
    }
    vec![trimmed.to_string()]
}

fn normalize_locale_tag(value: &str) -> Option<String> {
    // stripLocaleDecorators: cut encoding (`.`) and modifier (`@`) suffixes.
    let no_encoding = value.split('.').next().unwrap_or("");
    let no_modifier = no_encoding.split('@').next().unwrap_or("");
    let tag = no_modifier.replace('_', "-");
    if tag.is_empty() {
        return None;
    }
    let lower = tag.to_lowercase();
    if lower == "c" || lower == "posix" {
        return None;
    }
    if lower == "zh" || lower.starts_with("zh-") {
        return Some("zh-CN".to_string());
    }
    if lower == "en" || lower.starts_with("en-") {
        return Some("en-US".to_string());
    }
    None
}

/// Which catalog renders help for this invocation. Mirrors
/// `resolveLocale(options.locale, detectedLocale)`: only an explicit zh-CN or
/// `auto` resolved from a zh environment selects zh; `undefined` falls back to
/// en-US (the upstream quirk that keeps bare-zh-env help English).
fn help_is_zh(parsed: &Parsed) -> bool {
    match parsed.str_of("locale") {
        Some("zh-CN") => true,
        Some("auto") => detected_is_zh(),
        _ => false, // None → en-US default; explicit "en-US"
    }
}

// === dispatch (mirror of run.rs's validation order; EN strings duplicated ===
// === because run.rs is off-limits for edits) ===============================

const EMPTY_TARGET_ERROR: &str = "--target requires non-empty text.";
const FORCE_MCS_SCOPE_ERROR: &str = "--force-mcs can only be used with --prompt, --target, or tui.";
const TARGET_REPLACE_REQUIRES_TARGET_ERROR: &str = "--target-replace requires --target.";
const TARGET_CONFLICTS_WITH_PROMPT_ERROR: &str = "--target cannot be used with --prompt. Use either --target <objective> or --prompt \"/goal <objective>\".";
const BROWSER_EXECUTABLE_REQUIRES_HEADLESS_ERROR: &str =
    "--browser-executable requires --browser-use=headless.";
const BROWSER_USE_SCOPE_ERROR: &str =
    "--browser-use=headless can only be used with --prompt, --target, or tui.";
const SURFACE_SCOPE_ERROR: &str =
    "--surface can only be used with --prompt, --target, app-server, or agent-server.";
const MEMORY_BENCH_SCOPE_ERROR: &str = "--memory-bench can only be used with -p/--prompt.";
const RESUME_CONFLICT_ERROR: &str = "--resume and --continue cannot be used together.";
const OUTPUT_FORMATS: &[&str] = &["text", "json", "stream-json"];
const LOCALES: &[&str] = &["en-US", "zh-CN", "auto"];

fn command_name(positionals: &[String]) -> String {
    positionals.first().cloned().unwrap_or_else(|| "tui".to_string())
}

fn is_force_mcs_supported(positionals: &[String], prompt: bool, has_target: bool) -> bool {
    prompt || has_target || command_name(positionals) == "tui"
}

fn is_presentation_surface_supported(positionals: &[String], prompt: bool, has_target: bool) -> bool {
    let command = command_name(positionals);
    prompt || has_target || command == "app-server" || command == "agent-server"
}

/// node `path.resolve(base, p)`: absolute input is used as-is, otherwise
/// joined onto the process cwd; both lexically normalized (`.`/`..` folded).
fn resolve_path(p: &str) -> std::path::PathBuf {
    use std::path::Component;
    let path = std::path::Path::new(p);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut out: Vec<std::ffi::OsString> = Vec::new();
    for comp in joined.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                // Clamp at the filesystem root like path.resolve does.
                let only_root = out.len() == 1 && out[0] == Component::RootDir.as_os_str();
                if !out.is_empty() && !only_root {
                    out.pop();
                }
            }
            c => out.push(c.as_os_str().to_os_string()),
        }
    }
    out.into_iter().collect()
}

fn err_line(msg: &str) -> i32 {
    let _ = writeln!(std::io::stderr(), "{msg}");
    1
}

fn help_text(zh: bool) -> String {
    if zh {
        format_help_zh()
    } else {
        format_help(VERSION)
    }
}

/// Error message followed by help. Parse-arg errors upstream append the EN
/// help (`writeHelp(ctx.stderr)` without a locale) even under zh, so `zh` must
/// only be passed where the upstream call site is locale-aware.
fn err_msg_help_z(msg: &str, zh: bool) -> i32 {
    let mut e = std::io::stderr();
    let _ = write!(e, "{msg}\n\n{}", help_text(zh));
    1
}

/// Entry point run.rs already routes to for zh-selected invocations.
pub fn dispatch_or_fallback(argv: &[String]) -> i32 {
    let cleaned = match args::extract_disallowed_tools(argv) {
        Ok(a) => a,
        Err(msg) => return err_msg_help_z(&msg, false), // EN help upstream
    };
    let parsed: Parsed = match args::parse_args(&cleaned) {
        Ok(p) => p,
        Err(msg) => return err_msg_help_z(&msg, false), // EN help upstream
    };
    let zh = help_is_zh(&parsed);

    // locale (already validated by run.rs before this gate; EN single-line
    // error upstream regardless of locale — re-checked defensively).
    let locale = parsed.str_of("locale");
    if let Some(l) = locale {
        if !LOCALES.contains(&l) {
            return err_line(&format!(
                "Unsupported --locale value: {l}. Supported locales: en-US, zh-CN, auto."
            ));
        }
    }
    // mode
    if let Some(m) = parsed.str_of("mode") {
        let lower = m.to_lowercase();
        if !matches!(lower.as_str(), "build" | "plan" | "edit" | "yolo") {
            return err_line(&format!(
                "Unsupported --mode value: {m}. Supported modes: build, edit, plan, yolo."
            ));
        }
    }
    // browser-use value
    let browser_use = parsed.str_of("browser-use");
    if let Some(v) = browser_use {
        if v.to_lowercase() != "headless" {
            return err_line(&format!(
                "Unsupported --browser-use value: {v}. Supported value: headless."
            ));
        }
    }
    // surface value
    if let Some(v) = parsed.str_of("surface") {
        let lower = v.to_lowercase();
        if lower != "terminal" && lower != "desktop" {
            return err_line(&format!(
                "Unsupported --surface value: {v}. Supported surfaces: terminal, desktop."
            ));
        }
    }
    // browser-executable requires headless
    let browser_executable = parsed.str_of("browser-executable");
    if browser_executable.is_some() && browser_use != Some("headless") {
        return err_line(BROWSER_EXECUTABLE_REQUIRES_HEADLESS_ERROR);
    }
    // resume/continue conflict
    let cont = parsed.flag("continue");
    let resume = parsed.str_of("resume");
    if cont && resume.is_some() {
        return err_line(RESUME_CONFLICT_ERROR);
    }
    // output-format
    if let Some(v) = parsed.str_of("output-format") {
        if !OUTPUT_FORMATS.contains(&v) {
            return err_line(&format!(
                "--output-format must be one of text, json, stream-json (received: {v})."
            ));
        }
    }
    // target normalize
    let target = parsed.str_of("target");
    let target_replace = parsed.flag("target-replace");
    let has_target = match target {
        None => {
            if target_replace {
                return err_line(TARGET_REPLACE_REQUIRES_TARGET_ERROR);
            }
            false
        }
        Some(t) => {
            if t.trim().is_empty() {
                return err_line(EMPTY_TARGET_ERROR);
            }
            true
        }
    };
    // target+prompt conflict
    if has_target && parsed.str_of("prompt").is_some() {
        return err_line(TARGET_CONFLICTS_WITH_PROMPT_ERROR);
    }
    // surface scope
    if parsed.str_of("surface").is_some()
        && !is_presentation_surface_supported(
            &parsed.positionals,
            parsed.str_of("prompt").is_some(),
            has_target,
        )
    {
        return err_line(SURFACE_SCOPE_ERROR);
    }
    // help before version
    if parsed.flag("help") {
        let _ = write!(std::io::stdout(), "{}", help_text(zh));
        return 0;
    }
    if parsed.flag("version") {
        let _ = writeln!(std::io::stdout(), "{VERSION}");
        return 0;
    }
    let prompt = parsed.str_of("prompt").is_some();
    // memory-bench scope
    if parsed.flag("memory-bench") && (!prompt || !parsed.positionals.is_empty()) {
        return err_line(MEMORY_BENCH_SCOPE_ERROR);
    }
    // browser-use scope
    if browser_use == Some("headless")
        && !is_force_mcs_supported(&parsed.positionals, prompt, has_target)
    {
        return err_line(BROWSER_USE_SCOPE_ERROR);
    }
    // force-mcs scope
    if parsed.flag("force-mcs") && !is_force_mcs_supported(&parsed.positionals, prompt, has_target)
    {
        return err_line(FORCE_MCS_SCOPE_ERROR);
    }
    // cwd — faithful port of resolveCliCwd (packages/cli/src/cwd.ts). NOTE:
    // run.rs's "directory does not exist" variant was never golden-validated
    // (no EN case reaches the cwd error); the Node bundle prints these exact
    // strings against the RESOLVED absolute path.
    if let Some(cwd) = parsed.str_of("cwd") {
        if cwd.is_empty() {
            return err_line("--cwd requires a non-empty path.");
        }
        let resolved = resolve_path(cwd);
        match std::fs::metadata(&resolved) {
            Err(_) => {
                return err_line(&format!(
                    "--cwd path is not accessible: {}",
                    resolved.display()
                ))
            }
            Ok(m) if !m.is_dir() => {
                return err_line(&format!(
                    "--cwd must point to a directory: {}",
                    resolved.display()
                ))
            }
            Ok(_) => {}
        }
        if std::env::set_current_dir(&resolved).is_err() {
            return err_line(&format!(
                "--cwd path is not accessible: {}",
                resolved.display()
            ));
        }
    }

    if prompt || has_target {
        return exec_fallback(argv);
    }

    let json = parsed.flag("json");
    let verbose = parsed.flag("verbose");
    match command_name(&parsed.positionals).as_str() {
        "help" => {
            let _ = write!(std::io::stdout(), "{}", help_text(zh));
            0
        }
        "version" => {
            let _ = writeln!(std::io::stdout(), "{VERSION}");
            0
        }
        "app-server" | "agent-server" => crate::warm_pool::run(argv),
        "login" | "logout" | "tui" => exec_fallback(argv),
        // Runtime facts only; no zh copy exists upstream, so the native
        // rendering stays byte-identical (deviation lines masked in parity).
        "doctor" => crate::doctor::run(json, verbose),
        // List formatting is localized in Node → hand the full argv back.
        "commands" | "plugin" | "plugins" | "skills" => exec_fallback(argv),
        other => {
            let mut e = std::io::stderr();
            let _ = write!(e, "Unknown command: {other}\n\n{}", help_text(zh));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zh_help_matches_version_prefix() {
        let h = format_help_zh();
        assert!(h.starts_with(&format!("zcode {VERSION}\n\n用法:")));
        assert!(h.ends_with("/goal [action]        查看或设置当前 session goal\n"));
    }

    #[test]
    fn locale_tag_normalization() {
        assert_eq!(normalize_locale_tag("zh_CN.UTF-8").as_deref(), Some("zh-CN"));
        assert_eq!(normalize_locale_tag("zh").as_deref(), Some("zh-CN"));
        assert_eq!(normalize_locale_tag("en_US").as_deref(), Some("en-US"));
        assert_eq!(normalize_locale_tag("C"), None);
        assert_eq!(normalize_locale_tag("POSIX"), None);
        assert_eq!(normalize_locale_tag(""), None);
    }

    #[test]
    fn language_list_splits() {
        assert_eq!(locale_candidates("LANGUAGE", "zh:en ").len(), 2);
        assert_eq!(locale_candidates("LANG", "zh_CN.UTF-8").len(), 1);
        assert!(locale_candidates("LANG", "  ").is_empty());
    }
}
