//! The owner's coding-agent sessions (kb HAND-651..666): every Claude Code and Codex session running on the Mac, whatever app
//! shows it, read live, answered and sent to from FERB.app and the phones, or handed to FERB.
//!
//! - Watching is code only (HAND-664): hooks (`sessions.hook`, from FERB's `ferb agent-hook`), the transcripts' new
//!   lines, Codex's own daemon, the process table. No timer here ever starts a model call.
//! - Every send goes through [`Sessions::send`], whoever asks (the owner from FERB.app or a phone, FERB's own task):
//!   the path is chosen by code (HAND-656), the agent must be its terminal's foreground program before a typed path
//!   (HAND-658), a session that is asking something takes no typed text (its dialog would read it), and delivery is
//!   proven in the agent's own record afterwards (HAND-655).
//! - The owner's own typing into a session asks once (rule 6, GATE-585 for this feature: GATE-622): the first send
//!   shows the exact text and his Yes (`sessions.control`, the fingerprint on a phone) covers that session until he
//!   lets go, the session ends, his phone locks or FERB restarts. FERB's own task asks through the gateway's
//!   terminal question (GATE-585 as it is).
//! - The agent's permission prompts and questions become FERB's own questions (the gateway's store: FERB.app's
//!   notch, the phones' notifications), held while the Mac's own dialog stays up; the first answer wins (HAND-662,
//!   HAND-662). FERB never answers a permission prompt itself (HAND-663).
//! - Text that leaves the Mac goes through the daemon's redactor (rule 7): the timeline, cards, last lines.

mod appchats;
mod codex_link;
mod rpc;
mod terminals;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use ferb_core::interfaces::BoxFuture;
use ferb_sessions::claude::{self, HookEvent, Line};
use ferb_sessions::codes;
use ferb_sessions::host::{self, Table};
use ferb_sessions::model::*;
use ferb_sessions::{clip, codex, one_line, paths};
use serde_json::{json, Map, Value};
use tokio::sync::oneshot;

pub use appchats::{ChatRead, ChatWindow, Locate, CHECK_EVERY_S, MODEL_ASKS_PER_APP_PER_DAY};
pub use codex_link::CodexLink;
pub use rpc::register;
pub use terminals::{OPENERS, START_WAIT_S, WORKING_S};

/// The setting (config `[sessions] enabled`): off until the owner says Yes (HAND-696).
pub const SECTION: &str = "sessions";
pub const KEY: &str = "enabled";

/// The own questions' capabilities (the gateway's store; a permission's Yes takes the fingerprint on a phone).
pub const PERMISSION_ASK: &str = "agent_session.permission";
pub const QUESTION_ASK: &str = "agent_session.question";
pub const ENABLE_ASK: &str = "agent_session.enable";

/// How long a send waits to see the owner's words in the agent's own record before it says "not delivered" (HAND-655;
/// the lab measured 0.15-1.7 s on every path).
pub const DELIVERY_WAIT_S: f64 = 10.0;
/// A send answers within this many seconds of being asked, seen or not (a phone's call ends at the link's 10 s:
/// `call_deadline_s`); a delivery not seen by then is said as not seen yet (HAND-698).
pub const ANSWER_WITHIN_S: f64 = 8.0;
/// HAND-706: a session a client read within this many seconds is in front of the owner: its finished turn notifies
/// nothing (he sees it).
pub const LOOKED_AT_S: f64 = 10.0;
/// How often `claude agents --json` is read again while sessions are on (hooks give new sessions at once; this finds
/// the ones started before the hook was in place).
pub const AGENTS_EVERY_S: f64 = 30.0;
/// The longest message FERB types into a session.
pub const MAX_TEXT: usize = 20_000;

/// A call refused, with its stable code (`ferb_sessions::codes`) and the owner's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub code: i64,
    pub message: String,
}

fn no(code: i64, message: impl Into<String>) -> Refused {
    Refused {
        code,
        message: message.into(),
    }
}

/// Who asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caller {
    /// The owner in FERB.app.
    Mac,
    /// The owner on a paired phone (its device id).
    Phone(String),
    /// FERB's own task (its tree): the gateway already asked what it had to.
    Task(String),
}

impl Caller {
    pub fn key(&self) -> String {
        match self {
            Caller::Mac => "mac".into(),
            Caller::Phone(d) => format!("phone:{d}"),
            Caller::Task(t) => format!("task:{t}"),
        }
    }
    fn is_task(&self) -> bool {
        matches!(self, Caller::Task(_))
    }
}

/// What the foreground path is told (the Mac's driver does it: HAND-660).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Foreground {
    /// The host app.
    pub bundle_id: String,
    pub app: String,
    pub pid: i32,
    /// What proves the focused terminal is the session's (HAND-660, HAND-682): its title, or its program in the
    /// terminal's own words with its folder in the window's (offered only when no other session there shares them).
    pub proof: ferb_sessions::proof::Proof,
    /// The text to type ("" for a key alone); Return follows when `enter`.
    pub text: String,
    pub enter: bool,
    /// A key instead of text ("escape").
    pub key: String,
    /// An app chat (HAND-672): focus the window's own composer first; the proof is that focus, in that window.
    pub composer: bool,
    /// HAND-698: the chat pane's fingerprint: the composer is found again by it, right before typing (None: no chat).
    pub chat: Option<ferb_sessions::appchat::Fingerprint>,
    /// The window server's id of the session's window (0: the app's front window).
    pub window: i64,
    /// HAND-683: the terminal the owner chose as this session's (a `TerminalChoice` id; "" when he chose none): FERB
    /// types only there, after checking right before typing that it is still that same element.
    pub chosen: String,
}

/// Why the foreground path typed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotTyped {
    /// It couldn't (the words say why).
    Failed(String),
    /// HAND-683: it can't tell which terminal is the session's (or the chosen one changed): these are the ones it can
    /// see, for the owner to choose; `why` says so.
    Choose {
        why: String,
        terminals: Vec<TerminalChoice>,
    },
}

impl From<String> for NotTyped {
    fn from(s: String) -> Self {
        NotTyped::Failed(s)
    }
}

/// The Mac's own parts (`ferbd` gives the real ones; tests give stand-ins).
pub trait Mac: Send + Sync {
    /// The process table (pid, parent, group, terminal's group, terminal, program).
    fn processes(&self) -> Table;
    /// A process's working folder ("" when unreadable).
    fn cwd_of(&self, pid: i32) -> String;
    /// A tmux server's clients: (client pid, client tty).
    fn tmux_clients(&self, server: i32) -> Vec<(i32, String)>;
    /// The bundle id and name of the app in `bundle` (its folder).
    fn app(&self, bundle: &str) -> (String, String);
    /// `claude agents --json`'s output (None when Claude Code isn't found or it failed). `exe`: a running Claude
    /// Code's own program, when one is known.
    fn claude_agents(&self, exe: Option<&str>) -> Option<String>;
    /// Terminal: `do script` in the tab whose tty is `tty` (types the text and Return, in the background).
    fn terminal_type(&self, tty: &str, text: &str) -> Result<(), String>;
    /// Terminal: a new window running `command` (in the background).
    fn terminal_open(&self, command: &str) -> Result<(), String>;
    /// tmux: the text (Return after it when `enter`) or a key ("Escape") into the pane on `tty`, on the server `server`.
    fn tmux_send(
        &self,
        server: i32,
        tty: &str,
        text: &str,
        key: &str,
        enter: bool,
    ) -> Result<(), String>;
    /// The generic foreground path (HAND-660).
    fn foreground(&self, f: Foreground) -> BoxFuture<'static, Result<(), NotTyped>>;
    /// HAND-668: the windows of these apps (bundle ids) that could hold a chat.
    fn chat_windows(&self, _bundles: &[String]) -> Vec<ChatWindow> {
        Vec::new()
    }
    /// HAND-669, HAND-698: one read of a chat window (`front`: the app may be brought forward to read it, then put
    /// back): its nodes and editable elements, the pane `prove` finds there, and that pane's own nodes with its
    /// composer marked. None: unreadable.
    fn chat_read(
        &self,
        _w: &ChatWindow,
        _front: bool,
        _prove: &(dyn Fn(&ferb_sessions::appchat::WindowRead) -> Option<ferb_sessions::appchat::Proven>
              + Sync),
    ) -> Option<ChatRead> {
        None
    }
    /// HAND-672: presses the app's own stop control (a button with one of these labels) in the window.
    fn chat_press(&self, _w: &ChatWindow, _labels: &[String]) -> Result<(), String> {
        Err("this app has no stop control FERB can press".into())
    }
    /// HAND-654, PHN-797: the running app's own process for `bundle_id`, the one that owns its windows, given the
    /// process the tree found (`found`). An app's helper that left the app's tree (a terminal's session server under
    /// launchd) is in the app's bundle but owns no window. The found one when it is the app's, or can't be told.
    fn app_pid(&self, _bundle_id: &str, found: i32) -> i32 {
        found
    }
    /// HAND-692: a process's program as it was started (argv[0]'s file name; `exe`, its executable, when unknown).
    fn program_of(&self, _pid: i32, exe: &str) -> String {
        ferb_sessions::proof::program_name(exe)
    }
    /// HAND-692: when a terminal last had input or output (its device's times): seconds since 1970 and the same as
    /// ISO 8601. None when unknown.
    fn tty_activity(&self, _tty: &str) -> Option<(f64, String)> {
        None
    }
    /// HAND-692: every app's normal windows with a title: (the app's pid, the window's id, its title).
    fn window_titles(&self) -> Vec<(i32, i64, String)> {
        Vec::new()
    }
    /// HAND-693: a terminal's text as its app shows it (Err: why FERB can't read it, in the owner's words).
    fn terminal_text(&self, _t: &TermPlace) -> Result<String, String> {
        Err("FERB can't read this terminal's text here.".into())
    }
    /// HAND-695: a new window of the terminal app `bundle_id` in `folder`, in the background, typing nothing.
    fn terminal_open_at(&self, _bundle_id: &str, _folder: &str) -> Result<(), String> {
        Err("FERB can't open a terminal here.".into())
    }
}

