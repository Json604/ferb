//! The chat history RPCs (APP-805): FERB.app writes each chat entry as it changes and reads a day a page at a time;
//! its old saved chat comes over once. FERB.app only (ferb-rpc's APP_ONLY table). Port of
//! `ferb/daemon/areas/chat_history.py`. Must not interpret an entry (ferb-store's `ChatHistory` keeps it as the app
//! sent it, redacted).

use std::rc::Rc;

use ferb_rpc::server::{HandlerError, HandlerResult};
use ferb_rpc::wire::Params;
use ferb_store::chat_history::{ChatHistory, PAGE};
use serde_json::{json, Value};

use super::{ok, Handler, State};
use crate::shared::{is_true, refusal_as, text};

fn history(st: &State) -> ChatHistory<'_> {
    let redactor = st.redactor.clone();
    let clock = st.clock.clone();
    ChatHistory::new(
        st.db(),
        Box::new(move |v: &Value| redactor.value(v)),
        Box::new(move || ferb_clock::py::iso(&clock.now())),
    )
}

fn store_error(e: ferb_store::Error) -> HandlerError {
    match e {
        ferb_store::Error::Refused(m) => refusal_as("Refused", m),
        other => super::store_error(other),
    }
}

pub(crate) fn handlers() -> Vec<(&'static str, Handler)> {
    vec![
        ("chat.history.put", |s, p| Box::pin(put(s, p))),
        ("chat.history.import", |s, p| Box::pin(import(s, p))),
        ("chat.history.page", |s, p| Box::pin(page(s, p))),
        ("chat.history.days", |s, p| Box::pin(days(s, p))),
    ]
}

/// `chat.history.put {entries}`: entries that changed in the app replace the ones kept.
async fn put(st: Rc<State>, params: Params) -> HandlerResult {
    let entries = params.get("entries").cloned().unwrap_or(Value::Null);
    ok(history(&st).write(&entries, true).map_err(store_error)?)
}

/// `chat.history.import {entries}`: the chat an older FERB.app kept, sent once; an entry already kept stays.
async fn import(st: Rc<State>, params: Params) -> HandlerResult {
    let entries = params.get("entries").cloned().unwrap_or(Value::Null);
    ok(history(&st).write(&entries, false).map_err(store_error)?)
}

/// `chat.history.page {day, before?, limit?, open?}`.
async fn page(st: Rc<State>, params: Params) -> HandlerResult {
    let store = history(&st);
    let before = text(&params, "before");
    // the store's `isinstance(limit, int)` check: a value that isn't a whole number is refused as one out of range
    let limit = match params.get("limit") {
        None => PAGE,
        Some(v) if v.is_i64() => v.as_i64().unwrap_or(0),
        Some(_) => 0,
    };
    let day = text(&params, "day");
    let mut out = store
        .page(&day, (!before.is_empty()).then_some(before.as_str()), limit)
        .map_err(store_error)?;
    if is_true(&params, "open") {
        out["open"] = json!(store.open().map_err(store_error)?);
    }
    ok(out)
}

/// `chat.history.days {}`: each day with an entry, newest first.
async fn days(st: Rc<State>, _params: Params) -> HandlerResult {
    ok(json!({"days": history(&st).days().map_err(store_error)?}))
}
