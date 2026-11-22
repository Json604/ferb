//! The work log (MEM-441; the owner's IMP-1, IMP-6, IMP-17): what the owner worked on in Claude Code and Codex, one
//! entry per session per day, with the session's id so FERB can look the session up or reopen it. Built from the
//! prompt-history files (`ferb_memory::import_local`), so an entry outlives the tool's own transcript; no prompt
//! text is kept, only a model-written title, one or two sentences, up to four topics and a state.
//!
//! Each entry is a learned node of kind `work_log` (key `<tool>/<session>/<day>`; the session's id is kept as
//! `thread_id`, since a field named for a session is masked as a secret), so the one search FERB already
//! has finds it (`memory.learned`, by meaning and keywords, with `since`/`until` on the session's own day). A later
//! update summarises only entries that are new or grew (their prompts' hash changed and their count did not shrink:
//! a day the window's edge cuts into keeps its fuller summary). Kept until the owner deletes it (IMP-17).
//!
//! MEM-466 (the owner, 2026-11-23): no model reads his sessions unless he asks. The log is kept in step by code alone
//! (`worklog_index`, run when something reads it): every session-day gets its entry with the tool, the folder, the day,
//! the first and last time and the prompt count, and a plain title made from them; a summary (`worklog_update`, the
//! model) is written only when he asks FERB to learn from his sessions. `hash` is the prompts' hash the summary was
//! written for ("" for none yet); `seen` is the one code saw last.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use ferb_memory::import_local::{self, Line};
use ferb_memory::import_sources::{cut, owner_words, source_name, window_start_months};
use ferb_memory::pystr;
use ferb_memory::times;
use ferb_sanitize::origin;

use crate::import_job::ImportJob;
use crate::learning::Learning;
use crate::py;
use crate::sql;
use crate::{Error, Result};

pub const KIND: &str = "work_log";
/// One prompt as the summariser reads it.
pub const PROMPT_CHARS: usize = 700;
/// One session-day: its first prompts and its last, when longer.
pub const ENTRY_CHARS: usize = 6_000;
/// One model call.
pub const BATCH_CHARS: usize = 24_000;
pub const STATES: [&str; 4] = ["in progress", "finished", "dropped", "unclear"];
pub const MADE_BY: &str = "code:worklog/1";

/// One session-day, before its summary.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: String,
    pub tool: String,
    pub session: String,
    pub day: String,
    pub folder: String,
    pub first: String,
    pub last: String,
    pub count: usize,
    pub hash: String,
    pub text: String,
}

/// How a session is opened again (typing it into a terminal asks the owner first, AGENTS.md rule 6).
pub fn reopen(tool: &str, session: &str) -> String {
    if tool == "claude_code" {
        format!("claude --resume {session}")
    } else {
        format!("codex resume {session}")
    }
}

