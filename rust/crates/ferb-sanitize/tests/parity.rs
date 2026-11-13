//! The port against the Python build (decision: ENG-3). `rust/fixtures/sanitize/make.py` ran ferb/sanitize and
//! ferb/core/origin.py on invented inputs; every case here must give the same output, byte for byte. Offsets in
//! the fixtures are Python's (code points) and are converted from this crate's byte offsets before comparing.
//!
//! Every string in a fixture is base64 (the values are invented but key-shaped; see make.py). A missing or
//! incomplete fixture folder fails every test here: nothing passes by having nothing to check.

use std::path::PathBuf;

use ferb_sanitize::{
    formats, known, labels, origin, parity, redact, terminal, untrusted, Field, JsonError, Py,
    PyStr, Redactor,
};
use serde_json::Value;

fn dir() -> PathBuf {
    // FERB_SANITIZE_FIXTURES: a wider corpus made by `make.py --out` for a local run; the tracked one otherwise.
    match std::env::var_os("FERB_SANITIZE_FIXTURES") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sanitize"),
    }
}

fn read(name: &str) -> Value {
    let path = dir().join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "fixture {} is missing ({e}): run `uv run python rust/fixtures/sanitize/make.py`",
            path.display()
        )
    });
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("fixture {} is not JSON: {e}", path.display()))
}

fn manifest_count(group: &str) -> usize {
    read("manifest.json")["groups"][group]
        .as_u64()
        .unwrap_or_else(|| panic!("manifest.json has no group {group}")) as usize
}

fn cases(group: &str) -> Vec<Value> {
    let file = read(&format!("{group}.json"));
    let cases = file["cases"]
        .as_array()
        .unwrap_or_else(|| panic!("{group}.json has no cases"))
        .clone();
    assert_eq!(
        cases.len(),
        manifest_count(group),
        "{group}.json does not hold the cases manifest.json counts"
    );
    assert!(!cases.is_empty(), "{group}.json is empty");
    cases
}

fn unb64(text: &str) -> Vec<u8> {
    fn val(c: u8) -> u32 {
        match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => panic!("not base64"),
        }
    }
    let bytes: Vec<u8> = text.bytes().filter(|&b| b != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let n = chunk.iter().fold(0u32, |acc, &c| (acc << 6) | val(c)) << (6 * (4 - chunk.len()));
        let quad = n.to_be_bytes();
        out.extend_from_slice(&quad[1..chunk.len()]);
    }
    out
}

fn bytes(v: &Value) -> Vec<u8> {
    unb64(
        v.as_str()
            .unwrap_or_else(|| panic!("expected a base64 string, got {v}")),
    )
}

fn s(v: &Value) -> String {
    String::from_utf8(bytes(v)).expect("fixture text is UTF-8")
}

fn opt_s(v: &Value) -> Option<String> {
    (!v.is_null()).then(|| s(v))
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(s).collect())
        .unwrap_or_default()
}

/// A text output: `true` stands for "the same as the input".
fn same_or(v: &Value, input: &str) -> String {
    if v == &Value::Bool(true) {
        input.to_string()
    } else {
        s(v)
    }
}

/// Byte offset → code point offset.
fn cp(text: &str, byte: usize) -> usize {
    text[..byte].chars().count()
}

/// Code point offset → byte offset.
fn byte(text: &str, cp: usize) -> usize {
    text.char_indices().nth(cp).map_or(text.len(), |(i, _)| i)
}

fn spans_json(text: &str, spans: &[(usize, usize)]) -> Value {
    Value::Array(
        spans
            .iter()
            .map(|&(a, b)| serde_json::json!([cp(text, a), cp(text, b)]))
            .collect(),
    )
}

