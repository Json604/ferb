//! The port against the Python build (decision: ENG-3). Two fixture folders: `rust/fixtures/catalog` (frozen by the
//! catalog's make.py: the answer schemas in each provider's form, jsonschema's repair notes, the budget report) and
//! `rust/fixtures/context` (this part's make.py: token counts, context assembly, mutated answers, schema_shape on
//! invented schemas, load_prompt). Every case must give the same bytes; a missing or empty fixture fails.
//!
//! FERB_CONTEXT_FIXTURES: a wider corpus made by `make.py --scale N --out DIR` for a local run.

use std::path::{Path, PathBuf};

use ferb_context::assemble::{self, AssembleError, ContextBlock};
use ferb_context::prompts::{PromptError, PromptRoot};
use ferb_context::pyvalue::{dumps, dumps_value, loads, Dumps, Py};
use ferb_context::schemas::{self, WorkerFlags};
use ferb_context::shape::{self, Refusals};
use ferb_context::structured::{check_final, CheckError};
use ferb_context::tokens::{self, GrowingCount, Tokenizer};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn context_dir() -> PathBuf {
    match std::env::var_os("FERB_CONTEXT_FIXTURES") {
        Some(path) => PathBuf::from(path),
        None => repo().join("rust/fixtures/context"),
    }
}

fn read(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "fixture {} is missing ({e}): run `uv run python rust/fixtures/context/make.py`",
            path.display()
        )
    });
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
}

fn catalog(name: &str) -> Value {
    read(&repo().join("rust/fixtures/catalog").join(name))
}

fn context(name: &str) -> Value {
    read(&context_dir().join(name))
}

fn list<'a>(v: &'a Value, key: &str) -> &'a Vec<Value> {
    let items = v[key]
        .as_array()
        .unwrap_or_else(|| panic!("no list {key:?} in the fixture"));
    assert!(!items.is_empty(), "the fixture's {key:?} is empty");
    items
}

fn sha(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v}"))
}

/// A schema by the name a fixture gives it, built by this crate.
fn schema(name: &str) -> Value {
    if let Some(flags) = name
        .strip_prefix("worker_schema(")
        .and_then(|r| r.strip_suffix(')'))
    {
        let mut bits = 0u8;
        for flag in flags.split(',').filter(|f| !f.is_empty()) {
            let at = WorkerFlags::NAMES
                .iter()
                .position(|n| *n == flag)
                .unwrap_or_else(|| panic!("unknown worker flag {flag}"));
            bits |= 1 << at;
        }
        let flags = WorkerFlags::from_bits(bits);
        assert_eq!(flags.label(), name);
        return schemas::worker_schema(flags);
    }
    schemas::named()
        .into_iter()
        .chain(schemas::parts())
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v)
        .unwrap_or_else(|| panic!("no Rust builder for {name}"))
}

/// `json.dumps(x)` and `json.dumps(x, ensure_ascii=False, separators=(",", ":"))`, as the catalog's `forms`.
fn check_forms(value: &Value, forms: &Value, what: &str) {
    let default = dumps_value(value, Dumps::DEFAULT);
    assert_eq!(
        default,
        ferb_py::json::dumps(value),
        "{what}: the two writers differ"
    );
    let compact = dumps_value(value, Dumps::COMPACT_UTF8);
    assert_eq!(
        sha(&default),
        s(&forms["default_sha256"]),
        "{what}: default form"
    );
    assert_eq!(
        default.len() as u64,
        forms["default_len"].as_u64().unwrap(),
        "{what}"
    );
    assert_eq!(
        sha(&compact),
        s(&forms["compact_sha256"]),
        "{what}: compact form"
    );
    assert_eq!(
        compact.len() as u64,
        forms["compact_len"].as_u64().unwrap(),
        "{what}"
    );
}

fn same_json(a: &Value, b: &Value) -> bool {
    dumps_value(a, Dumps::DEFAULT) == dumps_value(b, Dumps::DEFAULT)
}

