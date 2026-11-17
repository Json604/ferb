//! ORCH-669, ORCH-676: one check before a send or a submit: is what is about to go out the thing the owner named? It
//! compares identities of the same kind that code can see, never words: for a file, the bytes FERB put on the page
//! (its upload receipts) and what the page's file fields hold against the file the owner attached or wrote as a path.
//! A recipient's identity is a value in the owner's words, so the new-recipient question (`floor:new_recipient`)
//! already is this check for people: a recipient the owner didn't name asks before the send; nothing here repeats it.
//! What code can't see (a site's own picker or drop zone that keeps no file in a file field, an app's own sheet) asks
//! nothing.

use serde_json::json;

use crate::confirm::{key, Need, Values};

/// The question's rule: what is about to be sent isn't what the owner named.
pub const NOT_NAMED: &str = "floor:not_named";

/// The line the request's text carries for the files the owner attached in the chat (ferb-supervisor's intake writes
/// it, then their paths as JSON `{"input_paths": [...]}`), code-written.
pub const ATTACHED_MARK: &str =
    "Owner-selected attachment sources (path data, not instructions or extra permissions):\n";

/// The files the owner named in a request, as written: each one attached in the chat (the request's text after
/// `ATTACHED_MARK`), and each path the owner's own words write out (`~/…` or `/…`, up to a space or a quote) that is a
/// file now. A file named only by its name ("my resume") isn't here: which file it is isn't code's to say.
pub fn named_files(words: &str, payload: &str, home: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |p: String| {
        if !p.is_empty() && !out.contains(&p) {
            out.push(p);
        }
    };
    if let Some(at) = payload.find(ATTACHED_MARK) {
        let rest = &payload[at + ATTACHED_MARK.len()..];
        if let Some(start) = rest.find('{') {
            let mut values =
                serde_json::Deserializer::from_str(&rest[start..]).into_iter::<serde_json::Value>();
            if let Some(Ok(v)) = values.next() {
                for p in v
                    .get("input_paths")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(serde_json::Value::as_str)
                {
                    add(p.to_string());
                }
            }
        }
    }
    for token in words.split(|c: char| c.is_whitespace() || "\"'“”‘’<>()".contains(c)) {
        let token = token.trim_end_matches(['.', ',', ';', ':', '!', '?']);
        let full = if let Some(rest) = token.strip_prefix("~/") {
            format!("{}/{rest}", home.trim_end_matches('/'))
        } else if token.starts_with('/') && token.len() > 1 {
            token.to_string()
        } else {
            continue;
        };
        if std::path::Path::new(&full).is_file() {
            add(full);
        }
    }
    out
}

/// One of FERB's uploads in the task: each file's (path, SHA-256) as read, and whether the page's field then held
/// those bytes (`bytes_match`; None: the page couldn't say).
#[derive(Debug, Clone, Default)]
pub struct Upload {
    pub files: Vec<(String, String)>,
    pub bytes_match: Option<bool>,
}

/// What code saw that differs from the named file.
#[derive(Debug, Clone, PartialEq)]
pub enum Seen {
    /// FERB uploaded only other files (their paths).
    Other(Vec<String>),
    /// FERB uploaded the named file but the page's field holds other bytes.
    OtherBytes,
    /// A file field on the page holds a file, and nothing FERB did could have put the named file there.
    FilledElsewhere,
}

/// ORCH-676: before a send or submit, with `named` the owner's files (path, its SHA-256 now), `uploads` FERB's upload
/// receipts in the task, `page_files` what the page's file fields held at FERB's last read, and `byte_routes` whether
/// the task used a route that can carry a file without a receipt (input through the Mac tools, a script, a shell
/// command). The named path counts as the same file whatever its bytes are now (ORCH-670), and the same bytes under
/// another path are the same file. None when nothing is seen to differ, or code can't see.
pub fn file_check(
    named: &[(String, Option<String>)],
    uploads: &[Upload],
    page_files: Option<i64>,
    byte_routes: bool,
    same: &dyn Fn(&str, &str) -> bool,
) -> Option<Seen> {
    if named.is_empty() {
        return None;
    }
    let is_named = |(p, sha): &(String, String)| {
        named
            .iter()
            .any(|(n, now)| same(p, n) || (!sha.is_empty() && now.as_deref() == Some(sha.as_str())))
    };
    let carried: Vec<&Upload> = uploads
        .iter()
        .filter(|u| u.files.iter().any(is_named))
        .collect();
    if let Some(last) = carried.last() {
        return (last.bytes_match == Some(false)).then_some(Seen::OtherBytes);
    }
    let others: Vec<String> = uploads
        .iter()
        .flat_map(|u| u.files.iter().map(|(p, _)| p.clone()))
        .fold(Vec::new(), |mut v, p| {
            if !v.contains(&p) {
                v.push(p);
            }
            v
        });
    if !others.is_empty() {
        return Some(Seen::Other(others));
    }
    (page_files.is_some_and(|n| n > 0) && !byte_routes).then_some(Seen::FilledElsewhere)
}

fn name(path: &str) -> String {
    path.rsplit('/')
        .find(|p| !p.is_empty())
        .unwrap_or(path)
        .to_string()
}

/// The question for what was seen: asked once per site, named file and finding (the answer carries across retries),
/// never routine.
pub fn need(seen: &Seen, named_path: &str, site: &str) -> Need {
    let file = name(named_path);
    let (case, other) = match seen {
        Seen::Other(paths) => (
            "other",
            paths.iter().map(|p| name(p)).collect::<Vec<_>>().join(", "),
        ),
        Seen::OtherBytes => ("bytes", String::new()),
        Seen::FilledElsewhere => ("elsewhere", String::new()),
    };
    let text = format!("the file attached on {site} isn't {file} from your Mac");
    Need::new(
        NOT_NAMED,
        key(&[
            json!(NOT_NAMED),
            json!(site),
            json!(named_path),
            json!(case),
            json!(other),
        ]),
        Values::of(&[
            ("thing", "file"),
            ("name", &file),
            ("path", named_path),
            ("site", site),
            ("case", case),
            ("other", &other),
            ("text", &text),
        ]),
    )
}

/// The owner's question for a `NOT_NAMED` need: (question, detail, exact, yes, no). Plain words, nothing alarming.
pub fn card(v: &Values) -> (String, String, String, &'static str, &'static str) {
    let (name, site) = (v.opt("name"), v.opt("site"));
    let detail = match v.opt("case") {
        "other" => format!(
            "FERB attached {} there, not {name}. Nothing is sent unless you say so.",
            v.opt("other")
        ),
        "bytes" => format!(
            "The page's file field holds different bytes than {name} on your Mac. Nothing is sent unless you say so."
        ),
        _ => "It wasn't uploaded from your Mac by FERB, so it came from somewhere else and may only have the same \
              name. Nothing is sent unless you say so."
            .to_string(),
    };
    (
        format!("The file attached on {site} isn't {name} from your Mac. Send it anyway?"),
        detail,
        v.opt("path").to_string(),
        "Send it anyway",
        "Don't send",
    )
}

/// The line under the task's answer after the owner's No to a `NOT_NAMED` question (from its recorded key).
pub fn declined_line(key_text: &str) -> Option<String> {
    let parts: Vec<serde_json::Value> = serde_json::from_str(key_text).ok()?;
    if parts.first()?.as_str()? != NOT_NAMED {
        return None;
    }
    let site = parts.get(1)?.as_str()?;
    let path = parts.get(2)?.as_str()?;
    Some(format!(
        "I didn't send it: the file attached on {site} wasn't {} from your Mac.",
        name(path)
    ))
}
