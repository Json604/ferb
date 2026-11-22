//! Codex, through its shared app-server daemon (the agent-remote lab's S0-5): JSON-RPC over a WebSocket on the
//! Unix socket `~/.codex/app-server-control/app-server-control.sock`. The terminal TUI and the desktop app run their
//! threads there; a second client (FERB) lists them, reads them, starts and interrupts turns, and hears their status
//! live. Nothing is written into Codex's settings: its own daemon is the signal (kb HAND-657).
//!
//! Observed (codex-cli 0.169/0.170): `thread/loaded/list`, `thread/read`, `thread/resume`, `turn/start` (shown live in
//! the TUI), `thread/status/changed`, `turn/started`, `turn/completed`, `item/started`, `item/completed`, `error`,
//! `thread/started`, `thread/closed`. From the schema only (no turn ran during the lab: usage limit): `turn/steer`,
//! `turn/interrupt`, the approval requests and their `decision` answers, `agentMessage`'s `text`.

use serde_json::{json, Value};

use crate::claude::Draft;
use crate::model::{Card, CardKind, Choice, ItemKind, Status, Waiting};
use crate::{clip, one_line};

/// The socket, relative to the owner's home.
pub const SOCKET: &str = ".codex/app-server-control/app-server-control.sock";

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

// ---------- what FERB sends ----------

/// A request (`id` given by the caller).
pub fn request(id: u64, method: &str, params: Value) -> String {
    json!({"id": id, "method": method, "params": params}).to_string()
}

/// The first request: FERB names itself.
pub fn initialize(id: u64, version: &str) -> String {
    request(
        id,
        "initialize",
        json!({"clientInfo": {"name": "ferb", "version": version}, "capabilities": {"experimentalApi": true}}),
    )
}

/// The notification that follows the initialize answer.
pub fn initialized() -> String {
    json!({"method": "initialized"}).to_string()
}

/// The text input of a turn.
pub fn text_input(text: &str) -> Value {
    json!([{"type": "text", "text": text}])
}

/// The answer to one of the daemon's own requests (an approval).
pub fn respond(id: &Value, result: Value) -> String {
    json!({"id": id, "result": result}).to_string()
}

// ---------- what Codex says ----------

/// A thread's facts.
#[derive(Clone, Debug, PartialEq)]
pub struct Thread {
    pub id: String,
    pub cwd: String,
    pub title: String,
    pub status: Status,
    pub waiting: Waiting,
    /// Codex's own short-lived threads (titles): never listed.
    pub ephemeral: bool,
    /// When it last changed (seconds since 1970).
    pub updated: i64,
    pub items: Vec<Draft>,
}

/// A thread status object (`{"type": "active", "activeFlags": [...]}`) as FERB's state.
pub fn status_of(v: &Value) -> (Status, Waiting) {
    let flags: Vec<&str> = v
        .get("activeFlags")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    match s(v, "type") {
        "active" => {
            if flags
                .iter()
                .any(|f| f.to_ascii_lowercase().contains("approval"))
            {
                (Status::WaitingForYou, Waiting::Permission)
            } else if flags
                .iter()
                .any(|f| f.to_ascii_lowercase().contains("input"))
            {
                (Status::WaitingForYou, Waiting::Question)
            } else {
                (Status::Working, Waiting::None)
            }
        }
        "notLoaded" => (Status::Ended, Waiting::None),
        _ => (Status::Idle, Waiting::None),
    }
}

/// A `thread` object (from `thread/read`, `thread/started`, `thread/resume`).
pub fn thread(v: &Value) -> Option<Thread> {
    let id = s(v, "id").to_owned();
    if id.is_empty() {
        return None;
    }
    let (status, waiting) = status_of(v.get("status").unwrap_or(&Value::Null));
    let title = [s(v, "name"), s(v, "preview")]
        .into_iter()
        .map(one_line)
        .find(|t| !t.is_empty())
        .map(|t| clip(&t, 120))
        .unwrap_or_default();
    let mut items = Vec::new();
    for turn in v
        .get("turns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for it in turn
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(d) = item(it, "") {
                items.push(d);
            }
        }
    }
    Some(Thread {
        id,
        cwd: s(v, "cwd").to_owned(),
        title,
        status,
        waiting,
        ephemeral: v.get("ephemeral").and_then(Value::as_bool) == Some(true),
        updated: v.get("updatedAt").and_then(Value::as_i64).unwrap_or(0),
        items,
    })
}

