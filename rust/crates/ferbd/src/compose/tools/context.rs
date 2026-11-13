//! The context readers and lookups (`registry.build`'s abilities, `add_trail`, `add_evidence`, `add_context_readers`,
//! compose.py's skills, secrets, about and conversations, `memory.learned`): each tool's run from the crate that
//! holds it, over this daemon's store and group A's memory.
//!
//! Ported here because no crate held them: the abilities catalog (`ferb/abilities`, its procedures read by path from
//! FERB's package folder, never copied), `memory.read` (`ferb/orchestrator/evidence.py` `MemoryRead`), `about.read`'s
//! run (`ferb/daemon/about.py` `AboutRead`). Fixture: rust/fixtures/ferbd/make_tools.py.

use std::collections::BTreeSet;
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use ferb_core::types::{ActionSpec, ToolResult};
use ferb_gateway::tool::{Args, Tool, ToolError};
use serde_json::{json, Map, Value};

use super::{Catalogued, Parts, Run};

fn read_spec() -> Arc<dyn Fn(&Args) -> ActionSpec + Send + Sync> {
    Arc::new(|_a| ActionSpec::new("read"))
}

fn tool(name: &'static str, run: Run) -> Arc<dyn Tool> {
    Arc::new(Catalogued {
        name,
        describe: read_spec(),
        run,
    })
}

fn obj(v: Value) -> Map<String, Value> {
    crate::basics::object(v)
}

/// A tool's answer (content, is_error, read sources) as its result.
pub fn answer(content: Value, is_error: bool, read_sources: Vec<(String, String)>) -> ToolResult {
    ToolResult {
        content: obj(content),
        is_error,
        read_sources,
        ..ToolResult::default()
    }
}

fn error(text: &str) -> ToolResult {
    answer(json!({"error": text}), true, Vec::new())
}

fn s(a: &Args, k: &str) -> Option<String> {
    match a.get(k) {
        Some(Value::String(v)) => Some(v.clone()),
        _ => None,
    }
}

fn s_or(a: &Args, k: &str) -> String {
    s(a, k).unwrap_or_default()
}

fn i(a: &Args, k: &str, d: i64) -> i64 {
    a.get(k).and_then(Value::as_i64).unwrap_or(d)
}

fn u(a: &Args, k: &str, d: usize) -> usize {
    a.get(k)
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(d)
}

fn b(a: &Args, k: &str) -> bool {
    a.get(k).and_then(Value::as_bool).unwrap_or(false)
}

/// An internal failure of a store read: Python's exception leaves the tool (outcome known: nothing changed).
fn failed(e: impl std::fmt::Display) -> ToolError {
    ToolError::failed("OperationalError", e.to_string())
}

// ---------------------------------------------------------------------------------------------------------------
// abilities (ferb/abilities: catalog.py, contracts.py, tools.py)
// ---------------------------------------------------------------------------------------------------------------

/// One packaged ability (`AbilityDefinition`); its procedure is `<package>/abilities/<id>/SKILL.md`.
pub struct Ability {
    pub id: &'static str,
    pub version: &'static str,
    pub description: &'static str,
    /// Its input model's `model_json_schema()`.
    pub input_schema: fn() -> Value,
    pub supported_interactions: &'static [&'static str],
    pub required_capabilities: &'static [&'static str],
    pub unavailable_reason: Option<&'static str>,
}

fn tab_schema(title: &str, description: &str) -> Value {
    json!({"additionalProperties": false,
           "properties": {"tab": {"description": description, "title": "Tab", "type": "string"}},
           "required": ["tab"], "title": title, "type": "object"})
}

/// `builtin_definitions()`, in its order.
pub const ABILITIES: [Ability; 2] = [
    Ability {
        id: "browser_verification",
        version: "1",
        description: "Browser verification: observe a visible check, use fresh evidence for its interaction, verify \
                      the website result, or preserve a precise owner handoff (browser_handoff). No one-call runner \
                      yet: read the procedure with abilities.read and follow it with the browser tools.",
        input_schema: || {
            tab_schema(
                "BrowserVerificationInput",
                "Existing task-owned tab in the authorized browser identity and profile.",
            )
        },
        supported_interactions: &[],
        required_capabilities: &[
            "browser.snapshot",
            "browser.screenshot",
            "browser.point",
            "browser.act",
            "browser.wait",
        ],
        unavailable_reason: Some(
            "The browser verification workflow and held-input lifecycle are not installed; the procedure is \
             available to read, but ability execution is unavailable.",
        ),
    },
    Ability {
        id: "google_docs",
        version: "1",
        description: "Google Docs, Sheets and Slides: how text gets into their canvas-drawn editors (paste with html \
                      at the caret, typing line by line, importing a file) and how to read it back (export \
                      addresses). No runner: read the procedure with abilities.read and follow it with the browser \
                      tools.",
        input_schema: || {
            tab_schema(
                "GoogleDocsInput",
                "Existing task-owned tab showing the Google Doc, Sheet or Slides file.",
            )
        },
        supported_interactions: &[],
        required_capabilities: &[
            "browser.snapshot",
            "browser.screenshot",
            "browser.point",
            "browser.act",
            "browser.open",
        ],
        unavailable_reason: Some(
            "A procedure to follow with the browser tools; there is no one-call runner.",
        ),
    },
];

