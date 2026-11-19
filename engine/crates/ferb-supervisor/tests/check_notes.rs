//! A check's question keeps its notes for the attempt after the owner's answer (ORCH-678).

use ferb_supervisor::flow::attempt::{check_notes, Next};

// decision: ORCH-678
#[test]
fn a_question_keeps_the_check_s_notes_for_the_attempt_after_the_answer() {
    let asked = Next {
        action: "ask".into(),
        question: "I found three addresses; keep trying, or go on with what I have?".into(),
        missing: "the fourth person's address".into(),
        instructions: "test the remaining generated addresses in the checker".into(),
        ..Next::default()
    };
    assert_eq!(
        check_notes(&asked),
        "\nWhat the check found missing: the fourth person's address\nWhat the check would try next, if the \
         owner's answer below wants more: test the remaining generated addresses in the checker"
    );
    // a question with no notes adds nothing: the attempt gets the owner's answer alone, as before
    let bare = Next {
        action: "ask".into(),
        question: "Which account?".into(),
        ..Next::default()
    };
    assert_eq!(check_notes(&bare), "");
}
