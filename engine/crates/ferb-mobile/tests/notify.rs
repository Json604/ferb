//! The phone's notifications say what FERB.app says (kb PHN-795, PHN-796): each end of a task and each kind of question
//! against the Mac's words. The Mac's words are read from the engine's own constants (their source files: this crate
//! builds for the phones and can't link the Mac's crates), so a change on the Mac fails here until the phones follow.

mod common;

use ferb_mobile::notify::{self, NotificationWords};
use ferb_mobile::question;
use serde_json::{json, Value};

const ANSWER_MARKS: &str = include_str!("../../ferb-orchestrator/src/answer_marks.rs");
const SUPERVISOR_WORDS: &str = include_str!("../../ferb-supervisor/src/words.rs");
const DAEMON_NOTIFY: &str = include_str!("../../ferb-daemon/src/areas_b/notify.rs");
const SEAMS: &str = include_str!("../../ferb-supervisor/src/flow/daemon_seams.rs");

/// `pub const NAME: &str = "…";` in `source`: the literal's words (these have no escapes).
fn constant(source: &str, name: &str) -> String {
    let at = source
        .find(&format!("pub const {name}: &str ="))
        .unwrap_or_else(|| panic!("the engine has no constant {name}"));
    let rest = &source[at..];
    let open = rest.find('"').unwrap() + 1;
    let close = open + rest[open..].find("\";").unwrap();
    let words = &rest[open..close];
    assert!(
        !words.contains('\\'),
        "{name} has an escape: read it by hand"
    );
    words.to_string()
}

fn ended(status: &str, name: &str, answer: &str) -> Option<NotificationWords> {
    notify::task_ended(status, name, answer, None, false)
}

fn words(title: &str, text: &str) -> Option<NotificationWords> {
    Some(NotificationWords {
        title: title.into(),
        text: text.into(),
    })
}

/// The Mac's words, from the engine.
struct Mac {
    stopped: String,
    not_finished: String,
    where_stood: String,
    declined: String,
    restarted: String,
    gap_lead: String,
    done_title: String,
    ended_title: String,
    question_title: String,
    question_lead: String,
}

fn mac() -> Mac {
    Mac {
        stopped: constant(ANSWER_MARKS, "STOPPED"),
        not_finished: constant(ANSWER_MARKS, "NOT_FINISHED"),
        where_stood: constant(ANSWER_MARKS, "WHERE_STOOD"),
        declined: constant(ANSWER_MARKS, "DECLINED_LINE"),
        restarted: constant(SUPERVISOR_WORDS, "RESTARTED_SAID"),
        gap_lead: constant(SUPERVISOR_WORDS, "GAP_LEAD"),
        done_title: constant(DAEMON_NOTIFY, "TASK_DONE_TITLE"),
        ended_title: constant(DAEMON_NOTIFY, "TASK_ENDED_TITLE"),
        question_title: constant(DAEMON_NOTIFY, "QUESTION_TITLE"),
        question_lead: constant(DAEMON_NOTIFY, "QUESTION_LEAD"),
    }
}

/// The library's copies are the engine's words, and both of the Mac's banner paths use them.
// decision: PHN-796
#[test]
fn the_library_holds_the_macs_own_words() {
    let m = mac();
    assert_eq!(notify::TASK_DONE_TITLE, m.done_title);
    assert_eq!(notify::TASK_ENDED_TITLE, m.ended_title);
    assert_eq!(notify::QUESTION_TITLE, m.question_title);
    assert_eq!(notify::QUESTION_LEAD, m.question_lead);
    // the end neither banner path posts: the library's is theirs
    let skipped = format!("status != \"{}\"", notify::NO_BANNER);
    assert!(
        DAEMON_NOTIFY.contains(&skipped),
        "notify.rs task_result: {skipped}"
    );
    assert!(
        SEAMS.contains(&skipped),
        "daemon_seams.rs task_result: {skipped}"
    );
    for title in ["TASK_DONE_TITLE", "TASK_ENDED_TITLE"] {
        assert!(
            DAEMON_NOTIFY.contains(&format!("{title}.into()")),
            "notify.rs task_result: {title}"
        );
        assert!(
            SEAMS.contains(&format!("notify::{title}.into()")),
            "daemon_seams.rs ResultsOwner::task_result: {title}"
        );
    }
}

