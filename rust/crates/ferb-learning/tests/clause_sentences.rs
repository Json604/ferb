//! MEM-431: the owner's words saved for a remembered fact stop where the sentence holding the planner's content ends,
//! as well as at a clause mark; a period that closes an abbreviation, or sits inside a word, never cuts one.

use ferb_learning::text::{capture, owner_clauses, whole_clauses};

// decision: MEM-431
#[test]
fn a_remembered_fact_never_takes_the_sentences_after_it() {
    // live 2026-11-15 23:04: the planner quoted only the roll number; the following sentences were saved with it
    let owner = "start 3 parallel agents, my name: Kartikey, roll no: 2024eb02238 . THEY SHOULNT MENTION AI or \
                 anuytign tthere. they are dpoing this assingment as me, so do eveyrhgin as said";
    let got = capture(owner, "roll no: 2024eb02238", "roll no: 2024eb02238");
    assert_eq!(got.origin, "owner_stated");
    assert_eq!(got.content, "roll no: 2024eb02238");
    // the same with the period against the number, and with lower-case sentences
    let got = capture(
        "my roll no is 2024eb02238. they shouldn't mention it, ok",
        "my roll no is 2024eb02238",
        "my roll no is 2024eb02238",
    );
    assert_eq!(got.content, "my roll no is 2024eb02238");
    assert_eq!(
        owner_clauses("i live in Delhi. They should not say so", "i live in Delhi").as_deref(),
        Some("i live in Delhi")
    );
    assert_eq!(
        owner_clauses("we met. my dentist is Asha now", "my dentist is Asha").as_deref(),
        Some("my dentist is Asha now")
    );
    // a whole sentence is now a whole clause (a forget's check)
    assert!(whole_clauses(
        "my name is Ashwini. I live in Pune",
        "I live in Pune"
    ));
}

// decision: MEM-431
#[test]
fn abbreviations_decimals_and_addresses_never_cut_a_clause() {
    for (text, part, want) in [
        (
            "my dentist is Dr. Zorblat",
            "my dentist",
            "my dentist is Dr. Zorblat",
        ),
        (
            "Dr. Rao is my dentist",
            "Rao is my dentist",
            "Dr. Rao is my dentist",
        ),
        (
            "my prof. Zorblat teaches on Mondays",
            "Zorblat teaches on Mondays",
            "my prof. Zorblat teaches on Mondays",
        ),
        (
            "I like e.g. blue shirts",
            "I like",
            "I like e.g. blue shirts",
        ),
        (
            "meet at 9 a.m. Tuesday",
            "meet at 9",
            "meet at 9 a.m. Tuesday",
        ),
        (
            "my roll no. 5 is fixed",
            "my roll",
            "my roll no. 5 is fixed",
        ),
        (
            "my site is kartikey.xyz and runs v2.1 now",
            "my site",
            "my site is kartikey.xyz and runs v2.1 now",
        ),
        (
            "my site is kartikey.xyz. Ask before posting there",
            "my site",
            "my site is kartikey.xyz",
        ),
    ] {
        assert_eq!(owner_clauses(text, part).as_deref(), Some(want), "{text}");
    }
    // a negation in the fact's own sentence stays
    assert_eq!(
        owner_clauses(
            "Never share my links publicly. Thanks a lot",
            "share my links publicly"
        )
        .as_deref(),
        Some("Never share my links publicly")
    );
}
