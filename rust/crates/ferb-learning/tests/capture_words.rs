//! MEM-430: a remembered item's quote is accepted by words, and what is saved is still the owner's own clause or,
//! when the owner's words don't hold it as written, FERB's understanding (never "your words" from a loosened match).

use ferb_learning::text::{capture, quoted};

// decision: MEM-430
#[test]
fn a_pasted_preference_is_accepted_and_saved_as_written_only_when_the_clause_is_the_owners() {
    // the quote bar a terminal paste carries sits between two of the owner's words
    let owner = "remember this: i always \n  ▎ prefer window seats, and book early";
    let got = capture(
        owner,
        "I always prefer window seats",
        "i always prefer window seats",
    );
    assert_eq!(got.refused, "", "the quote is the owner's words");
    // the owner's exact clause doesn't hold the content as written (a mark sits inside), so it is not saved as
    // the owner's words
    assert_eq!(got.origin, "inferred");
    assert_eq!(got.content, "I always prefer window seats");

    // a clean message: the saved text is the owner's own clause, as before
    let owner = "remember this: i always prefer window seats, and book early";
    let got = capture(
        owner,
        "always prefer window seats",
        "i always prefer window seats",
    );
    assert_eq!(got.origin, "owner_stated");
    assert_eq!(got.content, "i always prefer window seats");
}

// decision: MEM-430
#[test]
fn words_not_typed_are_still_refused_and_a_forget_s_clause_rule_is_unchanged() {
    let owner = "i always \n  ▎ prefer window seats";
    let got = capture(owner, "prefers aisle seats", "i prefer aisle seats");
    assert_eq!(got.refused, "quote not in the owner's words");
    // the clause form (a forget's rule) still compares the owner's clause as written
    assert!(quoted(owner, "i always prefer window seats", false));
    assert!(!quoted(owner, "i always prefer window seats", true));
}
