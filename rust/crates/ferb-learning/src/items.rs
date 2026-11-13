//! `items.py`'s `Memory`: the shared save path every memory item enters by, its origin rules, versions, end dates,
//! set-aside and restore (MEM-14), and `forget` (MEM-2, MEM-11). SQLite is the source of truth; nothing is
//! overwritten (a newer version closes the old one); the owner's words never end because of inferred text.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use serde_json::{json, Value};

use ferb_memory::pystr;
use ferb_memory::terms::{self, label, slug, subject_key};
use ferb_memory::times::{self, LocalZone};

use crate::py;
use crate::sql;
use crate::store::{marks, Row, Sql, Store};
use crate::text::{contains_words, fingerprint, query_terms};
use crate::{Clock, DateTime, Error, Ids, Result};

pub const KINDS: [&str; 5] = terms::KINDS;
pub const ORIGINS: [&str; 4] = ["owner_stated", "observed", "inferred", "third_party"];
pub const OBSERVED_DAYS: i64 = 3;
pub const MAC_PROFILE_KIND: &str = "memory.mac_profile";
const SQL_SLACK: i64 = terms::SQL_SLACK_MICROS;

/// A source: ("journal" | "trail", id).
pub type Source = (String, String);

fn refused(m: impl Into<String>) -> Error {
    Error::MemoryRefused(m.into())
}

/// `MemoryItem` (ferb/core/types.py).
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub kind: String,
    pub subject: String,
    pub key: String,
    pub content: String,
    pub source_ids: Vec<String>,
    pub observed_at: DateTime,
    pub valid_from: DateTime,
    pub valid_to: Option<DateTime>,
    pub supersedes: Option<String>,
    pub confidence: f64,
    pub origin: String,
    pub status: String,
    pub about: String,
    pub review_at: Option<DateTime>,
    pub set_aside: bool,
}

impl Item {
    pub fn json(&self) -> Value {
        let iso = |d: &Option<DateTime>| d.as_ref().map(py::iso);
        json!({"id": self.id, "kind": self.kind, "subject": self.subject, "key": self.key, "content": self.content,
               "source_ids": self.source_ids, "observed_at": py::iso(&self.observed_at),
               "valid_from": py::iso(&self.valid_from), "valid_to": iso(&self.valid_to),
               "supersedes": self.supersedes, "confidence": self.confidence, "origin": self.origin,
               "status": self.status, "about": self.about, "review_at": iso(&self.review_at),
               "set_aside": self.set_aside})
    }
}

/// items.py:211 `current`: no end, or an end still ahead.
pub fn current(item: &Item, now: &DateTime) -> Result<bool> {
    match &item.valid_to {
        None => Ok(true),
        Some(end) => py::after(end, now),
    }
}

/// items.py:217 `prompt_ready`.
pub fn prompt_ready(item: &Item, now: &DateTime) -> Result<bool> {
    Ok(current(item, now)?
        && !item.set_aside
        && (item.status == "active" || (item.status == "proposed" && item.origin == "observed")))
}

/// items.py:242 `Saved`.
#[derive(Clone, Debug)]
pub struct Saved {
    pub item: Item,
    pub replaced: Vec<Item>,
    pub not_replaced: Vec<String>,
}

/// items.py:264 `ForgetPlan`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ForgetPlan {
    pub items: Vec<String>,
    pub trail: Vec<String>,
    pub rebuild_days: Vec<String>,
    pub delete_summaries: Vec<String>,
    pub kept: i64,
    pub destinations: Vec<(String, String)>,
}

impl ForgetPlan {
    /// items.py:273 `describe`.
    pub fn describe(&self, store: &Store, plain: bool) -> Result<Vec<String>> {
        let mut out = Vec::new();
        if !self.items.is_empty() {
            let q = format!(
                "SELECT content, kind, subject, key FROM memory_items WHERE id IN ({}) ORDER BY seq",
                marks(self.items.len())
            );
            for r in store.query(
                &q,
                self.items.iter().map(|i| Sql::Text(i.clone())).collect(),
            )? {
                let said = format!(
                    "“{}”",
                    pystr::split_ws(&r.s("content"))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                out.push(if plain {
                    said
                } else {
                    format!(
                        "memory item {said} ({} · {} · {})",
                        r.s("kind"),
                        r.s("subject"),
                        r.s("key")
                    )
                });
            }
        }
        if !self.trail.is_empty() {
            out.push(format!(
                "{} recorded activity row(s) it was learned from",
                self.trail.len()
            ));
        }
        if !self.rebuild_days.is_empty() {
            out.push(format!(
                "daily summaries rebuilt without them: {}",
                self.rebuild_days.join(", ")
            ));
        }
        if !self.delete_summaries.is_empty() {
            out.push(format!(
                "daily summaries deleted (raw rows already gone): {}",
                self.delete_summaries.join(", ")
            ));
        }
        if !self.destinations.is_empty() {
            let names: Vec<&str> = self.destinations.iter().map(|(_, v)| v.as_str()).collect();
            out.push(format!(
                "FERB's record of having sent to {} (sending there asks you again)",
                names.join(", ")
            ));
        }
        Ok(out)
    }

    /// items.py:293 `keeps`.
    pub fn keeps(&self) -> &'static str {
        if self.kept > 0 {
            "Your own messages stay in the chat history; FERB won't learn it from them again."
        } else {
            "FERB won't learn it again."
        }
    }
}

