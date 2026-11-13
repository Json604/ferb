//! (a) and (b): rules, schedules, plans and `when.py` against what the Python code computed
//! (`rust/fixtures/clock/recurrence.json`, `when.json`, made by `make.py`): next runs across DST gaps and
//! overlaps, month ends, leap days, end dates and random windows in 16 zones, and every text Python composes.
//! decisions: ORCH-320, ORCH-334, ORCH-511, ORCH-550, ORCH-551, ORCH-552, ORCH-555, ORCH-556

mod common;

use std::collections::BTreeSet;

use chrono::NaiveDate;
use common::{error, fixture, iso, opt_time, same, time, Diffs};
use ferb_clock::py::{self, Time};
use ferb_clock::schedules::{
    self, clock_minutes, every_said, missing_times, plan_edit, plan_new, said_times, until_said,
    Plan, Rule, Schedule, ScheduleError, Spec, Timing, TimingSpec, Window,
};
use ferb_clock::when::{self, When, WhenError};
use ferb_clock::zone::Zone;
use serde_json::{json, Map, Value};

fn zone(v: &Value) -> Zone {
    Zone::named(v.as_str().unwrap()).expect("a zone the built-in database has")
}

fn s_err(e: &ScheduleError) -> Value {
    error(e.kind(), e.message())
}

fn w_err(e: &WhenError) -> Value {
    error(e.kind(), e.message())
}

fn spans(minutes: &BTreeSet<i64>) -> Value {
    let mut out: Vec<[i64; 2]> = Vec::new();
    for &m in minutes {
        match out.last_mut() {
            Some(last) if last[1] == m - 1 => last[1] = m,
            _ => out.push([m, m]),
        }
    }
    json!(out)
}

fn view(s: &Schedule, now: &Time, z: &Zone) -> Value {
    let mut runs = Vec::new();
    let mut after = *now;
    for _ in 0..6 {
        match s.next_after(&after) {
            Err(e) => {
                runs.push(s_err(&e));
                break;
            }
            Ok(None) => {
                runs.push(Value::Null);
                break;
            }
            Ok(Some(n)) => {
                runs.push(json!(py::iso(&n)));
                after = n;
            }
        }
    }
    let ends = match s.ends_after(now) {
        Ok(b) => json!(b),
        Err(e) => s_err(&e),
    };
    json!({"when_said": s.when_said(), "describe": s.describe(z), "runs": runs, "ends_after": ends,
           "run_minutes": spans(&s.run_minutes().expect("its minutes"))})
}

fn plan_json(p: &Plan, now: &Time, z: &Zone) -> Value {
    json!({"rule": Value::Object(p.rule.to_json()), "first": py::iso(&p.first), "booked": py::iso(&p.booked),
           "return_after": iso(&p.return_after), "until": iso(&p.until),
           "timing": p.timing.as_ref().map_or(Value::Null, Timing::to_json), "missing": p.missing,
           "schedule": view(&p.schedule, now, z)})
}

