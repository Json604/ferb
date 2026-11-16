//! GATE-608: a site's one answer per task covers the form's and the link's question for the same exact host, and
//! nothing else: every other question's key has no partner (payments, account security, new recipients, deletions,
//! terminals, foreground input keep their own answers).

use ferb_policy::confirm::{self, destination_key, sent_key, site_partner, SINK_DESTINATION};
use serde_json::json;

// decision: GATE-608
#[test]
fn the_form_and_the_link_key_of_one_host_answer_each_other() {
    assert_eq!(
        site_partner(&sent_key("github.com")),
        Some(destination_key("github.com"))
    );
    assert_eq!(
        site_partner(&destination_key("github.com")),
        Some(sent_key("github.com"))
    );
    // the exact host string: no www. or registrable-domain widening
    assert_ne!(
        site_partner(&sent_key("github.com")),
        Some(destination_key("www.github.com"))
    );
    assert_eq!(
        sent_key("github.com"),
        confirm::key(&[json!("sink:new_destination"), json!("github.com")])
    );
    assert_eq!(SINK_DESTINATION, "sink:destination");
}

// decision: GATE-608
#[test]
fn no_other_question_has_a_partner() {
    for parts in [
        json!(["floor:pay", "shop.example", null, 25.0, []]),
        json!(["floor:new_recipient", ["drop@collect.example"]]),
        json!(["floor:new_recipient", null, "site:github.com"]),
        json!(["floor:delete_permanent", "/Users/o/a.txt"]),
        json!(["floor:account_security", "github.com"]),
        json!(["terminal_app", "com.apple.terminal"]),
        json!(["sink:new_destination", null]),
        json!(["sink:new_destination", ""]),
        json!(["sink:new_destination", "github.com", "extra"]),
        json!(["sink:outside_value", "github.com"]),
    ] {
        let key = confirm::key(parts.as_array().unwrap());
        assert_eq!(site_partner(&key), None, "{key}");
    }
    assert_eq!(site_partner("not json"), None);
}
