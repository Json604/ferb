//! ORCH-672: what the owner is told when code's facts about an action decide or shape a result (the reply endings
//! of the 2026-11-17 redesign), written by code from recorded facts only: never model text, never the word "failed"
//! alone, about FERB and in plain words. The checker-facing lines of the fresh-look rule (ORCH-671) are here too.
//!
//! decision: ORCH-672, ORCH-668, ORCH-669, ORCH-671, ORCH-674

use ferb_orchestrator::models::Condition;
use ferb_orchestrator::verify::{ConditionResult, BY_MAC, BY_RECORD, DIFFERENT};

/// ORCH-672, D2 (off, as the owner chose: APP-956): whether a done answer whose proof is the checker's look at the
/// page (no receipt of FERB's own) carries a line saying so. Off: the basis is kept in the task's details (its
/// verification `basis`, `ferb trace`). Turning it on adds [`checked_line`] under such an answer.
// decision: APP-956
pub const HOW_CHECKED_IN_CHAT: bool = false;

/// ORCH-668, D3 (as the owner chose: APP-955): a result FERB reached on something other than what the owner named
/// ends its own way, as delivered with gaps ("Not done: <condition> — <what was done instead>"), never
/// done and never a bare failure. False: it ends as not done, with the same sentence.
// decision: APP-955
pub const DIFFERENT_ENDS_DELIVERED: bool = true;

/// The B line (off by [`HOW_CHECKED_IN_CHAT`]).
pub fn checked_line(site: &str) -> String {
    format!("I checked this on {site}'s page afterwards.")
}

/// F: the outcome holds and FERB did nothing in the step that could have done it.
pub const ALREADY_DONE: &str = "This was already done, so I didn't need to do anything.";

/// H: an action whose outcome FERB couldn't tell, never retried without the owner (the ledger's unknown).
pub fn unknown_line(places: &[String]) -> String {
    let at = match places {
        [] => String::new(),
        [one] => format!(" on {one}"),
        [rest @ .., last] => format!(" on {} and {last}", rest.join(", ")),
    };
    format!(
        "I'm not sure whether my last step{at} went through: I couldn't tell how it ended. I haven't tried it again, \
         so nothing can happen twice."
    )
}

fn name(path: &str) -> String {
    let n = ferb_fs::pypath::name(path);
    if n.is_empty() {
        path.to_string()
    } else {
        n
    }
}

fn names(list: &str) -> String {
    list.split(", ")
        .filter(|p| !p.is_empty())
        .map(name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// D: what FERB did instead of what the owner named, as the reason of its "Not done:" line (ORCH-668) and of the
/// question before a later send or submit (ORCH-669). From the result's `asked` and `acted_on` only.
pub fn different_reason(c: &Condition, r: &ConditionResult) -> String {
    let asked = name(&r.asked);
    match c.check.kind.as_str() {
        "uploaded" if r.by == BY_RECORD => format!(
            "{asked} wasn't uploaded from your Mac, so the file the site has came from somewhere else, even if its \
             name is the same"
        ),
        "uploaded" if r.acted_on.starts_with("different bytes") => {
            format!("the page's file field holds different bytes than {asked}")
        }
        "uploaded" => format!("I uploaded {} instead of {asked}", names(&r.acted_on)),
        "sent" => format!("it went to {}, not to {}", r.acted_on, r.asked),
        "trashed" if r.acted_on == "deleted for good" => {
            let n = name(c.check.path.as_deref().unwrap_or(""));
            format!("{n} was deleted for good instead of moved to the Trash")
        }
        "deleted" if r.acted_on == "moved to the Trash" => {
            let n = name(c.check.path.as_deref().unwrap_or(""));
            format!("{n} was moved to the Trash instead of deleted for good; it's still in the Trash")
        }
        "trashed" => format!(
            "I moved {} to the Trash instead; {asked} is still there",
            names(&r.acted_on)
        ),
        "deleted" => format!(
            "I deleted {} instead; {asked} is still there",
            names(&r.acted_on)
        ),
        _ => format!("I acted on {} instead", r.acted_on),
    }
}

/// E: the opener's sentence when code's own fact proves an action wasn't done (ORCH-664), from the fact's kind.
pub fn not_done_line(c: &Condition, r: &ConditionResult) -> String {
    if r.verdict == DIFFERENT {
        return format!("{}.", capital(&different_reason(c, r)));
    }
    let n = name(c.check.path.as_deref().unwrap_or(""));
    match (r.by.as_str(), c.check.kind.as_str()) {
        (BY_RECORD, "uploaded") => format!("I didn't upload {n} from your Mac."),
        (BY_RECORD, _) => match c.check.site.as_deref().filter(|s| !s.is_empty()) {
            Some(site) => format!(
                "I didn't do this: I only looked at {site} and didn't press, type or send anything there."
            ),
            None => "I didn't do this: I only looked, and didn't press, type or send anything.".into(),
        },
        (BY_MAC, "trashed") => format!("{n} is still in its folder; it wasn't moved to the Trash."),
        (BY_MAC, "deleted") => format!("{n} is still on your Mac; it wasn't deleted."),
        (BY_MAC, "downloaded") => {
            format!("{n} didn't download: nothing was downloaded in this task, and it isn't on your Mac.")
        }
        _ => String::new(),
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// ORCH-671: the checker's second chance when its pass had no look of its own after the step's last action
/// (model-facing, protected).
pub fn refused_look(at: &str) -> String {
    format!(
        "FERB refused this check's pass: the step's last action was on {at}, and this check hasn't looked at it since. \
         Before judging a condition met, look at it now (a snapshot or read of that tab, app or page); give met=true \
         only for what you see, and leave it unknown if you can't look."
    )
}

/// ORCH-671: a met that had no look after the step's last action, kept as can't tell (stored, never the owner's).
pub fn no_look_at(at: &str) -> String {
    format!(
        "This check made no look of its own at {at} after the step's last action, so its pass doesn't stand."
    )
}

/// ORCH-669: the worker's (and the check's) code fact when a condition names a file on the Mac to upload
/// (model-facing, protected): what carries that file, and to stop and ask before sending another one.
pub fn upload_fact(paths: &[String]) -> String {
    format!(
        "The owner named a file on this Mac to upload: {}. Only FERB's upload (browser.act with action upload and \
         files naming that path) carries it from the Mac; a file chosen in a site's own picker (its cloud drive, its \
         recent files) is another file, even with the same name. Before a send or submit with a file attached, make \
         sure the attachment came from that upload; if it didn't, don't send or submit: report not done with a \
         question for the owner.",
        paths.join(", ")
    )
}
