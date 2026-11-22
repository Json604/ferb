//! `import_job.py` (MEM-400..415): one memory import as a background job: its estimate, run record, progress,
//! resume, cancel and the one summary notice. A run reads its source again each start, skips the conversation parts
//! already read (`memory_import_done`), and works chunk by chunk, three at once (IMP-21): wait while the owner's chat
//! turn runs, recall, verify, keep the checked facts with where each was said (`memory_import_facts`), then record the
//! chunk's parts done. The facts are then placed into the profile (`import_profile`, IMP-20, IMP-31): only the
//! profile's lines become memory items. An export is checked around the reader and an older one sends nothing
//! (MEM-438); an export FERB fetched itself is deleted once its import is done (IMP-32).
//!
//! Fixed against Python (ENG-6): `import_job.py:186-190` saved a chunk's facts, its source journal row, its done
//! marks and the run's progress as separate writes, so a crash between them lost or doubled work (a resumed run
//! credited the facts it had already saved, or found a run marked done without its closing journal row). Here each
//! chunk's writes, and a run's closing writes, commit together.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde_json::{json, Map, Value};

use ferb_context::PromptRoot;
use ferb_core::interfaces::BoxFuture;
use ferb_memory::import_extract::{self as ex, Candidate, Chunk, OwnerIndex, Part, Verified};
use ferb_memory::import_sources::{self, source_name, Conversation, Unzstd, EXPORTS, SOURCES};
use ferb_memory::pystr;
use ferb_memory::terms::{slug, subject_key, Fact};
use ferb_memory::times::{self, Date, LocalZone};
use ferb_sanitize::origin;

use crate::calls::{self, learning_call, MakeRuntime, Spans};
use crate::distill::Notice;
use crate::items::{Item, Memory};
use crate::journal::Journal;
use crate::py;
use crate::sql;
use crate::store::Store;
use crate::{Error, Ids, Result};

pub const SOURCE_KIND: &str = "memory.import";
pub const FINISHED_KIND: &str = "memory.import_finished";
pub const VERIFY_CHARS: usize = 30_000;
pub const CLAIM_EVIDENCE_CHARS: usize = 1500;
pub const FAILURES_IN_A_ROW: i64 = 3;
pub const IDLE_POLL_S: f64 = 3.0;
pub const EXAMPLES: usize = 3;

/// What the import takes from other parts.
pub trait Seams {
    /// The one fused search: ids of the held facts that look like `query`, best first.
    fn similar(&self, query: &str) -> Vec<String>;
    /// Whether the owner's chat turn is under way.
    fn busy(&self) -> bool;
    /// `asyncio.sleep`.
    fn sleep(&self, seconds: f64) -> BoxFuture<'static, ()>;
}

/// import_job.py:294 `check`.
pub fn check(source: &str, path: &str) -> Result<()> {
    let Some(name) = source_name(source) else {
        let names: Vec<&str> = SOURCES.iter().map(|(k, _)| *k).collect();
        return Err(Error::ImportUnreadable(format!(
            "unknown source {}: one of {}",
            pystr::repr(source),
            names.join(", ")
        )));
    };
    if EXPORTS.contains(&source) && path.is_empty() {
        return Err(Error::ImportUnreadable(format!(
            "{name} is imported from its data export: choose the file"
        )));
    }
    Ok(())
}

fn numbers(counts: &Map<String, Value>) -> Value {
    Value::Object(
        counts
            .iter()
            .filter(|(k, _)| *k != "saved_ids")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    )
}

/// import_job.py:44 `Plan`.
pub struct Plan {
    pub conversations: Vec<Conversation>,
    pub todo: Vec<Part>,
    pub done_before: usize,
    pub chunks: Vec<Chunk>,
}

impl Plan {
    /// import_job.py:52 `counts`.
    pub fn counts(&self) -> Map<String, Value> {
        let chars: usize = self.chunks.iter().map(|c| pystr::char_len(&c.text())).sum();
        let mut m = Map::new();
        m.insert(
            "conversations".into(),
            json!(self.conversations.iter().filter(|c| !c.claims).count()),
        );
        m.insert(
            "saved_memories".into(),
            json!(self.conversations.iter().filter(|c| c.claims).count()),
        );
        m.insert(
            "messages".into(),
            json!(self
                .conversations
                .iter()
                .map(|c| c.messages.len())
                .sum::<usize>()),
        );
        m.insert(
            "owner_messages".into(),
            json!(self
                .conversations
                .iter()
                .flat_map(|c| &c.messages)
                .filter(|m| m.role == "owner")
                .count()),
        );
        m.insert("parts".into(), json!(self.todo.len() + self.done_before));
        m.insert("parts_done_before".into(), json!(self.done_before));
        m.insert("chunks".into(), json!(self.chunks.len()));
        m.insert("model_calls_estimate".into(), json!(2 * self.chunks.len()));
        m.insert("input_tokens_estimate".into(), json!((chars * 2) / 4));
        m
    }
}

/// import_extract.py:294 `SaveCounts`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SaveCounts {
    pub saved: Vec<String>,
    pub credited: i64,
    pub kept_owner: i64,
    pub older: i64,
    pub history: i64,
    pub refused: Vec<String>,
}

