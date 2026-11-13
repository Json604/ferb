//! The port against the Python build (decision: ENG-3). `rust/fixtures/trace/make.py` runs the real Python trace code
//! (`compose`, `render.story`, `timing`, `lines`, and `cli.admin.trace` on a fake client) on span trees built through
//! the real span store, and on the span tables of the recorded chat scenarios. Every case must give the same bytes.

use std::path::PathBuf;

use ferb_trace::lines;
use ferb_trace::py::{loads, str_of};
use ferb_trace::{breakdown, compose, story, timing_lines, trace_output, Mode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn read(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/trace")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "fixture {} is missing ({e}): run `uv run python rust/fixtures/trace/make.py`",
            path.display()
        )
    });
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name} is not JSON: {e}"))
}

fn dumps(v: &Value) -> String {
    ferb_py::json::dumps_utf8(v)
}

fn sha(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn list<'a>(v: &'a Value, key: &str) -> &'a Vec<Value> {
    let items = v[key]
        .as_array()
        .unwrap_or_else(|| panic!("no list {key:?}"));
    assert!(!items.is_empty(), "the fixture's {key:?} is empty");
    items
}

/// Collects differences so one run shows all of them.
#[derive(Default)]
struct Diffs {
    cases: usize,
    bad: Vec<String>,
}

impl Diffs {
    fn same(&mut self, what: &str, got: &str, want: &str) {
        self.cases += 1;
        if got != want {
            let at = got
                .chars()
                .zip(want.chars())
                .take_while(|(a, b)| a == b)
                .count();
            let show = |s: &str| {
                s.chars()
                    .skip(at.saturating_sub(40))
                    .take(120)
                    .collect::<String>()
            };
            self.bad.push(format!(
                "{what}: differs at char {at}\n   got:  {:?}\n   want: {:?}",
                show(got),
                show(want)
            ));
        }
    }