/// PHN-797: what finds a session's window, the same for sending and for Watch (HAND-682, HAND-683): its host, what
/// proves a terminal or window is the session's, the terminal the owner chose and an app chat's own window.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub host: Host,
    pub proof: ferb_sessions::proof::Proof,
    /// HAND-683: the owner's chosen terminal (a `TerminalChoice` id; "" when none).
    pub chosen: String,
    /// HAND-672, HAND-698: an app chat's window and its proven pane's fingerprint (None: no proven chat there now).
    pub chat: Option<(ChatWindow, ferb_sessions::appchat::Fingerprint)>,
    /// The window already known to be the session's (0: none): an app chat's own (HAND-672), or a terminal's by its
    /// app's words for it (HAND-692).
    pub window: i64,
}

/// HAND-693: where a terminal's text is read: its app, its device, how it is shown (tmux's pane, the app's own
/// terminal) and its window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TermPlace {
    pub bundle_id: String,
    pub app: String,
    /// The app's main process.
    pub pid: i32,
    /// The terminal's device (the pane's own for tmux).
    pub tty: String,
    /// Through tmux: its server's pid (0: not through tmux).
    pub tmux_server: i32,
    /// The window that is its (0: FERB can't tell which).
    pub window: i64,
}

/// FERB's own question to the owner (the gateway's store): (capability, summary, detail, options). Answers the
/// chosen option, None when it was set aside or not answered.
pub type AskFn = Arc<dyn Fn(Ask) -> (String, BoxFuture<'static, Option<String>>) + Send + Sync>;

/// One Mac notification: (ident, text); a later one with the same ident replaces it (HAND-706: never a chat line).
pub type Tell = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// A bus topic or journal kind with its data.
pub type Note = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// HAND-702: what became of the owner's hand-over message (the chat turn's own events).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hand {
    /// FERB started a task for it (its id).
    Started(String),
    /// FERB asked him something in its chat before starting (its words).
    Asked(String),
    /// FERB answered in its chat and started nothing (its words).
    Said(String),
    /// FERB couldn't start it (its words in the chat).
    Failed(String),
}

/// HAND-702: where the hand-over's news goes.
pub type HandNews = Arc<dyn Fn(Hand) + Send + Sync>;

/// The owner's hand-over as a chat message: (text, client id, where the turn's news goes).
pub type HandOver =
    Arc<dyn Fn(String, Option<String>, HandNews) -> Result<(), String> + Send + Sync>;

/// One own question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub capability: String,
    pub summary: String,
    pub detail: String,
    pub options: Vec<String>,
}

/// What the area takes from the composition.
pub struct Seams {
    pub mac: Arc<dyn Mac>,
    /// The owner's home folder (where `.claude` and `.codex` are).
    pub home: PathBuf,
    /// FERB's `ferb` program, which Claude Code's hooks run ("" when this FERB has none).
    pub hook_program: String,
    /// The daemon's own pid: a session FERB itself runs (a worker's Claude Code) is never the owner's.
    pub own_pid: i32,
    /// The setting, read now (None: never set).
    pub enabled: Arc<dyn Fn() -> Option<bool> + Send + Sync>,
    pub set_enabled: Arc<dyn Fn(bool) -> Result<(), String> + Send + Sync>,
    /// Setup is done (FERB may ask its own questions).
    pub setup_done: Arc<dyn Fn() -> bool + Send + Sync>,
    pub redact: Arc<dyn Fn(&str) -> String + Send + Sync>,
    pub publish: Note,
    pub journal: Note,
    /// HAND-706: one notification on the Mac (a banner FERB.app shows while the session isn't in front of him), one per
    /// session at a time (its ident replaces the last); never a line in the owner's chat.
    pub tell: Tell,
    pub ask: AskFn,
    /// Sets one of FERB's questions aside (it was answered elsewhere).
    pub close_ask: Arc<dyn Fn(&str) + Send + Sync>,
    /// The owner's words as a chat message (his hand-over): (text, client id, where the turn's news goes: HAND-702).
    pub hand_over: HandOver,
    pub now: Arc<dyn Fn() -> String + Send + Sync>,
    pub mono: Arc<dyn Fn() -> f64 + Send + Sync>,
    pub new_id: Arc<dyn Fn() -> String + Send + Sync>,
    pub version: String,
    /// HAND-668: the chat apps the owner added (bundle ids), beside the known ones.
    pub chat_apps: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
    /// HAND-669: FERB may bring an app forward to read its chat now (the owner is away from the Mac).
    pub may_take_front: Arc<dyn Fn() -> bool + Send + Sync>,
    /// HAND-699: the model's one look at a chat app's layout code couldn't tell.
    pub locate: Locate,
    /// HAND-699: where the learned layouts are kept (a file of FERB's own).
    pub layouts: PathBuf,
}

/// One held card.
struct Held {
    card: Card,
    input: Value,
    suggestions: Value,
    /// Claude Code: the waiting hook's reply.
    reply: Option<oneshot::Sender<String>>,
    /// Codex: the daemon's request id.
    codex: Option<Value>,
}

impl Entry {
    /// A new entry for a session first seen now.
    fn new(id: &str, agent: Agent, now: &str) -> Entry {
        let own = Agent::split(id)
            .map(|(_, o)| o.to_owned())
            .unwrap_or_default();
        Entry {
            s: Session {
                id: id.to_owned(),
                agent,
                agent_session: own,
                source: Source::Attached,
                pid: 0,
                cwd: String::new(),
                title: String::new(),
                status: Status::Idle,
                waiting: Waiting::None,
                host: None,
                send_paths: Vec::new(),
                no_send: String::new(),
                last_at: now.to_owned(),
                last_text: String::new(),
                control: String::new(),
                delegate: String::new(),
                goal: String::new(),
                terminals: Vec::new(),
                program: String::new(),
                window: String::new(),
                no_text: String::new(),
                ferb_note: String::new(),
            },
            transcript: None,
            offset: 0,
            partial: Vec::new(),
            items: Vec::new(),
            owners_title: false,
            cards: Vec::new(),
            via_ferb: false,
            turn: String::new(),
            host_key: (0, String::new()),
            exe: String::new(),
            term: None,
            looked_at: f64::NEG_INFINITY,
        }
    }
}

/// One session's state.
struct Entry {
    s: Session,
    transcript: Option<PathBuf>,
    offset: u64,
    partial: Vec<u8>,
    items: Vec<Item>,
    owners_title: bool,
    cards: Vec<Held>,
    /// The last turn was sent through FERB (so its end tells the owner).
    via_ferb: bool,
    /// Codex: the turn under way.
    turn: String,
    /// Host resolution's input, to redo it only when it changes.
    host_key: (i32, String),
    exe: String,
    /// HAND-692, HAND-693: a terminal's own state (None for an agent's session).
    term: Option<Term>,
    /// HAND-706: when a client last read its timeline (monotonic; never: -inf).
    looked_at: f64,
}

/// What FERB keeps of one terminal session (HAND-692, HAND-693).
#[derive(Default)]
struct Term {
    /// The process that names it (its terminal lives as long as it does).
    leader: i32,
    tty: String,
    /// The window that is its, by the app's own words (0: FERB can't tell).
    window: i64,
    /// When the terminal last had input or output (the device's times), and when FERB saw that change (monotonic).
    activity: f64,
    changed_at: f64,
    /// What has been shown of its text.
    screen: ferb_sessions::terminals::Screen,
    /// When its text was last read, and when a client last looked at it (monotonic).
    read_at: f64,
}

#[derive(Default)]
struct State {
    sessions: BTreeMap<String, Entry>,
    /// session → who controls it (GATE-622).
    control: HashMap<String, String>,
    last_agents: f64,
    last_published: String,
    hooks_installed: Option<bool>,
    asked_enable: bool,
    chats: appchats::Chats,
    layouts: appchats::Layouts,
    /// HAND-683: session → (its agent's process, the terminal the owner chose for it): until it ends (a new process,
    /// even under the same id, isn't it) or he stops controlling it.
    chosen: HashMap<String, (i32, String)>,
}

/// The area. Cheap to clone.
#[derive(Clone)]
pub struct Sessions {
    seams: Arc<Seams>,
    state: Arc<Mutex<State>>,
    codex: CodexLink,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a send ended with.
#[derive(Clone, Debug, PartialEq)]
pub struct Sent {
    pub path: SendPath,
    pub delivered: bool,
    pub seconds: f64,
}

/// What the owner is asked before his first typed send to a session (GATE-622).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeedsYes {
    pub question: String,
    pub detail: String,
    pub covers: String,
}

impl Sessions {
    pub fn new(seams: Seams) -> Sessions {
        let socket = seams.home.join(codex::SOCKET);
        let version = seams.version.clone();
        Sessions {
            seams: Arc::new(seams),
            state: Arc::default(),
            codex: CodexLink::new(socket, version),
        }
    }

    pub fn enabled(&self) -> bool {
        (self.seams.enabled)() == Some(true)
    }

    fn now(&self) -> String {
        (self.seams.now)()
    }

    fn redact(&self, t: &str) -> String {
        (self.seams.redact)(t)
    }

    // ---------- the list ----------

    /// The sessions as clients see them (ended ones are gone).
    pub fn list(&self) -> Vec<Session> {
        let st = lock(&self.state);
        let mut out: Vec<Session> = st
            .sessions
            .values()
            .filter(|e| e.s.status != Status::Ended)
            .map(|e| {
                let mut s = e.s.clone();
                s.control = st.control.get(&s.id).cloned().unwrap_or_default();
                s
            })
            .collect();
        out.sort_by(|a, b| b.last_at.cmp(&a.last_at));
        out
    }

    pub fn session(&self, id: &str) -> Option<Session> {
        self.list().into_iter().find(|s| s.id == id)
    }

