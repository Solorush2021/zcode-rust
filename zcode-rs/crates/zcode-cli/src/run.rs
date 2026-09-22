//! Dispatch port of packages/cli/src/run.ts — byte-exact error order and formats.

use crate::args::{self, Parsed};
use crate::fallback::exec_fallback;
use crate::help::format_help;
use crate::{commands_cmd, doctor, plugins_cmd, skills_cmd};
use std::io::Write;

pub const VERSION: &str = env!("ZCODE_VERSION");

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

fn err_line(msg: &str) -> i32 {
    let _ = writeln!(std::io::stderr(), "{msg}");
    1
}

fn err_msg_help(msg: &str) -> i32 {
    let mut e = std::io::stderr();
    let _ = write!(e, "{msg}\n\n{}", format_help(VERSION));
    1
}

pub fn run(argv: &[String]) -> i32 {
    // Internal/special invocations (run.ts:287-308) — __internal-search is native;
    // hooks/plugin-host/dwf-child own modules (batch-3 port targets).
    match argv.first().map(|s| s.as_str()) {
        Some("__internal-search") => return crate::internal_search::run(&argv[1..]),
        Some("hooks") => return crate::hooks_cmd::run(argv),
        Some("__zcode-plugin-host") | Some("plugin-host") => return crate::plugin_dwf::run(argv),
        Some("__zcode-dwf-child") | Some("dwf-child") => return crate::plugin_dwf::run(argv),
        _ => {}
    }
    // plugin-host / dwf-child detection is argv/env-shaped; route to fallback.
    if argv.first().map(|s| s.as_str()) == Some("plugin-host")
        || argv.first().map(|s| s.as_str()) == Some("dwf-child")
    {
        return exec_fallback(argv);
    }

    let cleaned = match args::extract_disallowed_tools(argv) {
        Ok(a) => a,
        Err(msg) => return err_msg_help(&msg),
    };
    let parsed: Parsed = match args::parse_args(&cleaned) {
        Ok(p) => p,
        Err(msg) => return err_msg_help(&msg),
    };

    // locale (single-line error, no help — case024)
    let locale = parsed.str_of("locale");
    if let Some(l) = locale {
        if !LOCALES.contains(&l) {
            return err_line(&format!(
                "Unsupported --locale value: {l}. Supported locales: en-US, zh-CN, auto."
            ));
        }
    }
    // zh-CN copy tables live in the Node bundle; route locale-selected runs there
    // (batch-3: i18n_zh module may serve them natively).
    if locale == Some("zh-CN") {
        return crate::i18n_zh::dispatch_or_fallback(argv);
    }
    // Env-detected Chinese locale also selects zh copy — conservative fallback.
    if crate::i18n_zh::env_is_zh() {
        return crate::i18n_zh::dispatch_or_fallback(argv);
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
    // browser-use
    let browser_use = parsed.str_of("browser-use");
    if let Some(v) = browser_use {
        if v.to_lowercase() != "headless" {
            return err_line(&format!(
                "Unsupported --browser-use value: {v}. Supported value: headless."
            ));
        }
    }
    // surface
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
    // help before version (case020 -hv prints help)
    if parsed.flag("help") {
        let _ = write!(std::io::stdout(), "{}", format_help(VERSION));
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
    // cwd: only validated when a command that uses it runs (version exited above).
    if let Some(cwd) = parsed.str_of("cwd") {
        let path = std::path::Path::new(cwd);
        if !path.is_dir() {
            return err_line(&format!("--cwd directory does not exist: {cwd}"));
        }
        if std::env::set_current_dir(path).is_err() {
            return err_line(&format!("--cwd directory is not accessible: {cwd}"));
        }
    }

    if prompt || has_target {
        return exec_fallback(argv);
    }

    let json = parsed.flag("json");
    let verbose = parsed.flag("verbose");
    let rest: Vec<String> = parsed.positionals.iter().skip(1).cloned().collect();
    match command_name(&parsed.positionals).as_str() {
        "help" => {
            let _ = write!(std::io::stdout(), "{}", format_help(VERSION));
            0
        }
        "version" => {
            let _ = writeln!(std::io::stdout(), "{VERSION}");
            0
        }
        "app-server" | "agent-server" | "login" | "logout" | "tui" => {
            match command_name(&parsed.positionals).as_str() {
                "app-server" | "agent-server" => crate::warm_pool::run(argv),
                _ => exec_fallback(argv),
            }
        }
        "doctor" => doctor::run(json, verbose),
        "commands" => commands_cmd::run(&rest, json, verbose),
        "plugin" | "plugins" => plugins_cmd::run(
            &rest,
            json,
            parsed.flag("all"),
            parsed.flag("available"),
            parsed.str_of("scope"),
        ),
        "skills" => skills_cmd::run(&rest, json, verbose),
        other => {
            let mut e = std::io::stderr();
            let _ = write!(e, "Unknown command: {other}\n\n{}", format_help(VERSION));
            1
        }
    }
}
