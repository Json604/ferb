//! The owner's coding-agent sessions on his Mac (kb HAND-651..666), as the phone reads and drives them: the list, one
//! session's conversation (raw, built by the Mac from the agent's own record, redacted there), the agent's questions,
//! sending his words, interrupting, starting one, handing one to FERB, watching its window.
//!
//! Classes (ferb-rpc `classes`): reads are private (the unlock window); the first send to a session asks the owner
//! once with the exact text and his Yes is `sessions.control`, signed by the fingerprint key (one per session per
//! control, GATE-622, GATE-622); later sends and interrupts are private while this phone controls it; answers to the agent,
//! starts and hand-overs are signed each time. Letting go works while locked. The window's picture is a live preview
//! (PHN-454): direct path or the owner's relay only, memory only. Nothing here is stored on the phone.

use std::sync::Arc;

use base64::Engine;
use serde::Serialize;
use serde_json::{json, Value};

use crate::api::{parse, ErrorCode, Mobile, MobileError, MobileResult, Shared, Signer};
use crate::remote::PreviewFrame;
use crate::transport::TransferSink;

fn text(v: &Value) -> String {
    v.as_str().map(str::to_string).unwrap_or_default()
}

fn folder_name(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_string()
}

/// One session in the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentSession {
    /// "claude:<id>" / "codex:<id>".
    pub id: String,
    /// "claude_code" or "codex".
    pub agent: String,
    /// "Claude Code" or "Codex".
    pub agent_name: String,
    /// The agent's own title ("" before it has one).
    pub title: String,
    /// The project folder, and its last part.
    pub folder: String,
    pub folder_name: String,
    /// working, waiting_for_you, idle.
    pub status: String,
    /// none, permission, question.
    pub waiting: String,
    /// The app that shows it ("Terminal", "Visual Studio Code"; "" when none does).
    pub host: String,
    /// Something can deliver the owner's words to it.
    pub can_send: bool,
    /// Why nothing can ("" when something can).
    pub no_send: String,
    /// Sending brings its app to the front on the Mac.
    pub foreground: bool,
    /// When something last happened (ISO 8601).
    pub last_at: String,
    /// The agent's last words, one line.
    pub last_text: String,
    /// This phone controls it now (its sends go without asking).
    pub controlled_here: bool,
    /// Someone controls it ("mac", "phone:…"; "" for no one).
    pub control: String,
    /// FERB is keeping it going for the owner.
    pub with_ferb: bool,
    /// The owner's goal for FERB ("" for none).
    pub goal: String,
}

impl AgentSession {
    /// Reads one session from the Mac's answer, leniently (`yours`: the Mac says this phone controls it).
    pub fn read(v: &Value) -> AgentSession {
        let agent = text(&v["agent"]);
        let folder = text(&v["cwd"]);
        let control = text(&v["control"]);
        let paths: Vec<String> = v["send_paths"]
            .as_array()
            .map(|a| a.iter().map(text).collect())
            .unwrap_or_default();
        AgentSession {
            id: text(&v["id"]),
            agent_name: match agent.as_str() {
                "codex" => "Codex".into(),
                // HAND-668: an app chat is named by its app
                "app_chat" => Some(text(&v["host"]["name"]))
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "App chat".into()),
                _ => "Claude Code".into(),
            },
            agent,
            title: text(&v["title"]),
            folder_name: folder_name(&folder),
            folder,
            status: text(&v["status"]),
            waiting: text(&v["waiting"]),
            host: text(&v["host"]["name"]),
            can_send: !paths.is_empty(),
            no_send: text(&v["no_send"]),
            foreground: paths.first().is_some_and(|p| p == "foreground"),
            last_at: text(&v["last_at"]),
            last_text: text(&v["last_text"]),
            controlled_here: v["yours"].as_bool().unwrap_or(false),
            control,
            with_ferb: !text(&v["delegate"]).is_empty() || !text(&v["goal"]).is_empty(),
            goal: text(&v["goal"]),
        }
    }
}

/// The list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionsList {
    /// The owner turned sessions on (else the list is empty and the phone says how to turn them on).
    pub enabled: bool,
    pub sessions: Vec<AgentSession>,
}

/// One timeline item (thinking is never sent; a tool call is one line).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionItem {
    pub seq: u64,
    pub at: String,
    /// user, assistant, tool, interrupted, error.
    pub kind: String,
    pub text: String,
    /// For a tool: its name.
    pub tool: String,
}

/// One answer a card offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionChoice {
    pub id: String,
    pub label: String,
}

/// The agent waits for the owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionCard {
    pub id: String,
    /// permission or question.
    pub kind: String,
    pub tool: String,
    /// What it would run or change, exactly.
    pub detail: String,
    pub question: String,
    /// Empty: answered on the Mac only.
    pub choices: Vec<SessionChoice>,
}