    /// The timeline after `after` (newest `limit` when `after` is 0), and the open cards.
    pub fn timeline(
        &self,
        id: &str,
        after: u64,
        limit: usize,
    ) -> Result<(Vec<Item>, bool, Vec<Card>), Refused> {
        match Agent::split(id).map(|(a, _)| a) {
            Some(Agent::Codex) => self.codex_refresh(id),
            Some(Agent::AppChat) => self.chat_viewed(id),
            // decision: HAND-693 — a terminal's text is read only while someone looks at it
            Some(Agent::Terminal) => {
                self.term_read(id, false);
            }
            _ => {}
        }
        let mono = (self.seams.mono)();
        let mut st = lock(&self.state);
        let e = st
            .sessions
            .get_mut(id)
            .filter(|e| e.s.status != Status::Ended)
            .ok_or_else(gone)?;
        e.looked_at = mono;
        let e = &*e;
        let limit = limit.clamp(1, 100);
        let newer: Vec<&Item> = e.items.iter().filter(|i| i.seq > after).collect();
        let (items, more): (Vec<Item>, bool) = if after == 0 {
            let start = newer.len().saturating_sub(limit);
            (
                newer[start..].iter().map(|i| (*i).clone()).collect(),
                start > 0,
            )
        } else {
            let take = newer.len().min(limit);
            (
                newer[..take].iter().map(|i| (*i).clone()).collect(),
                newer.len() > take,
            )
        };
        let cards = e.cards.iter().map(|h| h.card.clone()).collect();
        Ok((items, more, cards))
    }

    /// Items before `before` (paging back).
    pub fn earlier(
        &self,
        id: &str,
        before: u64,
        limit: usize,
    ) -> Result<(Vec<Item>, bool), Refused> {
        let st = lock(&self.state);
        let e = st.sessions.get(id).ok_or_else(gone)?;
        let older: Vec<&Item> = e.items.iter().filter(|i| i.seq < before).collect();
        let limit = limit.clamp(1, 100);
        let start = older.len().saturating_sub(limit);
        Ok((
            older[start..].iter().map(|i| (*i).clone()).collect(),
            start > 0,
        ))
    }

    // ---------- watching (code only) ----------

    /// One look: the setting and the hooks, the process table, Claude Code's list, the transcripts' new lines, Codex.
    /// Called every couple of seconds by the daemon's loop and after each hook; never calls a model.
    pub fn look(&self) {
        let on = self.enabled();
        self.reconcile_hooks(on);
        if !on {
            let had = !lock(&self.state).sessions.is_empty();
            if had {
                let mut st = lock(&self.state);
                for e in st.sessions.values_mut() {
                    for h in e.cards.drain(..) {
                        if let Some(tx) = h.reply {
                            let _ = tx.send(String::new());
                        }
                        if !h.card.approval_id.is_empty() {
                            (self.seams.close_ask)(&h.card.approval_id);
                        }
                    }
                }
                st.sessions.clear();
                st.control.clear();
            }
            self.codex.stop();
            self.changed();
            return;
        }
        let table = self.seams.mac.processes();
        let mono = (self.seams.mono)();
        let read_agents = {
            let st = lock(&self.state);
            st.last_agents == 0.0 || mono - st.last_agents >= AGENTS_EVERY_S
        };
        if read_agents {
            let exe = {
                let st = lock(&self.state);
                st.sessions
                    .values()
                    .find(|e| !e.exe.is_empty())
                    .map(|e| e.exe.clone())
            };
            if let Some(out) = self.seams.mac.claude_agents(exe.as_deref()) {
                for l in claude::parse_agents(&out) {
                    if l.pid > 0 && host::ancestors(&table, l.pid).contains(&self.seams.own_pid) {
                        continue;
                    }
                    self.note_claude(&l.session, l.pid, &l.cwd, None, &l.name, Some(l.busy));
                }
            }
            lock(&self.state).last_agents = mono;
        }
        self.codex.ensure(self);
        // liveness, hosts, transcripts
        let ids: Vec<String> = lock(&self.state).sessions.keys().cloned().collect();
        for id in ids {
            self.refresh(&id, &table);
        }
        // decision: HAND-692 — every terminal, whatever runs in it; an agent's session claims its own
        self.note_terminals(&table);
        // HAND-668: the chat apps' windows
        self.chats_look();
        self.changed();
    }

    /// Hooks in place exactly while sessions are on (HAND-696): Claude Code's settings, FERB's entries only.
    fn reconcile_hooks(&self, on: bool) {
        let program = self.seams.hook_program.clone();
        if program.is_empty() {
            return;
        }
        let file = self.seams.home.join(".claude").join("settings.json");
        let want = on && self.seams.home.join(".claude").is_dir();
        if lock(&self.state).hooks_installed == Some(want) {
            return;
        }
        let text = std::fs::read_to_string(&file).ok();
        let result = if want {
            if ferb_sessions::hooks::installed(text.as_deref(), &program) {
                Ok(None)
            } else {
                ferb_sessions::hooks::install(text.as_deref(), &program)
            }
        } else {
            ferb_sessions::hooks::remove(text.as_deref())
        };
        match result {
            Ok(Some(new)) => match write_atomic(&file, &new) {
                Ok(()) => {
                    (self.seams.journal)(
                        "sessions.hooks",
                        json!({"installed": want, "file": file.to_string_lossy()}),
                    );
                    lock(&self.state).hooks_installed = Some(want);
                }
                Err(e) => (self.seams.journal)("sessions.hooks_failed", json!({"error": e})),
            },
            Ok(None) => lock(&self.state).hooks_installed = Some(want),
            Err(e) => {
                (self.seams.journal)("sessions.hooks_failed", json!({"error": e}));
                lock(&self.state).hooks_installed = Some(want);
            }
        }
    }

