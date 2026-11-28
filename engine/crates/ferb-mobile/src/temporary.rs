//! The temporary chat (the owner, 2026-11-23): one conversation held only in the daemon's memory, whose turns run with
//! all of FERB's tools and no prior context. The phone turns it on and off with the same small icon
//! (`chat.temporary.set`), reads it when it opens (`chat.temporary.get`) and writes in it (`chat.send` with
//! `fresh: true`, with files as the chat sends them).
//!
//! The owner, 2026-11-24 (ORCH-715): "isnt everything same but only its a non saving chat???". So it is the chat with
//! another data source: the one watch (`chat.entries.watch`) follows the conversation the chat tab shows, the
//! temporary chat's (`temporary:<id>`) while one is on, and its changes and live frames (streamed words, the plan
//! line, steps, the answer) reach it exactly as the normal chat's do, with the same lock, link-loss and restart
//! handling. Nothing of it touches the phone's store, outbox or caches on disk: its entries are folded in memory by the
//! same fold and live state as any conversation, and dropped as soon as the Mac says it is off (`set`/`get` answer
//! `{"on": false}`, `chat.status` gives words without its `temporary`, or its watch says `ended`). A message is never
//! queued: with no link it is an error and the words stay with the app to try again.

use std::sync::Arc;

use serde::Serialize;
use serde_json::{json, Value};

use crate::api::{lock, Change, Mobile, MobileResult, OlderState, Shared, TranscriptView};
use crate::entries::{Applied, EntrySet};
use crate::entry::ChatEntry;
use crate::fold::Item;
use crate::live::LiveState;
use crate::store::{Attachment, Cursor};

/// The temporary chat's conversation starts with this (ferb-core `temporary::SESSION_PREFIX`).
pub const SESSION_PREFIX: &str = "temporary:";

/// The conversation is the temporary chat's (held in memory only).
pub fn is_memory(conversation: &str) -> bool {
    conversation.starts_with(SESSION_PREFIX)
}

/// The temporary chat's id from its conversation.
pub(crate) fn id_of(conversation: &str) -> Option<&str> {
    conversation
        .strip_prefix(SESSION_PREFIX)
        .filter(|id| !id.is_empty())
}

/// The temporary chat on now, as the phone holds it (memory only): which one, since when, its rev.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Held {
    pub(crate) id: String,
    pub(crate) started_at: Option<String>,
    pub(crate) rev: i64,
}

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
        let mut t = EntrySet::from_entries(entries).transcript(utc_offset_s);
        // decision: APP-1146 — the Mac can't quote what only its memory holds: no bubble here offers a reply
        t.items.iter_mut().for_each(Item::without_reply_offers);
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
    /// What the app draws of the temporary chat on now: its turns from memory (the watch's, a read's), folded with the
    /// live state as `Mobile::transcript` folds any conversation.
    pub(crate) fn temporary_view(&self) -> Option<TemporaryChat> {
        let held = lock(&self.temporary).clone()?;
        let conversation = format!("{SESSION_PREFIX}{}", held.id);
        let mut st = self.state.lock();
        let conv = st.conv(&conversation).ok()?;
        let mut t = conv.set.transcript(self.config.utc_offset_s);
        t.items.iter_mut().for_each(Item::without_reply_offers);
        let live = conv.live.view(&t);
        Some(TemporaryChat {
            on: true,
            id: Some(held.id),
            started_at: held.started_at,
            rev: held.rev,
            transcript: TranscriptView {
                conversation,
                items: t.items,
                questions: t.questions,
                live,
                outbox: Vec::new(),
                older: OlderState::Complete,
            },
        })
    }

    /// Holds `now` as the temporary chat on (None: off). Another chat than the one held (or none) drops the one held
    /// from memory. Whether it switched (on, off or another chat).
    fn hold(&self, now: Option<Held>) -> bool {
        let before = {
            let mut held = lock(&self.temporary);
            let before = held.clone();
            *held = now.clone();
            before
        };
        let switched = before.as_ref().map(|h| &h.id) != now.as_ref().map(|h| &h.id);
        if switched {
            if let Some(old) = before {
                self.state
                    .lock()
                    .convs_remove(&format!("{SESSION_PREFIX}{}", old.id));
            }
        }
        switched
    }

    /// The temporary chat went on, off or to another chat: the one watch follows the conversation the chat tab shows
    /// now (a sync, which catches the normal chat up and starts the watch on it or on the temporary chat). Locked, the
    /// watch stays closed: the unlock's sync starts it on whichever is shown then (PHN-125).
    fn follow(this: &Arc<Shared>) {
        let s = this.clone();
        this.env.spawner.spawn(Box::new(move || {
            let locked = s.state.lock().locked;
            if s.online() && !locked {
                let _ = Shared::sync_now(&s);
            }
        }));
    }

    /// A read of the whole chat (`chat.temporary.get`/`set`'s answer): held, its turns merged into what the watch
    /// brought (each entry's newest copy wins, so a read never takes back what the watch already showed), the watch
    /// moved when it switched. The chat as the app draws it.
    fn keep_temporary(this: &Arc<Shared>, v: &Value) -> TemporaryChat {
        if v["on"].as_bool() != Some(true) {
            if this.hold(None) {
                this.notify(Change::Temporary);
                Shared::follow(this);
            }
            return TemporaryChat::off();
        }
        let read = TemporaryChat::read(v, this.config.utc_offset_s);
        let Some(id) = read.id.clone() else {
            return TemporaryChat::off();
        };
        let switched = this.hold(Some(Held {
            id: id.clone(),
            started_at: read.started_at.clone(),
            rev: read.rev,
        }));
        let conversation = format!("{SESSION_PREFIX}{id}");
        let entries: Vec<ChatEntry> = ChatEntry::list(&v["entries"])
            .into_iter()
            .map(|mut e| {
                e.cut = false;
                e
            })
            .collect();
        let head = entries.iter().map(|e| e.seq).max();
        let changed = {
            let mut st = this.state.lock();
            match st.conv(&conversation) {
                Ok(conv) => {
                    let mut changed = false;
                    for e in entries {
                        conv.live.entry(&e);
                        changed |= conv.set.apply(e) == Applied::Kept;
                    }
                    if let (Some(m), Some(head)) = (conv.memory.as_mut(), head) {
                        let at = m.cursor.as_ref().map(|c| c.after_seq).unwrap_or(0);
                        if head > at {
                            m.cursor = Some(Cursor {
                                epoch: id.clone(),
                                after_seq: head,
                            });
                        }
                    }
                    changed
                }
                Err(_) => false,
            }
        };
        if switched || changed {
            this.notify(Change::Temporary);
        }
        if switched {
            Shared::follow(this);
        }
        this.temporary_view().unwrap_or(read)
    }
}

