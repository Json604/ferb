//! The `ferb` argument parser (ferb/cli/__init__.py `build_parser`), as argparse reads the same command lines:
//! subcommands, `--flag`, `--option value` and `--option=value`, unique long-option prefixes, choices and integers.
//! A command line argparse refuses exits with status 2; its usage text is not reproduced (FERB.app never runs one).

use std::collections::BTreeMap;

use serde_json::{json, Value};

#[derive(Clone, Copy)]
enum Kind {
    Flag,
    Value { int: bool },
}

#[derive(Clone, Copy)]
struct Opt {
    flag: &'static str,
    kind: Kind,
    default: Option<&'static str>,
    choices: &'static [&'static str],
}

#[derive(Clone, Copy)]
struct Pos {
    dest: &'static str,
    optional: bool,
    default: Option<&'static str>,
    choices: &'static [&'static str],
}

/// (dest, required, the commands).
type Sub = (&'static str, bool, Vec<(&'static str, Parser)>);

struct Parser {
    opts: Vec<Opt>,
    pos: Vec<Pos>,
    /// (dest, required, commands)
    sub: Option<Sub>,
    /// Options that can't be given together.
    exclusive: &'static [&'static str],
}

const fn flag(f: &'static str) -> Opt {
    Opt {
        flag: f,
        kind: Kind::Flag,
        default: None,
        choices: &[],
    }
}
const fn value(f: &'static str, default: Option<&'static str>) -> Opt {
    Opt {
        flag: f,
        kind: Kind::Value { int: false },
        default,
        choices: &[],
    }
}
const fn pos(dest: &'static str) -> Pos {
    Pos {
        dest,
        optional: false,
        default: None,
        choices: &[],
    }
}
const fn opt_pos(dest: &'static str, default: Option<&'static str>) -> Pos {
    Pos {
        dest,
        optional: true,
        default,
        choices: &[],
    }
}

fn p(opts: Vec<Opt>, pos: Vec<Pos>) -> Parser {
    Parser {
        opts,
        pos,
        sub: None,
        exclusive: &[],
    }
}

/// profiles.BROWSERS' keys (ferb/act/browser/profiles.py), the choices of `--browser`.
pub const BROWSER_KEYS: &[&str] = &[
    "chrome",
    "chrome-beta",
    "chrome-dev",
    "chrome-canary",
    "chromium",
    "brave",
    "edge",
    "edge-beta",
    "edge-dev",
    "edge-canary",
    "vivaldi",
    "arc",
    "opera",
];
pub const LOG_FLAGS: [&str; 7] = [
    "tasks", "trail", "tools", "browsers", "policy", "actions", "timers",
];
const LOG_OPTS: [&str; 7] = [
    "--tasks",
    "--trail",
    "--tools",
    "--browsers",
    "--policy",
    "--actions",
    "--timers",
];

