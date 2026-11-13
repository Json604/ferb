//! A role's own model row in config.toml (`[role_models] <role>` = {runtime, model, effort}), written as Python's
//! `config.set("role_models", role, row)` / `config.remove("role_models", role)` write it through tomlkit: a dict
//! becomes its own sub-table `[role_models.<role>]`. ferb-core's config writer doesn't write a table as a value yet,
//! so this module edits the text the way tomlkit does (checked against rust/fixtures/daemon-b `model_choice` and
//! probed against tomlkit for the cases below) and hands the result to ferb-core's parser and atomic save:
//!
//! - a new row goes after the last of the section's tables (`[role_models]`, `[role_models.*]`), or at the end of
//!   the file when there is none, after a blank line unless the text there already ends with one;
//! - a row that exists as its own sub-table keeps its place: its key lines are replaced, its trailing blank lines
//!   kept;
//! - removing a sub-table removes it from its header to the next header (a blank line before it stays);
//! - a row written inline inside a `[role_models]` table (an older config) is removed by ferb-core's own remove;
//!   replacing one is refused as not ported.

use ferb_core::config::{Config, Document};
use serde_json::{Map, Value};

/// One header line: its byte range (line start .. line end, newline included) and the table's dotted name.
struct Header {
    start: usize,
    name: String,
}

/// The table headers of a TOML text, in order: lines that open with `[` outside any value, string or comment.
fn headers(text: &str) -> Vec<Header> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let (mut depth, mut i) = (0i32, 0usize);
    let mut line_start = true;
    while i < bytes.len() {
        let c = bytes[i];
        if line_start && depth == 0 {
            let mut j = i;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'[' {
                let end = text[j..].find('\n').map_or(text.len(), |e| j + e);
                let line = text[j..end].trim();
                let inner = line.trim_start_matches('[');
                let close = inner.find(']').unwrap_or(inner.len());
                let name: String = inner[..close]
                    .split('.')
                    .map(|p| p.trim().trim_matches('"'))
                    .collect::<Vec<_>>()
                    .join(".");
                out.push(Header { start: i, name });
                i = (end + 1).min(bytes.len());
                line_start = true;
                continue;
            }
        }
        line_start = false;
        match c {
            b'\n' => line_start = true,
            b'#' => {
                i = text[i..].find('\n').map_or(text.len(), |e| i + e);
                continue;
            }
            b'"' | b'\'' => {
                let triple = text[i..].starts_with(if c == b'"' { "\"\"\"" } else { "'''" });
                let quote = if triple {
                    &text[i..i + 3]
                } else {
                    &text[i..i + 1]
                };
                let mut j = i + quote.len();
                while j < bytes.len() {
                    if c == b'"' && bytes[j] == b'\\' {
                        j += 2;
                        continue;
                    }
                    if text[j..].starts_with(quote) {
                        break;
                    }
                    if !triple && bytes[j] == b'\n' {
                        break;
                    }
                    j += 1;
                }
                i = (j + quote.len()).min(bytes.len());
                continue;
            }
            b'[' | b'{' => depth += 1,
            b']' | b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    out
}

/// A TOML basic string as tomlkit writes a new one.
fn basic(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn key(k: &str) -> String {
    if !k.is_empty()
        && k.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        k.to_string()
    } else {
        basic(k)
    }
}

fn body(row: &Map<String, Value>) -> Result<String, String> {
    let mut out = String::new();
    for (k, v) in row {
        let Value::String(s) = v else {
            return Err("config.toml: only text values are written in a role's row".into());
        };
        out.push_str(&format!("{} = {}\n", key(k), basic(s)));
    }
    Ok(out)
}

/// Where table `i`'s span ends: the next header's start, or the end of the text.
fn span_end(all: &[Header], i: usize, text: &str) -> usize {
    all.get(i + 1).map_or(text.len(), |h| h.start)
}

fn save(config: &Config, text: String) -> Result<(), String> {
    let doc = Document::parse(&text).map_err(|e| e.to_string())?;
    config.save(&doc).map_err(|e| e.to_string())
}

/// `config.set(section, row_key, row)` for a dict.
pub fn set_row(
    config: &Config,
    section: &str,
    row_key: &str,
    row: &Map<String, Value>,
) -> Result<(), String> {
    let text = config.load().map_err(|e| e.to_string())?.text().to_string();
    let lines = body(row)?;
    let all = headers(&text);
    let own = format!("{section}.{row_key}");
    if let Some(i) = all.iter().position(|h| h.name == own) {
        // in place: the header kept, its key lines replaced, its trailing blank lines kept
        let header_end = text[all[i].start..]
            .find('\n')
            .map_or(text.len(), |e| all[i].start + e + 1);
        let end = span_end(&all, i, &text);
        let span = &text[header_end..end];
        let kept = span.len() - span.trim_end_matches(['\n', ' ', '\t']).len();
        let trailing = &span[span.len() - kept..];
        let trailing = trailing.strip_prefix('\n').unwrap_or(trailing);
        let new = format!("{}{lines}{trailing}{}", &text[..header_end], &text[end..]);
        return save(config, new);
    }
    if config
        .get(section, row_key)
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err(format!(
            "config.toml: replacing {section}.{row_key} written inline is not ported"
        ));
    }
    let header = format!("[{}.{}]\n", key(section), key(row_key));
    let prefix = format!("{section}.");
    let last = all
        .iter()
        .enumerate()
        .filter(|(_, h)| h.name == section || h.name.starts_with(&prefix))
        .map(|(i, _)| i)
        .next_back();
    let new = match last {
        Some(i) => {
            let at = span_end(&all, i, &text);
            let gap = if text[..at].ends_with("\n\n") {
                ""
            } else {
                "\n"
            };
            format!("{}{gap}{header}{lines}{}", &text[..at], &text[at..])
        }
        None => {
            // as a new table at the end (ferb-core's gap for a new section)
            let gap = if text.is_empty() {
                ""
            } else if !text.ends_with('\n') {
                "\n"
            } else {
                let last_line = &text[..text.len() - 1];
                if last_line.is_empty() || last_line.ends_with('\n') || text.trim().is_empty() {
                    ""
                } else {
                    "\n"
                }
            };
            format!("{text}{gap}{header}{lines}")
        }
    };
    save(config, new)
}

