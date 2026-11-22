//! The imported profile (MEM-439..446; the owner's decisions from the memory-import lab: IMP-11, IMP-12, IMP-20,
//! IMP-27, IMP-31, IMP-33..IMP-38, IMP-41..IMP-43, IMP-46, IMP-47, IMP-50, IMP-51): every source's checked facts
//! turned into one list FERB can act on, kept up to date in place.
//!
//! - **Lines are memory items with an id that never changes** (`memory_import_lines`). A line's name (kind, subject,
//!   key) is fixed when it is made and no two lines share one. Only facts the profile has not seen go to the model
//!   (`import_place.md`), each with the few lines it might belong with: it joins a line (the line's words stay; the
//!   day is credited, which is how a repeat holds up), changes it (only a newer, different truth: a new version of
//!   the item, the old words kept as history), becomes a new line, or is left out with a reason (shown only when the
//!   owner asks, IMP-20). A line no new fact touches is not rewritten.
//! - **Code sets the dates and sources** from the facts a line rests on (`timed`, IMP-12): over when the newest is;
//!   until the newest end date given; due a check on the latest check date any gives.
//! - **The owner's corrections outrank every fact** (IMP-35, IMP-38), read from the memory item itself, so the Memory
//!   page and chat corrections count with nothing new to learn: his own words (an owner-stated version) are never
//!   replaced or repeated by an import; a line he removed (retracted or forgotten) is gone and the same thing said
//!   again is left out; a line he retired is over until he says it again after that. His scope is final.
//! - **Scope** (IMP-42, IMP-43): a line is general unless its own words tie it to a project or tool (a model reads
//!   the words, `import_scope.md`) or he scopes it. Where it was said (a session's folder, a chat's title) is kept as
//!   evidence and never ties it. The scope is shown in the item's `about`, so every role reads it on the entry.
//! - **Age and offers** (IMP-34, IMP-37, IMP-41, IMP-46, IMP-50, IMP-51): a line last said more than 30 days ago is
//!   kept, marked ("last said …") and asked about once; an offer is made only while its line is recent, not over and
//!   nothing since shows it done (`import_offers.md` reads the work log and newer lines); one 30 to 60 days old is
//!   asked about once, at most five, newest first.
//! - **Joining the always-on profile at once** (IMP-47): said on two or more days or in two or more assistants, and
//!   at least once in the last 30 days (`holds_up_now`, used by the core profile's `holds_up`).
//!
//! Imported lines stay FERB's understanding (`inferred`, MEM-402): they guide, never authorize.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde_json::{json, Map, Value};

use ferb_memory::import_extract::digest;
use ferb_memory::import_sources::source_name;
use ferb_memory::pystr;
use ferb_memory::terms::{query_terms, slug, subject_key, KINDS};
use ferb_memory::times;

use crate::import_job::{ImportJob, SOURCE_KIND};
use crate::items::Item;
use crate::py;
use crate::sql;
use crate::{Error, Result};

/// New facts in one placement call.
pub const PLACE_FACTS: usize = 40;
/// Lines shown with a new fact: the ones sharing the most (and rarest) words with it.
pub const NEAR: usize = 5;
/// A line last said longer ago is from a while ago: not stated beside what is current, its offer not made (FERB's
/// recency half-life; the owner confirmed 30, IMP-41).
pub const OLD_DAYS: i64 = 30;
/// An offer last said between OLD_DAYS and this long ago is asked about, once (IMP-51).
pub const ASK_DAYS: i64 = 60;
/// The most offers one ask holds (IMP-51).
pub const ASK_MOST: usize = 5;
/// An offer said this lately is never called past on a reading of its words alone (FERB's weekly pass; IMP-41).
pub const SETTLE_DAYS: i64 = 7;
/// What happened since, as shown to the offers check (the newest first).
pub const EVIDENCE_CHARS: usize = 60_000;
/// Why a fact is left out: the placement's reasons, and the owner's two.
pub const WHY: [&str; 5] = [
    "project detail",
    "one-off",
    "over",
    "too vague",
    "not about the owner",
];
pub const WRONG: &str = "you said it is wrong";
pub const OWNER: &str = "your own words stand";
/// The uses that steer FERB's actions (an owner's correction of one becomes the instruction too).
pub const GUIDES: [&str; 3] = ["follow", "choose", "repeat"];
const DAY: i64 = 86_400_000_000;

/// One checked fact the import read (`memory_import_facts`).
#[derive(Clone, Debug, PartialEq)]
pub struct Fact {
    pub id: String,
    pub source: String,
    pub kind: String,
    pub subject: String,
    pub key: String,
    pub content: String,
    pub said_at: String,
    pub conversation: String,
    pub place_kind: String,
    pub place: String,
    pub claim: bool,
    pub ended: bool,
    pub until: String,
    pub review_days: i64,
    pub line_id: Option<String>,
    pub left_why: Option<String>,
}

/// One line of the profile: its fixed name, its memory item's words and origin, and the rest (`data`).
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub id: String,
    pub item_id: String,
    pub kind: String,
    pub subject: String,
    pub key: String,
    pub content: String,
    pub origin: String,
    pub data: Map<String, Value>,
}

impl Line {
    pub fn s(&self, k: &str) -> String {
        self.data.get(k).map(py::py_str).unwrap_or_default()
    }

    pub fn b(&self, k: &str) -> bool {
        self.data.get(k).and_then(Value::as_bool).unwrap_or(false)
    }

    fn i(&self, k: &str) -> i64 {
        self.data.get(k).and_then(Value::as_i64).unwrap_or(0)
    }

    pub fn gone(&self) -> bool {
        self.b("gone")
    }

    /// Over: ended, or past its end date.
    pub fn over(&self, today: &str) -> bool {
        self.b("ended") || (!self.s("until").is_empty() && self.s("until").as_str() < today)
    }

    pub fn offer(&self) -> String {
        self.s("offer")
    }

    pub fn said(&self) -> String {
        self.s("said")
    }

    /// The text with the line's scope in front of it, in plain words.
    pub fn scoped(&self, text: &str) -> String {
        let scope = self.s("scope");
        if scope.is_empty() {
            return text.to_owned();
        }
        if self.s("scope_kind") == "tool" {
            format!("When using {scope}: {text}")
        } else {
            format!("In the {scope} project: {text}")
        }
    }
}

/// What FERB does with a fact of this kind (IMP-11).
pub fn use_of(kind: &str, key: &str) -> &'static str {
    match kind {
        "fact" if key.starts_with("recurring") => "repeat",
        "fact" => "know",
        "preference" => "follow",
        "env" => "choose",
        "person" => "person",
        "project" => "project",
        _ => "know",
    }
}

