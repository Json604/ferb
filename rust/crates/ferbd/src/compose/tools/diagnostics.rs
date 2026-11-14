//! `about.diagnostics` (DMN-595): FERB's own recent diagnostics for the planner and the workers, so FERB can tell the
//! owner why something of its own didn't work. A Rust addition: Python had no such tool (its schema and description
//! are in ferb-gateway's `catalog_additions.json`, beside Python's frozen catalog).
//!
//! What it reads, and nothing else:
//! - the running FERB: version, FERB.app's build (`FERB_APP_BUILD`, as the log's start line says it) and the pid;
//! - each browser profile's connection: found or paired, connected, the extension's version, why a connection was
//!   refused (the browser area's facts, as `browsers.list` reads them, without the profile's email);
//! - FERB's permissions (the About area's, as `about.read` reads them);
//! - the last lines FERB's own log writer wrote to `logs/daemon.err.log` (DMN-592: `<time> FERB: <text>`); anything
//!   else on that stream (another program's output, a backtrace's frames) is left out;
//! - the journal rows the journal itself labels FERB's own (origin `ferb`), the owner's chat left out (the
//!   conversation's kinds, which chat.history reads): by default each kind in the window with its count, first and
//!   last time and its latest row's data; with `kind`, that kind's (or family's) rows newest first. No list of
//!   "problem" kinds: a new kind shows up the day it is first written.
//!
//! Read-only by construction: the journal is read on a connection of its own opened read-only, the log file is only
//! read, and nothing here holds a write path. The gateway redacts the result as it redacts every tool's (the journal's
//! rows and the log's lines were redacted when written too), and labels it with the tool's origin, `ferb`.
//!
//! Bounds (technical, not owner caps): the log's last [`LOG_TAIL_BYTES`] are read and its last [`LOG_LINES`] lines
//! kept, each at most [`LINE_CHARS`]; at most [`KINDS`] kinds or [`ROWS`] rows, each row's data at most [`DATA_CHARS`]
//! ([`ROW_CHARS`] when one kind is asked for); the whole result at most [`MAX_CHARS`] characters as JSON, the oldest
//! log lines and records dropped first (counted in `omitted`).

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use ferb_gateway::tool::{Args, Tool, ToolError};
use rusqlite::{OpenFlags, OptionalExtension};
use serde_json::{json, Map, Value};

use super::context::answer;
use super::{Catalogued, Parts};

pub const NAME: &str = "about.diagnostics";
/// The daemon's own log (DMN-592), under FERB's home.
pub const LOG: &str = "logs/daemon.err.log";
pub const LOG_TAIL_BYTES: u64 = 256 * 1024;
pub const LOG_LINES: usize = 40;
pub const LINE_CHARS: usize = 300;
pub const KINDS: usize = 40;
pub const ROWS: usize = 20;
pub const DATA_CHARS: usize = 300;
pub const ROW_CHARS: usize = 1500;
pub const MAX_CHARS: usize = 16_000;
/// The journal window when none is given (the schema's default).
pub const SINCE_HOURS: f64 = 24.0;

/// What the daemon knows now that isn't in a file: the permissions' states and the browser profiles' connections.
pub type Permissions = Arc<dyn Fn() -> Vec<Value> + Send + Sync>;

/// The owner's chat, which chat.history reads: never part of FERB's diagnostics.
fn conversation(kind: &str) -> bool {
    use ferb_supervisor::words::{CONVERSATION_KINDS, RESULT_KINDS, SELF_POSTS, SUMMARY_KIND};
    CONVERSATION_KINDS.contains(&kind)
        || RESULT_KINDS.contains(&kind)
        || SELF_POSTS.contains(&kind)
        || kind == SUMMARY_KIND
}

fn clip(text: &str, chars: usize) -> String {
    if text.chars().count() > chars {
        format!("{}…", text.chars().take(chars).collect::<String>())
    } else {
        text.to_owned()
    }
}

/// `<YYYY-MM-DD HH:MM:SS.mmm> FERB: `: a line FERB's log writer wrote (`ferbd::log::line`).
fn ferb_line(line: &str) -> bool {
    let b = line.as_bytes();
    let digits = |r: std::ops::Range<usize>| r.into_iter().all(|i| b[i].is_ascii_digit());
    b.len() >= 30
        && digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10)
        && b[10] == b' '
        && digits(11..13)
        && b[13] == b':'
        && digits(14..16)
        && b[16] == b':'
        && digits(17..19)
        && b[19] == b'.'
        && digits(20..23)
        && &b[23..30] == b" FERB: "
}

