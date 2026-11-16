//! Finding the Mac's files from a paired phone (kb T-1211): its recent files (`phone.recent`) and a search of file
//! names (`phone.search`). Both are listings: names, sizes and times, never a file's content; both private (the unlock
//! window, PHN-125), and both judged by the very lists every phone file call is (PHN-482): FERB's own data never
//! shows, and a key place (FERB's never-read list, PHN-131's places, key files by name, a file with more than one name)
//! is left out and counted, so a phone learns nothing of what lies inside one without its fingerprint, folder by
//! folder, through `phone.files`.
//!
//! - **Recent** (PHN-526): the files FERB's tasks made (each finished task's `artifacts`, the result card's files) and
//!   the files this phone fetched or sent (its own audit rows), newest first; one that is gone, a key place or FERB's
//!   own data is left out and counted. Nothing new is recorded for it.
//! - **Search** (PHN-527): names holding the query (case and Unicode form ignored) under a folder the phone may
//!   browse (`~` by default), shallow folders first. Hidden names, `~/Library` (unless the search starts inside it) and
//!   the inside of apps are not looked into; links are not followed. At most `FileLimits::list_max` results (`cut`)
//!   and `FileLimits::search_names_max` names looked at (`incomplete`): both provisional numbers (T-1217). A search
//!   never starts inside a key place.

use std::collections::{HashSet, VecDeque};
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::Arc;

use ferb_policy::ppath;
use ferb_rpc::phone_gate::{self, Refusal};
use ferb_rpc::server::{HandlerError, HandlerResult};
use ferb_rpc::{Params, Server, Session};
use serde_json::{json, Value};

use super::files::{
    area, bad, coded, expand, fold, kind, names_in, open_at, open_for_phone, path_param, phone_of,
    stat_at, under, Area,
};
use super::{lock, refused, Inner, Phones};

/// A file one of FERB's tasks made (its outcome's `artifacts`).
#[derive(Clone, Debug, PartialEq)]
pub struct Made {
    pub path: String,
    /// When the task ended (ISO 8601).
    pub at: String,
    pub task_id: String,
    /// The task's name as FERB shows it.
    pub task_name: String,
}

/// The recent files a phone asks for when it names no number, and the most it may ask.
const RECENT_DEFAULT: usize = 50;
const RECENT_MOST: usize = 200;
/// The longest query taken (a part of a name).
const QUERY_CHARS: usize = 100;

/// One file as a listing shows it, or why it is left out.
enum Seen {
    Shown(Value),
    /// A key place or FERB's own data.
    Closed,
    /// No longer there (or not readable).
    Gone,
}

/// A path as the phone may see it in a listing: by every name it has now (a link judged where it leads too).
fn look(a: &Area, path: &str) -> Seen {
    let Ok(m) = std::fs::symlink_metadata(path) else {
        return Seen::Gone;
    };
    match a.judged(path, &[]) {
        Err(_) | Ok(true) => return Seen::Closed,
        Ok(false) => {}
    }
    let (m, real) = if m.file_type().is_symlink() {
        let Ok(real) = std::fs::canonicalize(path) else {
            return Seen::Gone;
        };
        let real = real.to_string_lossy().into_owned();
        match a.judged(&real, &[]) {
            Err(_) | Ok(true) => return Seen::Closed,
            Ok(false) => {}
        }
        match std::fs::metadata(&real) {
            Ok(m) => (m, real),
            Err(_) => return Seen::Gone,
        }
    } else {
        (m, path.to_string())
    };
    use std::os::unix::fs::MetadataExt;
    if m.is_file() && m.nlink() > 1 {
        return Seen::Closed; // a file with more than one name is a key place (PHN-140)
    }
    if !m.is_file() && !m.is_dir() {
        return Seen::Gone;
    }
    Seen::Shown(json!({
        "name": ppath::name(&real), "path": real, "dir": m.is_dir(),
        "size": if m.is_file() { m.len() } else { 0 }, "modified": m.mtime(),
    }))
}

