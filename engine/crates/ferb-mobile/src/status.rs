//! What FERB has waiting, as the Mac's phone gate shapes it for this phone (`chat.status`, kb PHN-125, PHN-126,
//! PHN-417, PHN-470): each open question with what its answer needs from the phone, what runs and what finished since
//! the Mac last showed it. The bridge pushes it over the link's event stream when it changes (its `status` event, the
//! daemon's own `chat.status` with `peek`), and the app can ask for it (`Mobile::status`); a locked phone gets it too.
//!
//! **Locked** (no unlock window: the app locked, or five idle minutes passed on the Mac): the gate holds the words back
//! (`held`), except for a session that proved the phone's own device key (`words`), which keeps every question's words
//! but a hidden input's (PHN-126, PHN-422). The question's needs are always there: a routine question
//! (`needs_unlock` false) is answered from the notification with no proof; the others need the phone's signed proof
//! with the question's one-time `challenge` (the fingerprint-only key where `fingerprint`), and `can_answer` false says
//! it can only be answered on the Mac.
//!
//! Held in memory only: never written to the store (the words of a question are not kept on the phone).

use serde::Serialize;
use serde_json::Value;

/// What answering one question needs from the phone (the gate's `phone` part of each approval).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuestionNeeds {
    /// The phone can answer it at all (false: a hidden input on a phone paired without a screen lock: on the Mac).
    pub can_answer: bool,
    /// A Yes needs the phone's signed proof (a non-routine check, or a hidden input). False: routine (PHN-417), answered
    /// from the notification's buttons with no prompt.
    pub needs_unlock: bool,
    /// A hidden input (a code, a password): typed in FERB, never in a notification (PHN-422).
    pub hidden: bool,
    /// Only the fingerprint-only key is taken for the Yes (the phone registered one, PHN-133, PHN-137).
    pub fingerprint: bool,
    /// The one-time challenge the proof signs (this session's, for this question).
    pub challenge: Option<String>,
    /// Why it can't be answered here, in the Mac's words.
    pub problem: Option<String>,
    /// The screen-use question ("let FERB use your screen") may be answered here with a Yes that also covers later
    /// screen-use questions until the owner is back at the Mac (GATE-615): offer that button only when true. The answer
    /// is `Answer::UntilBack`.
    pub until_back: bool,
}

impl QuestionNeeds {
    /// Routine: answered from the notification with no prompt (PHN-417, PHN-470).
    pub fn routine(&self) -> bool {
        self.can_answer && !self.needs_unlock && !self.hidden
    }

    fn read(part: &Value) -> QuestionNeeds {
        QuestionNeeds {
            can_answer: part["can_answer"].as_bool().unwrap_or(true),
            needs_unlock: part["needs_unlock"].as_bool().unwrap_or(false),
            hidden: part["hidden"].as_bool().unwrap_or(false),
            fingerprint: part["fingerprint"].as_bool().unwrap_or(false),
            challenge: part["challenge"].as_str().map(str::to_string),
            problem: part["error"]["message"].as_str().map(str::to_string),
            until_back: part["until_back"].as_bool().unwrap_or(false),
        }
    }
}

/// One open question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusQuestion {
    pub approval_id: String,
    /// approve, choose, input, … (the gateway's kind).
    pub kind: String,
    pub capability: Option<String>,
    pub task_id: Option<String>,
    /// The question's words; while `held`, only which kind of question it is ("FERB has a question").
    pub reason: String,
    /// The options' words (empty while held).
    pub options: Vec<String>,
    /// The words are held back (a locked phone, PHN-126).
    pub held: bool,
    pub needs: QuestionNeeds,
    /// What FERB.app's card shows for it (kb PHN-739), from the approval's own fields; while held, the kind's words.
    #[serde(skip_serializing)]
    pub words: Option<crate::question::QuestionShown>,
}

/// A task that runs now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusTask {
    pub task_id: String,
    /// Its short name and goal (empty while held).
    pub name: String,
    pub goal: String,
    pub started_at: Option<String>,
    pub held: bool,
}

