//! The secret-shape corpus (`engine/fixtures/sanitize-shapes/corpus.json`, NOT Python-recorded): which secret shapes
//! the redactor masks and which ordinary strings it leaves alone. Every value is invented (see the folder's
//! make.py and README.md).
//!
//! - every positive without a `limit` comes out with each of its secrets gone (not one 6-character piece left);
//! - every positive with a `limit` (a declared gap) still shows a secret, so the declared list stays true;
//! - every negative comes out unchanged;
//! - the stream (`RedactionStream`, fed in small chunks) masks every positive as `Redactor::text` does.
//!
//! `FERB_SHAPES_REPORT=<path>` writes the coverage table (shape, source lists, masked / partly / not; negatives
//! intact / wrongly masked) to that file. Values are never printed: a leak is described by how much of it showed.

use std::path::PathBuf;

use ferb_sanitize::Redactor;
use serde_json::Value;

fn corpus() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/sanitize-shapes/corpus.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is missing ({e})", path.display()));
    serde_json::from_str(&text).expect("corpus.json is JSON")
}

fn items<'a>(c: &'a Value, key: &str) -> &'a Vec<Value> {
    c[key]
        .as_array()
        .unwrap_or_else(|| panic!("corpus has no {key}"))
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or_default()
}

/// How much of `secret` still shows in `out`: the longest piece of it found there more often than the rest of
/// the input holds it (a label that shares letters with its value is not a leak), in characters. A secret of
/// fewer than 6 characters counts only whole.
fn shown(secret: &str, text: &str, out: &str) -> usize {
    let rest = text.replace(secret, "\u{0}");
    let leaked = |piece: &str| out.matches(piece).count() > rest.matches(piece).count();
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() < 6 {
        return if leaked(secret) { chars.len() } else { 0 };
    }
    for len in (6..=chars.len()).rev() {
        for start in 0..=chars.len() - len {
            let piece: String = chars[start..start + len].iter().collect();
            if leaked(&piece) {
                return len;
            }
        }
    }
    0
}

#[derive(Debug, PartialEq)]
enum Verdict {
    Masked,
    Partly(String),
    Not,
}

fn verdict(item: &Value, out: &str) -> Verdict {
    let secrets: Vec<&str> = item["secrets"]
        .as_array()
        .expect("secrets")
        .iter()
        .map(|v| v.as_str().unwrap_or_default())
        .collect();
    let mut leaks = Vec::new();
    let mut whole = 0;
    let text = s(item, "text");
    for (i, secret) in secrets.iter().enumerate() {
        let n = shown(secret, text, out);
        let total = secret.chars().count();
        if n == total {
            whole += 1;
        }
        if n > 0 {
            leaks.push(format!(
                "secret {} of {}: {n} of {total} characters showed",
                i + 1,
                secrets.len()
            ));
        }
    }
    if leaks.is_empty() {
        Verdict::Masked
    } else if whole == secrets.len() && !out.contains("[redacted]") {
        Verdict::Not
    } else {
        Verdict::Partly(leaks.join("; "))
    }
}

fn stream_text(r: &Redactor, text: &str, chunk: usize) -> String {
    let mut st = r.stream();
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let j = (i + chunk).min(bytes.len());
        out.extend(st.feed(&bytes[i..j], false));
        i = j;
    }
    out.extend(st.feed(b"", true));
    String::from_utf8(out).expect("utf-8")
}