fn parser() -> Parser {
    let logs = p(LOG_OPTS.iter().map(|f| flag(f)).collect(), vec![]);
    let mut trace = p(
        vec![flag("--full"), flag("--json")],
        vec![opt_pos("task_id", None)],
    );
    trace.exclusive = &["full", "json"];
    let verdict = p(
        vec![value("--note", Some(""))],
        vec![
            pos("task_id"),
            Pos {
                dest: "verdict",
                optional: false,
                default: None,
                choices: &["pass", "fail"],
            },
        ],
    );
    let daemon = p(
        vec![],
        vec![Pos {
            dest: "action",
            optional: false,
            default: None,
            choices: &["run"],
        }],
    );
    let memory = Parser {
        opts: vec![],
        pos: vec![],
        sub: Some((
            "mcmd",
            true,
            vec![
                ("list", p(vec![flag("--proposed"), flag("--all")], vec![])),
                ("show", p(vec![], vec![opt_pos("subject", Some("owner"))])),
                (
                    "add",
                    p(
                        vec![
                            Opt {
                                flag: "--kind",
                                kind: Kind::Value { int: false },
                                default: Some("fact"),
                                choices: &["fact", "preference", "person", "project", "env"],
                            },
                            value("--subject", Some("owner")),
                            value("--key", None),
                        ],
                        vec![pos("content")],
                    ),
                ),
                ("profile", p(vec![], vec![])),
                ("retract", p(vec![], vec![pos("id")])),
                ("edit", p(vec![], vec![pos("id"), pos("content")])),
                (
                    "forget",
                    p(vec![flag("--keep-sources"), flag("--yes")], vec![pos("id")]),
                ),
                ("dashboard", p(vec![], vec![])),
                ("eval-export", p(vec![], vec![])),
                ("embeddings", p(vec![], vec![])),
                ("digest", p(vec![], vec![])),
                (
                    "search",
                    p(
                        vec![
                            value("--kind", None),
                            value("--app", None),
                            value("--person", None),
                        ],
                        vec![pos("query")],
                    ),
                ),
                ("rollback", p(vec![], vec![pos("batch")])),
            ],
        )),
        exclusive: &[],
    };
    let runtimes = Parser {
        opts: vec![],
        pos: vec![],
        sub: Some((
            "rcmd",
            false,
            vec![
                ("pins", p(vec![], vec![])),
                (
                    "use",
                    p(
                        vec![value("--model", None)],
                        vec![pos("role"), pos("runtime")],
                    ),
                ),
            ],
        )),
        exclusive: &[],
    };
    let restore = p(vec![], vec![opt_pos("id", None)]);
    let pair_like = || {
        p(
            vec![Opt {
                flag: "--browser",
                kind: Kind::Value { int: false },
                default: None,
                choices: BROWSER_KEYS,
            }],
            vec![pos("profile")],
        )
    };
    let browsers = Parser {
        opts: vec![],
        pos: vec![],
        sub: Some((
            "bcmd",
            false,
            vec![
                (
                    "extension",
                    p(
                        vec![value("--out", None)],
                        vec![Pos {
                            dest: "action",
                            optional: true,
                            default: Some("path"),
                            choices: &["install", "path", "package"],
                        }],
                    ),
                ),
                ("pair", pair_like()),
                ("unpair", pair_like()),
            ],
        )),
        exclusive: &[],
    };
    let setup = p(
        vec![
            flag("--update"),
            flag("--codex-login"),
            flag("--codex-logout"),
            flag("--cli"),
            flag("--claude-logout"),
            flag("--cleanup"),
            value("--ok", None),
            flag("--service"),
            flag("--app-service"),
        ],
        vec![],
    );
    let stats = p(
        vec![Opt {
            flag: "--days",
            kind: Kind::Value { int: true },
            default: Some("7"),
            choices: &[],
        }],
        vec![],
    );
    let uninstall = p(vec![flag("--purge"), flag("--yes")], vec![]);
    Parser {
        opts: vec![],
        pos: vec![],
        sub: Some((
            "cmd",
            true,
            vec![
                ("logs", logs),
                ("trace", trace),
                ("verdict", verdict),
                ("daemon", daemon),
                ("memory", memory),
                ("runtimes", runtimes),
                ("restore", restore),
                ("browsers", browsers),
                ("ps", p(vec![], vec![])),
                ("mcp", p(vec![], vec![])),
                ("setup", setup),
                ("doctor", p(vec![], vec![])),
                ("stats", stats),
                ("uninstall", uninstall),
            ],
        )),
        exclusive: &[],
    }
}

/// What the command line asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum Parsed {
    /// `--version`: print this line, exit 0.
    Version,
    /// The namespace argparse builds (dest -> value).
    Args(BTreeMap<String, Value>),
    /// argparse refused it: exit 2 with this message.
    Error(String),
}

fn dest(flag: &str) -> String {
    flag.trim_start_matches('-').replace('-', "_")
}

/// Parses `argv` (without the program name) as `ferb`'s parser does.
pub fn parse(argv: &[String]) -> Parsed {
    if argv
        .first()
        .is_some_and(|a| a == "--version" || (a.len() >= 3 && "--version".starts_with(a.as_str())))
    {
        return Parsed::Version;
    }
    let mut ns = BTreeMap::new();
    match parse_with(&parser(), argv, &mut ns) {
        Ok(()) => Parsed::Args(ns),
        Err(e) => Parsed::Error(e),
    }
}

