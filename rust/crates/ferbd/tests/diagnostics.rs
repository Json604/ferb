//! `about.diagnostics` (DMN-595): FERB's own recent diagnostics, read-only, bounded, redacted and labelled as FERB's
//! own records; offered to the planner's lookups and to every worker; one call through the gateway in the real
//! program on a temp home with a seeded log and journal.

mod shim;
mod support;

use std::path::Path;

use ferbd::compose::tools::diagnostics as d;
use serde_json::{json, Value};
use shim::*;
use support::*;

/// The journal rows a test seeds: (at, kind, data, origin).
fn seed(db: &Path, rows: &[(&str, &str, Value, &str)]) {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    for (i, (at, kind, data, origin)) in rows.iter().enumerate() {
        conn.execute(
            "INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![
                format!("seeded-{kind}-{at}-{i}"),
                at,
                kind,
                ferb_py::json::dumps_utf8(data),
                origin
            ],
        )
        .unwrap();
    }
}

const LOG_TEXT: &str = "\
2026-01-01 08:00:00.000 FERB: starting: pid 1, FERB 0.28.0, build 202601010000, started by launchd (com.ferb.daemon)
some other program's output that FERB didn't write
2026-01-01 08:10:00.000 FERB: a panic in thread calls at src/x.rs:1:2: boom
   0: std::backtrace::Backtrace::create
