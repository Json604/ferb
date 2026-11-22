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

mod codex_link;
mod rpc;

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

pub use codex_link::CodexLink;
pub use rpc::register;

/// The setting (config `[sessions] enabled`): off until the owner says Yes (HAND-666).
pub const SECTION: &str = "sessions";
pub const KEY: &str = "enabled";

/// The own questions' capabilities (the gateway's store; a permission's Yes takes the fingerprint on a phone).
pub const PERMISSION_ASK: &str = "agent_session.permission";
pub const QUESTION_ASK: &str = "agent_session.question";
pub const ENABLE_ASK: &str = "agent_session.enable";

/// How long a send waits to see the owner's words in the agent's own record before it says "not delivered" (HAND-655;
/// the lab measured 0.15-1.7 s on every path).
pub const DELIVERY_WAIT_S: f64 = 10.0;
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
    /// What proves the focused window or terminal is the session's: its title, else its folder's name.
    pub proof: Vec<String>,
    /// The text to type ("" for a key alone); Return follows when `enter`.
    pub text: String,
    pub enter: bool,
    /// A key instead of text ("escape").
    pub key: String,
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
    fn foreground(&self, f: Foreground) -> BoxFuture<'static, Result<(), String>>;
}

/// FERB's own question to the owner (the gateway's store): (capability, summary, detail, options). Answers the
/// chosen option, None when it was set aside or not answered.
pub type AskFn = Arc<dyn Fn(Ask) -> (String, BoxFuture<'static, Option<String>>) + Send + Sync>;

/// One notice line: (ident, text).
pub type Tell = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// A bus topic or journal kind with its data.
pub type Note = Arc<dyn Fn(&str, Value) + Send + Sync>;

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
    /// One notice line in the owner's chat, once per ident (the notifier: every client and phone sees it).
    pub tell: Tell,
    pub ask: AskFn,
    /// Sets one of FERB's questions aside (it was answered elsewhere).
    pub close_ask: Arc<dyn Fn(&str) + Send + Sync>,
    /// The owner's words as a chat message (his hand-over): (text, client id).
    pub hand_over: Arc<dyn Fn(String, Option<String>) -> Result<(), String> + Send + Sync>,
    pub now: Arc<dyn Fn() -> String + Send + Sync>,
    pub mono: Arc<dyn Fn() -> f64 + Send + Sync>,
    pub new_id: Arc<dyn Fn() -> String + Send + Sync>,
    pub version: String,
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
        if Agent::split(id).map(|(a, _)| a) == Some(Agent::Codex) {
            self.codex_refresh(id);
        }
        let st = lock(&self.state);
        let e = st
            .sessions
            .get(id)
            .filter(|e| e.s.status != Status::Ended)
            .ok_or_else(gone)?;
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
        self.changed();
    }

    /// Hooks in place exactly while sessions are on (HAND-666): Claude Code's settings, FERB's entries only.
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
        let e = st.sessions.entry(id.clone()).or_insert_with(|| Entry {
            s: Session {
                id: id.clone(),
                agent: Agent::ClaudeCode,
                agent_session: sid.to_owned(),
                source: Source::Attached,
                pid: 0,
                cwd: String::new(),
                title: String::new(),
                status: Status::Idle,
                waiting: Waiting::None,
                host: None,
                send_paths: Vec::new(),
                no_send: String::new(),
                last_at: now.clone(),
                last_text: String::new(),
                control: String::new(),
                delegate: String::new(),
                goal: String::new(),
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
        });
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
                Host {
                    bundle_id,
                    name,
                    pid: f.pid,
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

    fn tell_finished(&self, session: &str) {
        let (agent, folder, last) = {
            let st = lock(&self.state);
            let Some(e) = st.sessions.get(session) else {
                return;
            };
            (e.s.agent, folder_name(&e.s.cwd), e.s.last_text.clone())
        };
        (self.seams.publish)(
            "sessions.finished",
            json!({"session": session, "agent": agent.name(), "folder": folder, "last": last}),
        );
        // FERB's own words only: the agent's reply is in the session, never in FERB's notice (rule 9)
        let ident = format!("sessions.finished:{session}:{}", (self.seams.new_id)());
        (self.seams.tell)(
            &ident,
            &format!("{} in {folder} is done and waiting for you.", agent.name()),
        );
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
        st.control
            .retain(|s, k| !(k == &key && id.is_none_or(|i| i == s)));
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
        let text = text.trim_end().to_owned();
        if text.trim().is_empty() {
            return Err(no(codes::SEND_FAILED, "There's nothing to send."));
        }
        if text.chars().count() > MAX_TEXT {
            return Err(no(codes::SEND_FAILED, "That message is too long to send."));
        }
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
        let before = self.last_seq(id);
        let started = (self.seams.mono)();
        self.deliver(&s, path, &text, "").await?;
        if let Some(e) = lock(&self.state).sessions.get_mut(id) {
            e.via_ferb = true;
            e.s.status = Status::Working;
        }
        (self.seams.journal)(
            "sessions.sent",
            json!({"session": id, "by": by.key(), "path": path.as_str(), "text": self.redact(&text)}),
        );
        let delivered = self.delivered(id, &text, before).await;
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
        if path.types() {
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
            SendPath::TerminalScript | SendPath::Foreground => {
                let mut proof = Vec::new();
                if !s.title.is_empty() {
                    proof.push(s.title.clone());
                }
                let folder = folder_name(&s.cwd);
                if !folder.is_empty() {
                    proof.push(folder);
                }
                self.seams
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
                    })
                    .await
                    .map_err(|e| no(codes::NOT_THE_SESSION, e))
            }
        }
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
    async fn delivered(&self, id: &str, text: &str, before: u64) -> bool {
        let want = one_line(&self.redact(text));
        let deadline = (self.seams.mono)() + DELIVERY_WAIT_S;
        loop {
            // a read of Codex's thread waits on its daemon: off the async thread
            let (me, i) = (self.clone(), id.to_owned());
            let _ = tokio::task::spawn_blocking(move || {
                if Agent::split(&i).map(|(a, _)| a) == Some(Agent::Codex) {
                    me.codex_refresh(&i);
                } else {
                    me.tail(&i);
                }
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
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
    }

    /// Stops the agent's turn (its own interrupt: Esc for Claude Code, `turn/interrupt` for Codex).
    pub async fn interrupt(&self, id: &str, by: &Caller) -> Result<(), Refused> {
        if !self.enabled() {
            return Err(off());
        }
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

    /// The command FERB types into a new Terminal window to start a session (HAND-665), shown exactly before the Yes.
    pub fn start_command(&self, agent: Agent, folder: &str, text: &str) -> Result<String, Refused> {
        let f = Path::new(folder);
        if !f.is_absolute() || !f.is_dir() {
            return Err(no(codes::SEND_FAILED, "That folder isn't on this Mac."));
        }
        let program = match agent {
            Agent::ClaudeCode => "claude",
            Agent::Codex => "codex",
        };
        let mut cmd = format!("cd {} && {program}", shell_quote(folder));
        if !text.trim().is_empty() {
            cmd.push(' ');
            cmd.push_str(&shell_quote(text.trim()));
        }
        Ok(cmd)
    }

    /// Starts a session in a new Terminal window, in the background (the owner said Yes to the exact command).
    pub async fn start(
        &self,
        agent: Agent,
        folder: &str,
        text: &str,
        by: &Caller,
    ) -> Result<String, Refused> {
        if !self.enabled() {
            return Err(off());
        }
        let cmd = self.start_command(agent, folder, text)?;
        let mac = self.seams.mac.clone();
        let c = cmd.clone();
        tokio::task::spawn_blocking(move || mac.terminal_open(&c))
            .await
            .map_err(|e| no(codes::SEND_FAILED, e.to_string()))?
            .map_err(|e| no(codes::SEND_FAILED, e))?;
        (self.seams.journal)(
            "sessions.started",
            json!({"agent": agent.as_str(), "folder": folder, "by": by.key(), "command": self.redact(&cmd)}),
        );
        // the new session gets control for its starter once it appears (by folder, the next looks)
        let me = self.clone();
        let (folder, by) = (folder.to_owned(), by.clone());
        let before: Vec<String> = self.list().into_iter().map(|s| s.id).collect();
        tokio::spawn(async move {
            for _ in 0..60 {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                me.look();
                if let Some(s) = me
                    .list()
                    .into_iter()
                    .find(|s| !before.contains(&s.id) && s.cwd == folder && s.agent == agent)
                {
                    if let Some(e) = lock(&me.state).sessions.get_mut(&s.id) {
                        e.s.source = Source::StartedByFerb;
                    }
                    let _ = me.take_control(&s.id, &by);
                    return;
                }
            }
        });
        Ok(cmd)
    }

    /// The owner hands a session to FERB with his goal (ORCH-687): his words become a chat message, a FERB task like
    /// any other, which reads and answers the session with its tools.
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
        let text = format!(
            "Take over my {} session{title} ({}{where_}, session {}) and keep it going until this is done: {goal}",
            s.agent.name(),
            s.cwd,
            s.id
        );
        (self.seams.hand_over)(text.clone(), client).map_err(|e| no(codes::SEND_FAILED, e))?;
        if let Some(e) = lock(&self.state).sessions.get_mut(id) {
            e.s.goal = self.redact(goal);
        }
        (self.seams.journal)("sessions.delegated", json!({"session": id}));
        self.changed();
        Ok(text)
    }

    /// A FERB task acted on a session: the session shows it is FERB's (until the task ends).
    pub fn task_touched(&self, id: &str, tree: &str) {
        if let Some(e) = lock(&self.state).sessions.get_mut(id) {
            e.s.delegate = tree.to_owned();
        }
        self.changed();
    }

    /// A task ended: the sessions it held are the owner's again.
    pub fn task_ended(&self, tree: &str) {
        let mut any = false;
        for e in lock(&self.state).sessions.values_mut() {
            if e.s.delegate == tree {
                e.s.delegate.clear();
                e.s.goal.clear();
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
        loop {
            match self.session(id) {
                None => return None,
                Some(s) if s.status != Status::Working => return Some(s),
                Some(s) if (self.seams.mono)() >= deadline => return Some(s),
                _ => {}
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    // ---------- the own question that turns sessions on ----------

    /// Once after setup, when Claude Code or Codex is on this Mac and the owner hasn't said: FERB asks (HAND-666).
    pub fn offer(&self) {
        if (self.seams.enabled)().is_some() || !(self.seams.setup_done)() {
            return;
        }
        let present =
            self.seams.home.join(".claude").is_dir() || self.seams.home.join(".codex").is_dir();
        if !present {
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
            let e = st.sessions.entry(id.clone()).or_insert_with(|| Entry {
                s: Session {
                    id: id.clone(),
                    agent: Agent::Codex,
                    agent_session: t.id.clone(),
                    source: Source::Attached,
                    pid: 0,
                    cwd: t.cwd.clone(),
                    title: String::new(),
                    status: t.status,
                    waiting: t.waiting,
                    host: None,
                    send_paths: Vec::new(),
                    no_send: String::new(),
                    last_at: now.clone(),
                    last_text: String::new(),
                    control: String::new(),
                    delegate: String::new(),
                    goal: String::new(),
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
        "Seeing your coding sessions is off. Turn it on in FERB's Settings.",
    )
}

/// The question that turns sessions on (HAND-666).
pub const ENABLE_SUMMARY: &str = "Let FERB see your Claude Code and Codex sessions?";
pub const ENABLE_DETAIL: &str = "Then you can follow and answer them from your phone or this Mac, start new ones, and hand one to me to keep going. I'd add a small hook to Claude Code's settings so I hear when a session finishes or needs you; Codex needs nothing added. What I read stays on this Mac, and you can turn this off in Settings.";
pub const ENABLE_YES: &str = "Yes, let FERB see them";
pub const ENABLE_NO: &str = "No, thanks";

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
    m.insert(
        "sessions".into(),
        Value::Array(me.list().iter().map(|s| session_json(s, by)).collect()),
    );
    m
}