fn parse_with(
    parser: &Parser,
    argv: &[String],
    ns: &mut BTreeMap<String, Value>,
) -> Result<(), String> {
    for o in &parser.opts {
        let d = dest(o.flag);
        let v = match (o.kind, o.default) {
            (Kind::Flag, _) => json!(false),
            (Kind::Value { int: true }, Some(d)) => json!(d.parse::<i64>().unwrap_or(0)),
            (_, Some(d)) => json!(d),
            (_, None) => Value::Null,
        };
        ns.insert(d, v);
    }
    for ps in &parser.pos {
        if ps.optional {
            ns.insert(
                ps.dest.into(),
                ps.default.map(|d| json!(d)).unwrap_or(Value::Null),
            );
        }
    }
    let mut positionals: Vec<&String> = Vec::new();
    let mut seen_exclusive: Vec<String> = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        let a = &argv[i];
        if a == "--" {
            positionals.extend(argv[i + 1..].iter());
            break;
        }
        if a.starts_with('-')
            && a.len() > 1
            && !a.chars().nth(1).is_some_and(|c| c.is_ascii_digit())
        {
            let (name, inline) = match a.split_once('=') {
                Some((n, v)) if n.starts_with("--") => (n.to_string(), Some(v.to_string())),
                _ => (a.clone(), None),
            };
            let exact = parser.opts.iter().find(|o| o.flag == name);
            let o = match exact {
                Some(o) => *o,
                None => {
                    let matches: Vec<&Opt> = if name.starts_with("--") {
                        parser
                            .opts
                            .iter()
                            .filter(|o| o.flag.starts_with(name.as_str()))
                            .collect()
                    } else {
                        Vec::new()
                    };
                    match matches.len() {
                        1 => *matches[0],
                        0 => {
                            return Err(format!("unrecognized arguments: {}", argv[i..].join(" ")))
                        }
                        _ => return Err(format!("ambiguous option: {name} could match ...")),
                    }
                }
            };
            let d = dest(o.flag);
            if parser.exclusive.contains(&d.as_str()) {
                if seen_exclusive.iter().any(|x| *x != d) {
                    return Err(format!("argument {}: not allowed with argument", o.flag));
                }
                seen_exclusive.push(d.clone());
            }
            match o.kind {
                Kind::Flag => {
                    if inline.is_some() {
                        return Err(format!("argument {}: ignored explicit argument", o.flag));
                    }
                    ns.insert(d, json!(true));
                }
                Kind::Value { int } => {
                    let v = match inline {
                        Some(v) => v,
                        None => {
                            i += 1;
                            match argv.get(i) {
                                Some(v) if !(v.starts_with('-') && v.len() > 1) => v.clone(),
                                _ => {
                                    return Err(format!(
                                        "argument {}: expected one argument",
                                        o.flag
                                    ))
                                }
                            }
                        }
                    };
                    if !o.choices.is_empty() && !o.choices.contains(&v.as_str()) {
                        return Err(format!("argument {}: invalid choice: '{v}'", o.flag));
                    }
                    let value = if int {
                        match crate::py::strip(&v).parse::<i64>() {
                            Ok(n) => json!(n),
                            Err(_) => {
                                return Err(format!(
                                    "argument {}: invalid int value: '{v}'",
                                    o.flag
                                ))
                            }
                        }
                    } else {
                        json!(v)
                    };
                    ns.insert(d, value);
                }
            }
            i += 1;
            continue;
        }
        positionals.push(a);
        i += 1;
        // a subcommand takes everything after it
        if let Some((sub_dest, _, commands)) = &parser.sub {
            if positionals.len() > parser.pos.len() {
                let name = positionals.pop().expect("just pushed");
                let Some((_, sub)) = commands.iter().find(|(n, _)| n == name) else {
                    let names: Vec<String> =
                        commands.iter().map(|(n, _)| format!("'{n}'")).collect();
                    return Err(format!(
                        "argument {sub_dest}: invalid choice: '{name}' (choose from {})",
                        names.join(", ")
                    ));
                };
                ns.insert(sub_dest.to_string(), json!(name));
                assign(parser, &positionals, ns)?;
                return parse_with(sub, &argv[i..], ns);
            }
        }
    }
    if let Some((sub_dest, required, _)) = &parser.sub {
        if *required {
            return Err(format!("the following arguments are required: {sub_dest}"));
        }
        ns.insert(sub_dest.to_string(), Value::Null);
    }
    assign(parser, &positionals, ns)
}

fn assign(
    parser: &Parser,
    positionals: &[&String],
    ns: &mut BTreeMap<String, Value>,
) -> Result<(), String> {
    let required = parser.pos.iter().filter(|p| !p.optional).count();
    if positionals.len() < required {
        let missing: Vec<&str> = parser
            .pos
            .iter()
            .filter(|p| !p.optional)
            .skip(positionals.len())
            .map(|p| p.dest)
            .collect();
        return Err(format!(
            "the following arguments are required: {}",
            missing.join(", ")
        ));
    }
    if positionals.len() > parser.pos.len() {
        let extra: Vec<String> = positionals[parser.pos.len()..]
            .iter()
            .map(|s| s.to_string())
            .collect();
        return Err(format!("unrecognized arguments: {}", extra.join(" ")));
    }
    // required positionals take their values first; an optional one gets what is left (argparse's matching for
    // these parsers, which each have at most one optional positional, after the required ones)
    let spare = positionals.len() - required;
    let mut it = positionals.iter();
    let mut spare_left = spare;
    for ps in &parser.pos {
        if ps.optional && spare_left == 0 {
            continue;
        }
        let Some(v) = it.next() else { break };
        if ps.optional {
            spare_left -= 1;
        }
        if !ps.choices.is_empty() && !ps.choices.contains(&v.as_str()) {
            return Err(format!("argument {}: invalid choice: '{v}'", ps.dest));
        }
        ns.insert(ps.dest.into(), json!(v));
    }
    Ok(())
}
