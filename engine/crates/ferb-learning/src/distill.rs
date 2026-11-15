//! `distill.py`: the daily distill. For each past day: the permanent summary by code first; one model run (the
//! worker runtime, no tools, typed output) for a narrative and proposed items citing evidence handles; code checks
//! every proposal before `Memory.save`. A model failure leaves the summary in place, is journaled
//! `memory.distill_failed` and the day is tried again on every hourly pass until it is distilled (MEM-421, T-1044:
//! no count limits the tries; unchanged here).

use std::collections::BTreeSet;
use std::rc::Rc;

use serde_json::{json, Value};

use ferb_context::PromptRoot;
use ferb_memory::pystr;
use ferb_memory::terms::{slug, subject_key};
use ferb_memory::times::{self, LocalZone};
use ferb_sanitize::origin;

use crate::calls::{self, learning_call, MakeRuntime, Spans};
use crate::items::{Item, Memory, KINDS};
use crate::journal::Journal;
use crate::notes::memory_origin;
use crate::py;
use crate::sql;
use crate::store::Store;
use crate::summary::{self, DayEvidence, Source};
use crate::text::{capture, query_terms, whole_clauses};
use crate::{Clock, Error, Result};

pub const NO_QUOTE: &str = "no quote of the owner's own words";
pub const OLDER_WORDS: &str = "the owner's words are older than what FERB remembers";
pub const NOT_A_CLAUSE: &str = "the quote isn't a whole clause of the owner's words";
pub const NOTHING_SHARED: &str = "the quote shares no telling word with what FERB remembers";
pub const RETIRED_NOTICE: &str =
    "I retired “{item}” because you said “{quote}” ({day}). Reply to undo.";
pub const REPLACED_NOTICE: &str =
    "I replaced “{item}” because you said “{quote}” ({day}). Reply to undo.";
pub const STALE_NOTICE: &str =
    "Memory: I stopped using “{item}”: it looks out of date ({day}). Tell me if it still holds.";
pub const ASIDE_NOTICE: &str = "Memory: “{item}” looks over ({day}), so I'll stop using it. It's kept; tell me if it still holds and I'll use it again.";
pub const ALREADY_STORED: &str =
    "already remembered (its kind, subject and key hold a current item)";

/// One quiet line in the owner's chat, once per id (the daemon's notice line, journaled `chat.notice`).
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub id: String,
    pub refs: Vec<String>,
    pub text: String,
    pub origin: String,
    /// Note cards, for the import's summary notice.
    pub cards: Option<Value>,
}

/// `str.format` of a template with named fields (values inserted as they are).
pub fn format(template: &str, fields: &[(&str, &str)]) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if fields.iter().any(|(k, _)| *k == &after[..close]) => {
                let name = &after[..close];
                out.push_str(
                    fields
                        .iter()
                        .find(|(k, _)| *k == name)
                        .map_or("", |(_, v)| v),
                );
                rest = &after[close + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// distill.py:62 `DISTILL_SCHEMA`.
pub fn distill_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["narrative", "items"],
        "properties": {
            "narrative": {"type": "string"},
            "items": {"type": "array", "items": {
                "type": "object", "additionalProperties": false,
                "required": ["kind", "subject", "key", "content", "origin", "confidence", "handles", "quote", "retire"],
                "properties": {
                    "kind": {"type": "string", "enum": KINDS},
                    "subject": {"type": "string"}, "key": {"type": "string"}, "content": {"type": "string"},
                    "origin": {"type": "string", "enum": ["observed", "inferred", "third_party"]},
                    "confidence": {"type": "number"},
                    "handles": {"type": "array", "items": {"type": "string"}},
                    "quote": {"anyOf": [{"type": "null"}, {
                        "type": "object", "additionalProperties": false, "required": ["handle", "text"],
                        "properties": {"handle": {"type": "string"}, "text": {"type": "string"}}}]},
                    "retire": {"type": "boolean"}}}}}})
}

