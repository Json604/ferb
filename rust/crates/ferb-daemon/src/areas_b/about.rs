//! FERB's self-description (DMN-6, DMN-22: `about.get`, `about.guide`, the always-on summary, the `about.read`
//! tool's document), FERB.app's saved secrets (APP-2: `secrets.saved`, `secrets.delete`) and "What FERB saw" (step
//! 12e: `seen.list`, `seen.image`). Port of `ferb/daemon/areas/about.py` and `ferb/daemon/about.py`.
//!
//! Must not change a setting, show a secret's value, let anything but FERB.app delete one or see a screenshot, take a
//! capture anew, or show an image of an app or site on the owner's exclusion list.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine;
use ferb_browser::profiles::{named, shown, BrowserProfile};
use ferb_identity::owner_secrets::OwnerSecrets;
use ferb_rpc::{HandlerError, HandlerResult, Server};
use ferb_runtimes::catalog::{self, ROLES};
use rusqlite::OptionalExtension;
use serde_json::{json, Map, Value};

use super::core::{ok, refusal, Host, P};
use super::notify::SharedDb;

/// The document's sections, in order (`about.SECTIONS`; ORCH-578: what lets FERB act without asking).
pub const SECTIONS: [&str; 11] = [
    "version",
    "models",
    "account",
    "permissions",
    "settings",
    "records",
    "health",
    "cost",
    "app",
    "permissions_in_force",
    "saved_logins",
];
/// The app guide FERB.app sends: at most this many entries of each list, each string at most this long.
pub const GUIDE_ITEMS: usize = 80;
pub const GUIDE_CHARS: usize = 400;
/// A shared FERB.app setting's value is one short line (APP-728).
pub const VALUE_CHARS: usize = 120;

/// The always-on lines (`about.summary`): no live numbers (prompt cache). `models`: role -> "runtime model".
pub fn summary(version: &str, models: &[(String, String)]) -> String {
    let shown: Vec<String> = models.iter().map(|(r, m)| format!("{r} {m}")).collect();
    format!(
        "- About FERB: version {version}; models: {}. The owner types or dictates messages to FERB in FERB.app \
         (dictation turns their speech into text); FERB always replies in text in FERB.app and never speaks aloud: \
         it has no voice. FERB's settings (AI models, permissions, browsers, notifications, privacy and how long \
         records are kept) are the owner's to change in FERB.app's Settings; FERB never changes them and tells the \
         owner where they are. A worker reads FERB's full self-description (what it records about the owner, where \
         and for how long, its permissions and settings, health, cost and how to do things in FERB.app) with the \
         about.read tool.",
        shown.join("; ")
    )
}

/// `" ".join(x.split())[:GUIDE_CHARS]` for a str, else "".
fn guide_text(x: Option<&Value>) -> String {
    match x {
        Some(Value::String(s)) => {
            ferb_py::value::prefix(&ferb_py::value::squeeze(s), GUIDE_CHARS).to_owned()
        }
        _ => String::new(),
    }
}

fn dicts(v: Option<&Value>) -> Vec<&Map<String, Value>> {
    match v {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_object).collect(),
        _ => vec![],
    }
}

