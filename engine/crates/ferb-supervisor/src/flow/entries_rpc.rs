//! The calls a client reads the chat with (stage 2, slice 5; ENG-8, ENG-10): the one conversation kept on the Mac, read
//! like from a server. `chat.entries.page` (older entries on scroll), `chat.entries.changes` (what changed after a
//! cursor), `chat.entries.get` (one entry, a long body in pieces), `chat.entries.days`, `chat.entries.watch` (a stream:
//! the changes a client missed, then live), `chat.entries.mark` (how far a client has received and read).
//!
//! `conversation` defaults to the current conversation (`continuity.current()`), never the literal "default". A cursor
//! is the conversation's `epoch` and a `seq`: a cursor of another epoch answers `reset_required` (read from the start),
//! one above the head `cursor_ahead`. `max_bytes` (for the phone link's frame limits) caps a reply's serialized size:
//! fewer entries come back (`more`), and an entry too big on its own comes back with its words cut and `body_cut
//! {chars, full_chars}` (`chat.entries.get` reads the rest). Entries are ENG-8's contract, serialized as the store keeps
//! them. Roles (auth.rs): FERB.app's role now; stage 3 adds the phone role (the reads Private, `mark` Plain).
//!
//! Notifications (ENG-10, "a result seen on one device is seen for notifications everywhere"): the shown flags stay
//! global and are not changed here (`task_results.shown` set by `results.ack` and by a stream that carried the outcome,
//! `timers.chat_shown`, `attention.ack`); every client's history reads the entries, which hold every result whatever was
//! acked, so a result one client acknowledged still reaches every other client's history. A client's marks
//! (`delivered_seq`, `read_pos`) are its own reading position, not a notification flag.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use ferb_core::chat_entry::Entry;
use ferb_core::interfaces::BoxFuture;
use ferb_rpc::server::{EventStream, HandlerError, HandlerResult};
use ferb_rpc::wire::Params;
use ferb_store::chat_entries as store;
use serde_json::{json, Map, Value};

use super::chat_log::{Watch, Watched};
use super::Inner;

/// A page's default and largest size (entries).
pub const PAGE: usize = 50;
pub const PAGE_MAX: usize = 500;
/// The smallest `max_bytes` a call takes (an entry's frame and its fields need room).
pub const MAX_BYTES_MIN: usize = 2048;
/// `chat.entries.get`'s default and largest piece of a long body (characters).
pub const PIECE: usize = 16_000;
pub const PIECE_MAX: usize = 200_000;
/// Room kept in a capped reply for its own fields around the entries.
pub(crate) const ROOM: usize = 512;

fn refusal(message: impl Into<String>) -> HandlerError {
    HandlerError::refusal("FerbError", message)
}

fn fault(e: ferb_store::Error) -> HandlerError {
    match e {
        ferb_store::Error::Value(m) | ferb_store::Error::Refused(m) => refusal(m),
        other => HandlerError::fault("OperationalError", other.to_string()),
    }
}

fn obj(v: Value) -> Params {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// A whole number of at least 0 (a JSON integer; a bool is not one), or None when absent.
fn number(params: &Params, key: &str) -> Result<Option<u64>, HandlerError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) if n.as_u64().is_some() => Ok(n.as_u64()),
        Some(_) => Err(refusal(format!(
            "{key} must be a whole number of at least 0"
        ))),
    }
}

fn text(params: &Params, key: &str) -> Result<Option<String>, HandlerError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.is_empty() => Ok(Some(s.clone())),
        Some(_) => Err(refusal(format!("{key} must be a text"))),
    }
}

fn limit(params: &Params) -> Result<usize, HandlerError> {
    match number(params, "limit")? {
        None => Ok(PAGE),
        Some(n) if (1..=PAGE_MAX as u64).contains(&n) => Ok(n as usize),
        Some(_) => Err(refusal(format!(
            "limit must be a whole number from 1 to {PAGE_MAX}"
        ))),
    }
}