// decision: GATE-596, GATE-598 (doubtful values stay masked), GATE-599 (the identifiers, `pii-` ids), GATE-600
#[test]
fn every_secret_shape_is_masked_and_every_declared_gap_still_shows() {
    let c = corpus();
    let r = Redactor::default();
    let mut ids = std::collections::BTreeSet::new();
    let mut failures = Vec::new();
    for item in items(&c, "positives") {
        let id = s(item, "id");
        assert!(ids.insert(id.to_string()), "duplicate id {id}");
        let out = r.text(s(item, "text"));
        let v = verdict(item, &out);
        let limit = s(item, "limit");
        if limit.is_empty() && v != Verdict::Masked {
            failures.push(format!("{id} ({}): {v:?}", s(item, "shape")));
        }
        if !limit.is_empty() && v == Verdict::Masked {
            failures.push(format!(
                "{id}: declared a gap ({limit}) but is masked now: move it out of the limits"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} shapes:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// decision: GATE-597 (publishable keys, public keys and certificates pass), GATE-598 (code or a template after a
// label is no value), GATE-599 (contact data, technical ids and look-alike numbers stay visible)
#[test]
fn ordinary_text_is_left_intact() {
    let c = corpus();
    let r = Redactor::default();
    let wrong: Vec<String> = items(&c, "negatives")
        .iter()
        .filter_map(|item| {
            let (text, out) = (s(item, "text"), r.text(s(item, "text")));
            let known = s(item, "known_fp");
            if known.is_empty() {
                (out != text).then(|| format!("{} ({}): {out}", s(item, "id"), s(item, "what")))
            } else {
                // a false positive of a rule ported from Python, declared: still masked, so the list stays true
                (out == text).then(|| {
                    format!(
                        "{}: declared a known false positive but is intact now",
                        s(item, "id")
                    )
                })
            }
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "{} wrongly masked:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn the_stream_masks_every_shape_as_the_whole_text_does() {
    let c = corpus();
    let r = Redactor::default();
    let mut failures = Vec::new();
    for item in items(&c, "positives") {
        if !s(item, "limit").is_empty() {
            continue;
        }
        let text = s(item, "text");
        for chunk in [1, 7, 64] {
            let out = stream_text(&r, text, chunk);
            if verdict(item, &out) != Verdict::Masked {
                failures.push(format!("{} (chunks of {chunk})", s(item, "id")));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "the stream leaked:\n{}",
        failures.join("\n")
    );
}

#[test]
fn report() {
    let Some(path) = std::env::var_os("FERB_SHAPES_REPORT") else {
        return;
    };
    let c = corpus();
    let r = Redactor::default();
    let mut lines = vec![
        "| id | shape | sources | result |".to_string(),
        "|---|---|---|---|".to_string(),
    ];
    let (mut masked, mut total) = (0, 0);
    for item in items(&c, "positives") {
        let out = r.text(s(item, "text"));
        let v = verdict(item, &out);
        total += 1;
        let result = match &v {
            Verdict::Masked => {
                masked += 1;
                "masked".to_string()
            }
            Verdict::Partly(what) => format!("PARTLY ({what})"),
            Verdict::Not => "NOT MASKED".to_string(),
        };
        let limit = s(item, "limit");
        let sources: Vec<&str> = item["sources"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        lines.push(format!(
            "| {} | {} | {} | {}{} |",
            s(item, "id"),
            s(item, "shape"),
            sources.join(", "),
            result,
            if limit.is_empty() {
                String::new()
            } else {
                format!(" (declared limit: {limit})")
            }
        ));
    }
    lines.push(format!("\n{masked} of {total} positives masked\n"));
    lines.push("| id | negative | result |".to_string());
    lines.push("|---|---|---|".to_string());
    let mut wrong = 0;
    for item in items(&c, "negatives") {
        let out = r.text(s(item, "text"));
        let ok = out == s(item, "text");
        wrong += usize::from(!ok);
        lines.push(format!(
            "| {} | {} | {} |",
            s(item, "id"),
            s(item, "what"),
            if ok {
                "intact".to_string()
            } else {
                format!("WRONGLY MASKED: {out}")
            }
        ));
    }
    lines.push(format!("\n{wrong} negatives wrongly masked\n"));
    std::fs::write(path, lines.join("\n")).expect("report written");
}

/// The layer's rules on inputs built from their own needles and delimiters, repeated: none may give up (a pattern
/// past fancy-regex's backtrack limit hides the whole text as UNREADABLE) and none may cost more than in proportion
/// (tests/scaling.rs judges the whole redactor by instruction counts; this checks the new needles reach it).
#[test]
fn the_layer_reads_its_own_worst_inputs() {
    let r = Redactor::default();
    let units = [
        "pat",
        "$",
        "$2y$10$",
        "--password ",
        "--token ",
        "</a>",
        "<password>x",
        "value=\"",
        "(\"",
        "setX(\"",
        "'k' => '",
        ":= \"",
        "machine ",
        "password ",
        "login ",
        "sshpass -p ",
        "curl -u a:",
        "a:1:b:c:",
        "seed: ",
        "word ",
        "kind: Secret\ndata:\n  k: ",
        "Private-Lines: 1\n",
        "\"kty\":\"d\":\"",
        "AKIA",
        "EAA",
        "key-",
        "-us1",
        ":AA",
        "Q~",
        ".atlasv1.",
        "hooks.slack.com/services/",
        "discord",
        "5H",
        "K1",
        ";password=",
        "access key: ",
        "passwort: ",
        "sb_secret_",
        "dop_v1_",
        "pypi-AgE",
    ];
    for unit in units {
        for n in [1_000, 8_000] {
            let text = format!("x {} y", unit.repeat(n / unit.len().max(1)));
            let out = r.text(&text);
            assert_ne!(
                out,
                ferb_sanitize::UNREADABLE,
                "{unit:?} * {n} could not be read"
            );
            let mut st = r.stream();
            let streamed = st.feed(text.as_bytes(), true);
            assert_ne!(
                String::from_utf8_lossy(&streamed).trim_end(),
                ferb_sanitize::UNREADABLE,
                "the stream could not read {unit:?} * {n}"
            );
        }
    }
}