impl SaveCounts {
    pub fn json(&self) -> Value {
        json!({"saved": self.saved, "credited": self.credited, "kept_owner": self.kept_owner, "older": self.older,
               "history": self.history, "refused": self.refused})
    }
}

/// import_extract.py:303 `said_at`: when a held fact was said (an imported one's own date, else its save time).
pub fn said_at(memory: &Memory, item: &Item) -> Result<crate::DateTime> {
    let row = memory.store.one(
        "SELECT MAX(said_at) FROM memory_import_items WHERE item_id = ?",
        sql![item.id],
    )?;
    match row.and_then(|r| r.opt_at(0)).filter(|s| !s.is_empty()) {
        Some(s) => py::parse(&s),
        None => Ok(item.valid_from),
    }
}

/// import_extract.py:309 `save`: facts naming one fact share its kind, subject and key; each fact's versions are
/// saved oldest first, so the newer words win and the older stay as history (MEM-409).
pub fn save(
    memory: &Memory,
    facts: &[Verified],
    run_id: &str,
    source_entry: &str,
    now: &crate::DateTime,
) -> Result<SaveCounts> {
    let mut counts = SaveCounts::default();
    let mut facts: Vec<Verified> = facts.to_vec();
    let index_of = |facts: &[Verified], id: &str| facts.iter().position(|f| f.id == id);
    for n in 0..facts.len() {
        let mut root = n;
        let mut hops = 0;
        while let Some(next) = index_of(&facts, &facts[root].same_as) {
            if next == root || hops >= facts.len() {
                break;
            }
            root = next;
            hops += 1;
        }
        let same_as = facts[root].same_as.clone();
        if !same_as.is_empty() && index_of(&facts, &same_as).is_none() {
            if let Some(held) = memory.get(&same_as)? {
                facts[n].kind = held.kind;
                facts[n].subject = held.subject;
                facts[n].key = held.key;
            }
        } else if root != n {
            let (k, s, y) = (
                facts[root].kind.clone(),
                facts[root].subject.clone(),
                facts[root].key.clone(),
            );
            facts[n].kind = k;
            facts[n].subject = s;
            facts[n].key = y;
        }
    }
    let mut groups: Vec<((String, String, String), Vec<Verified>)> = Vec::new();
    for f in &facts {
        let name = (
            f.kind.clone(),
            subject_key(&(memory.redact)(&f.subject)),
            slug(&(memory.redact)(&f.key), None),
        );
        match groups.iter_mut().find(|(k, _)| *k == name) {
            Some((_, v)) => v.push(f.clone()),
            None => groups.push((name, vec![f.clone()])),
        }
    }
    let about = facts
        .first()
        .map(|f| {
            format!(
                "imported from {}",
                source_name(&f.source).unwrap_or(&f.source)
            )
        })
        .unwrap_or_default();
    let today = times::astimezone(now, memory.zone.as_ref()).map(|d| Date {
        year: d.year,
        month: d.month,
        day: d.day,
    });
    for ((kind, subject, key), mut versions) in groups {
        versions.sort_by_key(|f| times::instant(&f.said_at));
        for f in versions {
            let current: Vec<Item> = memory
                .items(Some("active"), Some(&subject), true)?
                .into_iter()
                .filter(|i| i.kind == kind && i.key == key)
                .collect();
            if current.iter().any(|i| i.origin == "owner_stated") {
                counts.kept_owner += 1;
                continue;
            }
            let mut older = false;
            for i in &current {
                if py::after(&said_at(memory, i)?, &f.said_at)? {
                    older = true;
                    break;
                }
            }
            if older {
                counts.older += 1;
                continue;
            }
            let got = memory.save(
                &kind,
                &subject,
                &key,
                &f.content,
                "inferred",
                &[("journal".into(), source_entry.to_owned())],
                0.6,
                &[],
                &about,
                Some(false),
            );
            let item = match got {
                Ok(s) => s.item,
                Err(e @ (Error::MemoryRefused(_) | Error::Value(_))) => {
                    counts.refused.push(e.to_string());
                    continue;
                }
                Err(e) => return Err(e),
            };
            let fresh = !current.iter().any(|i| i.id == item.id);
            let over = f.ended || f.until.is_some_and(|u| today.is_some_and(|t| u < t));
            if !fresh {
                counts.credited += 1;
            } else if over {
                counts.history += 1;
            } else {
                counts.saved.push(item.id.clone());
            }
            memory.store.exec(
                "INSERT INTO memory_import_items (item_id, source, run_id, said_at) VALUES (?, ?, ?, ?) \
                 ON CONFLICT(item_id, source) DO UPDATE SET said_at = MAX(said_at, excluded.said_at)",
                sql![item.id, f.source, run_id, py::iso(&f.said_at)],
            )?;
            if fresh && (f.ended || f.until.is_some()) {
                let end = match f.until {
                    Some(u) => {
                        let offset =
                            times::astimezone(now, memory.zone.as_ref()).and_then(|d| d.offset);
                        crate::DateTime {
                            year: u.year,
                            month: u.month,
                            day: u.day,
                            hour: 23,
                            minute: 59,
                            second: 59,
                            microsecond: 999_999,
                            offset,
                        }
                    }
                    None => f.said_at,
                };
                memory.expire_at(&item.id, &end)?;
            } else if fresh && f.review_in_days > 0 {
                memory.set_review(
                    &item.id,
                    Some(&py::shift(now, f.review_in_days * 86_400_000_000)),
                )?;
            }
        }
    }
    Ok(counts)
}

