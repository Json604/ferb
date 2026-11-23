//! App chats (kb HAND-668..675, HAND-697..699): the desktop chat apps' conversations as sessions in the same list, read
//! from the app's own window by code (ferb-sessions `appchat`).
//!
//! - A window is a chat only when code proves its chat pane (HAND-698): the app's marks (data), else a fingerprint
//!   learned for that app's layout. A layout code can't tell is shown once to a model (HAND-699): a redacted outline,
//!   never what is written; its pick is checked by code before it is learned, and a pick that fails is dropped. Then
//!   that layout is found by code alone. At most one ask per layout and MODEL_ASKS_PER_APP_PER_DAY per app a day
//!   (HAND-700, the owner's number; it only guards against a loop).
//! - No proven chat: no session, nothing typed. An app whose windows hold none gets one quiet line in the list
//!   ("No agent chat open in {app}. Start one there to continue here.": the clients' words, HAND-697, HAND-705).
//! - Looked at while someone watches: the owner has it open (a client read its timeline lately), FERB's task holds it,
//!   or a send waits for its reply; every LOOK_REPLYING_S while the app replies, every LOOK_IDLE_S otherwise. Any
//!   other window is checked every CHECK_EVERY_S. A window on another Space is read by bringing its app forward only
//!   while the owner is away (`may_take_front`), every FOREGROUND_LOOK_S (HAND-674).

use std::collections::{BTreeMap, HashMap};

use ferb_sessions::appchat::{
    self, AppFacts, Fingerprint, Look, Pick, Proven, Speaker, State as ChatState, Watch, WindowRead,
};
use ferb_sessions::model::*;
use ferb_sessions::{clip, one_line};
use serde::{Deserialize, Serialize};
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

/// One read of a chat window (HAND-698): the window, the pane proven in it (by the `prove` the Mac was given), and the
/// pane's own nodes with its composer marked (empty when nothing was proven).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ChatRead {
    pub window: WindowRead,
    pub proven: Option<Proven>,
    pub pane: Vec<appchat::Node>,
}

/// The model's look at a window code couldn't tell (HAND-699): the outline in, its pick out.
pub type Locate = std::sync::Arc<
    dyn Fn(String) -> ferb_core::interfaces::BoxFuture<'static, Result<Pick, String>> + Send + Sync,
>;

/// How long after a client last read a chat's timeline it still counts as watched.
pub const VIEWED_S: f64 = 30.0;
/// How often a window that isn't watched is checked for a chat (one being opened or closed there), by code alone
/// (HAND-701, the owner's number).
pub const CHECK_EVERY_S: f64 = 15.0;
/// HAND-700 (the owner's number): model asks a day for one app's layouts code can't tell; no cap across apps.
pub const MODEL_ASKS_PER_APP_PER_DAY: u32 = 10;

/// One chat window's own state beside its session entry (a window that holds no chat has one too, to be checked
/// again).
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
    /// What code knows now: a chat (its pane's fingerprint), none, or not read yet.
    pub found: Found,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Found {
    Unread,
    Chat(Fingerprint),
    NoChat,
    /// The model is looking at this layout now.
    Asking,
}

