//! The wiring guard: every seam this program fills for an engine crate is a real implementation, or it goes through
//! `ferbd::off::off("id", value)` and `seams-off.txt` says why it is off. FERB's first real setup failed on seams the
//! composition had left as do-nothing values (`|| false`, `None`, `NoBanner`, `NoListeners`) that no recorded scenario
//! exercised; this test makes such a value a reviewed line in one file instead of a silent default.
//!
//! What it checks, reading the composition's own source (src/compose, src/areas_c, src/setup_env.rs,
//! src/device.rs; each file up to its `#[cfg(test)]` module):
//!
//! 1. No placeholder-shaped value outside an `off(...)` call (on the same line or within the three lines before it):
//!    a closure answering `false`, `None`, `{}`, an empty string or list; `Arc::new(String::new)` and the like; a
//!    `No…` stand-in; `..Default::default()` / `..Hooks::default()`; a struct field set to `None` (unless the file lists
//!    that field name as a `value`: a plain data field, never a seam).
//! 2. Every `off("id", …)` names an `off id | why` line of `seams-off.txt`, and every such line is used.
//! 3. For each `trait` line of the file, every method with a default body in that trait is either overridden by the
//!    named implementation or listed as `default Trait::method | why`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The composition's files: every place this program fills another crate's seams.
fn composition_files() -> Vec<PathBuf> {
    let src = crate_dir().join("src");
    let mut out = Vec::new();
    for dir in ["compose", "compose/tools", "areas_c"] {
        let mut files: Vec<PathBuf> = std::fs::read_dir(src.join(dir))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "rs"))
            .collect();
        files.sort();
        out.extend(files);
    }
    out.push(src.join("setup_env.rs"));
    out.push(src.join("device.rs"));
    out
}

/// A file's lines up to its test module.
fn code_lines(path: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap();
    text.lines()
        .take_while(|l| !l.trim_start().starts_with("#[cfg(test)]"))
        .map(str::to_string)
        .collect()
}

struct AllowList {
    off: BTreeMap<String, String>,
    values: BTreeSet<String>,
    defaults: BTreeMap<String, String>,
    /// (trait file, trait name, impl file, impl type)
    traits: Vec<(String, String, String, String)>,
}

fn allow_list() -> AllowList {
    let text = std::fs::read_to_string(crate_dir().join("seams-off.txt")).unwrap();
    let mut a = AllowList {
        off: BTreeMap::new(),
        values: BTreeSet::new(),
        defaults: BTreeMap::new(),
        traits: Vec::new(),
    };
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (head, why) = match line.split_once(" | ") {
            Some((h, w)) => (h.trim(), w.trim()),
            None => (line, ""),
        };
        let mut words = head.split_whitespace();
        let kind = words.next().unwrap_or("");
        let rest: Vec<&str> = words.collect();
        match kind {
            "off" | "default" => {
                assert!(
                    rest.len() == 1 && why.len() >= 20,
                    "seams-off.txt:{}: `{kind} <id> | <why>` needs an id and a reason of a sentence: {raw}",
                    n + 1
                );
                let map = if kind == "off" {
                    &mut a.off
                } else {
                    &mut a.defaults
                };
                assert!(
                    map.insert(rest[0].to_string(), why.to_string()).is_none(),
                    "seams-off.txt:{}: {} listed twice",
                    n + 1,
                    rest[0]
                );
            }
            "value" => {
                assert!(
                    rest.len() == 1 && why.len() >= 10,
                    "seams-off.txt:{}: `value <field> | <why>`: {raw}",
                    n + 1
                );
                a.values.insert(rest[0].to_string());
            }
            "trait" => {
                assert_eq!(
                    rest.len(),
                    4,
                    "seams-off.txt:{}: `trait <trait file> <Trait> <impl file> <Type>`: {raw}",
                    n + 1
                );
                a.traits.push((
                    rest[0].into(),
                    rest[1].into(),
                    rest[2].into(),
                    rest[3].into(),
                ));
            }
            other => panic!("seams-off.txt:{}: unknown line kind {other:?}", n + 1),
        }
    }
    a
}

/// The placeholder shapes, as plain text.
const SHAPES: [&str; 15] = [
    "|| false",
    "| {}",
    "|| {}",
    "| String::new()",
    "| Vec::new()",
    "Arc::new(String::new)",
    "Arc::new(Vec::new)",
    "Box::new(String::new)",
    "Box::new(Vec::new)",
    "Arc::new(No",
    "Box::new(No",
    "..Default::default()",
    "..Hooks::default()",
    "Box::pin(async {})",
    "Box::pin(async { None })",
];

/// A closure that answers `None` or `false`: `|…| None`, `|…| false`.
fn closure_none(line: &str) -> bool {
    ["| None", "| false"].iter().any(|shape| {
        line.find(shape)
            .is_some_and(|at| line[..at].contains('|') && !line[..at].ends_with('|'))
    })
}