/// import_extract.py:367 `imported_from`: which assistants each item was imported from.
pub fn imported_from(store: &Store, item_ids: &[String]) -> Result<Vec<(String, Vec<String>)>> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for batch in item_ids.chunks(500) {
        for r in store.query(
            &format!(
                "SELECT item_id, source FROM memory_import_items WHERE item_id IN ({}) ORDER BY source",
                crate::store::marks(batch.len())
            ),
            batch.iter().map(|i| crate::store::Sql::Text(i.clone())).collect(),
        )? {
            let (id, source) = (r.s("item_id"), r.s("source"));
            match out.iter_mut().find(|(k, _)| *k == id) {
                Some((_, v)) => v.push(source),
                None => out.push((id, vec![source])),
            }
        }
    }
    Ok(out)
}

fn fact(it: &Item) -> Fact {
    Fact {
        id: it.id.clone(),
        kind: it.kind.clone(),
        subject: it.subject.clone(),
        key: it.key.clone(),
        content: it.content.clone(),
        origin: it.origin.clone(),
        status: it.status.clone(),
        valid_from: it.valid_from,
        valid_to: it.valid_to,
        set_aside: it.set_aside,
    }
}

/// import_job.py:305 `_batches`: candidates in verification calls of about VERIFY_CHARS; a claim's owner evidence
/// shown shorter.
pub fn batches(mut candidates: Vec<Candidate>) -> Vec<Vec<Candidate>> {
    for c in &mut candidates {
        if c.claim {
            for (_, m, who) in &mut c.evidence {
                if who == "owner" {
                    m.text = import_sources::cut(&m.text, CLAIM_EVIDENCE_CHARS);
                }
            }
        }
    }
    let mut out: Vec<Vec<Candidate>> = Vec::new();
    let mut size = 0usize;
    for c in candidates {
        let chars = pystr::char_len(&c.content)
            + pystr::char_len(&c.quote)
            + c.evidence
                .iter()
                .map(|(_, m, _)| pystr::char_len(&m.text) + 40)
                .sum::<usize>()
            + 120;
        if !out.is_empty() && size + chars <= VERIFY_CHARS {
            out.last_mut().expect("a batch").push(c);
            size += chars;
        } else {
            out.push(vec![c]);
            size = chars;
        }
    }
    out
}

/// import_job.py:65 `ImportJob`.
pub struct ImportJob<'a> {
    pub memory: Rc<Memory>,
    pub store: Rc<Store>,
    pub journal: Rc<Journal>,
    pub make_runtime: Box<MakeRuntime<'a>>,
    pub prompts: Rc<PromptRoot>,
    pub redact: Box<dyn Fn(&str) -> String + 'a>,
    pub publish: Box<dyn Fn(&str, Value) + 'a>,
    pub tell: Box<dyn Fn(Notice) + 'a>,
    pub seams: Box<dyn Seams + 'a>,
    pub ids: Rc<dyn Ids>,
    pub spans: Option<Rc<dyn Spans + 'a>>,
    pub usage: Box<dyn Fn(&str, &Value) + 'a>,
    pub device_id: String,
    pub home: Option<PathBuf>,
    pub unzstd: Option<Box<dyn Unzstd + 'a>>,
    pub idle_poll: f64,
    pub chunk_chars: usize,
    /// Chunks read at once (the owner allowed three, IMP-21); each still waits for the owner's chat turn.
    pub at_once: usize,
    /// How many months back the import and the work log read: `import_sources::WINDOW_MONTHS`, the last month
    /// (MEM-458); the recordings were made with Python's three (`PYTHON_WINDOW_MONTHS`).
    pub window_months: u32,
}

/// Chunks read at once by default (IMP-21).
pub const AT_ONCE: usize = 3;

/// Where a verified fact was said: the conversation or session it came from, and its place.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Said {
    pub conversation: String,
    pub place_kind: String,
    pub place: String,
    pub claim: bool,
}

/// Polls every future to its end on this task (they are not `Send`; a few at once).
async fn join_all<T>(
    mut futs: Vec<std::pin::Pin<Box<dyn std::future::Future<Output = T> + '_>>>,
) -> Vec<T> {
    let mut out: Vec<Option<T>> = futs.iter().map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (i, f) in futs.iter_mut().enumerate() {
            if out[i].is_none() {
                match f.as_mut().poll(cx) {
                    std::task::Poll::Ready(v) => out[i] = Some(v),
                    std::task::Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(())
        }
    })
    .await;
    out.into_iter()
        .map(|v| v.expect("polled to its end"))
        .collect()
}