/// Each end of a task: the Mac's banner (its short name, the answer), the answer in the chat's words; an end the Mac
/// posts no banner for (his own stop, a question he declined, a restart's first turn: cancelled) posts nothing.
// decision: PHN-795, PHN-796
#[test]
fn each_end_of_a_task_reads_as_the_mac_says_it() {
    let m = mac();
    let gaps = format!(
        "I booked the table for two at 8.\n\n{} the window seat — they had none left.",
        m.gap_lead
    );
    let failed = format!("{} The site kept refusing the card.", m.not_finished);
    let stood = format!(
        "{} {}\n\nYou were on week 3 of 6.",
        m.stopped, m.where_stood
    );
    let interrupted =
        "This was interrupted when FERB restarted: check my Scaler dashboard. Want me to pick it up again?";
    let table: Vec<(&str, &str, &str, Option<NotificationWords>)> = vec![
        // done, with and without a short name
        (
            "done",
            "Scaler dashboard check",
            "Your dashboard shows **2** new classes.",
            words(
                "Scaler dashboard check",
                "Your dashboard shows 2 new classes.",
            ),
        ),
        (
            "done",
            "",
            "Done: the file is in Downloads.",
            words(&m.done_title, "Done: the file is in Downloads."),
        ),
        // delivered with gaps: its "Not done:" line
        (
            "done_with_gaps",
            "Table booking",
            &gaps,
            words("Table booking", &gaps),
        ),
        // failed
        ("failed", "Invoice", &failed, words("Invoice", &failed)),
        ("failed", "", &failed, words(&m.ended_title, &failed)),
        // stopped by the owner (the screenshot of 2026-11-22), with where things stood, and a declined question: his
        // own doing, no banner on the Mac, nothing on the phone (the chat shows it)
        ("cancelled", "Coursera BITS check", &m.stopped, None),
        ("cancelled", "Coursera BITS check", &stood, None),
        ("cancelled", "", &m.declined, None),
        // stopped to start again with the owner's change: the task carries on
        ("cancelled", "Coursera BITS check", &m.restarted, None),
        // ended asking him
        (
            "needs_user",
            "Gmail login",
            "I need you to sign in to Gmail first.",
            words("Gmail login", "I need you to sign in to Gmail first."),
        ),
        // FERB restarted under it
        (
            "interrupted",
            "",
            interrupted,
            words(&m.ended_title, interrupted),
        ),
        // no answer: the chat's status words
        ("failed", "Invoice", "", words("Invoice", "Failed")),
        (
            "done_with_gaps",
            "Invoice",
            "",
            words("Invoice", "Delivered, with parts not done"),
        ),
        (
            "interrupted",
            "Invoice",
            "",
            words("Invoice", "Interrupted"),
        ),
    ];
    for (status, name, answer, want) in table {
        let got = ended(status, name, answer);
        assert_eq!(got, want, "{status} {name:?} {answer:?}");
        if let Some(n) = got {
            for phone_only in ["Done", "Couldn't finish"] {
                assert_ne!(n.title, phone_only, "a status word the Mac never says");
            }
        }
    }
}

