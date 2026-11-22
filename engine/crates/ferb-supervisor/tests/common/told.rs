//! The chat's entries a scripted run kept (stage 2, ENG-8), for the "said once" checks of a stop's end (ORCH-688,
//! ORCH-689, APP-1113): the real supervisor's own ChatLog, fed by its streams as the daemon's is, written out at the
//! end, with the trace of what its projectors were fed (for ENG-15's order rules, tests/support_entries).
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use ferb_core::chat_entry::{AnswerBody, Entry, EntryKind, PlanBody};
use ferb_store::Db;
use ferb_supervisor::flow::Supervisor;
use serde_json::Value;

/// The trace items the log hands over.
pub type Items = Arc<Mutex<Vec<Value>>>;

thread_local! {
    /// The trace this test's thread set last (each test runs on its own thread).
    static LAST: std::cell::RefCell<Option<Items>> = const { std::cell::RefCell::new(None) };
}

/// Keep the log's trace from now on (set before the run); also this thread's [`last_trace`].
pub fn traced(sup: &Supervisor) -> Items {
    let items: Items = Arc::new(Mutex::new(Vec::new()));
    let sink = items.clone();
    sup.chat_log().set_trace(Arc::new(move |v: &Value| {
        sink.lock().unwrap().push(v.clone())
    }));
    LAST.with(|l| *l.borrow_mut() = Some(items.clone()));
    items
}

/// The trace this thread set last.
pub fn last_trace() -> Items {
    LAST.with(|l| l.borrow().clone())
        .expect("a trace set before the run")
}

/// Every conversation's stored entries, after everything the log was told is written.
pub fn stored(sup: &Supervisor, db: &Arc<Mutex<Db>>) -> Vec<Entry> {
    sup.chat_flush();
    let db = db.lock().unwrap();
    let conn = db.conn();
    let mut out = Vec::new();
    for c in ferb_store::chat_entries::conversations(conn).unwrap() {
        out.extend(
            ferb_store::chat_entries::page(conn, &c, None, None, 100_000)
                .unwrap()
                .entries,
        );
    }
    out
}

/// An answer's words.
pub fn answer_text(e: &Entry) -> String {
    e.body_as::<AnswerBody>()
        .map(|b| b.text)
        .unwrap_or_default()
}

/// The entries that show `words` to the owner: an answer whose text holds them, or a plan whose line or receipts do
/// (ORCH-638: a stop beside new work says the stop first in the new task's plan).
pub fn saying<'a>(entries: &'a [Entry], words: &str) -> Vec<&'a Entry> {
    entries
        .iter()
        .filter(|e| !e.deleted)
        .filter(|e| match e.kind {
            EntryKind::Answer => answer_text(e).contains(words),
            EntryKind::Plan => e.body_as::<PlanBody>().is_some_and(|p| {
                p.text.as_deref().is_some_and(|t| t.contains(words))
                    || p.receipts
                        .as_ref()
                        .is_some_and(|r| r.iter().any(|t| t.contains(words)))
            }),
            _ => false,
        })
        .collect()
}

/// The answers of one request.
pub fn answers_of<'a>(entries: &'a [Entry], request: &str) -> Vec<&'a Entry> {
    entries
        .iter()
        .filter(|e| e.kind == EntryKind::Answer && e.request_id.as_deref() == Some(request))
        .collect()
}

/// The messages' requests, in the order they were sent.
pub fn messages(entries: &[Entry]) -> Vec<String> {
    let mut m: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.kind == EntryKind::Message)
        .collect();
    m.sort_by_key(|e| e.pos);
    m.iter().filter_map(|e| e.request_id.clone()).collect()
}
