//! What the gateway reads about the owner at use (compose.py:111-124): the change record's read check, the owner's
//! own contacts (GATE-538) and the sources a saved username may still come from (L36 §6). Temporary folders only.

use std::collections::BTreeSet;

use ferbd::compose::gateway::{contacts_of, protection, reads_closed, source_holds};

// decision: GATE-538
/// memory.py:502-515: the accounts the owner's paired profiles are signed in to, and contacts the owner wrote about
/// themselves (owner stated) count; a contact FERB only read somewhere never does.
#[test]
fn the_owners_contacts_come_from_their_accounts_and_their_own_words() {
    let accounts = vec!["Owner.Invented@Example.test".to_string(), String::new()];
    let items = vec![
        (
            "owner_stated".to_string(),
            "my phone is +1 555 010 9999".to_string(),
        ),
        (
            "observed".to_string(),
            "someone@else.example wrote".to_string(),
        ),
    ];
    let got = contacts_of(&accounts, &items);
    assert_eq!(got, ["15550109999", "owner.invented@example.test"]);
}

/// memory.py:494-500 (`_username_sources`): a saved username's memory source holds only while that owner-stated item
/// does; any other source doesn't; without memory running, any source holds (Python's None).
#[test]
fn a_saved_usernames_source_holds_only_while_its_memory_item_does() {
    let ids: BTreeSet<String> = ["m-1".to_string()].into_iter().collect();
    assert!(source_holds(Some(Some(&ids)), "memory:m-1"));
    assert!(!source_holds(Some(Some(&ids)), "memory:m-2"));
    assert!(!source_holds(Some(Some(&ids)), "typed"));
    assert!(!source_holds(Some(None), "memory:m-1"));
    assert!(source_holds(None, "memory:m-2"));
}

/// compose.py:111: the change record reads only what the shell's sandbox may (`Protection.read_denied`): FERB's own
/// state and the never-read stores stay closed, the workspace and the owner's files are open.
#[test]
fn the_change_record_reads_what_the_protection_lists_allow() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let user_home = root.join("home");
    let home = user_home.join(".ferb");
    let app = root.join("FERB.app");
    for d in [&home.join("workspace"), &app] {
        std::fs::create_dir_all(d).unwrap();
    }
    let config = ferb_core::config::Config::new(&home);
    let lists = protection(&config, &user_home, &home, Some(&app)).expect("the lists load");
    let closed = reads_closed(lists);
    assert!(closed(&home.join("ferb.db")));
    assert!(closed(
        &user_home.join("Library/Keychains/login.keychain-db")
    ));
    assert!(!closed(&home.join("workspace").join("notes.txt")));
    assert!(!closed(&user_home.join("Documents").join("invented.txt")));
}