pub const PROCEDURE_UNAVAILABLE: &str =
    "The packaged ability procedure is unavailable or is a symlink.";
pub const NOT_INSTALLED: &str =
    "Choose an installed ability ID from abilities.list; paths and modules are not accepted.";

fn open_nofollow(dir: Option<&OwnedFd>, name: &Path, directory: bool) -> Option<OwnedFd> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(name.as_os_str().as_bytes()).ok()?;
    let mut flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    if directory {
        flags |= libc::O_DIRECTORY;
    }
    // SAFETY: `c` is NUL-terminated and lives across the call; `dir` is an open descriptor we own.
    let fd = unsafe {
        match dir {
            Some(d) => libc::openat(d.as_raw_fd(), c.as_ptr(), flags),
            None => libc::open(c.as_ptr(), flags),
        }
    };
    // SAFETY: a descriptor just opened and owned by nothing else.
    (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })
}

/// `_open_procedure`: the package folder opened without following a link, then the file in it the same way; the
/// text as Python's text mode reads it (universal newlines).
pub fn read_procedure(package: &Path, id: &str) -> Result<String, String> {
    let dir = open_nofollow(None, &package.join("abilities").join(id), true)
        .ok_or_else(|| PROCEDURE_UNAVAILABLE.to_string())?;
    let fd = open_nofollow(Some(&dir), Path::new("SKILL.md"), false)
        .ok_or_else(|| PROCEDURE_UNAVAILABLE.to_string())?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::fs::File::from(fd), &mut bytes)
        .map_err(|_| PROCEDURE_UNAVAILABLE.to_string())?;
    let text = String::from_utf8(bytes).map_err(|_| PROCEDURE_UNAVAILABLE.to_string())?;
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// `AbilityCatalog._metadata`.
pub fn ability_metadata(
    a: &Ability,
    package: &Path,
    installed: &BTreeSet<String>,
) -> Map<String, Value> {
    let missing: Vec<&str> = a
        .required_capabilities
        .iter()
        .copied()
        .filter(|c| !installed.contains(*c))
        .collect();
    let mut reasons: Vec<String> = a.unavailable_reason.iter().map(|r| r.to_string()).collect();
    if !missing.is_empty() {
        reasons.push(format!(
            "Required capabilities are not registered: {}.",
            missing.join(", ")
        ));
    }
    if let Err(e) = read_procedure(package, a.id) {
        reasons.push(e);
    }
    obj(
        json!({"id": a.id, "version": a.version, "description": a.description, "source_kind": "builtin",
               "origin": "ferb", "supported_interactions": a.supported_interactions,
               "required_capabilities": a.required_capabilities,
               "availability": {"available": reasons.is_empty(), "missing_capabilities": missing,
                                "reason": if reasons.is_empty() { Value::Null } else { json!(reasons.join(" ")) }}}),
    )
}

/// `registry.abilities.catalog()`: every ability's metadata, as every role's instructions list them.
pub fn abilities_catalog(package: &Path, installed: &BTreeSet<String>) -> Vec<Value> {
    ABILITIES
        .iter()
        .map(|a| Value::Object(ability_metadata(a, package, installed)))
        .collect()
}

/// `AbilityCatalog.catalog()`.
pub fn abilities_list(package: &Path, installed: &BTreeSet<String>) -> ToolResult {
    let rows: Vec<Value> = ABILITIES
        .iter()
        .map(|a| Value::Object(ability_metadata(a, package, installed)))
        .collect();
    answer(json!({"abilities": rows}), false, Vec::new())
}

/// `AbilityCatalog.read(name)`: a refusal for a name that isn't installed or a procedure that can't be read.
pub fn abilities_read(
    package: &Path,
    installed: &BTreeSet<String>,
    name: &str,
) -> Result<ToolResult, ToolError> {
    let a = ABILITIES
        .iter()
        .find(|a| a.id == name)
        .ok_or_else(|| ToolError::refused(NOT_INSTALLED))?;
    let body = read_procedure(package, a.id).map_err(ToolError::refused)?;
    let mut out = ability_metadata(a, package, installed);
    out.insert("input_schema".into(), (a.input_schema)());
    out.insert("body".into(), json!(body));
    Ok(answer(Value::Object(out), false, Vec::new()))
}

