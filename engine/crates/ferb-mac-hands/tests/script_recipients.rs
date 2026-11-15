//! `script_recipients` (GATE-588): the addresses an AppleScript writes in its strings, by the policy's shape check
//! (here a stand-in that takes a whole email-shaped word, as `typed_destination` does for these inputs).

use ferb_mac_hands::tools::script_recipients;

fn email(t: &str) -> Option<String> {
    let t = t.trim().trim_matches(|c: char| ",;<>()".contains(c));
    (t.contains('@') && !t.contains(char::is_whitespace) && t.contains('.')).then(|| t.to_string())
}

// decision: GATE-588
#[test]
fn each_string_literals_addresses_once_in_order() {
    let script = "set m to make new outgoing message with properties {subject:\"Hi \\\"you\\\"\", content:\"write to \
                  b@example.org, thanks\"}\ntell m to make new to recipient with properties {address:\"a@example.com\"}\n\
                  tell m to make new cc recipient with properties {address:\"b@example.org\"}\nsend m";
    assert_eq!(
        script_recipients(script, &email),
        vec!["b@example.org".to_string(), "a@example.com".to_string()]
    );
    // no strings, or an unclosed one: nothing
    assert!(script_recipients("send m", &email).is_empty());
    assert!(script_recipients("set x to \"a@example.com", &email).is_empty());
}
