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
    mark_folded(&st, &mut out);
    ok(out)
}

/// MEM-461: a notice the engine folded into another place (the review of what an import brought in) is marked
/// `folded` on the page as it goes out, beside the entry as the app kept it (never in it), so FERB.app doesn't draw it
/// again whether or not it was running when the fold was made. Its saved copy names the notice by the engine's `ref`.
fn mark_folded(st: &State, out: &mut Value) {
    let Some(entries) = out.get_mut("entries").and_then(Value::as_array_mut) else {
        return;
    };
    let refs: Vec<String> = entries
        .iter()
        .filter(|e| e["kind"] == "notice")
        .filter_map(|e| e["ref"].as_str().map(str::to_owned))
        .collect();
    if refs.is_empty() {
        return;
    }
    let folded = ferb_core::chat_entry::FOLDED;
    let Ok(rows) = st.store.query(
        &format!(
            "SELECT ref, json_extract(body_json, '$.{folded}') FROM chat_entries WHERE kind = 'notice' \
             AND deleted = 0 AND ref IN ({}) AND coalesce(json_extract(body_json, '$.{folded}'), '') != ''",
            ferb_learning::store::marks(refs.len())
        ),
        refs.into_iter().map(ferb_learning::store::Sql::Text).collect(),
    ) else {
        return;
    };
    let into: std::collections::BTreeMap<String, String> =
        rows.iter().map(|r| (r.s_at(0), r.s_at(1))).collect();
    for entry in entries.iter_mut() {
        let place = entry["ref"].as_str().and_then(|r| into.get(r)).cloned();
        if let (Some(place), true) = (place, entry["kind"] == "notice") {
            entry[folded] = json!(place);
        }
    }
}

/// `chat.history.days {}`: each day with an entry, newest first.
async fn days(st: Rc<State>, _params: Params) -> HandlerResult {
    ok(json!({"days": history(&st).days().map_err(store_error)?}))
}
