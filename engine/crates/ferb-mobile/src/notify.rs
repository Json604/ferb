//! What a phone notification says, in FERB.app's words (kb PHN-795, PHN-796): one mapping both phones use, so a task's
//! end and a question read on the phone as they read on the Mac.
//!
//! - A task's end is the Mac's banner for it (ferb-daemon areas_b/notify.rs `task_result`, ferb-supervisor
//!   flow/daemon_seams.rs `ResultsOwner::task_result`): the task's short name as the title ("FERB finished" for one
//!   that ended done without a name, "FERB" for any other end), and the answer as the text, in the words FERB.app's chat
//!   draws ("I couldn't finish this task. …", "Not done: …"). Never a status word of the phone's own. With no answer,
//!   the chat's status words under an answer ("Failed", "Delivered, with parts not done").
//! - A task that ended cancelled has no notification, as the Mac posts no banner for it (`status != "cancelled"` in
//!   both banner paths): a stop the owner asked for ("Stopped, as you asked.") and a question he declined ("Not done, as
//!   you asked.") are his own doing, and the chat shows them; the first turn of a run his follow-up started again
//!   ("Stopped here to start again with your change.", ORCH-683, also cancelled) isn't the task's end at all.
//! - A question is the Mac's banner for it (`question::shown`, notify.rs `approval_notice`): "FERB needs you" and
//!   "FERB needs your OK: …"; the planner's question, which has no banner, "FERB needs you" and its own words.
//!
//! What only the phone has (the held words, a hidden input, the fingerprint line, the lock screen) stays the apps'.

use serde::Serialize;

use crate::question::QuestionShown;
use crate::rich;
use crate::shown::{status_words, SITES};

/// The Mac's banner title for a task that ended done without a short name (notify.rs `TASK_DONE_TITLE`).
pub const TASK_DONE_TITLE: &str = "FERB finished";
/// The Mac's banner title for any other end without a short name (notify.rs `TASK_ENDED_TITLE`).
pub const TASK_ENDED_TITLE: &str = "FERB";
/// The Mac's banner title for a question (notify.rs `QUESTION_TITLE`); the planner's question takes it too.
pub const QUESTION_TITLE: &str = "FERB needs you";
/// What the Mac's banner text for a question starts with, before the question (notify.rs `QUESTION_LEAD`).
pub const QUESTION_LEAD: &str = "FERB needs your OK:";
/// The end the Mac posts no banner for: the owner's own stop, his declined question, a restart's first turn.
pub const NO_BANNER: &str = "cancelled";

/// A notification's title and text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct NotificationWords {
    pub title: String,
    pub text: String,
}

/// A task's end as the Mac's banner says it: `status` the end (done, done_with_gaps, failed, cancelled, needs_user,
/// interrupted, …), `name` the task's short name, `answer` what FERB answered and `display` the shorter words FERB.app
/// shows for it (a turn's `display_text`). `held`: the Mac held the words back (PHN-126): the title only, the text
/// empty for the app's own line. None for an end the Mac posts no banner for ([`NO_BANNER`]).
pub fn task_ended(
    status: &str,
    name: &str,
    answer: &str,
    display: Option<&str>,
    held: bool,
) -> Option<NotificationWords> {
    if status == NO_BANNER {
        return None;
    }
    let name = name.trim();
    let title = if !name.is_empty() {
        name.to_string()
    } else if status == "done" {
        TASK_DONE_TITLE.to_string()
    } else {
        TASK_ENDED_TITLE.to_string()
    };
    if held {
        return Some(NotificationWords {
            title,
            text: String::new(),
        });
    }
    let shown = display.filter(|d| !d.trim().is_empty()).unwrap_or(answer);
    let mut text = words(shown);
    if text.is_empty() {
        text = status_words(status).unwrap_or_default();
    }
    Some(NotificationWords { title, text })
}

/// A question as the Mac's banner says it; the planner's question (no banner on the Mac) as "FERB needs you" and its
/// own words.
pub fn question(q: &QuestionShown) -> NotificationWords {
    NotificationWords {
        title: q
            .banner_title
            .clone()
            .unwrap_or_else(|| QUESTION_TITLE.to_string()),
        text: q.banner_text.clone().unwrap_or_else(|| q.question.clone()),
    }
}

/// An answer's words as FERB.app's chat draws them: the Markdown's marks taken out, the "Sites contacted:" line left to
/// the chat's sites row.
fn words(answer: &str) -> String {
    let kept: Vec<&str> = answer
        .split('\n')
        .filter(|l| !l.starts_with(SITES))
        .collect();
    rich::plain_text(&rich::document(&kept.join("\n")))
        .trim()
        .to_string()
}
