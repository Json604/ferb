//! `ferb -h` / `--help` (T-1128): argparse's help for the program or the command named before the flag, printed and
//! exit 0, as the Python build's `build_parser` did. The texts are Python's, recorded with its argparse at 80 columns
//! (engine/fixtures/device/help.json, `make_help.py`); a terminal of another width got argparse's wrapping for that
//! width there, here always 80.
//!
//! Named differences ([`DIFFERENCES`]): `setup`'s words named the Python build's interpreter and venv, which the Rust
//! build has none of (ENG-2); they say what a checkout's setup does now. `uninstall` has the Rust build's
//! `--skip-browser-files` (DMN-598), and `stats` its `--from`/`--to` (ORCH-720), laid out as argparse lays them out.

use std::collections::BTreeMap;

use serde_json::Value;

/// What Python's argparse printed for each parser: "" the program, else its command path ("memory add").
const RECORDED: &str = include_str!("../../../../fixtures/device/help.json");

/// (command path, Python's text, this build's text): the only changes to the recorded help.
pub const DIFFERENCES: [(&str, &str, &str); 4] = [
    (
        "",
        "    setup               a checkout: install FERB's interpreter, venv and\n                        development LaunchAgent; FERB.app: its setup steps\n",
        "    setup               a checkout: install the development LaunchAgent for\n                        its built daemon; FERB.app: its setup steps\n",
    ),
    (
        "setup",
        "  --update         only reinstall FERB into a new venv and restart the daemon\n",
        "  --update         only reinstall FERB and restart the daemon\n",
    ),
    (
        "uninstall",
        "usage: ferb uninstall [-h] [--purge] [--yes]\n\noptions:\n  -h, --help  show this help message and exit\n  --purge\n  --yes       with --purge: the owner already confirmed in FERB.app (Settings\n              \u{2192} Advanced), so no question\n",
        "usage: ferb uninstall [-h] [--purge] [--yes] [--skip-browser-files]\n\noptions:\n  -h, --help            show this help message and exit\n  --purge\n  --yes                 with --purge: the owner already confirmed in FERB.app\n                        (Settings \u{2192} Advanced), so no question\n  --skip-browser-files  FERB.app's: FERB's background service already removed\n                        FERB's files in the browsers' folders\n",
    ),
    (
        "stats",
        "usage: ferb stats [-h] [--days DAYS]\n\noptions:\n  -h, --help   show this help message and exit\n  --days DAYS\n",
        "usage: ferb stats [-h] [--days DAYS] [--from FROM] [--to TO]\n\noptions:\n  -h, --help   show this help message and exit\n  --days DAYS\n  --from FROM  the model-use section's first local day (YYYY-MM-DD)\n  --to TO      its last local day (YYYY-MM-DD)\n",
    ),
];

/// Python's recorded texts, by command path.
pub fn recorded() -> BTreeMap<String, String> {
    let v: Value = serde_json::from_str(RECORDED).expect("help.json");
    v.as_object()
        .expect("an object")
        .iter()
        .map(|(k, t)| (k.clone(), t.as_str().unwrap_or_default().to_string()))
        .collect()
}

/// The help this build prints for a command path.
pub fn text(path: &str) -> Option<String> {
    let mut t = recorded().remove(path)?;
    for (p, python, rust) in DIFFERENCES {
        if p == path {
            t = t.replace(python, rust);
        }
    }
    Some(t)
}

/// argparse's `-h`: the first `-h` or `--help` (or an unambiguous abbreviation, `--h`, `--he`, `--hel`) before `--`,
/// with the commands named before it: the path whose help to print. None: no help asked.
pub fn asked(argv: &[String]) -> Option<String> {
    let recorded = recorded();
    let mut path: Vec<&str> = Vec::new();
    for a in argv {
        if a == "--" {
            return None;
        }
        if a == "-h" || (a.len() >= 3 && "--help".starts_with(a.as_str())) {
            return Some(path.join(" "));
        }
        let mut next = path.clone();
        next.push(a);
        // a command of the parser reached so far (once per level: argparse takes the first positional there)
        let deeper = next.join(" ");
        if recorded.contains_key(&deeper) && !a.starts_with('-') {
            path = next;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    // decision: DMN-601
    #[test]
    fn the_help_is_the_one_of_the_command_before_the_flag() {
        assert_eq!(asked(&args(&["--help"])).as_deref(), Some(""));
        assert_eq!(asked(&args(&["-h"])).as_deref(), Some(""));
        assert_eq!(asked(&args(&["--he"])).as_deref(), Some(""));
        assert_eq!(
            asked(&args(&["memory", "add", "-h"])).as_deref(),
            Some("memory add")
        );
        assert_eq!(
            asked(&args(&["trace", "abc", "--help"])).as_deref(),
            Some("trace")
        );
        assert_eq!(asked(&args(&["-h", "memory"])).as_deref(), Some(""));
        assert_eq!(asked(&args(&["logs", "--", "-h"])), None);
        assert_eq!(asked(&args(&["logs", "--tasks"])), None);
        assert_eq!(asked(&args(&["--h"])).as_deref(), Some(""));
    }

    /// Every text is Python's, but for the named differences, each of which applies.
    // decision: DMN-601
    #[test]
    fn the_texts_are_pythons_but_the_named_differences() {
        let recorded = recorded();
        assert_eq!(recorded.len(), 33);
        for (path, python) in &recorded {
            let ours = text(path).unwrap();
            let named: Vec<_> = DIFFERENCES.iter().filter(|d| d.0 == path).collect();
            if named.is_empty() {
                assert_eq!(&ours, python, "{path}");
            }
            for (_, from, to) in named {
                assert!(
                    python.contains(from),
                    "{path}: the recorded text has {from:?}"
                );
                assert!(ours.contains(to) && !ours.contains(from), "{path}");
            }
        }
        assert!(text("")
            .unwrap()
            .starts_with("usage: ferb [-h] [--version]\n"));
        assert!(!text("").unwrap().contains("venv"));
        assert!(!text("setup").unwrap().contains("venv"));
    }
}
