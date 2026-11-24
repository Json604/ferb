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
    /// A question of the temporary chat's task (`"temporary": true`): shown only in the temporary chat, never stored.
    pub temporary: bool,
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
    /// A task of the temporary chat (`"temporary": true`): shown only in the temporary chat, never stored.
    pub temporary: bool,
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

/// The Mac's power as insomnia mode reports it (APP-981): any part may be unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InsomniaPower {
    /// The battery's percent.
    pub battery: Option<u32>,
    pub on_battery: Option<bool>,
    pub lid_closed: Option<bool>,
    /// nominal, fair, serious, critical.
    pub thermal: Option<String>,
}

/// Insomnia mode's camera (APP-983): on only while a phone pulls frames; `state` is FERB.app's report (allowed,
/// denied, not_asked, unknown).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InsomniaCamera {
    pub on: bool,
    pub state: String,
}

/// A warning while insomnia mode is on (APP-981): battery_low, battery_urgent, hot, shield_down; `text` is the Mac's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InsomniaWarning {
    pub id: String,
    pub kind: String,
    pub text: String,
    pub at: Option<String>,
}

/// Someone at the Mac while insomnia mode is on (APP-983): `kind` touched (a person's input) or activation (an app
/// FERB didn't open came to the front, named by `app`). Answered by Show me, Lock my Mac or It's fine; gone (none)
/// once answered or the mode ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InsomniaNotice {
    pub id: String,
    pub kind: String,
    pub at: Option<String>,
    /// The Mac's words ("Someone is using your Mac's keyboard or trackpad.").
    pub text: String,
    pub app: Option<String>,
    /// How many times it happened in this episode.
    pub count: u32,
}

/// How insomnia mode last ended (kept by the Mac 10 minutes, so each device says it once).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InsomniaLastEnd {
    pub id: String,
    /// back, turned_off, …
    pub why: String,
    /// The Mac's words ("Welcome back. …").
    pub text: String,
    pub at: Option<String>,
}

/// FERB's helper on the Mac that keeps it running with the lid closed (as the daemon says it): its `state`, the
/// `error` of a failed set-up, and `problem`, plain words for why it isn't on (waiting for the owner's OK in System
/// Settings, a set-up that failed), none when nothing is wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InsomniaHelper {
    pub state: String,
    pub error: Option<String>,
    pub problem: Option<String>,
}

/// Insomnia mode (APP-980..APP-985): FERB keeps the Mac working while the owner is away. The daemon's status object
/// (`insomnia.status`, `chat.status`'s `insomnia`), read leniently: a part the Mac leaves out reads as its default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Insomnia {
    pub on: bool,
    /// shield (FERB.app covers the screen, the Mac stays unlocked) or locked; none while off.
    pub variant: Option<String>,
    pub since: Option<String>,
    /// Who turned it on: "mac" or a phone's endpoint id.
    pub by: Option<String>,
    /// The one Yes to bring apps to the front under the cover for this session (APP-984).
    pub screen_yes: bool,
    /// Minutes the mode stays on after FERB's work is done.
    pub grace_minutes: Option<u32>,
    /// Tasks running now.
    pub working: u32,
    /// When nothing runs: when the grace ends (ISO 8601).
    pub ends_at: Option<String>,
    pub shield_up: bool,
    pub mac_locked: bool,
    pub camera: InsomniaCamera,
    pub power: InsomniaPower,
    pub warnings: Vec<InsomniaWarning>,
    pub notice: Option<InsomniaNotice>,
    /// The Mac's own line ("Keeping your Mac working · 2 tasks running · battery 64%"); empty while off.
    pub line: String,
    /// While off: FERB holds the Mac from idle sleep because a task runs.
    pub task_awake: bool,
    pub last_end: Option<InsomniaLastEnd>,
    /// The Mac's own words for closing the lid on battery, true to its helper's state now (on and off); none from an
    /// older Mac (the app then says its own words).
    pub lid_line: Option<String>,
    /// Whether FERB can keep working with the lid closed on battery: ready, absent or unknown; none from an older Mac.
    pub closed_lid: Option<String>,
    /// FERB's lid helper on the Mac; none from an older Mac.
    pub helper: Option<InsomniaHelper>,
    /// The Mac is asleep and only briefly awake now (one of macOS's Power Nap wakes), on and off: replies are slow and
    /// screen work waits. False from an older Mac.
    pub asleep: bool,
    /// The Mac's own words for that while it is so ("Your Mac is asleep with its lid closed. …"), shown as given; none
    /// otherwise or from an older Mac.
    pub asleep_line: Option<String>,
}