/// `about.clean_guide`: the app guide as FERB.app sent it, kept only in its shape (DMN-545).
///
/// ENG-6: Python iterated `howtos`, `shortcuts` and `app_settings` whatever their type, so a string there was walked
/// character by character (and a number raised TypeError, an internal error); here anything but a list counts as
/// none (a string's characters are never dicts, so the kept guide is the same).
pub fn clean_guide(value: &Value) -> Map<String, Value> {
    let empty = Map::new();
    let data = value.as_object().unwrap_or(&empty);
    let mut howtos = vec![];
    for h in dicts(data.get("howtos")).into_iter().take(GUIDE_ITEMS) {
        let (t, s) = (guide_text(h.get("title")), guide_text(h.get("steps")));
        if !t.is_empty() && !s.is_empty() {
            howtos.push(json!({"title": t, "steps": s}));
        }
    }
    let mut shortcuts = vec![];
    for k in dicts(data.get("shortcuts")).into_iter().take(GUIDE_ITEMS) {
        let (a, keys) = (guide_text(k.get("action")), guide_text(k.get("keys")));
        if !a.is_empty() && !keys.is_empty() {
            shortcuts.push(json!({"action": a, "keys": keys}));
        }
    }
    let mut places = Map::new();
    if let Some(Value::Object(raw)) = data.get("places") {
        for (k, v) in raw.iter().take(GUIDE_ITEMS) {
            let (k, v) = (
                guide_text(Some(&Value::String(k.clone()))),
                guide_text(Some(v)),
            );
            places.insert(k, Value::String(v));
        }
    }
    places.retain(|k, v| !k.is_empty() && v.as_str().is_some_and(|s| !s.is_empty()));
    let mut settings = vec![];
    for row in dicts(data.get("app_settings"))
        .into_iter()
        .take(GUIDE_ITEMS)
    {
        let mut entry = Map::new();
        for f in ["key", "name", "where", "what"] {
            entry.insert(f.into(), json!(guide_text(row.get(f))));
        }
        let filled = |f: &str| entry[f].as_str().is_some_and(|s| !s.is_empty());
        if filled("key") && filled("name") && filled("where") {
            if let Some(v @ Value::String(_)) = row.get("value") {
                let v = guide_text(Some(v));
                entry.insert(
                    "value".into(),
                    json!(ferb_py::value::prefix(&v, VALUE_CHARS)),
                );
            }
            settings.push(Value::Object(entry));
        }
    }
    let mut out = Map::new();
    out.insert("howtos".into(), json!(howtos));
    out.insert("shortcuts".into(), json!(shortcuts));
    out.insert("places".into(), Value::Object(places));
    out.insert("app_settings".into(), json!(settings));
    out
}

/// The browser facts the account section reads (the browsers area's): profiles found now (none before Full Disk
/// Access) and whether FERB's own profile is paired as FERB.
pub trait Account: Send + Sync {
    fn profiles(&self) -> Vec<BrowserProfile>;
    fn identity_paired(&self) -> bool;
}

/// Where the stored screenshots are, and what the evidence store's rows reach (`EvidenceStore.stored_file`).
pub struct Evidence {
    pub root: PathBuf,
}

impl Evidence {
    /// A row's file, only when it resolves inside the store (symlinks followed); else None.
    pub fn stored_file(&self, rel: Option<&str>) -> Option<PathBuf> {
        let rel = rel.filter(|r| !r.is_empty())?;
        let path = std::fs::canonicalize(self.root.join(rel)).ok()?;
        let root = std::fs::canonicalize(&self.root).ok()?;
        path.starts_with(&root).then_some(path)
    }
}

/// `self.tasks.tree_of(task_id)`.
pub type TreeOf = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// A source of values the other parts own.
pub type Read = Arc<dyn Fn() -> Result<Value, String> + Send + Sync>;

pub struct Deps {
    /// `ferb.__version__`.
    pub version: String,
    /// FERB.app's guide as last sent (`_app_guide`, in memory).
    pub guide: Arc<Mutex<Option<Map<String, Value>>>>,
    /// `self.permission_check()`: each permission's status (TCC reads, run off the loop).
    pub grants: Arc<dyn Fn() -> Map<String, Value> + Send + Sync>,
    /// `_setup_containment()`.
    pub containment: Arc<dyn Fn() -> Map<String, Value> + Send + Sync>,
    pub account: Arc<dyn Account>,
    /// The daemon's store (health, cost, the evidence rows); None before it opens.
    pub store: Option<SharedDb>,
    /// `store_size(path)` and `cleanup.pending(path)` of that store.
    pub store_facts: Arc<dyn Fn() -> (u64, bool) + Send + Sync>,
    /// records.py `_permissions_in_force` and `_saved_logins` (group A's records area).
    pub permissions_in_force: Read,
    pub saved_logins: Read,
    /// The owner's saved secrets (`OwnerSecrets(home)`).
    pub owner_secrets: Arc<OwnerSecrets>,
    pub evidence: Option<Arc<Evidence>>,
    /// `self.tasks.tree_of(task_id)`.
    pub tree_of: TreeOf,
}