fn installed(p: &Parts) -> BTreeSet<String> {
    p.gateway()
        .map(|g| g.registry().names().into_iter().collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------------------------
// about.read (ferb/daemon/about.py AboutRead)
// ---------------------------------------------------------------------------------------------------------------

/// `AboutRead.run` over the document as the About area builds it.
pub fn about_read(
    section: Option<&str>,
    document: Result<Map<String, Value>, String>,
) -> Result<ToolResult, ToolError> {
    use ferb_daemon::areas_b::about::SECTIONS;
    if let Some(sec) = section {
        if !SECTIONS.contains(&sec) {
            return Ok(answer(
                json!({"error": format!("section must be one of: {}", SECTIONS.join(", "))}),
                true,
                Vec::new(),
            ));
        }
    }
    let doc = document.map_err(|e| ToolError::failed("FerbError", e))?;
    Ok(match section {
        Some(sec) => {
            let mut m = Map::new();
            m.insert(sec.into(), doc.get(sec).cloned().unwrap_or(Value::Null));
            answer(Value::Object(m), false, Vec::new())
        }
        None => answer(Value::Object(doc), false, Vec::new()),
    })
}

// ---------------------------------------------------------------------------------------------------------------
// memory.read (ferb/orchestrator/evidence.py MemoryRead), on group A's thread
// ---------------------------------------------------------------------------------------------------------------

/// MemoryRead's arguments.
#[derive(Clone, Debug, Default)]
pub struct MemoryArgs {
    pub query: String,
    pub item_id: Option<String>,
    pub forgotten: bool,
    pub after: usize,
    pub limit: usize,
    pub offset: usize,
    pub max_chars: usize,
}

impl MemoryArgs {
    pub fn of(a: &Args) -> MemoryArgs {
        MemoryArgs {
            query: s_or(a, "query"),
            item_id: s(a, "item_id"),
            forgotten: b(a, "forgotten"),
            after: u(a, "after", 0),
            limit: u(a, "limit", 20),
            offset: u(a, "offset", 0),
            max_chars: u(a, "max_chars", 12000),
        }
    }
}

/// Python's `str(datetime)`: `isoformat(sep=" ")`.
fn dt_str(d: &ferb_learning::DateTime) -> String {
    ferb_learning::py::iso(d).replacen('T', " ", 1)
}

/// `dataclasses.asdict(item)` as `json.dumps(..., default=str)` writes it: times as `str(datetime)`.
fn item_dumped(it: &ferb_learning::items::Item) -> Value {
    let t = |d: &Option<ferb_learning::DateTime>| d.as_ref().map(dt_str);
    json!({"id": it.id, "kind": it.kind, "subject": it.subject, "key": it.key, "content": it.content,
           "source_ids": it.source_ids, "observed_at": dt_str(&it.observed_at),
           "valid_from": dt_str(&it.valid_from), "valid_to": t(&it.valid_to),
           "supersedes": it.supersedes, "confidence": it.confidence, "origin": it.origin,
           "status": it.status, "about": it.about, "review_at": t(&it.review_at),
           "set_aside": it.set_aside})
}

fn passed(origins: &BTreeSet<String>) -> Vec<(String, String)> {
    origins
        .iter()
        .map(|o| ("origin".to_string(), o.clone()))
        .collect()
}

/// `_PagedFacts.page`: the value's JSON text, a window of it.
fn paged(value: &Value, offset: usize, size: usize, origins: &BTreeSet<String>) -> ToolResult {
    let text = ferb_py::json::dumps_utf8(value);
    let chars: Vec<char> = text.chars().collect();
    let end = (offset + size).min(chars.len());
    let shown: String = if offset >= end {
        String::new()
    } else {
        chars[offset..end].iter().collect()
    };
    answer(
        json!({"text": shown, "offset": offset,
               "next_offset": if end < chars.len() { json!(end) } else { Value::Null }, "total_chars": chars.len()}),
        false,
        passed(origins),
    )
}

/// `_listed_status`.
fn listed_status(it: &ferb_learning::items::Item, now: &ferb_learning::DateTime) -> String {
    if it.set_aside {
        return "set aside".into();
    }
    match &it.valid_to {
        // `end <= now`
        Some(end)
            if it.status == "active" && !ferb_learning::py::after(end, now).unwrap_or(true) =>
        {
            "ended".into()
        }
        _ => it.status.clone(),
    }
}

/// What memory.read reads on group A's thread.
pub struct MemorySide<'a> {
    pub memory: &'a ferb_learning::items::Memory,
    pub learning: &'a ferb_learning::learning::Learning,
    /// `_memory_item_search(query)`: item ids, best first.
    pub search: &'a dyn Fn(&str) -> Result<Vec<String>, String>,
}

impl MemorySide<'_> {
    /// `_origins`: `learning._source_origins("memory:" + id)`.
    fn origins(&self, item_id: &str) -> Result<BTreeSet<String>, ToolError> {
        self.learning
            .labels(&[format!("memory:{item_id}")])
            .map_err(|e| ToolError::failed(e.class(), e.to_string()))
    }
}