fn refused(m: impl Into<String>) -> Error {
    Error::MemoryRefused(m.into())
}

fn text_of(v: Option<&Value>) -> String {
    v.map_or_else(String::new, py::py_str)
}

fn key_error(k: &str) -> Error {
    Error::Crash("KeyError", format!("'{k}'"))
}

fn field<'a>(item: &'a Value, k: &str) -> Result<&'a Value> {
    item.get(k).ok_or_else(|| key_error(k))
}

/// distill.py:79 `check`: the proposal's sources, or MemoryRefused.
pub fn check(item: &Value, ev: &DayEvidence) -> Result<Vec<Source>> {
    let hs: Vec<String> = item
        .get("handles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(py::py_str)
        .collect();
    if hs.is_empty() || hs.iter().any(|h| ev.handle(h).is_none()) {
        return Err(refused("cites evidence that doesn't exist"));
    }
    let kinds: BTreeSet<char> = hs.iter().filter_map(|h| h.chars().next()).collect();
    let subject = item
        .get("subject")
        .map_or_else(|| "owner".into(), py::py_str);
    let about_owner = py::py_str(field(item, "kind")?) == "preference"
        || matches!(pystr::strip(&subject), "" | "owner");
    let origin_ = py::py_str(field(item, "origin")?);
    if origin_ == "observed" && kinds != BTreeSet::from(['E']) {
        return Err(refused(
            "an observed item cites only the owner's own activity",
        ));
    }
    if about_owner && kinds.contains(&'F') {
        return Err(refused(
            "FERB's own activity never becomes the owner's habits or preferences",
        ));
    }
    if origin_ == "third_party" && !kinds.contains(&'P') {
        return Err(refused("a third-party item must cite the third-party text"));
    }
    if origin_ != "third_party" && about_owner && kinds.contains(&'P') {
        return Err(refused("pasted text can't become a fact about the owner"));
    }
    Ok(hs
        .iter()
        .flat_map(|h| ev.handle(h).cloned().unwrap_or_default())
        .collect())
}

/// distill.py:97 `owner_said`: the owner's own words an inferred item rests on, and their message's journal id.
pub fn owner_said(item: &Value, ev: &DayEvidence) -> Option<(String, String)> {
    let quote = item.get("quote").and_then(Value::as_object)?;
    if item.get("origin").and_then(Value::as_str) != Some("inferred") {
        return None;
    }
    let handle = text_of(quote.get("handle"));
    let handles: Vec<String> = item
        .get("handles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(py::py_str)
        .collect();
    if !handles.contains(&handle) {
        return None;
    }
    let (journal_id, typed) = ev.owner_words(&handle)?;
    let got = capture(
        typed,
        &text_of(item.get("content")),
        &text_of(quote.get("text")),
    );
    (got.origin == "owner_stated").then(|| (got.content, journal_id.clone()))
}

/// distill.py:336 `yesterday`.
pub fn yesterday(now: &crate::DateTime, zone: &dyn LocalZone) -> String {
    let local = py::local(now, zone);
    times::ymd(&py::shift(&local, -86_400_000_000))
}

/// distill.py:112 `Distiller`.
pub struct Distiller<'a> {
    pub store: Rc<Store>,
    pub journal: Rc<Journal>,
    pub memory: Rc<Memory>,
    pub make_runtime: Box<MakeRuntime<'a>>,
    pub clock: Rc<dyn Clock>,
    pub zone: Rc<dyn LocalZone>,
    pub prompts: Rc<PromptRoot>,
    pub summary: Rc<dyn summary::Seams>,
    pub purge: Box<dyn Fn() + 'a>,
    pub device_id: String,
    pub record_usage: Box<dyn Fn(&str, &Value) + 'a>,
    pub spans: Option<Rc<dyn Spans + 'a>>,
    pub tell: Box<dyn Fn(Notice) + 'a>,
}

impl Distiller<'_> {
    /// distill.py:124 `pending_days`.
    pub fn pending_days(&self) -> Result<Vec<String>> {
        let today = py::local_day(&self.clock.now(), self.zone.as_ref());
        let mut days: BTreeSet<String> = self
            .store
            .query("SELECT DISTINCT day FROM trail WHERE day < ?", sql![today])?
            .iter()
            .map(|r| r.s("day"))
            .collect();
        for r in self
            .store
            .query("SELECT at FROM journal WHERE kind = 'chat.message'", sql![])?
        {
            let d = py::local_day(&py::parse(&r.s("at"))?, self.zone.as_ref());
            if d < today {
                days.insert(d);
            }
        }
        let done: BTreeSet<String> = self
            .store
            .query(
                "SELECT day FROM distill_runs WHERE status = 'distilled'",
                sql![],
            )?
            .iter()
            .map(|r| r.s("day"))
            .collect();
        let mut out = Vec::new();
        for d in days {
            if !done.contains(&d) || !summary::summarized(&self.store, &d)? {
                out.push(d);
            }
        }
        Ok(out)
    }

    /// distill.py:138 `write_summary`.
    pub fn write_summary(&self, day: &str) -> Result<DayEvidence> {
        let ev = summary::build(&self.store, self.summary.as_ref(), self.zone.as_ref(), day)?;
        if !summary::summarized(&self.store, day)? {
            let mut digest = ev.digest.clone();
            digest["made_by"] = json!(summary::SUMMARY_MADE_BY);
            self.journal
                .append(summary::SUMMARY_KIND, Some(digest), None)?;
        }
        self.run(day, "summarized", None, false)?;
        Ok(ev)
    }

    /// distill.py:145 `run_day`.
    pub async fn run_day(&self, day: &str) -> Result<Value> {
        let ev = self.write_summary(day)?;
        let row = self
            .store
            .one("SELECT * FROM distill_runs WHERE day = ?", sql![day])?
            .ok_or_else(|| {
                Error::Crash("TypeError", "'NoneType' object is not subscriptable".into())
            })?;
        if row.s("status") == "distilled" {
            return Ok(json!({"day": day, "status": row.s("status")}));
        }
        if ev.lines.is_empty() {
            self.run(day, "distilled", Some("nothing to distill"), true)?;
            return Ok(json!({"day": day, "status": "distilled", "proposed": 0}));
        }
        let out = match self.model(day, &ev).await {
            Ok(out) => out,
            Err(e) => {
                let why = e.described();
                self.run(day, "failed", Some(&why), true)?;
                let attempts = self
                    .store
                    .count("SELECT attempts FROM distill_runs WHERE day = ?", sql![day])?;
                self.journal.append(
                    "memory.distill_failed",
                    Some(json!({"day": day, "error": why, "attempts": attempts})),
                    None,
                )?;
                return Ok(json!({"day": day, "status": "failed", "error": why}));
            }
        };
        self.journal.append(
            summary::NARRATIVE_KIND,
            Some(json!({"day": day, "text": text_of(out.get("narrative")), "made_by": text_of(out.get("made_by"))})),
            None,
        )?;
        let (mut proposed, mut refused_list) = (0i64, Vec::<String>::new());
        let mut retired: Vec<String> = Vec::new();
        let mut stale: Vec<Value> = Vec::new();
        let mut guessed: Vec<Value> = Vec::new();
        let due: BTreeSet<String> = self
            .memory
            .due_for_review()?
            .into_iter()
            .map(|it| it.id)
            .collect();
        let items = out
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for item in &items {
            match self
                .one_item(day, item, &ev, &mut retired, &mut stale, &mut guessed)
                .await
            {
                Ok(saved) => proposed += i64::from(saved),
                Err(e) if caught(&e) => refused_list.push(e.to_string()),
                Err(e) => return Err(e),
            }
        }
        for item_id in due.iter().filter(|i| !retired.contains(i)) {
            self.memory.set_review(item_id, None)?;
        }
        let done = json!({"proposed": proposed, "refused": refused_list, "retired": retired,
                          "retired_as_check": stale, "kept_beside": guessed});
        self.run(day, "distilled", Some(&ferb_py::json::dumps(&done)), true)?;
        let mut result = json!({"day": day, "status": "distilled"});
        for (k, v) in done.as_object().into_iter().flatten() {
            result[k] = v.clone();
        }
        Ok(result)
    }

    /// One proposal: true when it was saved.
    async fn one_item(
        &self,
        day: &str,
        item: &Value,
        ev: &DayEvidence,
        retired: &mut Vec<String>,
        stale: &mut Vec<Value>,
        guessed: &mut Vec<Value>,
    ) -> Result<bool> {
        if item.get("retire").is_some_and(ferb_py::value::truthy) {
            check(item, ev)?;
            let target = self
                .current(item)?
                .into_iter()
                .next()
                .ok_or_else(|| refused("it retires nothing FERB remembers"))?;
            let stated = owner_said(item, ev);
            let why = match &stated {
                None => NO_QUOTE.to_owned(),
                Some((_, jid)) => self.may_end(
                    item,
                    ev,
                    jid,
                    std::slice::from_ref(&target),
                    std::slice::from_ref(&target),
                )?,
            };
            if why == OLDER_WORDS {
                return Err(refused(OLDER_WORDS));
            }
            if why.is_empty() {
                self.memory
                    .retire(&target.id, &format!("the owner's words on {day}"))?;
                retired.push(target.id.clone());
                let jid = stated.map(|s| s.1).unwrap_or_default();
                self.tell_owner(RETIRED_NOTICE, "retired", &target, item, &jid)?;
                return Ok(false);
            }
            let outcome = self
                .memory
                .looks_over(&target.id, &format!("FERB's check of {day}: {why}"))?;
            retired.push(target.id.clone());
            stale.push(json!({"item": target.id, "why": why, "outcome": outcome}));
            self.tell_stale(&target, day, outcome == "set_aside");
            return Ok(false);
        }
        let mut srcs = check(item, ev)?;
        let mut origin_ = py::py_str(field(item, "origin")?);
        let mut content = py::py_str(field(item, "content")?);
        let mut confidence = item
            .get("confidence")
            .and_then(Value::as_f64)
            .unwrap_or(0.6);
        let stated = owner_said(item, ev);
        let mut ends = self.current(item)?;
        if !ends.is_empty() && stated.is_none() {
            return Err(refused(ALREADY_STORED));
        }
        let mut why = String::new();
        if let Some((words, jid)) = &stated {
            let same = |e: &Item| {
                e.origin == "owner_stated"
                    && pystr::lower(&e.content) == pystr::lower(pystr::strip(words))
            };
            if ends.iter().any(same) {
                ends.clear();
            }
            if !ends.is_empty() {
                let about: Vec<Item> = ends
                    .iter()
                    .filter(|e| e.origin == "owner_stated")
                    .cloned()
                    .collect();
                why = self.may_end(item, ev, jid, &ends, &about)?;
            }
        }
        let owner_now = stated.is_some() && why.is_empty();
        if owner_now {
            let (words, jid) = stated.clone().expect("stated");
            origin_ = "owner_stated".into();
            content = words;
            confidence = 1.0;
            srcs = vec![("journal".into(), jid)];
        }
        let m = self
            .memory
            .save(
                &py::py_str(field(item, "kind")?),
                &py::py_str(field(item, "subject")?),
                &py::py_str(field(item, "key")?),
                &content,
                &origin_,
                &srcs,
                confidence,
                &[],
                "",
                Some(owner_now),
            )?
            .item;
        if !why.is_empty() {
            guessed.push(json!({"item": m.id, "why": why}));
        } else if let Some((_, jid)) = &stated {
            if !ends.is_empty() && m.origin == "owner_stated" {
                let ended = ends
                    .iter()
                    .find(|e| e.origin == "owner_stated")
                    .unwrap_or(&ends[0])
                    .clone();
                self.tell_owner(REPLACED_NOTICE, "replaced", &ended, item, jid)?;
            }
        }
        Ok(true)
    }

    fn current(&self, item: &Value) -> Result<Vec<Item>> {
        let subject = subject_key(&(self.memory.redact)(&text_of(item.get("subject"))));
        let key = slug(&(self.memory.redact)(&text_of(item.get("key"))), None);
        let kind = item.get("kind");
        Ok(self
            .memory
            .items(Some("active"), Some(&subject), true)?
            .into_iter()
            .filter(|i| kind == Some(&json!(i.kind)) && i.key == key)
            .collect())
    }

    fn may_end(
        &self,
        item: &Value,
        ev: &DayEvidence,
        journal_id: &str,
        ends: &[Item],
        about: &[Item],
    ) -> Result<String> {
        let Some(said) = self.said_at(journal_id)? else {
            return Ok(OLDER_WORDS.into());
        };
        for e in ends {
            if !py::after(&said, &e.valid_from)? {
                return Ok(OLDER_WORDS.into());
            }
        }
        let quote = field(item, "quote")?;
        let text = text_of(quote.get("text"));
        if !about.is_empty() {
            let handle = text_of(quote.get("handle"));
            let typed = ev.owner_words(&handle).ok_or_else(|| key_error(&handle))?;
            if !whole_clauses(&typed.1, &text) {
                return Ok(NOT_A_CLAUSE.into());
            }
        }
        let telling: BTreeSet<String> = self
            .memory
            .telling_words(&text, None)?
            .into_iter()
            .collect();
        if about
            .iter()
            .any(|e| !query_terms(&e.content).iter().any(|t| telling.contains(t)))
        {
            return Ok(NOTHING_SHARED.into());
        }
        Ok(String::new())
    }

    fn said_at(&self, journal_id: &str) -> Result<Option<crate::DateTime>> {
        match self
            .store
            .one("SELECT at FROM journal WHERE id = ?", sql![journal_id])?
        {
            Some(r) => Ok(Some(py::parse(&r.s("at"))?)),
            None => Ok(None),
        }
    }

    fn tell_owner(
        &self,
        template: &str,
        verb: &str,
        ended: &Item,
        item: &Value,
        journal_id: &str,
    ) -> Result<()> {
        let said = self.said_at(journal_id)?;
        let quote = item
            .get("quote")
            .map_or_else(String::new, |q| text_of(q.get("text")));
        let day = said.map_or_else(
            || "today".into(),
            |s| py::short_day(&py::local(&s, self.zone.as_ref())),
        );
        let text = format(
            template,
            &[
                ("item", &squeeze(&ended.content)),
                ("quote", &squeeze(&quote)),
                ("day", &day),
            ],
        );
        let labels = [
            origin::FERB.to_owned(),
            origin::OWNER.to_owned(),
            memory_origin(&ended.origin),
        ];
        (self.tell)(Notice {
            id: format!("memory-{verb}-{}", ended.id),
            refs: vec![
                format!("memory:{}", ended.id),
                format!("journal:{journal_id}"),
            ],
            text,
            origin: origin::join(labels.iter().map(String::as_str)),
            cards: None,
        });
        Ok(())
    }

    fn tell_stale(&self, ended: &Item, day: &str, aside: bool) {
        let text = format(
            if aside { ASIDE_NOTICE } else { STALE_NOTICE },
            &[("item", &squeeze(&ended.content)), ("day", day)],
        );
        let labels = [
            origin::FERB.to_owned(),
            origin::INFERRED.to_owned(),
            memory_origin(&ended.origin),
        ];
        (self.tell)(Notice {
            id: format!("memory-stale-{}", ended.id),
            refs: vec![format!("memory:{}", ended.id)],
            text,
            origin: origin::join(labels.iter().map(String::as_str)),
            cards: None,
        });
    }

    /// distill.py:293 `catch_up`.
    pub async fn catch_up(&self) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for d in self.pending_days()? {
            out.push(self.run_day(&d).await?);
        }
        (self.purge)();
        Ok(out)
    }

    fn run(&self, day: &str, status: &str, detail: Option<&str>, bump: bool) -> Result<()> {
        let now = py::iso(&self.clock.now());
        self.store.exec(
            "INSERT INTO distill_runs (day, attempts, status, at, detail) VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(day) DO UPDATE SET attempts = attempts + ?, status = CASE WHEN distill_runs.status = \
             'distilled' THEN 'distilled' ELSE excluded.status END, at = excluded.at, detail = COALESCE(excluded.detail, \
             distill_runs.detail)",
            sql![day, i64::from(bump), status, now, detail.map(str::to_owned), i64::from(bump)],
        )?;
        Ok(())
    }

    async fn model(&self, day: &str, ev: &DayEvidence) -> Result<Value> {
        let line = |i: &Item| format!("- {} / {} / {}: {}", i.kind, i.subject, i.key, i.content);
        let known: Vec<String> = self
            .memory
            .items(Some("active"), None, true)?
            .iter()
            .map(line)
            .collect();
        let pending: Vec<String> = self
            .memory
            .items(Some("proposed"), None, true)?
            .iter()
            .filter(|i| i.origin == "observed")
            .map(line)
            .collect();
        let due: Vec<String> = self
            .memory
            .due_for_review()?
            .iter()
            .map(|i| {
                format!(
                    "{} (stated {})",
                    line(i),
                    times::ymd(&py::local(&i.valid_from, self.zone.as_ref()))
                )
            })
            .collect();
        let or = |v: &[String], none: &str| {
            if v.is_empty() {
                none.to_owned()
            } else {
                v.join("\n")
            }
        };
        let ctx = format!(
            "Day: {day}\n\nEvidence (cite handles):\n{}\n\nWhat FERB already remembers (don't propose these again; \
             retire one that no longer holds):\n{}\n\nPatterns seen before, not yet confirmed (if today's evidence \
             shows one again, propose it with the same key):\n{}\n\nDue for a check (FERB set a date to check these \
             still hold; retire one that is over):\n{}",
            ev.lines.join("\n"),
            or(&known, "(nothing)"),
            or(&pending, "(none)"),
            or(&due, "(none)")
        );
        let schema = distill_schema();
        let made = (self.make_runtime)("worker", "supervisor", &[], &schema)?;
        let task = calls::learning_task(
            &format!("distill-{day}"),
            "distill",
            &self.device_id,
            &format!("distill-{day}"),
        );
        let system = self
            .prompts
            .load("memory/distill.md", &[])
            .map_err(|e| Error::Crash("OSError", e.to_string()))?;
        let usage = |name: &str, data: &Value| (self.record_usage)(name, data);
        let text = learning_call(
            &made,
            &task,
            &system,
            &ctx,
            &format!("daily evidence {day}"),
            &schema,
            "distill",
            self.spans.as_deref(),
            &usage,
        )
        .await?;
        let mut parsed = calls::loads(&text)?;
        if !parsed.is_object() {
            return Err(Error::Value("not an object".into()));
        }
        parsed["made_by"] = json!(format!(
            "model:{}:{}|{}",
            made.rt.name(),
            made.model
                .as_deref()
                .filter(|m| !m.is_empty())
                .unwrap_or("default"),
            calls::prompt_tag("memory/distill.md", &system)
        ));
        Ok(parsed)
    }
}

fn squeeze(s: &str) -> String {
    pystr::split_ws(s).collect::<Vec<_>>().join(" ")
}

/// distill.py:235: what a proposal's refusal is (`MemoryRefused`, `KeyError`, `ValueError`).
fn caught(e: &Error) -> bool {
    matches!(
        e,
        Error::MemoryRefused(_) | Error::Value(_) | Error::Json(_) | Error::InvalidAnswer(_)
    ) || matches!(e, Error::Crash(c, _) if *c == "KeyError" || *c == "ValueError")
}