fn redactor(args: &Value) -> Redactor {
    let (domains, apps) = (strs(&args["domains"]), strs(&args["apps"]));
    let r = Redactor::new(
        &domains.iter().map(String::as_str).collect::<Vec<_>>(),
        &apps.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    for value in strs(&args["known"]) {
        r.add_secret_value(&value);
    }
    r
}

/// Collects mismatches; fails at the end with how many and the first few.
struct Check {
    group: &'static str,
    checked: usize,
    failed: Vec<String>,
}

impl Check {
    fn new(group: &'static str) -> Check {
        Check {
            group,
            checked: 0,
            failed: Vec::new(),
        }
    }

    fn eq<T: PartialEq + std::fmt::Debug>(
        &mut self,
        what: &str,
        input: &dyn std::fmt::Debug,
        got: T,
        want: T,
    ) {
        self.checked += 1;
        if got != want {
            self.failed.push(format!(
                "{what} on {input:?}\n   rust:   {got:?}\n   python: {want:?}"
            ));
        }
    }

    fn done(self, cases: usize) {
        eprintln!(
            "parity {}: {} cases, {} comparisons, {} failed",
            self.group,
            cases,
            self.checked,
            self.failed.len()
        );
        assert!(
            self.failed.is_empty(),
            "{}: {} of {} comparisons differ from Python. First:\n{}",
            self.group,
            self.failed.len(),
            self.checked,
            self.failed
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
        assert!(
            self.checked >= cases,
            "{}: fewer comparisons than cases",
            self.group
        );
    }
}

fn check_redact(c: &mut Check, r: &Redactor, text: &str, out: &Value) {
    c.eq("text", &text, r.text(text), same_or(&out["text"], text));
    c.eq("ident", &text, r.ident(text), same_or(&out["ident"], text));
    c.eq(
        "hit",
        &text,
        r.hit(text).expect("every pattern runs").map(str::to_string),
        opt_s(&out["hit"]),
    );
    c.eq(
        "kinds",
        &text,
        r.kinds(text)
            .expect("every pattern runs")
            .iter()
            .map(|k| k.to_string())
            .collect::<Vec<_>>(),
        strs(&out["kinds"]),
    );
    // redact.py:6: redacting a redacted text changes nothing
    let once = r.text(text);
    c.eq("text twice", &text, r.text(&once), once);
}

#[test]
fn fixtures_are_present() {
    // Fails loudly when the fixtures were never made or a file was dropped.
    let manifest = read("manifest.json");
    let groups = manifest["groups"]
        .as_object()
        .expect("manifest.json lists its groups");
    for name in [
        "text",
        "spans",
        "lines",
        "known",
        "small",
        "fields",
        "walk",
        "stream",
        "untrusted",
        "origin",
        "py",
        "json",
        "entropy",
    ] {
        assert!(
            groups.contains_key(name),
            "manifest.json has no group {name}"
        );
        assert_eq!(cases(name).len() as u64, groups[name].as_u64().unwrap_or(0));
    }
    let total: u64 = groups.values().filter_map(Value::as_u64).sum();
    assert_eq!(Some(total), manifest["total"].as_u64());
    assert!(total >= 5000, "the corpus shrank to {total} cases");
    assert!(read("unicode.json")["lower"]
        .as_array()
        .is_some_and(|a| a.len() > 1000));
}

#[test]
fn every_python_pattern_compiles() {
    assert_eq!(ferb_sanitize::compile_patterns(), Ok(()));
}

// decision: GATE-501, GATE-502, GATE-206, GATE-211, GATE-212 (the formats and labelled classes of free text)
#[test]
fn text_matches_python() {
    let (all, mut c) = (cases("text"), Check::new("text"));
    for case in &all {
        let (args, out) = (&case["in"], &case["out"]);
        let text = s(&args["s"]);
        check_redact(&mut c, &redactor(args), &text, out);
    }
    c.done(all.len());
}

#[test]
fn spans_match_python() {
    let (all, mut c) = (cases("spans"), Check::new("spans"));
    for case in &all {
        let (text, out) = (s(&case["in"]["s"]), &case["out"]);
        let hits: Vec<Value> = formats::format_hits(&text, None)
            .iter()
            .map(|h| serde_json::json!([cp(&text, h.start), cp(&text, h.end), h.kind]))
            .collect();
        let want: Vec<Value> = out["format_hits"]
            .as_array()
            .expect("format_hits")
            .iter()
            .map(|h| serde_json::json!([h[0], h[1], s(&h[2])]))
            .collect();
        c.eq("format_hits", &text, hits, want);
        c.eq(
            "labelled_spans",
            &text,
            spans_json(&text, &labels::labelled_spans(&text, None)),
            out["labelled_spans"].clone(),
        );
        c.eq(
            "urls",
            &text,
            spans_json(&text, &formats::urls(&text)),
            out["urls"].clone(),
        );
        c.eq(
            "code_words",
            &text,
            spans_json(&text, &labels::code_words(&text)),
            out["code_words"].clone(),
        );
        c.eq(
            "secret_words",
            &text,
            spans_json(&text, &labels::secret_words(&text)),
            out["secret_words"].clone(),
        );
        c.eq(
            "id_bodies",
            &text,
            spans_json(&text, &known::id_bodies(&text, false)),
            out["id_bodies"].clone(),
        );
        c.eq(
            "id_bodies keyed",
            &text,
            spans_json(&text, &known::id_bodies(&text, true)),
            out["id_bodies_keyed"].clone(),
        );
    }
    c.done(all.len());
}

#[test]
fn line_rules_match_python() {
    let (all, mut c) = (cases("lines"), Check::new("lines"));
    for case in &all {
        let (line, out) = (s(&case["in"]["s"]), &case["out"]);
        c.eq(
            "list_line",
            &line,
            labels::list_line(&line),
            out["list_line"] == true,
        );
        c.eq(
            "label_line",
            &line,
            labels::label_line(&line),
            out["label_line"] == true,
        );
        c.eq(
            "link_line",
            &line,
            labels::link_line(&line),
            out["link_line"] == true,
        );
        c.eq(
            "quote_only",
            &line,
            labels::quote_only(&line),
            out["quote_only"] == true,
        );
        c.eq(
            "list_label_line",
            &line,
            labels::list_label_line(&line),
            out["list_label_line"] == true,
        );
        let span = labels::code_alone_span(&line).map_or(Value::Null, |(a, b)| {
            serde_json::json!([cp(&line, a), cp(&line, b)])
        });
        c.eq("code_alone_span", &line, span, out["code_alone"].clone());
    }
    c.done(all.len());
}

// decision: GATE-572 (owner): only FERB's own credentials are matched by value. GATE-573: exactly, in each escaped
// form, never strictly inside a generated id, a short one never inside a longer number; under 4 characters never.
#[test]
fn known_values_match_python() {
    let (all, mut c) = (cases("known"), Check::new("known"));
    for case in &all {
        let (op, args, out) = (case["op"].as_str().unwrap_or(""), &case["in"], &case["out"]);
        match op {
            "escaped_forms" => {
                let value = s(&args["s"]);
                c.eq(
                    "escaped_forms",
                    &value,
                    known::escaped_forms(&value),
                    strs(out),
                );
            }
            "forms" => {
                let r = redactor(args);
                let got: Vec<String> = r.secret_values().iter().map(str::to_string).collect();
                c.eq("secret_values", &"(a known set)", got, strs(out));
            }
            "redact" => {
                let text = s(&args["s"]);
                check_redact(&mut c, &redactor(args), &text, out);
            }
            "known" => {
                let (text, r) = (s(&args["s"]), redactor(args));
                check_redact(&mut c, &r, &text, out);
                let values = r.secret_values();
                let sorted = |keyed: bool| {
                    let mut found = known::known_spans(&text, &values, keyed);
                    found.sort();
                    let mut cps: Vec<(usize, usize)> = found
                        .iter()
                        .map(|&(a, b)| (cp(&text, a), cp(&text, b)))
                        .collect();
                    cps.sort();
                    Value::Array(
                        cps.iter()
                            .map(|&(a, b)| serde_json::json!([a, b]))
                            .collect(),
                    )
                };
                c.eq(
                    "known_spans",
                    &text,
                    sorted(false),
                    out["known_spans"].clone(),
                );
                c.eq(
                    "known_spans keyed",
                    &text,
                    sorted(true),
                    out["known_spans_keyed"].clone(),
                );
                let value = s(&args["value"]);
                c.eq(
                    "sealed_spans",
                    &text,
                    spans_json(&text, &known::sealed_spans(&text, &value, None)),
                    out["sealed_spans"].clone(),
                );
                c.eq(
                    "holds_sealed",
                    &text,
                    known::holds_sealed(&text, &values),
                    out["holds_sealed"] == true,
                );
            }
            other => panic!("known.json: unknown op {other}"),
        }
    }
    c.done(all.len());
}

#[test]
fn small_functions_match_python() {
    let (all, mut c) = (cases("small"), Check::new("small"));
    for case in &all {
        let (op, args, out) = (case["op"].as_str().unwrap_or(""), &case["in"], &case["out"]);
        match op {
            "label_class" => {
                let label = s(&args["s"]);
                c.eq(
                    op,
                    &label,
                    labels::label_class(&label).map(|k| k.as_str().to_string()),
                    opt_s(out),
                );
                c.eq(
                    "secret_key",
                    &label,
                    labels::secret_key(&label),
                    opt_s(out).as_deref() == Some("secret"),
                );
            }
            "words" => {
                let label = s(&args["s"]);
                c.eq(op, &label, labels::words(&label), strs(out));
            }
            "secret_param" => {
                let name = s(&args["s"]);
                c.eq(op, &name, formats::secret_param(&name), out == true);
            }
            "high_entropy" => {
                let value = s(&args["s"]);
                c.eq(op, &value, labels::high_entropy(&value), out == true);
            }
            "labelled_value" => {
                let value = s(&args["s"]);
                let cls = opt_s(&args["cls"]).map(|name| match name.as_str() {
                    "secret" => labels::LabelClass::Secret,
                    "code" => labels::LabelClass::Code,
                    _ => labels::LabelClass::Credential,
                });
                c.eq(
                    op,
                    &(cls, &value),
                    labels::labelled_value(cls, &value),
                    out == true,
                );
            }
            "payment_card" => {
                let value = s(&args["s"]);
                c.eq(op, &value, formats::payment_card(&value), out == true);
            }
            "named_kinds" => {
                let kinds = strs(&args["kinds"]);
                let refs: Vec<&str> = kinds.iter().map(String::as_str).collect();
                c.eq(op, &kinds, formats::named_kinds(&refs), s(out));
            }
            "hidden_note" => {
                let kinds = strs(&args["kinds"]);
                let refs: Vec<&str> = kinds.iter().map(String::as_str).collect();
                c.eq(
                    op,
                    &kinds,
                    formats::hidden_note(&refs, &s(&args["held_in"])),
                    s(out),
                );
            }
            "mask" => {
                let text = s(&args["s"]);
                let reps: Vec<(usize, usize, String)> = args["spans"]
                    .as_array()
                    .expect("spans")
                    .iter()
                    .map(|x| {
                        (
                            x[0].as_u64().unwrap_or(0) as usize,
                            x[1].as_u64().unwrap_or(0) as usize,
                            s(&x[2]),
                        )
                    })
                    .collect();
                let spans: Vec<(usize, usize, &str)> = reps
                    .iter()
                    .map(|(a, b, rep)| (byte(&text, *a), byte(&text, *b), rep.as_str()))
                    .collect();
                c.eq(op, &text, redact::mask(&text, &spans), s(out));
            }
            "printable" => {
                let text = s(&args["s"]);
                c.eq(op, &text, terminal::printable(&text), s(out));
            }
            other => panic!("small.json: unknown op {other}"),
        }
    }
    c.done(all.len());
}

// decision: GATE-206 (card and password fields), GATE-211 (code labels)
#[test]
fn fields_and_answers_match_python() {
    let (all, mut c) = (cases("fields"), Check::new("fields"));
    let r = Redactor::default();
    for case in &all {
        let (op, args, out) = (case["op"].as_str().unwrap_or(""), &case["in"], &case["out"]);
        match op {
            "field" => {
                let (text, role, label) = (s(&args["text"]), s(&args["role"]), s(&args["label"]));
                let (meta, kind) = (strs(&args["metadata"]), opt_s(&args["kind"]));
                let meta_refs: Vec<&str> = meta.iter().map(String::as_str).collect();
                let f = Field {
                    role: &role,
                    label: &label,
                    secure_input: args["secure_input"] == true,
                    concealed: args["concealed"] == true,
                    metadata: &meta_refs,
                    kind: kind.as_deref(),
                };
                let input = (&text, &role, &label, &meta, &kind);
                c.eq(
                    op,
                    &input,
                    r.field(&text, &f),
                    same_or(&out["field"], &text),
                );
                let mut all_meta = vec![label.as_str()];
                all_meta.extend(&meta_refs);
                c.eq(
                    "password_field",
                    &input,
                    labels::password_field(&role, &all_meta),
                    out["password_field"] == true,
                );
                c.eq(
                    "labelled_code",
                    &input,
                    labels::labelled_code(&label, &text),
                    out["labelled_code"] == true,
                );
                c.eq(
                    "labelled_secret",
                    &input,
                    labels::labelled_secret(&role, &label, &text),
                    out["labelled_secret"] == true,
                );
            }
            "answer" => {
                let (text, question, reply) = (
                    s(&args["text"]),
                    s(&args["question"]),
                    args["reply"] == true,
                );
                let input = (&text, &question, reply);
                c.eq(
                    op,
                    &input,
                    r.answer(&text, &question, reply),
                    same_or(&out["answer"], &text),
                );
                c.eq(
                    "hidden_answer",
                    &input,
                    labels::hidden_answer(&question, &text, reply),
                    out["hidden_answer"] == true,
                );
            }
            other => panic!("fields.json: unknown op {other}"),
        }
    }
    c.done(all.len());
}

/// Equal, key order included (a serde_json map compares without its order).
fn same_json(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y.iter())
                    .all(|((ka, va), (kb, vb))| ka == kb && same_json(va, vb))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| same_json(p, q))
        }
        _ => a == b,
    }
}