/// `MemoryRead.run`.
pub fn memory_read(m: &MemorySide<'_>, a: &MemoryArgs) -> Result<ToolResult, ToolError> {
    let lerr = |e: ferb_learning::Error| ToolError::failed(e.class(), e.to_string());
    if a.forgotten {
        let forgotten = ferb_learning::items::forgets_on_record(&m.memory.store).map_err(lerr)?;
        return Ok(answer(
            json!({"forgotten": forgotten, "note": "a forgotten item's contents are not kept, so FERB can't say what it was"}),
            false,
            Vec::new(),
        ));
    }
    if let Some(id) = a.item_id.as_deref().filter(|i| !i.is_empty()) {
        let Some(item) = m.memory.get(id).map_err(lerr)? else {
            return Ok(error("no memory item with that id"));
        };
        let mut r = paged(
            &item_dumped(&item),
            a.offset,
            a.max_chars,
            &m.origins(&item.id)?,
        );
        r.content
            .insert("source_refs".into(), json!([format!("memory:{}", item.id)]));
        return Ok(r);
    }
    let now = m.memory.clock.now();
    let items: Vec<ferb_learning::items::Item> = if !a.query.is_empty() {
        if ferb_memory::terms::query_terms(&a.query).is_empty() {
            return Ok(error(
                "memory_read needs words to look for; this query has none",
            ));
        }
        let ids = (m.search)(&a.query).map_err(|e| ToolError::failed("OperationalError", e))?;
        let mut out = Vec::new();
        for id in ids {
            if let Some(it) = m.memory.get(&id).map_err(lerr)? {
                if ferb_learning::items::prompt_ready(&it, &now).map_err(lerr)? {
                    out.push(it);
                }
            }
        }
        out
    } else {
        m.memory.current_items().map_err(lerr)?
    };
    let end = a.after + a.limit;
    let shown: Vec<&ferb_learning::items::Item> =
        items.iter().skip(a.after).take(a.limit).collect();
    let mut origins = BTreeSet::new();
    let mut selected = Vec::new();
    for it in &shown {
        origins.extend(m.origins(&it.id)?);
        let mut row = Map::new();
        for (k, v) in [
            ("id", json!(it.id)),
            ("kind", json!(it.kind)),
            ("subject", json!(it.subject)),
            ("key", json!(it.key)),
            ("origin", json!(it.origin)),
            (
                "label",
                json!(ferb_learning::items::origin_label(&it.origin)),
            ),
        ] {
            row.insert(k.into(), v);
        }
        if !it.about.is_empty() {
            row.insert("about".into(), json!(it.about));
        }
        let preview: String = it.content.chars().take(500).collect();
        row.insert("status".into(), json!(listed_status(it, &now)));
        row.insert(
            "valid_to".into(),
            json!(it.valid_to.as_ref().map(ferb_learning::py::iso)),
        );
        row.insert("preview".into(), json!(preview));
        row.insert(
            "preview_truncated".into(),
            json!(it.content.chars().count() > 500),
        );
        row.insert(
            "read_full".into(),
            json!({"tool": "memory.read", "item_id": it.id}),
        );
        selected.push(Value::Object(row));
    }
    let refs: Vec<String> = shown.iter().map(|it| format!("memory:{}", it.id)).collect();
    Ok(answer(
        json!({"items": selected, "next_after": if end < items.len() { json!(end) } else { Value::Null },
               "total_items": items.len(), "source_refs": refs}),
        false,
        passed(&origins),
    ))
}

/// `MEMORY_LEARNED_MOST`: a query ranks at most this many.
pub const MEMORY_LEARNED_MOST: usize = 100;

struct SearchClock<'a>(&'a dyn ferb_learning::Clock);
impl ferb_memory::search::Clock for SearchClock<'_> {
    fn now(&self) -> ferb_memory::times::DateTime {
        self.0.now()
    }
}

/// `_memory_item_search`: the facts ranked by the one search (keywords and recency: no embedding vector is read in
/// this build, as Python's when the model isn't loaded yet).
pub fn memory_item_search(
    memory: &ferb_learning::items::Memory,
    query: &str,
) -> Result<Vec<String>, String> {
    let db = memory.store.db.clone();
    let clock = SearchClock(memory.clock.as_ref());
    let search = ferb_memory::search::MemorySearch::new(db.conn(), &clock, None);
    let facts = ferb_memory::search::StoreFacts(db.conn());
    search
        .facts(&facts, query, None, "", Some(MEMORY_LEARNED_MOST), &[])
        .map(|hits| {
            hits.into_iter()
                .map(|h| h.doc.strip_prefix("memory:").unwrap_or(&h.doc).to_string())
                .collect()
        })
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------------------------------------------
// the tools
// ---------------------------------------------------------------------------------------------------------------

type GroupA = ferb_daemon::shared::Local<ferb_daemon::areas_a::State>;

fn group_a(p: &Parts) -> Result<GroupA, ToolError> {
    p.group_a()
        .ok_or_else(|| ToolError::failed("FerbError", "daemon not started"))
}

fn gone(e: impl std::fmt::Display) -> ToolError {
    ToolError::failed("FerbError", e.to_string())
}

fn spans<'a>(db: &'a ferb_store::Db, p: &Parts) -> ferb_store::spans::SpanStore<'a> {
    let r = p.redactor();
    ferb_store::spans::SpanStore::new(
        db,
        Box::new(ferb_gateway::gateway::StoreTime(p.clock())),
        Box::new(ferb_gateway::gateway::StoreIds(p.ids())),
        Box::new(move |v: &Value| r.value(v)),
        Box::new(|kind: &str| ferb_policy::rules::REVERSIBLE.contains(&kind)),
    )
}

