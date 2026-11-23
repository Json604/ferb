//! Claude Code, read through its own records (the agent-remote lab's S0-1, S0-3; kb HAND-661..7):
//!
//! - The transcript `~/.claude/projects/<folder>/<sessionId>.jsonl`, appended live one finished block at a time. Read
//!   by an **allowlist**: the owner's messages, the agent's text, one line per tool call, an interrupt, an API error,
//!   the session's title. Everything else is skipped without error (new record kinds appear every few versions).
//!   `api-request`, `api-request-blob` and `api-request-shape` hold the whole model request (the system context
//!   names the owner's account): they are never read into anything.
//! - The hooks FERB installs (`hooks`), which arrive 0.1-0.2 s before the transcript: the clock for states and cards.
//! - `claude agents --json`: the running interactive sessions (pid, sessionId, cwd, name, busy or idle).

use serde_json::{json, Map, Value};

use crate::model::{Card, CardKind, Choice, ItemKind};
use crate::{clip, one_line};

/// How long a tool line may be.
pub const TOOL_LINE: usize = 160;

/// An item read from one transcript line, before the daemon numbers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub at: String,
    pub kind: ItemKind,
    pub text: String,
    pub tool: String,
}

/// What one transcript line holds for the timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    Items(Vec<Draft>),
    /// The session's title: the agent's own (`ai-title`) or the owner's (`custom-title`, which wins).
    Title {
        text: String,
        owners: bool,
    },
    Skip,
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// Record types never read (they hold the whole model request).
pub const NEVER_READ: [&str; 3] = ["api-request", "api-request-blob", "api-request-shape"];

/// One line of a transcript.
pub fn parse_line(line: &str) -> Line {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
        return Line::Skip;
    };
    let kind = s(&v, "type");
    if NEVER_READ.contains(&kind) {
        return Line::Skip;
    }
    match kind {
        "user" | "assistant" => {}
        "ai-title" | "custom-title" | "summary" => {
            return title_of(&v, kind == "custom-title");
        }
        _ => return Line::Skip,
    }
    // a subagent's own turns and Claude Code's own notes are not the conversation
    if v.get("isSidechain").and_then(Value::as_bool) == Some(true)
        || v.get("isMeta").and_then(Value::as_bool) == Some(true)
    {
        return Line::Skip;
    }
    let at = s(&v, "timestamp").to_owned();
    let content = v.get("message").and_then(|m| m.get("content"));
    let mut out = Vec::new();
    if kind == "user" {
        let mut words: Vec<String> = Vec::new();
        match content {
            Some(Value::String(t)) => words.push(t.clone()),
            Some(Value::Array(blocks)) => {
                for b in blocks {
                    if s(b, "type") == "text" {
                        words.push(s(b, "text").to_owned());
                    }
                }
            }
            _ => {}
        }
        for w in words {
            if let Some(d) = user_words(&w, &at) {
                out.push(d);
            }
        }
    } else {
        let api_error = v.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true);
        if let Some(Value::Array(blocks)) = content {
            for b in blocks {
                match s(b, "type") {
                    "text" => {
                        let t = s(b, "text").trim();
                        if !t.is_empty() {
                            out.push(Draft {
                                at: at.clone(),
                                kind: if api_error {
                                    ItemKind::Error
                                } else {
                                    ItemKind::Assistant
                                },
                                text: t.to_owned(),
                                tool: String::new(),
                            });
                        }
                    }
                    "tool_use" => {
                        let name = s(b, "name").to_owned();
                        out.push(Draft {
                            at: at.clone(),
                            kind: ItemKind::Tool,
                            text: tool_line(b.get("input").unwrap_or(&Value::Null)),
                            tool: name,
                        });
                    }
                    // thinking is never shown; other blocks are skipped
                    _ => {}
                }
            }
        }
    }
    if out.is_empty() {
        Line::Skip
    } else {
        Line::Items(out)
    }
}

/// The owner's words in a user record: a typed message, an interrupt, a slash command's name. Claude Code's own
/// notes in the owner's turn (command output, reminders) are skipped.
fn user_words(t: &str, at: &str) -> Option<Draft> {
    let t = t.trim();
    if t.is_empty() {
        return None;
    }
    if t.starts_with("[Request interrupted by user") {
        return Some(Draft {
            at: at.to_owned(),
            kind: ItemKind::Interrupted,
            text: String::new(),
            tool: String::new(),
        });
    }
    if let Some(rest) = t.strip_prefix("<command-name>") {
        let name = rest.split("</command-name>").next().unwrap_or("").trim();
        return (!name.is_empty()).then(|| Draft {
            at: at.to_owned(),
            kind: ItemKind::User,
            text: name.to_owned(),
            tool: String::new(),
        });
    }
    if t.starts_with("<local-command")
        || t.starts_with("<system-reminder")
        || t.starts_with("<command-message")
        || t.starts_with("Caveat:")
    {
        return None;
    }
    Some(Draft {
        at: at.to_owned(),
        kind: ItemKind::User,
        text: t.to_owned(),
        tool: String::new(),
    })
}