/// `_runtime_for(role)` for the three configured roles (runtimes.py:30).
fn runtime_for(h: &Host, role: &str) -> Result<String, HandlerError> {
    let s = |v: Value| ferb_py::value::py_str(&v);
    let worker = s(h.get_or("runtimes", "worker", json!("codex"))?);
    Ok(match role {
        "orchestrator" => s(h.get_or("runtimes", "orchestrator", json!("codex"))?),
        "checker" => match h.get("runtimes", "checker")? {
            Some(v) => s(v),
            None => worker,
        },
        _ => worker,
    })
}

/// The config as the runtime catalog reads it.
struct Cfg<'a>(&'a Host);

impl catalog::Config for Cfg<'_> {
    fn get(&self, section: &str, key: &str) -> Option<Value> {
        self.0.config.get(section, key).ok().flatten()
    }
}

/// `_models`: role -> "runtime model" (planner for the orchestrator).
pub fn models(h: &Host) -> Result<Vec<(String, String)>, HandlerError> {
    let mut out = vec![];
    for role in ROLES {
        let name = runtime_for(h, role)?;
        let text = format!(
            "{} {}",
            catalog::label(&name),
            catalog::role_model(&Cfg(h), &name, role)
        );
        let role = if role == "orchestrator" {
            "planner"
        } else {
            role
        };
        out.push((role.to_owned(), ferb_py::value::strip(&text).to_owned()));
    }
    Ok(out)
}

/// `_about_summary`.
pub fn about_summary(h: &Host, version: &str) -> Result<String, HandlerError> {
    Ok(summary(version, &models(h)?))
}

fn config_text(h: &Host, section: &str, key: &str) -> Result<String, HandlerError> {
    Ok(match h.get(section, key)? {
        Some(v) if ferb_py::value::truthy(&v) => ferb_py::value::py_str(&v),
        _ => String::new(),
    })
}

/// `_identity_profile`: FERB's own profile from config (`[identity] browser/profile`).
pub fn identity_profile(h: &Host) -> Result<Option<(String, String)>, HandlerError> {
    let folder = h.get("identity", "profile")?;
    if !folder.as_ref().is_some_and(ferb_py::value::truthy) {
        return Ok(None);
    }
    let browser = ferb_py::value::py_str(&h.get_or("identity", "browser", json!("chrome"))?);
    Ok(Some((
        browser,
        ferb_py::value::py_str(&folder.unwrap_or_default()),
    )))
}

/// `_owner_browsing_profile` (`browsing.owner_profile`, APP-1).
pub fn owner_browsing_profile(h: &Host) -> Result<Option<(String, String)>, HandlerError> {
    let raw = config_text(h, "browsing", "owner_profile")?;
    let (b, f) = raw.split_once('/').unwrap_or((&raw, ""));
    Ok((!b.is_empty() && !f.is_empty()).then(|| (b.to_owned(), f.to_owned())))
}

fn fault(e: impl std::fmt::Display) -> HandlerError {
    HandlerError::fault("DatabaseError", e.to_string())
}

