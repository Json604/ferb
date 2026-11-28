//! ORCH-738: the line a done answer with something unconfirmed ends with keeps code's fixed opening; what the check
//! named goes in one short bracket on one line, and a missing, empty or overlong one leaves today's line.

use ferb_supervisor::words::{unconfirmed_line, UNCONFIRMED_LINE, UNCONFIRMED_SHORT_WORDS};

// decision: ORCH-738
#[test]
fn the_bracket_is_one_short_line_or_nothing() {
    assert_eq!(
        unconfirmed_line("whether the form was submitted"),
        "I couldn't confirm part of this (whether the form was submitted)."
    );
    assert_eq!(
        unconfirmed_line("  the price (on the seller's page)\n[today].  "),
        "I couldn't confirm part of this (the price on the seller's page today)."
    );
    assert_eq!(unconfirmed_line(""), UNCONFIRMED_LINE);
    assert_eq!(unconfirmed_line(" ( ) \n "), UNCONFIRMED_LINE);
    let long = vec!["word"; UNCONFIRMED_SHORT_WORDS + 1].join(" ");
    assert_eq!(unconfirmed_line(&long), UNCONFIRMED_LINE);
    let most = vec!["word"; UNCONFIRMED_SHORT_WORDS].join(" ");
    assert!(unconfirmed_line(&most).starts_with("I couldn't confirm part of this (word"));
    assert_eq!(unconfirmed_line(&"x".repeat(200)), UNCONFIRMED_LINE);
}