fn windows(v: &Value) -> Vec<Window> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|w| Window {
                    count: w["count"].as_i64().unwrap(),
                    start: w["start"].as_str().unwrap().into(),
                    end: w["end"].as_str().unwrap().into(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

fn spec(v: &Value) -> Spec {
    Spec {
        request: v["request"].as_str().unwrap().into(),
        when: When::from_json(&v["when"]),
        repeat: v["repeat"].as_str().unwrap().into(),
        weekday: v["weekday"].as_str().map(str::to_string),
        every_minutes: v["every_minutes"].as_i64(),
        times: strings(&v["times"]),
        random: windows(&v["random"]),
        when_active: v["when_active"].as_bool().unwrap_or(false),
        until: v.get("until").filter(|u| !u.is_null()).map(When::from_json),
    }
}

#[test]
fn rules_find_their_next_runs_as_python_does() {
    let f = fixture("recurrence.json");
    let mut d = Diffs::default();
    for case in f["rules"].as_array().unwrap() {
        if case.get("error").is_some() {
            panic!("a rule Python couldn't build: {case}");
        }
        let rule = Rule::from_json(case["rule"].as_object().unwrap());
        let anchor = time(&case["anchor"]);
        let mut cur = time(&case["after"]);
        let mut got = Vec::new();
        let steps = if case.get("dst").is_some() { 3 } else { 4 };
        for _ in 0..steps {
            match rule.next_after(&cur, &anchor) {
                Err(e) => {
                    got.push(s_err(&e));
                    break;
                }
                Ok(None) => {
                    got.push(Value::Null);
                    break;
                }
                Ok(Some(n)) => {
                    got.push(json!(py::iso(&n)));
                    cur = n;
                }
            }
        }
        let what = format!(
            "rule {} {} after {}",
            case["zone"], case["rule"], case["after"]
        );
        d.check(&what, &case["next"], &json!(got));
        d.check(
            &format!("{what} describe"),
            &case["describe"],
            &json!(rule.describe()),
        );
    }
    d.finish("rules");
}

#[test]
fn plans_from_the_planners_fields_match_python() {
    let f = fixture("recurrence.json");
    let mut d = Diffs::default();
    for case in f["plans"].as_array().unwrap() {
        let z = zone(&case["zone"]);
        let now = time(&case["now"]);
        let got = match plan_new(
            &spec(&case["spec"]),
            &now,
            case["seed"].as_str().unwrap(),
            &z,
        ) {
            Ok(p) => plan_json(&p, &now, &z),
            Err(e) => s_err(&e),
        };
        d.check(
            &format!("plan {} at {}: {}", case["zone"], case["now"], case["spec"]),
            &case["plan"],
            &got,
        );
    }
    d.finish("plans");
}

#[test]
fn schedule_edits_match_python() {
    let f = fixture("recurrence.json");
    let mut d = Diffs::default();
    for case in f["edits"].as_array().unwrap() {
        let z = zone(&case["zone"]);
        let now = time(&case["now"]);
        let o = &case["old"];
        let mut old = Schedule::draft(
            o["request"].as_str().unwrap(),
            o["targets"].as_object().unwrap().clone(),
            Rule::from_json(o["rule"].as_object().unwrap()),
            time(&o["first"]),
        );
        old.id = "sc-1".into();
        old.own_words = "words".into();
        old.state = o["state"].as_str().unwrap().into();
        old.until = opt_time(&o["until"]);
        let t = &case["timing"];
        let ts = TimingSpec {
            repeat: t["repeat"].as_str().unwrap().into(),
            times: strings(&t["times"]),
            every_minutes: t["every_minutes"].as_i64().unwrap_or(0),
            weekday: t["weekday"].as_str().unwrap_or("").into(),
            random: windows(&t["random"]),
        };
        let got = match plan_edit(
            &old,
            case["request"].as_str().unwrap(),
            &ts,
            &now,
            case["seed"].as_str().unwrap(),
        ) {
            Ok(p) => plan_json(&p, &now, &z),
            Err(e) => s_err(&e),
        };
        d.check(
            &format!(
                "edit {} at {}: {}",
                case["zone"], case["now"], case["timing"]
            ),
            &case["plan"],
            &got,
        );
    }
    d.finish("edits");
}

#[test]
fn the_words_about_timing_match_python() {
    let f = fixture("recurrence.json");
    let mut d = Diffs::default();
    for case in f["said_times"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        let said: Vec<Value> = said_times(text)
            .into_iter()
            .map(|(s, m)| json!([s, m.into_iter().collect::<Vec<_>>()]))
            .collect();
        d.check(&format!("said_times {text:?}"), &case["said"], &json!(said));
        d.check(
            &format!("missing_times {text:?}"),
            &case["missing"],
            &json!(missing_times(text, &BTreeSet::from([360, 1080, 720]))),
        );
    }
    for case in f["every_said"].as_array().unwrap() {
        let m = case["minutes"].as_i64().unwrap();
        d.check(
            &format!("every_said {m}"),
            &case["said"],
            &json!(every_said(m)),
        );
    }
    for case in f["clock_minutes"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        let got = match clock_minutes(text) {
            Ok(m) => json!({"text": text, "minutes": m}),
            Err(e) => json!({"text": text, "error": e.kind(), "message": e.message()}),
        };
        d.check(&format!("clock_minutes {text:?}"), case, &got);
    }
    for case in f["picks"].as_array().unwrap() {
        let t = Timing {
            times: vec![],
            windows: windows(&case["windows"]),
            seed: case["seed"].as_str().unwrap().into(),
            start: String::new(),
        };
        let day = NaiveDate::parse_from_str(case["day"].as_str().unwrap(), "%Y-%m-%d").unwrap();
        let taken: BTreeSet<i64> = case["taken"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_i64().unwrap())
            .collect();
        d.check(
            &format!("picks {case}"),
            &case["picks"],
            &json!(t.picks(day, &taken).unwrap()),
        );
    }
    for case in f["until_said"].as_array().unwrap() {
        let z = zone(&case["zone"]);
        d.check(
            &format!("until_said {case}"),
            &case["said"],
            &json!(until_said(&time(&case["end"]), &z)),
        );
    }
    for (k, v) in f["watch_said"].as_object().unwrap() {
        d.check(
            &format!("watch_said {k}"),
            v,
            &json!(schedules::watch_said(k)),
        );
    }
    d.finish("timing words");
}

fn when_case(w: &When, now: &Time, z: &Zone) -> Map<String, Value> {
    let r = |x: Result<Time, WhenError>| match x {
        Ok(t) => json!(py::iso(&t)),
        Err(e) => w_err(&e),
    };
    let mut out = Map::new();
    out.insert("resolve".into(), r(when::resolve(w, now, z)));
    out.insert("end_at".into(), r(when::end_at(w, now, z)));
    out.insert("end_of_day".into(), r(when::end_of_day(w, now, z)));
    out
}

#[test]
fn when_resolves_as_python_does() {
    let f = fixture("when.json");
    let mut d = Diffs::default();
    for case in f["cases"].as_array().unwrap() {
        let z = zone(&case["zone"]);
        let now = time(&case["now"]);
        let w = When::from_json(&case["when"]);
        let what = format!("{} at {}: {}", case["zone"], case["now"], case["when"]);
        for (k, v) in when_case(&w, &now, &z) {
            d.check(&format!("{k} {what}"), &case[&k], &v);
        }
        for repeat in ["none", "daily", "every"] {
            let key = format!("present_{repeat}");
            let Some(want) = case.get(&key) else { continue };
            let every = want["every"].as_i64();
            let got = match when::when_present(&w, repeat, &now, every, &z) {
                Ok((due, ret)) => {
                    json!({"every": want["every"], "due": py::iso(&due), "return_after": iso(&ret)})
                }
                Err(e) => {
                    json!({"every": want["every"], "error": e.kind(), "message": e.message()})
                }
            };
            d.check(&format!("{key} {what}"), want, &got);
        }
    }
    d.check("WHEN_SCHEMA", &f["schema"], &when::when_schema());
    d.check("WHEN_HELP", &f["help"], &json!(when::WHEN_HELP));
    d.check("DAY_WORDS", &f["day_words"], &json!(when::day_words()));
    d.finish("when");
}

/// decision: ORCH-320. Fixed against Python: an end whose one day source is empty (`date: ""`) made Python's
/// `_end_day` fail its `assert` (an AssertionError the chat turn doesn't catch); here it is refused in words.
#[test]
fn an_empty_day_source_is_refused_not_a_crash() {
    let z = Zone::named("Asia/Kolkata").unwrap();
    let now = py::parse("2026-11-13T09:00:00+05:30").unwrap();
    let w = When {
        date: Some(String::new()),
        ..When::default()
    };
    let e = when::end_at(&w, &now, &z).unwrap_err();
    assert_eq!(
        e.message(),
        "give exactly one calendar day source: day, date or in_days"
    );
}

#[test]
fn every_fixture_case_was_compared() {
    let f = fixture("recurrence.json");
    let n = ["rules", "plans", "edits"]
        .iter()
        .map(|k| f[k].as_array().unwrap().len())
        .sum::<usize>();
    let w = fixture("when.json")["cases"].as_array().unwrap().len();
    eprintln!("recurrence cases: {n}, when cases: {w}");
    assert!(n >= 2000 && w >= 1000);
    assert!(same(&json!(1.0), &json!(1)));
}