fn name_key(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn texts(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(py::py_str)
        .collect()
}

fn fact_row(r: &crate::store::Row) -> Fact {
    Fact {
        id: r.s("id"),
        source: r.s("source"),
        kind: r.s("kind"),
        subject: r.s("subject"),
        key: r.s("key"),
        content: r.s("content"),
        said_at: r.s("said_at"),
        conversation: r.s("conversation"),
        place_kind: r.s("place_kind"),
        place: r.s("place"),
        claim: r.i("claim") != 0,
        ended: r.i("ended") != 0,
        until: r.s("until"),
        review_days: r.i("review_days"),
        line_id: r.opt("line_id").filter(|s| !s.is_empty()),
        left_why: r.opt("left_why").filter(|s| !s.is_empty()),
    }
}

/// A checked fact's id: the same fact read again (a re-export, a resumed run) is the same row.
pub fn fact_id(
    source: &str,
    kind: &str,
    subject: &str,
    key: &str,
    content: &str,
    said: &str,
) -> String {
    format!(
        "f{}",
        &digest(&[source, kind, subject, key, content, said])[..10]
    )
}

fn new_line_id(kind: &str, rests: &[String], taken: &HashSet<String>) -> String {
    let mut sorted: Vec<&str> = rests.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    let seed = format!("{kind}\x1f{}", sorted.join("\x1f"));
    let mut n = 0;
    loop {
        let id = format!("l{}", &digest(&[&seed, &n.to_string()])[..10]);
        if !taken.contains(&id) {
            return id;
        }
        n += 1;
    }
}

/// Where a line's facts were said: (kind, folder name or conversation id) -> how many.
fn places(facts: &[&Fact]) -> BTreeMap<(String, String), usize> {
    let mut out = BTreeMap::new();
    for f in facts {
        let place = match f.place_kind.as_str() {
            "folder" if !f.place.is_empty() => ("folder".to_owned(), f.place.clone()),
            "chat" => ("chat".to_owned(), f.conversation.clone()),
            _ => (String::new(), String::new()),
        };
        *out.entry(place).or_insert(0) += 1;
    }
    out
}

/// The projects (folders, and chats about one project) a line's facts were said in, and how many facts came from
/// general chats and from places not known.
fn evidence(facts: &[&Fact], chats: &HashMap<String, String>) -> Value {
    let mut projects: BTreeMap<String, String> = BTreeMap::new();
    let (mut general, mut unknown) = (0usize, 0usize);
    for ((kind, name), n) in places(facts) {
        let about = match kind.as_str() {
            "folder" => Some(name.clone()),
            "chat" => chats.get(&name).cloned(),
            _ => None,
        };
        match about {
            None => unknown += n,
            Some(a) if a.is_empty() => general += n,
            Some(a) => {
                projects.entry(name_key(&a)).or_insert(a);
            }
        }
    }
    json!({"projects": projects.into_values().collect::<Vec<_>>(), "general": general, "unknown": unknown})
}

fn scope_mark(content: &str, ev: &Value) -> String {
    let mut parts = vec![content.to_owned()];
    parts.extend(texts(ev.get("projects")).iter().map(|p| name_key(p)));
    if ev["general"].as_u64().unwrap_or(0) > 0 {
        parts.push("general".into());
    }
    let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
    digest(&refs)[..12].to_owned()
}

impl ImportJob<'_> {
    pub(crate) fn today(&self) -> String {
        let now = self.memory.clock.now();
        times::astimezone(&now, self.memory.zone.as_ref())
            .map_or_else(|| times::ymd(&now), |d| times::ymd(&d))
    }

    /// The day `days` before today.
    pub(crate) fn days_ago(&self, days: i64) -> String {
        let now = py::shift(&self.memory.clock.now(), -days * DAY);
        times::astimezone(&now, self.memory.zone.as_ref())
            .map_or_else(|| times::ymd(&now), |d| times::ymd(&d))
    }

    /// The local day a fact was said.
    fn day_of(&self, said: &str) -> String {
        match py::parse(said) {
            Ok(at) => times::astimezone(&at, self.memory.zone.as_ref())
                .map_or_else(|| pystr::prefix(said, 10).to_owned(), |d| times::ymd(&d)),
            Err(_) => pystr::prefix(said, 10).to_owned(),
        }
    }

    fn clean(&self, v: Option<&Value>) -> String {
        let text = v.map(py::py_str).unwrap_or_default();
        (self.redact)(&pystr::split_ws(&text).collect::<Vec<_>>().join(" "))
    }

    /// Every checked fact (`placed`: false for only those not placed yet).
    pub fn facts(&self, unplaced_only: bool) -> Result<Vec<Fact>> {
        let q = if unplaced_only {
            "SELECT * FROM memory_import_facts WHERE line_id IS NULL AND left_why IS NULL ORDER BY said_at, id"
        } else {
            "SELECT * FROM memory_import_facts ORDER BY said_at, id"
        };
        Ok(self.store.query(q, sql![])?.iter().map(fact_row).collect())
    }

    /// Every line, each read again from its memory item: the owner's corrections show here (`refresh`).
    pub fn lines(&self) -> Result<Vec<Line>> {
        let mut out = Vec::new();
        for r in self.store.query(
            "SELECT * FROM memory_import_lines ORDER BY created_at, id",
            sql![],
        )? {
            let data = match ferb_py::json::loads(&r.s("data_json")).map_err(Error::Json)? {
                Value::Object(m) => m,
                _ => Map::new(),
            };
            let mut line = Line {
                id: r.s("id"),
                item_id: r.s("item_id"),
                kind: r.s("kind"),
                subject: r.s("subject"),
                key: r.s("key"),
                content: String::new(),
                origin: String::new(),
                data,
            };
            self.refresh(&mut line)?;
            out.push(line);
        }
        Ok(out)
    }

    /// The line's current words and origin from its memory item, and what the owner did to it.
    fn refresh(&self, line: &mut Line) -> Result<()> {
        let current: Option<Item> = self
            .memory
            .items(Some("active"), Some(&line.subject), true)?
            .into_iter()
            .find(|i| i.kind == line.kind && i.key == line.key);
        match current {
            Some(it) => {
                line.item_id = it.id.clone();
                line.content = it.content.clone();
                line.origin = it.origin.clone();
                line.data.remove("gone");
            }
            None => match self.memory.get(&line.item_id)? {
                // forgotten (the row is gone) or retracted: the owner called it wrong
                None => {
                    line.data.insert("gone".into(), json!(true));
                }
                Some(it) if it.status == "retracted" => {
                    line.content = it.content.clone();
                    line.origin = it.origin.clone();
                    line.data.insert("gone".into(), json!(true));
                }
                // retired: over, kept as history, until something said after it
                Some(it) => {
                    line.content = it.content.clone();
                    line.origin = it.origin.clone();
                    if line.s("done_by_owner").is_empty() {
                        let at = it
                            .valid_to
                            .map_or_else(|| py::iso(&self.memory.clock.now()), |t| py::iso(&t));
                        line.data.insert("done_by_owner".into(), json!(at));
                    }
                    line.data.insert("ended".into(), json!(true));
                }
            },
        }
        Ok(())
    }

    pub(crate) fn write_line(&self, line: &Line, fresh: bool) -> Result<()> {
        let now = py::iso(&self.memory.clock.now());
        let data = ferb_py::json::dumps(&Value::Object(line.data.clone()));
        if fresh {
            self.store.exec(
                "INSERT INTO memory_import_lines (id, item_id, kind, subject, key, data_json, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                sql![line.id, line.item_id, line.kind, line.subject, line.key, data, now, now],
            )?;
        } else {
            self.store.exec(
                "UPDATE memory_import_lines SET item_id = ?, data_json = ?, updated_at = ? WHERE id = ?",
                sql![line.item_id, data, now, line.id],
            )?;
        }
        Ok(())
    }

    /// When a line was said, where, on how many days, and when it stops holding, from the facts it rests on
    /// (IMP-12): over when the newest supporting fact is (or the placement read it as over); until the end date of
    /// the newest that gives one; due a check on the latest check date any gives, else the placement's suggestion.
    fn timed(&self, line: &mut Line, rests: &[&Fact]) {
        if rests.is_empty() {
            return;
        }
        let mut rests: Vec<&Fact> = rests.to_vec();
        rests.sort_by(|a, b| {
            let (x, y) = (py::parse(&a.said_at).ok(), py::parse(&b.said_at).ok());
            x.map(|t| times::instant(&t))
                .cmp(&y.map(|t| times::instant(&t)))
                .then_with(|| a.id.cmp(&b.id))
        });
        let newest = rests[rests.len() - 1];
        let days: BTreeSet<String> = rests.iter().map(|f| self.day_of(&f.said_at)).collect();
        let sources: BTreeSet<String> = rests.iter().map(|f| f.source.clone()).collect();
        let checks: Vec<String> = rests
            .iter()
            .filter(|f| f.review_days > 0)
            .filter_map(|f| py::parse(&f.said_at).ok().map(|t| (t, f.review_days)))
            .map(|(t, d)| self.day_of(&py::iso(&py::shift(&t, d * DAY))))
            .collect();
        let suggested = line.i("suggested_review");
        let review_on = match checks.iter().max() {
            Some(c) => c.clone(),
            None if suggested > 0 => py::parse(&newest.said_at)
                .map(|t| self.day_of(&py::iso(&py::shift(&t, suggested * DAY))))
                .unwrap_or_default(),
            None => String::new(),
        };
        let until = rests
            .iter()
            .rev()
            .find(|f| !f.until.is_empty())
            .map(|f| f.until.clone())
            .unwrap_or_default();
        let from: BTreeSet<String> = rests
            .iter()
            .filter(|f| !f.conversation.is_empty())
            .map(|f| f.conversation.clone())
            .collect();
        let ended = newest.ended || line.b("over");
        let d = &mut line.data;
        d.insert("sources".into(), json!(sources.iter().collect::<Vec<_>>()));
        d.insert("days".into(), json!(days.len()));
        d.insert("first_said".into(), json!(self.day_of(&rests[0].said_at)));
        d.insert("said".into(), json!(self.day_of(&newest.said_at)));
        d.insert(
            "ended".into(),
            json!(ended || d.contains_key("done_by_owner")),
        );
        d.insert("until".into(), json!(until));
        d.insert("review_on".into(), json!(review_on));
        d.insert("use".into(), json!(use_of(&line.kind, &line.key)));
        d.insert(
            "holds_up".into(),
            json!(days.len() > 1 || sources.len() > 1),
        );
        d.insert(
            "from".into(),
            json!(from.into_iter().take(12).collect::<Vec<_>>()),
        );
    }

    /// The lines a new fact might belong with: the same project or person, or sharing its rarer words.
    fn near<'l>(&self, new: &[&Fact], lines: &'l [Line]) -> Vec<&'l Line> {
        let terms: Vec<HashSet<String>> = lines
            .iter()
            .map(|l| {
                query_terms(&format!("{} {}", l.key.replace('_', " "), l.content))
                    .into_iter()
                    .collect()
            })
            .collect();
        let mut df: HashMap<&str, usize> = HashMap::new();
        for ts in &terms {
            for t in ts {
                *df.entry(t.as_str()).or_insert(0) += 1;
            }
        }
        let n = lines.len() as f64;
        let weight = |t: &str| -> f64 {
            let c = df.get(t).copied().unwrap_or(0) as f64;
            if c > 0.0 && c <= f64::max(2.0, n / 2.0) {
                ((n + 1.0) / c).ln()
            } else {
                0.0
            }
        };
        let mut picked: Vec<usize> = Vec::new();
        for f in new {
            let mine: HashSet<String> =
                query_terms(&format!("{} {}", f.key.replace('_', " "), f.content))
                    .into_iter()
                    .collect();
            let mut scored: Vec<(f64, &str, usize)> = terms
                .iter()
                .enumerate()
                .filter(|(_, ts)| ts.iter().any(|t| mine.contains(t)))
                .map(|(i, ts)| {
                    (
                        ts.iter()
                            .filter(|t| mine.contains(*t))
                            .map(|t| weight(t))
                            .sum::<f64>(),
                        lines[i].id.as_str(),
                        i,
                    )
                })
                .collect();
            scored.sort_by(|a, b| {
                b.0.partial_cmp(&a.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.1.cmp(b.1))
            });
            for (score, _, i) in scored.into_iter().take(NEAR) {
                if score > 0.0 && !picked.contains(&i) {
                    picked.push(i);
                }
            }
            if f.subject.starts_with("project:") || f.subject.starts_with("person:") {
                for (i, l) in lines.iter().enumerate() {
                    if name_key(&l.subject) == name_key(&f.subject) && !picked.contains(&i) {
                        picked.push(i);
                    }
                }
            }
        }
        picked.into_iter().map(|i| &lines[i]).collect()
    }

    /// The memory item's `about`: where it came from, its scope, and its age when old.
    fn about(&self, line: &Line) -> String {
        let names: Vec<&str> = texts(line.data.get("sources"))
            .iter()
            .map(|s| source_name(s).unwrap_or("another assistant"))
            .collect();
        let mut out = format!(
            "imported from {}",
            if names.is_empty() {
                "another assistant".to_owned()
            } else {
                names.join(" and ")
            }
        );
        let scope = line.s("scope");
        if !scope.is_empty() {
            if line.s("scope_kind") == "tool" {
                out.push_str(&format!("; only when using {scope}"));
            } else {
                out.push_str(&format!("; only in the {scope} project"));
            }
        }
        if self.old(line) {
            out.push_str(&format!("; last said {}", line.said()));
        }
        out
    }

    /// Its only evidence is from a while ago (IMP-41, IMP-48): not the owner's own words, not a line he gave an end
    /// date still ahead; a project counts as touched by its newest work-log session in a folder of its name.
    pub fn old(&self, line: &Line) -> bool {
        line.origin != "owner_stated"
            && !(line.s("until").as_str() >= self.today().as_str() && !line.s("until").is_empty())
            && !line.said().is_empty()
            && self.touched(line) < self.days_ago(OLD_DAYS)
    }

    fn touched(&self, line: &Line) -> String {
        // the owner's own "still on" (MEM-444) makes a line current again
        let said = line
            .said()
            .max(pystr::prefix(&line.s("confirmed"), 10).to_owned());
        if line.kind != "project" {
            return said;
        }
        let mut names: HashSet<String> =
            texts(line.data.get("said_in").and_then(|s| s.get("projects")))
                .iter()
                .map(|p| name_key(p))
                .collect();
        names.insert(name_key(
            line.subject.split_once(':').map_or(&*line.subject, |x| x.1),
        ));
        let newest = self
            .store
            .query(
                "SELECT body_json FROM learning_nodes WHERE kind = 'work_log' AND valid_to IS NULL",
                sql![],
            )
            .unwrap_or_default()
            .iter()
            .filter_map(|r| ferb_py::json::loads(&r.s_at(0)).ok())
            .filter(|b| names.contains(&name_key(&py::py_str_opt(b.get("folder")))))
            .map(|b| py::py_str_opt(b.get("day")))
            .max()
            .unwrap_or_default();
        said.max(newest)
    }

    /// Does the line join the always-on profile at once (IMP-47): said on two or more days or in two or more
    /// assistants, and at least once in the last 30 days.
    pub fn joins(&self, line: &Line) -> bool {
        line.b("holds_up") && line.said() >= self.days_ago(OLD_DAYS)
    }

    /// The items whose lines join the always-on profile at once (the core profile's `holds_up`, MEM-424 + IMP-47).
    pub fn holding_items(&self) -> Result<BTreeSet<String>> {
        Ok(self
            .lines()?
            .into_iter()
            .filter(|l| !l.gone() && self.joins(l))
            .map(|l| l.item_id)
            .collect())
    }

    /// Saves a line's words as a version of its memory item, through the one save path (so the owner's own words
    /// are never replaced, MEM-409); `None` when memory refused it (the owner asked FERB to forget it).
    fn save_words(&self, line: &Line, content: &str, entry: &str) -> Result<Option<Item>> {
        match self.memory.save(
            &line.kind,
            &line.subject,
            &line.key,
            content,
            "inferred",
            &[("journal".into(), entry.to_owned())],
            0.6,
            &[],
            &self.about(line),
            Some(false),
        ) {
            Ok(s) => Ok(Some(s.item)),
            Err(Error::MemoryRefused(_) | Error::Value(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// After a line changed: its dates on the item, the assistants it came from, its `about`.
    pub(crate) fn settle(&self, line: &Line, rests: &[&Fact], run_id: &str) -> Result<()> {
        for f in rests {
            self.store.exec(
                "INSERT INTO memory_import_items (item_id, source, run_id, said_at) VALUES (?, ?, ?, ?) \
                 ON CONFLICT(item_id, source) DO UPDATE SET said_at = MAX(said_at, excluded.said_at)",
                sql![line.item_id, f.source, run_id, f.said_at],
            )?;
        }
        let Some(item) = self.memory.get(&line.item_id)? else {
            return Ok(());
        };
        if item.valid_to.is_none() && item.status == "active" {
            let until = line.s("until");
            if line.b("ended") {
                if let Some(f) = rests.iter().max_by_key(|f| f.said_at.clone()) {
                    self.memory.expire_at(&item.id, &py::parse(&f.said_at)?)?;
                }
            } else if let Some(u) = times::Date::from_isoformat(&until) {
                let offset = times::astimezone(&self.memory.clock.now(), self.memory.zone.as_ref())
                    .and_then(|d| d.offset);
                let end = crate::DateTime {
                    year: u.year,
                    month: u.month,
                    day: u.day,
                    hour: 23,
                    minute: 59,
                    second: 59,
                    microsecond: 999_999,
                    offset,
                };
                self.memory.expire_at(&item.id, &end)?;
            } else if let Some(r) = times::Date::from_isoformat(&line.s("review_on")) {
                let at = crate::DateTime {
                    year: r.year,
                    month: r.month,
                    day: r.day,
                    hour: 9,
                    minute: 0,
                    second: 0,
                    microsecond: 0,
                    offset: times::astimezone(&self.memory.clock.now(), self.memory.zone.as_ref())
                        .and_then(|d| d.offset),
                };
                self.memory.set_review(&item.id, Some(&at))?;
            }
        }
        let about = self.about(line);
        if item.about != about && item.origin != "owner_stated" {
            self.memory.set_about(&item.id, &about)?;
        }
        Ok(())
    }

    /// Every current line's `about` as it stands today (its age moves with the days): set where it changed; no model.
    pub fn refresh_abouts(&self) -> Result<usize> {
        let mut changed = 0;
        for l in self.lines()? {
            if l.gone() {
                continue;
            }
            let Some(item) = self.memory.get(&l.item_id)? else {
                continue;
            };
            let about = self.about(&l);
            if item.valid_to.is_none()
                && item.status == "active"
                && item.origin != "owner_stated"
                && item.about != about
            {
                self.memory.set_about(&item.id, &about)?;
                changed += 1;
            }
        }
        Ok(changed)
    }

    /// Where placing stands, for the live card (MEM-456): `placed` batches of `to_place`. Published like the reading
    /// steps (the job's `memory.import.progress`), never stored.
    fn placing(&self, run_id: &str, placed: i64, to_place: i64) {
        (self.publish)(
            "memory.import.progress",
            json!({"run": run_id, "status": "running", "stage": "placing",
                   "counts": {"placed": placed, "to_place": to_place}}),
        );
    }

    fn leave(&self, ids: &[String], why: &str) -> Result<()> {
        for i in ids {
            self.store.exec(
                "UPDATE memory_import_facts SET left_why = ? WHERE id = ? AND line_id IS NULL",
                sql![why, i],
            )?;
        }
        Ok(())
    }

    fn rest_on(&self, line_id: &str, ids: &[String]) -> Result<()> {
        for i in ids {
            self.store.exec(
                "UPDATE memory_import_facts SET line_id = ?, left_why = NULL WHERE id = ?",
                sql![line_id, i],
            )?;
        }
        Ok(())
    }

    pub(crate) fn line_facts(&self, line_id: &str) -> Result<Vec<Fact>> {
        Ok(self
            .store
            .query(
                "SELECT * FROM memory_import_facts WHERE line_id = ? ORDER BY said_at, id",
                sql![line_id],
            )?
            .iter()
            .map(fact_row)
            .collect())
    }

    /// Places every fact the profile has not seen (IMP-31): only those go to the model, a batch at a time with the
    /// lines they might belong with; the rest of the profile is not rewritten. Then the scope of lines whose words or
    /// places changed, the offers, and the one ask. Returns what changed.
    pub async fn place(&self, run_id: &str) -> Result<Map<String, Value>> {
        let mut change: BTreeMap<String, i64> = BTreeMap::new();
        let new = self.facts(true)?;
        let mut lines = self.lines()?;
        let today = self.today();
        let entry = if new.is_empty() {
            String::new()
        } else {
            let sources: BTreeSet<&str> = new.iter().map(|f| f.source.as_str()).collect();
            self.journal
                .append(
                    SOURCE_KIND,
                    Some(json!({"run": run_id, "sources": sources, "profile": true})),
                    None,
                )?
                .id
        };
        let mut counts = Map::new();
        let mut taken: HashSet<String> = lines.iter().map(|l| l.id.clone()).collect();
        let by_id: HashMap<String, Fact> = new.iter().map(|f| (f.id.clone(), f.clone())).collect();
        let batches = new.len().div_ceil(PLACE_FACTS) as i64;
        for (i, batch) in new.chunks(PLACE_FACTS).enumerate() {
            // the live card's bar while placing (MEM-456): batches placed of batches
            self.placing(run_id, i as i64, batches);
            let refs: Vec<&Fact> = batch.iter().collect();
            let shown: Vec<Line> = self.near(&refs, &lines).into_iter().cloned().collect();
            let mut text: Vec<String> = batch
                .iter()
                .map(|f| {
                    let over =
                        f.ended || (!f.until.is_empty() && f.until.as_str() < today.as_str());
                    format!(
                        "N | {} | {} | {} | {} | {} | {} | {}{}",
                        f.id,
                        f.kind,
                        f.subject,
                        f.key,
                        pystr::prefix(&f.said_at, 10),
                        f.source,
                        f.content,
                        if over { " (over)" } else { "" }
                    )
                })
                .collect();
            text.extend(shown.iter().map(|l| {
                format!(
                    "L | {} | {} | {} | {} | {} | {}",
                    l.id,
                    l.kind,
                    l.subject,
                    l.key,
                    l.said(),
                    l.content
                )
            }));
            let shown_ids: HashSet<&str> = shown.iter().map(|l| l.id.as_str()).collect();
            let task = crate::calls::learning_task(
                &format!(
                    "memory-import-place-{}",
                    &digest(&[run_id, &batch[0].id])[..12]
                ),
                "update what FERB learned from other assistants",
                &self.device_id,
                "memory-import",
            );
            let answer = match self
                .call(
                    run_id,
                    &task,
                    "memory/import_place.md",
                    &text.join("\n"),
                    &place_schema(),
                    "memory_import_place",
                    &mut counts,
                )
                .await
            {
                Ok(a) => a,
                Err(e) => {
                    // these facts wait for the next update
                    self.journal.append(
                        "memory.import_place_failed",
                        Some(json!({"run": run_id, "error": pystr::prefix(&(self.redact)(&e.described()), 300)})),
                        None,
                    )?;
                    *change.entry("left for the next update".into()).or_default() +=
                        batch.len() as i64;
                    continue;
                }
            };
            let mut placed: HashSet<String> = HashSet::new();
            for pl in answer
                .get("placements")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let mut ids: Vec<String> = Vec::new();
                for x in texts(pl.get("facts")) {
                    let x = x.trim_matches(['[', ']', ' ']).to_owned();
                    if by_id.contains_key(&x)
                        && batch.iter().any(|f| f.id == x)
                        && !placed.contains(&x)
                        && !ids.contains(&x)
                    {
                        ids.push(x);
                    }
                }
                let action = pl.get("action").map(py::py_str).unwrap_or_default();
                let target = pl
                    .get("line")
                    .map(py::py_str)
                    .unwrap_or_default()
                    .trim_matches(['[', ']', ' '])
                    .to_owned();
                let at = lines.iter().position(|l| l.id == target);
                if ids.is_empty()
                    || (matches!(action.as_str(), "join" | "update")
                        && (at.is_none() || !shown_ids.contains(target.as_str())))
                {
                    continue; // names nothing real: those facts wait for the next update
                }
                let newest = ids
                    .iter()
                    .map(|i| by_id[i].said_at.clone())
                    .max_by_key(|s| py::parse(s).map(|t| times::instant(&t)).unwrap_or(0))
                    .unwrap_or_default();
                let target_line = at.map(|i| lines[i].clone());
                match (action.as_str(), target_line) {
                    ("join" | "update", Some(l)) if l.gone() => {
                        // the same thing the owner called wrong: never kept again
                        self.leave(&ids, WRONG)?;
                        *change.entry(format!("left out: {WRONG}")).or_default() +=
                            ids.len() as i64;
                    }
                    ("join" | "update", Some(l)) if l.origin == "owner_stated" => {
                        // no import replaces or repeats his own words (MEM-409)
                        self.leave(&ids, OWNER)?;
                        *change.entry(format!("left out: {OWNER}")).or_default() +=
                            ids.len() as i64;
                    }
                    ("drop", _) => {
                        let why = pl.get("why").map(py::py_str).unwrap_or_default();
                        let why = if WHY.contains(&why.as_str()) {
                            why
                        } else {
                            "one-off".into()
                        };
                        self.leave(&ids, &why)?;
                        *change.entry("left out".into()).or_default() += ids.len() as i64;
                    }
                    ("join" | "update", Some(mut l)) => {
                        let i = at.expect("found");
                        let mut done = !l.s("done_by_owner").is_empty();
                        if done {
                            let after =
                                |s: &str| py::parse(s).map(|t| times::instant(&t)).unwrap_or(0);
                            if after(&newest) > after(&l.s("done_by_owner")) {
                                l.data.remove("done_by_owner");
                                l.data.insert("ended".into(), json!(false));
                                done = false;
                                *change
                                    .entry("lines you had ended, said again since".into())
                                    .or_default() += 1;
                            }
                        }
                        let content = self.clean(pl.get("content"));
                        let newest_day = self.day_of(&newest);
                        let differs = action == "update"
                            && !content.is_empty()
                            && newest_day >= l.said()
                            && !done
                            && content != l.content;
                        let words = if differs {
                            content.clone()
                        } else {
                            l.content.clone()
                        };
                        if differs {
                            let mut history = l
                                .data
                                .get("history")
                                .and_then(Value::as_array)
                                .cloned()
                                .unwrap_or_default();
                            history.push(
                                json!({"content": l.content, "you": l.s("you"), "said": l.said(),
                                                "replaced_by_fact_said": newest_day}),
                            );
                            l.data.insert("history".into(), Value::Array(history));
                            let you = self.clean(pl.get("you"));
                            l.data.insert(
                                "you".into(),
                                json!(if you.is_empty() { content.clone() } else { you }),
                            );
                            l.data
                                .insert("ferb".into(), json!(self.clean(pl.get("ferb"))));
                            l.data
                                .insert("offer".into(), json!(self.clean(pl.get("offer"))));
                            l.data.insert(
                                "over".into(),
                                json!(pl.get("over").and_then(Value::as_bool).unwrap_or(false)),
                            );
                            l.data.insert(
                                "suggested_review".into(),
                                json!(pl
                                    .get("review_in_days")
                                    .and_then(Value::as_i64)
                                    .unwrap_or(0)
                                    .clamp(0, 3650)),
                            );
                        }
                        self.rest_on(&l.id, &ids)?;
                        let rests = self.line_facts(&l.id)?;
                        let refs: Vec<&Fact> = rests.iter().collect();
                        self.timed(&mut l, &refs);
                        if !done || differs {
                            // a new version when the words changed; the same words credit the day (a repeat holds up)
                            if let Some(item) = self.save_words(&l, &words, &entry)? {
                                l.item_id = item.id.clone();
                                l.content = item.content.clone();
                                l.origin = item.origin.clone();
                            }
                        }
                        self.write_line(&l, false)?;
                        self.settle(&l, &refs, run_id)?;
                        *change
                            .entry(if differs {
                                "lines reworded by a newer fact".into()
                            } else {
                                "lines a fact joined (words unchanged)".into()
                            })
                            .or_default() += 1;
                        lines[i] = l;
                    }
                    _ => {
                        // a new line (also: a different thing about a line called wrong)
                        let content = self.clean(pl.get("content"));
                        if content.is_empty() {
                            continue;
                        }
                        let first = &by_id[&ids[0]];
                        let subject = {
                            let s = self.clean(pl.get("subject"));
                            if s.is_empty() {
                                first.subject.clone()
                            } else {
                                s
                            }
                        };
                        let kind_said = pl.get("kind").map(py::py_str).unwrap_or_default();
                        let mut kind = if KINDS.contains(&kind_said.as_str()) {
                            kind_said
                        } else if subject.starts_with("project:") {
                            "project".into()
                        } else if subject.starts_with("person:") {
                            "person".into()
                        } else {
                            "fact".into()
                        };
                        if (kind == "project") != subject.starts_with("project:") {
                            kind = if subject.starts_with("project:") {
                                "project".into()
                            } else {
                                "fact".into()
                            };
                        }
                        let key = {
                            let k = self.clean(pl.get("key"));
                            if kind == "project" {
                                "identity".to_owned()
                            } else if k.is_empty() {
                                first.key.clone()
                            } else {
                                k
                            }
                        };
                        // a name of its own: no two lines share one, or the second would be read as a version of
                        // the first
                        let name = |k: &str, s: &str, y: &str| {
                            (k.to_owned(), subject_key(s), slug(y, None))
                        };
                        let mut key_used = key.clone();
                        let mut n = 1;
                        while lines.iter().any(|o| {
                            name(&o.kind, &o.subject, &o.key) == name(&kind, &subject, &key_used)
                        }) {
                            n += 1;
                            key_used = format!("{key}_{n}");
                        }
                        // the owner's own words under this name stand (MEM-409)
                        let owners = self
                            .memory
                            .items(Some("active"), Some(&subject), true)?
                            .into_iter()
                            .any(|i| {
                                i.kind == kind
                                    && i.key == slug(&key_used, None)
                                    && i.origin == "owner_stated"
                            });
                        if owners {
                            self.leave(&ids, OWNER)?;
                            *change.entry(format!("left out: {OWNER}")).or_default() +=
                                ids.len() as i64;
                            placed.extend(ids);
                            continue;
                        }
                        let you = self.clean(pl.get("you"));
                        let mut l = Line {
                            id: new_line_id(&kind, &ids, &taken),
                            item_id: String::new(),
                            kind: kind.clone(),
                            subject: subject_key(&subject),
                            key: slug(&key_used, None),
                            content: content.clone(),
                            origin: "inferred".into(),
                            data: Map::new(),
                        };
                        taken.insert(l.id.clone());
                        l.data.insert(
                            "you".into(),
                            json!(if you.is_empty() { content.clone() } else { you }),
                        );
                        l.data
                            .insert("ferb".into(), json!(self.clean(pl.get("ferb"))));
                        l.data
                            .insert("offer".into(), json!(self.clean(pl.get("offer"))));
                        l.data.insert(
                            "over".into(),
                            json!(pl.get("over").and_then(Value::as_bool).unwrap_or(false)),
                        );
                        l.data.insert(
                            "suggested_review".into(),
                            json!(pl
                                .get("review_in_days")
                                .and_then(Value::as_i64)
                                .unwrap_or(0)
                                .clamp(0, 3650)),
                        );
                        l.data.insert("history".into(), json!([]));
                        let mine: Vec<Fact> = ids.iter().map(|i| by_id[i].clone()).collect();
                        let refs: Vec<&Fact> = mine.iter().collect();
                        self.timed(&mut l, &refs);
                        let Some(item) = self.save_words(&l, &content, &entry)? else {
                            // memory refused it: the owner asked FERB to forget the same words
                            self.leave(&ids, WRONG)?;
                            placed.extend(ids);
                            continue;
                        };
                        l.item_id = item.id.clone();
                        l.subject = item.subject.clone();
                        l.key = item.key.clone();
                        self.write_line(&l, true)?;
                        self.rest_on(&l.id, &ids)?;
                        self.settle(&l, &refs, run_id)?;
                        *change.entry("new lines".into()).or_default() += 1;
                        lines.push(l);
                    }
                }
                placed.extend(ids);
            }
            let unplaced = batch.iter().filter(|f| !placed.contains(&f.id)).count() as i64;
            if unplaced > 0 {
                *change.entry("left for the next update".into()).or_default() += unplaced;
            }
        }
        if batches > 0 {
            self.placing(run_id, batches, batches);
        }
        change.insert("new facts".into(), new.len() as i64);
        let moved = self.judge_scope(run_id).await?;
        for (k, v) in moved {
            *change.entry(k).or_default() += v;
        }
        self.check_offers(run_id).await?;
        Ok(change.into_iter().map(|(k, v)| (k, json!(v))).collect())
    }

    // ---------- scope (IMP-42, IMP-43) ----------

    fn chats(&self) -> Result<HashMap<String, String>> {
        Ok(self
            .store
            .query(
                "SELECT conversation, about FROM memory_import_chats",
                sql![],
            )?
            .iter()
            .map(|r| (r.s("conversation"), r.s("about")))
            .collect())
    }

    /// Sets each line's scope. The model is asked only about lines whose words or places changed; a line it does not
    /// tie is general; the owner's scope is final. Also records what each chat is about.
    pub async fn judge_scope(&self, run_id: &str) -> Result<BTreeMap<String, i64>> {
        let mut change = BTreeMap::new();
        let mut chats = self.chats()?;
        let lines = self.lines()?;
        let mut facts_of: HashMap<String, Vec<Fact>> = HashMap::new();
        for f in self.facts(false)? {
            if let Some(l) = &f.line_id {
                facts_of.entry(l.clone()).or_default().push(f);
            }
        }
        let asked = |l: &Line, chats: &HashMap<String, String>| -> bool {
            if l.gone()
                || l.kind == "project"
                || l.s("scope_by") == "owner"
                || l.origin == "owner_stated"
            {
                return false;
            }
            let rests: Vec<&Fact> = facts_of
                .get(&l.id)
                .map(|v| v.iter().collect())
                .unwrap_or_default();
            let p = places(&rests);
            let known = p.keys().any(|(k, _)| !k.is_empty());
            let unread = p.keys().any(|(k, n)| k == "chat" && !chats.contains_key(n));
            known
                && (unread || l.s("scope_read") != scope_mark(&l.content, &evidence(&rests, chats)))
        };
        let todo: Vec<&Line> = lines.iter().filter(|l| asked(l, &chats)).collect();
        let mut verdicts: HashMap<String, Value> = HashMap::new();
        if !todo.is_empty() {
            let mut ids: Vec<((String, String), String)> = Vec::new();
            for l in &todo {
                let rests: Vec<&Fact> = facts_of
                    .get(&l.id)
                    .map(|v| v.iter().collect())
                    .unwrap_or_default();
                for place in places(&rests).keys() {
                    if !place.0.is_empty() && !ids.iter().any(|(p, _)| p == place) {
                        ids.push((place.clone(), format!("p{}", ids.len() + 1)));
                    }
                }
            }
            let title: HashMap<String, String> = facts_of
                .values()
                .flatten()
                .filter(|f| f.place_kind == "chat")
                .map(|f| (f.conversation.clone(), f.place.clone()))
                .collect();
            let mut text: Vec<String> = ids
                .iter()
                .map(|((kind, name), i)| {
                    let shown = if kind == "folder" {
                        name.clone()
                    } else {
                        title
                            .get(name)
                            .filter(|t| !t.is_empty())
                            .cloned()
                            .unwrap_or_else(|| "untitled".into())
                    };
                    format!("P | {i} | {kind} | {}", (self.redact)(&shown))
                })
                .collect();
            for l in &todo {
                let rests: Vec<&Fact> = facts_of
                    .get(&l.id)
                    .map(|v| v.iter().collect())
                    .unwrap_or_default();
                let said_in: Vec<String> = places(&rests)
                    .iter()
                    .filter(|(p, _)| !p.0.is_empty())
                    .map(|(p, n)| {
                        let i = &ids.iter().find(|(q, _)| q == p).expect("listed").1;
                        if *n > 1 {
                            format!("{i} x{n}")
                        } else {
                            i.clone()
                        }
                    })
                    .collect();
                text.push(format!(
                    "L | {} | {} | {} | {} | {}",
                    l.id,
                    use_of(&l.kind, &l.key),
                    l.subject,
                    l.content,
                    said_in.join(", ")
                ));
            }
            let task = crate::calls::learning_task(
                &format!(
                    "memory-import-scope-{}",
                    &digest(&[run_id, &todo[0].id])[..12]
                ),
                "where what FERB learned from other assistants applies",
                &self.device_id,
                "memory-import",
            );
            let mut counts = Map::new();
            match self
                .call(
                    run_id,
                    &task,
                    "memory/import_scope.md",
                    &text.join("\n"),
                    &scope_schema(),
                    "memory_import_scope",
                    &mut counts,
                )
                .await
            {
                Ok(answer) => {
                    for c in answer
                        .get("chats")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let id = py::py_str_opt(c.get("id"));
                        let id = id.trim_matches(['[', ']', ' ']);
                        if let Some(((_, conv), _)) =
                            ids.iter().find(|((k, _), i)| k == "chat" && i == id)
                        {
                            let about = self.clean(c.get("about"));
                            let about =
                                if ["", "general", "none"].contains(&name_key(&about).as_str()) {
                                    String::new()
                                } else {
                                    about
                                };
                            if !chats.contains_key(conv) {
                                self.store.exec(
                                    "INSERT OR IGNORE INTO memory_import_chats (conversation, about) VALUES (?, ?)",
                                    sql![conv, about],
                                )?;
                                chats.insert(conv.clone(), about);
                            }
                        }
                    }
                    for v in answer
                        .get("lines")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let id = py::py_str_opt(v.get("id"))
                            .trim_matches(['[', ']', ' '])
                            .to_owned();
                        verdicts.insert(id, v.clone());
                    }
                }
                Err(e) => {
                    self.journal.append(
                        "memory.import_scope_failed",
                        Some(json!({"run": run_id, "error": pystr::prefix(&(self.redact)(&e.described()), 300)})),
                        None,
                    )?;
                }
            }
            // a chat asked about and not answered for: not known, and not asked about again
            for ((kind, conv), _) in &ids {
                if kind == "chat" && !chats.contains_key(conv) {
                    self.store.exec(
                        "INSERT OR IGNORE INTO memory_import_chats (conversation, about) VALUES (?, '?')",
                        sql![conv],
                    )?;
                    chats.insert(conv.clone(), "?".into());
                }
            }
        }
        let todo_ids: HashSet<String> = todo.iter().map(|l| l.id.clone()).collect();
        for mut l in lines {
            let rests: Vec<&Fact> = facts_of
                .get(&l.id)
                .map(|v| v.iter().collect())
                .unwrap_or_default();
            let chats_known: HashMap<String, String> = chats
                .iter()
                .filter(|(_, a)| a.as_str() != "?")
                .map(|(k, a)| (k.clone(), a.clone()))
                .collect();
            let ev = evidence(&rests, &chats_known);
            let was = l.s("scope");
            let before = l.data.clone();
            l.data.insert("said_in".into(), ev.clone());
            if l.kind != "project" && l.s("scope_by") != "owner" && !l.gone() {
                if let Some(v) = verdicts.get(&l.id).filter(|_| todo_ids.contains(&l.id)) {
                    l.data
                        .insert("scope_read".into(), json!(scope_mark(&l.content, &ev)));
                    let kind = py::py_str_opt(v.get("scope"));
                    let name = if kind == "project" || kind == "tool" {
                        self.clean(v.get("name"))
                    } else {
                        String::new()
                    };
                    if name.is_empty() {
                        for k in ["scope", "scope_kind", "scope_by"] {
                            l.data.insert(k.into(), json!(""));
                        }
                    } else {
                        let matched = texts(ev.get("projects"))
                            .into_iter()
                            .find(|p| name_key(p) == name_key(&name))
                            .unwrap_or_else(|| name.clone());
                        l.data.insert(
                            "scope".into(),
                            json!(if kind == "project" { matched } else { name }),
                        );
                        l.data.insert("scope_kind".into(), json!(kind));
                        l.data.insert("scope_by".into(), json!("words"));
                    }
                }
            }
            if l.data != before {
                self.write_line(&l, false)?;
                if l.s("scope") != was {
                    *change
                        .entry(if l.s("scope").is_empty() {
                            "lines widened to general".into()
                        } else {
                            "lines now tied to a project or tool".into()
                        })
                        .or_default() += 1;
                    self.settle(&l, &rests, run_id)?;
                }
            }
        }
        Ok(change)
    }

    /// The owner's own word on where a line applies ("general", or a project's or tool's name): final (IMP-42).
    pub fn scope_by_owner(&self, item_id: &str, name: &str) -> Result<Line> {
        let mut line = self.line_of(item_id)?;
        let general = ["", "general"].contains(&name_key(name).as_str());
        let name = pystr::split_ws(&(self.redact)(name))
            .collect::<Vec<_>>()
            .join(" ");
        line.data.insert(
            "scope".into(),
            json!(if general { String::new() } else { name }),
        );
        line.data.insert(
            "scope_kind".into(),
            json!(if general { "" } else { "project" }),
        );
        line.data.insert("scope_by".into(), json!("owner"));
        self.write_line(&line, false)?;
        let rests = self.line_facts(&line.id)?;
        self.settle(&line, &rests.iter().collect::<Vec<_>>(), "")?;
        Ok(line)
    }

    /// The line an item (any version of it) belongs to.
    pub fn line_of(&self, item_id: &str) -> Result<Line> {
        let item = self
            .memory
            .get(item_id)?
            .ok_or_else(|| Error::Value(format!("no memory item {item_id}")))?;
        self.lines()?
            .into_iter()
            .find(|l| {
                l.item_id == item_id
                    || (l.kind == item.kind && l.subject == item.subject && l.key == item.key)
            })
            .ok_or_else(|| {
                Error::Value("that memory wasn't brought in from another assistant".into())
            })
    }

    // ---------- offers (IMP-27, IMP-34, IMP-37, IMP-46, IMP-50, IMP-51) ----------

    /// How the owner answered a line's offer ("" when he hasn't, or when the line has since gained something he said
    /// after answering).
    fn dismissed(&self, line: &Line) -> String {
        let Some(a) = line.data.get("offer_answer") else {
            return String::new();
        };
        let at = pystr::prefix(&py::py_str_opt(a.get("at")), 10).to_owned();
        if line.said() > at {
            return String::new();
        }
        py::py_str_opt(a.get("what"))
    }

    /// Why a line's offer is off by the line's own dates ("" when they allow it).
    fn dated(&self, line: &Line, today: &str) -> String {
        if line.over(today) {
            return "it has ended".into();
        }
        let review = line.s("review_on");
        if !review.is_empty() && review.as_str() <= today {
            return format!("it was due a check on {review}");
        }
        String::new()
    }

    /// The lines whose offer is still open, the newest first, and the ones set aside, each with why. A dismissed
    /// offer is in neither.
    pub fn offers(&self, lines: &[Line]) -> (Vec<Line>, Vec<(Line, String)>) {
        let today = self.today();
        let old = self.days_ago(OLD_DAYS);
        let (mut still, mut aside) = (Vec::new(), Vec::new());
        for l in lines.iter().filter(|l| !l.gone() && !l.offer().is_empty()) {
            let answer = self.dismissed(l);
            if answer == "done" || answer == "not interested" {
                continue;
            }
            let shown = l.data.get("offer_done").filter(|v| v.is_object());
            let why = if answer == "open" && !l.over(&today) {
                String::new()
            } else if let Some(s) =
                shown.filter(|s| py::py_str_opt(s.get("day")) >= l.said() && !l.over(&today))
            {
                if py::py_str_opt(s.get("ref")).is_empty() {
                    format!("its moment has probably passed, or it is already taken care of; you last said it on {}", l.said())
                } else {
                    format!(
                        "{}, {}: {}",
                        py::py_str_opt(s.get("what")),
                        py::py_str_opt(s.get("day")),
                        py::py_str_opt(s.get("text"))
                    )
                }
            } else {
                let d = self.dated(l, &today);
                if !d.is_empty() {
                    d
                } else if l.said() < old {
                    format!("you last said it on {}", l.said())
                } else {
                    String::new()
                }
            };
            if why.is_empty() {
                still.push(l.clone());
            } else {
                aside.push((l.clone(), why));
            }
        }
        still.sort_by_key(|l| std::cmp::Reverse((l.said(), l.id.clone())));
        (still, aside)
    }

    pub(crate) fn state(&self, key: &str) -> Result<String> {
        Ok(self
            .store
            .one(
                "SELECT value FROM memory_import_state WHERE key = ?",
                sql![key],
            )?
            .map(|r| r.s_at(0))
            .unwrap_or_default())
    }

    pub(crate) fn set_state(&self, key: &str, value: &str) -> Result<()> {
        self.store.exec(
            "INSERT INTO memory_import_state (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            sql![key, value],
        )?;
        Ok(())
    }

    /// Whether what happened since (the work log, newer lines) shows an offer done, or a one-time thing whose moment
    /// has passed. Code holds the evidence: "done" counts only when it names a line of evidence shown, not the offer's
    /// own, dated no earlier than the offer's line and, for a work-log entry, one the log marks finished or dropped;
    /// "past" only when the line was not said in the last week. Either way the offer is set aside, never dropped.
    /// Asked again only when the offers, the evidence or the day changed.
    pub async fn check_offers(&self, run_id: &str) -> Result<()> {
        let today = self.today();
        let since_ask = self.days_ago(ASK_DAYS);
        let lines = self.lines()?;
        let asked: Vec<&Line> = lines
            .iter()
            .filter(|l| {
                !l.gone()
                    && !l.offer().is_empty()
                    && self.dismissed(l).is_empty()
                    && self.dated(l, &today).is_empty()
                    && l.said() >= since_ask
            })
            .collect();
        if asked.is_empty() {
            return Ok(());
        }
        let since = asked.iter().map(|l| l.said()).min().unwrap_or_default();
        let mut found: Vec<(String, String, String, String, bool)> = Vec::new();
        let mut log: Vec<Value> = self
            .store
            .query(
                "SELECT body_json FROM learning_nodes WHERE kind = 'work_log' AND valid_to IS NULL",
                sql![],
            )?
            .iter()
            .filter_map(|r| ferb_py::json::loads(&r.s_at(0)).ok())
            .collect();
        log.sort_by_key(|b| py::py_str_opt(b.get("last")));
        for (n, e) in log.iter().enumerate() {
            if py::py_str_opt(e.get("day")) >= since {
                let state = py::py_str_opt(e.get("state"));
                found.push((
                    pystr::prefix(&py::py_str_opt(e.get("last")), 10).to_owned(),
                    format!("w{}", n + 1),
                    format!("work log, {state}"),
                    format!(
                        "{}: {}",
                        py::py_str_opt(e.get("title")),
                        py::py_str_opt(e.get("summary"))
                    ),
                    state == "finished" || state == "dropped",
                ));
            }
        }
        for l in &lines {
            if !l.gone() && l.said() >= since {
                found.push((
                    l.said(),
                    l.id.clone(),
                    "a newer fact".into(),
                    l.content.clone(),
                    false,
                ));
            }
        }
        found.sort_by_key(|e| std::cmp::Reverse((e.0.clone(), e.1.clone())));
        let mut evidence: Vec<(String, String, String, String, bool)> = Vec::new();
        let mut size = 0;
        for e in found {
            size += pystr::char_len(&e.3) + 40;
            if size > EVIDENCE_CHARS {
                break;
            }
            evidence.push(e);
        }
        if evidence.is_empty() {
            return Ok(());
        }
        let mut text = vec![format!("TODAY | {today}")];
        text.extend(asked.iter().map(|l| {
            format!(
                "O | {} | {} | {} | {}",
                l.id,
                l.said(),
                l.content,
                l.offer()
            )
        }));
        text.extend(
            evidence
                .iter()
                .map(|(day, r, what, said, _)| format!("E | {r} | {day} | {what} | {said}")),
        );
        let text = text.join("\n");
        let stamp = digest(&["memory/import_offers.md", &text])[..16].to_owned();
        if self.state("offers_stamp")? == stamp {
            return Ok(());
        }
        let task = crate::calls::learning_task(
            &format!("memory-import-offers-{}", &stamp[..12]),
            "which offers from other assistants are still open",
            &self.device_id,
            "memory-import",
        );
        let mut counts = Map::new();
        let answer = match self
            .call(
                run_id,
                &task,
                "memory/import_offers.md",
                &text,
                &offers_schema(),
                "memory_import_offers",
                &mut counts,
            )
            .await
        {
            Ok(a) => a,
            Err(_) => return Ok(()), // the dates still decide; asked again next time
        };
        let settled = self.days_ago(SETTLE_DAYS);
        for v in answer
            .get("offers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let id = py::py_str_opt(v.get("id"))
                .trim_matches(['[', ']', ' '])
                .to_owned();
            let r = py::py_str_opt(v.get("because"))
                .trim_matches(['[', ']', ' '])
                .to_owned();
            let Some(line) = asked.iter().find(|l| l.id == id) else {
                continue;
            };
            let mut line = (*line).clone();
            let state = py::py_str_opt(v.get("state"));
            let mark = if state == "past" && line.said() < settled {
                Some(json!({"ref": "", "day": today, "what": "", "text": ""}))
            } else if state == "done" && r != line.id {
                evidence
                    .iter()
                    .find(|e| {
                        e.1 == r && (e.4 || lines.iter().any(|l| l.id == r)) && e.0 >= line.said()
                    })
                    .map(|(day, r, what, said, _)| {
                        json!({"ref": r, "day": day, "what": what.split(',').next().unwrap_or(""),
                               "text": pystr::prefix(&(self.redact)(said), 200)})
                    })
            } else {
                None
            };
            if let Some(m) = mark {
                line.data.insert("offer_done".into(), m);
                self.write_line(&line, false)?;
            }
        }
        self.set_state("offers_stamp", &stamp)
    }

    /// The owner answers a line's offer: "done", "not interested", or "open" (IMP-38).
    pub fn answer_offer(&self, item_id: &str, what: &str) -> Result<Line> {
        let what = pystr::split_ws(&what.replace('-', " "))
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if !["done", "not interested", "open"].contains(&what.as_str()) {
            return Err(Error::Value(
                "an offer is answered with done, not interested or open".into(),
            ));
        }
        let mut line = self.line_of(item_id)?;
        if line.offer().is_empty() {
            return Err(Error::Value("that memory carries no offer".into()));
        }
        line.data.insert(
            "offer_answer".into(),
            json!({"what": what, "at": py::iso(&self.memory.clock.now())}),
        );
        line.data.remove("asking");
        self.write_line(&line, false)?;
        Ok(line)
    }

    /// An import's one ask about older offers (IMP-46, IMP-50, IMP-51): those last said between 30 and 60 days ago,
    /// never asked about before and not shown done; the newest five, the rest passed over. Each is asked about once.
    pub fn ask_round(&self) -> Result<Vec<Line>> {
        let lines = self.lines()?;
        let (old, ask_from) = (self.days_ago(OLD_DAYS), self.days_ago(ASK_DAYS));
        let (_, aside) = self.offers(&lines);
        let mut fresh: Vec<Line> = aside
            .into_iter()
            .filter(|(l, why)| {
                why.starts_with("you last said it on ")
                    && l.said() < old
                    && l.said() >= ask_from
                    && !l.data.contains_key("asked")
            })
            .map(|(l, _)| l)
            .collect();
        fresh.sort_by_key(|l| std::cmp::Reverse((l.said(), l.id.clone())));
        let today = self.today();
        for mut l in lines.into_iter().filter(|l| l.b("asking")) {
            l.data.remove("asking");
            self.write_line(&l, false)?;
        }
        for (n, l) in fresh.iter_mut().enumerate() {
            l.data.insert("asked".into(), json!(today));
            if n < ASK_MOST {
                l.data.insert("asking".into(), json!(true));
            }
            self.write_line(l, false)?;
        }
        fresh.truncate(ASK_MOST);
        Ok(fresh)
    }

    /// Lines from a while ago asked about once (IMP-41), those that steer FERB's work (MEM-462, `old_round_lines`).
    pub(crate) fn old_round(&self) -> Result<Vec<Line>> {
        self.old_round_lines()
    }

    /// The owner's typed reply to the review (MEM-444, MEM-460), read by the model (`memory/import_answer.md`: which
    /// numbered items he says yes or no to; never a word list in code) against the review as it stands now (an item
    /// he answered with its button since is not asked again), and applied by code through the review's one path
    /// (`answer_review`): always on, approve or decline; a line still on is current again, one he says is over is
    /// retired, kept as history; an offer's answer is kept on its line. An item he didn't name stays on the list.
    /// `_ident`: what the question named when asked (kept for an older build's question; the list now decides).
    pub async fn apply_answer(
        &self,
        profile: &crate::profile::CoreProfile,
        _ident: &Value,
        words: &str,
    ) -> Result<Map<String, Value>> {
        let items = self.review_numbered()?;
        let mut text: Vec<String> = items
            .iter()
            .map(|i| {
                format!(
                    "ASKED | {} | {} | {}",
                    i["n"],
                    py::py_str_opt(i.get("what")),
                    (self.redact)(&py::py_str_opt(i.get("text")))
                )
            })
            .collect();
        text.push(format!(
            "REPLY | {}",
            (self.redact)(&pystr::split_ws(words).collect::<Vec<_>>().join(" "))
        ));
        let task = crate::calls::learning_task(
            &format!("memory-import-answer-{}", &digest(&[words])[..12]),
            "apply the owner's answer about older memories",
            &self.device_id,
            "memory-import",
        );
        let mut counts = Map::new();
        let answer = self
            .call(
                "answer",
                &task,
                "memory/import_answer.md",
                &text.join("\n"),
                &answer_schema(),
                "memory_import_answer",
                &mut counts,
            )
            .await?;
        let (mut approved, mut declined, mut kept, mut aside, mut unclear) = (0, 0, 0, 0, 0);
        for v in answer
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let n = v.get("n").and_then(Value::as_i64).unwrap_or(0);
            let Some(item) = items.iter().find(|i| i["n"].as_i64() == Some(n)) else {
                continue;
            };
            let state = py::py_str_opt(v.get("state"));
            let id = vec![py::py_str_opt(item.get("item"))];
            let (part, said) = match (py::py_str_opt(item.get("what")).as_str(), state.as_str()) {
                (crate::import_review::ALWAYS_ON, "still_on") => {
                    (crate::import_review::ALWAYS_ON, "approve")
                }
                (crate::import_review::ALWAYS_ON, "done" | "not_wanted") => {
                    (crate::import_review::ALWAYS_ON, "decline")
                }
                ("line", "still_on") => (crate::import_review::STILL_TRUE, "yes"),
                ("line", "done" | "not_wanted") => (crate::import_review::STILL_TRUE, "no"),
                ("offer", "still_on") => (crate::import_review::OFFERS, "open"),
                ("offer", "done") => (crate::import_review::OFFERS, "done"),
                ("offer", "not_wanted") => (crate::import_review::OFFERS, "not interested"),
                _ => {
                    unclear += 1;
                    continue;
                }
            };
            if self.answer_review(profile, part, &id, said)? == 0 {
                continue;
            }
            match said {
                "approve" => approved += 1,
                "decline" => declined += 1,
                "yes" | "open" => kept += 1,
                _ => aside += 1,
            }
        }
        Ok(crate::import_review::counts(
            approved, declined, kept, aside, unclear,
        ))
    }

    /// What FERB says once it applied the owner's typed answer (owner-facing).
    pub fn answer_line(counts: &Map<String, Value>) -> String {
        let n = |k: &str| counts.get(k).and_then(Value::as_i64).unwrap_or(0);
        let (approved, declined, kept, aside) =
            (n("approved"), n("declined"), n("still_on"), n("set_aside"));
        if approved + declined == 0 {
            return match (kept, aside) {
                (0, 0) => {
                    "I couldn't tell which ones you meant, so I've left them as they were.".into()
                }
                (k, 0) => format!(
                    "Got it, I'll keep {} as they are.",
                    if k == 1 {
                        "that one".to_owned()
                    } else {
                        format!("those {k}")
                    }
                ),
                (0, a) => format!(
                    "Got it, I've put {} aside.",
                    if a == 1 {
                        "that one".to_owned()
                    } else {
                        format!("those {a}")
                    }
                ),
                (k, a) => format!("Got it: I'll keep {k} and I've put {a} aside."),
            };
        }
        let mut said = Vec::new();
        if approved > 0 {
            said.push(format!("{approved} always on now"));
        }
        if declined > 0 {
            said.push(format!("{declined} remembered but not always on"));
        }
        if kept > 0 {
            said.push(format!("{kept} kept as they are"));
        }
        if aside > 0 {
            said.push(format!("{aside} put aside"));
        }
        let list = match said.len() {
            1 => said[0].clone(),
            k => format!("{} and {}", said[..k - 1].join(", "), said[k - 1]),
        };
        format!("Got it: {list}.")
    }

    /// The one notice after an import (MEM-460): what FERB can now do because of it, a few examples, and the
    /// one ask (lines from a while ago; offers 30 to 60 days old). Plain, friendly, said to the owner.
    pub fn report(
        &self,
        opening: &str,
        change: &Map<String, Value>,
    ) -> Result<(String, Vec<Value>)> {
        let today = self.today();
        let lines: Vec<Line> = self
            .lines()?
            .into_iter()
            .filter(|l| !l.gone() && !l.over(&today))
            .collect();
        let n = |k: &str| change.get(k).and_then(Value::as_i64).unwrap_or(0);
        let learned = n("new lines") + n("lines reworded by a newer fact");
        let mut out = vec![opening.to_owned()];
        let current: Vec<&Line> = lines.iter().filter(|l| !self.old(l)).collect();
        let by = |u: &str| -> Vec<&Line> {
            current
                .iter()
                .copied()
                .filter(|l| l.s("use") == u)
                .collect()
        };
        let follow = by("follow");
        let choose = by("choose");
        let mut can: Vec<String> = Vec::new();
        if !follow.is_empty() {
            let ex: Vec<String> = follow
                .iter()
                .take(3)
                .map(|l| format!("“{}”", l.scoped(&l.s("you"))))
                .collect();
            can.push(format!(
                "I'll work the way you like ({} things), for example {}.",
                follow.len(),
                ex.join(", ")
            ));
        }
        if !choose.is_empty() {
            can.push(format!(
                "I know {} things about your setup, so I can pick the right account, app or device.",
                choose.len()
            ));
        }
        let (people, projects) = (by("person").len(), by("project").len());
        if people + projects > 0 {
            can.push(format!(
                "I know {projects} of your projects and {people} people you've mentioned."
            ));
        }
        if !can.is_empty() {
            out.push(String::new());
            out.extend(can);
        }
        let (still, _) = self.offers(&lines);
        if !still.is_empty() {
            out.push(String::new());
            out.push("Things I could help with, whenever you want (I won't start any of them on my own):".into());
            out.extend(
                still
                    .iter()
                    .take(3)
                    .map(|l| format!("- {}", l.scoped(&l.offer()))),
            );
        }
        out.push(String::new());
        out.push(if learned > 0 || !lines.is_empty() {
            "It's all on the Memory page as my understanding, not your words. If anything's wrong or out of date, just tell me and I'll fix it.".into()
        } else {
            "I didn't find anything new to remember this time.".into()
        });
        let mut cards: Vec<Value> = follow
            .iter()
            .take(3)
            .map(|l| {
                json!({"type": "note", "item_id": l.item_id, "text": l.content, "state": "saved",
                       "line": "Brought in from your other assistants", "label": ferb_memory::terms::label(&l.origin),
                       "origin": l.origin})
            })
            .collect();
        cards.push(json!({"type": "page", "page": "memory", "filter": {"imported_from": "any"}, "line": "Open the Memory page"}));
        Ok((out.join("\n"), cards))
    }
}