/// `phone.recent {limit?}`: the files FERB made and this phone fetched or sent, newest first.
fn recent(inner: &Inner, session: &Session, params: &Params) -> Result<Params, Refusal> {
    let phone = phone_of(session);
    let a = area(inner)?;
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(RECENT_DEFAULT, |n| n as usize)
        .clamp(1, RECENT_MOST);
    // (when, path, source, task id, task name)
    let mut rows: Vec<(
        Option<ferb_core::when::DateTime>,
        String,
        String,
        Value,
        Value,
    )> = Vec::new();
    if let Some(made) = &a.seams.made_files {
        for m in made(limit * 2) {
            rows.push((
                ferb_core::when::parse(&m.at),
                m.path,
                "made".into(),
                json!(m.task_id),
                json!(m.task_name),
            ));
        }
    }
    let audited = {
        let db = lock(&inner.seams.db);
        ferb_store::phones::audit_ok_of(
            db.conn(),
            &phone.device,
            &["phone.read", "phone.put_end"],
            limit * 4,
        )
        .unwrap_or_default()
    };
    for r in audited {
        let Some(path) = r.detail.get("path").and_then(Value::as_str) else {
            continue;
        };
        let source = if r.method == "phone.read" {
            "fetched"
        } else {
            "sent"
        };
        rows.push((
            ferb_core::when::parse(&r.at),
            path.to_string(),
            source.into(),
            Value::Null,
            Value::Null,
        ));
    }
    // newest first (by the instant, whatever offset each was written in), each path once
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    let (mut files, mut skipped, mut seen) = (Vec::new(), 0u64, HashSet::new());
    for (at, path, source, task_id, task_name) in rows {
        if files.len() >= limit {
            break;
        }
        if !path.starts_with('/') || !seen.insert(path.clone()) {
            continue;
        }
        match look(a, &path) {
            Seen::Shown(mut v) => {
                v["source"] = json!(source);
                v["at"] = json!(at.map(|t| ferb_core::when::isoformat(&t)));
                v["task_id"] = task_id;
                v["task_name"] = task_name;
                files.push(v);
            }
            Seen::Closed | Seen::Gone => skipped += 1,
        }
    }
    let mut out = Params::new();
    out.insert("files".into(), Value::Array(files));
    out.insert("skipped".into(), json!(skipped));
    Ok(out)
}

/// The folders a search never goes into (it still matches their names): the inside of apps and similar bundles.
fn bundle(name: &str) -> bool {
    let n = fold(name);
    [
        ".app",
        ".photoslibrary",
        ".musiclibrary",
        ".bundle",
        ".framework",
    ]
    .iter()
    .any(|s| n.ends_with(s))
}

/// Opens a folder under `root` by its parts, never following a link at any step.
fn open_chain(root: &OwnedFd, parts: &[Vec<u8>]) -> Option<OwnedFd> {
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK;
    let mut at: Option<OwnedFd> = None;
    for p in parts {
        let dir = at.as_ref().map_or(root.as_raw_fd(), AsRawFd::as_raw_fd);
        at = Some(OwnedFd::from(open_at(dir, p, flags, 0).ok()?));
    }
    match at {
        Some(fd) => Some(fd),
        None => root.try_clone().ok(),
    }
}

