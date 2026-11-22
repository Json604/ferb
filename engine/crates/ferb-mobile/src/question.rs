//! A question as FERB.app shows it (kb PHN-739): the card's header, its one question, the payment picture, the why
//! line, the exact thing in its box, the task's reason, the evidence, the buttons, the note and the typed answer's
//! box, all from the approval's own fields (the gateway's `card_parts`, GATE-513, GATE-566), never from its text.
//! Ported from apps/mac/FERBNotch/Sources/NotchCore/DaemonApproval.swift and FERBNotch/ApprovalView.swift so both
//! phones draw the Mac's words; the phone adds only its own lines (the fingerprint, its sheet). The Mac's banner for a
//! question (ferb-daemon areas_b/notify.rs `approval_notice`) is here too, for the phone's notification.

use serde::Serialize;
use serde_json::Value;

/// What a question shows, in FERB.app's order (ApprovalView.swift:22-108).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct QuestionShown {
    /// The card's header (ApprovalView.swift:26): the daemon's `title` (questions.rs `header`), else "Private input
    /// needed" for a hidden input, else "FERB needs your answer". Empty for the planner's question (a bubble, no card).
    pub header: String,
    /// A hidden input (DaemonApproval.swift:53): its field, never buttons.
    pub secret: bool,
    /// The small "from the task" tag (ApprovalView.swift:34): a task's own question, by the daemon's flag.
    pub from_task: bool,
    /// The one question (DaemonApproval.swift:23 `visibleReason`): the daemon's `summary`, else its `reason`.
    pub question: String,
    /// FERB's own words, drawn verbatim (DaemonApproval.swift:26 `plainText`); otherwise Markdown.
    pub plain: bool,
    /// The payment page's picture was captured (GATE-579), or why it couldn't be.
    pub picture: bool,
    pub picture_problem: Option<String>,
    /// Why FERB asks, under the question in grey (GATE-513).
    pub detail: Option<String>,
    /// The exact thing approved, in its own code box, word for word.
    pub exact: Option<String>,
    /// "The task's reason: …" (or "The task's note: …" for a retry), smaller (ApprovalView.swift:60-64).
    pub why: Option<String>,
    /// The "App-use evidence" lines, behind a disclosure (ApprovalView.swift:65-73).
    pub evidence: Vec<String>,
    /// Each option's button words, in the options' order (DaemonApproval.swift:49 `optionLabel`); empty for a hidden
    /// input or a question answered by typing.
    pub buttons: Vec<String>,
    /// The line under the buttons (what the 30-minute answer covers, GATE-566).
    pub note: Option<String>,
    /// The typed answer's box (DaemonApproval.swift:69 `answerBox`): "none", "answer" or "correction".
    pub answer_box: String,
    /// The field's placeholder: a hidden input's label ("Code", "Type it here", …) or "Or type your answer…" / "Or
    /// tell FERB what to do instead…" (ApprovalView.swift:95, 103).
    pub field: Option<String>,
    /// The decline beside the field: a hidden input's ("Don't fill in") or "Skip" for a question without choices.
    pub decline: Option<String>,
    /// The field's send button: a hidden input's ("Fill in") or "Send".
    pub send: Option<String>,
    /// The Mac's banner for it when no chat shows it (notify.rs `approval_notice`): "FERB needs you" and "FERB needs
    /// your OK: <question>". None where the Mac posts none (the profile question at setup, the planner's question).
    pub banner_title: Option<String>,
    pub banner_text: Option<String>,
}

/// The planner's question that waits on the owner (ferb-daemon phone/asking.rs): FERB's own bubble, answered by a
/// message.
pub const CLARIFY: &str = "clarify";

const SECRET_KINDS: [&str; 3] = ["owner_login_code", "owner_admin_password", "owner_secret"];

/// A question from the approval's fields: a question entry's body, or one of `chat.status`'s approvals.
pub fn shown(v: &Value) -> QuestionShown {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let some = |k: &str| s(k).filter(|t| !t.is_empty());
    let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    let kind = s("kind").unwrap_or_else(|| "owner_question".into());
    let reason = s("reason").unwrap_or_else(|| "FERB needs your answer.".into());
    if kind == CLARIFY {
        // FERB.app draws the planner's question as its reply bubble, answered from the composer: no card, no banner
        return QuestionShown {
            question: reason,
            answer_box: "none".into(),
            ..QuestionShown::default()
        };
    }
    let capability = s("capability");
    let summary = some("summary");
    let secret = SECRET_KINDS.contains(&kind.as_str());
    let options: Vec<String> = v
        .get("options")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|o| match o {
                    Value::String(t) => t.clone(),
                    Value::Object(m) => m
                        .get("label")
                        .or_else(|| m.get("text"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    other => other.to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    let labels: Vec<String> = v
        .get("labels")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|l| l.as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default();
    let from_task = flag("from_task");
    let options_only = flag("options_only");
    let runtime_switch = v.get("runtime_switch").is_some_and(|r| r.is_object());
    let answer_box = if secret || options.is_empty() {
        "answer"
    } else if options_only || runtime_switch {
        "none"
    } else if flag("corrects") {
        "correction"
    } else if from_task {
        "answer"
    } else {
        "none"
    };
    let buttons = if secret {
        Vec::new()
    } else {
        options
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let l = labels.get(i).map(|l| l.trim()).unwrap_or_default();
                if l.is_empty() {
                    o.clone()
                } else {
                    l.to_owned()
                }
            })
            .collect()
    };
    let field = if secret {
        Some(some("field_label").unwrap_or_else(|| {
            if kind == "owner_login_code" {
                "Code"
            } else {
                "Type it here"
            }
            .into()
        }))
    } else if answer_box == "correction" {
        Some("Or tell FERB what to do instead…".into())
    } else if answer_box == "answer" {
        Some("Or type your answer…".into())
    } else {
        None
    };
    let decline = if secret {
        Some(some("decline_label").unwrap_or_else(|| "Don't fill in".into()))
    } else if answer_box != "none" && options.is_empty() {
        Some("Skip".into())
    } else {
        None
    };
    let send = if secret {
        Some(some("send_label").unwrap_or_else(|| "Fill in".into()))
    } else if answer_box != "none" {
        Some("Send".into())
    } else {
        None
    };
    let why = some("why").map(|w| {
        let lead = if capability.as_deref() == Some("ferb.retry") {
            "The task's note: "
        } else {
            "The task's reason: "
        };
        format!("{lead}{w}")
    });
    let question = summary.clone().unwrap_or_else(|| reason.clone());
    let banner = capability.as_deref() != Some("setup.persona");
    QuestionShown {
        header: some("title").unwrap_or_else(|| {
            if secret {
                "Private input needed"
            } else {
                "FERB needs your answer"
            }
            .into()
        }),
        secret,
        from_task,
        plain: summary.is_some() || capability.as_deref() == Some("ferb.confirm"),
        picture: flag("picture"),
        picture_problem: some("picture_problem"),
        detail: some("detail"),
        exact: some("exact"),
        why,
        evidence: v
            .get("evidence")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|e| e.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        buttons,
        note: if secret { None } else { some("note") },
        answer_box: answer_box.into(),
        field,
        decline,
        send,
        banner_title: banner.then(|| crate::notify::QUESTION_TITLE.into()),
        banner_text: banner.then(|| format!("{} {question}", crate::notify::QUESTION_LEAD)),
        question,
    }
}