/// An offer cut to what it is about: its first clause, a few words.
pub fn object(offer: &str) -> String {
    let trimmed = offer.trim_end_matches(['.', ' ']);
    let mut cut = trimmed.len();
    for mark in [
        ",",
        ";",
        ":",
        "(",
        " so that ",
        " once ",
        " when ",
        " before ",
        " and tell ",
        " and notify ",
    ] {
        if let Some(i) = trimmed.find(mark) {
            cut = cut.min(i);
        }
    }
    let words: Vec<&str> = trimmed[..cut].split_whitespace().collect();
    let mut out = words.iter().take(8).copied().collect::<Vec<_>>().join(" ");
    if words.len() > 8 {
        out.push('…');
    }
    out
}

/// The ask about older offers, as the owner confirmed its shape (IMP-50, IMP-51).
pub fn ask_text(lines: &[Line]) -> Vec<String> {
    if lines.is_empty() {
        return Vec::new();
    }
    let mut out = vec!["Are these still on?".to_owned(), String::new()];
    out.extend(
        lines
            .iter()
            .enumerate()
            .map(|(n, l)| format!("{}. {}", n + 1, object(&l.offer()))),
    );
    out.push(String::new());
    out.push("Just say which, like \"first one yes, the rest are done\".".into());
    out
}