#[test]
fn structures_match_python() {
    let (all, mut c) = (cases("walk"), Check::new("walk"));
    for case in &all {
        let (op, args, out) = (case["op"].as_str().unwrap_or(""), &case["in"], &case["out"]);
        let source = s(&args["json"]);
        let obj: Value = serde_json::from_str(&source).expect("a JSON input");
        match op {
            "walk" => {
                let want: Value = serde_json::from_str(&s(&out["walk"])).expect("a JSON output");
                let got = redactor(args).value(&obj);
                let (shown, wanted) = if same_json(&got, &want) {
                    (String::new(), String::new())
                } else {
                    (got.to_string(), want.to_string())
                };
                c.eq("walk", &source, shown, wanted);
                c.eq(
                    "str()",
                    &source,
                    parity::to_str(&Py::from(&obj)),
                    s(&out["py_str"]),
                );
            }
            "py_str" => c.eq("str()", &source, parity::to_str(&Py::from(&obj)), s(out)),
            other => panic!("walk.json: unknown op {other}"),
        }
    }
    c.done(all.len());
}

/// A fixture's tagged tree (make.py `py_build`) as the value it stands for: {"t"} is what `r.text` returns and
/// {"r"} what `r` returns for the tree inside, both with their marks.
fn py_build(r: &Redactor, node: &Value) -> Py {
    match node {
        Value::Null => return Py::None,
        Value::Bool(b) => return Py::Bool(*b),
        _ => {}
    }
    let (tag, v) = node
        .as_object()
        .and_then(|o| o.iter().next())
        .unwrap_or_else(|| panic!("py.json: not a tagged value: {node}"));
    match tag.as_str() {
        "s" => Py::str(s(v)),
        "t" => Py::Str(r.text_marked(&PyStr::new(s(v)))),
        "b" => Py::Bytes(bytes(v)),
        "i" => Py::int(&s(v)).expect("an integer's digits"),
        "f" => Py::Float(v.as_f64().expect("a float")),
        "l" => Py::List(
            v.as_array()
                .expect("a list")
                .iter()
                .map(|x| py_build(r, x))
                .collect(),
        ),
        "d" => Py::Dict(
            v.as_array()
                .expect("a dict's items")
                .iter()
                .map(|kv| (PyStr::new(s(&kv[0])), py_build(r, &kv[1])))
                .collect(),
        ),
        "r" => r.py(&py_build(r, v)),
        other => panic!("py.json: unknown tag {other}"),
    }
}

