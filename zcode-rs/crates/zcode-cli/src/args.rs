//! Byte-exact replica of the TS CLI argument layer:
//! `extractDisallowedToolsArgs` (arguments.ts) + `node:util parseArgs` (strict)
//! as pinned by bench/golden-args fixtures (Node 24 semantics).

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Bool,
    Str,
    Multi,
}

pub struct OptSpec {
    pub long: &'static str,
    pub short: Option<char>,
    pub kind: Kind,
}

const fn b(long: &'static str, short: Option<char>) -> OptSpec {
    OptSpec { long, short, kind: Kind::Bool }
}
const fn s(long: &'static str, short: Option<char>) -> OptSpec {
    OptSpec { long, short, kind: Kind::Str }
}

pub const OPTIONS: &[OptSpec] = &[
    b("help", Some('h')),
    b("json", None),
    s("output-format", None),
    b("no-color", None),
    b("no-browser", None),
    s("browser-use", None),
    s("browser-executable", None),
    s("prompt", Some('p')),
    b("memory-bench", None),
    OptSpec { long: "attach", short: None, kind: Kind::Multi },
    s("cwd", None),
    s("locale", None),
    s("resume", None),
    s("target", None),
    b("target-replace", None),
    b("continue", Some('c')),
    b("force", Some('f')),
    b("force-mcs", None),
    s("mode", None),
    b("verbose", None),
    b("version", Some('v')),
    b("prepare-storage", None),
    b("stdio", None),
    s("surface", None),
    b("all", Some('a')),
    b("available", None),
    b("keep-data", None),
    s("scope", Some('s')),
    OptSpec { long: "sparse", short: None, kind: Kind::Multi },
];

#[derive(Default)]
pub struct Parsed {
    pub values: std::collections::HashMap<&'static str, Val>,
    pub positionals: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Bool(bool),
    Str(Option<String>),
    Multi(Vec<String>),
}

impl Parsed {
    pub fn flag(&self, name: &str) -> bool {
        matches!(self.values.get(name), Some(Val::Bool(true)))
    }
    pub fn str_of(&self, name: &str) -> Option<&str> {
        match self.values.get(name) {
            Some(Val::Str(Some(v))) => Some(v.as_str()),
            _ => None,
        }
    }
    pub fn multi(&self, name: &str) -> Vec<String> {
        match self.values.get(name) {
            Some(Val::Multi(v)) => v.clone(),
            _ => Vec::new(),
        }
    }
}

fn spec(long: &str) -> Option<&'static OptSpec> {
    OPTIONS.iter().find(|o| o.long == long)
}
fn spec_short(c: char) -> Option<&'static OptSpec> {
    OPTIONS.iter().find(|o| o.short == Some(c))
}

fn display(o: &OptSpec) -> String {
    match o.short {
        Some(c) => format!("-{}, --{}", c, o.long),
        None => format!("--{}", o.long),
    }
}

fn unknown_option_err(token: &str) -> String {
    // Node ships this hint ending with a double-quote only; replicate byte-exactly.
    format!(
        "Unknown option '{token}'. To specify a positional argument starting with a '-', \
place it at the end of the command after '--', as in '-- \"{token}\""
    )
}

fn ambiguous_err(token: &str, o: &OptSpec) -> String {
    // The "or '-s-XYZ'" alternative appears only when the token was the short form.
    let use_hint = if token.starts_with("--") {
        format!("use '--{}=-XYZ'.", o.long)
    } else {
        match o.short {
            Some(c) => format!("use '--{}=-XYZ' or '-{c}-XYZ'.", o.long),
            None => format!("use '--{}=-XYZ'.", o.long),
        }
    };
    format!(
        "Option '{token}' argument is ambiguous.\nDid you forget to specify the option argument for '{token}'?\nTo specify an option argument starting with a dash {use_hint}"
    )
}

fn set_value(p: &mut Parsed, o: &OptSpec, value: String) {
    match o.kind {
        Kind::Bool => {
            p.values.insert(o.long, Val::Bool(true));
        }
        Kind::Str => {
            p.values.insert(o.long, Val::Str(Some(value))); // duplicates: last wins (case050)
        }
        Kind::Multi => match p.values.get_mut(o.long) {
            Some(Val::Multi(v)) => v.push(value),
            _ => {
                p.values.insert(o.long, Val::Multi(vec![value]));
            }
        },
    }
}