/// What other parts are told (items.py:304-315); the daemon wires them.
pub struct Hooks {
    pub on_change: Box<dyn Fn()>,
    pub rebuild_summary: Box<dyn Fn(&str) -> Result<bool>>,
    pub on_forget: Box<dyn Fn(&ForgetPlan) -> Result<()>>,
    pub on_settled: Box<dyn Fn(&[String], bool)>,
    pub on_ended: Box<dyn Fn(&[String], Option<&str>) -> Result<()>>,
    pub on_saved: Box<dyn Fn(&Item) -> Result<()>>,
    pub on_set_aside: Box<dyn Fn(&[String]) -> Result<()>>,
}

impl Default for Hooks {
    fn default() -> Hooks {
        Hooks {
            on_change: Box::new(|| {}),
            rebuild_summary: Box::new(|_| Ok(false)),
            on_forget: Box::new(|_| Ok(())),
            on_settled: Box::new(|_, _| {}),
            on_ended: Box::new(|_, _| Ok(())),
            on_saved: Box::new(|_| Ok(())),
            on_set_aside: Box::new(|_| Ok(())),
        }
    }
}

/// items.py:299 `Memory`.
pub struct Memory {
    pub store: Rc<Store>,
    pub clock: Rc<dyn Clock>,
    pub ids: Rc<dyn Ids>,
    pub zone: Rc<dyn LocalZone>,
    pub publish: Box<dyn Fn(&str, Value)>,
    pub redact: Box<dyn Fn(&str) -> String>,
    pub hooks: RefCell<Hooks>,
}

impl Memory {
    pub fn new(
        store: Rc<Store>,
        clock: Rc<dyn Clock>,
        ids: Rc<dyn Ids>,
        zone: Rc<dyn LocalZone>,
        publish: Box<dyn Fn(&str, Value)>,
        redact: Box<dyn Fn(&str) -> String>,
    ) -> Memory {
        Memory {
            store,
            clock,
            ids,
            zone,
            publish,
            redact,
            hooks: RefCell::new(Hooks::default()),
        }
    }

    fn changed(&self) {
        (self.hooks.borrow().on_change)();
    }

    fn ended(&self, ids: &[String], new: Option<&str>) -> Result<()> {
        (self.hooks.borrow().on_ended)(ids, new)
    }

    fn saved_hook(&self, it: &Item) -> Result<()> {
        (self.hooks.borrow().on_saved)(it)
    }

    // ---------- reads ----------

    pub fn get(&self, item_id: &str) -> Result<Option<Item>> {
        match self
            .store
            .one("SELECT * FROM memory_items WHERE id = ?", sql![item_id])?
        {
            Some(r) => Ok(Some(self.item(&r)?)),
            None => Ok(None),
        }
    }

    fn item(&self, r: &Row) -> Result<Item> {
        let id = r.s("id");
        let srcs = self
            .store
            .query(
                "SELECT kind, source_id FROM memory_sources WHERE item_id = ? ORDER BY kind, source_id",
                sql![id],
            )?
            .iter()
            .map(|s| format!("{}:{}", s.s("kind"), s.s("source_id")))
            .collect();
        let extra = self.store.one(
            "SELECT about, review_at FROM memory_context WHERE item_id = ?",
            sql![id],
        )?;
        let aside = self
            .store
            .exists("SELECT 1 FROM memory_set_aside WHERE item_id = ?", sql![id])?;
        let opt_time = |v: Option<String>| -> Result<Option<DateTime>> {
            match v {
                Some(t) if !t.is_empty() => Ok(Some(py::parse(&t)?)),
                _ => Ok(None),
            }
        };
        Ok(Item {
            id: id.clone(),
            kind: r.s("kind"),
            subject: r.s("subject"),
            key: r.s("key"),
            content: r.s("content"),
            source_ids: srcs,
            observed_at: py::parse(&r.s("observed_at"))?,
            valid_from: py::parse(&r.s("valid_from"))?,
            valid_to: opt_time(r.opt("valid_to"))?,
            supersedes: r.opt("supersedes"),
            confidence: r.f("confidence"),
            origin: r.s("origin"),
            status: r.s("status"),
            about: extra.as_ref().map_or_else(String::new, |e| e.s("about")),
            review_at: opt_time(extra.as_ref().and_then(|e| e.opt("review_at")))?,
            set_aside: aside,
        })
    }

    fn slack(&self, now: &DateTime) -> String {
        py::iso(&py::shift(now, -SQL_SLACK))
    }

    /// items.py:337 `items`.
    pub fn items(
        &self,
        status: Option<&str>,
        subject: Option<&str>,
        valid_now: bool,
    ) -> Result<Vec<Item>> {
        let (mut q, mut args) = (
            "SELECT * FROM memory_items WHERE 1=1".to_owned(),
            Vec::new(),
        );
        if let Some(s) = status.filter(|s| !s.is_empty()) {
            q.push_str(" AND status = ?");
            args.push(Sql::Text(s.into()));
        }
        if let Some(s) = subject.filter(|s| !s.is_empty()) {
            q.push_str(" AND subject = ?");
            args.push(Sql::Text(s.into()));
        }
        let now = self.clock.now();
        if valid_now {
            q.push_str(" AND (valid_to IS NULL OR valid_to > ?)");
            args.push(Sql::Text(self.slack(&now)));
        }
        let mut out = Vec::new();
        for r in self
            .store
            .query(&(q + " ORDER BY subject, kind, key, seq"), args)?
        {
            let it = self.item(&r)?;
            if !valid_now || current(&it, &now)? {
                out.push(it);
            }
        }
        Ok(out)
    }