/// The last [`LOG_LINES`] lines FERB's own log writer wrote, oldest first.
pub fn log_lines(path: &Path) -> Vec<String> {
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(LOG_TAIL_BYTES);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if f.take(LOG_TAIL_BYTES).read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // the read began inside a line
    }
    let kept: Vec<String> = lines
        .into_iter()
        .filter(|l| ferb_line(l))
        .map(|l| clip(l, LINE_CHARS))
        .collect();
    kept[kept.len().saturating_sub(LOG_LINES)..].to_vec()
}

/// The journal's own records of FERB in the window: by kind, or one kind's (or family's) rows.
pub fn journal(db: &Path, since: &str, kind: Option<&str>) -> Result<Value, String> {
    let conn = rusqlite::Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    let e = |e: rusqlite::Error| e.to_string();
    match kind.filter(|k| !k.is_empty()) {
        None => {
            let mut stmt = conn
                .prepare(
                    "SELECT kind, COUNT(*), MIN(at), MAX(at), MAX(seq) FROM journal \
                     WHERE origin = 'ferb' AND at >= ?1 GROUP BY kind ORDER BY MAX(seq) DESC",
                )
                .map_err(e)?;
            let groups: Vec<(String, i64, String, String, i64)> = stmt
                .query_map([since], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })
                .map_err(e)?
                .collect::<Result<_, _>>()
                .map_err(e)?;
            let mut kinds = Vec::new();
            for (k, count, first, last, seq) in groups.into_iter().filter(|g| !conversation(&g.0)) {
                if kinds.len() == KINDS {
                    break;
                }
                let data: Option<String> = conn
                    .query_row("SELECT data_json FROM journal WHERE seq = ?1", [seq], |r| {
                        r.get(0)
                    })
                    .optional()
                    .map_err(e)?;
                kinds.push(
                    json!({"kind": k, "count": count, "first_at": first, "last_at": last,
                                  "latest": clip(&data.unwrap_or_default(), DATA_CHARS)}),
                );
            }
            Ok(json!({"since": since, "kinds": kinds}))
        }
        Some(k) => {
            let family = k.ends_with('.');
            let sql = format!(
                "SELECT at, kind, data_json FROM journal WHERE origin = 'ferb' AND at >= ?1 AND {} \
                 ORDER BY seq DESC",
                if family {
                    "substr(kind, 1, length(?2)) = ?2"
                } else {
                    "kind = ?2"
                }
            );
            let mut stmt = conn.prepare(&sql).map_err(e)?;
            let rows: Vec<(String, String, String)> = stmt
                .query_map([since, k], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(e)?
                .collect::<Result<_, _>>()
                .map_err(e)?;
            let rows: Vec<_> = rows.into_iter().filter(|r| !conversation(&r.1)).collect();
            let total = rows.len();
            let records: Vec<Value> = rows
                .into_iter()
                .take(ROWS)
                .map(|(at, kind, data)| json!({"at": at, "kind": kind, "data": clip(&data, ROW_CHARS)}))
                .collect();
            Ok(json!({"since": since, "kind": k, "total": total, "records": records}))
        }
    }
}

/// Each browser profile found or paired: its pairing and connection, as the browser area's `browsers.list` reads them
/// (the profile's email left out: the owner's account, not FERB's state).
pub fn browsers(cx: &ferb_daemon::areas_b::browsers::Cx) -> Result<Value, String> {
    let d = &cx.deps;
    let ferb = cx.identity_profile().map_err(|e| e.message)?;
    let bindings = d.pairings.all().map_err(|e| e.0)?;
    let paired: Vec<(String, String)> = bindings
        .iter()
        .map(|b| (b.browser.clone(), b.folder.clone()))
        .collect();
    let mut out = Vec::new();
    for p in ferb_browser::profiles::with_paired(d.platform.as_ref(), &cx.profiles(), &paired) {
        let b = bindings
            .iter()
            .find(|b| b.browser == p.browser && b.folder == p.folder);
        let br = d.hub.bridge(&p.browser, &p.folder);
        let connected = br.as_ref().is_some_and(|b| b.connected);
        let version = match (&br, b) {
            (Some(br), _) if !br.version.is_empty() => br.version.clone(),
            (_, Some(b)) => d.pairings.version(&b.install_id).map_err(|e| e.0)?,
            _ => String::new(),
        };
        let key = (p.browser.clone(), p.folder.clone());
        out.push(json!({"browser": p.browser, "folder": p.folder, "name": p.display_name,
                        "ferb_identity": Some(&key) == ferb.as_ref(), "paired": b.is_some(),
                        "identity": b.map(|b| b.identity.as_str()), "connected": connected,
                        "extension_version": version,
                        "refused": if connected { None } else { b.and_then(|b| d.hub.problem(b)) }}));
    }
    let unpaired = d.hub.unpaired();
    Ok(json!({"profiles": out, "unpaired_connections": unpaired.len()}))
}