/// What FERB learned of the apps' layouts (HAND-699), kept in a file of FERB's own.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct Layouts {
    /// bundle id → the chat panes' fingerprints learned in its layouts.
    #[serde(default)]
    learned: BTreeMap<String, Vec<Fingerprint>>,
    /// "<bundle id>|<layout>" → what the model's look ended with ("chat", "none", "dropped", "failed").
    #[serde(default)]
    asked: BTreeMap<String, String>,
    /// The day the counts are for, and the counts.
    #[serde(default)]
    day: String,
    #[serde(default)]
    app_asks_today: BTreeMap<String, u32>,
    #[serde(skip)]
    loaded: bool,
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

    /// The chat windows now: new ones checked, closed ones gone, each looked at when due.
    pub(super) fn chats_look(&self) {
        let (bundles, added) = self.chat_bundles();
        let windows = self.seams.mac.chat_windows(&bundles);
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
            match st.chats.get_mut(&id) {
                Some(c) => {
                    if c.win.title != w.title {
                        // a new title: the window may hold another chat now, or none
                        c.last_look = f64::NEG_INFINITY;
                    }
                    c.win = w.clone();
                }
                None => {
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
                            found: Found::Unread,
                        },
                    );
                }
            }
        }
        let ids: Vec<String> = lock(&self.state).chats.keys().cloned().collect();
        for id in ids {
            self.chat_look(&id, false);
        }
    }

    /// HAND-697: the apps that run with windows FERB checked and found no chat in (their names, once each).
    pub fn no_chat_apps(&self) -> Vec<String> {
        let st = lock(&self.state);
        let mut by_app: BTreeMap<String, (bool, bool)> = BTreeMap::new(); // (a chat or a look under way, a "none")
        for c in st.chats.values() {
            let e = by_app.entry(c.win.app.clone()).or_default();
            match c.found {
                Found::Chat(_) | Found::Asking | Found::Unread => e.0 = true,
                Found::NoChat => e.1 = true,
            }
        }
        by_app
            .into_iter()
            .filter(|(app, (some, none))| !app.is_empty() && *none && !*some)
            .map(|(app, _)| app)
            .collect()
    }

    /// A client is reading this chat: it counts as watched for VIEWED_S.
    pub(super) fn chat_viewed(&self, id: &str) {
        if let Some(c) = lock(&self.state).chats.get_mut(id) {
            c.viewed_at = (self.seams.mono)();
        }
        self.chat_look(id, false);
    }

    /// One look at a chat window when it is due (`now_please`: whatever the interval, e.g. a send waits on it): the
    /// window read, its chat pane proven by code (or not), its messages.
    pub(super) fn chat_look(&self, id: &str, now_please: bool) {
        let now = (self.seams.mono)();
        let (win, due, front_ok, facts, learned) = {
            let st = lock(&self.state);
            let Some(c) = st.chats.get(id) else { return };
            let watched = c.waiting
                || now - c.viewed_at < VIEWED_S
                || st
                    .sessions
                    .get(id)
                    .is_some_and(|e| !e.s.delegate.is_empty());
            let every = match (&c.found, watched, c.state) {
                (Found::Asking, _, _) => f64::INFINITY,
                (Found::Chat(_), true, Some(ChatState::Replying)) => appchat::LOOK_REPLYING_S,
                (Found::Chat(_), true, _) => appchat::LOOK_IDLE_S,
                _ => CHECK_EVERY_S,
            };
            let due = (now_please && c.found != Found::Asking) || now - c.last_look >= every;
            let front_ok = now - c.last_front >= appchat::FOREGROUND_LOOK_S;
            (
                c.win.clone(),
                due,
                front_ok,
                c.facts.clone(),
                st.layouts
                    .learned
                    .get(&c.win.bundle_id)
                    .cloned()
                    .unwrap_or_default(),
            )
        };
        if !due {
            return;
        }
        if !lock(&self.state).layouts.loaded {
            self.load_layouts();
        }
        let learned = if learned.is_empty() {
            lock(&self.state)
                .layouts
                .learned
                .get(&win.bundle_id)
                .cloned()
                .unwrap_or_default()
        } else {
            learned
        };
        // decision: HAND-669 — a window on another Space is read by bringing its app forward only while the owner is away
        let front = !win.on_space && front_ok && (self.seams.may_take_front)();
        // decision: HAND-698 — the chat pane proven by the app's marks, else a layout learned for it; never a guess
        let prove = |r: &WindowRead| {
            appchat::prove(&r.cands, &facts)
                .or_else(|| learned.iter().find_map(|fp| appchat::matches(fp, &r.cands)))
        };
        let read = if win.on_space || front {
            self.seams.mac.chat_read(&win, front, &prove)
        } else {
            None
        };
        {
            let mut st = lock(&self.state);
            if let Some(c) = st.chats.get_mut(id) {
                // a look that couldn't be tried (the owner is here, the window elsewhere) isn't a look
                if win.on_space || front {
                    c.last_look = now;
                }
                if front {
                    c.last_front = now;
                }
            }
        }
        let Some(read) = read else {
            if let Some(e) = lock(&self.state).sessions.get_mut(id) {
                e.s.send_paths.clear();
                e.s.no_send = format!(
                    "FERB reads {}'s chat when its window is on this screen, or while you're away from the Mac.",
                    win.app
                );
            }
            return;
        };
        let Some(proven) = read.proven else {
            self.no_chat(id);
            self.maybe_ask(id, &win, read.window);
            return;
        };
        let sent = lock(&self.state)
            .chats
            .get(id)
            .map(|c| c.sent.clone())
            .unwrap_or_default();
        let look = appchat::read(&read.pane, &facts, &sent);
        let Some(cand) = read
            .window
            .cands
            .iter()
            .find(|c| c.index == proven.composer)
        else {
            self.no_chat(id);
            return;
        };
        if look.composer.is_none() {
            self.no_chat(id);
            return;
        }
        let fp = appchat::fingerprint(cand, proven.pane);
        self.chat_found(id, &win, fp, look, now);
    }

    /// No chat in this window (now): its session, if it had one, leaves the list; nothing can be typed there.
    fn no_chat(&self, id: &str) {
        let had = {
            let mut st = lock(&self.state);
            if let Some(c) = st.chats.get_mut(id) {
                if c.found != Found::Asking {
                    c.found = Found::NoChat;
                }
                c.look = None;
                c.waiting = false;
            }
            st.sessions.contains_key(id)
        };
        if had {
            self.end(id);
        }
        self.changed();
    }

    /// A proven chat: its session listed (or kept) with its messages.
    fn chat_found(&self, id: &str, win: &ChatWindow, fp: Fingerprint, look: Look, now: f64) {
        let stamp = (self.seams.now)();
        let redact = self.seams.redact.clone();
        let mut st = lock(&self.state);
        let Some(c) = st.chats.get_mut(id) else {
            return;
        };
        let state = c.watch.see(&look, !c.facts.stop_labels.is_empty(), now);
        c.state = Some(state);
        if state == ChatState::Finished {
            c.waiting = false;
        }
        c.found = Found::Chat(fp);
        let blocks = look.blocks.clone();
        c.look = Some(look);
        let title = redact(&clip(&one_line(&win.title), 120));
        let e = st
            .sessions
            .entry(id.to_owned())
            .or_insert_with(|| super::Entry::new(id, Agent::AppChat, &stamp));
        e.s.title = title;
        e.s.host = Some(Host {
            bundle_id: win.bundle_id.clone(),
            name: win.app.clone(),
            pid: win.pid,
            via: Some(Via::App),
            tty: String::new(),
        });
        e.s.pid = win.pid;
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
                // decision: HAND-703 — the reply's parts, each redacted like its text (rule 7)
                parts: b
                    .parts
                    .iter()
                    .map(|p| ferb_sessions::model::Part {
                        kind: p.kind,
                        text: redact(&p.text),
                        label: redact(&p.label),
                        detail: redact(&p.detail),
                    })
                    .collect(),
            })
            .collect();
        if let Some(last) = e.items.iter().rev().find(|i| i.kind == ItemKind::Assistant) {
            e.s.last_text = clip(&one_line(&last.text), 200);
        }
        e.s.status = match state {
            ChatState::Replying => Status::Working,
            ChatState::Finished => Status::Idle,
        };
        e.s.send_paths = vec![SendPath::Foreground];
        e.s.no_send.clear();
        drop(st);
        self.changed();
    }

    // ---------- HAND-699: a layout code can't tell, shown once to a model ----------

    fn load_layouts(&self) {
        let text = std::fs::read_to_string(&self.seams.layouts).unwrap_or_default();
        let mut l: Layouts = serde_json::from_str(&text).unwrap_or_default();
        l.loaded = true;
        lock(&self.state).layouts = l;
    }

    fn save_layouts(&self) {
        let text = {
            let st = lock(&self.state);
            serde_json::to_string_pretty(&st.layouts).unwrap_or_default()
        };
        if let Err(e) = super::write_atomic(&self.seams.layouts, &text) {
            (self.seams.journal)("sessions.layouts_failed", json!({"error": e}));
        }
    }

    /// Asks the model about this window's layout when code couldn't tell it, at most once per layout and within the
    /// day's asks; its pick is checked by code, then learned (or the layout is remembered as holding no chat).
    fn maybe_ask(&self, id: &str, win: &ChatWindow, read: WindowRead) {
        if read.cands.iter().all(appchat::rejected) {
            // nothing one could type into but an editor, a terminal or a find box: no chat, nothing to ask
            return;
        }
        let key = format!("{}|{}", win.bundle_id, appchat::structure_key(&read.cands));
        let day: String = (self.seams.now)().chars().take(10).collect();
        {
            let mut st = lock(&self.state);
            let l = &mut st.layouts;
            if l.day != day {
                l.day = day;
                l.app_asks_today.clear();
            }
            let app_asks = l.app_asks_today.get(&win.bundle_id).copied().unwrap_or(0);
            // decision: HAND-700 — 10 asks per app a day, none across apps
            if l.asked.contains_key(&key) || app_asks >= MODEL_ASKS_PER_APP_PER_DAY {
                return;
            }
            l.app_asks_today.insert(win.bundle_id.clone(), app_asks + 1);
            l.asked.insert(key.clone(), "asking".into());
            if let Some(c) = st.chats.get_mut(id) {
                c.found = Found::Asking;
            }
        }
        self.save_layouts();
        let outline = self.redact(&appchat::outline(&read));
        (self.seams.journal)(
            "sessions.layout_asked",
            json!({"app": win.app, "layout": key}),
        );
        let ask = (self.seams.locate)(outline);
        let (me, cid, bundle) = (self.clone(), id.to_owned(), win.bundle_id.clone());
        let finish = async move {
            let answer = ask.await;
            let (outcome, learned) = match &answer {
                Err(_) => ("failed", None),
                Ok(p) if !p.chat => ("none", None),
                // decision: HAND-699 — the model's pick is learned only once code's own check holds
                Ok(p) => match appchat::check_pick(&read, p) {
                    Some(proven) => (
                        "chat",
                        read.cands
                            .iter()
                            .find(|c| c.index == proven.composer)
                            .map(|c| appchat::fingerprint(c, proven.pane)),
                    ),
                    None => ("dropped", None),
                },
            };
            {
                let mut st = lock(&me.state);
                st.layouts.asked.insert(key.clone(), outcome.into());
                if let Some(fp) = learned {
                    let list = st.layouts.learned.entry(bundle).or_default();
                    if !list.contains(&fp) {
                        list.push(fp);
                    }
                }
                if let Some(c) = st.chats.get_mut(&cid) {
                    c.found = Found::NoChat;
                    c.last_look = f64::NEG_INFINITY;
                }
            }
            me.save_layouts();
            (me.seams.journal)(
                "sessions.layout_answered",
                json!({"layout": key, "outcome": outcome}),
            );
            // looked at again by code: a learned layout is found now, with no model call
            let me2 = me.clone();
            let _ = tokio::task::spawn_blocking(move || me2.chat_look(&cid, true)).await;
        };
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                rt.spawn(finish);
            }
            Err(_) => {
                if let Some(c) = lock(&self.state).chats.get_mut(id) {
                    c.found = Found::NoChat;
                }
            }
        }
    }

    /// The layouts asked about (tests and the journal read them): layout key → outcome.
    pub fn layouts_asked(&self) -> Vec<(String, String)> {
        lock(&self.state)
            .layouts
            .asked
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
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

    /// The chat's window and its pane's fingerprint, for the foreground path (None: no proven chat there now).
    pub(super) fn chat_window(&self, id: &str) -> Option<(ChatWindow, Fingerprint)> {
        let st = lock(&self.state);
        let c = st.chats.get(id)?;
        match &c.found {
            Found::Chat(fp) => Some((c.win.clone(), fp.clone())),
            _ => None,
        }
    }
}

/// The chats a state keeps.
pub(super) type Chats = HashMap<String, Chat>;
