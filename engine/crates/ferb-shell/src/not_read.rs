//! What a command could not read, as facts for the worker (owner, 2026-11-16: "ferb should be able to say this that
//! it can't read this and why"; HAND-595, HAND-596). The sandbox denies every never-read folder to every command,
//! so a command that walks a tree (du, find, ls -R, a script) silently misses what is inside one. After a command,
//! its result names the never-read folders under what it could reach: its working folder and each path its words
//! name, taken by the shell's word rules alone, plus each one a refusal line in its output names. Code reads the
//! command's words only as paths (rule 4: no meaning, no intent); a folder may be listed that the command never
//! entered, which the fact says.

use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

use crate::protection::{Entry, Protection};

/// At most this many folders are listed; `more` counts the rest.
pub const CAP: usize = 12;
/// What a refusal in a command's output reads like (EPERM's and EACCES's own texts).
const DENIED: [&str; 2] = ["Operation not permitted", "Permission denied"];

pub const FACT: &str = "FERB's commands can't read these protected folders (the owner's never-read list), so \
whatever this command would have read inside them is missing from its output; \"Operation not permitted\" there is \
this protection, not a missing macOS permission. Listed: the protected folders under what this command could reach \
(its working folder and the paths it names; it may not have entered each) and any its output names in a refusal.";
pub const ALLOW: &str = "Only the owner can open one: run the command again with escape set to that folder's rule \
(FERB asks the owner once per folder for this task; a Yes lets this task read it), or report it as not read and \
why. fs.disk_usage counts a protected folder's size without reading what is in it, with no question.";

/// Why a never-read folder is protected, as the policy part words it (which of its lists names the folder).
pub fn why(shown: &str) -> String {
    #[cfg(feature = "policy")]
    {
        ferb_policy::protection::never_read_why(shown).to_string()
    }
    #[cfg(not(feature = "policy"))]
    {
        let _ = shown;
        "a folder on the owner's never-read list".to_string()
    }
}