/// A value as make.py's `py_show` writes it, strings as they are (the fixture's are base64): {"m"} is a string
/// with the redactor's current mark.
fn py_show(r: &Redactor, v: &Py) -> Value {
    use serde_json::json;
    let text = |t: &PyStr| if r.is_redacted(t) { "m" } else { "s" };
    match v {
        Py::None => Value::Null,
        Py::Bool(b) => Value::Bool(*b),
        Py::Str(t) => json!({ text(t): t.text }),
        Py::Bytes(b) => json!({ "b": b }),
        Py::Int(digits) => json!({ "i": digits }),
        Py::Float(f) => json!({ "f": f }),
        Py::List(items) => json!({ "l": items.iter().map(|x| py_show(r, x)).collect::<Vec<_>>() }),
        Py::Dict(d) => json!({ "d": d
            .iter()
            .map(|(k, x)| json!([k.text, py_show(r, x), r.is_redacted(k)]))
            .collect::<Vec<_>>() }),
    }
}

/// The fixture's side of `py_show`: its base64 strings read back.
fn py_want(v: &Value) -> Value {
    use serde_json::json;
    let Some((tag, x)) = v.as_object().and_then(|o| o.iter().next()) else {
        return v.clone();
    };
    match tag.as_str() {
        "s" | "m" | "i" => json!({ tag.as_str(): s(x) }),
        "b" => json!({ "b": bytes(x) }),
        "l" => {
            json!({ "l": x.as_array().expect("a list").iter().map(py_want).collect::<Vec<_>>() })
        }
        "d" => json!({ "d": x
            .as_array()
            .expect("a dict's items")
            .iter()
            .map(|kv| json!([s(&kv[0]), py_want(&kv[1]), kv[2]]))
            .collect::<Vec<_>>() }),
        _ => v.clone(),
    }
}

