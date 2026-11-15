//! The order rules every conversation's stored entries keep (stage 2, ENG-8, ENG-13), checked on a trace of what the
//! projectors were fed. Shared by `ferb-supervisor/tests/chat_entries.rs` (the pure projector over invented
//! interleavings) and `ferbd/tests/chat_replay.rs` (the wired log in the real program, every recorded scenario), which
//! includes this file by path.
//!
//! A trace is one JSON value per item, in the order the log handled them (ferb-supervisor `ChatLog::set_trace`):
//! - `{"conversation", "input": {"do": "frame"|"end"|"bus"|"notification", ...}, "changes": [entry, ...], "live"}`:
//!   one input fed to a conversation's projector and the entries it created or revised, in commit order;
//! - `{"conversation", "input": {"do": "restart"}, "changes": [...]}`: the start-up pass after a restart;
//! - `{"held": {"topic", "data"}}`: a bus event about a request whose stream was open, held for the stream's end;
//! - `{"left_out": ...}`, `{"unrouted": ...}`: a bus copy the stream fed itself; an answer to no conversation's question.
//!
//! The rules (each a sentence of the report):
//! 1. `seq` strictly increases in commit order; each id's revisions count `rev` 1, 2, 3... and keep `pos`; the stored
//!    entry is its last revision; no two entries share `pos` or `seq`.
//! 2. `pos` is creation order: an entry created later has a higher `pos`.
//! 3. A message comes before everything of its own request: its plan, answers, errors and questions have higher `pos`.
//! 4. A plan comes before its task's answers and before the questions its task's workers asked. No exception: APP-580
//!    decides where an answer SHOWS (inline under its message, or below quoting it), never its `pos`, which is when it
//!    was made. An inline answer made after newer turns shows above them, and its `pos` is still above its plan's. (An
//!    answer made before its task's plan could only be the planner's own reply, and a reply ends the turn's session:
//!    a plan after it is a later frame, which binds no task to the turn, `receiveAfterAnswer`.)
//! 5. A question asked before its task's end (by the order of the inputs: the first frame or bus event that carried
//!    it, held or not) comes before the answer that told that end.
//! 6. An answer names what it answers (`answers`, a message) with a lower `pos`; a later answer (`follows`, APP-914)
//!    has a higher `pos` than the answer it follows.
//! 7. Catch-up: folding the history's changes after any `K` by id gives exactly the stored entries whose `seq` is above
//!    `K`, each once, at their stored state.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet};

use ferb_core::chat_entry::{AnswerBody, Entry, EntryKind};
use serde_json::Value;

/// One fed input of a trace.
pub struct Fed {
    /// Its index in the trace (held and left-out items count).
    pub at: usize,
    pub conversation: String,
    pub input: Value,
    pub changes: Vec<Entry>,
}

/// A trace read: the fed inputs, and per approval id the index where it was first asked.
pub struct Trace {
    pub fed: Vec<Fed>,
    pub asked_at: HashMap<String, usize>,
}

fn approval_asked(v: &Value) -> Option<String> {
    let (topic, data) = match v.get("do").and_then(Value::as_str) {
        Some("frame") => {
            let f = &v["frame"];
            if f["type"] != "event" {
                return None;
            }
            (f["topic"].as_str()?, &f["data"])
        }
        Some("bus") | None => (v["topic"].as_str()?, &v["data"]),
        _ => return None,
    };
    if topic != "approval.requested" {
        return None;
    }
    data["approval_id"].as_str().map(str::to_string)
}