fn title_of(v: &Value, owners: bool) -> Line {
    let Some(obj) = v.as_object() else {
        return Line::Skip;
    };
    for (k, val) in obj {
        let key = k.to_ascii_lowercase();
        if (key.contains("title") || key == "summary") && key != "type" {
            if let Some(t) = val.as_str() {
                let t = one_line(t);
                if !t.is_empty() {
                    return Line::Title {
                        text: clip(&t, 120),
                        owners,
                    };
                }
            }
        }
    }
    Line::Skip
}

/// The fields a tool's input names what it acts on by, in order: the first one present is its line.
const NAMED_BY: [&str; 11] = [
    "command",
    "file_path",
    "notebook_path",
    "path",
    "pattern",
    "url",
    "query",
    "description",
    "prompt",
    "skill",
    "question",
];

/// One line for a tool call: what it acts on, from its input's own fields (structure only).
pub fn tool_line(input: &Value) -> String {
    for k in NAMED_BY {
        if let Some(t) = input.get(k).and_then(Value::as_str) {
            let t = one_line(t);
            if !t.is_empty() {
                return clip(&t, TOOL_LINE);
            }
        }
    }
    // a question tool: its first question
    if let Some(q) = input
        .get("questions")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
    {
        return clip(&one_line(s(q, "question")), TOOL_LINE);
    }
    String::new()
}

// ---------- hooks ----------

/// The hook events FERB installs (all command hooks to `ferb agent-hook claude`).
pub const HOOK_EVENTS: [&str; 7] = [
    "SessionStart",
    "UserPromptSubmit",
    "PermissionRequest",
    "PostToolUse",
    "Notification",
    "Stop",
    "SessionEnd",
];

/// The one event FERB holds until an answer (HAND-662): its dialog stays up on the Mac meanwhile, and the first answer
/// wins. No other event is ever held.
pub const HELD: &str = "PermissionRequest";