/// Every session-day in `lines` (noise dropped; a prompt repeated right after itself counted once), with the text
/// to summarise, redacted.
pub fn entries(
    lines: &[Line],
    zone: &dyn times::LocalZone,
    redact: &dyn Fn(&str) -> String,
) -> Vec<Entry> {
    let mut by: BTreeMap<String, (Entry, Vec<String>)> = BTreeMap::new();
    for l in lines {
        let words = owner_words(&l.text);
        if import_local::noise(&words) {
            continue;
        }
        let day =
            times::astimezone(&l.at, zone).map_or_else(|| times::ymd(&l.at), |d| times::ymd(&d));
        let key = format!("{}/{}/{day}", l.source, l.session);
        let at = py::iso(&l.at);
        let (e, prompts) = by.entry(key.clone()).or_insert_with(|| {
            (
                Entry {
                    key,
                    tool: l.source.clone(),
                    session: l.session.clone(),
                    day: day.clone(),
                    folder: l.folder.clone(),
                    first: at.clone(),
                    last: at.clone(),
                    count: 0,
                    hash: String::new(),
                    text: String::new(),
                },
                Vec::new(),
            )
        });
        e.last = at;
        if e.folder.is_empty() {
            e.folder = l.folder.clone();
        }
        if prompts.last() != Some(&words) {
            prompts.push(words);
        }
    }
    let mut out = Vec::new();
    for (_, (mut e, prompts)) in by {
        let cut_: Vec<String> = prompts
            .iter()
            .map(|p| redact(&cut(p, PROMPT_CHARS)))
            .collect();
        let mut text = cut_
            .iter()
            .map(|p| format!("- {p}"))
            .collect::<Vec<_>>()
            .join("\n");
        if pystr::char_len(&text) > ENTRY_CHARS {
            // a long day: how it began and how it ended
            let (mut head, mut tail, mut size) = (Vec::new(), Vec::new(), 0usize);
            for (a, b) in cut_.iter().zip(cut_.iter().rev()) {
                if size + pystr::char_len(a) + pystr::char_len(b) > ENTRY_CHARS
                    || head.len() + tail.len() + 2 > cut_.len()
                {
                    break;
                }
                head.push(a.clone());
                tail.push(b.clone());
                size += pystr::char_len(a) + pystr::char_len(b);
            }
            let left = cut_.len() - head.len() - tail.len();
            let mut parts: Vec<String> = head.iter().map(|p| format!("- {p}")).collect();
            parts.push(format!("  [... {left} more prompts in between ...]"));
            parts.extend(tail.iter().rev().map(|p| format!("- {p}")));
            text = parts.join("\n");
        }
        e.count = prompts.len();
        let refs: Vec<&str> = prompts.iter().map(String::as_str).collect();
        e.hash = ferb_memory::import_extract::digest(&refs)[..16].to_owned();
        e.text = text;
        out.push(e);
    }
    out.sort_by(|a, b| a.first.cmp(&b.first).then_with(|| a.key.cmp(&b.key)));
    out
}

/// The work log's current entries, by key.
pub fn kept(learning: &Learning) -> Result<BTreeMap<String, Value>> {
    let mut out = BTreeMap::new();
    for r in learning.store.query(
        "SELECT key, body_json FROM learning_nodes WHERE kind = ? AND valid_to IS NULL",
        sql![KIND],
    )? {
        out.insert(
            r.s("key"),
            ferb_py::json::loads(&r.s("body_json")).map_err(Error::Json)?,
        );
    }
    Ok(out)
}

pub fn schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["entries"], "properties": {"entries": {
        "type": "array", "items": {"type": "object", "additionalProperties": false,
                                   "required": ["ref", "title", "summary", "topics", "state"],
                                   "properties": {"ref": {"type": "string"}, "title": {"type": "string"},
                                                  "summary": {"type": "string"},
                                                  "topics": {"type": "array", "items": {"type": "string"}},
                                                  "state": {"type": "string"}}}}}})
}

/// The title code gives an entry that has no summary yet: the tool and the folder ("Claude Code in ferb").
pub fn plain_title(e: &Entry) -> String {
    let tool = source_name(&e.tool).unwrap_or(&e.tool);
    let folder = e
        .folder
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("");
    if folder.is_empty() {
        format!("{tool} session")
    } else {
        format!("{tool} in {folder}")
    }
}

