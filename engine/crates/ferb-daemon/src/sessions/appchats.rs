//! App chats (kb HAND-668..675): the desktop chat apps' conversations as sessions in the same list, read from the app's
//! own window by code (ferb-sessions `appchat`). Looked at only while someone watches: the owner has it open (a client
//! read its timeline lately), FERB's task holds it, or a send waits for its reply; every LOOK_REPLYING_S while the app
//! replies, every LOOK_IDLE_S otherwise. A window FERB can't read where it is (another Space) is read by bringing its
//! app forward only while the owner is away (`may_take_front`), every FOREGROUND_LOOK_S (the numbers: HAND-674).
//! Never a model.

use std::collections::HashMap;

use ferb_sessions::appchat::{self, AppFacts, Look, Speaker, State as ChatState, Watch};
use ferb_sessions::model::*;
use ferb_sessions::{clip, one_line};
use serde_json::json;

use super::{lock, Sessions};

/// A chat window the Mac lists.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatWindow {
    pub bundle_id: String,
    pub app: String,
    pub pid: i32,
    /// The window server's id.
    pub window: i64,
    pub title: String,
    /// On the current Space (Accessibility reads it without bringing anything forward).
    pub on_space: bool,
}

/// How long after a client last read a chat's timeline it still counts as watched.
pub const VIEWED_S: f64 = 30.0;

/// One app chat's own state beside its session entry.
pub(super) struct Chat {
    pub win: ChatWindow,
    pub facts: AppFacts,
    pub watch: Watch,
    pub state: Option<ChatState>,
    pub last_look: f64,
    pub last_front: f64,
    pub viewed_at: f64,
    /// A send (the owner's or FERB's) waits for its reply.
    pub waiting: bool,
    pub sent: Vec<String>,
    pub look: Option<Look>,
}

/// A chat's session id.
pub fn chat_id(w: &ChatWindow) -> String {
    Agent::AppChat.session_id(&format!("{}:{}", w.bundle_id, w.window))
}

impl Sessions {
    /// The apps whose chats FERB lists: the known ones (data) and the ones the owner added.
    fn chat_bundles(&self) -> (Vec<String>, Vec<AppFacts>) {
        let added: Vec<AppFacts> = (self.seams.chat_apps)()
            .into_iter()
            .map(|b| AppFacts {
                bundle_id: b,
                ..AppFacts::default()
            })
            .collect();
        let mut bundles: Vec<String> = appchat::known().into_iter().map(|f| f.bundle_id).collect();
        bundles.extend(added.iter().map(|f| f.bundle_id.clone()));
        bundles.sort();
        bundles.dedup();
        (bundles, added)
    }

