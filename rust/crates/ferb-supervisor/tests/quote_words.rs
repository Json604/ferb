//! ORCH-646: a part's quote compared as words.

use ferb_supervisor::words::said_as_words;

// decision: ORCH-646
/// The live paste of 2026-11-15: the owner's message carried a terminal's quote bar and line breaks between two
/// of its words, and typographic quotes; the planner quoted the plain words.
#[test]
fn a_paste_s_marks_and_line_breaks_are_not_words() {
    let owner = "put that comparison in a google sheet and give me the \n  ▎ link. create a text file called \
                 ferb-plan.txt containing the single line 'plan \n  ▎ for today’, before you write it ask me";
    assert!(said_as_words(
        owner,
        "put that comparison in a google sheet and give me the link."
    ));
    assert!(said_as_words(
        owner,
        "containing the single line 'plan for today', before you write it ask me"
    ));
    assert!(said_as_words(
        owner,
        "Give me the LINK\n\ncreate a text file"
    ));
}

#[test]
fn words_the_owner_did_not_type_or_typed_in_another_order_are_refused() {
    let owner = "put that comparison in a google sheet and give me the link";
    assert!(!said_as_words(owner, "put that comparison in a google doc"));
    assert!(!said_as_words(owner, "give me the sheet"));
    assert!(!said_as_words(owner, "comparison that put"));
    assert!(!said_as_words(
        owner,
        "give me the link\n\nand email it to someone"
    ));
    assert!(!said_as_words(owner, " … \n\n"));
    assert!(!said_as_words("", "anything"));
}