2026-01-01 08:20:00.000 FERB: browser.connection_refused {\"reason\": \"bad_mac\", \"profile\": \"chrome/Profile 1\"}
2026-01-01 08:30:00.000 FERB: a stray value: Card 4111 1111 1111 1111, Your verification code is 482913.
";

fn rows() -> Vec<(&'static str, &'static str, Value, &'static str)> {
    vec![
        // outside the 24 hours before the fake clock's 2026-01-01T09:00:00Z
        (
            "2025-12-30T08:00:00+00:00",
            "browsers.pair_failed",
            json!({"step": "old"}),
            "ferb",
        ),
        (
            "2026-01-01T08:00:00+00:00",
            "browsers.pair_failed",
            json!({"step": "quiet", "seen": "no connection"}),
            "ferb",
        ),
        (
            "2026-01-01T08:05:00+00:00",
            "browsers.pair_failed",
            json!({"step": "fallback", "seen": "timeout"}),
            "ferb",
        ),
        (
            "2026-01-01T08:06:00+00:00",
            "screen.capture_timeout",
            json!({"timeout_s": 10.0}),
            "ferb",
        ),
        // a kind nobody listed: it shows because the journal labels it FERB's own
        (
            "2026-01-01T08:07:00+00:00",
            "browser.profile_disconnected",
            json!({"profile": "chrome/Default", "tabs": 2}),
            "ferb",
        ),
        // the owner's chat (chat.reply) and anything not labelled FERB's own stay out; FERB's own
        // acknowledgement is its record
        (
            "2026-01-01T08:08:00+00:00",
            "chat.reply",
            json!({"text": "a reply"}),
            "ferb",
        ),
        (
            "2026-01-01T08:08:30+00:00",
            "chat.ack",
            json!({"request_id": "r1"}),
            "ferb",
        ),
        (
            "2026-01-01T08:09:00+00:00",
            "chat.message",
            json!({"text": "owner words"}),
            "owner",
        ),
        (
            "2026-01-01T08:09:30+00:00",
            "task.outcome",
            json!({"text": "from a page"}),
            "external:web:shop.example,ferb",
        ),
    ]
}

fn kinds(v: &Value) -> Vec<String> {
    v["journal"]["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["kind"].as_str().unwrap().to_owned())
        .collect()
}

// decision: DMN-595
#[test]
fn the_diagnostics_read_ferb_s_own_records_bounded_and_change_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    std::fs::create_dir_all(home.join("logs")).unwrap();
    std::fs::write(home.join(d::LOG), LOG_TEXT).unwrap();
    drop(ferbd::basics::open_store(&home.join("ferb.db")).unwrap());
    seed(&home.join("ferb.db"), &rows());
    let before = |p: &Path| std::fs::read(p).unwrap();
    let (db0, log0) = (before(&home.join("ferb.db")), before(&home.join(d::LOG)));
    let since = "2025-12-31T09:00:00+00:00";
    let out = d::compose(
        home,
        since,
        None,
        json!({"ferb": "0.28.0"}),
        json!({}),
        vec![],
    )
    .unwrap();
    let out = Value::Object(out);
    // the log: only the lines FERB's log writer wrote, oldest first
    let lines: Vec<&str> = out["log"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert!(lines[0].contains("starting: pid 1"));
    assert!(lines
        .iter()
        .all(|l| !l.contains("other program") && !l.contains("Backtrace")));
    // the journal: FERB's own kinds in the window, newest first, each with its count and latest data
    assert_eq!(
        kinds(&out),
        [
            "chat.ack",
            "browser.profile_disconnected",
            "screen.capture_timeout",
            "browsers.pair_failed"
        ]
    );
    let pair = &out["journal"]["kinds"][3];
    assert_eq!(pair["count"], 2);
    assert_eq!(pair["first_at"], "2026-01-01T08:00:00+00:00");
    assert_eq!(pair["latest"], r#"{"step": "fallback", "seen": "timeout"}"#);
    // one family, newest first
    let fam = Value::Object(
        d::compose(home, since, Some("browsers."), json!({}), json!({}), vec![]).unwrap(),
    );
    assert_eq!(fam["journal"]["total"], 2);
    assert_eq!(
        fam["journal"]["records"][0]["data"],
        r#"{"step": "fallback", "seen": "timeout"}"#
    );
    let chat = Value::Object(
        d::compose(home, since, Some("chat."), json!({}), json!({}), vec![]).unwrap(),
    );
    // the chat family: FERB's acknowledgement only, never the owner's words or a reply
    assert_eq!(chat["journal"]["total"], 1, "{chat}");
    assert_eq!(chat["journal"]["records"][0]["kind"], "chat.ack", "{chat}");
    // nothing written: the store and the log are byte for byte as they were
    assert_eq!(before(&home.join("ferb.db")), db0);
    assert_eq!(before(&home.join(d::LOG)), log0);

    // bounded: a long log and many kinds are cut to MAX_CHARS, the oldest first, and counted
    let long: String = (0..2000)
        .map(|i| {
            format!(
                "2026-01-01 08:{:02}:00.000 FERB: line {i} {}\n",
                i % 60,
                "x".repeat(400)
            )
        })
        .collect();
    std::fs::write(home.join(d::LOG), long).unwrap();
    let many: Vec<(String, String)> = (0..80)
        .map(|i| {
            (
                format!("2026-01-01T08:{:02}:00+00:00", i % 60),
                format!("kind.n{i}"),
            )
        })
        .collect();
    let rows: Vec<(&str, &str, Value, &str)> = many
        .iter()
        .map(|(at, k)| {
            (
                at.as_str(),
                k.as_str(),
                json!({"text": "y".repeat(900)}),
                "ferb",
            )
        })
        .collect();
    seed(&home.join("ferb.db"), &rows);
    let big = d::compose(home, since, None, json!({}), json!({}), vec![]).unwrap();
    let text = ferb_py::json::dumps_utf8(&big);
    assert!(text.chars().count() <= d::MAX_CHARS, "{}", text.len());
    assert!(big["omitted"].as_u64().unwrap() > 0);
    let last = big["log"]["lines"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .as_str()
        .unwrap();
    assert!(last.contains("line 1999 "), "the newest line stays: {last}");
    assert!(last.chars().count() <= d::LINE_CHARS + 1);
}

// decision: DMN-595
#[test]
fn the_planner_and_workers_get_it_and_a_call_through_the_gateway_is_redacted_and_labelled() {
    // the planner's lookups name it (the supervisor offers each one that is installed)
    assert!(ferb_supervisor::words::PLANNER_TOOLS.contains(&d::NAME));
    let tmp = root();
    let (run, mut shim) = started(tmp.path(), "01");
    // a worker's listing (every scope) and the planner's (its own grant) both hold it
    let worker = run.control(json!({"do": "listing", "identity": "owner", "scopes": all_scopes()}));
    let names = |l: &Value| -> Vec<String> {
        l.as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect()
    };
    assert!(names(&worker).contains(&d::NAME.to_owned()));
    let planner = run.control(json!({"do": "listing", "identity": "owner",
        "scopes": ferb_supervisor::words::PLANNER_SCOPES, "observe_only": true,
        "tools": ferb_supervisor::words::PLANNER_TOOLS, "max_calls": 10}));
    assert!(names(&planner).contains(&d::NAME.to_owned()), "{planner}");
    let ferb = run.control(json!({"do": "listing", "identity": "ferb", "scopes": []}));
    assert!(names(&ferb).contains(&d::NAME.to_owned()));

    std::fs::create_dir_all(run.home.join("logs")).unwrap();
    std::fs::write(run.home.join(d::LOG), LOG_TEXT).unwrap();
    seed(&run.home.join("ferb.db"), &rows());
    let got = shim.call(d::NAME, json!({}));
    assert_eq!(got["is_error"], false, "{got}");
    let c = &got["content"];
    assert_eq!(c["running"]["ferb"], "0.28.0", "{c}");
    assert!(c["browsers"]["profiles"].is_array(), "{c}");
    assert!(!c["permissions"].as_array().unwrap().is_empty(), "{c}");
    assert!(kinds(c).contains(&"browsers.pair_failed".to_owned()), "{c}");
    assert!(
        !kinds(c)
            .iter()
            .any(|k| ["chat.reply", "chat.message", "task.outcome"].contains(&k.as_str())),
        "{c}"
    );
    // the same redaction as every result: no card number or code reaches the model
    let all = got.to_string();
    assert!(
        !all.contains("4111 1111") && !all.contains("482913"),
        "{all}"
    );
    assert!(c["log"]["lines"].as_array().unwrap().len() == 4, "{c}");
    // labelled FERB's own records
    assert!(got["footer"].to_string().contains("Origin: ferb"), "{got}");
    // the schema's checks: an empty window or an unknown argument is refused; one kind narrows it
    let bad = shim.call(d::NAME, json!({"since_hours": 0}));
    assert_eq!(bad["is_error"], true, "{bad}");
    let extra = shim.call(d::NAME, json!({"path": "config.toml"}));
    assert_eq!(extra["is_error"], true, "{extra}");
    let one = shim.call(
        d::NAME,
        json!({"kind": "screen.capture_timeout", "since_hours": 48}),
    );
    assert_eq!(one["content"]["journal"]["total"], 1, "{one}");
}