/// One page of a session's conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionTimeline {
    pub session: Option<AgentSession>,
    pub items: Vec<SessionItem>,
    /// More before (a first page) or after (a later one).
    pub more: bool,
    pub cards: Vec<SessionCard>,
}

/// What the owner is asked before FERB types for him (rule 6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionYes {
    pub question: String,
    /// The exact text (or command).
    pub detail: String,
    /// What the Yes covers.
    pub covers: String,
}

/// What a send did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionSent {
    /// Asked first: nothing was sent; the app shows it and calls `session_control` on the owner's Yes.
    pub needs_yes: Option<SessionYes>,
    pub sent: bool,
    /// The words were seen in the agent's own record.
    pub delivered: bool,
    /// agent_api, tmux, terminal_script, foreground.
    pub path: String,
}

/// What a new session would be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionStartPlan {
    /// Folders to choose from (the running sessions', then the recent ones).
    pub folders: Vec<String>,
    /// For a chosen folder: the exact command and its question.
    pub needs_yes: Option<SessionYes>,
}

fn yes_of(v: &Value) -> Option<SessionYes> {
    v.is_object().then(|| SessionYes {
        question: text(&v["question"]),
        detail: text(&v["detail"]),
        covers: text(&v["covers"]),
    })
}

fn sent_of(v: &Value) -> SessionSent {
    SessionSent {
        needs_yes: yes_of(&v["needs_yes"]),
        sent: v["sent"].as_bool().unwrap_or(false),
        delivered: v["delivered"].as_bool().unwrap_or(false),
        path: text(&v["path"]),
    }
}

fn need(id: &str) -> MobileResult<()> {
    if id.trim().is_empty() {
        return Err(MobileError::new(ErrorCode::Refused, "Which session?"));
    }
    Ok(())
}

impl Mobile {
    /// The owner's coding sessions on the Mac (`sessions.list`; needs the unlock).
    pub fn sessions_list(&self) -> MobileResult<SessionsList> {
        let t = self.shared.transport_online()?;
        let v = Shared::call(&t, "sessions.list", json!({}))?;
        Ok(SessionsList {
            enabled: v["enabled"].as_bool().unwrap_or(false),
            sessions: v["sessions"]
                .as_array()
                .map(|a| a.iter().map(AgentSession::read).collect())
                .unwrap_or_default(),
        })
    }

