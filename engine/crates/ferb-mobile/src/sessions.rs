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
    /// HAND-683: FERB couldn't tell which terminal in its app is this session's: the ones it can see, for the owner to
    /// choose (his pick goes with `session_control`). Empty otherwise.
    pub terminals: Vec<SessionChoice>,
    /// HAND-692: the program in front in its terminal ("agy", "zsh", "claude"; "" when unknown).
    pub program: String,
    /// HAND-692: its window or tab in its app's own words ("" when FERB can't tell).
    pub window: String,
    /// HAND-693: why FERB can't show this terminal's text ("" when it can).
    pub no_text: String,
    /// HAND-702: FERB's task that keeps this session going ("" when none has started): Take it back stops it.
    pub ferb_task: String,
    /// HAND-702: FERB's latest word on the hand-over, shown in the session ("" for none).
    pub ferb_note: String,
    /// HAND-710: FERB reads none of this session's words (a terminal, an app's window): its screen is its window,
    /// live (Watch), and what the owner types goes into that window ("Type here"). Its conversation is always empty
    /// and it can't be handed to FERB.
    pub watch: bool,
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
                // HAND-692: a terminal is named by the program in front in it
                "terminal" => Some(text(&v["program"]))
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "Terminal".into()),
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
            // HAND-710: the window path brings its app to the front too
            foreground: paths
                .first()
                .is_some_and(|p| p == "foreground" || p == "window"),
            last_at: text(&v["last_at"]),
            last_text: text(&v["last_text"]),
            controlled_here: v["yours"].as_bool().unwrap_or(false),
            control,
            with_ferb: !text(&v["delegate"]).is_empty() || !text(&v["goal"]).is_empty(),
            goal: text(&v["goal"]),
            terminals: v["terminals"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|t| SessionChoice {
                            id: text(&t["id"]),
                            label: text(&t["label"]),
                        })
                        .filter(|t| !t.id.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            program: text(&v["program"]),
            window: text(&v["window"]),
            no_text: text(&v["no_text"]),
            ferb_task: text(&v["delegate"]),
            ferb_note: text(&v["ferb_note"]),
            watch: v["watch"].as_bool().unwrap_or(false),
        }
    }
}

/// The list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionsList {
    /// The owner turned sessions on (else the list is empty and the phone says how to turn them on).
    pub enabled: bool,
    pub sessions: Vec<AgentSession>,
    /// HAND-697: the chat apps open on the Mac with no chat FERB could find (their names).
    pub no_chats: Vec<String>,
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
    /// HAND-703: the reply's parts in order (an app chat's turn); empty: `text` alone, drawn as Markdown.
    pub parts: Vec<SessionPart>,
    /// HAND-709: a message FERB sent while it kept the session going (shown as FERB's, not the owner's).
    pub ferb: bool,
}

/// HAND-703: one part of a reply, as the app shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionPart {
    /// text (Markdown), thinking, tool, meta.
    pub kind: String,
    /// A text part's Markdown, a thought's or a step's words (a step's output), a meta line.
    pub text: String,
    /// A thought's or a step's row ("Thought 1s", "List zip sizes and contents").
    pub label: String,
    /// A step's second words (what it ran).
    pub detail: String,
}