/// `config.remove(section, row_key)` for a row: its own sub-table goes from its header to the next header; a row
/// written inline goes through ferb-core's remove.
pub fn remove_row(config: &Config, section: &str, row_key: &str) -> Result<(), String> {
    let text = config.load().map_err(|e| e.to_string())?.text().to_string();
    let all = headers(&text);
    let own = format!("{section}.{row_key}");
    if let Some(i) = all.iter().position(|h| h.name == own) {
        let new = format!(
            "{}{}",
            &text[..all[i].start],
            &text[span_end(&all, i, &text)..]
        );
        return save(config, new);
    }
    config
        .remove(section, &[row_key])
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    /// The texts tomlkit 0.13 wrote for these steps, each after a load and save (probed 2026-11-13).
    #[test]
    fn rows_are_written_as_tomlkit_writes_them() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("home");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.join("config.toml");
        let write = |t: &str| {
            std::fs::write(&path, t).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        };
        let cfg = Config::new(&dir);
        let read = || std::fs::read_to_string(&path).unwrap();
        write("[a]\nx = 1\n");
        set_row(
            &cfg,
            "role_models",
            "worker",
            &row(json!({"runtime": "c", "model": ""})),
        )
        .unwrap();
        assert_eq!(
            read(),
            "[a]\nx = 1\n\n[role_models.worker]\nruntime = \"c\"\nmodel = \"\"\n"
        );
        remove_row(&cfg, "role_models", "worker").unwrap();
        assert_eq!(read(), "[a]\nx = 1\n\n");
        set_row(
            &cfg,
            "role_models",
            "orchestrator",
            &row(json!({"runtime": "c", "model": "m"})),
        )
        .unwrap();
        assert_eq!(
            read(),
            "[a]\nx = 1\n\n[role_models.orchestrator]\nruntime = \"c\"\nmodel = \"m\"\n"
        );
        set_row(
            &cfg,
            "role_models",
            "worker",
            &row(json!({"runtime": "c", "model": "w"})),
        )
        .unwrap();
        assert_eq!(
            read(),
            "[a]\nx = 1\n\n[role_models.orchestrator]\nruntime = \"c\"\nmodel = \"m\"\n\n[role_models.worker]\nruntime = \"c\"\nmodel = \"w\"\n"
        );
        set_row(
            &cfg,
            "role_models",
            "orchestrator",
            &row(json!({"runtime": "d", "model": "m2", "effort": "e"})),
        )
        .unwrap();
        assert_eq!(
            read(),
            "[a]\nx = 1\n\n[role_models.orchestrator]\nruntime = \"d\"\nmodel = \"m2\"\neffort = \"e\"\n\n[role_models.worker]\nruntime = \"c\"\nmodel = \"w\"\n"
        );
        remove_row(&cfg, "role_models", "orchestrator").unwrap();
        assert_eq!(
            read(),
            "[a]\nx = 1\n\n[role_models.worker]\nruntime = \"c\"\nmodel = \"w\"\n"
        );
        write("[a]\nx = 1\n\n[role_models.worker]\nruntime = \"c\"\n\n[b]\ny = 2\n");
        set_row(
            &cfg,
            "role_models",
            "orchestrator",
            &row(json!({"runtime": "c", "model": "m"})),
        )
        .unwrap();
        assert_eq!(
            read(),
            "[a]\nx = 1\n\n[role_models.worker]\nruntime = \"c\"\n\n[role_models.orchestrator]\nruntime = \"c\"\nmodel = \"m\"\n[b]\ny = 2\n"
        );
        remove_row(&cfg, "role_models", "worker").unwrap();
        assert_eq!(
            read(),
            "[a]\nx = 1\n\n[role_models.orchestrator]\nruntime = \"c\"\nmodel = \"m\"\n[b]\ny = 2\n"
        );
        write("[a]\nx = 1\n\n[role_models]\nworker = {runtime = \"c\"}\n");
        set_row(
            &cfg,
            "role_models",
            "orchestrator",
            &row(json!({"runtime": "c", "model": "m"})),
        )
        .unwrap();
        assert_eq!(
            read(),
            "[a]\nx = 1\n\n[role_models]\nworker = {runtime = \"c\"}\n\n[role_models.orchestrator]\nruntime = \"c\"\nmodel = \"m\"\n"
        );
    }
}
