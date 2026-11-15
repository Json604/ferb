//! `import_job.py` (MEM-400..415): one memory import as a background job: its estimate, run record, progress,
//! resume, cancel and the one summary notice. A run reads its source again each start, skips the conversation parts
//! already read (`memory_import_done`), and works chunk by chunk: wait while the owner's chat turn runs, recall,
//! verify, save, then record the chunk's parts done.
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
use ferb_memory::terms::{label, slug, subject_key, Fact};
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
        };
        let p = (!path.is_empty()).then(|| Path::new(path));
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
        let since = import_sources::window_start(&now)
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

    /// import_job.py:146 `run`.
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
        let mut entry: Option<String> = None;
        let mut failures = 0;
        for chunk in &plan.chunks {
            let facts = match self.chunk(run_id, chunk, &index, &mut counts).await {
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
            // the chunk's source row, facts, done marks and progress land together (the fix, see the module doc)
            let done = self.together(|| {
                if !facts.is_empty() && entry.is_none() {
                    entry = Some(self.source_entry(run_id, &source)?);
                }
                let now = self.memory.clock.now();
                let got = if facts.is_empty() {
                    SaveCounts::default()
                } else {
                    save(&self.memory, &facts, run_id, entry.as_deref().unwrap_or(""), &now)?
                };
                let stamp = py::iso(&self.memory.clock.now());
                for part in &chunk.parts {
                    self.store.exec(
                        "INSERT OR IGNORE INTO memory_import_done (source, part, run_id, at) VALUES (?, ?, ?, ?)",
                        sql![source, part.key, run_id, stamp],
                    )?;
                }
                let current = self.memory.current_ids(Some("inferred"))?;
                let mut ids: Vec<Value> = counts["saved_ids"].as_array().cloned().unwrap_or_default();
                ids.extend(got.saved.iter().map(|s| json!(s)));
                ids.retain(|i| current.contains(&py::py_str(i)));
                counts.insert("facts_saved".into(), json!(ids.len()));
                counts.insert("saved_ids".into(), Value::Array(ids));
                add(&mut counts, "credited", got.credited);
                add(&mut counts, "kept_owner", got.kept_owner);
                add(&mut counts, "older", got.older);
                add(&mut counts, "history", got.history);
                add(&mut counts, "refused", got.refused.len() as i64);
                add(&mut counts, "chunks_done", 1);
                add(&mut counts, "chunks_left", -1);
                self.set(run_id, &counts, "running", "")
            });
            done?;
            self.progress(run_id, &counts, "reading");
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
    ) -> Result<Vec<Verified>> {
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
            let (kept, _) =
                ex::check_verify(&Value::Object(answer), &batch, &held_ids, &chunk.source)?;
            add(counts, "dropped", (batch.len() - kept.len()) as i64);
            add(counts, "verified", kept.len() as i64);
            facts.extend(kept);
        }
        Ok(facts)
    }

    #[allow(clippy::too_many_arguments)]
    async fn call(
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

    fn source_entry(&self, run_id: &str, source: &str) -> Result<String> {
        Ok(self
            .journal
            .append(
                SOURCE_KIND,
                Some(json!({"run": run_id, "source": source, "name": source_name(source)})),
                None,
            )?
            .id)
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

    /// import_job.py:264 `tell_done`: the one summary notice (MEM-406).
    pub fn tell_done(&self, run_id: &str) -> Result<()> {
        let Some(row) = self.row(run_id)? else {
            return Ok(());
        };
        let counts = &row["counts"];
        let name = py::py_str(&row["name"]);
        let mut items = Vec::new();
        for i in counts
            .get("saved_ids")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(it) = self.memory.get(&py::py_str(i))? {
                if it.status == "active" && it.valid_to.is_none() {
                    items.push(it);
                }
            }
        }
        let n = counts
            .get("facts_saved")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let examples: Vec<&Item> = items.iter().take(EXAMPLES).collect();
        let plural = if n != 1 { "s" } else { "" };
        let squeeze = |s: &str| pystr::split_ws(s).collect::<Vec<_>>().join(" ");
        let text = if row["status"] == json!("done") && n > 0 {
            let quoted: Vec<String> = examples
                .iter()
                .map(|it| format!("“{}”", squeeze(&it.content)))
                .collect();
            format!(
                "Imported {n} fact{plural} from {name} as my understanding, for example {}. They're on the Memory \
                 page (imported from {name}); tell me if any is wrong.",
                quoted.join(", ")
            )
        } else if row["status"] == json!("done") {
            format!("Imported nothing new from {name}: I found nothing more to remember there.")
        } else {
            let error = py::py_str(&row["error"]);
            let why = if error.is_empty() {
                py::py_str(&row["status"])
            } else {
                error
            };
            format!("The import from {name} stopped ({why}) after {n} fact{plural}. Start it again to go on from there.")
        };
        let line = format!("Imported from {name}");
        let mut cards: Vec<Value> = examples
            .iter()
            .map(|it| {
                json!({"type": "note", "item_id": it.id, "text": squeeze(&it.content), "state": "saved", "line": line,
                       "label": label(&it.origin), "origin": it.origin})
            })
            .collect();
        cards.push(
            json!({"type": "page", "page": "memory", "filter": {"imported_from": row["source"]},
                          "line": "Open the Memory page"}),
        );
        (self.tell)(Notice {
            id: format!("memory-import-{run_id}"),
            refs: examples
                .iter()
                .map(|it| format!("memory:{}", it.id))
                .collect(),
            text,
            origin: origin::join([origin::FERB, origin::INFERRED]),
            cards: Some(Value::Array(cards)),
        });
        Ok(())
    }
}

fn add(counts: &mut Map<String, Value>, k: &str, n: i64) {
    let now = counts.get(k).and_then(Value::as_i64).unwrap_or(0);
    counts.insert(k.into(), json!(now + n));
}