    /// items.py:349 `current_items`.
    pub fn current_items(&self) -> Result<Vec<Item>> {
        let now = self.clock.now();
        let mut out = Vec::new();
        for it in self.items(None, None, true)? {
            if prompt_ready(&it, &now)? {
                out.push(it);
            }
        }
        Ok(out)
    }

    /// items.py:354 `history`.
    pub fn history(&self, item_id: &str) -> Result<Vec<Item>> {
        let Some(it) = self.get(item_id)? else {
            return Ok(Vec::new());
        };
        self.store
            .query(
                "SELECT * FROM memory_items WHERE kind = ? AND subject = ? AND key = ? ORDER BY seq",
                sql![it.kind, it.subject, it.key],
            )?
            .iter()
            .map(|r| self.item(r))
            .collect()
    }

    /// items.py:363 `telling_words`.
    pub fn telling_words(&self, text: &str, at: Option<&DateTime>) -> Result<Vec<String>> {
        let when = py::iso(&at.copied().unwrap_or_else(|| self.clock.now()));
        let terms = query_terms(text);
        let valid =
            "m.status = 'active' AND valid_from <= ? AND (valid_to IS NULL OR valid_to > ?)";
        let total = if terms.is_empty() {
            0
        } else {
            self.store.count(
                &format!("SELECT COUNT(*) FROM memory_items m WHERE {valid}"),
                sql![when, when],
            )?
        };
        if total == 0 {
            return Ok(Vec::new());
        }
        terms::weighted(&terms, total, |term| {
            self.store.count(
                &format!(
                    "SELECT COUNT(*) FROM memory_fts f JOIN memory_items m ON m.seq = f.rowid \
                     WHERE memory_fts MATCH ? AND {valid}"
                ),
                sql![format!("\"{term}\""), when, when],
            )
        })
    }

    /// items.py:379 `current_ids`.
    pub fn current_ids(&self, origin: Option<&str>) -> Result<BTreeSet<String>> {
        let now = self.clock.now();
        let mut q = "SELECT id, valid_to FROM memory_items WHERE status = 'active' \
                     AND (valid_to IS NULL OR valid_to > ?)"
            .to_owned();
        let mut args = sql![self.slack(&now)];
        if let Some(o) = origin.filter(|o| !o.is_empty()) {
            q.push_str(" AND origin = ?");
            args.push(Sql::Text(o.into()));
        }
        let mut out = BTreeSet::new();
        for r in self.store.query(&q, args)? {
            let keep = match r.opt("valid_to") {
                Some(t) if !t.is_empty() => py::after(&py::parse(&t)?, &now)?,
                _ => true,
            };
            if keep {
                out.insert(r.s("id"));
            }
        }
        Ok(out)
    }

    /// items.py:390 `_versions`.
    fn versions(&self, kind: &str, subject: &str, key: &str) -> Result<Vec<Item>> {
        let when = py::iso(&self.clock.now());
        self.store
            .query(
                "SELECT * FROM memory_items WHERE kind = ? AND subject = ? AND key = ? AND status = 'active' \
                 AND valid_from <= ? AND (valid_to IS NULL OR valid_to > ?)",
                sql![kind, subject, key, when, when],
            )?
            .iter()
            .map(|r| self.item(r))
            .collect()
    }

    /// items.py:398 `evidence_days`.
    pub fn evidence_days(&self, item_id: &str) -> Result<i64> {
        self.store.count(
            "SELECT COUNT(DISTINCT day) AS n FROM memory_sources WHERE item_id = ? AND kind = 'trail' \
             AND actor = 'owner'",
            sql![item_id],
        )
    }

    fn mac_sources_only(&self, item_id: &str) -> Result<bool> {
        let rows = self.store.query(
            "SELECT s.kind, j.kind AS jkind FROM memory_sources s LEFT JOIN journal j ON j.id = s.source_id \
             AND s.kind = 'journal' WHERE s.item_id = ?",
            sql![item_id],
        )?;
        Ok(!rows.is_empty()
            && rows.iter().all(|r| {
                r.s("kind") == "journal" && r.opt("jkind").as_deref() == Some(MAC_PROFILE_KIND)
            }))
    }

    /// items.py:410 `from_mac_scan`.
    pub fn from_mac_scan(&self, item_id: &str) -> Result<bool> {
        self.store.exists(
            "SELECT 1 FROM memory_sources s JOIN journal j ON j.id = s.source_id WHERE s.item_id = ? \
             AND s.kind = 'journal' AND j.kind = ? LIMIT 1",
            sql![item_id, MAC_PROFILE_KIND],
        )
    }

    // ---------- writes ----------

    /// items.py:420 `propose`.
    pub fn propose(
        &self,
        kind: &str,
        subject: &str,
        key: &str,
        content: &str,
        origin: &str,
        sources: &[Source],
        confidence: f64,
    ) -> Result<Item> {
        Ok(self
            .save(
                kind,
                subject,
                key,
                content,
                origin,
                sources,
                confidence,
                &[],
                "",
                None,
            )?
            .item)
    }

