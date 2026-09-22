//! Help text: byte-exact embed of the TS `formatCliHelp` English copy,
//! version substituted at runtime (version occurs exactly once, line 1).

pub fn format_help(version: &str) -> String {
    let template = include_str!("help_template.txt");
    // Template line 1 is "zcode <captured-version>"; rebuild with live version.
    let rest = template.split_once('\n').map(|(_, r)| r).unwrap_or("");
    format!("zcode {version}\n{rest}")
}