/// A struct field set to `None` (`name: None,`), with its name.
fn none_field(line: &str) -> Option<String> {
    let t = line.trim();
    let name = t.strip_suffix(": None,")?;
    (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_'))
        .then(|| name.to_string())
}

fn excused(lines: &[String], i: usize) -> bool {
    let from = i.saturating_sub(3);
    lines[from..=i]
        .iter()
        .any(|l| l.contains("off(\"") || l.contains("off::off("))
}

// decision: ENG-1
#[test]
fn every_seam_is_wired_or_named_off() {
    let allow = allow_list();
    let mut found = Vec::new();
    let mut used_off: BTreeSet<String> = BTreeSet::new();
    for path in composition_files() {
        let lines = code_lines(&path);
        let rel = path
            .strip_prefix(crate_dir())
            .unwrap()
            .display()
            .to_string();
        // the off ids this file uses (`off("id", …)`, the id on the call's line or the next)
        let joined = lines.join("\n");
        let mut rest = joined.as_str();
        while let Some(at) = rest.find("off(") {
            let after = rest[at + 4..].trim_start();
            if let Some(quoted) = after.strip_prefix('"') {
                if let Some(end) = quoted.find('"') {
                    used_off.insert(quoted[..end].to_string());
                }
            }
            rest = &rest[at + 4..];
        }
        for (i, line) in lines.iter().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if excused(&lines, i) {
                continue;
            }
            let shape = SHAPES
                .iter()
                .find(|s| code.contains(*s))
                .map(|s| s.to_string())
                .or_else(|| closure_none(code).then(|| "|…| None/false".to_string()))
                .or_else(|| {
                    none_field(code)
                        .filter(|f| !allow.values.contains(f))
                        .map(|f| format!("{f}: None"))
                });
            if let Some(shape) = shape {
                found.push(format!("{rel}:{}: `{shape}`: {}", i + 1, line.trim()));
            }
        }
    }
    assert!(
        found.is_empty(),
        "placeholder-shaped seams outside `off(...)` (wire each, or wrap it in off(\"id\", …) and say why in \
         seams-off.txt):\n{}",
        found.join("\n")
    );
    let listed: BTreeSet<String> = allow.off.keys().cloned().collect();
    let unlisted: Vec<_> = used_off.difference(&listed).collect();
    assert!(
        unlisted.is_empty(),
        "off ids with no line in seams-off.txt: {unlisted:?}"
    );
    let unused: Vec<_> = listed.difference(&used_off).collect();
    assert!(
        unused.is_empty(),
        "seams-off.txt lists off ids no code uses: {unused:?}"
    );
}

/// The block that starts at the first line containing `head` (its braces matched), as text.
fn block(text: &str, head: &str) -> Option<String> {
    let at = text.find(head)?;
    let body = &text[at..];
    let open = body.find('{')?;
    let mut depth = 0i32;
    for (i, c) in body[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(body[..open + i + 1].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// The methods of a trait block that have a default body.
fn defaults_of(trait_block: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = trait_block;
    while let Some(at) = rest.find("fn ") {
        let after = &rest[at + 3..];
        let name: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        let semi = after.find(';').unwrap_or(usize::MAX);
        let brace = after.find('{').unwrap_or(usize::MAX);
        if brace < semi && !name.is_empty() {
            out.insert(name);
        }
        rest = after;
    }
    out
}

/// The methods an impl block defines.
fn methods_of(impl_block: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = impl_block;
    while let Some(at) = rest.find("fn ") {
        let after = &rest[at + 3..];
        let name: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            out.insert(name);
        }
        rest = after;
    }
    out
}

// decision: ENG-1
#[test]
fn every_default_a_trait_gives_is_overridden_or_named() {
    let allow = allow_list();
    let crates = crate_dir().parent().unwrap().to_path_buf();
    let mut missing = Vec::new();
    let mut named: BTreeSet<String> = BTreeSet::new();
    for (trait_file, trait_name, impl_file, impl_type) in &allow.traits {
        let t = std::fs::read_to_string(crates.join(trait_file))
            .unwrap_or_else(|e| panic!("{trait_file}: {e}"));
        let tb = block(&t, &format!("pub trait {trait_name}"))
            .unwrap_or_else(|| panic!("no `pub trait {trait_name}` in {trait_file}"));
        let i = std::fs::read_to_string(crates.join(impl_file))
            .unwrap_or_else(|e| panic!("{impl_file}: {e}"));
        let ib = block(&i, &format!("{trait_name} for {impl_type} {{"))
            .unwrap_or_else(|| panic!("no `impl … {trait_name} for {impl_type}` in {impl_file}"));
        let overridden = methods_of(&ib[ib.find('{').unwrap()..]);
        for m in defaults_of(&tb[tb.find('{').unwrap()..]) {
            let key = format!("{trait_name}::{m}");
            named.insert(key.clone());
            if !overridden.contains(&m) && !allow.defaults.contains_key(&key) {
                missing.push(format!("{key} ({impl_type} in {impl_file})"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "trait defaults left in place without a reason (override each, or list `default Trait::method | why` in \
         seams-off.txt):\n{}",
        missing.join("\n")
    );
    let stale: Vec<_> = allow
        .defaults
        .keys()
        .filter(|k| !named.contains(*k))
        .collect();
    assert!(
        stale.is_empty(),
        "seams-off.txt names defaults no listed trait has: {stale:?}"
    );
}

/// The guard's own parts: what counts as a placeholder.
#[test]
fn the_guard_sees_the_shapes_it_names() {
    assert!(closure_none("    browser_pid: Arc::new(|_fd| None),"));
    assert!(!closure_none(
        "    let x = y.map(|v| v.len()).unwrap_or(None);"
    ));
    assert!(closure_none("    test_task: Box::new(|_| false),"));
    assert!(!closure_none("    a || false"));
    assert_eq!(none_field("        tabs: None,"), Some("tabs".to_string()));
    assert_eq!(none_field("        let x: Option<u8> = None;"), None);
    assert!(SHAPES
        .iter()
        .any(|s| "sensing_paused: Arc::new(|| false),".contains(s)));
    let lines = vec![
        "    let a = 1;".to_string(),
        "    click_through: crate::off::off(\"chat.click_through\",".to_string(),
        "        None),".to_string(),
    ];
    assert!(excused(&lines, 2));
    assert!(!excused(&lines[..1], 0));
    let t = "pub trait T {\n    fn a(&self);\n    fn b(&self) -> bool {\n        false\n    }\n}\n";
    assert_eq!(defaults_of(t), BTreeSet::from(["b".to_string()]));
}