/// One thread item as a timeline item (reasoning is never shown; unknown kinds are skipped).
pub fn item(v: &Value, at: &str) -> Option<Draft> {
    let at = if at.is_empty() {
        v.get("completedAtMs")
            .or_else(|| v.get("startedAtMs"))
            .and_then(Value::as_i64)
            .map(iso_ms)
            .unwrap_or_default()
    } else {
        at.to_owned()
    };
    let d = |kind, text: String, tool: &str| Draft {
        at: at.clone(),
        kind,
        text,
        tool: tool.to_owned(),
    };
    match s(v, "type") {
        "userMessage" => {
            let text = v
                .get("content")
                .and_then(Value::as_array)
                .map(|c| {
                    c.iter()
                        .filter(|b| s(b, "type") == "text")
                        .map(|b| s(b, "text"))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            (!text.trim().is_empty()).then(|| d(ItemKind::User, text.trim().to_owned(), ""))
        }
        "agentMessage" => {
            let t = s(v, "text").trim().to_owned();
            (!t.is_empty()).then(|| d(ItemKind::Assistant, t, ""))
        }
        "commandExecution" => {
            let cmd = match v.get("command") {
                Some(Value::String(c)) => c.clone(),
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => String::new(),
            };
            Some(d(ItemKind::Tool, clip(&one_line(&cmd), 160), "Shell"))
        }
        "fileChange" => {
            let paths: Vec<&str> = v
                .get("changes")
                .and_then(Value::as_array)
                .map(|c| {
                    c.iter()
                        .map(|x| s(x, "path"))
                        .filter(|p| !p.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            Some(d(ItemKind::Tool, clip(&paths.join(", "), 160), "Edit"))
        }
        "mcpToolCall" => Some(d(
            ItemKind::Tool,
            String::new(),
            &format!("{}.{}", s(v, "server"), s(v, "tool")),
        )),
        "webSearch" => Some(d(
            ItemKind::Tool,
            clip(&one_line(s(v, "query")), 160),
            "Web search",
        )),
        _ => None,
    }
}

/// What one message from the daemon means for FERB.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The answer to FERB's request `id`.
    Answer {
        id: u64,
        result: Result<Value, String>,
    },
    Status {
        thread: String,
        status: Status,
        waiting: Waiting,
    },
    TurnStarted {
        thread: String,
        turn: String,
    },
    TurnDone {
        thread: String,
        error: String,
    },
    Item {
        thread: String,
        draft: Draft,
    },
    Started(Thread),
    Closed {
        thread: String,
    },
    Error {
        thread: String,
        text: String,
    },
    /// One of the daemon's own requests that waits on an answer (an approval).
    Asks {
        id: Value,
        thread: String,
        card: Card,
    },
    Other,
}

/// One message (a text frame's JSON).
pub fn event(text: &str) -> Event {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Event::Other;
    };
    let method = s(&v, "method");
    let p = v.get("params").cloned().unwrap_or(Value::Null);
    let thread = s(&p, "threadId").to_owned();
    if method.is_empty() {
        let Some(id) = v.get("id").and_then(Value::as_u64) else {
            return Event::Other;
        };
        let result = match v.get("error") {
            Some(e) => Err(s(e, "message").to_owned()),
            None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
        };
        return Event::Answer { id, result };
    }
    if let Some(id) = v.get("id").filter(|i| !i.is_null()) {
        // a request from the daemon: only approvals are understood; FERB leaves any other to the app that runs it
        let card_kind = match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                CardKind::Permission
            }
            _ => return Event::Other,
        };
        let detail = match p.get("command") {
            Some(Value::String(c)) => c.clone(),
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
            _ => s(&p, "reason").to_owned(),
        };
        let tool = if method.contains("fileChange") {
            "Edit"
        } else {
            "Shell"
        };
        let card = Card {
            id: String::new(),
            session: String::new(),
            kind: card_kind,
            tool: tool.into(),
            detail,
            question: String::new(),
            choices: vec![
                Choice {
                    id: "allow_once".into(),
                    label: "Allow".into(),
                },
                Choice {
                    id: "allow_for_session".into(),
                    label: "Allow for this session".into(),
                },
                Choice {
                    id: "deny".into(),
                    label: "Deny".into(),
                },
            ],
            asked_at: String::new(),
            approval_id: String::new(),
        };
        return Event::Asks {
            id: id.clone(),
            thread,
            card,
        };
    }
    match method {
        "thread/status/changed" => {
            let (status, waiting) = status_of(p.get("status").unwrap_or(&Value::Null));
            Event::Status {
                thread,
                status,
                waiting,
            }
        }
        "turn/started" => Event::TurnStarted {
            thread,
            turn: p
                .get("turn")
                .map(|t| s(t, "id").to_owned())
                .unwrap_or_default(),
        },
        "turn/completed" => Event::TurnDone {
            thread,
            error: p
                .get("turn")
                .and_then(|t| t.get("error"))
                .map(|e| s(e, "message").to_owned())
                .unwrap_or_default(),
        },
        "item/completed" => match p.get("item").and_then(|i| item(i, "")) {
            Some(draft) => Event::Item { thread, draft },
            None => Event::Other,
        },
        "thread/started" => match p.get("thread").and_then(thread_of) {
            Some(t) => Event::Started(t),
            None => Event::Other,
        },
        "thread/closed" => Event::Closed { thread },
        "error" => Event::Error {
            thread,
            text: p
                .get("error")
                .map(|e| s(e, "message").to_owned())
                .unwrap_or_default(),
        },
        _ => Event::Other,
    }
}

fn thread_of(v: &Value) -> Option<Thread> {
    thread(v)
}

/// The answer to an approval request: Codex's own decision words (schema; T-2 in the lab).
pub fn approval_answer(choice: &str) -> Option<Value> {
    let decision = match choice {
        "allow_once" => "accept",
        "allow_for_session" => "acceptForSession",
        "deny" => "decline",
        _ => return None,
    };
    Some(json!({"decision": decision}))
}

/// Milliseconds since 1970 as ISO 8601 (UTC, millisecond precision).
pub fn iso_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let milli = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    let (y, m, d) = civil(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// Days since 1970-01-01 as (year, month, day) (Howard Hinnant's civil_from_days).
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