    fn done(self, name: &str) {
        println!(
            "{name}: {} comparisons, {} differ",
            self.cases,
            self.bad.len()
        );
        assert!(
            self.bad.is_empty(),
            "{} differ:\n{}",
            self.bad.len(),
            self.bad
                .iter()
                .take(12)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

#[test]
fn traces_match_python() {
    let fixture = read("traces.json");
    let cases = list(&fixture, "cases");
    let mut d = Diffs::default();
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let spans = c["spans"].as_array().unwrap();
        let task_id = c["task_id"].as_str().unwrap();
        let input = json!({"task_id": task_id, "spans": spans, "outcome": c["outcome"]});
        let has_episode = c["has_episode"].as_bool().unwrap();
        if has_episode && c["composed"].as_bool().unwrap() {
            d.same(
                &format!("{name}: episode"),
                &dumps(&compose(&input)),
                &dumps(&c["episode"]),
            );
        }
        let rows = breakdown(spans);
        d.same(
            &format!("{name}: breakdown"),
            &dumps(&Value::Array(rows.clone())),
            &dumps(&c["breakdown"]),
        );
        let want_lines: Vec<String> = c["timing_lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l.as_str().unwrap().to_owned())
            .collect();
        d.same(
            &format!("{name}: timing lines"),
            &timing_lines(&rows).join("\n"),
            &want_lines.join("\n"),
        );
        let result = json!({"task_id": task_id, "spans": spans, "outcome": c["outcome"], "episode": c["episode"]});
        d.same(
            &format!("{name}: story"),
            &trace_output(&result, Mode::Story),
            c["story"].as_str().unwrap(),
        );
        let bare =
            json!({"task_id": task_id, "spans": spans, "outcome": c["outcome"], "episode": null});
        d.same(
            &format!("{name}: plain tree"),
            &trace_output(&bare, Mode::Story),
            c["plain"].as_str().unwrap(),
        );
        for (mode, key) in [(Mode::Full, "full"), (Mode::Json, "json")] {
            let got = trace_output(&result, mode);
            match c[key].as_str() {
                Some(want) => d.same(&format!("{name}: {key}"), &got, want),
                None => {
                    d.same(
                        &format!("{name}: {key} length"),
                        &got.len().to_string(),
                        &c[format!("{key}_len")].to_string(),
                    );
                    d.same(
                        &format!("{name}: {key} hash"),
                        &sha(&got),
                        c[format!("{key}_sha256")].as_str().unwrap(),
                    );
                }
            }
        }
    }
    assert!(cases.len() >= 60, "only {} trace cases", cases.len());
    d.done("traces");
}

#[test]
fn stories_match_python() {
    let fixture = read("render.json");
    let mut d = Diffs::default();
    for c in list(&fixture, "stories") {
        let verdict = c["verdict"].clone();
        let want: Vec<&str> = c["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l.as_str().unwrap())
            .collect();
        d.same(
            c["name"].as_str().unwrap(),
            &story(&c["episode"], &verdict).join("\n"),
            &want.join("\n"),
        );
    }
    d.done("stories");
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

#[test]
fn line_helpers_match_python() {
    let f = read("lines.json");
    let mut d = Diffs::default();
    for c in list(&f, "clip") {
        let got = match c.get("value") {
            Some(v) => lines::clip_value(v, c["n"].as_u64().unwrap() as usize),
            None => lines::clip(s(&c["text"]), c["n"].as_u64().unwrap() as usize),
        };
        d.same(&format!("clip {}", dumps(c)), &got, s(&c["out"]));
    }
    for c in list(&f, "shorten") {
        let got = lines::shorten(
            s(&c["text"]),
            c["n"].as_u64().unwrap() as usize,
            c["fold"].as_bool().unwrap(),
        );
        d.same(&format!("shorten {}", dumps(c)), &got, s(&c["out"]));
    }
    for c in list(&f, "plural") {
        d.same(
            "plural",
            &lines::plural(c["n"].as_i64().unwrap(), s(&c["one"]), s(&c["many"])),
            s(&c["out"]),
        );
    }
    for c in list(&f, "choices") {
        d.same(
            "choices",
            &lines::choices(c["options"].as_array().unwrap()),
            s(&c["out"]),
        );
    }
    for c in list(&f, "point_checks") {
        d.same(
            "point_checks",
            &lines::point_checks(&c["counts"]),
            s(&c["out"]),
        );
    }
    for c in list(&f, "duration") {
        let got = lines::duration_value(&c["seconds"]);
        d.same(
            &format!("duration {}", dumps(&c["seconds"])),
            &got,
            s(&c["out"]),
        );
    }
    for c in list(&f, "seconds") {
        d.same(
            &format!("seconds {}", dumps(&c["value"])),
            &lines::seconds(c["value"].as_f64().unwrap()),
            s(&c["out"]),
        );
    }
    for c in list(&f, "no_on_screen_action") {
        let got = lines::no_on_screen_action(
            s(&c["what"]),
            &c["processes"],
            s(&c["task_id"]),
            &c["in_task"],
            &c["last_ended_s"],
        );
        d.same(
            &format!("no_on_screen_action {}", dumps(c)),
            &got,
            s(&c["out"]),
        );
    }
    for c in list(&f, "summarize") {
        let got = lines::summarize(
            s(&c["tool"]),
            &c["args"],
            &c["content"],
            c["is_error"].as_bool().unwrap(),
        );
        d.same(&format!("summarize {}", dumps(c)), &got, s(&c["out"]));
    }
    for c in list(&f, "step_line") {
        let got = lines::step_line(
            s(&c["at"]),
            s(&c["tool"]),
            &c["args"],
            s(&c["why"]),
            s(&c["summary"]),
            c["seconds"].as_f64(),
            c["error"].as_str(),
        );
        d.same(&format!("step_line {}", dumps(c)), &got, s(&c["out"]));
    }
    for c in list(&f, "front") {
        let got = lines::front_pid(&c["app"]).map_or(Value::Null, Value::from);
        // Python returns the pid itself, so a bool pid comes back as true/false
        let want = &c["out"];
        let same = match (&got, want) {
            (g, Value::Bool(b)) => g.as_i64() == Some(i64::from(*b)),
            (g, w) => g == w,
        };
        d.same(
            &format!("front_pid {}", dumps(&c["app"])),
            &same.to_string(),
            "true",
        );
    }
    for c in list(&f, "cameforward") {
        let mut cf = lines::CameForward::new();
        if !c["baseline"].is_null() {
            cf.baseline(&c["baseline"]);
        }
        for (step, want) in c["steps"]
            .as_array()
            .unwrap()
            .iter()
            .zip(c["outs"].as_array().unwrap())
        {
            let got = cf.change(step);
            d.same(
                &format!("cameforward {}", dumps(step)),
                &got,
                s(&want["out"]),
            );
            d.same(
                "cameforward pid",
                &format!("{:?}", cf.pid),
                &format!("{:?}", want["pid"].as_i64()),
            );
            d.same(
                "cameforward epoch",
                &format!("{:?}", cf.epoch),
                &format!("{:?}", want["epoch"].as_i64()),
            );
        }
    }
    d.done("lines");
}

// The one deliberate difference from Python (decision: ENG-6, rust/PLAN.md): `ferb trace` on a span whose
// recorded result is JSON but not an object (a number, a list, null, a string) crashes in Python with an
// AttributeError (ferb/cli/admin.py:80, `r.get`); the Rust port shows the result as it shows text that is no JSON.
#[test]
fn a_result_that_is_json_but_not_an_object_no_longer_crashes() {
    let f = read("crashes.json");
    for c in list(&f, "cases") {
        assert_eq!(c["python_raises"], "AttributeError", "{}", c["name"]);
        let name = s(&c["name"]);
        let got = trace_output(&c["result"], Mode::Story);
        let shown = match name {
            "string" => "→ \"text\"",
            "number" => "→ 5",
            "list" => "→ [1, 2]",
            "null" => "→ null",
            other => panic!("unknown case {other}"),
        };
        assert!(got.contains(&format!("\n      {shown}\n")), "{name}: {got}");
    }
}

#[test]
fn python_helpers_read_values_as_python_does() {
    use ferb_trace::py::{json_obj, repr, splitlines};
    assert_eq!(
        repr(&json!([1, "it's", null, true, 2.5, {"k": [1e22]}])),
        "[1, \"it's\", None, True, 2.5, {'k': [1e+22]}]"
    );
    assert_eq!(splitlines("a\r\nb\rc\u{2028}d\n"), vec!["a", "b", "c", "d"]);
    assert_eq!(json_obj(&json!("{\"a\": 1}")), json!({"a": 1}));
    assert_eq!(json_obj(&json!("[1]")), json!({}));
    assert_eq!(json_obj(&json!("nope")), json!({}));
    assert_eq!(str_of(&json!(1.0)), "1.0");
    assert!(loads("{").is_none());
}