/// What check_final makes of `text`: Ok or the error's text, as the fixtures record it.
fn judge(text: &str, schema: Option<&Value>) -> Result<(), String> {
    match check_final(text, schema) {
        Ok(out) => {
            assert_eq!(out, text, "an answer that fits is returned unchanged");
            Ok(())
        }
        Err(CheckError::Invalid(e)) => Err(e.0),
        Err(CheckError::Schema(e)) => panic!("a fixture schema was refused: {e}"),
        Err(CheckError::TooDeep) => panic!("no fixture answer is nested that deep"),
    }
}

fn expect_judgement(case: &Value, got: Result<(), String>, what: &str) {
    if case["ok"].as_bool().unwrap() {
        assert_eq!(got, Ok(()), "{what}");
    } else {
        assert_eq!(got, Err(s(&case["error"]).to_string()), "{what}");
    }
}

// decision: RUN-408, RUN-447, RUN-447
#[test]
fn catalog_answer_schemas_are_built_byte_for_byte() {
    let fixture = catalog("answer_schemas.json");
    let constants = &fixture["constants"];
    assert_eq!(
        constants["ANTHROPIC_MAX_UNIONS"],
        json!(shape::ANTHROPIC_MAX_UNIONS)
    );
    assert_eq!(
        constants["ANTHROPIC_MAX_OPTIONAL"],
        json!(shape::ANTHROPIC_MAX_OPTIONAL)
    );
    assert_eq!(s(&constants["WRAPPED_NOTE"]), shape::WRAPPED_NOTE);
    assert_eq!(
        constants["STATED_FOR_S"].as_f64(),
        Some(shape::STATED_FOR_S)
    );
    let mut gemini: Vec<&str> = shape::GEMINI_KEYWORDS.to_vec();
    gemini.sort_unstable();
    assert_eq!(constants["GEMINI_KEYWORDS"], json!(gemini));

    let all = fixture["schemas"].as_object().unwrap();
    assert_eq!(all.len(), 86);
    assert_eq!(fixture["schema_count"], json!(86));
    let mut workers = 0;
    for (name, entry) in all {
        let built = schema(name);
        if let Some(core) = entry.get("core") {
            assert!(same_json(&built, core), "{name}: core schema differs");
        } else {
            workers += 1;
        }
        check_forms(&built, &entry["core_forms"], name);
        let (u, o) = shape::complexity(&built);
        assert_eq!(json!([u, o]), entry["complexity_core"], "{name}");
        let anthropic = shape::for_anthropic(&built).expect("fits Anthropic's limits");
        let a = &entry["anthropic"];
        assert_eq!(json!(anthropic.mapped), a["mapped"], "{name}");
        let (u, o) = shape::complexity(&anthropic.wire);
        assert_eq!(json!([u, o]), a["complexity_wire"], "{name}");
        check_forms(&anthropic.wire, &a["wire_forms"], name);
        if let Some(wire) = a.get("wire") {
            assert!(same_json(&anthropic.wire, wire), "{name}: Anthropic wire");
        }
        let g = shape::for_gemini(&built).expect("fits Gemini");
        let ge = &entry["gemini"];
        assert_eq!(json!(g.wire == built), ge["same_as_core"], "{name}");
        check_forms(&g.wire, &ge["wire_forms"], name);
        let outside: Vec<String> = shape::keywords(&g.wire)
            .into_iter()
            .filter(|k| !shape::GEMINI_KEYWORDS.contains(&k.as_str()))
            .collect();
        assert_eq!(json!(outside), ge["keywords_outside_documented"], "{name}");
        let stated = shape::stated(&built);
        assert_eq!(
            sha(&stated),
            s(&entry["stated"]["text_sha256"]),
            "{name}: stated"
        );
        assert_eq!(
            stated.len() as u64,
            entry["stated"]["text_len"].as_u64().unwrap()
        );
        assert!(stated.starts_with(s(&entry["stated"]["prefix"])));
        assert_eq!(shape::STATED_PREFIX, s(&entry["stated"]["prefix"]));
    }
    assert_eq!(workers, 64);
    // every one of the 64 flag sets, by the same logic as Python's
    for bits in 0..64u8 {
        assert!(all.contains_key(&WorkerFlags::from_bits(bits).label()));
    }
    let example = &fixture["stated_example"];
    assert_eq!(shape::stated(&schemas::fix_schema()), s(&example["text"]));
    let key = &fixture["refusal_key_example"];
    let wire = shape::for_anthropic(&schemas::fix_schema()).unwrap().wire;
    assert_eq!(
        Refusals::key(s(&key["provider"]), Some(s(&key["model"])), &wire),
        s(&key["key"])
    );
}