// redact.py:50, 164-173, 209, 282-297, 377: the mark, bytes and integers of any size, as Python holds them.
#[test]
fn marks_bytes_and_big_integers_match_python() {
    let (all, mut c) = (cases("py"), Check::new("py"));
    for case in &all {
        let (args, out) = (&case["in"], &case["out"]);
        let r = redactor(args);
        let built = py_build(&r, &args["tree"]);
        for value in strs(&args["then"]) {
            r.add_secret_value(&value);
        }
        if args["reconfigure"] == true {
            let (domains, apps) = (strs(&args["domains"]), strs(&args["apps"]));
            r.configure(
                &domains.iter().map(String::as_str).collect::<Vec<_>>(),
                &apps.iter().map(String::as_str).collect::<Vec<_>>(),
            );
        }
        let got = r.py(&built);
        let shown = args["tree"].to_string();
        c.eq("py", &shown, py_show(&r, &got), py_want(&out["out"]));
        c.eq(
            "py twice",
            &shown,
            py_show(&r, &r.py(&got)),
            py_want(&out["twice"]),
        );
    }
    c.done(all.len());
}

// redact.py:279: an integer is checked by its digits, whatever its size, and written back with all of them.
#[test]
fn json_text_matches_python() {
    let (all, mut c) = (cases("json"), Check::new("json"));
    let (mut refused, mut big) = (0, 0);
    for case in &all {
        let (args, out) = (&case["in"], &case["out"]);
        let (text, r) = (s(&args["json"]), redactor(args));
        if out.is_null() {
            refused += 1;
            c.eq("json refused", &text, r.json(&text, true).ok(), None);
            continue;
        }
        // a run of 20 digits or more: past 64 bits
        big += usize::from(
            text.split(|ch: char| !ch.is_ascii_digit())
                .any(|run| run.len() >= 20),
        );
        c.eq(
            "json",
            &text,
            r.json(&text, true),
            Ok::<_, JsonError>(s(&out["ascii"])),
        );
        c.eq(
            "json plain",
            &text,
            r.json(&text, false),
            Ok::<_, JsonError>(s(&out["plain"])),
        );
    }
    assert!(
        refused >= 30 && big >= 10,
        "json.json: {refused} refused texts, {big} with big integers"
    );
    c.done(all.len());
}