/// A task that finished since the Mac last showed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusResult {
    pub task_id: String,
    pub request_id: Option<String>,
    pub at: Option<String>,
    /// done, failed, … (the task's status).
    pub status: String,
    pub name: String,
    pub answer: String,
    pub held: bool,
}

/// FERB using the Mac's screen on a phone's "until I'm back at my Mac" (GATE-615): the gate's `screen_use` in
/// `chat.status`, and what `screen_use.status` and `screen_use.end` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScreenUse {
    /// A phone's "until I'm back" covers screen-use questions now.
    pub active: bool,
    /// Since when (ISO 8601).
    pub since: Option<String>,
    /// The phone that said it (its endpoint id on the link).
    pub phone: Option<String>,
    /// That phone's name on the Mac.
    pub phone_name: Option<String>,
    /// The Mac offers "until I'm back" on a phone now (a screen-use question may be answered so).
    pub can_offer: bool,
}

impl ScreenUse {
    pub fn read(v: &Value) -> ScreenUse {
        ScreenUse {
            active: v["active"].as_bool().unwrap_or(false),
            since: opt(&v["since"]),
            phone: opt(&v["phone"]),
            phone_name: opt(&v["phone_name"]),
            can_offer: v["can_offer"].as_bool().unwrap_or(false),
        }
    }
}