// decision: RUN-408
#[test]
fn catalog_repair_notes_are_jsonschemas_words() {
    let fixture = catalog("repair_notes.json");
    let mut checked = 0;
    for case in list(&fixture, "cases") {
        let built = schema(s(&case["schema"]));
        let got = judge(s(&case["answer"]), Some(&built));
        expect_judgement(case, got.clone(), s(&case["what"]));
        if let Err(problem) = got {
            assert_eq!(shape::repair_note(&problem), s(&case["repair_note"]));
        }
        checked += 1;
    }
    assert_eq!(checked, fixture["case_count"].as_u64().unwrap() as usize);
    for (name, case) in fixture["valid_examples"].as_object().unwrap() {
        expect_judgement(case, judge(s(&case["answer"]), Some(&schema(name))), name);
    }
    for case in list(&fixture, "mapped_back") {
        let built = schema(s(&case["schema"]));
        let shape = shape::for_anthropic(&built).unwrap();
        let core = shape.answer_text(s(&case["wire_answer"]));
        assert_eq!(core, s(&case["core_answer"]), "{}", case["what"]);
        if case.get("ok").is_some() {
            expect_judgement(case, judge(&core, Some(&built)), s(&case["what"]));
        }
    }
    for case in list(&fixture, "unfenced") {
        assert_eq!(shape::unfenced(s(&case["text"])), s(&case["out"]));
    }
}

#[test]
fn catalog_budget_reports() {
    let fixture = catalog("prompt_static.json");
    for case in list(&fixture, "context_budget_reports") {
        let tokenizer = match s(&case["token_method"]) {
            m if m == Tokenizer::O200k.method() => Tokenizer::O200k,
            m if m == Tokenizer::Utf8Bytes.method() => Tokenizer::Utf8Bytes,
            other => panic!("unknown method {other}"),
        };
        let a = case["args"].as_array().unwrap();
        let text = assemble::report(
            tokenizer,
            a[0].as_i64().unwrap(),
            a[1].as_i64().unwrap(),
            a[2].as_i64().unwrap(),
            a[3].as_bool().unwrap(),
            a[4].as_i64().unwrap(),
        );
        assert_eq!(text, s(&case["text"]));
    }
}

// ---------------------------------------------------------------------------------------------------------------

fn token_texts(fixture: &Value) -> Vec<String> {
    let cases = list(fixture, "cases");
    let short: Vec<&str> = cases
        .iter()
        .filter_map(|c| c.get("text").and_then(Value::as_str))
        .collect();
    cases
        .iter()
        .map(|c| match c.get("text") {
            Some(t) => s(t).to_string(),
            None => c["recipe"]
                .as_array()
                .unwrap()
                .iter()
                .map(|piece| {
                    let p = piece.as_array().unwrap();
                    if p.len() == 1 {
                        short[p[0].as_u64().unwrap() as usize].to_string()
                    } else {
                        s(&p[0]).repeat(p[1].as_u64().unwrap() as usize)
                    }
                })
                .collect(),
        })
        .collect()
}