    /// Removes FERB's hooks whatever the setting says (FERB.app's uninstall).
    pub fn remove_hooks(&self) -> Result<bool, String> {
        let file = self.seams.home.join(".claude").join("settings.json");
        let text = std::fs::read_to_string(&file).ok();
        match ferb_sessions::hooks::remove(text.as_deref())? {
            Some(new) => {
                write_atomic(&file, &new)?;
                lock(&self.state).hooks_installed = Some(false);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// A Claude Code session seen (by its list, a hook or its process).
    fn note_claude(
        &self,
        sid: &str,
        pid: i32,
        cwd: &str,
        transcript: Option<&str>,
        name: &str,
        busy: Option<bool>,
    ) {
        let id = Agent::ClaudeCode.session_id(sid);
        let now = self.now();
        let mut st = lock(&self.state);
        let e = st
            .sessions
            .entry(id.clone())
            .or_insert_with(|| Entry::new(&id, Agent::ClaudeCode, &now));
        if e.s.status == Status::Ended {
            return;
        }
        if pid > 0 {
            e.s.pid = pid;
        }
        if !cwd.is_empty() {
            e.s.cwd = cwd.to_owned();
        }
        if let Some(t) = transcript.filter(|t| !t.is_empty()) {
            e.transcript = Some(PathBuf::from(t));
        }
        if !name.is_empty() && !e.owners_title && e.s.title.is_empty() {
            e.s.title = self.redact(&clip(&one_line(name), 120));
        }
        if let Some(b) = busy {
            if e.cards.is_empty() {
                e.s.status = if b { Status::Working } else { Status::Idle };
            }
        }
    }

    /// One session's liveness, host, paths and new transcript lines.
    fn refresh(&self, id: &str, table: &Table) {
        let (agent, pid, transcript_known, sid) = {
            let st = lock(&self.state);
            let Some(e) = st.sessions.get(id) else { return };
            if e.s.agent == Agent::AppChat || e.term.is_some() {
                return;
            }
            (
                e.s.agent,
                e.s.pid,
                e.transcript.is_some(),
                e.s.agent_session.clone(),
            )
        };
        if agent == Agent::ClaudeCode {
            if pid > 0 && !table.contains_key(&pid) {
                self.end(id);
                return;
            }
            if !transcript_known {
                if let Some(p) = find_transcript(&self.seams.home, &sid) {
                    if let Some(e) = lock(&self.state).sessions.get_mut(id) {
                        e.transcript = Some(p);
                    }
                }
            }
            self.tail(id);
        }
        self.resolve_host(id, table);
    }

    fn resolve_host(&self, id: &str, table: &Table) {
        let (agent, pid) = {
            let st = lock(&self.state);
            let Some(e) = st.sessions.get(id) else { return };
            (e.s.agent, e.s.pid)
        };
        // the host is redone when the agent's terminal or parent changed (a parent that ended leaves it to launchd)
        let key = (
            pid,
            table
                .get(&pid)
                .map(|p| format!("{}|{}", p.tty, p.ppid))
                .unwrap_or_default(),
        );
        let unchanged = lock(&self.state)
            .sessions
            .get(id)
            .is_some_and(|e| e.host_key == key && e.s.host.is_some());
        let host = if unchanged {
            lock(&self.state)
                .sessions
                .get(id)
                .and_then(|e| e.s.host.clone())
        } else if pid > 0 {
            let mac = self.seams.mac.clone();
            host::resolve(table, pid, &|server| mac.tmux_clients(server)).map(|f| {
                let (bundle_id, name) = if f.bundle.is_empty() {
                    (String::new(), String::new())
                } else {
                    self.seams.mac.app(&f.bundle)
                };
                // the app's own process, which owns its windows (HAND-654, PHN-797)
                let pid = if bundle_id.is_empty() {
                    f.pid
                } else {
                    self.seams.mac.app_pid(&bundle_id, f.pid)
                };
                Host {
                    bundle_id,
                    name,
                    pid,
                    via: Some(f.via),
                    tty: f.tty,
                }
            })
        } else {
            None
        };
        let exe = table.get(&pid).map(|p| p.exe.clone()).unwrap_or_default();
        let api = agent == Agent::Codex && self.codex.connected();
        let (paths_, why) = paths::send_paths(agent, host.as_ref(), api);
        let mut st = lock(&self.state);
        if let Some(e) = st.sessions.get_mut(id) {
            e.host_key = key;
            e.s.host = host;
            e.s.send_paths = paths_;
            e.s.no_send = why;
            if !exe.is_empty() {
                e.exe = exe;
            }
        }
    }

    /// Reads a Claude Code transcript's new lines (an allowlist, HAND-661).
    fn tail(&self, id: &str) {
        let (path, offset) = {
            let st = lock(&self.state);
            let Some(e) = st.sessions.get(id) else { return };
            match &e.transcript {
                Some(p) => (p.clone(), e.offset),
                None => return,
            }
        };
        let Ok(mut f) = std::fs::File::open(&path) else {
            return;
        };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len <= offset {
            return;
        }
        use std::io::{Read, Seek, SeekFrom};
        if f.seek(SeekFrom::Start(offset)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        if f.take(len - offset).read_to_end(&mut buf).is_err() {
            return;
        }
        let mut st = lock(&self.state);
        let Some(e) = st.sessions.get_mut(id) else {
            return;
        };
        e.offset = len;
        e.partial.extend_from_slice(&buf);
        let Some(last_nl) = e.partial.iter().rposition(|b| *b == b'\n') else {
            return;
        };
        let whole: Vec<u8> = e.partial.drain(..=last_nl).collect();
        let text = String::from_utf8_lossy(&whole);
        for line in text.lines() {
            match claude::parse_line(line) {
                Line::Items(ds) => {
                    for d in ds {
                        let seq = e.items.last().map_or(1, |i| i.seq + 1);
                        let item = Item {
                            seq,
                            at: d.at.clone(),
                            kind: d.kind,
                            text: (self.seams.redact)(&d.text),
                            tool: d.tool,
                            parts: Vec::new(),
                        };
                        if item.kind == ItemKind::Assistant {
                            e.s.last_text = clip(&one_line(&item.text), 200);
                        }
                        if !item.at.is_empty() {
                            e.s.last_at = item.at.clone();
                        }
                        e.items.push(item);
                    }
                }
                Line::Title { text, owners } => {
                    if owners || !e.owners_title {
                        e.s.title = (self.seams.redact)(&text);
                        e.owners_title |= owners;
                    }
                }
                Line::Skip => {}
            }
        }
    }

    /// The session ended: its cards are released, its control ends, it leaves the list.
    fn end(&self, id: &str) {
        let mut st = lock(&self.state);
        st.control.remove(id);
        if let Some(e) = st.sessions.get_mut(id) {
            e.s.status = Status::Ended;
            for h in e.cards.drain(..) {
                if let Some(tx) = h.reply {
                    let _ = tx.send(String::new());
                }
                if !h.card.approval_id.is_empty() {
                    (self.seams.close_ask)(&h.card.approval_id);
                }
            }
        }
        st.sessions.remove(id);
    }

    /// Publishes `sessions.changed` when the list changed since the last time.
    fn changed(&self) {
        let list = self.list();
        let now = serde_json::to_string(&list).unwrap_or_default();
        let mut st = lock(&self.state);
        if st.last_published == now {
            return;
        }
        st.last_published = now;
        drop(st);
        (self.seams.publish)(
            "sessions.changed",
            json!({"count": list.len(), "waiting": list.iter().filter(|s| s.status == Status::WaitingForYou).count()}),
        );
    }

    // ---------- hooks ----------

    /// One hook event from Claude Code (`sessions.hook`). Answers the hook's stdout: "" except for a held permission
    /// request answered by the owner (HAND-662). Never waits on anything but the owner's answer to a held card; when
    /// sessions are off it answers at once.
    pub async fn hook(&self, payload: &Value, ancestors: &[i32]) -> String {
        if !self.enabled() {
            return String::new();
        }
        let Some(h) = claude::parse_hook(payload) else {
            return String::new();
        };
        // decision: HAND-662 — a Claude Code FERB itself runs (a worker's) is not one of the owner's sessions
        if ancestors.contains(&self.seams.own_pid) {
            return String::new();
        }
        let table = self.seams.mac.processes();
        let pid = agent_of(&table, ancestors);
        self.note_claude(&h.session, pid, &h.cwd, Some(&h.transcript), "", None);
        let id = Agent::ClaudeCode.session_id(&h.session);
        let now = self.now();
        let mut tell: Option<(String, String)> = None;
        let mut wait: Option<oneshot::Receiver<String>> = None;
        let mut held_card = String::new();
        let mut withdraw: Vec<Held> = Vec::new();
        {
            let mut st = lock(&self.state);
            let Some(e) = st.sessions.get_mut(&id) else {
                return String::new();
            };
            e.s.last_at = now.clone();
            match &h.event {
                HookEvent::Start => e.s.status = Status::Idle,
                HookEvent::Prompt { .. } => {
                    e.s.status = Status::Working;
                    e.s.waiting = Waiting::None;
                    withdraw.append(&mut e.cards);
                }
                HookEvent::Permission {
                    tool,
                    input,
                    suggestions,
                } => {
                    let card_id = (self.seams.new_id)();
                    let mut card = claude::card(&card_id, &id, tool, input, suggestions, &now);
                    card.detail = (self.seams.redact)(&card.detail);
                    card.question = (self.seams.redact)(&card.question);
                    let (tx, rx) = oneshot::channel();
                    e.s.status = Status::WaitingForYou;
                    e.s.waiting = match card.kind {
                        CardKind::Permission => Waiting::Permission,
                        CardKind::Question => Waiting::Question,
                    };
                    e.cards.push(Held {
                        card,
                        input: input.clone(),
                        suggestions: suggestions.clone(),
                        reply: Some(tx),
                        codex: None,
                    });
                    wait = Some(rx);
                    held_card = card_id.clone();
                    tell = Some((id.clone(), card_id));
                }
                HookEvent::ToolDone { tool, input } => {
                    let (gone, keep): (Vec<Held>, Vec<Held>) =
                        std::mem::take(&mut e.cards).into_iter().partition(|c| {
                            claude::same_call(&c.card.tool_or_question(), &c.input, tool, input)
                        });
                    e.cards = keep;
                    withdraw = gone;
                    if e.cards.is_empty() {
                        e.s.status = Status::Working;
                        e.s.waiting = Waiting::None;
                    }
                }
                HookEvent::Notice { kind, .. } => {
                    if kind == "idle_prompt" && e.cards.is_empty() {
                        e.s.status = Status::Idle;
                    }
                }
                HookEvent::Stop { last } => {
                    withdraw.append(&mut e.cards);
                    e.s.status = Status::Idle;
                    e.s.waiting = Waiting::None;
                    if !last.trim().is_empty() {
                        e.s.last_text = clip(&one_line(&(self.seams.redact)(last)), 200);
                    }
                    if std::mem::take(&mut e.via_ferb) {
                        tell = Some((id.clone(), String::new()));
                    }
                }
                HookEvent::End => {
                    e.s.status = Status::Ended;
                }
                HookEvent::Other(_) => {}
            }
        }
        for h in withdraw {
            self.release(h);
        }
        if matches!(h.event, HookEvent::End) {
            self.end(&id);
        }
        self.tail(&id);
        self.changed();
        if let Some((sid, card_id)) = tell {
            if card_id.is_empty() {
                self.tell_finished(&sid);
            } else {
                self.ask_card(&sid, &card_id);
            }
        }
        match wait {
            // the owner's answer, or "" when the card is withdrawn; a dropped call (the hook's process ended: Claude
            // Code's own timeout, or FERB restarting) drops the receiver and the card goes with it (`Guard`)
            Some(rx) => {
                let guard = CardGuard {
                    sessions: self.clone(),
                    session: id.clone(),
                    card: held_card,
                };
                let out = rx.await.unwrap_or_default();
                drop(guard);
                out
            }
            None => String::new(),
        }
    }

    /// A withdrawn card: its hook freed with no decision, its question set aside.
    fn release(&self, h: Held) {
        if let Some(tx) = h.reply {
            let _ = tx.send(String::new());
        }
        if !h.card.approval_id.is_empty() {
            (self.seams.close_ask)(&h.card.approval_id);
        }
        (self.seams.publish)(
            "sessions.card_gone",
            json!({"session": h.card.session, "card": h.card.id}),
        );
    }

    /// Asks the owner a card as FERB's own question; his answer answers the card.
    fn ask_card(&self, session: &str, card_id: &str) {
        let (card, folder, agent) = {
            let st = lock(&self.state);
            let Some(e) = st.sessions.get(session) else {
                return;
            };
            let Some(h) = e.cards.iter().find(|h| h.card.id == card_id) else {
                return;
            };
            (h.card.clone(), folder_name(&e.s.cwd), e.s.agent)
        };
        let (capability, summary, detail) = match card.kind {
            CardKind::Permission => (
                PERMISSION_ASK,
                format!(
                    "{} in {folder} wants to use {}",
                    agent.name(),
                    if card.tool.is_empty() {
                        "a tool"
                    } else {
                        &card.tool
                    }
                ),
                card.detail.clone(),
            ),
            CardKind::Question => (
                QUESTION_ASK,
                format!("{} in {folder} asks: {}", agent.name(), card.question),
                card.detail.clone(),
            ),
        };
        if card.choices.is_empty() {
            // a question only the Mac's own dialog can answer: told, not asked
            (self.seams.publish)(
                "sessions.card",
                json!({"session": session, "card": card.id}),
            );
            return;
        }
        let options: Vec<String> = card.choices.iter().map(|c| c.label.clone()).collect();
        let (approval_id, answer) = (self.seams.ask)(Ask {
            capability: capability.into(),
            summary,
            detail,
            options,
        });
        {
            let mut st = lock(&self.state);
            if let Some(h) = st
                .sessions
                .get_mut(session)
                .and_then(|e| e.cards.iter_mut().find(|h| h.card.id == card_id))
            {
                h.card.approval_id = approval_id;
            }
        }
        (self.seams.publish)(
            "sessions.card",
            json!({"session": session, "card": card.id}),
        );
        let me = self.clone();
        let (session, card_id) = (session.to_owned(), card_id.to_owned());
        tokio::spawn(async move {
            if let Some(label) = answer.await {
                let choice = card
                    .choices
                    .iter()
                    .find(|c| c.label == label)
                    .map(|c| c.id.clone());
                if let Some(choice) = choice {
                    let _ = me.resolve(&session, &card_id, &choice, None);
                }
            }
        });
    }

    /// Answers a card (the owner's choice; `by` is checked by the caller). The own question is set aside when it
    /// was answered here.
    pub fn resolve(
        &self,
        session: &str,
        card_id: &str,
        choice: &str,
        by: Option<&Caller>,
    ) -> Result<(), Refused> {
        let held = {
            let mut st = lock(&self.state);
            let e = st.sessions.get_mut(session).ok_or_else(gone)?;
            let at = e
                .cards
                .iter()
                .position(|h| h.card.id == card_id)
                .ok_or_else(|| no(codes::CARD_GONE, "That question was already answered."))?;
            if by.is_some_and(Caller::is_task) && e.cards[at].card.kind == CardKind::Permission {
                // decision: HAND-663 — FERB never answers the agent's permission prompts
                return Err(no(
                    codes::OWNERS_ONLY,
                    "Only the owner answers the agent's permission prompts.",
                ));
            }
            if !e.cards[at].card.choices.iter().any(|c| c.id == choice) {
                return Err(no(
                    codes::CARD_GONE,
                    "That isn't one of the question's answers.",
                ));
            }
            let h = e.cards.remove(at);
            if e.cards.is_empty() {
                e.s.status = Status::Working;
                e.s.waiting = Waiting::None;
            }
            h
        };
        if by.is_some() && !held.card.approval_id.is_empty() {
            (self.seams.close_ask)(&held.card.approval_id);
        }
        (self.seams.journal)(
            "sessions.answered",
            json!({"session": session, "card": card_id, "choice": choice, "by": by.map(Caller::key).unwrap_or_else(|| "question".into())}),
        );
        if let Some(tx) = held.reply {
            let _ = tx.send(claude::reply(
                choice,
                &held.card,
                &held.input,
                &held.suggestions,
            ));
        }
        if let Some(rid) = held.codex {
            if let Some(result) = codex::approval_answer(choice) {
                self.codex.respond(&rid, result);
            }
        }
        (self.seams.publish)(
            "sessions.card_gone",
            json!({"session": session, "card": card_id}),
        );
        self.changed();
        Ok(())
    }

    /// A turn sent through FERB ended: said on the session (its status) and, only while the owner isn't looking at it,
    /// as one Mac notification for that session (HAND-706: the owner, "the chat is spammed": never a chat line).
    fn tell_finished(&self, session: &str) {
        let mono = (self.seams.mono)();
        let (agent, folder, last, looked) = {
            let st = lock(&self.state);
            let Some(e) = st.sessions.get(session) else {
                return;
            };
            let where_ = match (e.s.agent, e.s.host.as_ref()) {
                // an app chat is named by its app alone
                (Agent::AppChat, Some(h)) => h.name.clone(),
                _ => format!("{} in {}", e.s.agent.name(), folder_name(&e.s.cwd)),
            };
            (
                e.s.agent,
                where_,
                e.s.last_text.clone(),
                mono - e.looked_at < LOOKED_AT_S,
            )
        };
        (self.seams.publish)(
            "sessions.finished",
            json!({"session": session, "agent": agent.name(), "folder": folder, "last": last}),
        );
        if looked {
            return;
        }
        // FERB's own words only: the agent's reply is in the session, never in FERB's notice (rule 9)
        let ident = format!("sessions.finished:{session}");
        (self.seams.tell)(&ident, &format!("{folder} is done and waiting for you."));
    }

    // ---------- sending ----------

    /// The question the owner is asked before his first typed send (GATE-622), None when the send needs none.
    pub fn needs_yes(
        &self,
        id: &str,
        text: &str,
        by: &Caller,
    ) -> Result<Option<NeedsYes>, Refused> {
        let s = self.session(id).ok_or_else(gone)?;
        if by.is_task() || s.control == by.key() {
            return Ok(None);
        }
        let Some(path) = s.send_paths.first() else {
            return Err(no(codes::NO_SEND_PATH, s.no_send.clone()));
        };
        let app = s
            .host
            .as_ref()
            .map(|h| h.name.clone())
            .filter(|n| !n.is_empty());
        let place = match (path, &app) {
            (_, Some(a)) if s.agent == Agent::AppChat => a.clone(),
            (_, Some(a)) if s.agent == Agent::Terminal => format!(
                "{} in {a}",
                if s.program.is_empty() {
                    "the terminal"
                } else {
                    &s.program
                }
            ),
            (SendPath::AgentApi, _) => format!("{} in {}", s.agent.name(), folder_name(&s.cwd)),
            (_, Some(a)) => format!("{} in {a}", s.agent.name()),
            (_, None) => s.agent.name().to_owned(),
        };
        let question = if path.foreground() {
            format!(
                "Type this into {place}? FERB will bring {} to the front to do it.",
                app.unwrap_or_else(|| "its window".into())
            )
        } else {
            format!("Send this to {place}?")
        };
        Ok(Some(NeedsYes {
            question,
            detail: self.redact(text),
            covers: "Yes lets you send to this session from here until you stop controlling it."
                .into(),
        }))
    }

    /// The owner's Yes (GATE-622): he controls this session from `by` until he lets go.
    pub fn take_control(&self, id: &str, by: &Caller) -> Result<(), Refused> {
        let mut st = lock(&self.state);
        if !st
            .sessions
            .get(id)
            .is_some_and(|e| e.s.status != Status::Ended)
        {
            return Err(gone());
        }
        st.control.insert(id.to_owned(), by.key());
        drop(st);
        (self.seams.journal)("sessions.control", json!({"session": id, "by": by.key()}));
        self.changed();
        Ok(())
    }

    /// The owner lets go (or a phone locked): `by`'s control ends.
    pub fn release_control(&self, id: Option<&str>, by: &Caller) -> usize {
        let key = by.key();
        let mut st = lock(&self.state);
        let before = st.control.len();
        let gone: Vec<String> = st
            .control
            .iter()
            .filter(|(s, k)| *k == &key && id.is_none_or(|i| i == s.as_str()))
            .map(|(s, _)| s.clone())
            .collect();
        st.control
            .retain(|s, k| !(k == &key && id.is_none_or(|i| i == s)));
        // HAND-683: his choice of terminal lasts while he controls the session
        for s in &gone {
            st.chosen.remove(s);
        }
        let n = before - st.control.len();
        drop(st);
        if n > 0 {
            (self.seams.journal)("sessions.release", json!({"session": id, "by": key}));
            self.changed();
        }
        n
    }

    /// Sends the owner's (or FERB's task's) words into a session: the first path that applies, the structural checks
    /// before, the proof in the agent's own record after.
    pub async fn send(&self, id: &str, text: &str, by: &Caller) -> Result<Sent, Refused> {
        if !self.enabled() {
            return Err(off());
        }
        let asked_at = (self.seams.mono)();
        let text = text.trim_end().to_owned();
        if text.trim().is_empty() {
            return Err(no(codes::SEND_FAILED, "There's nothing to send."));
        }
        if text.chars().count() > MAX_TEXT {
            return Err(no(codes::SEND_FAILED, "That message is too long to send."));
        }
        self.prime(id).await;
        let s = self.session(id).ok_or_else(gone)?;
        if !by.is_task() && s.control != by.key() {
            return Err(no(
                codes::NOT_THE_SESSION,
                "Say Yes to sending to this session first.",
            ));
        }
        let path = *s
            .send_paths
            .first()
            .ok_or_else(|| no(codes::NO_SEND_PATH, s.no_send.clone()))?;
        if s.waiting != Waiting::None {
            return Err(no(
                codes::CARD_OPEN,
                format!("{} is asking something. Answer that first.", s.agent.name()),
            ));
        }
        if s.agent == Agent::Terminal {
            self.term_unchanged(&s)?;
        }
        let before = self.last_seq(id);
        let started = (self.seams.mono)();
        self.deliver(&s, path, &text, "").await?;
        if s.agent == Agent::AppChat {
            self.chat_sent(id, &text);
        }
        if let Some(e) = lock(&self.state).sessions.get_mut(id) {
            e.via_ferb = true;
            e.s.status = Status::Working;
        }
        (self.seams.journal)(
            "sessions.sent",
            json!({"session": id, "by": by.key(), "path": path.as_str(), "text": self.redact(&text)}),
        );
        let delivered = self.delivered(id, &text, before, asked_at).await;
        self.changed();
        Ok(Sent {
            path,
            delivered,
            seconds: (((self.seams.mono)() - started) * 100.0).round() / 100.0,
        })
    }

    /// One path's delivery (text, or a key when `key` is set).
    async fn deliver(
        &self,
        s: &Session,
        path: SendPath,
        text: &str,
        key: &str,
    ) -> Result<(), Refused> {
        let failed = |e: String| no(codes::SEND_FAILED, e);
        // HAND-694: a terminal session is the terminal itself: whatever leads it gets the words (checked in `send`)
        if path.types() && s.agent != Agent::AppChat && s.agent != Agent::Terminal {
            // decision: HAND-658 — typed text reaches the agent only when it is its terminal's foreground program
            let table = self.seams.mac.processes();
            if !host::in_front(&table, s.pid) {
                return Err(no(
                    codes::NOT_IN_FRONT,
                    format!(
                        "{} isn't the program in front in its terminal, so typing there would reach something else. Nothing was sent.",
                        s.agent.name()
                    ),
                ));
            }
        }
        let host = s.host.clone().unwrap_or_default();
        let agent_tty = self
            .seams
            .mac
            .processes()
            .get(&s.pid)
            .map(|p| p.tty.clone())
            .unwrap_or_default();
        match path {
            SendPath::AgentApi => {
                let turn = lock(&self.state)
                    .sessions
                    .get(&s.id)
                    .map(|e| e.turn.clone())
                    .unwrap_or_default();
                if key == "escape" {
                    self.codex
                        .interrupt(&s.agent_session, &turn)
                        .await
                        .map_err(failed)
                } else {
                    self.codex
                        .send(&s.agent_session, text, &turn)
                        .await
                        .map_err(failed)
                }
            }
            SendPath::Tmux => {
                let server = self.tmux_server(s.pid);
                let mac = self.seams.mac.clone();
                let (t, k) = (
                    text.to_owned(),
                    if key == "escape" {
                        "Escape".to_owned()
                    } else {
                        String::new()
                    },
                );
                let enter = key.is_empty();
                tokio::task::spawn_blocking(move || {
                    mac.tmux_send(server, &agent_tty, &t, &k, enter)
                })
                .await
                .map_err(|e| failed(e.to_string()))?
                .map_err(failed)
            }
            SendPath::TerminalScript if key.is_empty() => {
                let mac = self.seams.mac.clone();
                let (tty, t) = (host.tty.clone(), text.to_owned());
                tokio::task::spawn_blocking(move || mac.terminal_type(&tty, &t))
                    .await
                    .map_err(|e| failed(e.to_string()))?
                    .map_err(failed)
            }
            // HAND-672: an app chat's stop is its own stop control
            SendPath::Foreground if s.agent == Agent::AppChat && key == "escape" => {
                let (win, labels) = self
                    .chat_stop(&s.id)
                    .ok_or_else(|| no(codes::SEND_FAILED, "It isn't replying now."))?;
                let mac = self.seams.mac.clone();
                tokio::task::spawn_blocking(move || mac.chat_press(&win, &labels))
                    .await
                    .map_err(|e| failed(e.to_string()))?
                    .map_err(failed)
            }
            SendPath::TerminalScript | SendPath::Foreground => {
                let Place {
                    proof,
                    chosen,
                    chat,
                    ..
                } = self.place_of(s, &host);
                // decision: HAND-698 — no proven chat pane: nothing is typed anywhere in the app
                if s.agent == Agent::AppChat && chat.is_none() {
                    let app = s.host.as_ref().map(|h| h.name.clone()).unwrap_or_default();
                    return Err(no(
                        codes::NO_SEND_PATH,
                        format!("No agent chat open in {app}. Start one there to continue here."),
                    ));
                }
                let typed = self
                    .seams
                    .mac
                    .foreground(Foreground {
                        bundle_id: host.bundle_id.clone(),
                        app: host.name.clone(),
                        pid: host.pid,
                        proof,
                        text: if key.is_empty() {
                            text.to_owned()
                        } else {
                            String::new()
                        },
                        enter: key.is_empty(),
                        key: key.to_owned(),
                        composer: chat.is_some(),
                        window: chat.as_ref().map_or(0, |(w, _)| w.window),
                        chat: chat.as_ref().map(|(_, fp)| fp.clone()),
                        chosen,
                    })
                    .await;
                self.typed(&s.id, typed)
            }
        }
    }

    /// PHN-797: where the session is shown on the Mac, for its Watch view: found as a send finds it. None when no
    /// app's window shows it (a detached tmux, an agent with no app above it).
    pub fn place(&self, id: &str) -> Option<Place> {
        let s = self.session(id)?;
        let host = s
            .host
            .clone()
            .filter(|h| h.pid > 0 && !h.bundle_id.is_empty())?;
        Some(self.place_of(&s, &host))
    }

    /// The send path's facts about where the session is (HAND-682, HAND-683, HAND-672).
    fn place_of(&self, s: &Session, host: &Host) -> Place {
        let chat = (s.agent == Agent::AppChat)
            .then(|| self.chat_window(&s.id))
            .flatten();
        let (chosen, window) = {
            let st = lock(&self.state);
            let chosen = st
                .chosen
                .get(&s.id)
                .filter(|(pid, _)| *pid == s.pid)
                .map(|(_, t)| t.clone())
                .unwrap_or_default();
            let window = st
                .chats
                .get(&s.id)
                .map(|c| c.win.window)
                .or_else(|| {
                    st.sessions
                        .get(&s.id)
                        .and_then(|e| e.term.as_ref())
                        .map(|t| t.window)
                })
                .unwrap_or(0);
            (chosen, window)
        };
        Place {
            host: host.clone(),
            proof: self.proof_of(s, host),
            chosen,
            chat,
            window,
        }
    }

    /// HAND-682: what proves a terminal is this session's. Its program and folder are offered only when no other
    /// running session in the same app shares both: then a terminal showing them can only be this one's.
    fn proof_of(&self, s: &Session, host: &Host) -> ferb_sessions::proof::Proof {
        use ferb_sessions::proof::{program_name, Proof};
        let table = self.seams.mac.processes();
        let program_of = |pid: i32| {
            table
                .get(&pid)
                .map(|p| program_name(&p.exe))
                .unwrap_or_default()
        };
        let (program, folder) = (program_of(s.pid), folder_name(&s.cwd));
        let shared = !program.is_empty()
            && !folder.is_empty()
            && self.list().iter().any(|o| {
                o.id != s.id
                    && o.status != Status::Ended
                    && o.host.as_ref().is_some_and(|h| h.pid == host.pid)
                    && folder_name(&o.cwd) == folder
                    && program_of(o.pid) == program
            });
        Proof {
            title: s.title.clone(),
            program: if shared { String::new() } else { program },
            folder: if shared { String::new() } else { folder },
        }
    }

    /// HAND-683: what the foreground path's answer means for the session. Typed: any choice of terminals offered
    /// goes. Couldn't tell which terminal: the ones FERB can see go on the session for the owner to choose, and a
    /// choice that no longer holds is forgotten (he is asked again).
    fn typed(&self, id: &str, r: Result<(), NotTyped>) -> Result<(), Refused> {
        let mut st = lock(&self.state);
        match r {
            Ok(()) => {
                if let Some(e) = st.sessions.get_mut(id) {
                    e.s.terminals.clear();
                }
                Ok(())
            }
            Err(NotTyped::Failed(why)) => Err(no(codes::NOT_THE_SESSION, why)),
            Err(NotTyped::Choose { why, terminals }) => {
                st.chosen.remove(id);
                let offered = !terminals.is_empty();
                if let Some(e) = st.sessions.get_mut(id) {
                    e.s.terminals = terminals;
                }
                drop(st);
                self.changed();
                Err(no(
                    if offered {
                        codes::CHOOSE_TERMINAL
                    } else {
                        codes::NOT_THE_SESSION
                    },
                    why,
                ))
            }
        }
    }

    /// HAND-683: the owner's choice of terminal for a session, one of those it offers; it lasts until the session
    /// ends or he stops controlling it.
    pub fn choose_terminal(&self, id: &str, terminal: &str) -> Result<(), Refused> {
        let mut st = lock(&self.state);
        let pid = st.sessions.get(id).map_or(0, |e| e.s.pid);
        let offered = st
            .sessions
            .get(id)
            .is_some_and(|e| e.s.terminals.iter().any(|t| t.id == terminal));
        if !offered {
            return Err(no(
                codes::NOT_THE_SESSION,
                "That terminal isn't one FERB offered for this session. Send again to see them.",
            ));
        }
        st.chosen.insert(id.to_owned(), (pid, terminal.to_owned()));
        drop(st);
        (self.seams.journal)("sessions.terminal_chosen", json!({"session": id}));
        Ok(())
    }

    fn tmux_server(&self, pid: i32) -> i32 {
        let t = self.seams.mac.processes();
        host::ancestors(&t, pid)
            .into_iter()
            .skip(1)
            .find(|p| t.get(p).is_some_and(host::is_tmux))
            .unwrap_or(0)
    }

    fn last_seq(&self, id: &str) -> u64 {
        lock(&self.state)
            .sessions
            .get(id)
            .and_then(|e| e.items.last().map(|i| i.seq))
            .unwrap_or(0)
    }

    /// HAND-655: the sent words appear as the owner's message in the agent's own record.
    async fn delivered(&self, id: &str, text: &str, before: u64, asked_at: f64) -> bool {
        let want = one_line(&self.redact(text));
        let deadline = ((self.seams.mono)() + DELIVERY_WAIT_S).min(asked_at + ANSWER_WITHIN_S);
        loop {
            // a read of Codex's thread waits on its daemon: off the async thread
            let (me, i) = (self.clone(), id.to_owned());
            let (me2, i2, t2) = (self.clone(), id.to_owned(), text.to_owned());
            let terminal = Agent::split(id).map(|(a, _)| a) == Some(Agent::Terminal);
            if terminal {
                // decision: HAND-693 — a terminal's proof is its own text showing the words
                if tokio::task::spawn_blocking(move || me2.term_delivered(&i2, &t2))
                    .await
                    .unwrap_or(false)
                {
                    return true;
                }
            }
            let _ = tokio::task::spawn_blocking(move || match Agent::split(&i).map(|(a, _)| a) {
                Some(Agent::Codex) => me.codex_refresh(&i),
                Some(Agent::AppChat) => me.chat_look(&i, true),
                Some(Agent::Terminal) => {}
                _ => me.tail(&i),
            })
            .await;
            let found = lock(&self.state).sessions.get(id).is_some_and(|e| {
                e.items.iter().any(|i| {
                    i.seq > before && i.kind == ItemKind::User && one_line(&i.text) == want
                })
            });
            if found {
                return true;
            }
            if (self.seams.mono)() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(if terminal {
                500
            } else {
                150
            }))
            .await;
        }
    }

    /// Stops the agent's turn (its own interrupt: Esc for Claude Code, `turn/interrupt` for Codex).
    pub async fn interrupt(&self, id: &str, by: &Caller) -> Result<(), Refused> {
        if !self.enabled() {
            return Err(off());
        }
        self.prime(id).await;
        let s = self.session(id).ok_or_else(gone)?;
        if !by.is_task() && s.control != by.key() {
            return Err(no(
                codes::NOT_THE_SESSION,
                "Say Yes to sending to this session first.",
            ));
        }
        let path = *s
            .send_paths
            .first()
            .ok_or_else(|| no(codes::NO_SEND_PATH, s.no_send.clone()))?;
        // Terminal's scripting can't press a key alone: Esc goes in through the foreground path
        let path = if path == SendPath::TerminalScript {
            SendPath::Foreground
        } else {
            path
        };
        self.deliver(&s, path, "", "escape").await?;
        (self.seams.journal)("sessions.interrupt", json!({"session": id, "by": by.key()}));
        Ok(())
    }

    /// The folders a new session may start in: the running sessions' and the ones Claude Code's own prompt history
    /// names most recently (at most 20, existing folders only).
    pub fn folders(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .list()
            .into_iter()
            .map(|s| s.cwd)
            .filter(|c| !c.is_empty())
            .collect();
        let history = self.seams.home.join(".claude").join("history.jsonl");
        if let Ok(text) = std::fs::read_to_string(&history) {
            for line in text.lines().rev().take(2000) {
                if let Some(p) = serde_json::from_str::<Value>(line)
                    .ok()
                    .and_then(|v| v.get("project").and_then(Value::as_str).map(str::to_owned))
                {
                    if !out.contains(&p) && Path::new(&p).is_dir() {
                        out.push(p);
                    }
                }
                if out.len() >= 20 {
                    break;
                }
            }
        }
        out.dedup();
        out.truncate(20);
        out
    }

    /// An older phone's New session named an agent (before HAND-691): its command, as that phone showed it, is the
    /// agent's program with the first words; it is typed into the new terminal like any command the owner gives.
    pub fn legacy_command(agent: Agent, text: &str) -> Option<String> {
        let program = match agent {
            Agent::ClaudeCode => "claude",
            Agent::Codex => "codex",
            Agent::AppChat | Agent::Terminal => return None,
        };
        let mut cmd = program.to_owned();
        if !text.trim().is_empty() {
            cmd.push(' ');
            cmd.push_str(&shell_quote(text.trim()));
        }
        Some(cmd)
    }

    /// The owner hands a session to FERB with his goal (ORCH-687): his words become a chat message, a FERB task like
    /// any other, which reads and answers the session with its tools. HAND-702: the session says what became of it
    /// (`ferb_note`), and "FERB is on it" lasts only while FERB's task for it runs.
    pub fn delegate(
        &self,
        id: &str,
        goal: &str,
        client: Option<String>,
    ) -> Result<String, Refused> {
        if !self.enabled() {
            return Err(off());
        }
        let goal = goal.trim();
        if goal.is_empty() {
            return Err(no(
                codes::SEND_FAILED,
                "Say what FERB should get done in this session.",
            ));
        }
        let s = self.session(id).ok_or_else(gone)?;
        let text = hand_over_text(&s, goal);
        let me = self.clone();
        let sid = id.to_owned();
        let news: HandNews = Arc::new(move |h: Hand| me.hand_news(&sid, h));
        // the goal is set before the message goes: its first news can come at once
        if let Some(e) = lock(&self.state).sessions.get_mut(id) {
            e.s.goal = self.redact(goal);
            e.s.delegate.clear();
            e.s.ferb_note = format!("Handed to FERB: “{}”", clip(&one_line(&e.s.goal), 160));
        }
        if let Err(e) = (self.seams.hand_over)(text.clone(), client, news) {
            if let Some(x) = lock(&self.state).sessions.get_mut(id) {
                x.s.goal.clear();
                x.s.ferb_note.clear();
            }
            self.changed();
            return Err(no(codes::SEND_FAILED, e));
        }
        (self.seams.journal)("sessions.delegated", json!({"session": id}));
        self.changed();
        Ok(text)
    }

    /// HAND-702: what became of a hand-over message, said in the session. Only a started task keeps "FERB is on it";
    /// a question, an answer or a failure ends it and says so.
    pub fn hand_news(&self, id: &str, h: Hand) {
        let quote = |t: &str| clip(&one_line(&self.redact(t)), 300);
        {
            let mut st = lock(&self.state);
            let Some(e) = st.sessions.get_mut(id) else {
                return;
            };
            // a later hand-over, or his taking it back, already replaced this one
            if e.s.goal.is_empty() && e.s.delegate.is_empty() {
                return;
            }
            match &h {
                Hand::Started(task) => {
                    e.s.delegate = task.clone();
                    e.s.ferb_note = format!("FERB is on it: “{}”", clip(&one_line(&e.s.goal), 160));
                }
                Hand::Asked(q) => {
                    e.s.goal.clear();
                    e.s.delegate.clear();
                    e.s.ferb_note = format!(
                        "FERB asked you something in its chat before it starts: “{}”",
                        quote(q)
                    );
                }
                Hand::Said(t) => {
                    e.s.goal.clear();
                    e.s.delegate.clear();
                    e.s.ferb_note =
                        format!("FERB answered in its chat and didn't start: “{}”", quote(t));
                }
                Hand::Failed(t) => {
                    e.s.goal.clear();
                    e.s.delegate.clear();
                    e.s.ferb_note = format!("FERB couldn't start on it: “{}”", quote(t));
                }
            }
        }
        (self.seams.journal)(
            "sessions.hand_news",
            json!({"session": id, "news": match h {
                Hand::Started(_) => "started",
                Hand::Asked(_) => "asked",
                Hand::Said(_) => "said",
                Hand::Failed(_) => "failed",
            }}),
        );
        self.changed();
    }

    /// A FERB task acted on a session: the session shows it is FERB's (until the task ends).
    pub fn task_touched(&self, id: &str, tree: &str) {
        if let Some(e) = lock(&self.state).sessions.get_mut(id) {
            if e.s.delegate != tree {
                e.s.ferb_note = "FERB is working in this session.".into();
            }
            e.s.delegate = tree.to_owned();
        }
        self.changed();
    }

    /// A task ended (`status`: done, failed, stopped, cancelled…): the sessions it held are the owner's again, and
    /// each says how it ended (HAND-702).
    pub fn task_ended(&self, tree: &str, status: &str) {
        if tree.is_empty() {
            return;
        }
        let mut any = false;
        for e in lock(&self.state).sessions.values_mut() {
            if e.s.delegate == tree {
                e.s.delegate.clear();
                e.s.goal.clear();
                e.s.ferb_note = match status {
                    "done" => "FERB finished. The session is yours again.",
                    "stopped" | "cancelled" | "interrupted" => {
                        "FERB stopped. The session is yours again."
                    }
                    _ => "FERB stopped without finishing. Its chat says why.",
                }
                .into();
                any = true;
            }
        }
        if any {
            self.changed();
        }
    }

    /// Waits in code (no model calls) until the session's turn is over: idle, waiting for the owner, or ended;
    /// `limit_s` at most. Answers the session as it stands then (None: it ended).
    pub async fn wait_turn(&self, id: &str, limit_s: f64) -> Option<Session> {
        let deadline = (self.seams.mono)() + limit_s;
        // a turn just sent may not have started yet: give the agent a moment to begin
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let chat = Agent::split(id).map(|(a, _)| a) == Some(Agent::AppChat);
        loop {
            if chat {
                // HAND-671: the app's reply is watched by code, a look every LOOK_REPLYING_S until it settles
                let (me, i) = (self.clone(), id.to_owned());
                let _ = tokio::task::spawn_blocking(move || me.chat_look(&i, true)).await;
            }
            match self.session(id) {
                None => return None,
                Some(s) if s.status != Status::Working => return Some(s),
                Some(s) if (self.seams.mono)() >= deadline => return Some(s),
                _ => {}
            }
            tokio::time::sleep(std::time::Duration::from_millis(if chat {
                (ferb_sessions::appchat::LOOK_REPLYING_S * 1000.0) as u64
            } else {
                500
            }))
            .await;
        }
    }

    // ---------- the own question that turns sessions on ----------

    /// Once after setup, while the owner hasn't said: FERB asks (HAND-696: every Mac has terminals).
    pub fn offer(&self) {
        if (self.seams.enabled)().is_some() || !(self.seams.setup_done)() {
            return;
        }
        {
            let mut st = lock(&self.state);
            if st.asked_enable {
                return;
            }
            st.asked_enable = true;
        }
        let (_, answer) = (self.seams.ask)(Ask {
            capability: ENABLE_ASK.into(),
            summary: ENABLE_SUMMARY.into(),
            detail: ENABLE_DETAIL.into(),
            options: vec![ENABLE_YES.into(), ENABLE_NO.into()],
        });
        let me = self.clone();
        tokio::spawn(async move {
            let Some(choice) = answer.await else {
                // set aside (a restart, another answer): asked again at the next start
                lock(&me.state).asked_enable = false;
                return;
            };
            let yes = choice == ENABLE_YES;
            if let Err(e) = (me.seams.set_enabled)(yes) {
                (me.seams.journal)("sessions.setting_failed", json!({"error": e}));
                return;
            }
            (me.seams.journal)(
                "sessions.enabled",
                json!({"enabled": yes, "by": "question"}),
            );
            let me2 = me.clone();
            let _ = tokio::task::spawn_blocking(move || me2.look()).await;
        });
    }

    // ---------- Codex ----------

    /// A Codex thread's facts and items, read again (`thread/read`).
    fn codex_refresh(&self, id: &str) {
        let Some((Agent::Codex, tid)) = Agent::split(id) else {
            return;
        };
        if let Some(t) = self.codex.read_now(tid) {
            self.note_codex(&t, true);
        }
    }

    /// A Codex thread seen.
    pub(crate) fn note_codex(&self, t: &codex::Thread, with_items: bool) {
        if t.ephemeral {
            return;
        }
        let id = Agent::Codex.session_id(&t.id);
        let now = self.now();
        let table = self.seams.mac.processes();
        let pid = codex_pid(&table, &t.cwd, &*self.seams.mac);
        {
            let mut st = lock(&self.state);
            let e = st.sessions.entry(id.clone()).or_insert_with(|| {
                let mut e = Entry::new(&id, Agent::Codex, &now);
                e.s.cwd = t.cwd.clone();
                e.s.status = t.status;
                e.s.waiting = t.waiting;
                e
            });
            if t.status == Status::Ended {
                e.s.status = Status::Ended;
            } else if e.cards.is_empty() {
                e.s.status = t.status;
                e.s.waiting = t.waiting;
            }
            if !t.title.is_empty() {
                e.s.title = (self.seams.redact)(&t.title);
            }
            if !t.cwd.is_empty() {
                e.s.cwd = t.cwd.clone();
            }
            if pid > 0 {
                e.s.pid = pid;
            }
            if with_items {
                e.items = t
                    .items
                    .iter()
                    .enumerate()
                    .map(|(i, d)| Item {
                        seq: i as u64 + 1,
                        at: d.at.clone(),
                        kind: d.kind,
                        text: (self.seams.redact)(&d.text),
                        tool: d.tool.clone(),
                        parts: Vec::new(),
                    })
                    .collect();
                if let Some(last) = e.items.iter().rev().find(|i| i.kind == ItemKind::Assistant) {
                    e.s.last_text = clip(&one_line(&last.text), 200);
                }
            }
        }
        if t.status == Status::Ended {
            self.end(&id);
        } else {
            self.resolve_host(&id, &table);
        }
    }

    /// One Codex event (from the link's reader).
    pub(crate) fn on_codex(&self, ev: codex::Event) {
        match ev {
            codex::Event::Status {
                thread,
                status,
                waiting,
            } => {
                let id = Agent::Codex.session_id(&thread);
                if status == Status::Ended {
                    self.end(&id);
                } else if let Some(e) = lock(&self.state).sessions.get_mut(&id) {
                    if e.cards.is_empty() || status != Status::Working {
                        e.s.status = status;
                        e.s.waiting = waiting;
                    }
                    e.s.last_at = self.now();
                } else {
                    // a thread FERB hasn't read yet (this thread is the events' own, never the reader's)
                    self.codex_refresh(&id);
                }
            }
            codex::Event::TurnStarted { thread, turn } => {
                let id = Agent::Codex.session_id(&thread);
                if let Some(e) = lock(&self.state).sessions.get_mut(&id) {
                    e.turn = turn;
                    e.s.status = Status::Working;
                }
            }
            codex::Event::TurnDone { thread, .. } => {
                let id = Agent::Codex.session_id(&thread);
                let mut withdraw = Vec::new();
                let tell = {
                    let mut st = lock(&self.state);
                    match st.sessions.get_mut(&id) {
                        Some(e) => {
                            e.turn.clear();
                            e.s.status = Status::Idle;
                            e.s.waiting = Waiting::None;
                            withdraw.append(&mut e.cards);
                            std::mem::take(&mut e.via_ferb)
                        }
                        None => false,
                    }
                };
                for h in withdraw {
                    self.release(h);
                }
                self.codex_refresh(&id);
                if tell {
                    self.tell_finished(&id);
                }
            }
            codex::Event::Item { thread, .. } => {
                self.codex_refresh(&Agent::Codex.session_id(&thread));
            }
            codex::Event::Started(t) => self.note_codex(&t, false),
            codex::Event::Closed { thread } => self.end(&Agent::Codex.session_id(&thread)),
            codex::Event::Asks {
                id: rid,
                thread,
                mut card,
            } => {
                let id = Agent::Codex.session_id(&thread);
                let card_id = (self.seams.new_id)();
                card.id = card_id.clone();
                card.session = id.clone();
                card.asked_at = self.now();
                card.detail = (self.seams.redact)(&card.detail);
                let known = {
                    let mut st = lock(&self.state);
                    match st.sessions.get_mut(&id) {
                        Some(e) => {
                            e.s.status = Status::WaitingForYou;
                            e.s.waiting = Waiting::Permission;
                            e.cards.push(Held {
                                card,
                                input: Value::Null,
                                suggestions: Value::Null,
                                reply: None,
                                codex: Some(rid),
                            });
                            true
                        }
                        None => false,
                    }
                };
                if known {
                    self.ask_card(&id, &card_id);
                }
            }
            codex::Event::Error { .. } | codex::Event::Answer { .. } | codex::Event::Other => {}
        }
        self.changed();
    }
}

/// Releases a held card when the hook's call is dropped (its process ended: Claude Code's own timeout, or FERB
/// restarting): Claude Code's own dialog decides, as it would without FERB.
struct CardGuard {
    sessions: Sessions,
    session: String,
    card: String,
}

impl Drop for CardGuard {
    fn drop(&mut self) {
        let held = {
            let mut st = lock(&self.sessions.state);
            st.sessions.get_mut(&self.session).and_then(|e| {
                let at = e.cards.iter().position(|h| h.card.id == self.card)?;
                Some(e.cards.remove(at))
            })
        };
        if let Some(h) = held {
            self.sessions.release(h);
        }
    }
}

trait ToolOrQuestion {
    fn tool_or_question(&self) -> String;
}

impl ToolOrQuestion for Card {
    fn tool_or_question(&self) -> String {
        match self.kind {
            CardKind::Permission => self.tool.clone(),
            CardKind::Question => claude::QUESTION_TOOL.to_owned(),
        }
    }
}

fn gone() -> Refused {
    no(codes::SESSION_GONE, "That session has ended.")
}

fn off() -> Refused {
    no(
        codes::SESSIONS_OFF,
        "Seeing your terminal sessions is off. Turn it on in FERB's Settings.",
    )
}

/// The question that turns sessions on (HAND-696).
pub const ENABLE_SUMMARY: &str = "Let FERB see your terminal sessions?";
pub const ENABLE_DETAIL: &str = "Then you can follow your terminals and type into them from your phone or this Mac, Claude Code and Codex too, open new ones, and hand one to me to keep going. I'd add a small hook to Claude Code's settings so I hear when a session finishes or needs you; nothing else is changed. What I read stays on this Mac, and you can turn this off in Settings.";
pub const ENABLE_YES: &str = "Yes, let FERB see them";
pub const ENABLE_NO: &str = "No, thanks";

/// HAND-702: the owner's hand-over message: the session by its kind, title, folder, app and id, that it is on this
/// Mac and is driven with the agent_session tools (read it, type into it), and his goal in his words.
pub fn hand_over_text(s: &Session, goal: &str) -> String {
    let where_ = match s
        .host
        .as_ref()
        .map(|h| h.name.as_str())
        .filter(|n| !n.is_empty())
    {
        Some(app) => format!(" in {app}"),
        None => String::new(),
    };
    let title = if s.title.is_empty() {
        String::new()
    } else {
        format!(" \"{}\"", s.title)
    };
    let what = match s.agent {
        Agent::AppChat => {
            let app = s.host.as_ref().map(|h| h.name.clone()).unwrap_or_default();
            format!("my {app} chat{title} (session {})", s.id)
        }
        Agent::Terminal => {
            let program = if s.program.is_empty() {
                String::new()
            } else {
                format!(" running {}", s.program)
            };
            format!("my terminal{program} ({}{where_}, session {})", s.cwd, s.id)
        }
        _ => format!(
            "my {} session{title} ({}{where_}, session {})",
            s.agent.name(),
            s.cwd,
            s.id
        ),
    };
    format!(
        "Take over {what} on this Mac. It's a session on the Mac, not a person to message: read it and type into it with the agent_session tools, and keep it going until this is done: {goal}"
    )
}

/// The last part of a folder ("FERB" for "/Users/x/Desktop/FERB").
pub fn folder_name(cwd: &str) -> String {
    Path::new(cwd)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| cwd.to_owned())
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Writes a file whole or not at all (a temporary file beside it, then a rename), keeping its permissions.
pub fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let dir = path.parent().ok_or("no folder")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(
        ".{}.ferb-{}",
        path.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default(),
        std::process::id()
    ));
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    if let Ok(m) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, m.permissions());
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

/// A Claude Code session's transcript, by its id, under `~/.claude/projects/*/`.
pub fn find_transcript(home: &Path, sid: &str) -> Option<PathBuf> {
    let projects = home.join(".claude").join("projects");
    let name = format!("{sid}.jsonl");
    std::fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|d| d.path().join(&name))
        .find(|p| p.is_file())
}

