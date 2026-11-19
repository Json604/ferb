//! ORCH-677: a "Not done:" line is the check's reason alone, once per distinct reason; the parts that were done are
//! never listed as not done. A "different" gap and a step not started carry their own subject in their reason, so
//! their lines read as before.

use ferb_supervisor::flow::Gap;
use ferb_supervisor::lines::{after_gaps_question, gap_lines, with_gap_lines};

fn gap(index: i64, condition: &str, reason: &str) -> Gap {
    Gap {
        index,
        condition: condition.into(),
        reason: reason.into(),
        evidence: String::new(),
    }
}

// decision: ORCH-677
#[test]
fn one_cause_across_two_conditions_is_said_once_without_the_conditions() {
    let gaps = [
        gap(
            0,
            "Your draft includes three people with the right To and CC placement",
            "A recipient's address is in the draft twice (you asked me to stop before I removed it)",
        ),
        gap(
            3,
            "The main people are in To and the others in CC",
            "A recipient's address is in the draft twice (you asked me to stop before I removed it).",
        ),
    ];
    assert_eq!(
        gap_lines(&gaps),
        vec!["Not done: A recipient's address is in the draft twice (you asked me to stop before I removed it)."]
    );
    assert_eq!(
        with_gap_lines("Left the draft open, unsent.", &gaps),
        "Left the draft open, unsent.\n\nNot done: A recipient's address is in the draft twice (you asked me to stop \
         before I removed it)."
    );
}

// decision: ORCH-677
#[test]
fn two_causes_are_two_lines() {
    let gaps = [
        gap(
            0,
            "Five real participants took part",
            "only you can bring real participants",
        ),
        gap(
            1,
            "The report cites the course's rubric",
            "The rubric isn't on the course page",
        ),
    ];
    assert_eq!(
        gap_lines(&gaps),
        vec![
            "Not done: only you can bring real participants.",
            "Not done: The rubric isn't on the course page."
        ]
    );
}

/// The reasons `different_ending` and a step not started write carry their condition or step, so those lines are the
/// same as before ORCH-677.
// decision: ORCH-677, ORCH-668, ORCH-653
#[test]
fn a_different_gap_and_a_step_not_started_read_as_before() {
    let gaps = [
        gap(
            0,
            "report.pdf from your Mac is attached",
            "report.pdf from your Mac is attached — report.pdf wasn't uploaded from your Mac, so the file the site has \
             came from somewhere else, even if its name is the same",
        ),
        gap(
            -1,
            "Submit the form",
            "Submit the form — not started: you said not to go on after the parts not done",
        ),
    ];
    assert_eq!(
        gap_lines(&gaps),
        vec![
            "Not done: report.pdf from your Mac is attached — report.pdf wasn't uploaded from your Mac, so the file the \
             site has came from somewhere else, even if its name is the same.",
            "Not done: Submit the form — not started: you said not to go on after the parts not done."
        ]
    );
}

// decision: ORCH-677, ORCH-653
#[test]
fn the_question_before_the_next_step_names_each_reason_once() {
    let gaps = vec![
        gap(
            0,
            "The offices are listed",
            "the cities are invented, so no office exists.",
        ),
        gap(
            1,
            "Each office has its hours",
            "the cities are invented, so no office exists",
        ),
    ];
    assert_eq!(
        after_gaps_question(&[("Find the offices".into(), gaps)], "Submit the form"),
        "Find the offices finished with these parts not done: the cities are invented, so no office exists. Go on to \
         Submit the form?"
    );
}