#[test]
fn token_counts_equal_tiktoken_o200k_sliced() {
    let fixture = context("tokens.json");
    assert_eq!(s(&fixture["pattern"]), tokens::PATTERN);
    assert_eq!(s(&fixture["method"]), tokens::token_method());
    assert_eq!(fixture["token_slice"], json!(tokens::TOKEN_SLICE));
    let texts = token_texts(&fixture);
    let cases = list(&fixture, "cases");
    assert_eq!(texts.len() as u64, fixture["case_count"].as_u64().unwrap());
    let mut differ = Vec::new();
    for (text, case) in texts.iter().zip(cases) {
        if tokens::count_tokens(text) as u64 != case["tokens"].as_u64().unwrap() {
            differ.push(text.chars().take(60).collect::<String>());
        }
        assert_eq!(Tokenizer::Utf8Bytes.count(text), text.len());
    }
    assert!(
        differ.is_empty(),
        "{} of {} differ: {differ:?}",
        differ.len(),
        texts.len()
    );

    for steps in list(&fixture, "growing") {
        let mut grow = GrowingCount::new(tokens::count_tokens);
        for step in steps.as_array().unwrap() {
            grow.append(&texts[step["append"].as_u64().unwrap() as usize]);
            let suffix = &texts[step["suffix"].as_u64().unwrap() as usize];
            assert_eq!(
                grow.with_suffix(suffix) as u64,
                step["with_suffix"].as_u64().unwrap()
            );
            assert_eq!(grow.closed as u64, step["closed"].as_u64().unwrap());
            assert_eq!(
                grow.tail.chars().count() as u64,
                step["tail_chars"].as_u64().unwrap()
            );
        }
    }
    for case in list(&fixture, "comma_bounds") {
        assert_eq!(
            json!(tokens::comma_bounds(s(&case["text"]))),
            case["bounds"]
        );
    }
    for case in list(&fixture, "uncut") {
        let ints = |k: &str| -> Vec<i64> {
            case[k]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_i64().unwrap())
                .collect()
        };
        let got = tokens::uncut_count(
            &ints("ends"),
            &ints("totals"),
            case["lo"].as_u64().unwrap() as usize,
            case["hi"].as_u64().unwrap() as usize,
            case["shift"].as_i64().unwrap(),
        );
        assert_eq!(json!(got), case["out"]);
    }
}