/// `_about_document`: the whole self-description, redacted (FERB.app's About page and `about.read` read this one).
pub fn document(h: &Host, d: &Deps) -> HandlerResult {
    let guide = d
        .guide
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| clean_guide(&json!({})));
    let places = guide["places"].as_object().cloned().unwrap_or_default();
    let rows = ferb_mac_hands::permissions::rows(&(d.grants)(), &(d.containment)(), &[]);
    let values = crate::areas_a::settings::read(&h.config).map_err(|e| refusal(e.to_string()))?;
    let address = config_text(h, "mailbox", "address")?;
    let (prof, owner) = (identity_profile(h)?, owner_browsing_profile(h)?);
    // HAND-424: profiles by name with the account each is signed in to, read fresh
    let found = if prof.is_some() || owner.is_some() {
        d.account.profiles()
    } else {
        vec![]
    };
    let mine = prof.as_ref().map(|(b, f)| named(&found, b, f));
    let theirs = owner.as_ref().map(|(b, f)| named(&found, b, f));
    let account = if !address.is_empty() {
        json!({"email": address, "browser": prof.as_ref().map(|p| p.0.clone()).unwrap_or_default(),
               "profile": prof.as_ref().map(|p| p.1.clone()).unwrap_or_default(),
               "profile_name": mine.as_ref().map(|m| m.display_name.clone()).unwrap_or_default(),
               "paired": d.account.identity_paired()})
    } else {
        let mut note = "FERB has no Google account of its own; features that need its own inbox (one-time codes \
                        for its own sign-ups, sending from its address) aren't set up"
            .to_owned();
        if let Some(t) = &theirs {
            note.push_str(&format!("; it browses as the owner in {}", shown(t, None)));
        }
        let mut a = json!({"email": "", "note": note});
        if let Some(t) = &theirs {
            a["owner_profile"] = json!({"browser": t.browser, "folder": t.folder, "name": t.display_name,
                                         "email": t.email});
        }
        a
    };
    let settings: Vec<Value> = values
        .iter()
        .map(|(k, v)| {
            let place = places
                .get(k)
                .cloned()
                .unwrap_or(json!("FERB.app → Settings"));
            json!({"key": k, "value": v, "where": place})
        })
        .collect();
    let get_days = |section: &str, key: &str, default: i64| -> i64 {
        match h.config.get(section, key).ok().flatten() {
            Some(Value::Number(n)) => n
                .as_i64()
                .unwrap_or_else(|| n.as_f64().unwrap_or(0.0) as i64),
            Some(Value::Bool(b)) => i64::from(b),
            _ => default,
        }
    };
    let records = ferb_store::retention::describe(&get_days);
    let app_settings = guide.get("app_settings").cloned().unwrap_or(json!([]));
    let app = json!({"howtos": guide["howtos"], "shortcuts": guide["shortcuts"],
                     "where_settings_live": if places.is_empty() {
                         json!("FERB.app hasn't sent its guide since FERB started") } else { Value::Object(places) },
                     "settings": if ferb_py::value::truthy(&app_settings) { app_settings } else { json!([]) }});
    let in_force = match &d.store {
        Some(_) => (d.permissions_in_force)().map_err(fault)?,
        None => json!({}),
    };
    let doc = json!({
        "version": {"ferb": d.version},
        "models": models(h)?.into_iter().map(|(r, m)| (r, Value::String(m))).collect::<Map<String, Value>>(),
        "account": account,
        "permissions": rows.iter().map(|r| json!({"id": r["id"], "status": r["status"]})).collect::<Vec<_>>(),
        "settings": settings,
        "records": records,
        "health": health(h, d)?,
        "cost": cost(h, d)?,
        "app": app,
        "permissions_in_force": in_force,
        "saved_logins": (d.saved_logins)().map_err(fault)?,
    });
    ok(h.redact(&doc))
}

/// `_about_health`: the database's size and clean-up state; FERB's CPU and memory over the last day (L30).
fn health(h: &Host, d: &Deps) -> Result<Value, HandlerError> {
    let mut out = Map::new();
    let Some(db) = &d.store else {
        return Ok(Value::Object(out));
    };
    let (size, pending) = (d.store_facts)();
    out.insert("database_bytes".into(), json!(size));
    out.insert("cleanup_waiting_for_ok".into(), json!(pending));
    let since = ferb_core::when::isoformat(&(h.now() - chrono::Duration::days(1)));
    let db = db.lock().unwrap();
    let conn = db.conn();
    let row = conn
        .query_row(
            "SELECT SUM(window_s), SUM(cpu_s), MAX(peak_footprint) FROM resource_samples WHERE at >= ?",
            [&since],
            |r| Ok((r.get::<_, Option<f64>>(0)?, r.get::<_, Option<f64>>(1)?, r.get::<_, Option<i64>>(2)?)),
        )
        .ok();
    let last = conn
        .query_row(
            "SELECT footprint FROM resource_samples ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get::<_, Option<i64>>(0),
        )
        .optional()
        .ok()
        .flatten();
    if let Some((Some(window), cpu, peak)) = row {
        if window != 0.0 {
            out.insert(
                "cpu_share_last_day".into(),
                json!(ferb_py_round(cpu.unwrap_or(0.0) / window, 4)),
            );
            out.insert("memory_peak_last_day_bytes".into(), json!(peak));
        }
    }
    if let Some(Some(now)) = last {
        out.insert("memory_now_bytes".into(), json!(now));
    }
    Ok(Value::Object(out))
}