/// The shell's words of a command: split on blanks and the control operators outside quotes, quotes removed,
/// backslash escapes kept as the character. Nothing is expanded or run.
pub fn words(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut chars = command.chars();
    let (mut single, mut double) = (false, false);
    while let Some(c) = chars.next() {
        if single {
            if c == '\'' {
                single = false;
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
                started = true;
            }
            '"' => {
                double = !double;
                started = true;
            }
            '\'' if !double => {
                single = true;
                started = true;
            }
            c if !double && (c.is_whitespace() || ";|&()<>`".contains(c)) => {
                if started || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                started = false;
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `..` and `.` folded without looking at the disk.
fn fold(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// A word as the folder a command could reach through it: `~`, `$HOME` and `${HOME}` as the home, relative to the
/// working folder, cut before the first part a shell would still expand (a pattern or a variable). None for a word
/// that names no path (an option, a URL, plain text).
fn root(word: &str, cwd: &Path, home: &Path) -> Option<PathBuf> {
    let word = word.trim();
    if word.is_empty() || word.starts_with('-') || word.contains("://") {
        return None;
    }
    let pathlike = word.starts_with(['/', '~', '.', '*'])
        || word.starts_with("$HOME")
        || word.starts_with("${HOME}")
        || word.contains('/');
    if !pathlike {
        return None;
    }
    let full = if word == "~" || word == "$HOME" || word == "${HOME}" {
        home.to_path_buf()
    } else if let Some(rest) = word.strip_prefix("~/") {
        home.join(rest)
    } else if let Some(rest) = word
        .strip_prefix("$HOME/")
        .or_else(|| word.strip_prefix("${HOME}/"))
    {
        home.join(rest)
    } else if word.starts_with('~') {
        return None; // another account's home: not resolved here
    } else if word.starts_with('/') {
        PathBuf::from(word)
    } else {
        cwd.join(word)
    };
    let mut kept = PathBuf::new();
    for c in full.components() {
        let s = c.as_os_str().to_string_lossy();
        if s.contains(['*', '?', '[', '{', '$']) {
            break;
        }
        kept.push(c.as_os_str());
    }
    Some(fold(&kept))
}

/// The folders a command could reach: its working folder, then each path its words name (and `--opt=path`'s,
/// `NAME=path`'s path).
pub fn roots(command: &str, cwd: &Path, home: &Path) -> Vec<PathBuf> {
    let mut out = vec![cwd.to_path_buf()];
    for w in words(command) {
        let mut cands = vec![w.as_str()];
        if let Some((_, after)) = w.split_once('=') {
            cands.push(after);
        }
        for c in cands {
            if let Some(r) = root(c, cwd, home) {
                if !out.contains(&r) {
                    out.push(r);
                }
            }
        }
    }
    out
}

/// A path as written and as it resolves now (/tmp and /var are links on macOS).
fn both(p: &Path) -> Vec<PathBuf> {
    let mut out = vec![p.to_path_buf()];
    if let Ok(real) = std::fs::canonicalize(p) {
        if !out.contains(&real) {
            out.push(real);
        }
    }
    out
}

fn reached(e: &Entry, roots: &[PathBuf]) -> bool {
    let es = [&e.path, &e.typed];
    roots
        .iter()
        .flat_map(|r| both(r))
        .any(|r| es.iter().any(|ep| ep.starts_with(&r) || r.starts_with(ep)))
}

/// Whether `line` names `path` as a whole path (the next character ends it).
fn names(line: &str, path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(i) = line[from..].find(path) {
        let end = from + i + path.len();
        match line[end..].chars().next() {
            None => return true,
            Some(c)
                if c == '/'
                    || c == ':'
                    || c == '\''
                    || c == '"'
                    || c == ')'
                    || c.is_whitespace() =>
            {
                return true
            }
            _ => {}
        }
        from = end;
    }
    false
}

/// The rule's folder as shown (`read:~/.ssh` -> `~/.ssh`).
fn shown(e: &Entry) -> &str {
    e.id.strip_prefix("read:").unwrap_or(&e.id)
}

fn refused_in(e: &Entry, output: &str) -> bool {
    let forms = [
        e.path.to_string_lossy().into_owned(),
        e.typed.to_string_lossy().into_owned(),
        shown(e).to_string(),
    ];
    output
        .lines()
        .filter(|l| DENIED.iter().any(|d| l.contains(d)))
        .any(|l| forms.iter().any(|f| names(l, f)))
}

/// Whether every refusal line in the output names a never-read folder: then the refusals were this protection's,
/// and the folder-scope note about refused writes (HAND-203's `sandbox_note`) would misstate them.
pub fn refusals_all_protected(protection: &dyn Protection, output: &str) -> bool {
    let mut lines = output
        .lines()
        .filter(|l| DENIED.iter().any(|d| l.contains(d)))
        .peekable();
    lines.peek().is_some()
        && lines.all(|l| {
            protection.reads().iter().any(|e| {
                [
                    e.path.to_string_lossy().into_owned(),
                    e.typed.to_string_lossy().into_owned(),
                    shown(e).to_string(),
                ]
                .iter()
                .any(|f| names(l, f))
            })
        })
}

/// The `not_read` fact for a command (None when it reached no never-read folder): the folders its output names in
/// a refusal first, then those under what it could reach, in the list's order; the rule an approved escape lifted
/// for it left out.
// decision: HAND-596
pub fn fact(
    protection: &dyn Protection,
    command: &str,
    cwd: &Path,
    lifted: Option<&str>,
    output: &str,
) -> Option<Value> {
    let reads: Vec<&Entry> = protection
        .reads()
        .iter()
        .filter(|e| Some(e.id.as_str()) != lifted)
        .collect();
    let roots = roots(command, cwd, protection.home());
    let mut hit: Vec<&Entry> = reads
        .iter()
        .copied()
        .filter(|e| refused_in(e, output))
        .collect();
    for e in &reads {
        // a listed folder that isn't on this Mac hid nothing; another app's container is never looked at, so counts
        let here = protection.app_data(&e.typed)
            || [&e.path, &e.typed]
                .iter()
                .any(|p| std::fs::symlink_metadata(p).is_ok());
        if here && !hit.iter().any(|h| h.id == e.id) && reached(e, &roots) {
            hit.push(e);
        }
    }
    folders(hit.iter().map(|e| e.id.as_str()), FACT, ALLOW)
}

/// The fact's shape, shared by every tool that reports it: the fact, how to get one allowed, at most [`CAP`] folders
/// (path, rule, why) and how many more.
pub fn folders<'a>(rules: impl Iterator<Item = &'a str>, fact: &str, allow: &str) -> Option<Value> {
    let mut seen: Vec<&str> = Vec::new();
    for r in rules {
        if !seen.contains(&r) {
            seen.push(r);
        }
    }
    if seen.is_empty() {
        return None;
    }
    let listed: Vec<Value> = seen
        .iter()
        .take(CAP)
        .map(|r| {
            let path = r.strip_prefix("read:").unwrap_or(r);
            json!({"path": path, "rule": r, "why": why(path)})
        })
        .collect();
    let mut out = json!({"fact": fact, "allow": allow, "folders": listed});
    if seen.len() > CAP {
        out["more"] = json!(seen.len() - CAP);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_follow_the_shells_quoting() {
        assert_eq!(
            words(
                r#"du -sh "$HOME/Library/Application Support"/* 2>&1 | sort -h; ls ~/a\ b 'c d'"#
            ),
            vec![
                "du",
                "-sh",
                "$HOME/Library/Application Support/*",
                "2",
                "1",
                "sort",
                "-h",
                "ls",
                "~/a b",
                "c d"
            ]
        );
    }

    #[test]
    fn roots_are_the_working_folder_and_the_named_paths() {
        let (cwd, home) = (Path::new("/w"), Path::new("/Users/o"));
        assert_eq!(
            roots(
                "du -sh ~/Library/*/x ../up --dir=/opt/a/../b plain https://x.example/a ~other/z",
                cwd,
                home
            ),
            vec![
                PathBuf::from("/w"),
                PathBuf::from("/Users/o/Library"),
                PathBuf::from("/up"),
                PathBuf::from("/opt/b"),
            ]
        );
    }

    #[test]
    fn a_path_is_named_only_whole() {
        assert!(names(
            "du: /h/Library/Mail: Operation not permitted",
            "/h/Library/Mail"
        ));
        assert!(!names(
            "du: /h/Library/Mailboxes: Operation not permitted",
            "/h/Library/Mail"
        ));
        assert!(names(
            "PermissionError: [Errno 1] Operation not permitted: '/h/.ssh'",
            "/h/.ssh"
        ));
    }
}