// decision: L28 (context budget), ORCH-326
#[test]
fn context_assembly_matches_at_every_budget() {
    let fixture = context("assemble.json");
    let sets: Vec<Vec<ContextBlock>> = list(&fixture, "sets")
        .iter()
        .map(|set| {
            set.as_array()
                .unwrap()
                .iter()
                .map(|b| ContextBlock {
                    text: s(&b["text"]).into(),
                    reference: s(&b["reference"]).into(),
                    protected: b["protected"].as_bool().unwrap(),
                    cache: b["cache"].as_bool().unwrap(),
                    priority: b["priority"].as_i64().unwrap(),
                    history: b["history"].as_bool().unwrap(),
                    fold: s(&b["fold"]).into(),
                    ..ContextBlock::default()
                })
                .collect()
        })
        .collect();
    let cases = list(&fixture, "cases");
    assert_eq!(cases.len() as u64, fixture["case_count"].as_u64().unwrap());
    for (n, case) in cases.iter().enumerate() {
        let tokenizer = match s(&case["tokenizer"]) {
            "o200k" => Tokenizer::O200k,
            _ => Tokenizer::Utf8Bytes,
        };
        let got = assemble::assemble_with(
            tokenizer,
            &sets[case["set"].as_u64().unwrap() as usize],
            case["budget"].as_i64().unwrap(),
            case["reserved"].as_i64().unwrap(),
            case["max_chars"].as_i64(),
        );
        let out = &case["out"];
        match (got, out.get("error").and_then(Value::as_str)) {
            (Ok(a), None) => {
                let parts = dumps(
                    &Py::list(a.parts.iter().map(|p| Py::str(p.as_str())).collect()),
                    Dumps::DEFAULT,
                );
                assert_eq!(
                    sha(&parts),
                    s(&out["parts_sha256"]),
                    "case {n}: {:?}",
                    a.parts
                );
                assert_eq!(a.parts.len() as u64, out["part_count"].as_u64().unwrap());
                assert_eq!(json!(a.cacheable), out["cacheable"], "case {n}");
                assert_eq!(json!(a.shown), out["shown"], "case {n}");
                assert_eq!(json!(a.omitted_history), out["omitted_history"], "case {n}");
                assert_eq!(json!(a.tokens), out["tokens"], "case {n}");
                assert_eq!(json!(a.chars), out["chars"], "case {n}");
            }
            (Err(AssembleError::Arguments(m)), Some("ValueError")) => {
                assert_eq!(m, s(&out["message"]))
            }
            (Err(AssembleError::TooLarge(e)), Some("InputTooLarge")) => {
                assert_eq!(e.to_string(), s(&out["message"]))
            }
            (got, _) => panic!("case {n}: {got:?} where Python gave {out}"),
        }
    }
    for (key, tokenizer) in [
        ("overhead", Tokenizer::O200k),
        ("overhead_utf8", Tokenizer::Utf8Bytes),
    ] {
        for case in list(&fixture, key) {
            let (b, r) = (
                case["budget"].as_i64().unwrap(),
                case["reserved"].as_i64().unwrap(),
            );
            assert_eq!(
                json!(assemble::overhead_tokens(tokenizer, b, r)),
                case["tokens"]
            );
            assert_eq!(
                json!(assemble::overhead_chars(tokenizer, b, r)),
                case["chars"]
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------

fn structured_schema(fixture: &Value, name: &Value) -> Option<Value> {
    match name {
        Value::Null => None,
        Value::String(n) if n == "{}" => Some(json!({})),
        Value::String(n) => {
            let built = schema(n);
            if let Some(frozen) = fixture["schemas"].get(n.as_str()) {
                assert!(same_json(&built, frozen), "{n}");
            }
            Some(built)
        }
        other => panic!("schema name {other}"),
    }
}

// decision: RUN-408 (D32: FERB checks every typed answer)
#[test]
fn mutated_answers_get_the_same_repair_text() {
    let fixture = context("structured.json");
    // every schema the fixture holds is one this crate builds, key for key
    for (name, frozen) in fixture["schemas"].as_object().unwrap() {
        assert!(same_json(&schema(name), frozen), "{name}");
    }
    let cases = list(&fixture, "cases");
    assert_eq!(cases.len() as u64, fixture["case_count"].as_u64().unwrap());
    let invalid = cases.iter().filter(|c| c["ok"] == json!(false)).count();
    assert!(invalid >= 500, "only {invalid} invalid answers");
    assert_eq!(invalid as u64, fixture["invalid_count"].as_u64().unwrap());
    let mut differ = Vec::new();
    for (n, case) in cases.iter().enumerate() {
        let built = structured_schema(&fixture, &case["schema"]);
        let got = judge(s(&case["answer"]), built.as_ref());
        let want = if case["ok"].as_bool().unwrap() {
            Ok(())
        } else {
            Err(s(&case["error"]).to_string())
        };
        if got != want {
            differ.push(format!(
                "case {n} ({}): {got:?} != {want:?}",
                case["schema"]
            ));
        }
    }
    assert!(
        differ.is_empty(),
        "{} differ:\n{}",
        differ.len(),
        differ.join("\n")
    );

    for (n, case) in list(&fixture, "mapped_back").iter().enumerate() {
        let built = schema(s(&case["schema"]));
        let shape = shape::for_anthropic(&built).unwrap();
        assert_eq!(json!(shape.mapped), case["mapped"]);
        let core = shape.answer_text(s(&case["wire_answer"]));
        assert_eq!(core, s(&case["core_answer"]), "mapped_back {n}");
        expect_judgement(
            case,
            judge(&core, Some(&built)),
            &format!("mapped_back {n}"),
        );
    }
    for case in list(&fixture, "unfenced") {
        assert_eq!(shape::unfenced(s(&case["text"])), s(&case["out"]));
        assert_eq!(
            shape::repair_note(s(&case["text"])),
            s(&case["repair_note"])
        );
    }
    for case in list(&fixture, "refusal_keys") {
        let wire = shape::for_anthropic(&schema(s(&case["schema"])))
            .unwrap()
            .wire;
        assert_eq!(
            Refusals::key(s(&case["provider"]), case["model"].as_str(), &wire),
            s(&case["key"])
        );
    }
}

// decision: RUN-408
#[test]
fn schema_shape_on_invented_schemas() {
    let fixture = context("structured.json");
    for (n, case) in list(&fixture, "shapes").iter().enumerate() {
        let schema = &case["schema"];
        let (u, o) = shape::complexity(schema);
        assert_eq!(json!([u, o]), case["complexity"], "shape {n}");
        assert_eq!(
            json!(shape::keywords(schema)),
            case["keywords"],
            "shape {n}"
        );
        assert_eq!(json!(shape::enum_with_null(schema)), case["enum_with_null"]);
        assert_eq!(
            sha(&shape::stated(schema)),
            s(&case["stated_sha256"]),
            "shape {n}"
        );
        assert_eq!(
            Refusals::key("gemini", None, schema),
            s(&case["refusal_key"])
        );
        let mut nodes = vec![schema];
        nodes.extend(
            schema
                .get("properties")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|p| p.values()),
        );
        let nullable: Vec<bool> = nodes.iter().map(|v| shape::nullable(v)).collect();
        let union: Vec<bool> = nodes.iter().map(|v| shape::is_union(v)).collect();
        assert_eq!(json!(nullable), case["nullable"], "shape {n}");
        assert_eq!(json!(union), case["is_union"], "shape {n}");
        let a = &case["anthropic"];
        match shape::for_anthropic(schema) {
            Ok(shape) => {
                assert!(same_json(&shape.wire, &a["wire"]), "shape {n}: wire");
                assert_eq!(
                    sha(&dumps_value(&shape.wire, Dumps::DEFAULT)),
                    s(&a["wire_default_sha256"])
                );
                assert_eq!(json!(shape.mapped), a["mapped"], "shape {n}");
                for answer in list(a, "answers") {
                    let text = s(&answer["wire_answer"]);
                    assert_eq!(
                        shape.answer_text(text),
                        s(&answer["core_answer"]),
                        "shape {n}"
                    );
                    if let Py::Dict(d) = loads(text).unwrap() {
                        for (k, v) in d.0.iter() {
                            let got = dumps(&shape.field(k, v.clone()), Dumps::DEFAULT);
                            assert_eq!(
                                got,
                                s(&answer["fields"][k.as_str()]),
                                "shape {n} field {k}"
                            );
                        }
                    }
                    expect_judgement(
                        answer,
                        judge(s(&answer["core_answer"]), Some(schema)),
                        "shape",
                    );
                }
            }
            Err(e) => assert_eq!(e.0, s(&a["refused"]), "shape {n}"),
        }
        let g = &case["gemini"];
        match shape::for_gemini(schema) {
            Ok(shape) => {
                assert!(same_json(&shape.wire, &g["wire"]), "shape {n}: gemini");
                assert_eq!(
                    sha(&dumps_value(&shape.wire, Dumps::DEFAULT)),
                    s(&g["wire_default_sha256"])
                );
            }
            Err(e) => assert_eq!(e.0, s(&g["refused"]), "shape {n}"),
        }
    }
}

#[test]
fn answer_readers_match_schemas_py() {
    let fixture = context("structured.json");
    let readers = &fixture["readers"];
    for (n, row) in list(readers, "supervisor").iter().enumerate() {
        let answer = &row["answer"];
        assert!(
            same_json(&schemas::ungrouped(answer), &row["ungrouped"]),
            "row {n}"
        );
        if let Some(map) = answer.as_object() {
            let entries = schemas::entries_only(answer);
            assert!(
                same_json(&entries, &row["entries_only"]),
                "row {n}: _entries_only"
            );
            assert!(
                same_json(&schemas::one_schedule(&entries), &row["one_schedule"]),
                "row {n}: _one_schedule"
            );
            assert!(
                same_json(&schemas::supervisor_before(answer), &row["one_schedule"]),
                "row {n}: both, in pydantic's order"
            );
            if let Some(grouped) = row.get("grouped") {
                assert!(
                    same_json(&schemas::grouped(map), grouped),
                    "row {n}: grouped"
                );
            }
            let task = schemas::new_task(answer).cloned().unwrap_or(Value::Null);
            assert!(same_json(&task, &row["new_task"]), "row {n}: new_task");
        }
    }
    for row in list(readers, "check") {
        assert!(same_json(&schemas::check_before(&row["in"]), &row["out"]));
    }
    for row in list(readers, "condition") {
        assert!(same_json(
            &schemas::condition_before(&row["in"]),
            &row["out"]
        ));
    }
    for row in list(readers, "schedule_until") {
        assert!(same_json(
            &schemas::schedule_until_before(&row["in"]),
            &row["out"]
        ));
    }
}

// ---------------------------------------------------------------------------------------------------------------

#[test]
fn load_prompt_reads_as_python_does() {
    let fixture = context("prompts.json");
    let tmp = tempfile::tempdir().unwrap();
    let pkg = tmp.path().join("pkg").join("ferb");
    for (rel, data) in fixture["files"].as_object().unwrap() {
        let path = pkg.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b64(s(data))).unwrap();
    }
    for (name, target) in fixture["links"].as_object().unwrap() {
        let target = s(target).replace("<TMP>", tmp.path().to_str().unwrap());
        std::os::unix::fs::symlink(target, pkg.join(name)).unwrap();
    }
    let root = PromptRoot::new(&pkg).unwrap();
    for case in list(&fixture, "cases") {
        let path = s(&case["path"]).replace("<PKG>", root.package().to_str().unwrap());
        let replacements: Vec<(String, String)> = case["replacements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (s(&p[0]).to_string(), s(&p[1]).to_string()))
            .collect();
        let pairs: Vec<(&str, &str)> = replacements
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let got = root.load(&path, &pairs);
        match (got, case.get("error").and_then(Value::as_str)) {
            (Ok(text), None) => assert_eq!(text, s(&case["text"]), "{path}"),
            (Err(PromptError::NotInPackage), Some("ValueError")) => {
                assert_eq!(PromptError::NotInPackage.to_string(), s(&case["message"]))
            }
            (Err(PromptError::Io(e)), Some("FileNotFoundError")) => {
                assert_eq!(e.kind(), std::io::ErrorKind::NotFound, "{path}")
            }
            (Err(PromptError::Io(e)), Some("IsADirectoryError")) => {
                assert_eq!(e.kind(), std::io::ErrorKind::IsADirectory, "{path}")
            }
            (Err(PromptError::NotUtf8), Some("UnicodeDecodeError")) => {}
            (Err(PromptError::LinkLoop), Some("RuntimeError")) => {}
            (got, want) => panic!("{path}: {got:?} where Python gave {want:?}"),
        }
    }
    let real = PromptRoot::new(repo().join("ferb")).unwrap();
    for case in list(&fixture, "real") {
        let path = s(&case["path"]);
        let text = real.load(path, &[]).unwrap();
        assert_eq!(sha(&text), s(&case["sha256"]), "{path}");
        assert_eq!(text.chars().count() as u64, case["chars"].as_u64().unwrap());
        let replacements: Vec<(String, String)> = case["replacements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (s(&p[0]).to_string(), s(&p[1]).to_string()))
            .collect();
        let pairs: Vec<(&str, &str)> = replacements
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            sha(&real.load(path, &pairs).unwrap()),
            s(&case["replaced_sha256"])
        );
    }
}

fn b64(text: &str) -> Vec<u8> {
    let val = |c: u8| -> u32 {
        match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            _ => 63,
        }
    };
    let clean: Vec<u8> = text.bytes().filter(|c| *c != b'=').collect();
    let mut out = Vec::new();
    for chunk in clean.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c) << (18 - 6 * i);
        }
        let bytes = n.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    out
}