/// Python's `round(x, n)` for a float (correctly rounded, half to even on the exact decimal value).
pub fn ferb_py_round(x: f64, n: i32) -> f64 {
    let text = format!("{:.*}", n as usize, x);
    text.parse().unwrap_or(x)
}

/// `_about_cost`: the priced model calls in traces over the last 7 and 30 days.
fn cost(h: &Host, d: &Deps) -> Result<Value, HandlerError> {
    let Some(db) = &d.store else {
        return Ok(json!({}));
    };
    let mut out = json!({"basis": "API-equivalent price of the model calls FERB traced"});
    let db = db.lock().unwrap();
    for days in [7, 30] {
        let since = ferb_core::when::isoformat(&(h.now() - chrono::Duration::days(days)));
        let sum: Option<f64> = db
            .conn()
            .query_row(
                "SELECT SUM(cost_usd) FROM spans WHERE cost_usd IS NOT NULL AND started_at >= ?",
                [&since],
                |r| r.get(0),
            )
            .map_err(fault)?;
        out[format!("last_{days}_days_usd")] = json!(ferb_py_round(sum.unwrap_or(0.0), 2));
    }
    Ok(out)
}

pub fn register(server: &Server, host: &Host, deps: &Arc<Deps>) {
    let (h, d) = (host.clone(), deps.clone());
    server.register_unary("about.get", move |_s, _p: Map<String, Value>| {
        let (h, d) = (h.clone(), d.clone());
        async move {
            tokio::task::spawn_blocking(move || document(&h, &d))
                .await
                .map_err(join)?
        }
    });
    let d = deps.clone();
    server.register_unary("about.guide", move |_s, p: Map<String, Value>| {
        let d = d.clone();
        async move {
            let g = clean_guide(&Value::Object(p));
            let n = |k: &str| match &g[k] {
                Value::Array(a) => a.len(),
                Value::Object(o) => o.len(),
                _ => 0,
            };
            let answer = json!({"howtos": n("howtos"), "shortcuts": n("shortcuts"), "places": n("places"),
                                "app_settings": n("app_settings")});
            *d.guide.lock().unwrap() = Some(g);
            ok(answer)
        }
    });
    let d = deps.clone();
    server.register_unary("secrets.saved", move |_s, _p: Map<String, Value>| {
        let d = d.clone();
        async move {
            let entries = tokio::task::spawn_blocking(move || d.owner_secrets.inventory())
                .await
                .map_err(join)?
                .map_err(identity_error)?;
            ok(json!({"entries": entries}))
        }
    });
    let (h, d) = (host.clone(), deps.clone());
    server.register_unary("secrets.delete", move |_s, p: Map<String, Value>| {
        let (h, d) = (h.clone(), d.clone());
        async move {
            let p = P(&p);
            let (place, name) = (p.text_or_empty("place"), p.text_or_empty("name"));
            if place.is_empty() || name.is_empty() {
                return Err(refusal("say which saved secret: its place and its name"));
            }
            let (pl, nm) = (place.clone(), name.clone());
            let out = tokio::task::spawn_blocking(move || d.owner_secrets.delete(&pl, &nm))
                .await
                .map_err(join)?
                .map_err(identity_error)?;
            h.journal(
                "secret.deleted",
                json!({"place": place, "name": name}),
                Some("owner"),
            )?;
            ok(out)
        }
    });
    let (h, d) = (host.clone(), deps.clone());
    server.register_unary("seen.list", move |_s, p: Map<String, Value>| {
        let (h, d) = (h.clone(), d.clone());
        async move { seen_list(&h, &d, &p) }
    });
    let (h, d) = (host.clone(), deps.clone());
    server.register_unary("seen.image", move |_s, p: Map<String, Value>| {
        let (h, d) = (h.clone(), d.clone());
        async move { seen_image(&h, &d, &p) }
    });
}

fn join(e: tokio::task::JoinError) -> HandlerError {
    HandlerError::fault("RuntimeError", e.to_string())
}

