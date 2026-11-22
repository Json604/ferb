//! The session model every client reads (FERB.app, the phones): what the daemon's `sessions.list` and
//! `sessions.timeline` answer with. Field names are the wire's.

use serde::{Deserialize, Serialize};

/// Which coding agent runs the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    ClaudeCode,
    Codex,
}

impl Agent {
    pub fn as_str(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude_code",
            Agent::Codex => "codex",
        }
    }
    pub fn parse(s: &str) -> Option<Agent> {
        match s {
            "claude_code" | "claude" => Some(Agent::ClaudeCode),
            "codex" => Some(Agent::Codex),
            _ => None,
        }
    }
    /// The agent's name as the owner reads it.
    pub fn name(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "Claude Code",
            Agent::Codex => "Codex",
        }
    }
    /// The id prefix of its sessions (`claude:<sessionId>`, `codex:<threadId>`).
    pub fn prefix(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude",
            Agent::Codex => "codex",
        }
    }
    /// The session id FERB uses for the agent's own id.
    pub fn session_id(self, own: &str) -> String {
        format!("{}:{own}", self.prefix())
    }
    /// The agent and its own id, from FERB's session id.
    pub fn split(id: &str) -> Option<(Agent, &str)> {
        let (p, own) = id.split_once(':')?;
        let a = match p {
            "claude" => Agent::ClaudeCode,
            "codex" => Agent::Codex,
            _ => return None,
        };
        (!own.is_empty()).then_some((a, own))
    }
}

/// Who started the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The owner started it himself; FERB found it.
    Attached,
    /// FERB started it on the owner's word (`sessions.start`).
    StartedByFerb,
}

/// What the session is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Working,
    WaitingForYou,
    Idle,
    Ended,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Working => "working",
            Status::WaitingForYou => "waiting_for_you",
            Status::Idle => "idle",
            Status::Ended => "ended",
        }
    }
}

/// Why it waits for the owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Waiting {
    #[default]
    None,
    /// The agent asks to run something (a permission prompt).
    Permission,
    /// The agent asks the owner a question with options.
    Question,
}

/// How the agent reaches the app that shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    /// The agent runs in the app's own terminal (any terminal app, an IDE's terminal).
    Direct,
    /// Through tmux: the pane's client is in the app.
    Tmux,
    /// The app is the agent's own (a desktop app talking to the agent's daemon).
    App,
}

/// The app that shows the session (HAND-654): a fact from the process tree, never configured per app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Host {
    /// The app's bundle id ("" when unknown).
    pub bundle_id: String,
    /// The app's name as the owner reads it ("Terminal", "Visual Studio Code").
    pub name: String,
    /// The app's main process.
    pub pid: i32,
    pub via: Option<Via>,
    /// The terminal the agent (or its tmux client) reads from, e.g. "/dev/ttys004".
    pub tty: String,
}

/// One way FERB can deliver the owner's text (HAND-656), in the order they are tried.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendPath {
    /// The agent's own interface (Codex's app-server): background and exact.
    AgentApi,
    /// tmux's own input to the pane: background and exact.
    Tmux,
    /// Terminal's scripting (`do script` in the session's tab): background.
    TerminalScript,
    /// The host app brought forward and typed into, once code has proven the focused terminal is the session's.
    Foreground,
}

impl SendPath {
    pub fn as_str(self) -> &'static str {
        match self {
            SendPath::AgentApi => "agent_api",
            SendPath::Tmux => "tmux",
            SendPath::TerminalScript => "terminal_script",
            SendPath::Foreground => "foreground",
        }
    }
    /// Whether this path types into a terminal or a window (AGENTS.md rule 6: asked first).
    pub fn types(self) -> bool {
        !matches!(self, SendPath::AgentApi)
    }
    /// Whether it takes the foreground (rule 10: the owner's Yes covers it).
    pub fn foreground(self) -> bool {
        matches!(self, SendPath::Foreground)
    }
}

/// One session as clients see it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// `claude:<sessionId>` / `codex:<threadId>`.
    pub id: String,
    pub agent: Agent,
    /// The agent's own id.
    pub agent_session: String,
    pub source: Source,
    /// The agent's process (0 when FERB doesn't know it, e.g. a Codex thread with no terminal).
    pub pid: i32,
    /// The project folder.
    pub cwd: String,
    /// The agent's own title for it ("" before it has one).
    pub title: String,
    pub status: Status,
    pub waiting: Waiting,
    pub host: Option<Host>,
    /// What FERB would try, in order; empty when nothing can deliver (the clients say why).
    pub send_paths: Vec<SendPath>,
    /// When nothing can deliver: why, in the owner's words.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub no_send: String,
    /// The last time something happened (ISO 8601).
    pub last_at: String,
    /// The agent's last words, one line, at most 200 characters, redacted.
    pub last_text: String,
    /// Who controls it now: "mac", "phone:<device>" ("" for no one).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub control: String,
    /// The FERB task the owner handed it to ("" for none).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub delegate: String,
    /// The owner's goal for that task.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub goal: String,
}

/// What a timeline item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// The owner's message (typed at the Mac, or sent through FERB).
    User,
    /// The agent's text.
    Assistant,
    /// One tool call, one line ("Bash: npm test").
    Tool,
    /// The owner stopped the agent's turn.
    Interrupted,
    /// The agent's error.
    Error,
}

/// One timeline item, built by code from the agent's own record (raw, HAND-661); thinking is never shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// The session's own order (1, 2, …).
    pub seq: u64,
    /// ISO 8601 ("" when the record has no time).
    pub at: String,
    pub kind: ItemKind,
    /// The words (redacted by the daemon before they leave the Mac).
    pub text: String,
    /// For a tool: its name ("Bash", "Edit").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool: String,
}

/// What a card asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    /// The agent asks to run something.
    Permission,
    /// The agent asks the owner a question with options.
    Question,
}

/// One answer a card offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    /// What the answer sends ("allow_once", "allow_for_session", "deny", or an option's label).
    pub id: String,
    /// The button's words.
    pub label: String,
}

/// The agent waits for the owner: a permission prompt or a question, built by code from the agent's own request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    pub session: String,
    pub kind: CardKind,
    /// For a permission: the tool's name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool: String,
    /// What it would run or change, exactly as the agent asked (redacted).
    pub detail: String,
    /// For a question: the question itself.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub question: String,
    pub choices: Vec<Choice>,
    pub asked_at: String,
    /// The FERB question that carries it (the gateway's approval id; "" before it is asked).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub approval_id: String,
}