// labels.py:92: the 3.0 line. Every split of 16 to 56 characters whose entropy is 3 bits or within 1e-9 of it,
// and the bits of each term of the sum for every count and length up to 4096.
#[test]
fn entropy_at_the_line_matches_python() {
    let (all, mut c) = (cases("entropy"), Check::new("entropy"));
    let (mut on, mut off, mut digests) = (0, 0, 0);
    for case in &all {
        let (args, out) = (&case["in"], &case["out"]);
        match case["op"].as_str().unwrap_or("") {
            "high_entropy" => {
                let value = s(&args["s"]);
                let counts: Vec<u32> = args["counts"]
                    .as_array()
                    .expect("counts")
                    .iter()
                    .map(|x| x.as_u64().expect("a count") as u32)
                    .collect();
                if out == true {
                    on += 1
                } else {
                    off += 1
                }
                c.eq(
                    "high_entropy",
                    &value,
                    labels::high_entropy(&value),
                    out == true,
                );
                c.eq(
                    "entropy_at_least_3",
                    &counts,
                    parity::entropy_at_least_3(&counts),
                    out == true,
                );
            }
            "log2_digest" => {
                let top = args["n"].as_u64().expect("n") as u32;
                let mut digest = 0u64;
                for n in 1..=top {
                    for count in 1..=n {
                        let (log, term) = parity::entropy_term(count, n);
                        for x in [log, term] {
                            digest = digest
                                .wrapping_mul(1_099_511_628_211)
                                .wrapping_add(x.to_bits());
                        }
                    }
                }
                digests += 1;
                c.eq("log2 digest", &top, format!("{digest:016x}"), s(out));
            }
            other => panic!("entropy.json: unknown op {other}"),
        }
    }
    // Python's sum lands on 3.0 for every split found (none falls just under it); `small` holds the values under.
    assert!(
        on >= 20 && on + off + 1 == all.len() && digests == 1,
        "entropy.json: {on} on the line, {off} under, {digests} digests"
    );
    c.done(all.len());
}

#[test]
fn the_stream_matches_python() {
    let (all, mut c) = (cases("stream"), Check::new("stream"));
    for (n, case) in all.iter().enumerate() {
        let (args, out) = (&case["in"], &case["out"]);
        let r = redactor(args);
        let mut stream = r.stream();
        if let Some(bound) = args["max_pending"].as_u64() {
            stream.max_pending = bound as usize;
        }
        let chunks: Vec<Vec<u8>> = if args["data"].is_string() {
            let data = bytes(&args["data"]);
            let mut pos = 0;
            args["cuts"]
                .as_array()
                .expect("cuts")
                .iter()
                .map(|cut| {
                    let end = cut.as_u64().unwrap_or(0) as usize;
                    let chunk = data[pos..end].to_vec();
                    pos = end;
                    chunk
                })
                .collect()
        } else {
            args["chunks"]
                .as_array()
                .expect("chunks")
                .iter()
                .map(|chunk| match chunk {
                    Value::String(_) => bytes(chunk),
                    rep => bytes(&rep["rep"]).repeat(rep["n"].as_u64().unwrap_or(0) as usize),
                })
                .collect()
        };
        let last_final = args["last_final"] == true;
        let mut got: Vec<(usize, Vec<u8>)> = Vec::new();
        let mut feeds = 0;
        for (i, chunk) in chunks.iter().enumerate() {
            let released = stream.feed(chunk, last_final && i == chunks.len() - 1);
            if !released.is_empty() {
                got.push((feeds, released));
            }
            feeds += 1;
        }
        if !last_final {
            let released = stream.feed(b"", true);
            if !released.is_empty() {
                got.push((feeds, released));
            }
            feeds += 1;
        }
        let want: Vec<(usize, Vec<u8>)> = out["outs"]
            .as_array()
            .expect("outs")
            .iter()
            .map(|o| (o[0].as_u64().unwrap_or(0) as usize, bytes(&o[1])))
            .collect();
        c.eq(
            "feeds",
            &n,
            feeds as u64,
            out["feeds"].as_u64().unwrap_or(0),
        );
        let show = |v: &[(usize, Vec<u8>)]| -> Vec<(usize, String)> {
            v.iter()
                .map(|(i, b)| (*i, String::from_utf8_lossy(b).into_owned()))
                .collect()
        };
        let input: Vec<String> = chunks
            .iter()
            .take(40)
            .map(|b| String::from_utf8_lossy(b).chars().take(60).collect())
            .collect();
        c.eq(
            "stream",
            &(n, &args["max_pending"], input),
            show(&got),
            show(&want),
        );
        c.eq("stream bytes", &n, got, want);
    }
    c.done(all.len());
}