/// `--disallowedTools` / `--disallowed-tools` extraction (arguments.ts:127).
/// The disallowlist only feeds the agent runtime; the fallback path receives
/// the original argv, so the parsed values are dropped here.
pub fn extract_disallowed_tools(argv: &[String]) -> Result<Vec<String>, String> {
    let is_option_token = |v: &str| v == "--" || (v.starts_with('-') && v.len() > 1);
    let mut args = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];
        if arg.starts_with("--disallowedTools=") || arg.starts_with("--disallowed-tools=") {
            i += 1;
            continue;
        }
        if arg == "--disallowedTools" || arg == "--disallowed-tools" {
            let mut consumed = false;
            while i + 1 < argv.len() && !is_option_token(&argv[i + 1]) {
                consumed = true;
                i += 1;
            }
            if !consumed {
                return Err(format!("{arg} requires at least one tool."));
            }
            i += 1;
            continue;
        }
        args.push(arg.clone());
        i += 1;
    }
    Ok(args)
}

pub fn parse_args(argv: &[String]) -> Result<Parsed, String> {
    let mut p = Parsed::default();
    let mut terminated = false;
    let mut i = 0;
    while i < argv.len() {
        let tok = &argv[i];
        if terminated {
            p.positionals.push(tok.clone());
            i += 1;
            continue;
        }
        if tok == "--" {
            terminated = true;
            i += 1;
            continue;
        }
        if let Some(body) = tok.strip_prefix("--") {
            let (name, inline) = match body.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (body, None),
            };
            let o = spec(name).ok_or_else(|| unknown_option_err(&format!("--{name}")))?;
            match o.kind {
                Kind::Bool => {
                    if inline.is_some() {
                        return Err(format!("Option '{}' does not take an argument", display(o)));
                    }
                    p.values.insert(o.long, Val::Bool(true));
                }
                Kind::Str | Kind::Multi => {
                    let value = match inline {
                        Some(v) => v.to_string(),
                        None => {
                            let next = argv.get(i + 1);
                            match next {
                                None => {
                                    return Err(format!(
                                        "Option '{} <value>' argument missing",
                                        display(o)
                                    ))
                                }
                                Some(n) if is_option_token(n) => {
                                    return Err(ambiguous_err(tok, o))
                                }
                                Some(n) => {
                                    i += 1;
                                    n.clone()
                                }
                            }
                        }
                    };
                    set_value(&mut p, o, value);
                }
            }
            i += 1;
            continue;
        }
        if tok.len() > 1 && tok.starts_with('-') {
            let chars: Vec<char> = tok.chars().skip(1).collect();
            let mut ci = 0;
            while ci < chars.len() {
                let c = chars[ci];
                let o = spec_short(c)
                    .ok_or_else(|| unknown_option_err(&format!("-{c}")))?;
                match o.kind {
                    Kind::Bool => {
                        p.values.insert(o.long, Val::Bool(true));
                        ci += 1;
                    }
                    Kind::Str | Kind::Multi => {
                        let mut rest: String = chars[ci + 1..].iter().collect();
                        if rest.starts_with('=') {
                            rest.remove(0);
                        }
                        let value = if rest.is_empty() {
                            let next = argv.get(i + 1);
                            match next {
                                None => {
                                    return Err(format!(
                                        "Option '{} <value>' argument missing",
                                        display(o)
                                    ))
                                }
                                Some(n) if is_option_token(n) => {
                                    return Err(ambiguous_err(tok, o))
                                }
                                Some(n) => {
                                    i += 1;
                                    n.clone()
                                }
                            }
                        } else {
                            rest
                        };
                        set_value(&mut p, o, value);
                        break;
                    }
                }
            }
            i += 1;
            continue;
        }
        p.positionals.push(tok.clone());
        i += 1;
    }
    Ok(p)
}

fn is_option_token(v: &str) -> bool {
    v == "--" || (v.starts_with('-') && v.len() > 1)
}
