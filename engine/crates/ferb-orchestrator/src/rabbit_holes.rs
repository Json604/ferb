//! The owner's "no rabbit holes" rule (ORCH-679): FERB asks the owner "Keep trying, or go on with what I have?" instead
//! of digging on by itself. The check only labels what is left of each unmet condition (verifier.md's `left` and
//! `progress`); code applies the rule from those labels alone:
//!
//! - a `retry` after which a condition labelled guess, search or stuck, already retried in this step and with some or
//!   most of it reached, would be dug into again becomes the question;
//! - an `accept_with_gaps` that code refuses only because the worker reported not done (ORCH-675's limit, ORCH-681)
//!   becomes the question, where today's code ended it failed.
//!
//! - ORCH-690 (owner, 2026-11-22: "Yes, always ask me"): a retry, or a refused gapped accept, while what is left is
//!   the owner's own step becomes the question at once: a condition the check labelled `owner`, or a tool's
//!   `owner_step` fact still standing in the attempt (code's own fact, whatever the check labelled);
//! - ORCH-691 (owner, 2026-11-22: "8 failed actions or 4 min"): a retry, or a refused gapped accept, after FERB's
//!   in-attempt guard stopped the attempt (`stuck`) becomes the question at once.
//!
//! ORCH-680: the rule never reads words, and its only outcomes are the checker's own choice, unchanged, or the
//! question. A missing or malformed label, no `progress` to ask with, a condition code's own facts proved false, or
//! the owner having said "keep trying" in this step leaves the checker's choice as it is.
//!
//! decision: ORCH-679, ORCH-680, ORCH-681, ORCH-690, ORCH-691

/// FERB's fixed words after the check's own two sentences (the owner's phrasing, ORCH-679).
pub const ASK_TAIL: &str = "Keep trying, or go on with what I have?";
/// The question's two choices.
pub const KEEP_TRYING: &str = "Keep trying";
pub const GO_ON: &str = "Go on with what I have";

/// What the check says is left of one unmet condition (`left.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Fix,
    Guess,
    Search,
    Stuck,
    Owner,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Fix,
        Kind::Guess,
        Kind::Search,
        Kind::Stuck,
        Kind::Owner,
    ];

    /// The label as the check wrote it; None for anything else.
    pub fn of(label: &str) -> Option<Kind> {
        match label {
            "fix" => Some(Kind::Fix),
            "guess" => Some(Kind::Guess),
            "search" => Some(Kind::Search),
            "stuck" => Some(Kind::Stuck),
            "owner" => Some(Kind::Owner),
            _ => None,
        }
    }

    /// Guess, search and stuck: more digging with no known end.
    pub fn digs(self) -> bool {
        matches!(self, Kind::Guess | Kind::Search | Kind::Stuck)
    }
}

/// How much of the condition the work reached (`left.reached`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reached {
    None,
    Some,
    Most,
}

impl Reached {
    pub const ALL: [Reached; 3] = [Reached::None, Reached::Some, Reached::Most];

    pub fn of(label: &str) -> Option<Reached> {
        match label {
            "none" => Some(Reached::None),
            "some" => Some(Reached::Some),
            "most" => Some(Reached::Most),
            _ => None,
        }
    }
}

/// One unmet condition as the rule sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unmet {
    /// The check's labels; None when missing or malformed.
    pub kind: Option<Kind>,
    pub reached: Option<Reached>,
    /// This step already took a retry while this condition was unmet.
    pub retried: bool,
    /// Code's own facts proved it false (ORCH-664).
    pub proved_false: bool,
}

/// What the check chose, as code holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checked<'a> {
    /// The checker's `next`.
    pub next: &'a str,
    /// With `next` accept_with_gaps: code's limits refused it only because the worker reported not done.
    pub refused_not_done: bool,
    /// The check gave `progress` with a missing sentence (the question and the "Not done:" line need it).
    pub said: bool,
    /// The owner already said "keep trying" to this question in this step.
    pub owner_kept: bool,
    /// ORCH-690: a tool's `owner_step` fact stands in this attempt (the last call of that tool reported it).
    pub owner_step: bool,
    /// ORCH-691: FERB's in-attempt guard stopped this attempt (8 failed or unchanged on-screen actions on one app,
    /// or 4 minutes on the step without progress).
    pub stuck: bool,
}

/// What code does with the check's choice. There is no other outcome (ORCH-680).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// The checker's own choice, as today's code takes it.
    AsChecked,
    /// The question to the owner, in its place.
    Ask,
}

/// The rule.
pub fn rule(checked: &Checked, unmet: &[Unmet]) -> Rule {
    let gapped = checked.next == "accept_with_gaps" && checked.refused_not_done;
    // ORCH-691: each time the guard stops an attempt it asks, an earlier "keep trying" in the step included
    let kept = checked.owner_kept && !checked.stuck;
    if !(checked.next == "retry" || gapped) || kept || !checked.said {
        return Rule::AsChecked;
    }
    if unmet.is_empty() || unmet.iter().any(|u| u.proved_false) {
        return Rule::AsChecked;
    }
    if gapped {
        // the check itself judged that more work can't meet it: the owner decides instead of code failing it
        return Rule::Ask;
    }
    // ORCH-690, ORCH-691: the owner's step, or an attempt the guard stopped: asked at once, no retry first
    if checked.owner_step || checked.stuck || unmet.iter().any(|u| u.kind == Some(Kind::Owner)) {
        return Rule::Ask;
    }
    if unmet
        .iter()
        .any(|u| u.kind.is_none() || u.reached.is_none())
    {
        return Rule::AsChecked;
    }
    let due = unmet.iter().any(|u| {
        u.retried && u.kind.is_some_and(Kind::digs) && u.reached.is_some_and(|r| r >= Reached::Some)
    });
    if due {
        Rule::Ask
    } else {
        Rule::AsChecked
    }
}

/// ORCH-690: whether a tool's `owner_step` fact stands after these calls of the attempt, in order: (tool, its
/// output's `owner_step` was set). The latest call of each tool decides: a later call of the same tool without it (the
/// pairing that went through once the owner switched FERB on) takes it back.
pub fn owner_step_stands<'a>(calls: impl IntoIterator<Item = (&'a str, bool)>) -> bool {
    let mut last: std::collections::BTreeMap<&str, bool> = std::collections::BTreeMap::new();
    for (tool, stands) in calls {
        last.insert(tool, stands);
    }
    last.values().any(|s| *s)
}

/// The question: the check's two sentences, then FERB's fixed words. Each sentence ends with its stop.
pub fn question(found: &str, missing: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for t in [found, missing] {
        let t = crate::py::strip(t);
        if t.is_empty() {
            continue;
        }
        let t = if t.ends_with(['.', '!', '?']) {
            t.to_string()
        } else {
            format!("{t}.")
        };
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out.push(ASK_TAIL.to_string());
    out.join(" ")
}