/// The result trimmed to [`MAX_CHARS`]: the oldest log lines and the oldest records go first.
pub fn bounded(mut out: Map<String, Value>) -> Map<String, Value> {
    let size = |o: &Map<String, Value>| ferb_py::json::dumps_utf8(o).chars().count();
    let mut omitted = 0;
    while size(&out) > MAX_CHARS {
        let log = out
            .get("log")
            .and_then(|l| l.get("lines"))
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let journal = out.get("journal").cloned().unwrap_or(Value::Null);
        let list = if journal.get("kinds").is_some() {
            "kinds"
        } else {
            "records"
        };
        let rec = journal
            .get(list)
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        if log == 0 && rec == 0 {
            break;
        }
        if log >= rec {
            if let Some(Value::Array(a)) = out.get_mut("log").and_then(|l| l.get_mut("lines")) {
                a.remove(0);
            }
        } else if let Some(Value::Array(a)) = out.get_mut("journal").and_then(|j| j.get_mut(list)) {
            a.pop();
        }
        omitted += 1;
    }
    if omitted > 0 {
        out.insert("omitted".into(), json!(omitted));
    }
    out
}

/// One call: what it reads, put together and bounded. `browsers` and `permissions` are what the daemon knows now.
pub fn compose(
    home: &Path,
    since: &str,
    kind: Option<&str>,
    running: Value,
    browsers: Value,
    permissions: Vec<Value>,
) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    out.insert("running".into(), running);
    out.insert("browsers".into(), browsers);
    out.insert("permissions".into(), Value::Array(permissions));
    out.insert(
        "log".into(),
        json!({"file": LOG, "lines": log_lines(&home.join(LOG))}),
    );
    out.insert(
        "journal".into(),
        journal(&home.join("ferb.db"), since, kind)?,
    );
    Ok(bounded(out))
}

fn since_hours(a: &Args) -> f64 {
    a.get("since_hours")
        .and_then(Value::as_f64)
        .unwrap_or(SINCE_HOURS)
}

/// The tool in the registry (after about.read, as the catalog's addition says).
pub fn tool(p: &Parts) -> Arc<dyn Tool> {
    let parts = p.clone();
    Arc::new(Catalogued {
        name: NAME,
        describe: Arc::new(|_a| ferb_core::types::ActionSpec::new("read")),
        run: Arc::new(move |a, _c| {
            let p = parts.clone();
            let hours = since_hours(a);
            let kind = a.get("kind").and_then(Value::as_str).map(str::to_owned);
            Box::pin(async move {
                let now = p.clock().now();
                let back = chrono::Duration::milliseconds((hours * 3_600_000.0).min(1e15) as i64);
                let since = ferb_core::when::isoformat(&(now - back));
                let running = json!({"ferb": crate::compose::VERSION,
                    "build": std::env::var("FERB_APP_BUILD").unwrap_or_else(|_| "none".into()),
                    "pid": std::process::id()});
                let area = p.browsers().and_then(|b| b.area());
                let permissions = p.diagnostics_permissions();
                let home = p.home().clone();
                let content = tokio::task::spawn_blocking(move || {
                    let browsers = match &area {
                        Some(cx) => browsers(cx)?,
                        None => json!({"profiles": [], "unpaired_connections": 0}),
                    };
                    let permissions = permissions.map(|f| f()).unwrap_or_default();
                    compose(
                        &home,
                        &since,
                        kind.as_deref(),
                        running,
                        browsers,
                        permissions,
                    )
                })
                .await
                .map_err(|e| ToolError::failed("FerbError", e.to_string()))?
                .map_err(|e| ToolError::failed("FerbError", e))?;
                Ok(answer(Value::Object(content), false, Vec::new()).into())
            })
        }),
    })
}