impl SessionItem {
    /// Reads one item from the Mac's answer, leniently.
    pub fn read(i: &Value) -> SessionItem {
        SessionItem {
            seq: i["seq"].as_u64().unwrap_or(0),
            at: text(&i["at"]),
            kind: text(&i["kind"]),
            text: text(&i["text"]),
            tool: text(&i["tool"]),
            parts: i["parts"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|p| SessionPart {
                            kind: text(&p["kind"]),
                            text: text(&p["text"]),
                            label: text(&p["label"]),
                            detail: text(&p["detail"]),
                        })
                        .filter(|p| !p.kind.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            ferb: i["ferb"].as_bool().unwrap_or(false),
        }
    }
}

/// HAND-695: a new terminal the Mac opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionStarted {
    /// The new session's id, to open it ("" when it hadn't shown yet: it appears in the list once it does).
    pub session: String,
    /// The app it opened in ("Terminal").
    pub host: String,
    /// The command typed into it ("" for none).
    pub command: String,
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
    /// permission, question, or ferb (HAND-709: FERB's own question about its drive, "keep going?").
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

/// A new session's parameters: a terminal in `folder` with an optional `command` (HAND-691); an agent's name is the
/// form an older Mac still reads.
fn start_params(agent: &str, folder: &str, words: &str) -> Value {
    if agent.is_empty() || agent == "terminal" {
        json!({"folder": folder, "command": words})
    } else {
        json!({"agent": agent, "folder": folder, "text": words})
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
            no_chats: v["no_chats"]
                .as_array()
                .map(|a| a.iter().map(text).filter(|n| !n.is_empty()).collect())
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
                .map(|a| a.iter().map(SessionItem::read).collect())
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
    /// `terminal`: the one he chose of those the session offers (HAND-683; "" for none). Signed over the exact
    /// session, words and choice by `signer` (the fingerprint key where registered).
    pub fn session_control(
        &self,
        session: &str,
        words: &str,
        terminal: &str,
        signer: Arc<dyn Signer>,
    ) -> MobileResult<SessionSent> {
        need(session)?;
        let t = self.shared.transport_online()?;
        let mut params = json!({"session": session, "text": words});
        if !terminal.is_empty() {
            params["terminal"] = json!(terminal);
        }
        let v = Shared::call_signed(&t, "sessions.control", params, Some(&signer))?;
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

    /// The folders a new session may start in; with a folder, the exact command and its question. A new session is a
    /// terminal (HAND-691): `agent` is "terminal" (or empty) and `words` is the command to type, if any.
    pub fn session_prepare(
        &self,
        agent: &str,
        folder: &str,
        words: &str,
    ) -> MobileResult<SessionStartPlan> {
        let t = self.shared.transport_online()?;
        let mut p = json!({});
        if !folder.is_empty() {
            p = start_params(agent, folder, words);
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

    /// Opens a new terminal in `folder` on the Mac (signed), typing `words` when given (the command `session_prepare`
    /// showed); `agent` is "terminal" (or empty). Answers the new session, to open it.
    pub fn session_start(
        &self,
        agent: &str,
        folder: &str,
        words: &str,
        signer: Arc<dyn Signer>,
    ) -> MobileResult<SessionStarted> {
        let t = self.shared.transport_online()?;
        let v = Shared::call_signed(
            &t,
            "sessions.start",
            start_params(agent, folder, words),
            Some(&signer),
        )?;
        Ok(SessionStarted {
            session: text(&v["session"]),
            host: text(&v["host"]),
            command: text(&v["command"]),
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    // decision: HAND-683
    #[test]
    fn a_session_carries_the_terminals_the_owner_may_choose_from() {
        let v = json!({"id": "claude:s1", "agent": "claude_code", "send_paths": ["foreground"],
            "terminals": [{"id": "front", "label": "The terminal in front in Code now (Terminal 1, 2.1.330)"},
                          {"id": "w7:1:Terminal 2, 2.1.330", "label": "main.rs — app · Terminal 2, 2.1.330"}, {"label": "no id"}]});
        let s = AgentSession::read(&v);
        assert_eq!(
            s.terminals
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            ["front", "w7:1:Terminal 2, 2.1.330"]
        );
        assert!(AgentSession::read(&json!({"id": "claude:s2"}))
            .terminals
            .is_empty());
    }

    // decision: HAND-692, HAND-695
    #[test]
    fn a_terminal_is_named_by_its_program_and_a_new_one_carries_only_folder_and_command() {
        let s = AgentSession::read(
            &json!({"id": "term:110", "agent": "terminal", "program": "agy", "cwd": "/u/Desktop",
            "window": "Desktop — agy — 120×30", "host": {"name": "Terminal"}, "send_paths": ["terminal_script"]}),
        );
        assert_eq!(
            (
                s.agent_name.as_str(),
                s.program.as_str(),
                s.folder_name.as_str()
            ),
            ("agy", "agy", "Desktop")
        );
        assert_eq!(s.window, "Desktop — agy — 120×30");
        assert_eq!(
            AgentSession::read(&json!({"id": "term:1", "agent": "terminal"})).agent_name,
            "Terminal"
        );
        assert_eq!(
            start_params("terminal", "/p", "agy"),
            json!({"folder": "/p", "command": "agy"})
        );
        assert_eq!(
            start_params("", "/p", ""),
            json!({"folder": "/p", "command": ""})
        );
    }

    // decision: HAND-702, HAND-716
    #[test]
    fn a_hand_over_s_task_and_note_and_a_reply_s_parts_are_read() {
        let s = AgentSession::read(
            &json!({"id": "codex:t1", "agent": "codex", "delegate": "tree9",
            "goal": "chat with it", "ferb_note": "FERB is on it: “chat with it”"}),
        );
        assert_eq!((s.ferb_task.as_str(), s.with_ferb), ("tree9", true));
        assert_eq!(s.ferb_note, "FERB is on it: “chat with it”");
        let s = AgentSession::read(&json!({"id": "codex:t1", "agent": "codex",
            "ferb_note": "FERB couldn't start on it: “…”"}));
        assert_eq!((s.ferb_task.as_str(), s.with_ferb), ("", false));
        let i = SessionItem::read(
            &json!({"seq": 3, "kind": "assistant", "text": "Thought\nIt reverts PR",
            "parts": [{"kind": "thinking", "label": "Thought", "detail": "1s"},
                      {"kind": "text", "text": "It reverts PR #15."},
                      {"kind": "tool", "label": "List zip sizes", "detail": "ls", "text": "1 file"},
                      {"kind": "meta", "text": "Just now"}, {"text": "no kind"}]}),
        );
        let kinds: Vec<&str> = i.parts.iter().map(|p| p.kind.as_str()).collect();
        assert_eq!(kinds, ["thinking", "text", "tool", "meta"]);
        assert_eq!(
            (
                i.parts[2].label.as_str(),
                i.parts[2].detail.as_str(),
                i.parts[2].text.as_str()
            ),
            ("List zip sizes", "ls", "1 file")
        );
        assert!(
            SessionItem::read(&json!({"seq": 1, "kind": "user", "text": "hi"}))
                .parts
                .is_empty()
        );
    }

    // decision: HAND-709
    #[test]
    fn a_message_ferb_sent_is_read_as_ferb_s() {
        let i = SessionItem::read(
            &json!({"seq": 4, "kind": "user", "text": "run the tests", "ferb": true}),
        );
        assert!(i.ferb);
        assert!(!SessionItem::read(&json!({"seq": 5, "kind": "user", "text": "hi"})).ferb);
    }
}
