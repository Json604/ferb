//! Where Rust can fail and Python cannot (AGENTS.md rule 7: a secret never reaches a model, a trace, a log or the
//! database): a pattern fancy-regex gives up on, and a system with no random bytes. Neither may panic, and
//! neither may let text through unread.
//!
//! `parity::fail_from(Some(k))` makes this thread's pattern calls fail from the `k`th on; the sweeps below fail
//! each call of an operation in turn.

use ferb_sanitize::untrusted::{self, NoRandomness};
use ferb_sanitize::{checked, parity, Field, Py, Redactor, Unread, UNREADABLE};

const SECRETS: &[&str] = &[
    "hunter2",
    "482913",
    "4111 1111 1111 1111",
    "MIIEowIBAAKCAQEA1234abcd",
];
// The key lines are written in two parts: the commit gate's `secrets` check reads tracked files for their shape.
const TEXT: &str = concat!(
    "Password: hunter2\nYour verification code is 482913.\nCard 4111 1111 1111 1111 exp 03/27\n",
    "-----BEGIN RSA PRIV",
    "ATE KEY-----\nMIIEowIBAAKCAQEA1234abcd\n-----END RSA PRIV",
    "ATE KEY-----\nbye\n"
);

fn shows_a_secret(out: &str) -> bool {
    SECRETS.iter().any(|secret| out.contains(secret))
}

/// `call` with every pattern call in it failing in turn, from the first to one past the last: each answer goes to
/// `judge` with whether a call failed (the answer of a run in which none did is the last). Returns how many runs
/// had a failing call.
fn sweep<T>(call: impl Fn() -> T, judge: impl Fn(&T, bool)) -> u64 {
    for k in 0.. {
        parity::fail_from(Some(k));
        let out = checked(&call);
        parity::fail_from(None);
        let failed = out.is_err();
        // `checked` here only reports whether a call failed; the answer is judged as the caller gets it
        parity::fail_from(Some(k));
        let answer = call();
        parity::fail_from(None);
        judge(&answer, failed);
        if !failed {
            return k;
        }
        assert!(k < 100_000, "the sweep does not end");
    }
    0
}

// The guarded answers of untrusted.rs when a pattern cannot be run, and Python's answers when it can: a wrapper
// that never closes, over a million and a half characters, is read (no input was found on which fancy-regex
// itself gives up on one of the crate's patterns; the hook stands in for it).
#[test]
fn unread_text_is_never_taken_for_the_owners_words() {
    let open = format!(
        "typed words <untrusted-data id=\"ab\">\n{}",
        "x".repeat(1_500_000)
    );
    assert_eq!(
        checked(|| untrusted::typed_lines_of(&open)),
        Ok(open.clone())
    );
    assert!(!untrusted::holds_pasted(&open));
    assert!(untrusted::recorded_own_words(&open, Some(0)).is_some());
    parity::fail_from(Some(0));
    let unread = (
        checked(|| untrusted::typed_lines_of(&open)),
        checked(|| untrusted::pasted_parts(&open)),
        untrusted::holds_pasted(&open),
        untrusted::recorded_own_words(&open, Some(0)),
    );
    parity::fail_from(None);
    assert_eq!(unread, (Err(Unread), Err(Unread), true, None));
}

#[test]
fn whichever_pattern_call_fails_no_text_comes_back_in_part() {
    let r = Redactor::default();
    r.add_secret_value("ferb-own-credential-Q7w8E9r0");
    let whole = |out: &String, failed: bool| {
        assert_eq!(
            out == UNREADABLE,
            failed,
            "a failed call must hide the whole text, and only then"
        );
        assert!(!shows_a_secret(out));
    };
    let runs = sweep(|| r.text(TEXT), whole);
    assert!(runs > 20, "only {runs} pattern calls in a text");
    sweep(|| r.ident(TEXT), whole);
    sweep(
        || {
            r.field(
                TEXT,
                &Field {
                    label: "notes",
                    ..Field::default()
                },
            )
        },
        whole,
    );
    sweep(|| r.answer(TEXT, "What is the code?", true), whole);
    sweep(
        || String::from_utf8_lossy(&r.bytes(format!("{TEXT}\u{ff}").as_bytes())).into_owned(),
        whole,
    );
    sweep(
        || String::from_utf8_lossy(&r.bytes(&[TEXT.as_bytes(), b"\xff\xfe"].concat())).into_owned(),
        whole,
    );
}