fn continuity<'a>(
    db: &'a ferb_store::Db,
    p: &Parts,
) -> ferb_orchestrator::continuity::Continuity<'a> {
    let r = p.redactor();
    let (clock, ids, r2) = (p.clock(), p.ids(), p.redactor());
    let spans_db = p.home().join("ferb.db");
    ferb_orchestrator::continuity::Continuity::new(
        db,
        Arc::new(move |v: &Value| r.value(v)),
        Arc::new(move |root: &str| {
            // SpanStore.task_origins on a connection of its own (the continuity's borrows this one)
            let Ok(db) = ferb_store::Db::open(&spans_db) else {
                return BTreeSet::new();
            };
            let r = r2.clone();
            let s = ferb_store::spans::SpanStore::new(
                &db,
                Box::new(ferb_gateway::gateway::StoreTime(clock.clone())),
                Box::new(ferb_gateway::gateway::StoreIds(ids.clone())),
                Box::new(move |v: &Value| r.value(v)),
                Box::new(|_k: &str| false),
            );
            s.task_origins(root).unwrap_or_default()
        }),
        Arc::new(|kind: &str, data: &Value| ferb_learning::journal::journal_origin(kind, data)),
        Arc::new(|| ferb_runtimes::codex::token_hex(6)),
    )
}

fn task_root(p: &Parts, task_id: &str) -> Option<String> {
    p.task(task_id).map(|t| t.tree_id.unwrap_or(t.task_id))
}

/// `add_trail`, `task.evidence`, the context readers, secrets.list, about.read, conversation.read, memory.learned,
/// and build()'s abilities: by name.
pub fn tool_named(p: &Parts, name: &'static str) -> Arc<dyn Tool> {
    let parts = p.clone();
    let run: Run = match name {
        "abilities.list" => Arc::new(move |_a, _c| {
            let p = parts.clone();
            Box::pin(async move { Ok(abilities_list(p.package(), &installed(&p)).into()) })
        }),
        "abilities.read" => Arc::new(move |a, _c| {
            let p = parts.clone();
            let name = s_or(a, "name");
            Box::pin(
                async move { abilities_read(p.package(), &installed(&p), &name).map(Into::into) },
            )
        }),
        "skills.read" => Arc::new(move |a, _c| {
            let p = parts.clone();
            let (skill, file) = (
                s_or(a, "skill"),
                s(a, "file").unwrap_or_else(|| "SKILL.md".into()),
            );
            Box::pin(async move {
                let skills = ferb_orchestrator::skills::Skills {
                    directory: p.home().join("skills").to_string_lossy().into_owned(),
                };
                let text = skills
                    .read_file(&skill, &file)
                    .map_err(ToolError::refused)?;
                let (fields, body) = if file == "SKILL.md" {
                    ferb_orchestrator::skills::Skills::parse(&text)
                } else {
                    (Vec::new(), text)
                };
                let mut out = Map::new();
                out.insert("id".into(), json!(skill));
                out.insert("file".into(), json!(file));
                for (k, v) in fields {
                    out.insert(k, json!(v));
                }
                out.insert("body".into(), json!(body));
                Ok(answer(Value::Object(out), false, Vec::new()).into())
            })
        }),
        "about.read" => Arc::new(move |a, _c| {
            let p = parts.clone();
            let section = s(a, "section");
            Box::pin(async move {
                if let Some(sec) = &section {
                    if !ferb_daemon::areas_b::about::SECTIONS.contains(&sec.as_str()) {
                        return about_read(Some(sec), Ok(Map::new())).map(Into::into);
                    }
                }
                let doc = match p.about_document() {
                    Some(f) => tokio::task::spawn_blocking(move || f())
                        .await
                        .unwrap_or_else(|e| Err(e.to_string())),
                    None => Err("daemon not started".into()),
                };
                about_read(section.as_deref(), doc).map(Into::into)
            })
        }),
        "trail.query" | "trail.days" => Arc::new(move |a, _c| {
            let p = parts.clone();
            let a = a.clone();
            Box::pin(async move {
                tokio::task::spawn_blocking(move || trail_run(&p, name, &a))
                    .await
                    .map_err(gone)?
            })
        }),
        "task.evidence" => Arc::new(move |a, c| {
            let p = parts.clone();
            let (a, caller) = (a.clone(), c.task_id.clone());
            Box::pin(async move { evidence_run(&p, &a, &caller) })
        }),
        "chat.history" => Arc::new(move |a, c| {
            let p = parts.clone();
            let (a, task) = (a.clone(), c.task_id.clone());
            Box::pin(async move {
                // its store borrows aren't Send: on a thread of its own, driven by this runtime
                super::mac::off_thread(move || {
                    Box::pin(async move { chat_history_run(&p, &a, &task).await })
                })
                .await
                .ok_or_else(|| gone("the reader's thread ended"))?
            })
        }),
        "memory.read" => Arc::new(move |a, _c| {
            let p = parts.clone();
            let args = MemoryArgs::of(a);
            Box::pin(async move {
                let ga = group_a(&p)?;
                ga.call(move |st: Rc<ferb_daemon::areas_a::State>| async move {
                    let search = |q: &str| memory_item_search(&st.memory, q);
                    memory_read(
                        &MemorySide {
                            memory: &st.memory,
                            learning: &st.learning,
                            search: &search,
                        },
                        &args,
                    )
                })
                .await
                .map_err(gone)?
                .map(Into::into)
            })
        }),
        "memory.learned" => Arc::new(move |a, _c| {
            let p = parts.clone();
            let args = ferb_learning::tools::Args {
                query: s_or(a, "query"),
                item_id: s(a, "item_id"),
                kind: s(a, "kind"),
                app: s(a, "app"),
                person: s(a, "person"),
                since: s(a, "since"),
                until: s(a, "until"),
                offset: u(a, "offset", 0),
                max_chars: u(a, "max_chars", 12000),
            };
            Box::pin(async move {
                let ga = group_a(&p)?;
                ga.call(move |st: Rc<ferb_daemon::areas_a::State>| async move {
                    ferb_learning::tools::run(Some(&st.learning), &args, None)
                        .map_err(|e| ToolError::failed(e.class(), e.to_string()))
                })
                .await
                .map_err(gone)?
                .map(|o| answer(o.content, o.is_error, o.read_sources).into())
            })
        }),
        "secrets.list" => Arc::new(move |a, _c| {
            let p = parts.clone();
            let place = s_or(a, "place");
            Box::pin(async move {
                let Some(store) = p.owner_secrets() else {
                    return Err(ToolError::failed("FerbError", "daemon not started"));
                };
                let ga = p.group_a();
                let r = tokio::task::spawn_blocking(move || {
                    // `_username_sources`: the memory items a username may still come from (L36 §6)
                    let current: Option<BTreeSet<String>> = ga.and_then(|g| {
                        g.call_blocking(|st: Rc<ferb_daemon::areas_a::State>| async move {
                            st.memory.current_ids(Some("owner_stated")).ok()
                        })
                        .ok()
                        .flatten()
                    });
                    let holding = |source: &str| -> bool {
                        match (&current, source.strip_prefix("memory:")) {
                            (Some(ids), Some(id)) => ids.contains(id),
                            _ => false,
                        }
                    };
                    let holding: ferb_identity::owner_secrets::Holding<'_> = if current.is_some() {
                        Some(&holding)
                    } else {
                        None
                    };
                    // the task's owner-typed sites (`HiddenInput._question_sites`) are the gateway's and not public in
                    // this build: none are listed (seam mismatch, listed)
                    ferb_identity::secret_tools::secrets_list(
                        &store,
                        &place,
                        &|_| Vec::new(),
                        holding,
                    )
                })
                .await
                .map_err(gone)?;
                Ok(answer(
                    r.content,
                    r.is_error,
                    if r.is_error {
                        Vec::new()
                    } else {
                        vec![
                            ("origin".into(), "private".into()),
                            ("origin".into(), "inferred".into()),
                        ]
                    },
                )
                .into())
            })
        }),
        "conversation.read" => Arc::new(move |a, c| {
            let p = parts.clone();
            let args = ferb_orchestrator::conversation_tools::ReadArgs {
                conversation_id: s(a, "conversation_id"),
                offset: u(a, "offset", 0),
                max_chars: u(a, "max_chars", 12000),
            };
            let task = c.task_id.clone();
            Box::pin(async move {
                let db = p.db();
                let d = crate::basics::lock(&db);
                let r = p.redactor();
                let store = ferb_orchestrator::conversations::ConversationStore::new(
                    &d,
                    Arc::new(String::new),
                    Arc::new(move |v: &Value| r.value(v)),
                    Arc::new(|_t: &str, _d: Value| {}),
                );
                let now = String::new;
                let tree = ferb_orchestrator::tree::TaskTree::new(Some(&d), Box::new(now));
                let root = tree.tree_of(&task).ok().flatten();
                let ans = ferb_orchestrator::conversation_tools::conversation_read(
                    &store, root, &task, &args,
                )
                .map_err(failed)?;
                Ok(answer(ans.content, ans.is_error, ans.read_sources).into())
            })
        }),
        other => unreachable!("not a context reader: {other}"),
    };
    tool(name, run)
}