/// `Refused` → FERB's own words; anything else is an internal error, as Python's other exceptions are.
fn identity_error(e: ferb_identity::Error) -> HandlerError {
    match e {
        ferb_identity::Error::Refused(text) => refusal(text),
        ferb_identity::Error::HelperFailed { message, .. } => refusal(message),
        other => HandlerError::fault("OSError", other.to_string()),
    }
}

/// `_seen_item`: one stored screenshot as "What FERB saw" lists it.
fn seen_item(row: &BTreeMap<String, Value>) -> Map<String, Value> {
    let data: Value = row
        .get("data_json")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .and_then(|s| ferb_py::json::loads(s).ok())
        .unwrap_or(json!({}));
    let data = if data.is_object() { data } else { json!({}) };
    let field = |k: &str| match data.get(k) {
        Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
        _ => String::new(),
    };
    let url = field("url");
    let site = if url.starts_with("http://") || url.starts_with("https://") {
        ferb_policy::pyurl::urlsplit(&url)
            .ok()
            .and_then(|s| s.hostname())
            .unwrap_or_default()
    } else {
        String::new()
    };
    let title = ["tab_title", "title", "window"]
        .iter()
        .find_map(|k| data.get(*k).filter(|v| ferb_py::value::truthy(v)).cloned());
    let title = match title {
        Some(Value::String(s)) => s,
        _ => String::new(),
    };
    let col = |k: &str| row.get(k).cloned().unwrap_or(Value::Null);
    let mut out = Map::new();
    out.insert("id".into(), col("id"));
    out.insert("at".into(), col("captured_at"));
    out.insert("kind".into(), col("kind"));
    out.insert("task_id".into(), col("task_id"));
    out.insert("app".into(), json!(field("app")));
    out.insert("bundle_id".into(), json!(field("bundle_id")));
    out.insert("site".into(), json!(site));
    out.insert("url".into(), json!(url));
    out.insert("title".into(), json!(title));
    out.insert("tool".into(), json!(field("tool")));
    out.insert("origin".into(), col("origin"));
    out
}

/// `_seen_excluded`: an image of an app or site the owner excludes now.
fn seen_excluded(h: &Host, item: &Map<String, Value>) -> Result<bool, HandlerError> {
    let ex = super::sensing::exclusions(h)?;
    let s = |k: &str| {
        item.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let url = s("url").unwrap_or("");
    Ok(ex.app(s("bundle_id"), s("app"))
        || ex.url(Some(url).filter(|u| !u.is_empty()))
        || url == ferb_sanitize::redact::EXCLUDED
        || item
            .get("origin")
            .map(ferb_py::value::py_str)
            .is_some_and(|o| o.contains("excluded")))
}

fn sql_value(v: rusqlite::types::ValueRef<'_>) -> Value {
    use rusqlite::types::ValueRef;
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => json!(i),
        ValueRef::Real(f) => json!(f),
        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
        ValueRef::Blob(b) => json!(hex::encode(b)),
    }
}

/// `_seen_rows`: stored images of the evidence window, newest first, that the exclusion list allows today.
fn seen_rows(
    h: &Host,
    db: &SharedDb,
    where_: &str,
    args: &[String],
    limit: usize,
) -> Result<(Vec<Map<String, Value>>, usize), HandlerError> {
    let days =
        ferb_store::retention::days(match h.get_or("retention", "evidence_days", json!(7))? {
            Value::Number(n) => n.as_i64().unwrap_or(7),
            _ => 7,
        });
    let cutoff = ferb_core::when::isoformat(&(h.now() - chrono::Duration::days(days)));
    let rows: Vec<BTreeMap<String, Value>> = {
        let db = db.lock().unwrap();
        let sql = format!(
            "SELECT * FROM evidence WHERE path IS NOT NULL AND mime LIKE 'image/%' AND captured_at >= ? \
             AND ({where_}) ORDER BY captured_at DESC, rowid DESC"
        );
        let mut stmt = db.conn().prepare(&sql).map_err(fault)?;
        let names: Vec<String> = stmt.column_names().map(|s| s.to_string()).collect();
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&cutoff];
        for a in args {
            params.push(a);
        }
        let mapped = stmt
            .query_map(params.as_slice(), |r| {
                let mut m = BTreeMap::new();
                for (i, n) in names.iter().enumerate() {
                    m.insert(n.clone(), sql_value(r.get_ref(i)?));
                }
                Ok(m)
            })
            .map_err(fault)?;
        mapped.collect::<Result<_, _>>().map_err(fault)?
    };
    let mut shown = vec![];
    for row in &rows {
        let item = seen_item(row);
        if !seen_excluded(h, &item)? {
            shown.push(item);
        }
    }
    let total = shown.len();
    shown.truncate(limit);
    Ok((shown, total))
}

