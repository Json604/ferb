//! A private key whose lines are separate items (AGENTS.md rule 7: a secret never reaches a model, a trace, a log
//! or the database). Fixed against Python (ENG-6, redact.py:342, 393, 395): Python left a key body unredacted when
//! it sat under a non-text key, in a nested list or in bytes between the BEGIN and END items, read no node's text
//! as a key line, and in a dict never looked for a key when no value was a short text with a digit; it masked a
//! bare boolean between the lines. Here every text, integer and byte string from the BEGIN item to the END item is
//! masked at any depth, keys and shape kept, and what cannot carry a key's lines (a boolean, None, a float) stays.
//! The values are invented; the key lines are written in two parts for the commit gate's `secrets` check.
//!
//! decision: ENG-6

use ferb_sanitize::{Py, PyStr, Redactor, REDACTED};
use serde_json::{json, Value};

const BEGIN: &str = concat!("-----BEGIN RSA PRIV", "ATE KEY-----");
const END: &str = concat!("-----END RSA PRIV", "ATE KEY-----");
const BODY: &str = "QUJDZx9Qw3Er7Ty1Ui5Op2As";

fn redact(v: Value) -> Value {
    Redactor::default().value(&v)
}

fn shows_the_body(v: &Value) -> bool {
    v.to_string().contains(BODY)
}

#[test]
fn the_body_alone_is_not_a_secret_so_only_its_neighbours_can_hide_it() {
    // what makes the tests below mean something: the invented line passes as text on its own
    assert_eq!(Redactor::default().text(BODY), BODY);
    assert!(shows_the_body(&redact(json!([BODY, {"data": BODY}]))));
}

#[test]
fn a_body_under_a_non_text_key_between_the_items_is_masked() {
    assert_eq!(
        redact(json!([BEGIN, {"data": BODY}, END, "after"])),
        json!([REDACTED, {"data": REDACTED}, REDACTED, "after"])
    );
    // a node with text: every value, not only the node's text
    assert_eq!(
        redact(json!([BEGIN, {"text": "x", "data": BODY}, END])),
        json!([REDACTED, {"text": REDACTED, "data": REDACTED}, REDACTED])
    );
}

#[test]
fn lists_in_dicts_and_deeper_nesting_are_masked_with_their_shape_kept() {
    let out = redact(json!([
        BEGIN,
        {"data": {"deep": [BODY, {"more": [[BODY]]}], "n": 7, "f": 1.5, "ok": null, "on": true, "e": ""}},
        [[BODY], {"x": BODY}],
        END,
    ]));
    assert_eq!(
        out,
        json!([
            REDACTED,
            {"data": {"deep": [REDACTED, {"more": [[REDACTED]]}], "n": REDACTED, "f": 1.5, "ok": null, "on": true, "e": ""}},
            [[REDACTED], {"x": REDACTED}],
            REDACTED,
        ])
    );
}

#[test]
fn a_bare_boolean_none_and_float_between_the_items_stay() {
    // a text or an integer is masked whole, as Python does; a boolean cannot carry a key's lines
    assert_eq!(
        redact(json!([BEGIN, BODY, true, false, 5, 1.5, null, "", END])),
        json!([REDACTED, REDACTED, true, false, REDACTED, 1.5, null, REDACTED, REDACTED])
    );
}

#[test]
fn a_dict_s_values_are_read_as_key_lines_even_with_no_digit() {
    // Python returned before looking when no value was a short text with a digit
    assert_eq!(
        redact(json!({"a": BEGIN, "b": "QUJDREVGRw==", "c": END})),
        json!({"a": REDACTED, "b": REDACTED, "c": REDACTED})
    );
    assert_eq!(
        redact(json!({"a": BEGIN, "b": {"data": [BODY]}, "c": false, "d": END})),
        json!({"a": REDACTED, "b": {"data": [REDACTED]}, "c": false, "d": REDACTED})
    );
}

#[test]
fn a_key_whose_lines_are_nodes_texts_is_masked() {
    let out = redact(json!([
        {"role": "text", "text": BEGIN},
        {"role": "text", "text": BODY},
        {"role": "text", "text": END},
        {"role": "text", "text": "after"},
    ]));
    assert!(!shows_the_body(&out), "{out}");
    assert!(!out.to_string().contains("-----END"), "{out}");
    assert_eq!(out[3], json!({"role": "text", "text": "after"}));
}

#[test]
fn bytes_between_the_items_are_masked() {
    let r = Redactor::default();
    let tree = Py::List(vec![
        Py::str(BEGIN),
        Py::Bytes(BODY.as_bytes().to_vec()),
        Py::Dict(vec![(
            PyStr::new("raw"),
            Py::Bytes(BODY.as_bytes().to_vec()),
        )]),
        Py::Bytes(Vec::new()),
        Py::str(END),
    ]);
    let Py::List(out) = r.py(&tree) else {
        panic!("a list comes back a list")
    };
    assert_eq!(out[1].as_str(), Some(REDACTED));
    assert_eq!(out[2].get("raw").and_then(Py::as_str), Some(REDACTED));
    assert_eq!(out[3], Py::Bytes(Vec::new()));
    let again = r.py(&Py::List(out.clone()));
    assert_eq!(again, Py::List(out), "redacting twice changes nothing");
}

#[test]
fn the_stream_hides_a_key_fed_line_by_line() {
    // the stream reads text, not items: its key rule is the text form's, checked here on a key cut into feeds
    let r = Redactor::default();
    let mut s = r.stream();
    let mut out = Vec::new();
    for chunk in [BEGIN, "\n", &BODY[..7], &BODY[7..], "\n", END, "\nafter\n"] {
        out.extend(s.feed(chunk.as_bytes(), false));
    }
    out.extend(s.feed(b"", true));
    let out = String::from_utf8(out).expect("UTF-8");
    assert!(!out.contains(BODY) && !out.contains(&BODY[7..]), "{out}");
    assert!(out.ends_with("after\n"), "{out}");
}