fn opt_u32(v: &Value) -> Option<u32> {
    v.as_u64()
        .or_else(|| v.as_f64().filter(|f| *f >= 0.0).map(|f| f.round() as u64))
        .map(|n| n.min(u32::MAX as u64) as u32)
}

impl Insomnia {
    pub fn read(v: &Value) -> Insomnia {
        let power = &v["power"];
        let camera = &v["camera"];
        Insomnia {
            on: v["on"].as_bool().unwrap_or(false),
            variant: opt(&v["variant"]),
            since: opt(&v["since"]),
            by: opt(&v["by"]),
            screen_yes: v["screen_yes"].as_bool().unwrap_or(false),
            grace_minutes: opt_u32(&v["grace_minutes"]),
            working: opt_u32(&v["working"]).unwrap_or(0),
            ends_at: opt(&v["ends_at"]),
            shield_up: v["shield_up"].as_bool().unwrap_or(false),
            mac_locked: v["mac_locked"].as_bool().unwrap_or(false),
            camera: InsomniaCamera {
                on: camera["on"].as_bool().unwrap_or(false),
                state: opt(&camera["state"]).unwrap_or_else(|| "unknown".into()),
            },
            power: InsomniaPower {
                battery: opt_u32(&power["battery"]),
                on_battery: power["on_battery"].as_bool(),
                lid_closed: power["lid_closed"].as_bool(),
                thermal: opt(&power["thermal"]),
            },
            warnings: list(&v["warnings"])
                .iter()
                .filter(|w| w.is_object())
                .map(|w| InsomniaWarning {
                    id: text(&w["id"]),
                    kind: text(&w["kind"]),
                    text: text(&w["text"]),
                    at: opt(&w["at"]),
                })
                .collect(),
            notice: v
                .get("notice")
                .filter(|n| n.is_object())
                .map(|n| InsomniaNotice {
                    id: text(&n["id"]),
                    kind: text(&n["kind"]),
                    at: opt(&n["at"]),
                    text: text(&n["text"]),
                    app: opt(&n["app"]),
                    count: opt_u32(&n["count"]).unwrap_or(1),
                }),
            line: v["line"].as_str().unwrap_or_default().to_string(),
            task_awake: v["task_awake"].as_bool().unwrap_or(false),
            last_end: v
                .get("last_end")
                .filter(|e| e.is_object())
                .map(|e| InsomniaLastEnd {
                    id: text(&e["id"]),
                    why: text(&e["why"]),
                    text: text(&e["text"]),
                    at: opt(&e["at"]),
                }),
            lid_line: opt(&v["lid_line"]),
            closed_lid: opt(&v["closed_lid"]),
            helper: v
                .get("helper")
                .filter(|h| h.is_object())
                .map(|h| InsomniaHelper {
                    state: text(&h["state"]),
                    error: opt(&h["error"]),
                    problem: opt(&h["problem"]),
                }),
            asleep: v["asleep"].as_bool().unwrap_or(false),
            asleep_line: opt(&v["asleep_line"]),
        }
    }
}

/// One of FERB's alerts (kb APP-1105, APP-1106, PHN-791): the Mac getting hot, insomnia mode's end, a database set
/// aside, … Shown for a while above the chat, never in it; it stays until the owner puts it away on any device
/// ([`crate::remote::Remote::dismiss_alert`]), and goes from every device then. Only while words are given (a locked
/// phone's status holds them back).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusAlert {
    /// The notice's id (what putting it away names).
    pub id: String,
    /// The Mac's words.
    pub text: String,
    /// When FERB said it (ISO 8601).
    pub at: Option<String>,
}