/// `_seen_list`. ENG-6: a `limit` that isn't a whole number was Python's ValueError (an internal error); refused.
fn seen_list(h: &Host, d: &Deps, params: &Map<String, Value>) -> HandlerResult {
    let Some(db) = &d.store else {
        return Err(refusal("daemon not started"));
    };
    let p = P(params);
    let limit = match params.get("limit") {
        Some(v) if ferb_py::value::truthy(v) => match v {
            Value::Number(n) if n.as_i64().is_some() => n.as_i64().unwrap_or(300),
            Value::Number(n) => n.as_f64().unwrap_or(300.0) as i64,
            Value::Bool(_) => 1,
            Value::String(s) if ferb_py::value::strip(s).parse::<i64>().is_ok() => {
                ferb_py::value::strip(s).parse().unwrap_or(300)
            }
            _ => return Err(refusal("limit must be a number of screenshots")),
        },
        _ => 300,
    };
    let limit = limit.clamp(1, 2000) as usize;
    let (request, task) = (p.text_or_empty("request_id"), p.text_or_empty("task_id"));
    let (where_, args): (&str, Vec<String>) = if !request.is_empty() {
        (
            "task_id = ? OR task_id IN (SELECT id FROM tasks WHERE tree_id IN \
             (SELECT tree_id FROM tasks WHERE request_id = ?) OR request_id = ?)",
            vec![request.clone(), request.clone(), request],
        )
    } else if !task.is_empty() {
        let tree =
            (d.tree_of)(&task).unwrap_or_else(|| task.split('.').next().unwrap_or("").to_owned());
        (
            "task_id = ? OR (task_id >= ? || '.' AND task_id < ? || '/') OR \
             task_id IN (SELECT id FROM tasks WHERE tree_id = ?)",
            vec![tree.clone(), tree.clone(), tree.clone(), tree],
        )
    } else {
        ("1", vec![])
    };
    let (items, total) = seen_rows(h, db, where_, &args, limit)?;
    let days = match h.get("retention", "evidence_days")? {
        Some(v) if ferb_py::value::truthy(&v) => v.as_i64().unwrap_or(7),
        _ => 7,
    };
    ok(json!({"captures": items, "total": total, "days": days}))
}

/// `_seen_image`: one stored redacted screenshot's bytes (base64), as the model got it.
fn seen_image(h: &Host, d: &Deps, params: &Map<String, Value>) -> HandlerResult {
    let (Some(db), Some(evidence)) = (&d.store, &d.evidence) else {
        return Err(refusal("daemon not started"));
    };
    let eid = P(params).text_or_empty("id");
    let (items, _) = seen_rows(h, db, "id = ?", std::slice::from_ref(&eid), 1)?;
    let Some(item) = items.into_iter().next() else {
        return Err(refusal(
            "that screenshot isn't kept any more, or it shows something you exclude",
        ));
    };
    let row: Option<(Option<String>, Option<String>)> = db
        .lock()
        .unwrap()
        .conn()
        .query_row(
            "SELECT path, mime FROM evidence WHERE id = ?",
            [&eid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(fault)?;
    let path = row
        .as_ref()
        .and_then(|(p, _)| evidence.stored_file(p.as_deref()));
    let image = path.and_then(|p: PathBuf| std::fs::read(Path::new(&p)).ok());
    let Some(image) = image else {
        return Err(refusal("that screenshot isn't kept any more"));
    };
    let mime = row
        .and_then(|(_, m)| m)
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "image/jpeg".into());
    let mut out = item;
    out.insert("mime".into(), json!(mime));
    out.insert(
        "image".into(),
        json!(base64::engine::general_purpose::STANDARD.encode(image)),
    );
    Ok(out)
}