fn trail_run(p: &Parts, name: &str, a: &Args) -> Result<ferb_gateway::tool::ToolOutput, ToolError> {
    let db = ferb_store::Db::open(p.home().join("ferb.db")).map_err(failed)?;
    let ids = super::IdsRef(p.ids());
    let r = p.redactor();
    let zoned = p.zoned();
    let seams = ferb_sense::trail::Seams {
        exclusions: Box::new(ferb_sense::exclusions::Exclusions::defaults),
        publish: Box::new(|_, _| {}),
        device_id: Box::new(String::new),
        paused: Box::new(|| false),
        test_task: Box::new(|_| false),
        is_browser: Box::new(ferb_mac_hands::apps::known),
    };
    let trail = ferb_sense::trail::Trail::new(
        &db,
        zoned.as_ref(),
        &ids,
        r.as_ref(),
        seams,
        Arc::new(ferb_sense::attribution::Attribution::new(|| 0.0)),
    );
    let terr = |e: ferb_sense::trail::Error| match e {
        ferb_sense::trail::Error::Value(m) => ToolError::failed("ValueError", m),
        other => ToolError::failed("OperationalError", other.to_string()),
    };
    if name == "trail.query" {
        let q = ferb_sense::tools::QueryInput {
            source: s(a, "source"),
            event_id: s(a, "event_id"),
            since_hours: a.get("since_hours").and_then(Value::as_f64).unwrap_or(24.0),
            since: s(a, "since"),
            until: s(a, "until"),
            host: s(a, "host"),
            actor: s(a, "actor"),
            limit: i(a, "limit", 100),
            before_seq: a.get("before_seq").and_then(Value::as_i64),
            text: s(a, "text"),
        };
        return ferb_sense::tools::trail_query(Some(&trail), &q)
            .map(Into::into)
            .map_err(terr);
    }
    let d = ferb_sense::tools::DaysInput {
        text: s(a, "text"),
        since_days: i(a, "since_days", 30),
        limit: i(a, "limit", 10),
    };
    let store = ferb_learning::store::Store::new(Rc::new(
        ferb_store::Db::open(p.home().join("ferb.db")).map_err(failed)?,
    ));
    let zone = Zone(p.zoned().zone());
    let day_refs = |day: &str| -> Vec<String> {
        match ferb_learning::summary::build(&store, &SummarySeams, &zone, day) {
            Ok(ev) => ev
                .handles
                .iter()
                .flat_map(|(_, sources)| sources.iter().map(|(k, id)| format!("{k}:{id}")))
                .collect(),
            Err(_) => Vec::new(),
        }
    };
    ferb_sense::tools::trail_days(Some(&trail), &d, &day_refs)
        .map(Into::into)
        .map_err(terr)
}

