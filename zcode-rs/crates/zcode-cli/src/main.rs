use std::io::Write;
use std::process::ExitCode;

mod args;
mod commands_cmd;
mod doctor;
mod fallback;
mod help;
mod hooks_cmd;
mod i18n_zh;
mod internal_search;
mod plugin_dwf;
mod plugins_cmd;
mod run;
mod skills_cmd;
mod warm_pool;

fn main() -> ExitCode {
    // Suppress panic backtraces: the TS binary prints clean errors, never Rust panic noise.
    std::panic::set_hook(Box::new(|info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unexpected error".to_string());
        let _ = writeln!(std::io::stderr(), "Error: {msg}");
    }));
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let code = run::run(&argv);
    ExitCode::from(code as u8)
}