impl StatusAlert {
    /// The alerts among `chat.status`'s unseen notices (each marked `alert`), oldest first as the Mac lists them.
    pub fn of(notices: &Value) -> Vec<StatusAlert> {
        list(notices)
            .iter()
            .filter(|n| n["alert"].as_bool() == Some(true))
            .filter_map(|n| {
                let id = opt(&n["id"])?;
                let text = opt(&n["text"])?;
                Some(StatusAlert {
                    id,
                    text,
                    at: opt(&n["at"]),
                })
            })
            .collect()
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
    /// Insomnia mode (APP-980..APP-985); none from a Mac that doesn't say (an older FERB).
    pub insomnia: Option<Insomnia>,
    /// FERB's alerts not yet put away (APP-1106); empty while the words are held back.
    pub alerts: Vec<StatusAlert>,
    /// The memory import's live card (MEM-456) while one goes on; none otherwise, while the words are held back, or
    /// from a Mac that doesn't say.
    pub memory_import: Option<ImportCard>,
    /// The temporary chat while one is on (`chat.status`'s `temporary`): which one, its change number and its start.
    /// None while none is on, or from a Mac that doesn't say. Memory only, like the rest of the status.
    pub temporary: Option<TemporaryState>,
}

/// The temporary chat as `chat.status` names it while one is on: its id, its `rev` (one more on every change: its
/// start, each entry's change, its end) and when it started. Its turns come through the watch (`last_temporary_chat`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TemporaryState {
    pub id: String,
    pub rev: i64,
    pub started_at: Option<String>,
}

impl TemporaryState {
    /// From `chat.status`'s `temporary` (an object with an `id`); None for anything else.
    pub fn read(v: &Value) -> Option<TemporaryState> {
        let id = opt(&v["id"])?;
        Some(TemporaryState {
            id,
            rev: v["rev"].as_i64().unwrap_or(0),
            started_at: opt(&v["started_at"]),
        })
    }
}

/// The memory import's live card (MEM-456), in the Mac's own words: drawn at the end of today's chat, as FERB.app
/// draws it, from the moment an import FERB tells the owner about starts until its one notice comes in (the card is
/// live state, never a chat entry). Each row's bar is `done` of `total` (none while `total` is 0), with `count` beside
/// it; `found_line` under it, `detail` quieter, `note` set apart (a gap, or a try that didn't go through, with what
/// happens next). A row's `name` is shown only when the card has more than one row ([`ImportCard::names_rows`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportCard {
    pub title: String,
    pub rows: Vec<ImportRow>,
}

/// One source on the card: a run reading or placing, or a website's watch waiting for its export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportRow {
    /// Stable while the row lives ("run:<id>", "site:<source>"): the bar animates within one key.
    pub key: String,
    pub source: String,
    pub name: String,
    /// reading, placing, asking, waiting_mail, checking_mail.
    pub stage: String,
    pub line: String,
    pub detail: String,
    pub done: u32,
    pub total: u32,
    pub count: String,
    pub found: u32,
    pub found_line: String,
    pub note: String,
}

impl ImportRow {
    /// How full the bar is, 0 to 1; None: no bar (nothing planned to count yet, or a website's waiting stage).
    pub fn fraction(&self) -> Option<f32> {
        (self.total > 0).then(|| self.done.min(self.total) as f32 / self.total as f32)
    }
}

impl ImportCard {
    /// From the Mac's card (`chat.status`'s `memory_import`, the bus's `memory.import.live` `card`); None for anything
    /// that isn't one with a row.
    pub fn read(v: &Value) -> Option<ImportCard> {
        let n = |x: &Value| {
            x.as_u64()
                .map(|n| n.min(u32::MAX as u64) as u32)
                .unwrap_or(0)
        };
        let rows: Vec<ImportRow> = list(&v["rows"])
            .iter()
            .map(|r| ImportRow {
                key: text(&r["key"]),
                source: text(&r["source"]),
                name: text(&r["name"]),
                stage: text(&r["stage"]),
                line: text(&r["line"]),
                detail: text(&r["detail"]),
                done: n(&r["done"]),
                total: n(&r["total"]),
                count: text(&r["count"]),
                found: n(&r["found"]),
                found_line: text(&r["found_line"]),
                note: text(&r["note"]),
            })
            .filter(|r| !r.key.is_empty())
            .collect();
        (!rows.is_empty()).then(|| ImportCard {
            title: text(&v["title"]),
            rows,
        })
    }

