//! The temporary chat (the owner, 2026-11-23): one conversation held only in the daemon's memory, whose turns run with
//! all of FERB's tools and no prior context. The phone reads it from the Mac (`chat.temporary.get`), turns it on and
//! off with the same small icon (`chat.temporary.set`) and writes in it (`chat.send` with `fresh: true`).
//!
//! Nothing of it touches the phone's store, outbox or caches on disk: the entries are folded in memory by the same fold
//! as any conversation (`EntrySet::transcript`) and the last copy read is kept in memory only, dropped as soon as the
//! Mac says it is off (`chat.temporary.get`/`set` answer `{"on": false}`, or `chat.status` gives words without its
//! `temporary`). A message is never queued: with no link it is an error and the words stay with the app to try again.

use std::sync::Arc;

use serde::Serialize;
use serde_json::{json, Value};

use crate::api::{lock, Change, Mobile, MobileResult, OlderState, Shared, TranscriptView};
use crate::entries::EntrySet;
use crate::entry::ChatEntry;
use crate::live::LiveState;

/// The temporary chat as the app draws it: whether one is on, which, since when, its change number, and its turns
/// folded as FERB.app folds a conversation (`transcript.conversation` is "temporary:<id>"; empty while off).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TemporaryChat {
    pub on: bool,
    pub id: Option<String>,
    pub started_at: Option<String>,
    pub rev: i64,
    pub transcript: TranscriptView,
}

impl TemporaryChat {
    /// None on: the normal chat.
    pub fn off() -> TemporaryChat {
        TemporaryChat {
            on: false,
            id: None,
            started_at: None,
            rev: 0,
            transcript: TranscriptView {
                conversation: String::new(),
                items: Vec::new(),
                questions: Vec::new(),
                live: Vec::new(),
                outbox: Vec::new(),
                older: OlderState::Complete,
            },
        }
    }

    /// From `chat.temporary.get`/`set`'s answer: its entries (ENG-8, as `chat.entries.page` gives them) folded by the
    /// fold every conversation uses. An entry whose words a reply cut keeps the words it has (there is no stored copy
    /// to read whole: the temporary chat's entries live only in the daemon's memory).
    pub fn read(v: &Value, utc_offset_s: i32) -> TemporaryChat {
        if v["on"].as_bool() != Some(true) {
            return TemporaryChat::off();
        }
        let id = v["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let conversation = v["conversation"]
            .as_str()
            .map(str::to_string)
            .or_else(|| id.as_ref().map(|i| format!("temporary:{i}")))
            .unwrap_or_default();
        let entries = ChatEntry::list(&v["entries"]).into_iter().map(|mut e| {
            e.cut = false;
            e
        });
        let t = EntrySet::from_entries(entries).transcript(utc_offset_s);
        // decision: APP-1126 — what the chat draws live over its turns with no frame yet (the typing dots of a turn that
        // is working, its card's and checklist's rows): the same view as the normal chat's (`Mobile::transcript`), from
        // a live state that no watch feeds here, so nothing streams in it
        let live = LiveState::new().view(&t);
        TemporaryChat {
            on: true,
            id,
            started_at: v["started_at"].as_str().map(str::to_string),
            rev: v["rev"].as_i64().unwrap_or(0),
            transcript: TranscriptView {
                conversation,
                items: t.items,
                questions: t.questions,
                live,
                outbox: Vec::new(),
                older: OlderState::Complete,
            },
        }
    }
}

impl Shared {
    /// Keeps the copy just read (memory only) and tells the app when it moved.
    fn keep_temporary(&self, now: TemporaryChat) -> TemporaryChat {
        let moved = {
            let mut held = lock(&self.temporary);
            let moved = held.as_ref() != Some(&now) && (now.on || held.is_some());
            *held = now.on.then(|| now.clone());
            moved
        };
        if moved {
            self.notify(Change::Temporary);
        }
        now
    }
}

impl Mobile {
    /// The temporary chat as the Mac holds it now (`chat.temporary.get`; the unlock window, like the chat): on or off,
    /// and while on its turns folded. Kept in memory as `last_temporary_chat`. Network: not on the UI thread.
    pub fn temporary_chat(&self) -> MobileResult<TemporaryChat> {
        let t = self.shared.transport_online()?;
        let v = Shared::call(&t, "chat.temporary.get", json!({}))?;
        Ok(self.read_temporary(&v))
    }

    /// Turns the temporary chat on or off (`chat.temporary.set {on}`): on keeps one already on; off ends it on the Mac
    /// (its tasks stop, its turns are forgotten) and the phone drops its copy. Never queued.
    pub fn set_temporary_chat(&self, on: bool) -> MobileResult<TemporaryChat> {
        let t = self.shared.transport_online()?;
        let v = Shared::call(&t, "chat.temporary.set", json!({"on": on}))?;
        Ok(self.read_temporary(&v))
    }

    /// Writes in the temporary chat (`chat.send {text, fresh: true, client_id}`), sent now and never through the
    /// outbox: with no link, or a link that drops before the Mac's acceptance, it is an error and the words stay with
    /// the app. Then reads the temporary chat again (the Mac starts one when none is on).
    pub fn send_temporary(&self, text: &str) -> MobileResult<TemporaryChat> {
        let t = self.shared.transport_online()?;
        let params =
            json!({"text": text, "fresh": true, "client_id": self.shared.config.client_id});
        t.send(&params.to_string())
            .map_err(crate::api::MobileError::from_link)?;
        self.temporary_chat()
    }

    /// The temporary chat as last read, while it is on (memory only; none after a start until it is read, and none
    /// once the Mac says it is off). Safe on the UI thread.
    pub fn last_temporary_chat(&self) -> MobileResult<Option<TemporaryChat>> {
        if self.shared.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(crate::api::MobileError::closed());
        }
        Ok(lock(&self.shared.temporary).clone())
    }

    fn read_temporary(&self, v: &Value) -> TemporaryChat {
        let now = TemporaryChat::read(v, self.shared.config.utc_offset_s);
        self.shared.keep_temporary(now)
    }
}

/// What a new status says of the temporary chat: whether it moved (on, off, another id or rev) against the one before.
/// A status whose words are held back (a locked phone, PHN-126) that leaves `temporary` out says nothing of it: the one
/// before is carried over, so locking the phone never reads as the chat ending.
pub(crate) fn carry(
    before: Option<&crate::status::PhoneStatus>,
    now: &mut crate::status::PhoneStatus,
) -> bool {
    let was = before.and_then(|s| s.temporary.clone());
    if now.temporary.is_none() && !now.words {
        now.temporary = was.clone();
    }
    was != now.temporary
}

/// Drops the copy held once the Mac's status says no temporary chat is on.
pub(crate) fn status_moved(this: &Arc<Shared>, on: bool) {
    if !on {
        lock(&this.temporary).take();
    }
    this.notify(Change::Temporary);
}
