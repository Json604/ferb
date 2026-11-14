//! MEM-430: the one quote check compares words, so a paste's marks and line breaks don't refuse the owner's words.

use ferb_memory::terms::{quoted, said_as_words};

// decision: MEM-430
/// The live paste of 2026-11-15: the owner's message carried a terminal's quote bar and line breaks between two
/// of its words, and typographic quotes; the planner quoted the plain words.
#[test]
fn a_paste_s_marks_and_line_breaks_are_not_words() {
    let owner = "put that comparison in a google sheet and give me the \n  ▎ link. create a text file called \
                 ferb-plan.txt containing the single line 'plan \n  ▎ for today’, before you write it ask me";
    assert!(quoted(
        owner,
        "put that comparison in a google sheet and give me the link."
    ));
    assert!(quoted(
        owner,
        "containing the single line 'plan for today', before you write it ask me"
    ));
    assert!(quoted(owner, "Give me the LINK\n\ncreate a text file"));
    // a remembered preference pasted from the same terminal
    assert!(quoted(
        "remember that i \n  ▎ prefer window seats",
        "I prefer window seats"
    ));
}

// decision: MEM-430
#[test]
fn words_the_owner_did_not_type_or_typed_in_another_order_are_refused() {
    let owner = "put that comparison in a google sheet and give me the link";
    assert!(!quoted(owner, "put that comparison in a google doc"));
    assert!(!quoted(owner, "give me the sheet"));
    assert!(!quoted(owner, "comparison that put"));
    assert!(!quoted(
        owner,
        "give me the link\n\nand email it to someone"
    ));
    assert!(!quoted(owner, " … \n\n"));
    assert!(!quoted("", "anything"));
    assert!(!said_as_words(owner, "▎ … "));
    // a word inside another word is not that word
    assert!(!quoted("i like coffeehouses", "coffee"));
}