/// The agent among a hook's ancestors: the first that leads its terminal's foreground group (the hook's own shell is
/// in the agent's group but doesn't lead it).
pub fn agent_of(t: &Table, ancestors: &[i32]) -> i32 {
    ancestors
        .iter()
        .copied()
        .find(|p| {
            t.get(p)
                .is_some_and(|x| x.pgid == x.pid && x.pid > 0 && !x.tty.is_empty())
        })
        .unwrap_or(0)
}

/// The Codex program with a terminal working in `cwd`, when exactly one is.
fn codex_pid(t: &Table, cwd: &str, mac: &dyn Mac) -> i32 {
    if cwd.is_empty() {
        return 0;
    }
    let found: Vec<i32> = t
        .values()
        .filter(|p| !p.tty.is_empty() && host::program(&p.exe) == "codex")
        .filter(|p| mac.cwd_of(p.pid) == cwd)
        .map(|p| p.pid)
        .collect();
    if found.len() == 1 {
        found[0]
    } else {
        0
    }
}

/// The JSON a client reads for a session's state; `yours`: the caller controls it.
pub fn session_json(s: &Session, by: &Caller) -> Value {
    let mut v = serde_json::to_value(s).unwrap_or(Value::Null);
    if let Value::Object(m) = &mut v {
        m.insert("yours".into(), json!(s.control == by.key()));
    }
    v
}

/// Builds the `sessions.list` answer for `by`.
pub fn list_json(me: &Sessions, by: &Caller) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("enabled".into(), json!(me.enabled()));
    // HAND-697: the chat apps open with no chat FERB could find (their names; the clients say "No agent chat open in …", HAND-705)
    m.insert(
        "no_chats".into(),
        json!(if me.enabled() {
            me.no_chat_apps()
        } else {
            Vec::new()
        }),
    );
    m.insert(
        "sessions".into(),
        Value::Array(me.list().iter().map(|s| session_json(s, by)).collect()),
    );
    m
}