/// One hook event, as read.
#[derive(Clone, Debug, PartialEq)]
pub struct Hook {
    pub session: String,
    pub cwd: String,
    pub transcript: String,
    pub event: HookEvent,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HookEvent {
    Start,
    Prompt {
        text: String,
    },
    Permission {
        tool: String,
        input: Value,
        suggestions: Value,
    },
    ToolDone {
        tool: String,
        input: Value,
    },
    Notice {
        kind: String,
        message: String,
    },
    Stop {
        last: String,
    },
    End,
    Other(String),
}

/// A hook's JSON (stdin of the hook command). None when it names no session.
pub fn parse_hook(v: &Value) -> Option<Hook> {
    let session = s(v, "session_id").to_owned();
    if session.is_empty() {
        return None;
    }
    let tool = || s(v, "tool_name").to_owned();
    let input = || v.get("tool_input").cloned().unwrap_or(Value::Null);
    let event = match s(v, "hook_event_name") {
        "SessionStart" => HookEvent::Start,
        "UserPromptSubmit" => HookEvent::Prompt {
            text: s(v, "prompt").to_owned(),
        },
        "PermissionRequest" => HookEvent::Permission {
            tool: tool(),
            input: input(),
            suggestions: v
                .get("permission_suggestions")
                .cloned()
                .unwrap_or(Value::Null),
        },
        "PostToolUse" => HookEvent::ToolDone {
            tool: tool(),
            input: input(),
        },
        "Notification" => HookEvent::Notice {
            kind: s(v, "notification_type").to_owned(),
            message: s(v, "message").to_owned(),
        },
        "Stop" => HookEvent::Stop {
            last: s(v, "last_assistant_message").to_owned(),
        },
        "SessionEnd" => HookEvent::End,
        other => HookEvent::Other(other.to_owned()),
    };
    Some(Hook {
        session,
        cwd: s(v, "cwd").to_owned(),
        transcript: s(v, "transcript_path").to_owned(),
        event,
    })
}

/// The question tool's name (its questions arrive as a permission request, HAND-662).
pub const QUESTION_TOOL: &str = "AskUserQuestion";

/// The card for a held permission request. A question with more than one part, or with several answers at once, is
/// a card with no choices: it is answered on the Mac.
pub fn card(
    id: &str,
    session: &str,
    tool: &str,
    input: &Value,
    suggestions: &Value,
    at: &str,
) -> Card {
    if tool == QUESTION_TOOL {
        let qs = input
            .get("questions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let first = qs.first().cloned().unwrap_or(Value::Null);
        let simple =
            qs.len() == 1 && first.get("multiSelect").and_then(Value::as_bool) != Some(true);
        let choices = if simple {
            first
                .get("options")
                .and_then(Value::as_array)
                .map(|o| {
                    o.iter()
                        .filter_map(|x| x.get("label").and_then(Value::as_str))
                        .filter(|l| !l.trim().is_empty())
                        .map(|l| Choice {
                            id: l.to_owned(),
                            label: l.to_owned(),
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let detail = first
            .get("options")
            .and_then(Value::as_array)
            .map(|o| {
                o.iter()
                    .map(|x| {
                        let d = s(x, "description");
                        if d.is_empty() {
                            s(x, "label").to_owned()
                        } else {
                            format!("{}: {}", s(x, "label"), d)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        return Card {
            id: id.to_owned(),
            session: session.to_owned(),
            kind: CardKind::Question,
            tool: String::new(),
            detail,
            question: s(&first, "question").to_owned(),
            choices,
            asked_at: at.to_owned(),
            approval_id: String::new(),
        };
    }
    let mut choices = vec![Choice {
        id: "allow_once".into(),
        label: "Allow".into(),
    }];
    if suggestions.as_array().is_some_and(|a| !a.is_empty()) {
        choices.push(Choice {
            id: "allow_for_session".into(),
            label: "Allow for this session".into(),
        });
    }
    choices.push(Choice {
        id: "deny".into(),
        label: "Deny".into(),
    });
    Card {
        id: id.to_owned(),
        session: session.to_owned(),
        kind: CardKind::Permission,
        tool: tool.to_owned(),
        detail: permission_detail(input),
        question: String::new(),
        choices,
        asked_at: at.to_owned(),
        approval_id: String::new(),
    }
}

/// What a permission would run or change, exactly: the command, else the path, else the input's text fields.
pub fn permission_detail(input: &Value) -> String {
    if let Some(c) = input.get("command").and_then(Value::as_str) {
        return c.trim().to_owned();
    }
    let line = tool_line(input);
    if !line.is_empty() {
        return line;
    }
    match input {
        Value::Object(m) if !m.is_empty() => {
            clip(&one_line(&Value::Object(m.clone()).to_string()), 400)
        }
        _ => String::new(),
    }
}

/// The hook's reply for an answer, as Claude Code reads it on stdout. "" is no decision (Claude Code's own prompt
/// decides, HAND-662).
pub fn reply(choice: &str, card: &Card, input: &Value, suggestions: &Value) -> String {
    let decision = match card.kind {
        CardKind::Permission => match choice {
            "allow_once" => json!({"behavior": "allow"}),
            "allow_for_session" => json!({"behavior": "allow", "updatedPermissions": suggestions}),
            "deny" => json!({"behavior": "deny", "message": "The owner said no, from FERB."}),
            _ => return String::new(),
        },
        CardKind::Question | CardKind::Ferb => {
            if !card.choices.iter().any(|c| c.id == choice) {
                return String::new();
            }
            let mut updated = match input {
                Value::Object(m) => m.clone(),
                _ => Map::new(),
            };
            let mut answers = Map::new();
            answers.insert(card.question.clone(), json!(choice));
            updated.insert("answers".into(), Value::Object(answers));
            json!({"behavior": "allow", "updatedInput": updated})
        }
    };
    json!({"hookSpecificOutput": {"hookEventName": HELD, "decision": decision}}).to_string()
}

/// Whether a finished tool is the one a card asked about (HAND-662: the request has no tool_use_id, so by tool and
/// input).
pub fn same_call(card_tool: &str, card_input: &Value, tool: &str, input: &Value) -> bool {
    if card_tool != tool {
        return false;
    }
    if tool == QUESTION_TOOL {
        // the answered input carries `answers` besides the asked one
        return card_input.get("questions") == input.get("questions");
    }
    card_input == input
}

// ---------- claude agents --json ----------

/// One running session as `claude agents --json` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub pid: i32,
    pub session: String,
    pub cwd: String,
    pub name: String,
    pub busy: bool,
}

/// `claude agents --json`'s output, read leniently: a list, or an object holding one.
pub fn parse_agents(out: &str) -> Vec<Listed> {
    let Ok(v) = serde_json::from_str::<Value>(out.trim()) else {
        return Vec::new();
    };
    let list = match &v {
        Value::Array(a) => a.clone(),
        Value::Object(m) => m
            .values()
            .find_map(|x| x.as_array().cloned())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    list.iter()
        .filter_map(|a| {
            let session = ["sessionId", "session_id", "id"]
                .iter()
                .find_map(|k| a.get(*k).and_then(Value::as_str))?
                .to_owned();
            let pid = a.get("pid").and_then(Value::as_i64).unwrap_or(0) as i32;
            Some(Listed {
                pid,
                session,
                cwd: s(a, "cwd").to_owned(),
                name: s(a, "name").to_owned(),
                busy: matches!(s(a, "status"), "busy" | "working" | "running"),
            })
        })
        .collect()
}