/// A trace from its items.
pub fn read(items: &[Value]) -> Trace {
    let mut fed = Vec::new();
    let mut asked_at = HashMap::new();
    for (at, item) in items.iter().enumerate() {
        if let Some(held) = item.get("held") {
            if let Some(a) = approval_asked(held) {
                asked_at.entry(a).or_insert(at);
            }
            continue;
        }
        let Some(conversation) = item.get("conversation").and_then(Value::as_str) else {
            continue;
        };
        if let Some(a) = approval_asked(&item["input"]) {
            asked_at.entry(a).or_insert(at);
        }
        let changes = item["changes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| serde_json::from_value::<Entry>(e.clone()).expect("an entry"))
            .collect();
        fed.push(Fed {
            at,
            conversation: conversation.to_string(),
            input: item["input"].clone(),
            changes,
        });
    }
    Trace { fed, asked_at }
}

/// The tree a task id belongs to (`T.worker.verify` → `T`).
fn root(task: &str) -> &str {
    task.split('.').next().unwrap_or(task)
}

/// Every rule over one trace and the stored entries (all conversations): what breaks, one line each.
pub fn check(name: &str, trace: &Trace, stored: &[Entry]) -> Vec<String> {
    let mut out = Vec::new();
    let mut convs: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
    for e in stored {
        convs.entry(e.conversation_id.as_str()).or_default().push(e);
    }
    let mut history: BTreeMap<&str, Vec<(usize, &Entry)>> = BTreeMap::new();
    for f in &trace.fed {
        for c in &f.changes {
            history
                .entry(f.conversation.as_str())
                .or_default()
                .push((f.at, c));
        }
    }
    for (conv, hist) in &history {
        let mine: Vec<&Entry> = convs.get(conv).cloned().unwrap_or_default();
        out.extend(
            history_rules(hist, &mine)
                .into_iter()
                .map(|l| format!("{name} [{conv}]: {l}")),
        );
        out.extend(
            catch_up(hist, &mine)
                .into_iter()
                .map(|l| format!("{name} [{conv}]: {l}")),
        );
    }
    for (conv, mine) in &convs {
        if !history.contains_key(conv) {
            out.push(format!(
                "{name} [{conv}]: stored entries no trace item made"
            ));
        }
        let made_at: HashMap<&str, usize> = history
            .get(conv)
            .into_iter()
            .flatten()
            .filter(|(_, e)| e.rev == 1)
            .map(|(at, e)| (e.id.as_str(), *at))
            .collect();
        out.extend(
            structure(mine, &made_at, &trace.asked_at)
                .into_iter()
                .map(|l| format!("{name} [{conv}]: {l}")),
        );
    }
    out
}

/// Rules 1 and 2 on one conversation's history (its changes in commit order) and its stored entries.
fn history_rules(hist: &[(usize, &Entry)], stored: &[&Entry]) -> Vec<String> {
    let mut out = Vec::new();
    let mut last_seq = 0;
    let mut last: HashMap<&str, &Entry> = HashMap::new();
    let mut last_created_pos: Option<(u64, &str)> = None;
    for (_, e) in hist {
        if e.seq <= last_seq {
            out.push(format!(
                "seq {} of {} after seq {last_seq} (rule 1: seq grows in commit order)",
                e.seq, e.id
            ));
        }
        last_seq = last_seq.max(e.seq);
        match last.get(e.id.as_str()) {
            None => {
                if e.rev != 1 {
                    out.push(format!("{} first seen at rev {} (rule 1)", e.id, e.rev));
                }
                if let Some((pos, id)) = last_created_pos {
                    if e.pos <= pos {
                        out.push(format!(
                            "{} ({:?}) created after {id} but pos {} <= {pos} (rule 2: pos is creation order)",
                            e.id, e.kind, e.pos
                        ));
                    }
                }
                last_created_pos = Some((e.pos, e.id.as_str()));
            }
            Some(before) => {
                if e.rev != before.rev + 1 {
                    out.push(format!(
                        "{} rev {} after rev {} (rule 1: a revision never lowers or skips rev)",
                        e.id, e.rev, before.rev
                    ));
                }
                if e.pos != before.pos {
                    out.push(format!(
                        "{} moved from pos {} to {} (rule 1)",
                        e.id, before.pos, e.pos
                    ));
                }
            }
        }
        last.insert(e.id.as_str(), e);
    }
    let (mut pos, mut seq) = (HashSet::new(), HashSet::new());
    for e in stored {
        if !pos.insert(e.pos) {
            out.push(format!("two entries at pos {} (rule 1)", e.pos));
        }
        if !seq.insert(e.seq) {
            out.push(format!("two entries at seq {} (rule 1)", e.seq));
        }
        match last.get(e.id.as_str()) {
            Some(l) if *l == *e => {}
            Some(l) => out.push(format!(
                "{} stored at seq {} rev {}, its last revision is seq {} rev {} (rule 1)",
                e.id, e.seq, e.rev, l.seq, l.rev
            )),
            None => out.push(format!("{} stored, never made (rule 1)", e.id)),
        }
    }
    for id in last.keys() {
        if !stored.iter().any(|e| e.id == *id) {
            out.push(format!("{id} made, not stored (rule 1)"));
        }
    }
    out
}

/// Rule 7: for every cut `K` (0 and each seq), the history's changes after `K` folded by id are exactly the stored
/// entries with `seq > K`, each once, at their stored state.
fn catch_up(hist: &[(usize, &Entry)], stored: &[&Entry]) -> Vec<String> {
    let mut cuts: Vec<u64> = vec![0];
    cuts.extend(hist.iter().map(|(_, e)| e.seq));
    let mut out = Vec::new();
    for k in cuts {
        let mut folded: BTreeMap<&str, &Entry> = BTreeMap::new();
        for (_, e) in hist.iter().filter(|(_, e)| e.seq > k) {
            folded.insert(e.id.as_str(), e);
        }
        let want: BTreeMap<&str, &Entry> = stored
            .iter()
            .filter(|e| e.seq > k)
            .map(|e| (e.id.as_str(), *e))
            .collect();
        if folded != want {
            out.push(format!(
                "after seq {k}: the changes give {:?}, the stored entries above it are {:?} (rule 7)",
                folded.keys().collect::<Vec<_>>(),
                want.keys().collect::<Vec<_>>()
            ));
            break;
        }
    }
    out
}

/// Rules 3 to 6 on one conversation's stored entries; `made_at`: the trace index that created each entry; `asked_at`:
/// where each approval was first asked.
fn structure(
    stored: &[&Entry],
    made_at: &HashMap<&str, usize>,
    asked_at: &HashMap<String, usize>,
) -> Vec<String> {
    let mut out = Vec::new();
    let by_id: HashMap<&str, &Entry> = stored.iter().map(|e| (e.id.as_str(), *e)).collect();
    let body = |e: &Entry| e.body_as::<AnswerBody>().unwrap_or_default();
    let is_answer = |e: &Entry| matches!(e.kind, EntryKind::Answer | EntryKind::Error);
    // rule 3
    for m in stored.iter().filter(|e| e.kind == EntryKind::Message) {
        let Some(r) = &m.request_id else { continue };
        for e in stored {
            if e.id != m.id && e.request_id.as_deref() == Some(r) && e.pos < m.pos {
                out.push(format!(
                    "{:?} {} (pos {}) of request {r} before its message (pos {}) (rule 3)",
                    e.kind, e.id, e.pos, m.pos
                ));
            }
        }
    }
    // rule 4
    for p in stored.iter().filter(|e| e.kind == EntryKind::Plan) {
        let Some(t) = &p.task_id else { continue };
        for e in stored {
            let of_task = match e.kind {
                EntryKind::Answer | EntryKind::Error => e.task_id.as_deref() == Some(t),
                EntryKind::Question => e.task_id.as_deref().map(root) == Some(t.as_str()),
                _ => false,
            };
            if of_task && e.pos < p.pos {
                out.push(format!(
                    "{:?} {} (pos {}, {}) of task {t} before its plan (pos {}) (rule 4)",
                    e.kind,
                    e.id,
                    e.pos,
                    if is_answer(e) {
                        format!("from {}, {:?}", body(e).from, e.placement)
                    } else {
                        "question".into()
                    },
                    p.pos
                ));
            }
        }
    }
    // rule 5
    for q in stored.iter().filter(|e| e.kind == EntryKind::Question) {
        let (Some(a), Some(t)) = (&q.approval_id, &q.task_id) else {
            continue;
        };
        let Some(asked) = asked_at.get(a) else {
            continue;
        };
        for e in stored.iter().filter(|e| is_answer(e)) {
            let ends = body(e).from == "task" && e.task_id.as_deref() == Some(root(t));
            let Some(told) = made_at.get(e.id.as_str()) else {
                continue;
            };
            if ends && asked < told && e.pos < q.pos {
                out.push(format!(
                    "question {} (asked at item {asked}, pos {}) after the end of its task {} (told at item {told}, pos {}) (rule 5)",
                    q.id, q.pos, e.id, e.pos
                ));
            }
        }
    }
    // rule 6
    for e in stored.iter().filter(|e| is_answer(e)) {
        if let Some(m) = &e.answers {
            match by_id.get(m.as_str()) {
                Some(m) if m.pos < e.pos && m.kind == EntryKind::Message => {}
                Some(m) => out.push(format!(
                    "{} (pos {}) answers {} ({:?}, pos {}) (rule 6)",
                    e.id, e.pos, m.id, m.kind, m.pos
                )),
                None => out.push(format!(
                    "{} answers {m}, not in the conversation (rule 6)",
                    e.id
                )),
            }
        }
        if let Some(f) = body(e).follows {
            match by_id.get(f.as_str()) {
                Some(f) if f.pos < e.pos => {}
                Some(f) => out.push(format!(
                    "{} (pos {}) follows {} (pos {}) (rule 6)",
                    e.id, e.pos, f.id, f.pos
                )),
                None => out.push(format!(
                    "{} follows {f}, not in the conversation (rule 6)",
                    e.id
                )),
            }
        }
    }
    out
}