/// The answer in the words the chat draws: the shorter words FERB.app shows when there are some, Markdown's marks out,
/// the sites line left to the chat's sites row; the held words give the title only.
// decision: PHN-795, PHN-126
#[test]
fn the_answer_reads_as_the_chat_draws_it_and_held_words_stay_home() {
    let answer = "## Perks\n\n- **2 TB** storage\n- [Gemini](https://gemini.google.com) in Gmail\n\nSites contacted: one.google.com";
    let n = notify::task_ended("done", "Google AI Pro perks", answer, None, false).unwrap();
    assert!(
        !n.text.contains("**") && !n.text.contains("##") && !n.text.contains("https://"),
        "{}",
        n.text
    );
    assert!(
        n.text.starts_with("Perks") && n.text.contains("2 TB storage"),
        "{}",
        n.text
    );
    assert!(!n.text.contains("Sites contacted"), "{}", n.text);
    let short = notify::task_ended(
        "done",
        "Perks",
        "The whole long answer.",
        Some("The short one."),
        false,
    )
    .unwrap();
    assert_eq!(short.text, "The short one.");
    assert_eq!(notify::task_ended("cancelled", "", "", None, true), None);
    let held = notify::task_ended("failed", "", "", None, true).unwrap();
    assert_eq!(
        held,
        NotificationWords {
            title: mac().ended_title,
            text: String::new()
        }
    );
    assert_eq!(
        notify::task_ended("done", "", "", None, true)
            .unwrap()
            .title,
        mac().done_title
    );
}

/// Every end the Mac's chat shows in the fixtures (FERB.app's own fold, transcripts.json): the notification says the
/// bubble's words under the task's name; a cancelled end (his stop, among them) posts nothing, as on the Mac.
// decision: PHN-795
#[test]
fn every_end_in_the_macs_chat_reads_the_same_on_the_phone() {
    let m = mac();
    let golden = common::read("transcripts.json");
    let mut turns = Vec::new();
    fn walk<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
        match v {
            Value::Object(o) => {
                if o.get("item").and_then(Value::as_str) == Some("turn") {
                    out.push(v);
                }
                o.values().for_each(|x| walk(x, out));
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    walk(&golden, &mut turns);
    let mut stops = 0;
    let mut checked = 0;
    for t in turns {
        let (Some(_), Some(answer)) = (t["task_id"].as_str(), t["answer"].as_object()) else {
            continue;
        };
        let status = t["status"].as_str().unwrap_or("done");
        if ["working", "needs_user", "merged"].contains(&status) {
            continue;
        }
        let text = answer["text"].as_str().unwrap_or("");
        let name = t["task_name"].as_str().unwrap_or("");
        let n = notify::task_ended(status, name, text, answer["display_text"].as_str(), false);
        let Some(n) = n else {
            assert_eq!(status, "cancelled", "{t}");
            stops += 1;
            continue;
        };
        assert!(!text.starts_with(&m.stopped), "{t}");
        checked += 1;
        let want_title = if !name.trim().is_empty() {
            name.trim().to_string()
        } else if status == "done" {
            m.done_title.clone()
        } else {
            m.ended_title.clone()
        };
        assert_eq!(n.title, want_title, "{t}");
        if !text.trim().is_empty() {
            assert!(!n.text.is_empty(), "{t}");
        }
    }
    assert!(checked > 100 && stops > 5, "{checked} ends, {stops} stops");
}

/// A question: the Mac's banner; the planner's question, which has none, "FERB needs you" and its own words.
// decision: PHN-795, PHN-767
#[test]
fn a_question_reads_as_the_macs_banner() {
    let m = mac();
    let q = question::shown(
        &json!({"approval_id": "ap-1", "kind": "owner_question", "capability": "plan.ask",
        "summary": "Book the table for two at 8?", "reason": "The step: browser.act …", "options": ["Yes", "No"]}),
    );
    assert_eq!(
        notify::question(&q),
        NotificationWords {
            title: m.question_title.clone(),
            text: format!("{} Book the table for two at 8?", m.question_lead)
        }
    );
    let planner = question::shown(
        &json!({"approval_id": "clarify:r1", "kind": question::CLARIFY,
        "reason": "Which email account should I use?"}),
    );
    assert_eq!(
        notify::question(&planner),
        NotificationWords {
            title: m.question_title,
            text: "Which email account should I use?".into()
        }
    );
}