    /// items.py:425 `save`: every item enters here.
    #[allow(clippy::too_many_arguments)]
    pub fn save(
        &self,
        kind: &str,
        subject: &str,
        key: &str,
        content: &str,
        origin: &str,
        sources: &[Source],
        confidence: f64,
        replaces: &[String],
        about: &str,
        from_owner: Option<bool>,
    ) -> Result<Saved> {
        let kind = pystr::strip(kind).to_owned();
        let subject = subject_key(&(self.redact)(subject));
        let key = slug(&(self.redact)(key), None);
        let content = (self.redact)(pystr::strip(content));
        let mut srcs: Vec<Source> = Vec::new();
        for s in sources {
            if !srcs.contains(s) {
                srcs.push(s.clone());
            }
        }
        let owner_now = from_owner.unwrap_or(origin == "owner_stated");
        self.check(&kind, &subject, &key, &content, origin, &srcs)?;
        let now = py::iso(&self.clock.now());
        let mut same = self
            .store
            .one(
                "SELECT id FROM memory_items WHERE kind = ? AND subject = ? AND key = ? AND lower(content) = lower(?) \
                 AND (origin = ? OR origin = 'owner_stated') AND status != 'retracted' AND valid_to IS NULL \
                 ORDER BY origin = 'owner_stated' DESC, seq DESC",
                sql![kind, subject, key, content, origin],
            )?
            .map(|r| r.s("id"));
        if same.is_none() && origin == "observed" {
            same = self
                .store
                .one(
                    "SELECT id FROM memory_items WHERE kind = ? AND subject = ? AND key = ? AND origin = 'observed' \
                     AND status = 'proposed' AND valid_to IS NULL ORDER BY seq LIMIT 1",
                    sql![kind, subject, key],
                )?
                .map(|r| r.s("id"));
        }
        let item_id = match &same {
            Some(id) => {
                if owner_now {
                    self.restore(id)?;
                }
                id.clone()
            }
            None => {
                let id = self.ids.new_id();
                self.store.exec(
                    "INSERT INTO memory_items (id, kind, subject, key, content, origin, status, confidence, observed_at, \
                     valid_from, created_at) VALUES (?, ?, ?, ?, ?, ?, 'proposed', ?, ?, ?, ?)",
                    sql![id, kind, subject, key, content, origin, confidence, now, now, now],
                )?;
                (self.publish)(
                    "memory.proposed",
                    json!({"id": id, "kind": kind, "subject": subject, "origin": origin}),
                );
                id
            }
        };
        self.add_sources(&item_id, &srcs)?;
        if !pystr::strip(about).is_empty() {
            self.set_about(&item_id, about)?;
        }
        let mut wanted: Vec<String> = Vec::new();
        for i in replaces {
            if !i.is_empty() && *i != item_id && !wanted.contains(i) {
                wanted.push(i.clone());
            }
        }
        let (named, refused_ids) = if owner_now {
            self.replaceable(&wanted)?
        } else {
            (Vec::new(), wanted.clone())
        };
        let mut replaced: Vec<Item> = Vec::new();
        let commits = matches!(origin, "owner_stated" | "inferred" | "third_party")
            || (origin == "observed"
                && (self.evidence_days(&item_id)? >= OBSERVED_DAYS
                    || self.mac_sources_only(&item_id)?));
        if commits {
            let reason = match origin {
                "owner_stated" => "owner stated",
                "inferred" => "FERB's understanding",
                "third_party" => "a fact about the world",
                _ => "observed",
            };
            replaced = self.commit_one(&item_id, reason, owner_now, &named)?;
            if same.is_some() && !named.is_empty() {
                let done: BTreeSet<String> = replaced.iter().map(|r| r.id.clone()).collect();
                let rest: Vec<Item> = named
                    .iter()
                    .filter(|o| !done.contains(&o.id))
                    .cloned()
                    .collect();
                replaced.extend(self.end(&rest, &item_id)?);
            }
        }
        let it = self
            .get(&item_id)?
            .ok_or_else(|| Error::Crash("AssertionError", String::new()))?;
        if same.is_some() && it.status == "active" && replaced.is_empty() {
            self.saved_hook(&it)?;
        }
        Ok(Saved {
            item: it,
            replaced,
            not_replaced: refused_ids,
        })
    }

    fn replaceable(&self, ids: &[String]) -> Result<(Vec<Item>, Vec<String>)> {
        let now = self.clock.now();
        let (mut ok, mut refused_ids) = (Vec::new(), Vec::new());
        for i in ids {
            match self.get(i)? {
                None => refused_ids.push(i.clone()),
                Some(it) if it.status != "active" || !current(&it, &now)? => {
                    refused_ids.push(i.clone())
                }
                Some(it) => ok.push(it),
            }
        }
        Ok((ok, refused_ids))
    }