#[test]
fn untrusted_marking_matches_python() {
    let (all, mut c) = (cases("untrusted"), Check::new("untrusted"));
    for case in &all {
        let (args, out) = (&case["in"], &case["out"]);
        let m = s(&args["s"]);
        let marked = untrusted::mark(&m);
        c.eq(
            "mark.for_model",
            &m,
            marked.for_model,
            s(&out["mark"]["for_model"]),
        );
        c.eq(
            "mark.own_words",
            &m,
            marked.own_words,
            s(&out["mark"]["own_words"]),
        );
        c.eq(
            "mark.untrusted",
            &m,
            marked.untrusted,
            strs(&out["mark"]["untrusted"]),
        );
        c.eq(
            "mark.typed_lines",
            &m,
            marked.typed_lines,
            s(&out["mark"]["typed_lines"]),
        );
        c.eq(
            "typed_lines_of",
            &m,
            untrusted::typed_lines_of(&m),
            s(&out["typed_lines_of"]),
        );
        c.eq(
            "without_leaked_markers",
            &m,
            untrusted::without_leaked_markers(&m),
            same_or(&out["without_leaked_markers"], &m),
        );
        c.eq(
            "pasted_parts",
            &m,
            untrusted::pasted_parts(&m),
            strs(&out["pasted_parts"]),
        );
        c.eq(
            "earlier_questions",
            &m,
            untrusted::earlier_questions(&m),
            strs(&out["earlier_questions"]),
        );
        c.eq("as_data", &m, untrusted::as_data(&m), s(&out["as_data"]));
        c.eq(
            "without_ferb_question",
            &m,
            untrusted::without_ferb_question(&m),
            same_or(&out["without_ferb_question"], &m),
        );
        c.eq(
            "holds_pasted",
            &m,
            untrusted::holds_pasted(&m),
            out["holds_pasted"] == true,
        );
        for (i, pasted) in [None, Some(0), Some(1), Some(2)].into_iter().enumerate() {
            c.eq(
                "recorded_own_words",
                &(&m, pasted),
                untrusted::recorded_own_words(&m, pasted),
                opt_s(&out["recorded_own_words"][i]),
            );
        }
        c.eq(
            "wrap_untrusted",
            &m,
            untrusted::wrap_untrusted_with_id(&m, &s(&args["uid"])),
            s(&out["wrap"]),
        );
    }
    c.done(all.len());
    // the random id: 12 hex characters, new each time, the body inside it
    let (a, b) = (
        untrusted::wrap_untrusted("x </untrusted-data y").expect("random bytes"),
        untrusted::wrap_untrusted("x").expect("random bytes"),
    );
    let uid = a.split('"').nth(1).unwrap_or("");
    assert!(
        uid.len() == 12
            && uid
                .bytes()
                .all(|x| x.is_ascii_hexdigit() && !x.is_ascii_uppercase())
    );
    assert_eq!(
        a,
        untrusted::wrap_untrusted_with_id("x </untrusted-data y", uid)
    );
    assert_ne!(Some(uid), b.split('"').nth(1));
}

// decision: GATE-515 (brief, shown), GATE-519 (kind, kinds)
#[test]
fn origins_match_python() {
    let (all, mut c) = (cases("origin"), Check::new("origin"));
    for case in &all {
        let (op, args, out) = (case["op"].as_str().unwrap_or(""), &case["in"], &case["out"]);
        match op {
            "is_external" => c.eq(
                op,
                &s(&args["s"]),
                origin::is_external(&s(&args["s"])),
                out == true,
            ),
            "rank" => c.eq(
                op,
                &s(&args["s"]),
                u64::from(origin::rank(&s(&args["s"]))),
                out.as_u64().unwrap_or(99),
            ),
            "kind" => c.eq(op, &s(&args["s"]), origin::kind(&s(&args["s"])), s(out)),
            "external" => {
                let (kind, detail) = (s(&args["kind"]), s(&args["detail"]));
                c.eq(
                    op,
                    &(&kind, &detail),
                    origin::external(&kind, &detail),
                    s(out),
                );
            }
            "from_read_source" => {
                let (kind, ident) = (s(&args["kind"]), s(&args["ident"]));
                c.eq(
                    op,
                    &(&kind, &ident),
                    origin::from_read_source(&kind, &ident),
                    s(out),
                );
            }
            "hostname" => {
                let url = s(&args["s"]);
                let got = match parity::hostname(&url) {
                    Ok(host) => ("ok".to_string(), host),
                    Err(_) => ("err".to_string(), None),
                };
                c.eq(op, &url, got, (s(&out[0]), opt_s(&out[1])));
            }
            "origins" => {
                let items = strs(&args["items"]);
                let it = || items.iter().map(String::as_str);
                let text = opt_s(&args["text"]);
                let set = |v: std::collections::BTreeSet<String>| v.into_iter().collect::<Vec<_>>();
                c.eq(
                    "least_trusted",
                    &items,
                    origin::least_trusted(it()),
                    s(&out["least_trusted"]),
                );
                c.eq(
                    "stored_outside",
                    &items,
                    origin::stored_outside(it()),
                    out["stored_outside"] == true,
                );
                c.eq("join", &items, origin::join(it()), s(&out["join"]));
                c.eq(
                    "kinds",
                    &items,
                    set(origin::kinds(it())),
                    strs(&out["kinds"]),
                );
                c.eq(
                    "brief",
                    &items,
                    set(origin::brief(it())),
                    strs(&out["brief"]),
                );
                c.eq("shown", &items, origin::shown(it()), s(&out["shown"]));
                c.eq(
                    "split",
                    &text,
                    set(origin::split(text.as_deref())),
                    strs(&out["split"]),
                );
                c.eq(
                    "recorded",
                    &text,
                    set(origin::recorded(text.as_deref(), false)),
                    strs(&out["recorded"][0]),
                );
                c.eq(
                    "recorded with output",
                    &text,
                    set(origin::recorded(text.as_deref(), true)),
                    strs(&out["recorded"][1]),
                );
            }
            other => panic!("origin.json: unknown op {other}"),
        }
    }
    c.done(all.len());
}

