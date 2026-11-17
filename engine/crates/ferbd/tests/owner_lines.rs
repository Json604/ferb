//! PHN-584, PHN-585, GATE-615: the owner's one line for a phone left unused for 30 days and for the end of "until I'm
//! back at my Mac" (ferbd compose::consumers::owner_line), posted once per ident through the notices path.

use ferbd::compose::consumers::owner_line;
use serde_json::{json, Map, Value};

fn m(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

/// PHN-585: the unused phone's line names it and says it stays paired and where to remove it; its ident changes
/// only when the phone was used again (its last use), so it is told once per unused stretch. GATE-615: an end is
/// told with its own words; a start, or an end with nothing to say, is not.
// decision: PHN-584
// decision: PHN-585
// decision: GATE-615
#[test]
fn the_owner_is_told_once_in_plain_words() {
    let (id, text) = owner_line(
        "phone.unused",
        &m(json!({"phone": "ep1", "name": "Pixel 9", "last_seen": "2026-10-01T09:00:00+05:30", "days": 30})),
        true,
    )
    .unwrap();
    assert_eq!(id, "phone-unused-ep1-2026-10-01T09:00:00+05:30");
    assert_eq!(
        text,
        "Your phone \u{201c}Pixel 9\u{201d} hasn't been used with FERB for 30 days. It stays paired; if you don't use \
         it any more, you can remove it in Settings \u{2192} Phone."
    );
    assert!(!text.contains("AI"));
    let ended =
        m(json!({"state": "ended", "why": "owner_input", "since": "S", "line": "Welcome back."}));
    assert_eq!(
        owner_line(ferb_gateway::screen_use::TOPIC, &ended, true),
        Some((
            "screen-until-back-S-owner_input".into(),
            "Welcome back.".into()
        ))
    );
    let started = m(json!({"state": "started", "active": true}));
    assert_eq!(
        owner_line(ferb_gateway::screen_use::TOPIC, &started, true),
        None
    );
    let unsaid = m(json!({"state": "ended", "why": "answer_failed"}));
    assert_eq!(
        owner_line(ferb_gateway::screen_use::TOPIC, &unsaid, true),
        None
    );
}

/// GATE-618 (owner: "make sure user is shon ferb acknowledgeds them coming back, shodlnt seem it was a spam message"):
/// a return with no question shown gets the welcome line here; one during a hold is said by FERB.app in place of its
/// "Are you back?" line (here only when FERB.app isn't connected); a hold and a silent hold's end say nothing here.
// decision: GATE-618
// decision: PHN-593
#[test]
fn a_return_is_welcomed_once_and_a_silent_hold_says_nothing() {
    let line = ferb_gateway::screen_use::Ended::OwnerInput.line().unwrap();
    assert!(line.starts_with("Welcome back.") && !line.contains("AI"));
    let clear = m(
        json!({"state": "ended", "why": "owner_input", "since": "S", "line": line, "during_hold": false}),
    );
    assert_eq!(
        owner_line(ferb_gateway::screen_use::TOPIC, &clear, true).map(|(_, t)| t),
        Some(line.to_owned())
    );
    let held = m(
        json!({"state": "ended", "why": "owner_input", "since": "S", "line": line, "during_hold": true}),
    );
    assert_eq!(
        owner_line(ferb_gateway::screen_use::TOPIC, &held, true),
        None
    );
    assert!(owner_line(ferb_gateway::screen_use::TOPIC, &held, false).is_some());
    for state in ["checking", "resumed"] {
        let quiet = m(json!({"state": state, "active": true, "title": "Are you back?"}));
        assert_eq!(
            owner_line(ferb_gateway::screen_use::TOPIC, &quiet, true),
            None,
            "{state}"
        );
    }
}