    /// Each row says its source's name (more than one source on the card).
    pub fn names_rows(&self) -> bool {
        self.rows.len() > 1
    }
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
                    temporary: a["temporary"].as_bool().unwrap_or(false),
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
                    temporary: r["temporary"].as_bool().unwrap_or(false),
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
            insomnia: v
                .get("insomnia")
                .filter(|s| s.is_object())
                .map(Insomnia::read),
            alerts: StatusAlert::of(&v["notices"]),
            memory_import: v
                .get("memory_import")
                .filter(|c| c.is_object())
                .and_then(ImportCard::read),
            temporary: v
                .get("temporary")
                .filter(|t| t.is_object())
                .and_then(TemporaryState::read),
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

    /// The memory import's card reads as the Mac sent it; the bar is parts done of parts planned; none without rows.
    // decision: MEM-456
    #[test]
    fn the_memory_import_card_reads_as_the_mac_sent_it() {
        let v = json!({"memory_import": {"title": "Bringing in what Claude Code knows about you", "rows": [
            {"key": "run:r1", "source": "claude_code", "name": "Claude Code", "stage": "reading",
             "line": "Reading what you've typed", "detail": "", "done": 33, "total": 40, "count": "33 of 40",
             "found": 221, "found_line": "221 things found so far", "note": ""}]}});
        let card = PhoneStatus::read(&v).memory_import.expect("a card");
        assert_eq!(card.title, "Bringing in what Claude Code knows about you");
        assert!(!card.names_rows());
        assert_eq!(card.rows[0].count, "33 of 40");
        assert_eq!(card.rows[0].fraction(), Some(33.0 / 40.0));
        assert_eq!(card.rows[0].found_line, "221 things found so far");
        let waiting =
            ImportCard::read(&json!({"title": "t", "rows": [{"key": "site:chatgpt", "total": 0}]}));
        assert_eq!(
            waiting.unwrap().rows[0].fraction(),
            None,
            "a website waiting for its mail has no bar"
        );
        assert_eq!(
            PhoneStatus::read(&json!({"memory_import": {"title": "t", "rows": []}})).memory_import,
            None
        );
        assert_eq!(
            PhoneStatus::read(&json!({"memory_import": null})).memory_import,
            None
        );
        assert_eq!(PhoneStatus::read(&json!({})).memory_import, None);
    }

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