impl ImportJob<'_> {
    /// The prompt-history files' sizes and times: what tells the index nothing changed (no read at all then).
    fn history_mark(&self) -> String {
        let home = self.home.clone().unwrap_or_default();
        import_local::FILES
            .iter()
            .map(|f| {
                let m = std::fs::metadata(home.join(f.1)).ok();
                let len = m.as_ref().map_or(0, |m| m.len());
                let at = m
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_micros());
                format!("{}:{len}:{at}", f.0)
            })
            .collect::<Vec<_>>()
            .join(";")
    }

    /// MEM-466: the work log kept in step with the prompt-history files by code alone, no model: a session-day not
    /// in it yet gets its entry (a plain title, no summary); one that grew gets its new count and times, keeping any
    /// summary he asked for earlier. Nothing is read when the files haven't changed since the last time. Counts.
    pub fn worklog_index(&self, learning: &Learning) -> Result<Map<String, Value>> {
        let mark = self.history_mark();
        let mut out = Map::new();
        if self.state("worklog_index_mark")? == mark {
            out.insert("unchanged".into(), json!(true));
            return Ok(out);
        }
        let now = self.memory.clock.now();
        let since = window_start_months(&now, self.window_months)
            .ok_or_else(|| Error::Value("year 0 is out of range".into()))?;
        let home = self.home.clone().unwrap_or_default();
        let mut lines = Vec::new();
        for (source, ..) in import_local::FILES {
            lines.extend(import_local::lines(&home, source, &since, &now));
        }
        let found = entries(&lines, self.memory.zone.as_ref(), &*self.redact);
        let index = kept(learning)?;
        let (mut added, mut grown) = (0, 0);
        for e in &found {
            let old = index.get(&e.key);
            let field =
                |o: &Value, k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
            if old.is_some_and(|o| field(o, "seen") == e.hash || field(o, "hash") == e.hash) {
                continue;
            }
            let mut said = old.and_then(Value::as_object).cloned().unwrap_or_default();
            if said
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .is_empty()
            {
                said.insert("title".into(), json!(plain_title(e)));
            }
            if old.is_some() {
                grown += 1;
            } else {
                added += 1;
            }
            let summarised = said
                .get("hash")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            self.put_entry_as(learning, e, &said, &summarised)?;
        }
        self.set_state("worklog_index_mark", &mark)?;
        out.insert("session_days".into(), json!(found.len()));
        out.insert("added".into(), json!(added));
        out.insert("grown".into(), json!(grown));
        Ok(out)
    }

    fn put_entry(&self, learning: &Learning, e: &Entry, said: &Map<String, Value>) -> Result<()> {
        self.put_entry_as(learning, e, said, &e.hash)
    }

    /// One entry written; `summarised`: the prompts' hash its summary was written for ("" when it has none yet).
    fn put_entry_as(
        &self,
        learning: &Learning,
        e: &Entry,
        said: &Map<String, Value>,
        summarised: &str,
    ) -> Result<()> {
        let clip = |k: &str, n: usize| {
            pystr::prefix(
                &(self.redact)(&said.get(k).map(py::py_str).unwrap_or_default()),
                n,
            )
            .to_owned()
        };
        let state = said.get("state").map(py::py_str).unwrap_or_default();
        let topics: Vec<String> = said
            .get("topics")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(4)
            .map(|t| pystr::prefix(&(self.redact)(&py::py_str(t)), 30).to_owned())
            .collect();
        let body = json!({"tool": e.tool, "tool_name": source_name(&e.tool).unwrap_or(&e.tool), "thread_id": e.session,
            "day": e.day, "folder": e.folder, "first": e.first, "last": e.last, "prompts": e.count, "hash": summarised,
            "seen": e.hash,
            "title": clip("title", 120), "summary": clip("summary", 500), "topics": topics,
            "state": if STATES.contains(&state.as_str()) { state } else { "unclear".into() },
            "reopen": reopen(&e.tool, &e.session)});
        let node = match learning.put(
            KIND,
            &e.key,
            &body,
            &[],
            origin::INFERRED,
            MADE_BY,
            None,
            "active",
            false,
            None,
            true,
        ) {
            Ok(n) => n,
            Err(Error::LearningRefused(_)) => return Ok(()), // the owner deleted this entry: never written again
            Err(e) => return Err(e),
        };
        // dated by the session's own day, so "what did I do last month" finds it (memory.learned since/until)
        learning.store.exec(
            "UPDATE learning_nodes SET valid_from = ? WHERE id = ?",
            sql![e.last, py::py_str_opt(node.get("id"))],
        )?;
        Ok(())
    }

    /// Brings the work log up to date with what was typed in the window: only entries that are new or grew go to
    /// the model; a folder learned since (Codex) is filled in with no model call. Returns counts.
    pub async fn worklog_update(&self, learning: &Learning) -> Result<Map<String, Value>> {
        let now = self.memory.clock.now();
        let since = window_start_months(&now, self.window_months)
            .ok_or_else(|| Error::Value("year 0 is out of range".into()))?;
        let home = self.home.clone().unwrap_or_default();
        let mut lines = Vec::new();
        for (source, ..) in import_local::FILES {
            lines.extend(import_local::lines(&home, source, &since, &now));
        }
        let found = entries(&lines, self.memory.zone.as_ref(), &*self.redact);
        let index = kept(learning)?;
        let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0) as usize;
        let mut todo: Vec<&Entry> = Vec::new();
        let (mut grown, mut named) = (0, 0);
        for e in &found {
            match index.get(&e.key) {
                // MEM-466: an entry the code index made has no summary yet: new to the summary
                None => todo.push(e),
                Some(old)
                    if old
                        .get("hash")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .is_empty() =>
                {
                    todo.push(e)
                }
                Some(old)
                    if py::py_str_opt(old.get("hash")) != e.hash
                        && e.count >= num(old, "prompts") =>
                {
                    grown += 1;
                    todo.push(e);
                }
                Some(old) => {
                    if !e.folder.is_empty() && py::py_str_opt(old.get("folder")).is_empty() {
                        let mut said = old.as_object().cloned().unwrap_or_default();
                        said.insert("folder".into(), json!(e.folder));
                        let mut kept_entry = e.clone();
                        kept_entry.count = num(old, "prompts");
                        kept_entry.hash = py::py_str_opt(old.get("hash"));
                        self.put_entry(learning, &kept_entry, &said)?;
                        named += 1;
                    }
                }
            }
        }
        let mut batches: Vec<Vec<(&Entry, String)>> = Vec::new();
        let mut size = 0;
        for e in &todo {
            let block = format!(
                "[{}] folder: {}, {}, {} prompts\n{}\n",
                e.key,
                if e.folder.is_empty() {
                    "not recorded"
                } else {
                    &e.folder
                },
                e.day,
                e.count,
                e.text
            );
            if batches.last().is_some_and(|b| !b.is_empty())
                && size + pystr::char_len(&block) > BATCH_CHARS
            {
                batches.push(Vec::new());
                size = 0;
            }
            if batches.is_empty() {
                batches.push(Vec::new());
            }
            size += pystr::char_len(&block);
            batches.last_mut().expect("a batch").push((e, block));
        }
        let (mut written, mut missed, mut calls) = (0, 0, 0);
        let mut counts = Map::new();
        for batch in batches.iter().filter(|b| !b.is_empty()) {
            calls += 1;
            let ctx = batch
                .iter()
                .map(|(_, b)| b.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let task = crate::calls::learning_task(
                &format!(
                    "worklog-{}",
                    &ferb_memory::import_extract::digest(&[&batch[0].0.key])[..12]
                ),
                "keep the work log of the owner's coding sessions",
                &self.device_id,
                "worklog",
            );
            let answer = match self
                .call(
                    "worklog",
                    &task,
                    "memory/worklog.md",
                    &ctx,
                    &schema(),
                    "worklog_summary",
                    &mut counts,
                )
                .await
            {
                Ok(a) => a,
                Err(_) => {
                    // these entries are left for the next run
                    missed += batch.len();
                    continue;
                }
            };
            let got: BTreeMap<String, Map<String, Value>> = answer
                .get("entries")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|x| {
                    x.as_object()
                        .map(|m| (py::py_str_opt(m.get("ref")), m.clone()))
                })
                .collect();
            for (e, _) in batch {
                match got.get(&e.key) {
                    Some(said) => {
                        self.put_entry(learning, e, said)?;
                        written += 1;
                    }
                    None => missed += 1, // left for the next run, never guessed
                }
            }
        }
        let mut out = Map::new();
        out.insert("session_days".into(), json!(found.len()));
        out.insert("new".into(), json!(todo.len() - grown));
        out.insert("grown".into(), json!(grown));
        out.insert("calls".into(), json!(calls));
        out.insert("written".into(), json!(written));
        out.insert("left_for_next_run".into(), json!(missed));
        out.insert("folders_filled_in".into(), json!(named));
        Ok(out)
    }
}