#[test]
fn a_failed_pattern_call_hides_a_whole_record() {
    let r = Redactor::default();
    let record = Py::from_json(
        r#"{"note": "Password: hunter2", "items": ["Your verification code", "482913"], "pin": 4821,
            "task_id": "0123456789abcdef0123456789abcdef", "value": "4111 1111 1111 1111", "n": 4111111111111111}"#,
    )
    .expect("a record");
    let runs = sweep(
        || r.py(&record),
        |out, failed| {
            assert_eq!(*out == Py::str(UNREADABLE), failed);
            let shown = out.to_json(false).expect("JSON");
            assert!(!shows_a_secret(&shown) && !shown.contains("4111111111111111"));
        },
    );
    assert!(runs > 20, "only {runs} pattern calls in a record");
    let as_value: serde_json::Value =
        serde_json::from_str(&record.to_json(false).expect("JSON")).expect("JSON");
    sweep(
        || r.value(&as_value),
        |out, failed| {
            assert_eq!(
                *out == serde_json::Value::String(UNREADABLE.to_string()),
                failed
            )
        },
    );
    sweep(
        || r.json(&record.to_json(true).expect("JSON"), true),
        |out, failed| {
            assert_eq!(
                out.as_deref() == Ok(&format!("\"{UNREADABLE}\"")[..]),
                failed
            )
        },
    );
}

#[test]
fn a_gate_cannot_take_an_unread_text_for_one_with_no_secret() {
    let r = Redactor::default();
    let text = format!(
        "push with {}",
        ["gh", "p_", "Zx9Qw3Er7Ty1Ui5Op2As8Df4Gh6Jk0"].concat()
    );
    assert_eq!(r.hit(&text), Ok(Some("GitHub token")));
    sweep(
        || r.hit(&text),
        |out, failed| assert_eq!(out.is_err(), failed),
    );
    sweep(
        || r.kinds(&text),
        |out, failed| assert_eq!(out.is_err(), failed),
    );
    sweep(
        || r.spans(&text, false),
        |out, failed| assert_eq!(out.is_err(), failed),
    );
    parity::fail_from(Some(0));
    assert_eq!(r.hit(&text), Err(Unread));
    parity::fail_from(None);
    assert_eq!(
        r.hit(&text),
        Ok(Some("GitHub token")),
        "a failure must not outlast its call"
    );
}

#[test]
fn a_stream_that_cannot_read_shows_nothing_of_what_it_held() {
    let r = Redactor::default();
    let runs = sweep(
        || {
            let mut stream = r.stream();
            let mut out = stream.feed(TEXT.as_bytes(), false);
            out.extend(stream.feed(b"", true));
            String::from_utf8(out).expect("UTF-8")
        },
        |out, failed| {
            assert_eq!(out.contains(UNREADABLE), failed);
            assert!(!shows_a_secret(out));
        },
    );
    assert!(runs > 20, "only {runs} pattern calls in a read");
    // after the failed read: the marker, then nothing up to the next line break, then the stream reads on
    let mut stream = r.stream();
    parity::fail_from(Some(0));
    let first = stream.feed(b"Password: hunt", false);
    parity::fail_from(None);
    assert_eq!(first, format!("{UNREADABLE}\n").into_bytes());
    assert_eq!(stream.feed(b"er2 rest of the line", false), b"");
    let mut after = stream.feed(b"\nnext line\n", false);
    after.extend(stream.feed(b"", true));
    assert_eq!(after, b"next line\n");
    // inside a private key the stream stays shut until the key's END line
    let mut stream = r.stream();
    stream.max_pending = 64;
    let mut out = stream.feed(
        format!(
            "-----BEGIN RSA PRIV{}\n{}",
            "ATE KEY-----",
            "QUJD".repeat(40)
        )
        .as_bytes(),
        false,
    );
    parity::fail_from(Some(0));
    out.extend(stream.feed(b"QUJDREVG\n", false));
    parity::fail_from(None);
    out.extend(stream.feed(b"MIIEowIBAAKCAQEA1234abcd\nmore\n", false));
    out.extend(stream.feed(
        concat!("-----END RSA PRIV", "ATE KEY-----\nafter\n").as_bytes(),
        true,
    ));
    let shown = String::from_utf8(out).expect("UTF-8");
    assert!(
        !shown.contains("QUJD") && !shown.contains("MIIE") && !shown.contains("more"),
        "{shown}"
    );
    assert!(shown.ends_with("after\n"), "{shown}");
}

#[test]
fn an_owners_message_is_kept_whole_when_its_clean_up_cannot_run() {
    let message = "[200~remind me at 5[201~";
    assert_eq!(untrusted::mark(message).for_model, "remind me at 5");
    parity::fail_from(Some(0));
    let marked = untrusted::mark(&format!("\u{1b}[200~{message}\u{1b}[201~"));
    parity::fail_from(None);
    assert_eq!(marked.for_model, message, "the request is never discarded");
}

#[test]
fn no_random_bytes_no_wrapper() {
    assert_eq!(
        untrusted::wrap_untrusted_from("outside text", |_| false),
        Err(NoRandomness)
    );
    let wrapped = untrusted::wrap_untrusted_from("outside text", |bytes| {
        bytes.copy_from_slice(&[0x0a, 0x1b, 0x2c, 0x3d, 0x4e, 0x5f]);
        true
    });
    assert_eq!(
        wrapped,
        Ok(untrusted::wrap_untrusted_with_id(
            "outside text",
            "0a1b2c3d4e5f"
        ))
    );
}