    /// Insomnia mode as the daemon's contract gives it (on, with a notice and a warning; off, with the last end), and
    /// a Mac that doesn't say (an older FERB): none.
    // decision: APP-980
    // decision: APP-981
    // decision: APP-983
    #[test]
    fn insomnia_is_read_on_and_off_and_absent_from_an_older_mac() {
        let on = PhoneStatus::read(&json!({
            "approvals": [], "running": [], "results": [],
            "insomnia": {"on": true, "variant": "shield", "since": "2026-11-20T15:00:00+05:30", "by": "mac",
                "screen_yes": true, "grace_minutes": 10, "working": 2, "ends_at": null, "shield_up": true,
                "mac_locked": false, "camera": {"on": false, "state": "allowed"},
                "power": {"battery": 18, "on_battery": true, "lid_closed": null, "thermal": "fair"},
                "warnings": [{"id": "w-1", "kind": "battery_low", "text": "Your Mac's battery is at 18%.", "at": "x"}],
                "notice": {"id": "n-1", "kind": "touched", "at": "y", "app": null, "count": 3,
                           "text": "Someone is using your Mac's keyboard or trackpad."},
                "line": "Keeping your Mac working · 2 tasks running · battery 18%", "last_end": null}
        }));
        let i = on.insomnia.unwrap();
        assert!(i.on && i.screen_yes && i.shield_up && !i.mac_locked);
        assert_eq!(i.variant.as_deref(), Some("shield"));
        assert_eq!((i.working, i.grace_minutes), (2, Some(10)));
        assert_eq!(i.camera.state, "allowed");
        assert_eq!(
            (i.power.battery, i.power.on_battery, i.power.lid_closed),
            (Some(18), Some(true), None)
        );
        assert_eq!(i.warnings[0].kind, "battery_low");
        let n = i.notice.unwrap();
        assert_eq!(
            (n.id.as_str(), n.kind.as_str(), n.count),
            ("n-1", "touched", 3)
        );
        assert!(i.line.starts_with("Keeping your Mac working"));
        let off = Insomnia::read(
            &json!({"on": false, "power": {}, "task_awake": true, "line": "",
            "last_end": {"id": "e-1", "why": "back", "text": "Welcome back.", "at": "z"}}),
        );
        assert!(!off.on && off.task_awake && off.notice.is_none() && off.warnings.is_empty());
        assert_eq!(off.camera.state, "unknown");
        assert_eq!(off.last_end.unwrap().why, "back");
        let older = PhoneStatus::read(&json!({"approvals": [], "running": [], "results": []}));
        assert_eq!(older.insomnia, None);
        assert!(
            off.lid_line.is_none() && off.closed_lid.is_none(),
            "an older Mac says neither"
        );
    }

    /// The lid line and the closed-lid state, on and off, as the Mac says them now.
    // decision: APP-981
    #[test]
    fn the_lid_line_and_the_closed_lid_state_are_read_on_and_off() {
        let off = Insomnia::read(
            &json!({"on": false, "power": {}, "line": "", "closed_lid": "absent",
            "lid_line": "On battery with the lid closed, your Mac sleeps until you open it or plug it in."}),
        );
        assert_eq!(off.closed_lid.as_deref(), Some("absent"));
        assert!(off.lid_line.unwrap().starts_with("On battery"));
        let on = Insomnia::read(&json!({"on": true, "closed_lid": "ready",
            "lid_line": "You can close the lid too: FERB keeps working, plugged in or on battery."}));
        assert_eq!((on.on, on.closed_lid.as_deref()), (true, Some("ready")));
        assert!(on.lid_line.is_some());
        assert!(
            on.helper.is_none() && off.helper.is_none(),
            "a Mac that doesn't say"
        );
        let waiting = Insomnia::read(
            &json!({"on": false, "helper": {"state": "requires_approval", "error": null,
            "problem": "FERB's helper is waiting for your OK in System Settings → General → Login Items."}}),
        );
        let h = waiting.helper.unwrap();
        assert_eq!(h.state, "requires_approval");
        assert!(h.error.is_none() && h.problem.unwrap().contains("Login Items"));
        let fine = Insomnia::read(
            &json!({"on": true, "helper": {"state": "enabled", "error": null, "problem": null}}),
        );
        assert_eq!(fine.helper.unwrap().problem, None);
    }

    // decision: APP-1106, PHN-791
    #[test]
    fn alerts_are_read_from_the_unseen_notices_and_held_back_while_locked() {
        let v = json!({"notices": [
            {"id": "memory-always-on-1", "text": "Now always on: tea", "at": "a"},
            {"id": "alert:insomnia-warning-hot", "text": "Your Mac is getting hot.", "at": "b", "alert": true},
            {"id": "alert:set-aside-ferb.db", "text": "", "alert": true},
        ]});
        let st = PhoneStatus::read(&v);
        assert_eq!(
            st.alerts,
            vec![StatusAlert {
                id: "alert:insomnia-warning-hot".into(),
                text: "Your Mac is getting hot.".into(),
                at: Some("b".into()),
            }]
        );
        // the gate keeps no field of a notice while the words are held: nothing to show
        let locked = PhoneStatus::read(&json!({"locked": true, "words": false, "notices": [{}]}));
        assert!(locked.alerts.is_empty());
        assert!(PhoneStatus::read(&json!({})).alerts.is_empty());
    }
}
