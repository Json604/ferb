//! The owner's choices about personal identifiers (GATE-601): a "don't mask this" grant for one value, kept by the
//! owner (GATE-602); which saved values count as identifiers (GATE-603); the identifiers of the owner's own chat
//! message, found to be kept sealed (GATE-604); contact data and technical identifiers left visible (GATE-599).
//! Every value here is invented: the numbers pass their check digit and belong to no one.

use ferb_sanitize::{identifier_kind, PyStr, Redactor, REDACTED};
use serde_json::json;

/// An invented PAN (issuer layout: five letters, the fourth a holder type, four digits, a letter).
const PAN: &str = "ABCPE1234F";
/// An invented Aadhaar number in its printed 4-4-4 groups (Verhoeff-valid).
const AADHAAR: &str = "2345 6789 0124";

// decision: GATE-602
#[test]
fn a_granted_identifier_shows_and_another_of_its_kind_stays_masked() {
    let r = Redactor::default();
    let text = format!("my PAN is {PAN} and my brother's is ABCPE1234G; Aadhaar {AADHAAR}");
    let before = r.text(&text);
    assert!(
        !before.contains(PAN) && !before.contains("2345 6789"),
        "{before}"
    );
    // the grant is for one value, written any way the owner writes it
    assert_eq!(r.show_identifiers(["abcpe-1234-f"]), 1);
    let after = r.text(&text);
    assert!(after.contains(PAN), "{after}");
    assert!(
        !after.contains("ABCPE1234G"),
        "another value of the same kind stays masked: {after}"
    );
    assert!(!after.contains("2345 6789"), "{after}");
    // the notice names only what was hidden
    assert_eq!(r.kinds(&text).unwrap(), vec!["PAN", "Aadhaar number"]);
    // a record's key that names the identifier: the granted value shows, another stays masked
    let shown = r.value(&json!({"pan": PAN, "aadhaar": "234567890124"}));
    assert_eq!(shown, json!({"pan": PAN, "aadhaar": REDACTED}));
    // removed: masked again, also a string this redactor returned while the grant held
    let marked = r.text_marked(&PyStr::new(text.as_str()));
    assert!(marked.text.contains(PAN));
    r.show_identifiers(std::iter::empty::<&str>());
    assert!(!r.text(&text).contains(PAN));
    assert!(
        !r.text_marked(&marked).text.contains(PAN),
        "a mark from before the change is not trusted"
    );
}

// decision: GATE-602
#[test]
fn a_grant_never_unmasks_what_another_rule_masks() {
    let r = Redactor::default();
    // an Aadhaar-shaped run that is also a Luhn-valid card number stays masked as a card
    let card = "4111 1111 1111";
    r.show_identifiers([card, "4111111111111111", PAN]);
    assert!(!r
        .text("card 4111 1111 1111 1111")
        .contains("4111 1111 1111 1111"));
    // after a password label the value is a secret, granted or not
    assert!(!r.text(&format!("password: {PAN}")).contains(PAN));
    // the same value as an identifier shows
    assert!(r.text(&format!("PAN: {PAN}")).contains(PAN));
}

// decision: GATE-603
#[test]
fn a_value_is_an_identifier_only_by_its_own_layout_and_check() {
    assert_eq!(identifier_kind(PAN), Some("PAN"));
    assert_eq!(
        identifier_kind(&format!("  {AADHAAR} ")),
        Some("Aadhaar number")
    );
    // a check digit off: not one
    assert_eq!(identifier_kind("2345 6789 0125"), None);
    // label-only kinds have no layout of their own: a passport-like value alone is not one
    assert_eq!(identifier_kind("K1234567"), None);
    // a password, a card number, a value with more around it
    for value in [
        "hunter2",
        "4111 1111 1111 1111",
        "PAN ABCPE1234F",
        "ABCPE1234F ok",
    ] {
        assert_eq!(identifier_kind(value), None, "{value}");
    }
}

// decision: GATE-604
#[test]
fn the_identifiers_of_a_message_are_found_with_their_kinds_and_only_those() {
    let r = Redactor::default();
    let text =
        format!("Use PAN {PAN}, Aadhaar {AADHAAR}, password hunter2 and card 4111 1111 1111 1111.");
    let found = r.identifiers(&text).unwrap();
    let got: Vec<(&str, &str)> = found.iter().map(|&(a, b, k)| (&text[a..b], k)).collect();
    assert_eq!(got, vec![(PAN, "PAN"), (AADHAAR, "Aadhaar number")]);
    // a granted one is shown, so it is not kept sealed
    r.show_identifiers([PAN]);
    let found = r.identifiers(&text).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].2, "Aadhaar number");
}

// decision: GATE-601
#[test]
fn contact_data_and_technical_identifiers_stay_visible() {
    let r = Redactor::default();
    for text in [
        "Asha Verma",
        "asha.verma@example.com",
        "+91 98765 43210",
        "(415) 555-0134",
        "14 Park Street, Kolkata 700016",
        "born 1990-04-12",
        "DOB: 12/04/1990",
        "server 192.168.1.20",
        "fe80::1ff:fe23:4567:890a",
        "MAC 00:1A:2B:3C:4D:5E",
    ] {
        assert_eq!(r.text(text), text);
    }
}