pub fn place_schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["placements"], "properties": {
        "placements": {"type": "array", "items": {
            "type": "object", "additionalProperties": false,
            "required": ["facts", "action", "line", "why", "kind", "subject", "key", "content", "you", "ferb", "offer",
                         "review_in_days", "over"],
            "properties": {"facts": {"type": "array", "items": {"type": "string"}},
                           "action": {"type": "string", "enum": ["join", "update", "new", "drop"]},
                           "line": {"type": "string"}, "why": {"type": "string"}, "kind": {"type": "string"},
                           "subject": {"type": "string"}, "key": {"type": "string"}, "content": {"type": "string"},
                           "you": {"type": "string"}, "ferb": {"type": "string"}, "offer": {"type": "string"},
                           "review_in_days": {"type": "integer"}, "over": {"type": "boolean"}}}}}})
}

pub fn scope_schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["chats", "lines"], "properties": {
        "chats": {"type": "array", "items": {"type": "object", "additionalProperties": false, "required": ["id", "about"],
                                             "properties": {"id": {"type": "string"}, "about": {"type": "string"}}}},
        "lines": {"type": "array", "items": {
            "type": "object", "additionalProperties": false, "required": ["id", "scope", "name"],
            "properties": {"id": {"type": "string"},
                           "scope": {"type": "string", "enum": ["owner", "project", "tool", "rule"]},
                           "name": {"type": "string"}}}}}})
}

pub fn answer_schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["items"], "properties": {"items": {
        "type": "array", "items": {"type": "object", "additionalProperties": false, "required": ["n", "state"],
                                   "properties": {"n": {"type": "integer"},
                                                  "state": {"type": "string",
                                                            "enum": ["still_on", "done", "not_wanted", "unclear"]}}}}}})
}

pub fn offers_schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["offers"], "properties": {"offers": {
        "type": "array", "items": {"type": "object", "additionalProperties": false, "required": ["id", "state", "because"],
                                   "properties": {"id": {"type": "string"},
                                                  "state": {"type": "string", "enum": ["open", "done", "past"]},
                                                  "because": {"type": "string"}}}}}})
}
