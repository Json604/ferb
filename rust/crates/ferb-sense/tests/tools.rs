//! The trail tools (`ferb/sense/tools.py`) against `rust/fixtures/sense/tools.json`: time bounds, read sources,
//! `trail.query` and `trail.days`.

mod common;

use std::sync::Arc;

use common::{fixture, tmp, Diffs};
use ferb_clock::clock::FakeClock;
use ferb_clock::zone::Zone;
use ferb_core::ids::SequentialIds;
use ferb_core::types::ToolResult;
use ferb_core::when::{isoformat, parse};
use ferb_sense::attribution::Attribution;
use ferb_sense::exclusions::Exclusions;
use ferb_sense::observation::NoRedact;
use ferb_sense::tools::{
    digest_origins, time_bound, trail_days, trail_query, trail_sources, DaysInput, QueryInput, Tz,
};
use ferb_sense::trail::{Seams, Trail};
use ferb_store::Db;
use rusqlite::params;
use serde_json::{json, Value};

fn sources(r: &[(String, String)]) -> Value {
    json!(r.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>())
}

fn result(r: &ToolResult) -> Value {
    json!({"output": r.content, "read_sources": sources(&r.read_sources), "is_error": r.is_error})
}

// decision: SENSE-408
#[test]
fn time_bounds_and_the_tools_match_python() {
    let f = fixture("tools.json");
    let mut d = Diffs::default();
    let kolkata = Zone::named("Asia/Kolkata").unwrap();
    for c in f["time_bound"].as_array().unwrap() {
        let tz = c[0].as_str().map(|z| Tz::Zone(Zone::named(z).unwrap()));
        let got = match time_bound(c[1].as_str(), c[2].as_bool().unwrap(), tz, &kolkata) {
            Ok(t) => json!(t.map(|t| isoformat(&t))),
            Err(e) => json!({"error": e}),
        };
        d.check(&format!("time_bound {c}"), &c[3], &got);
    }
    for c in f["sources"].as_array().unwrap() {
        let s: Vec<&str> = c[0]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        d.check(
            &format!("sources {}", c[0]),
            &c[1],
            &sources(&trail_sources(&s)),
        );
    }
    let path = tmp().join("db/tools.db");
    let _ = std::fs::remove_file(&path);
    let db = Db::open(&path).unwrap();
    db.migrate().unwrap();
    let clock = FakeClock::new(parse("2026-03-04T12:00:00+05:30")).with_zone(kolkata);
    let ids = SequentialIds::new("t");
    let excl = Exclusions::default();
    let trail = Trail::new(
        &db,
        &clock,
        &ids,
        &NoRedact,
        Seams {
            exclusions: Box::new(|| excl.clone()),
            publish: Box::new(|_, _| {}),
            device_id: Box::new(|| "dev".into()),
            paused: Box::new(|| false),
            test_task: Box::new(|_| false),
            is_browser: Box::new(|_| false),
        },
        Arc::new(Attribution::new(|| 0.0)),
    );
    for (i, (at, source, data)) in [
        (
            "2026-03-03T09:00:00+05:30",
            "window",
            json!({"title": "yesterday"}),
        ),
        (
            "2026-03-04T08:00:00+05:30",
            "browser_tab",
            json!({"host": "a.example", "title": "A"}),
        ),
        (
            "2026-03-04T11:00:00+05:30",
            "app_focus",
            json!({"app": "Notes"}),
        ),
        (
            "2026-03-04T11:30:00+05:30",
            "clipboard",
            json!({"text": "copied"}),
        ),
    ]
    .iter()
    .enumerate()
    {
        db.conn()
            .execute(
                "INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) VALUES \
                 (?, ?, ?, 'dev', 'owner', NULL, ?, ?)",
                params![format!("q{i}"), at, &at[..10], source, ferb_py::json::dumps(data)],
            )
            .unwrap();
    }
    db.conn()
        .execute(
            "INSERT INTO journal (id, at, kind, data_json) VALUES ('s1','t','trail.day_summary',?), \
             ('s2','t','trail.day_narrative',?), ('s3','t','trail.day_summary',?)",
            params![
                ferb_py::json::dumps(&json!({"day": "2026-03-03", "apps_min": {"Notes": 3}})),
                ferb_py::json::dumps(&json!({"day": "2026-03-03", "text": "A day"})),
                ferb_py::json::dumps(&json!({"day": "2026-03-02"}))
            ],
        )
        .unwrap();
    for run in f["query"].as_array().unwrap() {
        let a = &run["args"];
        if a == "no trail" {
            let r = trail_query(None, &QueryInput::default()).unwrap();
            d.check(
                "query without a trail",
                &json!({"output": run["output"], "is_error": run["is_error"]}),
                &json!({"output": r.content, "is_error": r.is_error}),
            );
            continue;
        }
        let st = |k: &str| a.get(k).and_then(Value::as_str).map(String::from);
        let input = QueryInput {
            source: st("source"),
            since_hours: a.get("since_hours").and_then(Value::as_f64).unwrap_or(24.0),
            since: st("since"),
            until: st("until"),
            host: st("host"),
            ..QueryInput::default()
        };
        let r = trail_query(Some(&trail), &input).unwrap();
        d.check(
            &format!("query {a}"),
            &json!({"output": run["output"], "read_sources": run["read_sources"],
                                               "is_error": run["is_error"]}),
            &result(&r),
        );
    }
    let handles = f["handles"].as_object().unwrap().clone();
    let refs = |day: &str| -> Vec<String> {
        handles
            .get(day)
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
            .unwrap_or_default()
    };
    for run in f["days"].as_array().unwrap() {
        let a = &run["args"];
        let input = DaysInput {
            text: a.get("text").and_then(Value::as_str).map(String::from),
            since_days: a.get("since_days").and_then(Value::as_i64).unwrap_or(30),
            limit: a.get("limit").and_then(Value::as_i64).unwrap_or(10),
        };
        let r = trail_days(Some(&trail), &input, &refs).unwrap();
        d.check(
            &format!("days {a}"),
            &json!({"output": run["output"], "read_sources": run["read_sources"]}),
            &json!({"output": r.content, "read_sources": sources(&r.read_sources)}),
        );
    }
    for c in f["digest_origins"].as_array().unwrap() {
        d.check(
            &format!("digest_origins {c}"),
            &c[2],
            &json!(digest_origins(c[0].as_str().unwrap(), &c[1])),
        );
    }
    d.finish("tools");
}