    /// The chat windows now: new ones listed, closed ones gone, watched ones looked at when due.
    pub(super) fn chats_look(&self) {
        let (bundles, added) = self.chat_bundles();
        let windows = self.seams.mac.chat_windows(&bundles);
        let now = (self.seams.mono)();
        let stamp = (self.seams.now)();
        let ids: Vec<String> = windows.iter().map(chat_id).collect();
        let gone: Vec<String> = {
            let st = lock(&self.state);
            st.chats
                .keys()
                .filter(|k| !ids.contains(k))
                .cloned()
                .collect()
        };
        for id in gone {
            lock(&self.state).chats.remove(&id);
            self.end(&id);
        }
        for w in windows {
            let id = chat_id(&w);
            let mut st = lock(&self.state);
            if !st.chats.contains_key(&id) {
                let facts = appchat::facts_for(&w.bundle_id, &w.app, &added);
                st.chats.insert(
                    id.clone(),
                    Chat {
                        win: w.clone(),
                        facts,
                        watch: Watch::default(),
                        state: None,
                        last_look: f64::NEG_INFINITY,
                        last_front: f64::NEG_INFINITY,
                        viewed_at: f64::NEG_INFINITY,
                        waiting: false,
                        sent: Vec::new(),
                        look: None,
                    },
                );
            } else if let Some(c) = st.chats.get_mut(&id) {
                c.win = w.clone();
            }
            let title = (self.seams.redact)(&clip(&one_line(&w.title), 120));
            let e = st
                .sessions
                .entry(id.clone())
                .or_insert_with(|| super::Entry::new(&id, Agent::AppChat, &stamp));
            e.s.title = title;
            e.s.host = Some(Host {
                bundle_id: w.bundle_id.clone(),
                name: w.app.clone(),
                pid: w.pid,
                via: Some(Via::App),
                tty: String::new(),
            });
            e.s.pid = w.pid;
            drop(st);
        }
        let watched: Vec<String> = {
            let st = lock(&self.state);
            st.chats
                .iter()
                .filter(|(id, c)| {
                    let held = st
                        .sessions
                        .get(*id)
                        .is_some_and(|e| !e.s.delegate.is_empty());
                    held || c.waiting || now - c.viewed_at < VIEWED_S
                })
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in watched {
            self.chat_look(&id, false);
        }
    }

    /// A client is reading this chat: it counts as watched for VIEWED_S.
    pub(super) fn chat_viewed(&self, id: &str) {
        if let Some(c) = lock(&self.state).chats.get_mut(id) {
            c.viewed_at = (self.seams.mono)();
        }
        self.chat_look(id, false);
    }

    /// One look at a chat, when it is due (`now`: whatever the interval, e.g. a send waits on it).
    pub(super) fn chat_look(&self, id: &str, now_please: bool) {
        let now = (self.seams.mono)();
        let (win, due, front_ok) = {
            let st = lock(&self.state);
            let Some(c) = st.chats.get(id) else { return };
            let every = match c.state {
                Some(ChatState::Replying) => appchat::LOOK_REPLYING_S,
                _ => appchat::LOOK_IDLE_S,
            };
            let due = now_please || now - c.last_look >= every;
            let front_ok = now - c.last_front >= appchat::FOREGROUND_LOOK_S;
            (c.win.clone(), due, front_ok)
        };
        if !due {
            return;
        }
        // decision: HAND-669 — a window on another Space is read by bringing its app forward only while the owner is away
        let front = !win.on_space && front_ok && (self.seams.may_take_front)();
        let nodes = if win.on_space || front {
            self.seams.mac.chat_tree(&win, front)
        } else {
            None
        };
        let mut st = lock(&self.state);
        let sent = st.chats.get(id).map(|c| c.sent.clone()).unwrap_or_default();
        let Some(c) = st.chats.get_mut(id) else {
            return;
        };
        // a look that couldn't be tried (the owner is here, the window elsewhere) isn't a look
        if win.on_space || front {
            c.last_look = now;
        }
        if front {
            c.last_front = now;
        }
        let Some(nodes) = nodes else {
            drop(st);
            if let Some(e) = lock(&self.state).sessions.get_mut(id) {
                e.s.send_paths.clear();
                e.s.no_send = format!(
                    "FERB reads {}'s chat when its window is on this screen, or while you're away from the Mac.",
                    win.app
                );
            }
            return;
        };
        let look = appchat::read(&nodes, &c.facts, &sent);
        let state = c.watch.see(&look, !c.facts.stop_labels.is_empty(), now);
        c.state = Some(state);
        if state == ChatState::Finished {
            c.waiting = false;
        }
        let has_composer = look.composer.is_some();
        let blocks = look.blocks.clone();
        c.look = Some(look);
        let redact = self.seams.redact.clone();
        if let Some(e) = st.sessions.get_mut(id) {
            e.items = blocks
                .iter()
                .enumerate()
                .map(|(i, b)| Item {
                    seq: i as u64 + 1,
                    at: String::new(),
                    kind: match b.speaker {
                        Speaker::Owner => ItemKind::User,
                        Speaker::App => ItemKind::Assistant,
                    },
                    text: redact(&b.text),
                    tool: String::new(),
                })
                .collect();
            if let Some(last) = e.items.iter().rev().find(|i| i.kind == ItemKind::Assistant) {
                e.s.last_text = clip(&one_line(&last.text), 200);
            }
            e.s.status = match state {
                ChatState::Replying => Status::Working,
                ChatState::Finished => Status::Idle,
            };
            if has_composer {
                e.s.send_paths = vec![SendPath::Foreground];
                e.s.no_send.clear();
            } else {
                e.s.send_paths.clear();
                e.s.no_send = format!("FERB can't find where to type in this {} window.", win.app);
            }
        }
        drop(st);
        self.changed();
    }

    /// Before a send or its question: an app chat is looked at now (its composer found), off the async thread.
    pub async fn prime(&self, id: &str) {
        if Agent::split(id).map(|(a, _)| a) != Some(Agent::AppChat) {
            return;
        }
        let (me, i) = (self.clone(), id.to_owned());
        let _ = tokio::task::spawn_blocking(move || me.chat_viewed(&i)).await;
    }

    /// A send went into a chat: its text confirms the owner's message, and its reply is awaited.
    pub(super) fn chat_sent(&self, id: &str, text: &str) {
        let now = (self.seams.mono)();
        if let Some(c) = lock(&self.state).chats.get_mut(id) {
            c.sent.push(text.to_owned());
            if c.sent.len() > 50 {
                c.sent.remove(0);
            }
            c.waiting = true;
            c.watch.touch(now);
            c.state = Some(ChatState::Replying);
        }
        (self.seams.journal)("sessions.chat_sent", json!({"session": id}));
    }

    /// The window and the stop control a chat shows now (for an interrupt).
    pub(super) fn chat_stop(&self, id: &str) -> Option<(ChatWindow, Vec<String>)> {
        let st = lock(&self.state);
        let c = st.chats.get(id)?;
        c.look.as_ref().filter(|l| l.stop_control)?;
        Some((c.win.clone(), c.facts.stop_labels.clone()))
    }

    /// The chat's window, for the foreground path.
    pub(super) fn chat_window(&self, id: &str) -> Option<ChatWindow> {
        lock(&self.state).chats.get(id).map(|c| c.win.clone())
    }
}

/// The chats a state keeps.
pub(super) type Chats = HashMap<String, Chat>;