    fn trail_rows(&self, ids: &[String]) -> Result<Vec<Row>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        self.store.query(
            &format!(
                "SELECT id, day, actor FROM trail WHERE id IN ({})",
                marks(ids.len())
            ),
            ids.iter().map(|i| Sql::Text(i.clone())).collect(),
        )
    }

    fn check(
        &self,
        kind: &str,
        subject: &str,
        key: &str,
        content: &str,
        origin: &str,
        srcs: &[Source],
    ) -> Result<()> {
        if !KINDS.contains(&kind) || !ORIGINS.contains(&origin) {
            return Err(refused(format!("unknown kind / origin {kind} / {origin}")));
        }
        if content.is_empty() {
            return Err(refused("empty content"));
        }
        if srcs.is_empty() {
            return Err(refused("an item needs at least one source"));
        }
        let trail: Vec<String> = srcs
            .iter()
            .filter(|s| s.0 == "trail")
            .map(|s| s.1.clone())
            .collect();
        let rows = self.trail_rows(&trail)?;
        if rows.len() != trail.len() {
            return Err(refused("a trail source doesn't exist"));
        }
        let actors: BTreeSet<String> = rows.iter().map(|r| r.s("actor")).collect();
        if origin == "observed" {
            let journal: Vec<&String> = srcs
                .iter()
                .filter(|s| s.0 == "journal")
                .map(|s| &s.1)
                .collect();
            let mut mac = !journal.is_empty() && journal.len() == srcs.len();
            if mac {
                for j in &journal {
                    let r = self
                        .store
                        .one("SELECT kind FROM journal WHERE id = ?", sql![*j])?;
                    if r.is_none_or(|r| r.s("kind") != MAC_PROFILE_KIND) {
                        mac = false;
                        break;
                    }
                }
            }
            if !mac
                && (trail.is_empty()
                    || trail.len() != srcs.len()
                    || actors != BTreeSet::from(["owner".to_owned()]))
            {
                return Err(refused(
                    "an observed item needs the owner's own trail rows only (never FERB's activity)",
                ));
            }
        }
        if matches!(origin, "observed" | "inferred")
            && actors.contains("ferb")
            && (kind == "preference" || subject == "owner")
        {
            return Err(refused(
                "FERB's own activity never becomes the owner's habits or preferences",
            ));
        }
        if origin == "third_party" && (kind == "preference" || subject == "owner") {
            return Err(refused(
                "third-party content can't become a preference or a fact about the owner",
            ));
        }
        if origin != "owner_stated"
            && self.store.exists(
                "SELECT 1 FROM memory_forgotten WHERE fingerprint = ?",
                sql![fingerprint(kind, subject, key, content)],
            )?
        {
            return Err(refused("the owner asked FERB to forget this"));
        }
        Ok(())
    }

    fn add_sources(&self, item_id: &str, srcs: &[Source]) -> Result<()> {
        let trail_ids: Vec<String> = srcs
            .iter()
            .filter(|s| s.0 == "trail")
            .map(|s| s.1.clone())
            .collect();
        let trail = self.trail_rows(&trail_ids)?;
        for (kind, sid) in srcs {
            let (mut day, mut actor) = (None, None);
            if kind == "trail" {
                if let Some(r) = trail.iter().find(|r| r.s("id") == *sid) {
                    day = r.opt("day");
                    actor = r.opt("actor");
                }
            } else if kind == "journal" {
                if let Some(r) = self
                    .store
                    .one("SELECT at FROM journal WHERE id = ?", sql![sid])?
                {
                    day = Some(py::local_day(&py::parse(&r.s("at"))?, self.zone.as_ref()));
                }
            }
            self.store.exec(
                "INSERT OR IGNORE INTO memory_sources (item_id, kind, source_id, day, actor) VALUES (?, ?, ?, ?, ?)",
                sql![item_id, kind, sid, day, actor],
            )?;
        }
        Ok(())
    }

    /// items.py:539 `commit`: proposed -> active.
    pub fn commit(&self, ids: &[String], reason: &str) -> Result<()> {
        for id in ids {
            if let Some(it) = self.get(id)? {
                if it.status != "active" {
                    self.commit_one(id, reason, it.origin == "owner_stated", &[])?;
                }
            }
        }
        Ok(())
    }

    fn commit_one(
        &self,
        item_id: &str,
        reason: &str,
        owner_now: bool,
        named: &[Item],
    ) -> Result<Vec<Item>> {
        let Some(it) = self.get(item_id)? else {
            return Ok(Vec::new());
        };
        if it.status == "active" {
            return Ok(Vec::new());
        }
        let now = py::iso(&self.clock.now());
        let prev: Vec<Item> = self
            .versions(&it.kind, &it.subject, &it.key)?
            .into_iter()
            .filter(|o| o.id != it.id)
            .collect();
        let mut closes: Vec<Item> = prev
            .into_iter()
            .filter(|o| owner_now || o.origin != "owner_stated")
            .collect();
        for o in named {
            if !closes.iter().any(|c| c.id == o.id) {
                closes.push(o.clone());
            }
        }
        for o in &closes {
            self.store.exec(
                "UPDATE memory_items SET valid_to = ? WHERE id = ?",
                sql![now, o.id],
            )?;
        }
        let same_fact: Vec<&Item> = closes
            .iter()
            .filter(|o| (&o.kind, &o.subject, &o.key) == (&it.kind, &it.subject, &it.key))
            .collect();
        let supersedes = if closes.is_empty() {
            None
        } else if let Some(last) = same_fact.last() {
            Some(last.id.clone())
        } else {
            closes.last().map(|c| c.id.clone())
        };
        self.store.exec(
            "UPDATE memory_items SET status = 'active', valid_from = ?, supersedes = ? WHERE id = ?",
            sql![now, supersedes, item_id],
        )?;
        (self.publish)("memory.committed", json!({"id": item_id, "reason": reason}));
        if !closes.is_empty() {
            let ids: Vec<String> = closes.iter().map(|c| c.id.clone()).collect();
            self.ended(&ids, Some(item_id))?;
        }
        let saved = self
            .get(item_id)?
            .ok_or_else(|| Error::Crash("AssertionError", String::new()))?;
        self.saved_hook(&saved)?;
        self.changed();
        Ok(closes)
    }

    fn end(&self, items: &[Item], by: &str) -> Result<Vec<Item>> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let now = py::iso(&self.clock.now());
        for o in items {
            self.store.exec(
                "UPDATE memory_items SET valid_to = ? WHERE id = ?",
                sql![now, o.id],
            )?;
        }
        let ids: Vec<String> = items.iter().map(|c| c.id.clone()).collect();
        self.ended(&ids, Some(by))?;
        self.changed();
        Ok(items.to_vec())
    }

    /// items.py:582 `set_about` (MEM-10).
    pub fn set_about(&self, item_id: &str, about: &str) -> Result<()> {
        let text = pystr::split_ws(&(self.redact)(about))
            .collect::<Vec<_>>()
            .join(" ");
        self.store.exec(
            "INSERT INTO memory_context (item_id, about) VALUES (?, ?) \
             ON CONFLICT(item_id) DO UPDATE SET about = excluded.about",
            sql![item_id, pystr::prefix(&text, 200)],
        )?;
        Ok(())
    }

    /// `at.astimezone(self.clock.now().tzinfo)`.
    fn in_clock_zone(&self, at: &DateTime) -> DateTime {
        let now = self.clock.now();
        let offset = now
            .offset
            .map_or(0, |(s, us)| i64::from(s) * 1_000_000 + i64::from(us));
        times::from_wall_micros(times::instant(at) + offset, now.offset).unwrap_or(*at)
    }

    /// items.py:588 `set_review`.
    pub fn set_review(&self, item_id: &str, at: Option<&DateTime>) -> Result<()> {
        let stamp = at.map(|a| py::iso(&self.in_clock_zone(a)));
        self.store.exec(
            "INSERT INTO memory_context (item_id, review_at) VALUES (?, ?) \
             ON CONFLICT(item_id) DO UPDATE SET review_at = excluded.review_at",
            sql![item_id, stamp],
        )?;
        Ok(())
    }

    /// items.py:594 `due_for_review`.
    pub fn due_for_review(&self) -> Result<Vec<Item>> {
        let now = self.clock.now();
        let mut due = Vec::new();
        for r in self.store.query(
            "SELECT item_id FROM memory_context WHERE review_at IS NOT NULL",
            sql![],
        )? {
            let Some(it) = self.get(&r.s("item_id"))? else {
                continue;
            };
            let Some(review) = it.review_at else { continue };
            if !py::after(&review, &now)? && it.status == "active" && current(&it, &now)? {
                due.push(it);
            }
        }
        due.sort_by_key(|it| {
            it.review_at
                .map_or(times::instant(&now), |r| times::instant(&r))
        });
        Ok(due)
    }

    /// items.py:602 `retract`.
    pub fn retract(&self, item_id: &str, _reason: &str, never_again: bool) -> Result<()> {
        let it = self
            .get(item_id)?
            .ok_or_else(|| refused(format!("no item {item_id}")))?;
        self.store.exec(
            "UPDATE memory_items SET status = 'retracted' WHERE id = ?",
            sql![item_id],
        )?;
        if never_again {
            self.never_again(&it)?;
        }
        self.ended(&[item_id.to_owned()], None)?;
        (self.hooks.borrow().on_settled)(&[item_id.to_owned()], false);
        self.changed();
        Ok(())
    }

    /// items.py:615 `expire_at`.
    pub fn expire_at(&self, item_id: &str, at: &DateTime) -> Result<()> {
        let stamp = py::iso(&self.in_clock_zone(at));
        self.store.exec(
            "UPDATE memory_items SET valid_to = ? WHERE id = ? AND valid_to IS NULL",
            sql![stamp, item_id],
        )?;
        (self.publish)("memory.expires", json!({"id": item_id, "at": stamp}));
        self.changed();
        Ok(())
    }

    fn current_or_refuse(&self, item_id: &str) -> Result<(Item, DateTime)> {
        let now = self.clock.now();
        match self.get(item_id)? {
            Some(it) if it.status == "active" && current(&it, &now)? => Ok((it, now)),
            _ => Err(refused(format!("no current item {item_id}"))),
        }
    }

    /// items.py:622 `retire`.
    pub fn retire(&self, item_id: &str, reason: &str) -> Result<()> {
        let (_, now) = self.current_or_refuse(item_id)?;
        self.store.exec(
            "UPDATE memory_items SET valid_to = ? WHERE id = ?",
            sql![py::iso(&now), item_id],
        )?;
        (self.publish)("memory.retired", json!({"id": item_id, "reason": reason}));
        self.ended(&[item_id.to_owned()], None)?;
        self.changed();
        Ok(())
    }

    /// items.py:633 `looks_over` (MEM-14): FERB's understanding and what it observed are retired; the owner's
    /// words are set aside.
    pub fn looks_over(&self, item_id: &str, why: &str) -> Result<&'static str> {
        let (it, now) = self.current_or_refuse(item_id)?;
        if it.origin != "owner_stated" {
            self.retire(item_id, why)?;
            return Ok("retired");
        }
        let text = pystr::split_ws(&(self.redact)(why))
            .collect::<Vec<_>>()
            .join(" ");
        self.store.exec(
            "INSERT OR REPLACE INTO memory_set_aside (item_id, at, why) VALUES (?, ?, ?)",
            sql![item_id, py::iso(&now), pystr::prefix(&text, 200)],
        )?;
        (self.publish)("memory.set_aside", json!({"id": item_id, "reason": why}));
        (self.hooks.borrow().on_set_aside)(&[item_id.to_owned()])?;
        self.changed();
        Ok("set_aside")
    }

    /// items.py:651 `restore`.
    pub fn restore(&self, item_id: &str) -> Result<bool> {
        if self.store.exec(
            "DELETE FROM memory_set_aside WHERE item_id = ?",
            sql![item_id],
        )? == 0
        {
            return Ok(false);
        }
        (self.publish)("memory.restored", json!({"id": item_id}));
        if let Some(it) = self.get(item_id)? {
            self.saved_hook(&it)?;
        }
        self.changed();
        Ok(true)
    }

    fn never_again(&self, it: &Item) -> Result<()> {
        self.store.exec(
            "INSERT OR IGNORE INTO memory_forgotten (fingerprint, at) VALUES (?, ?)",
            sql![
                fingerprint(&it.kind, &it.subject, &it.key, &it.content),
                py::iso(&self.clock.now())
            ],
        )?;
        Ok(())
    }

    /// items.py:667 `edit`: the owner's correction, a new owner-stated version.
    pub fn edit(&self, item_id: &str, content: &str, source: &Source) -> Result<Item> {
        let it = self
            .get(item_id)?
            .ok_or_else(|| refused(format!("no item {item_id}")))?;
        Ok(self
            .save(
                &it.kind,
                &it.subject,
                &it.key,
                content,
                "owner_stated",
                std::slice::from_ref(source),
                1.0,
                &[item_id.to_owned()],
                &it.about,
                None,
            )?
            .item)
    }

    // ---------- forget = purge ----------

    /// items.py:677 `plan_forget`.
    pub fn plan_forget(&self, item_id: &str, keep_sources: bool) -> Result<ForgetPlan> {
        if self.get(item_id)?.is_none() {
            return Err(refused(format!("no item {item_id}")));
        }
        let mut set: BTreeSet<String> = self.history(item_id)?.into_iter().map(|v| v.id).collect();
        set.insert(item_id.to_owned());
        let items: Vec<String> = set.into_iter().collect();
        let item_params = || {
            items
                .iter()
                .map(|i| Sql::Text(i.clone()))
                .collect::<Vec<_>>()
        };
        let sources = self.store.query(
            &format!(
                "SELECT DISTINCT kind, source_id FROM memory_sources WHERE item_id IN ({})",
                marks(items.len())
            ),
            item_params(),
        )?;
        let mut rows: BTreeSet<String> = sources
            .iter()
            .filter(|s| s.s("kind") == "trail")
            .map(|s| s.s("source_id"))
            .collect();
        if !rows.is_empty() {
            let mut params = item_params();
            params.extend(rows.iter().map(|r| Sql::Text(r.clone())));
            let shared = self.store.query(
                &format!(
                    "SELECT DISTINCT source_id FROM memory_sources WHERE kind = 'trail' AND item_id NOT IN ({}) \
                     AND source_id IN ({})",
                    marks(items.len()),
                    marks(rows.len())
                ),
                params,
            )?;
            for r in shared {
                rows.remove(&r.s("source_id"));
            }
        }
        let mut plan = ForgetPlan {
            items: items.clone(),
            trail: if keep_sources {
                Vec::new()
            } else {
                rows.into_iter().collect()
            },
            kept: sources.iter().filter(|s| s.s("kind") == "journal").count() as i64,
            ..ForgetPlan::default()
        };
        if !plan.trail.is_empty() {
            let summarized = self.summary_days()?;
            let days: BTreeSet<String> = self
                .trail_rows(&plan.trail)?
                .iter()
                .map(|r| r.s("day"))
                .collect();
            plan.rebuild_days = days
                .into_iter()
                .filter(|d| summarized.contains(d))
                .collect();
        }
        plan.destinations = self.named_destinations(&items)?;
        Ok(plan)
    }

    fn named_destinations(&self, items: &[String]) -> Result<Vec<(String, String)>> {
        let m = marks(items.len());
        let params = || {
            items
                .iter()
                .map(|i| Sql::Text(i.clone()))
                .collect::<Vec<_>>()
        };
        let mut texts = Vec::new();
        for r in self.store.query(
            &format!(
                "SELECT m.subject, m.key, m.content, c.about FROM memory_items m LEFT JOIN memory_context c \
                 ON c.item_id = m.id WHERE m.id IN ({m})"
            ),
            params(),
        )? {
            let subject = r.s("subject");
            let name = subject
                .split_once(':')
                .map_or("", |(_, n)| n)
                .replace(['_', '-'], " ");
            let about = r.opt("about").unwrap_or_default();
            let key = r.s("key").replace('_', " ");
            let content = r.s("content");
            let parts: Vec<&str> = [content.as_str(), about.as_str(), name.as_str(), key.as_str()]
                .into_iter()
                .filter(|p| !pystr::strip(p).is_empty())
                .collect();
            texts.push(parts.join("\n"));
        }
        let text = texts.join("\n");
        let digits: String = text
            .chars()
            .filter(|c| !(pystr::is_space(*c) || matches!(c, '-' | '(' | ')' | '.' | '+')))
            .collect();
        let mut found = Vec::new();
        for r in self.store.query(
            "SELECT kind, value FROM known_destinations ORDER BY at",
            sql![],
        )? {
            let (kind, value) = (r.s("kind"), r.s("value"));
            if value.starts_with("sha256:") || value.is_empty() {
                continue;
            }
            let hit = match kind.as_str() {
                "phone" => pystr::char_len(&value) >= 6 && digits.contains(&value),
                "host" => {
                    let bare = value.strip_prefix("www.").unwrap_or(&value).to_owned();
                    let labels: Vec<&str> = bare.split('.').collect();
                    let site = if labels.len() >= 2 {
                        labels[labels.len() - 2].to_owned()
                    } else {
                        bare.clone()
                    };
                    let mut options = vec![value.clone(), bare.clone()];
                    if pystr::char_len(&site) >= 4 {
                        options.push(site);
                    }
                    options.iter().any(|v| contains_words(&text, v))
                }
                _ => {
                    contains_words(&text, &value)
                        || {
                            let mut any = false;
                            for x in self.store.query(
                            &format!("SELECT subject FROM memory_items WHERE id IN ({m}) AND subject LIKE 'person:%'"),
                            params(),
                        )? {
                            let subject = x.s("subject");
                            let n = subject.split_once(':').map_or("", |(_, n)| n).replace('_', " ");
                            if pystr::char_len(&n) >= 4 && contains_words(&value, &n) {
                                any = true;
                                break;
                            }
                        }
                            any
                        }
                }
            };
            if hit {
                found.push((kind, value));
            }
        }
        Ok(found)
    }

    fn summary_days(&self) -> Result<BTreeSet<String>> {
        let mut out = BTreeSet::new();
        for r in self.store.query(
            "SELECT data_json FROM journal WHERE kind = 'trail.day_summary'",
            sql![],
        )? {
            let d = ferb_py::json::loads(&r.s("data_json")).map_err(Error::Json)?;
            out.insert(py::py_str_opt(d.get("day")));
        }
        Ok(out)
    }

    /// items.py:741 `forget`: the item, its versions, their source links and (unless `keep_sources`) the trail rows
    /// they came from, in one transaction; each version's fingerprint kept so it is never learned again; the
    /// summaries of the days whose rows went rebuilt; the WAL emptied.
    pub fn forget(&self, item_id: &str, keep_sources: bool) -> Result<ForgetPlan> {
        let mut plan = self.plan_forget(item_id, keep_sources)?;
        (self.hooks.borrow().on_forget)(&plan)?;
        let now = py::iso(&self.clock.now());
        let m = marks(plan.items.len());
        let params = || {
            plan.items
                .iter()
                .map(|i| Sql::Text(i.clone()))
                .collect::<Vec<_>>()
        };
        let prints: Vec<String> = self
            .store
            .query(
                &format!("SELECT kind, subject, key, content FROM memory_items WHERE id IN ({m})"),
                params(),
            )?
            .iter()
            .map(|r| fingerprint(&r.s("kind"), &r.s("subject"), &r.s("key"), &r.s("content")))
            .collect();
        self.store.plain("BEGIN")?;
        let body = (|| -> Result<Vec<String>> {
            for table in [
                "memory_sources",
                "memory_context",
                "memory_profile",
                "memory_set_aside",
            ] {
                self.store.exec(
                    &format!("DELETE FROM {table} WHERE item_id IN ({m})"),
                    params(),
                )?;
            }
            self.store.exec(
                &format!("DELETE FROM memory_items WHERE id IN ({m})"),
                params(),
            )?;
            for (kind, value) in &plan.destinations {
                self.store.exec(
                    "DELETE FROM known_destinations WHERE kind = ? AND value = ?",
                    sql![kind, value],
                )?;
            }
            for tid in &plan.trail {
                self.store
                    .exec("DELETE FROM trail WHERE id = ?", sql![tid])?;
                self.store.exec(
                    "DELETE FROM memory_sources WHERE kind = 'trail' AND source_id = ?",
                    sql![tid],
                )?;
            }
            let mut deleted = Vec::new();
            for day in &plan.rebuild_days {
                if !self
                    .store
                    .exists("SELECT 1 FROM trail WHERE day = ? LIMIT 1", sql![day])?
                {
                    deleted.push(day.clone());
                }
            }
            for day in &deleted {
                self.store.exec(
                    "DELETE FROM journal WHERE kind = 'trail.day_summary' AND json_extract(data_json, '$.day') = ?",
                    sql![day],
                )?;
            }
            for fp in &prints {
                self.store.exec(
                    "INSERT OR IGNORE INTO memory_forgotten (fingerprint, at) VALUES (?, ?)",
                    sql![fp, now],
                )?;
            }
            self.store.plain("COMMIT")?;
            Ok(deleted)
        })();
        match body {
            Ok(deleted) => plan.delete_summaries.extend(deleted),
            Err(e) => {
                if self.store.in_transaction() {
                    self.store.plain("ROLLBACK")?;
                }
                return Err(e);
            }
        }
        plan.rebuild_days
            .retain(|d| !plan.delete_summaries.contains(d));
        for day in &plan.rebuild_days {
            (self.hooks.borrow().rebuild_summary)(day)?;
        }
        self.store.truncate_wal("memory.forget")?;
        (self.hooks.borrow().on_settled)(&plan.items, true);
        self.changed();
        Ok(plan)
    }
}

/// items.py:169 `forgets_on_record` (ORCH-578): each forget the memory store journaled, when and how many items.
pub fn forgets_on_record(store: &Store) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for r in store.query(
        "SELECT at, data_json FROM journal WHERE kind = 'memory.forgot' ORDER BY seq",
        sql![],
    )? {
        let raw = r
            .opt("data_json")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "{}".into());
        let data = ferb_py::json::loads(&raw).map_err(Error::Json)?;
        if data.get("via").is_none() {
            if let Some(n @ Value::Number(num)) = data.get("items") {
                if num.is_i64() || num.is_u64() {
                    out.push(json!({"at": r.s("at"), "items": n}));
                }
            }
        }
    }
    Ok(out)
}

/// The origin label (items.py:224 `label`).
pub fn origin_label(origin: &str) -> &str {
    label(origin)
}