impl crate::api::State {
    pub(crate) fn convs_remove(&mut self, conversation: &str) {
        self.drop_conv(conversation);
    }
}

impl Mobile {
    /// The temporary chat as the Mac holds it now (`chat.temporary.get`; the unlock window, like the chat): on or off,
    /// and while on its turns folded. Read when the app opens, unlocks or comes back; after that the watch keeps it
    /// (`last_temporary_chat`). Network: not on the UI thread.
    pub fn temporary_chat(&self) -> MobileResult<TemporaryChat> {
        let t = self.shared.transport_online()?;
        let v = Shared::call(&t, "chat.temporary.get", json!({}))?;
        Ok(Shared::keep_temporary(&self.shared, &v))
    }

    /// Turns the temporary chat on or off (`chat.temporary.set {on}`): on keeps one already on; off ends it on the Mac
    /// (its tasks stop, its turns and files are forgotten) and the phone drops its copy. Never queued.
    pub fn set_temporary_chat(&self, on: bool) -> MobileResult<TemporaryChat> {
        let t = self.shared.transport_online()?;
        let v = Shared::call(&t, "chat.temporary.set", json!({"on": on}))?;
        Ok(Shared::keep_temporary(&self.shared, &v))
    }

    /// Writes in the temporary chat (`chat.send {text, fresh: true, client_id}`), sent now and never through the
    /// outbox: with no link, or a link that drops before the Mac's acceptance, it is an error and the words stay with
    /// the app. Then reads the temporary chat once (the Mac starts one when none is on).
    pub fn send_temporary(&self, text: &str) -> MobileResult<TemporaryChat> {
        self.send_temporary_with(text, &[])
    }

    /// The same with files, as the chat sends them (`send_with`): this phone's finished uploads on the Mac, by their
    /// ids. The Mac moves them into the temporary chat's own folder and deletes them when it ends (ORCH-717).
    pub fn send_temporary_with(
        &self,
        text: &str,
        attachments: &[Attachment],
    ) -> MobileResult<TemporaryChat> {
        let t = self.shared.transport_online()?;
        let mut params =
            json!({"text": text, "fresh": true, "client_id": self.shared.config.client_id});
        if !attachments.is_empty() {
            params["attachments"] = json!(attachments
                .iter()
                .map(|a| a.upload_id.clone())
                .collect::<Vec<_>>());
        }
        t.send(&params.to_string())
            .map_err(crate::api::MobileError::from_link)?;
        self.temporary_chat()
    }

    /// The temporary chat while it is on, as its watch keeps it (memory only; none after a start until it is read,
    /// and none once the Mac says it is off). Safe on the UI thread.
    pub fn last_temporary_chat(&self) -> MobileResult<Option<TemporaryChat>> {
        if self.shared.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(crate::api::MobileError::closed());
        }
        Ok(self.shared.temporary_view())
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

/// The Mac's status moved the temporary chat: on, off or another chat is held and told, and the watch follows the
/// chat tab (`Temporary`); a new rev of the chat held needs nothing (its watch brings the change), unless no watch
/// follows it now (it failed): then it is started again.
pub(crate) fn status_moved(this: &Arc<Shared>, now: Option<crate::status::TemporaryState>) {
    let held = now.map(|t| Held {
        id: t.id,
        started_at: t.started_at,
        rev: t.rev,
    });
    let conversation = held.as_ref().map(|h| format!("{SESSION_PREFIX}{}", h.id));
    if this.hold(held) {
        this.notify(Change::Temporary);
        Shared::follow(this);
    } else if let Some(c) = conversation {
        if !this.state.lock().watching(&c) {
            Shared::follow(this);
        }
    }
}

/// Its watch said the temporary chat `conversation` ended: dropped and told, when it is the one held; the watch
/// follows the normal chat again.
pub(crate) fn ended(this: &Arc<Shared>, conversation: &str) {
    let held = lock(&this.temporary)
        .as_ref()
        .is_some_and(|h| id_of(conversation) == Some(h.id.as_str()));
    if held {
        this.hold(None);
        this.notify(Change::Temporary);
    } else {
        this.state.lock().convs_remove(conversation);
    }
    Shared::follow(this);
}