    /// One session's conversation: the newest `limit` (after 0), those after `after`, or those before `before`.
    pub fn session_timeline(
        &self,
        session: &str,
        after: u64,
        before: u64,
        limit: u32,
    ) -> MobileResult<SessionTimeline> {
        need(session)?;
        let t = self.shared.transport_online()?;
        let mut p = json!({"session": session, "limit": limit.clamp(1, 100)});
        if after > 0 {
            p["after"] = json!(after);
        }
        if before > 0 {
            p["before"] = json!(before);
        }
        let v = Shared::call(&t, "sessions.timeline", p)?;
        Ok(SessionTimeline {
            session: v["session"]
                .is_object()
                .then(|| AgentSession::read(&v["session"])),
            items: v["items"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|i| SessionItem {
                            seq: i["seq"].as_u64().unwrap_or(0),
                            at: text(&i["at"]),
                            kind: text(&i["kind"]),
                            text: text(&i["text"]),
                            tool: text(&i["tool"]),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            more: v["more"].as_bool().unwrap_or(false),
            cards: v["cards"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|c| SessionCard {
                            id: text(&c["id"]),
                            kind: text(&c["kind"]),
                            tool: text(&c["tool"]),
                            detail: text(&c["detail"]),
                            question: text(&c["question"]),
                            choices: c["choices"]
                                .as_array()
                                .map(|o| {
                                    o.iter()
                                        .map(|x| SessionChoice {
                                            id: text(&x["id"]),
                                            label: text(&x["label"]),
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// The owner's words into a session (`sessions.send`): sent when this phone controls it, else `needs_yes` (nothing
    /// sent) for the app to ask.
    pub fn session_send(&self, session: &str, words: &str) -> MobileResult<SessionSent> {
        need(session)?;
        let t = self.shared.transport_online()?;
        let v = Shared::call(
            &t,
            "sessions.send",
            json!({"session": session, "text": words}),
        )?;
        Ok(sent_of(&v))
    }

    /// The owner's Yes (GATE-622): this phone controls the session from now on, and `words` (may be empty) are sent.
    /// Signed over the exact session and words by `signer` (the fingerprint key where registered).
    pub fn session_control(
        &self,
        session: &str,
        words: &str,
        signer: Arc<dyn Signer>,
    ) -> MobileResult<SessionSent> {
        need(session)?;
        let t = self.shared.transport_online()?;
        let v = Shared::call_signed(
            &t,
            "sessions.control",
            json!({"session": session, "text": words}),
            Some(&signer),
        )?;
        Ok(sent_of(&v))
    }

    /// Lets go of a session (allowed while locked: it only takes something away).
    pub fn session_release(&self, session: &str) -> MobileResult<u32> {
        let t = self.shared.transport_online()?;
        let v = Shared::call(&t, "sessions.release", json!({"session": session}))?;
        Ok(v["released"].as_u64().unwrap_or(0) as u32)
    }

    /// Answers the agent's permission prompt or question (signed each time, HAND-663).
    pub fn session_answer(
        &self,
        session: &str,
        card: &str,
        choice: &str,
        signer: Arc<dyn Signer>,
    ) -> MobileResult<bool> {
        need(session)?;
        let t = self.shared.transport_online()?;
        let v = Shared::call_signed(
            &t,
            "sessions.answer",
            json!({"session": session, "card": card, "choice": choice}),
            Some(&signer),
        )?;
        Ok(v["answered"].as_bool().unwrap_or(false))
    }

    /// Stops the agent's turn (this phone must control the session).
    pub fn session_interrupt(&self, session: &str) -> MobileResult<bool> {
        need(session)?;
        let t = self.shared.transport_online()?;
        let v = Shared::call(&t, "sessions.interrupt", json!({"session": session}))?;
        Ok(v["interrupted"].as_bool().unwrap_or(false))
    }

    /// The folders a new session may start in; with a folder, the exact command and its question.
    pub fn session_prepare(
        &self,
        agent: &str,
        folder: &str,
        words: &str,
    ) -> MobileResult<SessionStartPlan> {
        let t = self.shared.transport_online()?;
        let mut p = json!({});
        if !folder.is_empty() {
            p = json!({"agent": agent, "folder": folder, "text": words});
        }
        let v = Shared::call(&t, "sessions.prepare", p)?;
        Ok(SessionStartPlan {
            folders: v["folders"]
                .as_array()
                .map(|a| a.iter().map(text).filter(|s| !s.is_empty()).collect())
                .unwrap_or_default(),
            needs_yes: yes_of(&v["needs_yes"]),
        })
    }

    /// Starts a session in a new Terminal window (signed: it types the command shown by `session_prepare`).
    pub fn session_start(
        &self,
        agent: &str,
        folder: &str,
        words: &str,
        signer: Arc<dyn Signer>,
    ) -> MobileResult<String> {
        let t = self.shared.transport_online()?;
        let v = Shared::call_signed(
            &t,
            "sessions.start",
            json!({"agent": agent, "folder": folder, "text": words}),
            Some(&signer),
        )?;
        Ok(text(&v["command"]))
    }

    /// Hands a session to FERB with the owner's goal (signed): his words become a FERB task in the chat.
    pub fn session_delegate(
        &self,
        session: &str,
        goal: &str,
        signer: Arc<dyn Signer>,
    ) -> MobileResult<String> {
        need(session)?;
        let t = self.shared.transport_online()?;
        let v = Shared::call_signed(
            &t,
            "sessions.delegate",
            json!({"session": session, "goal": goal}),
            Some(&signer),
        )?;
        Ok(text(&v["message"]))
    }

    /// One answer of a session's window as a live picture (`tasks.preview` with `session`, through `link.get what:
    /// preview`): the same rules as Watch live (direct path or own relay only, in memory only).
    pub fn session_preview(&self, session: &str, have: Option<i64>) -> MobileResult<PreviewFrame> {
        need(session)?;
        let t = self.shared.transport_bulk("Watching a session")?;
        let mut params =
            json!({"what": "preview", "live": true, "task_id": "", "session": session});
        if let Some(h) = have {
            params["have"] = json!(h);
        }
        struct Quiet;
        impl TransferSink for Quiet {
            fn progress(&self, _done: u64, _of: u64) {}
        }
        let id = format!("preview-{}", self.shared.env.ids.new_id());
        let reply = t
            .get(&id, &params.to_string(), None, false, Arc::new(Quiet))
            .map_err(MobileError::from_link)?;
        let mut v = parse(&reply)?;
        let jpeg = match v.get_mut("data").map(Value::take) {
            Some(Value::String(d)) if !d.is_empty() => base64::engine::general_purpose::STANDARD
                .decode(d.as_bytes())
                .ok()
                .filter(|b| !b.is_empty()),
            _ => None,
        };
        let meta = &v["meta"];
        Ok(PreviewFrame {
            jpeg,
            seq: meta["seq"].as_i64(),
            state: text(&meta["state"]),
            line: meta["line"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            fresh: meta["fresh"].as_bool().unwrap_or(false),
            busy: meta["busy"].as_bool().unwrap_or(false),
            gone: meta["state"] == "none" && meta["seq"].is_null(),
            empty_text: meta["problem"]["text"]
                .as_str()
                .or_else(|| meta["line"].as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        })
    }
}