/// The Mac's zone as the memory part reads offsets.
pub struct Zone(pub ferb_clock::zone::Zone);

impl ferb_memory::times::LocalZone for Zone {
    fn offset_at(&self, unix_seconds: i64) -> i32 {
        let utc = chrono::DateTime::from_timestamp(unix_seconds, 0)
            .unwrap_or_default()
            .fixed_offset();
        self.0.local(&utc).offset().local_minus_utc()
    }
}

/// summary.py's other parts: test runs and the day's security digest.
pub struct SummarySeams;

impl ferb_learning::summary::Seams for SummarySeams {
    fn test_task(
        &self,
        store: &ferb_learning::store::Store,
        task_id: &str,
    ) -> ferb_learning::Result<bool> {
        ferb_orchestrator::test_runs::TestRuns::new(&store.db)
            .task(Some(task_id))
            .map_err(|e| ferb_learning::Error::Sql("OperationalError".into(), e.to_string()))
    }

    fn security_digest(
        &self,
        store: &ferb_learning::store::Store,
        day: &str,
    ) -> ferb_learning::Result<Value> {
        ferb_policy::stores::security_digest(&store.db, day).map_err(ferb_learning::Error::from)
    }
}

/// A task row as `_inspection_parent` reads it: (parent, role, request, identity, scopes).
type TaskRow = (
    Option<String>,
    String,
    String,
    String,
    std::collections::BTreeSet<String>,
);

/// environment.py `_inspection_parent`: a registered checker (role `verifier`) borrows its worker's tabs and evidence
/// when its parent is a registered worker of the same request, identity and tree, holding at least its scopes. Both
/// live records are the gateway's; their roles, scopes and request are the task rows the supervisor wrote.
fn inspection_parent(p: &Parts, db: &ferb_store::Db, task_id: &str) -> Option<String> {
    let child = p.task(task_id)?;
    let row = |id: &str| -> Option<TaskRow> {
        db.conn()
            .query_row(
                "SELECT parent_id, role, request_id, identity, scopes_json FROM tasks WHERE id = ?",
                [id],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            )
            .ok()
            .map(|(parent, role, request, identity, scopes)| {
                let scopes: std::collections::BTreeSet<String> =
                    serde_json::from_str(&scopes).unwrap_or_default();
                (parent, role, request, identity, scopes)
            })
    };
    let (parent_id, role, request, identity, scopes) = row(task_id)?;
    if role != "verifier" {
        return None;
    }
    let parent_id = parent_id?;
    let parent = p.task(&parent_id)?;
    let (_, prole, prequest, pidentity, pscopes) = row(&parent_id)?;
    if prole != "worker" || !scopes.is_subset(&pscopes) {
        return None;
    }
    ((request, identity, child.tree_id) == (prequest, pidentity, parent.tree_id))
        .then_some(parent_id)
}

