//! ferb/orchestrator/attempts.py: the attempt count behind the check-in (ORCH-307). Counts only the attempts the
//! checker chose to retry; it never judges one.
//!
//! decision: ORCH-307

/// The owner's number (2026-10-30); changing it is the owner's decision.
pub const CHECK_IN_AFTER: u32 = 5;
/// ORCH-707 (the owner's number to change): attempts in a row in one step that couldn't run because something broke
/// inside FERB, after which FERB asks the owner instead of trying again by itself. Such an attempt never counts toward
/// CHECK_IN_AFTER.
pub const INTERNAL_ERRORS_IN_A_ROW: u32 = 3;
pub const KEEP_GOING: &str = "Yes";
pub const STOP_HERE: &str = "No";

/// attempts.py:17 `Attempts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attempts {
    pub failed: u32,
    pub allowed: u32,
}

impl Default for Attempts {
    fn default() -> Attempts {
        Attempts {
            failed: 0,
            allowed: CHECK_IN_AFTER,
        }
    }
}

impl Attempts {
    /// One more attempt failed and another is about to start: whether the check-in is due before it.
    pub fn failed_one(&mut self) -> bool {
        self.failed += 1;
        self.failed >= self.allowed
    }

    /// The owner said keep going: CHECK_IN_AFTER more attempts before the next check-in.
    pub fn allow_more(&mut self) {
        self.allowed = self.failed + CHECK_IN_AFTER;
    }
}

/// attempts.py:33 `check_in_question`.
pub fn check_in_question(failed: u32, goal: &str) -> String {
    format!("I've tried {failed} times and haven't got this done yet: {goal}. Keep going?")
}

/// attempts.py:38 `stopped_after`.
pub fn stopped_after(failed: u32, unattended: bool) -> String {
    if unattended {
        format!(
            "I tried {failed} times without getting it done, and nobody was there to ask whether to keep going, \
             so I stopped here."
        )
    } else {
        format!("You asked me to stop after {failed} attempts.")
    }
}

/// ORCH-707: the question after INTERNAL_ERRORS_IN_A_ROW attempts in a row that couldn't run, with what the last one
/// said (the worker's reason, e.g. "something broke inside FERB (...)"); the answers are ORCH-679's.
pub fn internal_errors_question(times: u32, why: &str) -> String {
    let why = crate::py::strip(why).trim_end_matches('.');
    format!(
        "FERB couldn't run this step {times} times in a row because of a problem of its own, not anything you asked. \
         The last time: {why}. {}",
        crate::rabbit_holes::ASK_TAIL
    )
}