/// What FERB has waiting, as this phone may see it now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PhoneStatus {
    /// The Mac treats this phone as locked: no unlock window.
    pub locked: bool,
    /// Words are given (unlocked, or a locked session that proved the device key).
    pub words: bool,
    /// Stop everything is on (`stop.engage`); FERB does nothing until it is lifted on the Mac.
    pub stopped: bool,
    pub questions: Vec<StatusQuestion>,
    pub running: Vec<StatusTask>,
    pub results: Vec<StatusResult>,
    /// FERB using the screen on a phone's "until I'm back" (GATE-615); none from a Mac that doesn't say.
    pub screen_use: Option<ScreenUse>,
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn opt(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

fn list(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

impl PhoneStatus {
    /// From `chat.status` as the gate shaped it (a stand-in's plain answer reads as unlocked, with words).
    pub fn read(v: &Value) -> PhoneStatus {
        let locked = v["locked"].as_bool().unwrap_or(false);
        PhoneStatus {
            locked,
            words: v["words"].as_bool().unwrap_or(!locked),
            stopped: v["stopped"].as_bool().unwrap_or(false),
            questions: list(&v["approvals"])
                .iter()
                .map(|a| StatusQuestion {
                    approval_id: text(&a["approval_id"]),
                    kind: text(&a["kind"]),
                    capability: opt(&a["capability"]),
                    task_id: opt(&a["task_id"]),
                    reason: text(&a["reason"]),
                    options: list(&a["options"])
                        .iter()
                        .map(|o| match o {
                            Value::Object(m) => m
                                .get("label")
                                .or_else(|| m.get("text"))
                                .map(text)
                                .unwrap_or_default(),
                            other => text(other),
                        })
                        .collect(),
                    held: a["held"].as_bool().unwrap_or(false),
                    needs: QuestionNeeds::read(&a["phone"]),
                    words: Some(crate::question::shown(a)),
                })
                .collect(),
            running: list(&v["running"])
                .iter()
                .map(|r| StatusTask {
                    task_id: text(&r["task_id"]),
                    name: text(&r["name"]),
                    goal: text(&r["goal"]),
                    started_at: opt(&r["started_at"]),
                    held: r["held"].as_bool().unwrap_or(false),
                })
                .collect(),
            results: list(&v["results"])
                .iter()
                .map(|r| StatusResult {
                    task_id: text(&r["task_id"]),
                    request_id: opt(&r["request_id"]),
                    at: opt(&r["at"]),
                    status: text(&r["status"]),
                    name: text(&r["name"]),
                    answer: text(&r["answer"]),
                    held: r["held"].as_bool().unwrap_or(false),
                })
                .collect(),
            screen_use: v
                .get("screen_use")
                .filter(|s| s.is_object())
                .map(ScreenUse::read),
        }
    }

    /// The question's needs, by its id.
    pub fn needs(&self, approval_id: &str) -> Option<&QuestionNeeds> {
        self.questions
            .iter()
            .find(|q| q.approval_id == approval_id)
            .map(|q| &q.needs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A locked status from the gate (as ferb-daemon's `status_for` writes it) reads as held, with each question's needs.
    // decision: PHN-126
    // decision: PHN-417
    #[test]
    fn a_locked_status_reads_with_its_needs_and_held_words() {
        let s = PhoneStatus::read(&json!({
            "locked": true, "words": false, "stopped": false,
            "approvals": [
                {"approval_id": "a1", "kind": "approve", "options": [], "reason": "FERB has a question", "held": true,
                 "phone": {"can_answer": true, "needs_unlock": false}},
                {"approval_id": "a2", "kind": "approve", "options": [], "reason": "FERB asks before it goes on", "held": true,
                 "phone": {"can_answer": true, "needs_unlock": true, "hidden": false, "challenge": "c2", "fingerprint": true}},
                {"approval_id": "a3", "kind": "input", "options": [], "reason": "FERB needs a private answer", "held": true,
                 "phone": {"can_answer": false, "hidden": true, "error": {"code": "unlock_required", "message": "On the Mac."}}}
            ],
            "running": [{"task_id": "t1", "started_at": "x", "held": true}],
            "results": [{"task_id": "t0", "status": "done", "at": "y", "held": true}]
        }));
        assert!(s.locked && !s.words);
        assert!(s.questions[0].needs.routine());
        assert!(!s.questions[1].needs.routine() && s.questions[1].needs.fingerprint);
        assert_eq!(s.needs("a2").unwrap().challenge.as_deref(), Some("c2"));
        assert!(!s.questions[2].needs.can_answer && s.questions[2].needs.hidden);
        assert_eq!(s.questions[2].needs.problem.as_deref(), Some("On the Mac."));
        assert!(s.running[0].held && s.running[0].name.is_empty());
        assert_eq!(s.results[0].status, "done");
        assert!(s.questions.iter().all(|q| !q.needs.until_back));
        assert_eq!(s.screen_use, None);
    }

    /// The screen-use question offered "until I'm back" (its `phone.until_back`), and FERB using the screen on it.
    // decision: GATE-615
    #[test]
    fn until_back_is_offered_per_question_and_screen_use_is_read() {
        let s = PhoneStatus::read(&json!({
            "locked": false, "words": true, "stopped": false,
            "approvals": [
                {"approval_id": "s1", "kind": "approve", "options": [], "reason": "Let FERB use your screen?",
                 "phone": {"can_answer": true, "needs_unlock": false, "until_back": true}},
                {"approval_id": "s2", "kind": "approve", "options": [], "reason": "Open it?",
                 "phone": {"can_answer": true, "needs_unlock": false}}
            ],
            "running": [], "results": [],
            "screen_use": {"active": true, "since": "2026-11-17T10:00:00+05:30", "phone": "EP1",
                           "phone_name": "Pixel", "can_offer": true}
        }));
        assert!(s.questions[0].needs.until_back && s.questions[0].needs.routine());
        assert!(!s.questions[1].needs.until_back);
        let u = s.screen_use.unwrap();
        assert!(u.active && u.can_offer);
        assert_eq!(u.since.as_deref(), Some("2026-11-17T10:00:00+05:30"));
        assert_eq!(
            (u.phone.as_deref(), u.phone_name.as_deref()),
            (Some("EP1"), Some("Pixel"))
        );
        let idle = ScreenUse::read(&json!({"active": false, "can_offer": false}));
        assert!(!idle.active && idle.since.is_none() && idle.phone_name.is_none());
    }
}