impl ImportJob<'_> {
    fn zone(&self) -> &dyn LocalZone {
        self.memory.zone.as_ref()
    }

    /// import_job.py:81 `read`: the source's conversations in the window, redacted.
    pub fn read(&self, source: &str, path: &str) -> Result<Vec<Conversation>> {
        let opts = import_sources::ReadOptions {
            now: self.memory.clock.now(),
            redact: &*self.redact,
            home: self.home.as_deref(),
            zone: self.zone(),
            unzstd: self.unzstd.as_deref(),
            months: self.window_months,
        };
        let p = (!path.is_empty()).then(|| Path::new(path));
        // an export's plain checks around the reader (MEM-438)
        if let Some(p) = p.filter(|_| EXPORTS.contains(&source)) {
            ferb_memory::import_exports::check(source, p)?;
            let got = import_sources::read(source, Some(p), &opts)
                .map_err(|e| ferb_memory::import_exports::read_error(source, p, e))?;
            if got.is_empty() {
                return Err(ferb_memory::import_exports::nothing(source, p).into());
            }
            return Ok(got);
        }
        Ok(import_sources::read(source, p, &opts)?)
    }

    /// import_job.py:85 `plan`.
    pub fn plan(&self, source: &str, conversations: Vec<Conversation>) -> Result<Plan> {
        let all = ex::parts(&conversations, ex::CHUNK_CHARS, self.zone());
        let done: BTreeSet<String> = self
            .store
            .query(
                "SELECT part FROM memory_import_done WHERE source = ?",
                sql![source],
            )?
            .iter()
            .map(|r| r.s("part"))
            .collect();
        let total = all.len();
        let todo: Vec<Part> = all.into_iter().filter(|p| !done.contains(&p.key)).collect();
        let chunks = ex::chunks(source, &todo, self.chunk_chars);
        Ok(Plan {
            conversations,
            done_before: total - todo.len(),
            todo,
            chunks,
        })
    }

    /// import_job.py:92 `estimate`: what an import would read, without a model call.
    pub fn estimate(&self, source: &str, path: &str) -> Result<Value> {
        check(source, path)?;
        let conversations = self.read(source, path)?;
        let now = self.memory.clock.now();
        let since = import_sources::window_start_months(&now, self.window_months)
            .ok_or_else(|| Error::Value("year 0 is out of range".into()))?;
        let mut out = Map::new();
        out.insert("source".into(), json!(source));
        out.insert("name".into(), json!(source_name(source)));
        out.insert("since".into(), json!(py::iso(&since)));
        out.insert("until".into(), json!(py::iso(&now)));
        out.extend(self.plan(source, conversations)?.counts());
        Ok(Value::Object(out))
    }

    /// import_job.py:102 `create`.
    pub fn create(&self, source: &str, path: &str) -> Result<String> {
        check(source, path)?;
        if !self.running()?.is_empty() {
            return Err(Error::ImportBusy(
                "an import is already running; cancel it or wait for it to finish".into(),
            ));
        }
        let run_id = self.ids.new_id();
        let now = py::iso(&self.memory.clock.now());
        self.store.exec(
            "INSERT INTO memory_import_runs (id, source, path, status, started_at, updated_at) \
             VALUES (?, ?, ?, 'running', ?, ?)",
            sql![run_id, source, path, now, now],
        )?;
        Ok(run_id)
    }

    /// import_job.py:111 `running`.
    pub fn running(&self) -> Result<Vec<String>> {
        Ok(self
            .store
            .query(
                "SELECT id FROM memory_import_runs WHERE status = 'running' ORDER BY started_at",
                sql![],
            )?
            .iter()
            .map(|r| r.s("id"))
            .collect())
    }

    /// import_job.py:115 `row`.
    pub fn row(&self, run_id: &str) -> Result<Option<Value>> {
        let Some(r) = self.store.one(
            "SELECT * FROM memory_import_runs WHERE id = ?",
            sql![run_id],
        )?
        else {
            return Ok(None);
        };
        let source = r.s("source");
        Ok(Some(
            json!({"run": r.s("id"), "source": source, "name": source_name(&source).unwrap_or(&source),
            "status": r.s("status"), "started_at": r.s("started_at"), "updated_at": r.s("updated_at"),
            "finished_at": r.opt("finished_at"), "error": r.s("error"),
            "counts": ferb_py::json::loads(&r.s("counts_json")).map_err(Error::Json)?}),
        ))
    }

    /// import_job.py:123 `status`.
    pub fn status(&self) -> Result<Value> {
        let mut runs = Vec::new();
        for r in self.store.query(
            "SELECT id FROM memory_import_runs ORDER BY status = 'running' DESC, started_at DESC, rowid DESC LIMIT 10",
            sql![],
        )? {
            if let Some(row) = self.row(&r.s("id"))? {
                runs.push(row);
            }
        }
        Ok(json!({"current": runs.first().cloned(), "runs": runs}))
    }

    fn set(
        &self,
        run_id: &str,
        counts: &Map<String, Value>,
        status: &str,
        error: &str,
    ) -> Result<()> {
        let now = py::iso(&self.memory.clock.now());
        self.store.exec(
            "UPDATE memory_import_runs SET status = ?, updated_at = ?, counts_json = ?, error = ?, finished_at = \
             CASE WHEN ? = 'running' THEN NULL ELSE ? END WHERE id = ?",
            sql![status, now, ferb_py::json::dumps(counts), error, status, now, run_id],
        )?;
        Ok(())
    }

    /// Fixed against Python: writes that must land together, in one transaction.
    fn together<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.store.plain("BEGIN IMMEDIATE")?;
        match f() {
            Ok(v) => {
                self.store.plain("COMMIT")?;
                Ok(v)
            }
            Err(e) => {
                if self.store.in_transaction() {
                    self.store.plain("ROLLBACK")?;
                }
                Err(e)
            }
        }
    }

    /// import_job.py:137 `end`: a run stopped from outside (cancelled), keeping its counts.
    pub fn end(&self, run_id: &str, status: &str, error: &str) -> Result<()> {
        let Some(row) = self.row(run_id)? else {
            return Ok(());
        };
        if row["status"] != json!("running") {
            return Ok(());
        }
        let counts = row["counts"].as_object().cloned().unwrap_or_default();
        self.together(|| {
            self.set(run_id, &counts, status, error)?;
            self.journal.append(
                FINISHED_KIND,
                Some(json!({"run": run_id, "source": row["source"], "status": status, "counts": numbers(&counts)})),
                None,
            )?;
            Ok(())
        })?;
        (self.publish)(
            "memory.import.progress",
            json!({"run": run_id, "status": status, "counts": row["counts"]}),
        );
        Ok(())
    }

    /// import_job.py:146 `run`, reading only: the chunks' checked facts are kept with where each was said
    /// (`memory_import_facts`) and placed into the profile afterwards (`place`, IMP-20, IMP-31). Three chunks are
    /// read at once (IMP-21); each chunk's facts, done marks and progress still land together, in order.
    pub async fn run(&self, run_id: &str) -> Result<Value> {
        let row = match self.row(run_id)? {
            Some(r) if r["status"] == json!("running") => r,
            _ => return Err(Error::Value("no running import with that id".into())),
        };
        let source = py::py_str(&row["source"]);
        let path = self
            .store
            .one(
                "SELECT path FROM memory_import_runs WHERE id = ?",
                sql![run_id],
            )?
            .map(|r| r.s_at(0))
            .unwrap_or_default();
        let mut counts = row["counts"].as_object().cloned().unwrap_or_default();
        counts.entry("model_calls").or_insert(json!(0));
        let conversations = match self.read(&source, &path) {
            Ok(c) => c,
            Err(e @ (Error::ImportUnreadable(_) | Error::Crash("OSError", _))) => {
                return self.finish(run_id, &source, &mut counts, "failed", &e.to_string());
            }
            Err(e) => return Err(e),
        };
        // an export older than the newest one imported sends nothing (MEM-438)
        let newest = ferb_memory::import_exports::newest(&conversations);
        if EXPORTS.contains(&source.as_str()) {
            if let (Some(n), Some(before)) = (
                newest,
                self.store.one(
                    "SELECT newest, at FROM memory_import_exports WHERE source = ?",
                    sql![source],
                )?,
            ) {
                let was = py::parse(&before.s("newest"))?;
                if times::instant(&n) < times::instant(&was) {
                    counts.insert("older_export".into(), json!(before.s("at")));
                    return self.finish(run_id, &source, &mut counts, "done", "");
                }
            }
        }
        let index = OwnerIndex::new(&conversations);
        let plan = self.plan(&source, conversations)?;
        for (k, v) in plan.counts() {
            counts.entry(k).or_insert(v);
        }
        for k in [
            "chunks_done",
            "candidates",
            "dropped",
            "verified",
            "facts_saved",
            "credited",
            "kept_owner",
            "older",
            "history",
            "refused",
            "failed_chunks",
        ] {
            counts.entry(k).or_insert(json!(0));
        }
        counts.insert("chunks_left".into(), json!(plan.chunks.len()));
        counts.entry("saved_ids").or_insert(json!([]));
        self.set(run_id, &counts, "running", "")?;
        self.progress(run_id, &counts, "reading");
        let mut failures = 0;
        for window in plan.chunks.chunks(self.at_once.max(1)) {
            let shared = std::cell::RefCell::new(Map::new());
            let base = counts.clone();
            let futs: Vec<
                std::pin::Pin<
                    Box<dyn std::future::Future<Output = Result<Vec<(Verified, Said)>>> + '_>,
                >,
            > = window
                .iter()
                .map(|chunk| {
                    let shared = &shared;
                    let index = &index;
                    let base = &base;
                    Box::pin(async move {
                        // the run's counts as they stood, so a progress line while waiting shows them
                        let mut mine = base.clone();
                        let got = self.chunk(run_id, chunk, index, &mut mine).await;
                        let mut all = shared.borrow_mut();
                        for (k, v) in mine {
                            let was = base.get(&k).and_then(Value::as_i64).unwrap_or(0);
                            let n = all.get(&k).and_then(Value::as_i64).unwrap_or(0)
                                + v.as_i64().unwrap_or(0)
                                - was;
                            all.insert(k, json!(n));
                        }
                        got
                    })
                        as std::pin::Pin<Box<dyn std::future::Future<Output = _> + '_>>
                })
                .collect();
            let results = join_all(futs).await;
            for (k, v) in shared.into_inner() {
                add(&mut counts, &k, v.as_i64().unwrap_or(0));
            }
            for (chunk, got) in window.iter().zip(results) {
                let facts = match got {
                    Ok(f) => f,
                    Err(e) => {
                        failures += 1;
                        let last = pystr::prefix(&(self.redact)(&e.described()), 300).to_owned();
                        add(&mut counts, "failed_chunks", 1);
                        self.journal.append(
                            "memory.import_chunk_failed",
                            Some(json!({"run": run_id, "source": source, "error": last})),
                            None,
                        )?;
                        if failures >= FAILURES_IN_A_ROW {
                            return self.finish(run_id, &source, &mut counts, "failed", &last);
                        }
                        continue;
                    }
                };
                failures = 0;
                // the chunk's facts, done marks and progress land together (the fix, see the module doc)
                self.together(|| {
                    let stamp = py::iso(&self.memory.clock.now());
                    let mut kept = 0;
                    for (f, said) in &facts {
                        let said_at = py::iso(&f.said_at);
                        let id = crate::import_profile::fact_id(&source, &f.kind, &f.subject, &f.key, &f.content, &said_at);
                        kept += self.store.exec(
                            "INSERT OR IGNORE INTO memory_import_facts (id, source, run_id, kind, subject, key, content, \
                             said_at, conversation, place_kind, place, claim, ended, until, review_days, at) \
                             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                            sql![id, source, run_id, f.kind, (self.redact)(&f.subject), (self.redact)(&f.key),
                                 (self.redact)(&f.content), said_at, said.conversation, said.place_kind,
                                 (self.redact)(&said.place), said.claim, f.ended,
                                 f.until.map(|u| u.isoformat()).unwrap_or_default(), f.review_in_days, stamp],
                        )?;
                    }
                    for part in &chunk.parts {
                        self.store.exec(
                            "INSERT OR IGNORE INTO memory_import_done (source, part, run_id, at) VALUES (?, ?, ?, ?)",
                            sql![source, part.key, run_id, stamp],
                        )?;
                    }
                    add(&mut counts, "facts_saved", kept as i64);
                    add(&mut counts, "chunks_done", 1);
                    add(&mut counts, "chunks_left", -1);
                    self.set(run_id, &counts, "running", "")
                })?;
                self.progress(run_id, &counts, "reading");
            }
        }
        if let (Some(n), true) = (newest, EXPORTS.contains(&source.as_str())) {
            let at = py::iso(&self.memory.clock.now());
            self.store.exec(
                "INSERT INTO memory_import_exports (source, newest, at) VALUES (?, ?, ?) ON CONFLICT(source) DO UPDATE \
                 SET newest = excluded.newest, at = excluded.at WHERE excluded.newest > memory_import_exports.newest",
                sql![source, py::iso(&n), at],
            )?;
        }
        self.finish(run_id, &source, &mut counts, "done", "")
    }

    async fn wait_idle(&self, run_id: &str, counts: &Map<String, Value>) {
        let mut told = false;
        while self.seams.busy() {
            if !told {
                self.progress(run_id, counts, "waiting for your current chat turn");
                told = true;
            }
            self.seams.sleep(self.idle_poll).await;
        }
    }

    async fn chunk(
        &self,
        run_id: &str,
        chunk: &Chunk,
        index: &OwnerIndex,
        counts: &mut Map<String, Value>,
    ) -> Result<Vec<(Verified, Said)>> {
        let keys: Vec<&str> = std::iter::once(chunk.source.as_str())
            .chain(chunk.parts.iter().map(|p| p.key.as_str()))
            .collect();
        let name = source_name(&chunk.source).unwrap_or(&chunk.source);
        let task = calls::learning_task(
            &format!("memory-import-{}", &ex::digest(&keys)[..12]),
            &format!("import memory from {name}"),
            &self.device_id,
            "memory-import",
        );
        let answer = self
            .call(
                run_id,
                &task,
                "memory/import_recall.md",
                &chunk.text(),
                &ex::recall_schema(),
                "memory_import_recall",
                counts,
            )
            .await?;
        let (candidates, dropped) =
            ex::check_recall(&Value::Object(answer), chunk, index, &*self.redact)?;
        add(
            counts,
            "candidates",
            (candidates.len() + dropped.len()) as i64,
        );
        add(counts, "dropped", dropped.len() as i64);
        // where each candidate was said, by the reader that knows it, never a model (MEM-436)
        let refs = chunk.refs();
        let said_of: std::collections::HashMap<String, Said> = candidates
            .iter()
            .filter_map(|c| {
                let (_, _, conv) = refs.iter().find(|(r, _, _)| *r == c.message)?;
                let (place_kind, place) = if conv.claims {
                    (String::new(), String::new())
                } else if !conv.place.is_empty() {
                    ("folder".to_owned(), conv.place.clone())
                } else {
                    ("chat".to_owned(), conv.title.clone())
                };
                Some((
                    c.id.clone(),
                    Said {
                        conversation: conv.id.clone(),
                        place_kind,
                        place,
                        claim: c.claim,
                    },
                ))
            })
            .collect();
        let mut facts = Vec::new();
        for batch in batches(candidates) {
            let lookup = |id: &str| -> std::result::Result<Option<Fact>, ferb_memory::Error> {
                self.memory
                    .get(id)
                    .map(|o| o.as_ref().map(fact))
                    .map_err(|e| ferb_memory::Error::Refused(e.to_string()))
            };
            let mut similar = |q: &str| self.seams.similar(q);
            let held = ex::similar_held(&lookup, &batch, &mut similar, &self.memory.clock.now())?;
            let ctx = ex::verify_context(&chunk.source, &batch, &held, self.zone());
            let answer = self
                .call(
                    run_id,
                    &task,
                    "memory/import_verify.md",
                    &ctx,
                    &ex::verify_schema(),
                    "memory_import_verify",
                    counts,
                )
                .await?;
            let held_ids: HashSet<String> = held.iter().map(|h| h.id.clone()).collect();
            // the reading verify names becomes kept or not and the fact's kind in code (MEM-454)
            let answer = ex::verdicts(&Value::Object(answer), &batch)?;
            let (kept, _) = ex::check_verify(&answer, &batch, &held_ids, &chunk.source)?;
            add(counts, "dropped", (batch.len() - kept.len()) as i64);
            add(counts, "verified", kept.len() as i64);
            facts.extend(kept.into_iter().map(|v| {
                let said = said_of.get(&v.id).cloned().unwrap_or_default();
                (v, said)
            }));
        }
        Ok(facts)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn call(
        &self,
        run_id: &str,
        task: &ferb_core::types::Task,
        prompt: &str,
        ctx: &str,
        schema: &Value,
        role: &str,
        counts: &mut Map<String, Value>,
    ) -> Result<Map<String, Value>> {
        self.wait_idle(run_id, counts).await;
        let made = (self.make_runtime)("worker", "supervisor", &[], schema)?;
        add(counts, "model_calls", 1);
        let system = self
            .prompts
            .load(prompt, &[])
            .map_err(|e| Error::Crash("OSError", e.to_string()))?;
        let usage = |n: &str, d: &Value| (self.usage)(n, d);
        let text = learning_call(
            &made,
            task,
            &system,
            ctx,
            &format!("imported history ({role})"),
            schema,
            role,
            self.spans.as_deref(),
            &usage,
        )
        .await?;
        Ok(ex::as_json(&Value::String(text))?)
    }

    fn progress(&self, run_id: &str, counts: &Map<String, Value>, stage: &str) {
        (self.publish)(
            "memory.import.progress",
            json!({"run": run_id, "status": "running", "stage": stage, "counts": numbers(counts)}),
        );
    }

    fn finish(
        &self,
        run_id: &str,
        source: &str,
        counts: &mut Map<String, Value>,
        status: &str,
        error: &str,
    ) -> Result<Value> {
        self.together(|| {
            self.set(run_id, counts, status, error)?;
            let mut data = json!({"run": run_id, "source": source, "status": status, "counts": numbers(counts)});
            if !error.is_empty() {
                data["error"] = json!(error);
            }
            self.journal.append(FINISHED_KIND, Some(data), None)?;
            Ok(())
        })?;
        let mut told = json!({"run": run_id, "status": status, "counts": numbers(counts)});
        if !error.is_empty() {
            told["error"] = json!(error);
        }
        (self.publish)("memory.import.progress", told);
        Ok(self.row(run_id)?.unwrap_or_else(|| json!({})))
    }

    /// Places the run's facts into the profile (IMP-31) and keeps what changed on the run, for its notice.
    pub async fn place_run(&self, run_id: &str) -> Result<Map<String, Value>> {
        let change = self.place(run_id).await?;
        // MEM-460: what holds up is listed for the owner's word, never joined by itself
        self.suggest_always_on()?;
        if let Some(row) = self.row(run_id)? {
            let mut counts = row["counts"].as_object().cloned().unwrap_or_default();
            counts.insert("profile".into(), Value::Object(change.clone()));
            let n = |k: &str| change.get(k).and_then(Value::as_i64).unwrap_or(0);
            counts.insert(
                "facts_saved".into(),
                json!(n("new lines") + n("lines reworded by a newer fact")),
            );
            self.store.exec(
                "UPDATE memory_import_runs SET counts_json = ? WHERE id = ?",
                sql![ferb_py::json::dumps(&Value::Object(counts)), run_id],
            )?;
        }
        Ok(change)
    }

    /// The one notice after an import (MEM-460): the profile's report (`report`), said to the owner; a run that
    /// stopped says so, and an older export too. `closing`: a line the caller adds (the export file deleted).
    pub fn tell_done(&self, run_id: &str) -> Result<()> {
        self.tell_done_with(run_id, "")
    }

    pub fn tell_done_with(&self, run_id: &str, closing: &str) -> Result<()> {
        let Some(row) = self.row(run_id)? else {
            return Ok(());
        };
        let counts = &row["counts"];
        let name = py::py_str(&row["name"]);
        let source = py::py_str(&row["source"]);
        let local = !EXPORTS.contains(&source.as_str());
        let change = counts
            .get("profile")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let n = |k: &str| change.get(k).and_then(Value::as_i64).unwrap_or(0);
        let learned = n("new lines") + n("lines reworded by a newer fact");
        // a run that read to its end, every chunk of it; one that left parts unread says so (they are read next time)
        let unread = counts
            .get("failed_chunks")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let (text, cards) = if row["status"] == json!("done") && unread > 0 && learned == 0 {
            (
                format!(
                    "I couldn't finish bringing in what {name} knows about you: part of it couldn't be read just now. \
                     Ask me again any time and I'll go on from where I stopped."
                ),
                Vec::new(),
            )
        } else if row["status"] == json!("done") {
            let older = counts
                .get("older_export")
                .map(py::py_str)
                .unwrap_or_default();
            if !older.is_empty() {
                (
                    format!(
                        "This {name} export is older than the one I brought in on {}, so there was nothing new in it.",
                        pystr::prefix(&older, 10)
                    ),
                    Vec::new(),
                )
            } else {
                let opening = match (local, learned) {
                    (true, 0) => format!("I read what you've typed into {name} on this Mac. Nothing new to remember this time."),
                    (true, k) => format!(
                        "I read what you've typed into {name} on this Mac over the last month, and learned {k} thing{} about you and how you like to work.",
                        if k == 1 { "" } else { "s" }
                    ),
                    (false, 0) => format!("I've gone through what {name} knew about you. Nothing new to remember this time."),
                    (false, k) => format!(
                        "I've brought in what {name} knew about you from your last month of chats: {k} thing{} I didn't know before.",
                        if k == 1 { "" } else { "s" }
                    ),
                };
                self.report(&opening, &change)?
            }
        } else {
            let error = py::py_str(&row["error"]);
            let why = if error.is_empty() {
                py::py_str(&row["status"])
            } else {
                error
            };
            (
                format!("I couldn't finish bringing in what {name} knows about you: {why}. Ask me again any time and I'll go on from where I stopped."),
                Vec::new(),
            )
        };
        let text = if closing.is_empty() {
            text
        } else {
            format!("{text}\n\n{closing}")
        };
        (self.tell)(Notice {
            id: format!("memory-import-{run_id}"),
            refs: cards
                .iter()
                .filter_map(|c| {
                    c.get("item_id")
                        .map(|i| format!("memory:{}", py::py_str(i)))
                })
                .collect(),
            text,
            origin: origin::join([origin::FERB, origin::INFERRED]),
            cards: Some(Value::Array(cards)),
        });
        Ok(())
    }

    /// An export FERB fetched itself, to delete once its import is done (IMP-32).
    pub fn fetched(&self, path: &str, source: &str, run_id: &str) -> Result<()> {
        self.store.exec(
            "INSERT INTO memory_import_files (path, source, run_id, fetched_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT(path) DO UPDATE SET run_id = excluded.run_id",
            sql![path, source, run_id, py::iso(&self.memory.clock.now())],
        )?;
        Ok(())
    }

    /// Deletes the exports FERB fetched for this run once it has read them to the end (done, no chunk failed), or
    /// every one whose run failed, was cancelled or left a chunk unread a day ago or more (IMP-32). Never a file the owner gave (only fetched ones are recorded). Returns what was
    /// deleted, by source.
    pub fn delete_fetched(&self, run_id: Option<&str>) -> Result<Vec<(String, String)>> {
        let now = self.memory.clock.now();
        let mut gone = Vec::new();
        for r in self.store.query(
            "SELECT f.path, f.source, f.run_id, f.fetched_at, r.status, r.finished_at, r.counts_json \
             FROM memory_import_files f LEFT JOIN memory_import_runs r ON r.id = f.run_id",
            sql![],
        )? {
            let (path, status) = (r.s("path"), r.opt("status").unwrap_or_default());
            // read to its end: done with no chunk left unread (a failed chunk's parts would be lost with the file)
            let counts = ferb_py::json::loads(&r.opt("counts_json").unwrap_or_else(|| "{}".into())).unwrap_or_default();
            let read_all = status == "done" && counts.get("failed_chunks").and_then(Value::as_i64).unwrap_or(0) == 0;
            let this = run_id.is_some_and(|id| id == r.s("run_id")) && read_all;
            let since = r.opt("finished_at").filter(|s| !s.is_empty()).unwrap_or_else(|| r.s("fetched_at"));
            let stale = !read_all
                && status != "running"
                && py::parse(&since).is_ok_and(|t| times::instant(&now) - times::instant(&t) >= 86_400_000_000);
            if !(this || stale) {
                continue;
            }
            let p = Path::new(&path);
            let removed = if p.is_dir() {
                std::fs::remove_dir_all(p)
            } else {
                std::fs::remove_file(p)
            };
            match removed {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Error::Crash("OSError", e.to_string())),
            }
            self.store.exec("DELETE FROM memory_import_files WHERE path = ?", sql![path])?;
            gone.push((r.s("source"), status));
        }
        Ok(gone)
    }
}

fn add(counts: &mut Map<String, Value>, k: &str, n: i64) {
    let now = counts.get(k).and_then(Value::as_i64).unwrap_or(0);
    counts.insert(k.into(), json!(now + n));
}
