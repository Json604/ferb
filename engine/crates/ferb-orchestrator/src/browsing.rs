//! ferb/orchestrator/browsing.py: the words of where FERB browses when it has no browser profile of its own
//! (APP-777). Words only: the daemon decides.

pub const BROWSING_ASK: &str = "browsing.choose";
pub const NONE: &str = "None";
pub const KIND: &str = "browsing";
pub const ASKED: &str = "owner_profile_asked";

pub const QUESTION_TITLE: &str = "Which browser profile should FERB use for web work?";
pub const QUESTION_DETAIL: &str = "FERB has no profile of its own. It works there in the background, as you: sites see your accounts and its searches show in that profile's history. Risky steps still ask you. With None, web tasks are slower and often fail. Change it any time in Settings → Browsers.";
pub const GONE: &str = "The profile FERB used for web work, {label}, isn't there any more. ";
pub const CHOSEN: &str =
    "FERB will do its web work in {label} from now on. You can change it in Settings → Browsers.";
pub const NONE_CHOSEN: &str = "Done: FERB won't use any of your profiles for web work. Some tasks will be much slower and less reliable: without a browser, FERB fetches pages directly, and pages that need JavaScript, a sign-in or a bot check can't be read that way. You can pick a profile anytime in Settings → Browsers.";
pub const NO_BROWSER_RUN: &str = "This run had no browser: you haven't picked which of your profiles FERB uses for web work, so it read pages without one (slower and less reliable). I'll ask you the next time a task needs it.";
pub const GONE_RUN: &str = "This run had no browser: the profile FERB used for web work, {label}, isn't there any more, so it read pages without one (slower and less reliable). I'll ask you the next time a task needs it.";
/// ORCH-645: what an open browsing question's answer settles, beside it in the planner's open questions. HAND-580:
/// never a task whose own words named a profile (its task's `profiles`).
pub const QUESTION_SETTLES: &str = "the browser profile every web task uses when the owner's words name none for \
     it, this message's tasks included (a task whose words name a profile lists it in the task's profiles and \
     browses there); a plan never asks the owner which profile again while this is open, and FERB uses the answer \
     by itself";
/// ORCH-648: the planner's fact while the owner hasn't picked a profile for FERB's web work (was environment.py's "in
/// a profile the owner names (none chosen for it)", which led plans to ask which profile as a step of their own).
pub const NOT_PICKED_TAIL: &str = "in the profile the owner picks for it: none picked yet. The first time a task browses as the owner, FERB asks them once by itself which profile to use (a plan never asks it as a step) and uses that profile from then on, unless the owner's words name another one for a task.";
pub const UNASKED_FACT: &str = "not set up; when a task does web work as FERB, FERB asks the owner once which of their browser profiles to use, and works there as the owner";
pub const CHOSEN_FACT: &str =
    "not set up; web work as FERB runs as the owner in their {label}, the profile they picked";
pub const NONE_FACT: &str = "not set up, and the owner chose to give FERB none of their profiles: web work as FERB has no browser, so pages are fetched directly, pages that need JavaScript, a sign-in or a bot check can't be read, and it is slower and less reliable. The owner can pick a profile in Settings → Browsers";

/// `QUESTION`: the title, a newline, the why line (GATE-566).
pub fn question() -> String {
    format!("{QUESTION_TITLE}\n{QUESTION_DETAIL}")
}

/// `str.format(label=...)` of one of the `{label}` texts above.
pub fn with_label(text: &str, label: &str) -> String {
    crate::py::format_named(text, &[("label", label)])
}