fn max_bytes(params: &Params) -> Result<Option<usize>, HandlerError> {
    match number(params, "max_bytes")? {
        None => Ok(None),
        Some(n) if n as usize >= MAX_BYTES_MIN => Ok(Some(n as usize)),
        Some(_) => Err(refusal(format!(
            "max_bytes must be at least {MAX_BYTES_MIN}"
        ))),
    }
}

/// `conversation`, else the current one; its stream (made on first read, so a client always gets an epoch).
fn conversation(inner: &Inner, params: &Params) -> Result<(String, store::Stream), HandlerError> {
    let c = match text(params, "conversation")? {
        Some(c) => c,
        None => inner
            .continuity(|c| c.current())
            .map_err(|e| HandlerError::fault("OperationalError", e.to_string()))?,
    };
    // decision: ORCH-716 — a temporary conversation has no stored stream: it is read only by its watch (from memory),
    // never opened in the store here
    if ferb_core::temporary::is_session(&c) {
        return Err(refusal(
            "The temporary chat is kept only in memory: read it with chat.entries.watch.",
        ));
    }
    let db = inner.db();
    let stream = store::open_stream(db.conn(), &c, &store::new_epoch()).map_err(fault)?;
    Ok((c, stream))
}

/// An entry as a reply carries it: the contract's JSON, cut to `cap` bytes when given (its words shortened, with
/// `body_cut {chars, full_chars}`).
pub fn entry_json(entry: &Entry, cap: Option<usize>) -> Value {
    let whole = serde_json::to_value(entry).unwrap_or(Value::Null);
    let Some(cap) = cap else {
        return whole;
    };
    if whole.to_string().len() <= cap {
        return whole;
    }
    let Some(said) = entry.body.get("text").and_then(Value::as_str) else {
        // nothing to cut but the body: it is left out, said so
        let mut v = whole;
        v["body"] = json!({});
        v["body_cut"] = json!({"chars": 0, "full_chars": 0});
        return v;
    };
    let chars: Vec<char> = said.chars().collect();
    let with = |n: usize| -> Value {
        let mut v = whole.clone();
        v["body"]["text"] = json!(chars[..n].iter().collect::<String>());
        v["body_cut"] = json!({"chars": n, "full_chars": chars.len()});
        v
    };
    // the most words that fit
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if with(mid).to_string().len() <= cap {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    with(lo)
}

/// Entries for a reply, in order, within `max_bytes` when given: each cut to fit on its own; then as many as fit
/// (from the front, or from the back when `newest` is true); whether some were left out.
fn fit(entries: &[Entry], max_bytes: Option<usize>, newest: bool) -> (Vec<Value>, bool) {
    let Some(max) = max_bytes else {
        return (entries.iter().map(|e| entry_json(e, None)).collect(), false);
    };
    let budget = max.saturating_sub(ROOM);
    let mut out: VecDeque<Value> = VecDeque::new();
    let mut used = 0usize;
    let order: Box<dyn Iterator<Item = &Entry>> = if newest {
        Box::new(entries.iter().rev())
    } else {
        Box::new(entries.iter())
    };
    let mut cut = false;
    for e in order {
        let mut v = entry_json(e, Some(budget));
        let mut n = v.to_string().len() + 1;
        if used + n > budget && !out.is_empty() {
            // the room left takes this entry cut, when there is room for some of its words; then the reply is full
            cut = true;
            let left = budget - used;
            if left < MAX_BYTES_MIN / 2 {
                break;
            }
            v = entry_json(e, Some(left - 1));
            n = v.to_string().len() + 1;
            if used + n > budget {
                break;
            }
            if newest {
                out.push_front(v);
            } else {
                out.push_back(v);
            }
            break;
        }
        used += n;
        if newest {
            out.push_front(v);
        } else {
            out.push_back(v);
        }
    }
    (out.into_iter().collect(), cut)
}

/// `chat.entries.page {conversation?, before_pos?, day?, limit?, max_bytes?}`: the newest entries before `before_pos`
/// (or the newest of all), oldest first; `more` when older ones exist, `before_pos` to ask for them.
pub fn page(inner: &Inner, params: &Params) -> HandlerResult {
    let (c, stream) = conversation(inner, params)?;
    let before = number(params, "before_pos")?;
    let day = text(params, "day")?;
    if let Some(d) = &day {
        let ok = d.len() == 10
            && d.char_indices().all(|(i, ch)| {
                if i == 4 || i == 7 {
                    ch == '-'
                } else {
                    ch.is_ascii_digit()
                }
            });
        if !ok {
            return Err(refusal("day must be YYYY-MM-DD"));
        }
    }
    let (limit, cap) = (limit(params)?, max_bytes(params)?);
    let found = {
        let db = inner.db();
        store::page(db.conn(), &c, before, day.as_deref(), limit).map_err(fault)?
    };
    let (entries, cut) = fit(&found.entries, cap, true);
    let kept = &found.entries[found.entries.len() - entries.len()..];
    let more = found.more || cut;
    Ok(obj(json!({
        "conversation": c, "epoch": stream.epoch, "head": stream.last_seq,
        "entries": entries, "more": more,
        "before_pos": if more { kept.first().map(|e| json!(e.pos)).unwrap_or(Value::Null) } else { Value::Null },
    })))
}

/// A cursor's verdict as a reply: None when it is good.
fn cursor_reply(c: &str, check: store::CursorCheck, stream: &store::Stream) -> Option<Value> {
    match check {
        store::CursorCheck::Ok { .. } => None,
        store::CursorCheck::ResetRequired { epoch } => Some(json!({
            "conversation": c, "reset_required": true, "epoch": epoch.unwrap_or_else(|| stream.epoch.clone()),
            "head": stream.last_seq})),
        store::CursorCheck::Ahead { head } => Some(json!({
            "conversation": c, "cursor_ahead": true, "epoch": stream.epoch, "head": head})),
    }
}

/// `chat.entries.changes {conversation?, epoch, after_seq, limit?, max_bytes?}`: every entry changed after the cursor,
/// once each at its latest state, in `seq` order; `next_seq` is the cursor to ask from next (`more` when more wait).
pub fn changes(inner: &Inner, params: &Params) -> HandlerResult {
    let (c, stream) = conversation(inner, params)?;
    let epoch = text(params, "epoch")?
        .ok_or_else(|| refusal("epoch is needed (from chat.entries.page)"))?;
    let after = number(params, "after_seq")?.ok_or_else(|| refusal("after_seq is needed"))?;
    let (limit, cap) = (limit(params)?, max_bytes(params)?);
    let db = inner.db();
    let check = store::check_cursor(db.conn(), &c, &epoch, after).map_err(fault)?;
    if let Some(reply) = cursor_reply(&c, check, &stream) {
        return Ok(obj(reply));
    }
    let found = store::changes(db.conn(), &c, after, limit).map_err(fault)?;
    drop(db);
    let (entries, cut) = fit(&found.entries, cap, false);
    let next = found.entries[..entries.len()]
        .last()
        .map(|e| e.seq)
        .unwrap_or(after);
    Ok(obj(json!({
        "conversation": c, "epoch": stream.epoch, "head": found.head, "entries": entries,
        "more": found.more || cut, "next_seq": next,
    })))
}

/// `chat.entries.get {id, offset?, max_chars?}`: one entry; a long `body.text` comes in pieces (`text_piece {offset,
/// chars, full_chars, more}`).
pub fn get(inner: &Inner, params: &Params) -> HandlerResult {
    let id = text(params, "id")?.ok_or_else(|| refusal("id is needed"))?;
    let offset = number(params, "offset")?.unwrap_or(0) as usize;
    let piece = match number(params, "max_chars")? {
        None => PIECE,
        Some(n) if (1..=PIECE_MAX as u64).contains(&n) => n as usize,
        Some(_) => {
            return Err(refusal(format!(
                "max_chars must be a whole number from 1 to {PIECE_MAX}"
            )))
        }
    };
    let entry = {
        let db = inner.db();
        store::get(db.conn(), &id).map_err(fault)?
    };
    // a temporary chat's entry has no stored copy: its words whole from memory (a watch frame cut them)
    let Some(entry) = entry.or_else(|| inner.temporary_entry(&id)) else {
        return Err(refusal("There's no chat entry with that id."));
    };
    let mut v = entry_json(&entry, None);
    if let Some(said) = entry.body.get("text").and_then(Value::as_str) {
        let chars: Vec<char> = said.chars().collect();
        if offset > 0 || chars.len() > piece {
            let from = offset.min(chars.len());
            let to = (from + piece).min(chars.len());
            v["body"]["text"] = json!(chars[from..to].iter().collect::<String>());
            v["text_piece"] = json!({"offset": from, "chars": to - from, "full_chars": chars.len(),
                                     "more": to < chars.len()});
        }
    }
    Ok(obj(json!({"entry": v})))
}

/// `chat.entries.days {conversation?}`: each day that has an entry, newest first.
pub fn days(inner: &Inner, params: &Params) -> HandlerResult {
    let (c, stream) = conversation(inner, params)?;
    let days = {
        let db = inner.db();
        store::days(db.conn(), &c).map_err(fault)?
    };
    Ok(obj(
        json!({"conversation": c, "epoch": stream.epoch, "days": days}),
    ))
}

/// `chat.entries.mark {client_id, conversation?, delivered_seq?, read_pos?}`: a client's marks, each only moving
/// forward (a lower one is kept as it was).
pub fn mark(inner: &Inner, params: &Params) -> HandlerResult {
    let client = text(params, "client_id")?.ok_or_else(|| refusal("client_id is needed"))?;
    if client.chars().count() > super::chat::CLIENT_ID_MAX {
        return Err(refusal(format!(
            "client_id must be a short text of at most {} characters",
            super::chat::CLIENT_ID_MAX
        )));
    }
    let (c, _) = conversation(inner, params)?;
    let (delivered, read) = (
        number(params, "delivered_seq")?,
        number(params, "read_pos")?,
    );
    let at = inner.now_iso();
    let db = inner.db();
    let marks = db
        .transaction(|conn| {
            store::touch_client(conn, &client, &at)?;
            store::mark(conn, &client, &c, delivered, read, &at)
        })
        .map_err(fault)?;
    Ok(obj(json!({"marks": {
        "client_id": marks.client_id, "conversation": marks.conversation_id,
        "delivered_seq": marks.delivered_seq, "read_pos": marks.read_pos, "updated_at": marks.updated_at}})))
}

/// `chat.entries.watch {conversation?, epoch?, after_seq?, max_bytes?}`: a stream. With a cursor: the changes after it
/// (`{"type": "change", "catchup": true, "entry"}`, each entry once), then `{"type": "caught_up", "epoch", "head"}`;
/// without one, `caught_up` at once. Then each committed change (`{"type": "change", "entry"}`) and each live frame of
/// the conversation (`{"type": "live", "request_id", "frame"}`, never stored) as they happen. A watcher too slow to
/// keep up gets `{"type": "behind"}` and the stream ends (it reads `changes` from its cursor and watches again). A bad
/// cursor: one `reset_required` or `cursor_ahead` frame, then the end. A temporary conversation (`temporary:<id>`) is
/// watched the same way from the daemon's memory, its cursor's `epoch` the chat's id; its end, or a conversation that
/// is not the chat on now, is one `{"type": "ended", "conversation"}` frame, then the end.
pub fn watch(inner: &Arc<Inner>, params: &Params) -> Result<Box<dyn EventStream>, HandlerError> {
    let epoch = text(params, "epoch")?;
    let after = number(params, "after_seq")?;
    let cap = max_bytes(params)?;
    if after.is_some() && epoch.is_none() {
        return Err(refusal("after_seq goes with its epoch"));
    }
    // decision: ORCH-716 — the temporary chat's watch is this same stream, fed from the daemon's memory (temporary.rs
    // `temporary_watch`): its changes and live frames as the kept chat's, and `ended` when it ends; nothing stored
    if let Some(c) = text(params, "conversation")?.filter(|c| ferb_core::temporary::is_session(c)) {
        let cursor = epoch.as_deref().map(|e| (e, after.unwrap_or(0)));
        let (first, watch, sent) = inner.temporary_watch(&c, cursor, cap);
        return Ok(Box::new(Frames {
            first,
            watch,
            inner: inner.clone(),
            sent,
            cap,
            ended: false,
            conversation: c,
        }));
    }
    let (c, stream) = conversation(inner, params)?;
    let mut first: VecDeque<Value> = VecDeque::new();
    // the watcher is on before the catch-up is read: nothing committed in between is missed (a change read both ways
    // is sent once, by its seq)
    let watch = inner.chat_log.watch(&c);
    let mut sent = {
        let db = inner.db();
        store::head(db.conn(), &c).map_err(fault)?
    };
    if let (Some(epoch), Some(after)) = (&epoch, after) {
        let db = inner.db();
        let check = store::check_cursor(db.conn(), &c, epoch, after).map_err(fault)?;
        if let Some(mut reply) = cursor_reply(&c, check, &stream) {
            drop(db);
            inner.chat_log.unwatch(watch.id);
            let kind = if reply.get("reset_required").is_some() {
                "reset_required"
            } else {
                "cursor_ahead"
            };
            reply["type"] = json!(kind);
            return Ok(Box::new(Frames {
                first: VecDeque::from([reply]),
                watch: None,
                inner: inner.clone(),
                sent: 0,
                cap,
                ended: false,
                conversation: c,
            }));
        }
        let mut from = after;
        loop {
            let found = store::changes(db.conn(), &c, from, PAGE_MAX).map_err(fault)?;
            for e in &found.entries {
                let mut v = json!({"type": "change", "catchup": true});
                v["entry"] = entry_json(e, cap.map(|m| m.saturating_sub(ROOM)));
                first.push_back(v);
                from = e.seq;
            }
            sent = sent.max(from);
            if !found.more {
                break;
            }
        }
    }
    first.push_back(
        json!({"type": "caught_up", "conversation": c, "epoch": stream.epoch, "head": sent}),
    );
    Ok(Box::new(Frames {
        first,
        watch: Some(watch),
        inner: inner.clone(),
        sent,
        cap,
        ended: false,
        conversation: c,
    }))
}

/// The watch stream: its first frames, then the live feed.
struct Frames {
    first: VecDeque<Value>,
    watch: Option<Watch>,
    inner: Arc<Inner>,
    /// The highest seq sent: a change at or below it was sent already.
    sent: u64,
    cap: Option<usize>,
    ended: bool,
    /// The conversation watched (said in its `ended` frame).
    conversation: String,
}

impl EventStream for Frames {
    fn next(&mut self) -> BoxFuture<'_, Option<HandlerResult>> {
        Box::pin(async move {
            if let Some(v) = self.first.pop_front() {
                return Some(Ok(obj(v)));
            }
            if self.ended {
                return None;
            }
            let Some(watch) = self.watch.as_mut() else {
                self.ended = true;
                return None;
            };
            loop {
                match watch.rx.recv().await {
                    Some(Watched::Change(e)) => {
                        if e.seq <= self.sent {
                            continue;
                        }
                        self.sent = e.seq;
                        let mut v = json!({"type": "change"});
                        v["entry"] = entry_json(&e, self.cap.map(|m| m.saturating_sub(ROOM)));
                        return Some(Ok(obj(v)));
                    }
                    Some(Watched::Live { request_id, frame }) => {
                        let v = json!({"type": "live", "request_id": request_id, "frame": frame});
                        if self.cap.is_some_and(|m| v.to_string().len() > m) {
                            continue; // a live frame too big for the link is left out: it is never stored anyway
                        }
                        return Some(Ok(obj(v)));
                    }
                    Some(Watched::Ended) => {
                        // the temporary chat ended: nothing more comes (its watch reads the chat on now, or the kept chat)
                        self.ended = true;
                        return Some(Ok(obj(
                            json!({"type": "ended", "conversation": self.conversation}),
                        )));
                    }
                    None => {
                        self.ended = true;
                        if watch.behind.load(Ordering::SeqCst) {
                            return Some(Ok(obj(json!({"type": "behind", "head": self.sent}))));
                        }
                        return None;
                    }
                }
            }
        })
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        if let Some(w) = &self.watch {
            self.inner.chat_log.unwatch(w.id);
        }
    }
}