/// `phone.search {query, path?}`: names holding the query under a folder, shallow first.
fn search(inner: &Inner, session: &Session, params: &Params) -> Result<Params, Refusal> {
    let phone = phone_of(session);
    let a = area(inner)?;
    let query: String = params
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .chars()
        .take(QUERY_CHARS)
        .collect();
    if query.is_empty() {
        return Err(bad("A search needs part of a file's name."));
    }
    let want = fold(&query);
    let sent = path_param(params, true)?;
    let home = a.seams.judge.home();
    let typed = expand(&sent, &home)
        .ok_or_else(|| bad("A folder is named by its whole path (or one starting with ~/)."))?;
    // decision: PHN-527
    if a.judged(&typed, &[])? {
        return Err(refused(
            phone_gate::FILE_NOT_ALLOWED,
            "FERB doesn't search inside places that need your fingerprint. Open that folder itself to look inside.",
        ));
    }
    let o = open_for_phone(inner, a, &phone, "phone.files", &sent, true)?;
    if o.high {
        return Err(refused(
            phone_gate::FILE_NOT_ALLOWED,
            "FERB doesn't search inside places that need your fingerprint. Open that folder itself to look inside.",
        ));
    }
    let library = ppath::div(&home, "Library");
    let skip_library = !under(&o.real, &library);
    let root = OwnedFd::from(o.file);
    let (most, names_max) = (a.limits.list_max, a.limits.search_names_max);
    let (mut files, mut looked, mut skipped, mut cut, mut incomplete) =
        (Vec::new(), 0u64, 0u64, false, false);
    let mut queue: VecDeque<(Vec<Vec<u8>>, String)> =
        VecDeque::from([(Vec::new(), o.real.clone())]);
    'walk: while let Some((parts, real)) = queue.pop_front() {
        let Some(dir) = open_chain(&root, &parts) else {
            continue;
        };
        let Ok(mut names) = names_in(dir.as_raw_fd()) else {
            continue;
        };
        names.sort();
        for raw in names {
            if looked as usize >= names_max {
                incomplete = true;
                break 'walk;
            }
            looked += 1;
            let name = String::from_utf8_lossy(&raw).into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(st) = stat_at(dir.as_raw_fd(), &raw) else {
                continue;
            };
            let child = ppath::div(&real, &name);
            let (closed, key) = a.closed_or_key(&child);
            #[allow(clippy::unnecessary_cast)]
            let many = kind(&st) == libc::S_IFREG && st.st_nlink as u64 > 1;
            if closed || key || many {
                skipped += 1;
                continue;
            }
            let is_dir = kind(&st) == libc::S_IFDIR;
            if !is_dir && kind(&st) != libc::S_IFREG {
                continue; // links, pipes, devices: not followed, not listed
            }
            if fold(&name).contains(&want) {
                if files.len() >= most {
                    cut = true;
                    break 'walk;
                }
                #[allow(clippy::unnecessary_cast)]
                files.push(json!({
                    "name": name, "path": child, "dir": is_dir,
                    "size": if is_dir { 0 } else { st.st_size as u64 }, "modified": st.st_mtime,
                }));
            }
            if is_dir && !bundle(&name) && !(skip_library && child == library) {
                let mut next = parts.clone();
                next.push(raw);
                queue.push_back((next, child));
            }
        }
    }
    let mut out = Params::new();
    out.insert("path".into(), json!(o.real));
    out.insert("query".into(), json!(query));
    out.insert("files".into(), Value::Array(files));
    out.insert("looked".into(), json!(looked));
    out.insert("skipped".into(), json!(skipped));
    out.insert("cut".into(), json!(cut));
    out.insert("incomplete".into(), json!(incomplete));
    Ok(out)
}

type Find = fn(&Inner, &Session, &Params) -> Result<Params, Refusal>;

/// Registers `phone.recent` and `phone.search` (each walks the disk: off the runtime's threads).
pub(crate) fn register(phones: &Phones, server: &Server) {
    let finds: [(&str, Find); 2] = [("phone.recent", recent), ("phone.search", search)];
    for (name, f) in finds {
        let inner: Arc<Inner> = phones.inner.clone();
        server.register_unary(name, move |s: Session, p: Params| {
            let inner = inner.clone();
            async move {
                tokio::task::spawn_blocking(move || f(&inner, &s, &p).map_err(coded))
                    .await
                    .unwrap_or_else(|e| -> HandlerResult {
                        Err(HandlerError::fault("RuntimeError", e.to_string()))
                    })
            }
        });
    }
}