fn evidence_run(
    p: &Parts,
    a: &Args,
    caller: &str,
) -> Result<ferb_gateway::tool::ToolOutput, ToolError> {
    let args = ferb_orchestrator::evidence::EvidenceArgs {
        query: s_or(a, "query"),
        kind: s_or(a, "kind"),
        after: i(a, "after", 0),
        limit: i(a, "limit", 20),
        span_id: s(a, "span_id"),
        span_ids: a.get("span_ids").and_then(Value::as_array).map(|l| {
            l.iter()
                .map(|v| {
                    v.as_str()
                        .map(String::from)
                        .unwrap_or_else(|| ferb_py::value::py_str(v))
                })
                .collect()
        }),
        offset: u(a, "offset", 0),
        max_chars: u(a, "max_chars", 12000),
        evidence_id: s(a, "evidence_id"),
        task_id: s(a, "task_id"),
    };
    let db = p.db();
    let d = crate::basics::lock(&db);
    let spans = spans(&d, p);
    let roots = p.clone();
    let readable_parts = p.clone();
    let captures_home = p.home().join("evidence");
    let captures_db = p.home().join("ferb.db");
    let r = p.redactor();
    let ev = ferb_orchestrator::evidence::TaskEvidence {
        spans: &spans,
        // `_inspection_parent`: a registered checker borrowing its worker's evidence (the caller's, read now)
        parent: {
            let (me, parent) = (caller.to_string(), inspection_parent(p, &d, caller));
            Arc::new(move |t: &str| if t == me { parent.clone() } else { None })
        },
        root: Arc::new(move |t: &str| roots.task(t).and_then(|rec| rec.tree_id)),
        captures: Some(Arc::new(move |task: &str| {
            let Ok(db) = ferb_store::Db::open(&captures_db) else {
                return Vec::new();
            };
            let r = r.clone();
            let Ok(store) = ferb_mac_hands::evidence::EvidenceStore::new(
                &captures_home,
                Arc::new(|| (String::new(), String::new())),
                Arc::new(String::new),
                Arc::new(move |v: &Value| r.value(v)),
                Arc::new(super::mac::NoPaint),
            ) else {
                return Vec::new();
            };
            store
                .list(db.conn(), task)
                .unwrap_or_default()
                .into_iter()
                .map(obj)
                .collect()
        })),
        secrets: {
            let r = p.redactor();
            Arc::new(move || r.secret_values().iter().map(str::to_string).collect())
        },
        readable: Arc::new(move |t: &str| conversation_tasks(&readable_parts, t)),
        record_sources: Arc::new(ferb_learning::lineage::record_sources),
    };
    let ans = ev.run(&args, caller).map_err(failed)?;
    Ok(answer(ans.content, ans.is_error, ans.read_sources).into())
}

/// `_conversation_tasks`: the tasks of the conversation a running task belongs to (from its live record's tree and
/// the stored chat); none for a task without a live record.
fn conversation_tasks(p: &Parts, task_id: &str) -> BTreeSet<String> {
    let Some(rec) = p.task(task_id) else {
        return BTreeSet::new();
    };
    let Ok(db) = ferb_store::Db::open(p.home().join("ferb.db")) else {
        return BTreeSet::new();
    };
    let tree = rec.tree_id.unwrap_or_else(|| task_id.to_string());
    continuity(&db, p).readable_tasks(&tree).unwrap_or_default()
}

async fn chat_history_run(
    p: &Parts,
    a: &Args,
    task: &str,
) -> Result<ferb_gateway::tool::ToolOutput, ToolError> {
    let args = ferb_orchestrator::evidence::HistoryArgs {
        query: s_or(a, "query"),
        since: s(a, "since"),
        until: s(a, "until"),
        entry_id: s(a, "entry_id"),
        task_id: s(a, "task_id"),
        include_tasks: b(a, "include_tasks"),
        after: i(a, "after", 0),
        limit: i(a, "limit", 20),
        offset: u(a, "offset", 0),
        max_chars: u(a, "max_chars", 12000),
    };
    let root = task_root(p, task);
    let sp = p.clone();
    // `_chat_search`: the conversation's entries by keywords (no embedding vector in this build), only its own
    let search: ferb_orchestrator::evidence::Search =
        Arc::new(move |root, query, since, until, limit| {
            let p = sp.clone();
            Box::pin(async move { chat_search(&p, &root, &query, since, until, limit) })
        });
    let home = p.home().join("ferb.db");
    let db = ferb_store::Db::open(&home).map_err(failed)?;
    let c = continuity(&db, p);
    let ans = ferb_orchestrator::evidence::chat_history(&c, root, Some(&search), &args)
        .await
        .map_err(failed)?;
    Ok(answer(ans.content, ans.is_error, ans.read_sources).into())
}

fn chat_search(
    p: &Parts,
    root: &str,
    query: &str,
    since: Option<String>,
    until: Option<String>,
    limit: i64,
) -> Vec<String> {
    let Ok(db) = ferb_store::Db::open(p.home().join("ferb.db")) else {
        return Vec::new();
    };
    let c = continuity(&db, p);
    let Ok(Some(conversation)) = c.conversation_of(root) else {
        return Vec::new();
    };
    let schedules = c.reaches_schedules(root);
    struct Now(Arc<dyn ferb_core::interfaces::Clock>);
    impl ferb_memory::search::Clock for Now {
        fn now(&self) -> ferb_memory::times::DateTime {
            ferb_memory::times::DateTime::from_isoformat(&ferb_core::when::isoformat(&self.0.now()))
                .expect("an ISO time")
        }
    }
    let clock = Now(p.clock());
    let search = ferb_memory::search::MemorySearch::new(db.conn(), &clock, None);
    let accept = |row: &ferb_memory::search::ChatRow| {
        c.belongs(
            &conversation,
            &row.kind,
            &row.session,
            &row.request,
            &row.doc,
            schedules,
        )
        .unwrap_or(false)
    };
    search
        .chat(
            query,
            None,
            limit.max(0) as usize,
            &ferb_memory::search::CHAT_KINDS,
            &accept,
            &[],
            since.as_deref(),
            until.as_deref(),
        )
        .map(|hits| {
            hits.into_iter()
                .map(|h| h.doc.strip_prefix("journal:").unwrap_or(&h.doc).to_string())
                .collect()
        })
        .unwrap_or_default()
}