/// `str.lower()` and `str.casefold()` as Python 3.12 answers (Unicode 15.0), for every character and for strings
/// around the capital sigma. The port reads Python's own tables, so this Rust's newer Unicode data changes nothing.
#[test]
fn lowercase_and_casefold_match_python() {
    let file = read("unicode.json");
    let mut python: std::collections::BTreeMap<u32, String> = std::collections::BTreeMap::new();
    for entry in file["lower"].as_array().expect("lower") {
        python.insert(entry[0].as_u64().unwrap_or(0) as u32, s(&entry[1]));
    }
    assert!(python.len() > 1000);
    for cp in 0..=0x10ffffu32 {
        let Some(ch) = char::from_u32(cp) else {
            continue;
        };
        let want = python.get(&cp).cloned().unwrap_or_else(|| ch.to_string());
        assert_eq!(
            parity::lower(&ch.to_string()),
            want,
            "lower() of U+{cp:04X}"
        );
    }
    let words = file["lower_words"].as_array().expect("lower_words");
    assert!(words.len() > 300);
    for pair in words {
        assert_eq!(
            parity::lower(&s(&pair[0])),
            s(&pair[1]),
            "lower() of {:?}",
            s(&pair[0])
        );
    }
    for pair in file["casefold_words"].as_array().expect("casefold_words") {
        assert_eq!(
            parity::casefold(&s(&pair[0])),
            s(&pair[1]),
            "casefold() of {:?}",
            s(&pair[0])
        );
    }
}

// decision: GATE-572 (owner), GATE-573: only what FERB registers as its own credential is matched by value; a value
// under 4 characters is never registered; forgetting one stops the match; nothing prints a value.
#[test]
fn only_registered_values_are_known_and_none_is_printed() {
    let r = Redactor::default();
    let value = ["ferb", "-own-", "credential", "-Q7w8E9r0"].concat();
    let text = format!("the proxy said {value} twice: {value}");
    assert_eq!(r.text(&text), text, "a value nobody registered stays");
    assert_eq!(r.hit(&text), Ok(None));
    assert_eq!(
        r.preload([value.as_str(), "abc", ""]),
        1,
        "values under 4 characters are not known"
    );
    assert_eq!(r.text(&text), "the proxy said [redacted] twice: [redacted]");
    assert_eq!(r.hit(&text), Ok(Some("a saved secret")));
    assert_eq!(r.kinds(&text), Ok(vec![redact::SAVED_SECRET]));
    assert_eq!(r.text("abc"), "abc");
    for shown in [
        format!("{r:?}"),
        format!("{:?}", r.secret_values()),
        format!("{:?}", r.stream()),
    ] {
        assert!(
            !shown.contains(&value) && !shown.contains("credential"),
            "a Debug form shows a known value"
        );
    }
    r.forget_secret_value(&value);
    assert_eq!(r.text(&text), text);
}

// stream.py:1-10: whatever the chunking, the stream releases exactly what `text` gives for the whole, and a secret
// split across chunks never leaves in part.
#[test]
fn a_split_secret_never_leaves_the_stream() {
    let r = Redactor::default();
    let whole = "Your verification code is 123456.\nCard 4111 1111 1111 1111\nbye\n";
    for size in 1..12 {
        let mut stream = r.stream();
        let mut out = Vec::new();
        for chunk in whole.as_bytes().chunks(size) {
            let released = stream.feed(chunk, false);
            let seen = String::from_utf8_lossy(&released).into_owned();
            assert!(
                !seen.contains("123") && !seen.contains("4111"),
                "a part of a secret left at chunk size {size}"
            );
            out.extend(released);
        }
        out.extend(stream.feed(b"", true));
        assert_eq!(String::from_utf8(out).expect("UTF-8"), r.text(whole));
    }
}
