//! ORCH-660, ORCH-661: an action check (sent, paid, submitted, trashed, deleted) proves "not done" only when every route
//! by which the action could have happened in the task is one it reads; else code can't tell, its evidence names what
//! was recorded, and the checker's look decides. And it never says "not done" beside a card of the owner's result that
//! could be the action. Through the real checks on a real store with recorded calls (invented hosts and names, shaped
//! as the live task 0abecb83 of 2026-11-16 recorded them).

#![cfg(feature = "store")]

mod common;

use common::{real_tempdir, TestPolicy};
use ferb_context::Py;
use ferb_orchestrator::models::Condition;
use ferb_orchestrator::verify::{self, CheckHost, Checks, ConditionResult, PathError, CANT_SETTLE};
use ferb_orchestrator::{cards, ports, sql, validate};
use ferb_store::Db;
use serde_json::{json, Value};

const SITE: &str = "study.example.edu";
const OTHER: &str = "seek.example.edu";
const TASK: &str = "t1.worker";

struct Host {
    db: Db,
    home: String,
}

impl CheckHost for Host {
    fn resolve(&self, path: &str, _must_exist: bool) -> Result<String, PathError> {
        Ok(match path.strip_prefix("~/") {
            Some(rest) => format!("{}/{rest}", self.home),
            None => path.to_string(),
        })
    }
    fn urls_of(&self, _: &str) -> Vec<String> {
        Vec::new()
    }
    fn latest_acting_call(&self, _: &str, _: Option<&str>) -> sql::Result<Option<Value>> {
        Ok(None)
    }
    fn policy(&self) -> &dyn ports::Policy {
        &TestPolicy
    }
    fn db(&self) -> Option<&Db> {
        Some(&self.db)
    }
}

struct World {
    _dir: tempfile::TempDir,
    host: Host,
    n: usize,
}

fn world() -> World {
    let (dir, real) = real_tempdir();
    let db = Db::open(real.join("ferb.db")).unwrap();
    db.migrate().unwrap();
    let home = real.join("user").to_string_lossy().into_owned();
    std::fs::create_dir_all(&home).unwrap();
    World {
        _dir: dir,
        host: Host { db, home },
        n: 0,
    }
}

impl World {
    /// A finished tool call of the worker's task: (input, output, error).
    fn call(&mut self, input: Value, output: Value, error: Option<&str>) {
        self.n += 1;
        self.host
            .db
            .conn()
            .execute(
                "INSERT INTO spans (id, task_id, kind, started_at, ended_at, input_json, output_json, error) \
                 VALUES (?, ?, 'tool.call', ?, ?, ?, ?, ?)",
                rusqlite::params![
                    format!("s{}", self.n),
                    TASK,
                    format!("2026-01-01T09:00:{:02}+00:00", self.n),
                    format!("2026-01-01T09:00:{:02}+00:00", self.n),
                    ferb_py::json::dumps(&input),
                    ferb_py::json::dumps(&output),
                    error
                ],
            )
            .unwrap();
    }

    /// The owner's Yes (or No) recorded for a rule.
    fn answer(&self, id: &str, rule: &str, granted: bool) {
        self.host
            .db
            .conn()
            .execute(
                "INSERT INTO confirmations (id, tree_id, rule, key, granted, at) VALUES (?, 't1', ?, ?, ?, \
                 '2026-01-01T09:00:00+00:00')",
                rusqlite::params![id, rule, format!("{rule}:k"), granted as i64],
            )
            .unwrap();
    }

    fn check(&self, check: Value) -> (Condition, ConditionResult) {
        let c = condition(check);
        let r = Checks { host: &self.host }.run(0, &c, TASK).unwrap();
        (c, r)
    }

    /// The owner's result cards of the request (`ran_cards`: payments, then sends).
    fn cards(&self) -> Vec<Value> {
        let mut out = cards::payments(&self.host.db, &TestPolicy, "t1").unwrap();
        out.extend(cards::sends(&self.host.db, "t1").unwrap());
        out
    }
}

fn condition(check: Value) -> Condition {
    let mut full = json!({"kind": null, "path": null, "text": null, "url_contains": null, "site": null,
                          "expected": null, "tool": null});
    for (k, v) in check.as_object().unwrap() {
        full[k] = v.clone();
    }
    validate::task_spec_validate(&Py::from_value(&json!({"goal": "g", "identity": "owner",
        "success_conditions": [{"text": "Every open assignment is completed and submitted.", "owner_text": "",
                                "check": full, "layer": "outcome"}]})))
    .unwrap()
    .success_conditions[0]
        .clone()
}

fn ok() -> Value {
    json!({"ok": true, "after": {"changed": false}, "observed_change": false})
}

fn changed() -> Value {
    json!({"ok": true, "after": {"changed": true}, "observed_change": true})
}

/// The live task's records, shaped as stored: reads, input into Chrome's window through the Mac tools under the
/// owner's Yes to screen use, a send to the course's grader through Chrome (one of them ended with an error, its
/// outcome unknown), then the browser tools' clicks on the university's other host.
fn the_live_task(w: &mut World) {
    w.answer("cf-screen", "floor:foreground", true);
    w.answer("cf-new", "floor:new_recipient", true);
    w.call(
        json!({"tool": "browser.tabs", "kind": "read", "target": {}}),
        ok(),
        None,
    );
    w.call(
        json!({"tool": "mac.snapshot", "kind": "read", "target": {"site": "com.google.Chrome"}}),
        ok(),
        None,
    );
    w.call(
        json!({"tool": "mac.act", "kind": "mac_input", "target": {"site": "com.google.Chrome"},
                  "confirmations": ["cf-screen"]}),
        ok(),
        None,
    );
    w.call(
        json!({"tool": "mac.act", "kind": "mac_input", "fact_kinds": ["send"],
                  "target": {"site": "com.google.Chrome", "recipients": ["Course grader"],
                             "account": "student@study.example.edu"},
                  "confirmations": ["cf-screen", "cf-new"]}),
        ok(),
        None,
    );
    w.call(json!({"tool": "mac.click", "kind": "mac_input", "fact_kinds": ["send"],
                  "target": {"site": "com.google.Chrome", "recipients": ["Course grader"]},
                  "confirmations": ["cf-screen"]}), changed(),
           Some("Yielded: stopped for the owner after 0 of 2 steps (click); the owner used the mouse/keyboard"));
    w.call(
        json!({"tool": "browser.act", "kind": "click", "target": {"site": SITE,
                  "page": "https://study.example.edu/dashboard"}, "confirmations": []}),
        ok(),
        None,
    );
    w.call(
        json!({"tool": "browser.act", "kind": "click", "target": {"site": OTHER,
                  "page": "https://seek.example.edu/courses/c1"}, "confirmations": []}),
        changed(),
        None,
    );
}

/// The live case: the work went through Chrome's window (Mac tools) and to another host than the condition named.
/// Code can't tell (it no longer proves the condition false), says what it recorded, and the result's Sent card
/// stands beside it without contradiction.
// decision: ORCH-660, ORCH-661
#[test]
fn a_submit_done_through_the_window_is_not_proved_false() {
    let mut w = world();
    the_live_task(&mut w);
    let (_, r) = w.check(json!({"kind": "submitted", "site": SITE}));
    assert_eq!(r.met, None, "{r:?}");
    assert_eq!(r.path, "code");
    assert_eq!(
        r.evidence,
        format!(
            "{CANT_SETTLE}: they show no form on {SITE} submitted in this task, but input went into com.google.Chrome \
             through the Mac tools; a send to Course grader went through com.google.Chrome"
        )
    );
    assert!(w
        .cards()
        .iter()
        .any(|c| c["type"] == "send" && c["recipient"] == "Course grader"));
}

/// A browser-tool submit recorded to a different host: no proof for the named host, and no "nothing was submitted"
/// either. The evidence names the host it went to.
// decision: ORCH-660
#[test]
fn a_browser_submit_to_another_host_is_cant_tell_naming_that_host() {
    let mut w = world();
    w.call(
        json!({"tool": "browser.act", "kind": "submit", "target": {"site": OTHER}}),
        changed(),
        None,
    );
    let (_, r) = w.check(json!({"kind": "submitted", "site": SITE}));
    assert_eq!(r.met, None, "{r:?}");
    assert_eq!(
        r.evidence,
        format!(
            "{CANT_SETTLE}: they show no form on {SITE} submitted in this task, but a form was submitted to {OTHER}"
        )
    );
}

/// The protection the check exists for: a worker claiming it submitted when no call that could submit exists (reads
/// only, or a browser click on the named site that changed nothing) is still proved false, told as before.
// decision: ORCH-660, ORCH-609
#[test]
fn a_claimed_submit_with_no_acting_call_is_still_proved_false() {
    for with_click in [false, true] {
        let mut w = world();
        w.call(
            json!({"tool": "browser.snapshot", "kind": "read", "target": {"site": SITE}}),
            ok(),
            None,
        );
        w.call(json!({"tool": "mac.snapshot", "kind": "read", "target": {"site": "com.google.Chrome"}}), ok(), None);
        if with_click {
            w.call(
                json!({"tool": "browser.act", "kind": "click", "target": {"site": SITE}}),
                ok(),
                None,
            );
        }
        let (c, r) = w.check(json!({"kind": "submitted", "site": SITE}));
        assert_eq!(r.met, Some(false), "{r:?}");
        assert_eq!(
            r.evidence,
            format!("FERB didn't submit it; no form on {SITE} went through in this task")
        );
        assert_eq!(
            verify::not_done_said(&c),
            format!("No form on {SITE} was submitted.")
        );
    }
}

/// A recorded form submission to the named host is met by code, as before.
// decision: ORCH-576, ORCH-624
#[test]
fn a_recorded_submit_to_the_named_host_is_met_by_code() {
    let mut w = world();
    w.call(
        json!({"tool": "browser.act", "kind": "submit", "target": {"site": SITE}}),
        changed(),
        None,
    );
    let (_, r) = w.check(json!({"kind": "submitted", "site": SITE}));
    assert_eq!(r.met, Some(true), "{r:?}");
    assert_eq!(r.evidence, format!("FERB submitted the form on {SITE}"));
}

/// An attempt at the action on its own target that ended with an error doesn't record that nothing happened.
// decision: ORCH-660
#[test]
fn an_attempt_that_ended_with_an_error_is_cant_tell() {
    let mut w = world();
    w.call(
        json!({"tool": "browser.act", "kind": "submit", "target": {"site": SITE}}),
        ok(),
        Some("timed out"),
    );
    let (_, r) = w.check(json!({"kind": "submitted", "site": SITE}));
    assert_eq!(r.met, None, "{r:?}");
    assert!(r.evidence.ends_with(&format!(
        "but an attempt at it on {SITE} ended with an error, which doesn't record whether it went through"
    )));
}

/// Trash and delete: a shell command, a script or the trash of a folder holding the path are routes the check
/// doesn't read; with none of them the check still proves it.
// decision: ORCH-660
#[test]
fn trash_and_delete_count_shell_scripts_and_folders() {
    // paths as the gateway records them, resolved (the process's HOME is never read or changed here)
    let cases: Vec<(Value, Option<&str>)> = vec![
        (
            json!({"tool": "shell.run", "kind": "run_shell", "target": {}}),
            Some("a shell command ran"),
        ),
        (
            json!({"tool": "mac.script", "kind": "applescript", "target": {"site": "com.apple.finder"}}),
            Some("a script ran in com.apple.finder"),
        ),
        (
            json!({"tool": "fs.trash", "kind": "trash", "target": {"path": "HOME/old"}}),
            Some("was moved to the Trash"),
        ),
        (
            json!({"tool": "fs.trash", "kind": "trash", "target": {"path": "HOME/elsewhere.txt"}}),
            None,
        ),
        (
            json!({"tool": "fs.trash", "kind": "trash", "target": {"path": "HOME/ol"}}),
            None,
        ),
        (
            json!({"tool": "fs.read", "kind": "read", "target": {"path": "HOME/old/a.txt"}}),
            None,
        ),
    ];
    for (call, route) in cases {
        let mut w = world();
        let call: Value =
            serde_json::from_str(&call.to_string().replace("HOME", &w.host.home)).unwrap();
        w.call(call.clone(), ok(), None);
        let (_, r) = w.check(json!({"kind": "trashed", "path": "~/old/a.txt"}));
        match route {
            Some(said) => {
                assert_eq!(r.met, None, "{call} {r:?}");
                assert!(r.evidence.contains(said), "{call} {r:?}");
            }
            None => assert_eq!(r.met, Some(false), "{call} {r:?}"),
        }
    }
}

/// The consistency rule, over every combination of the recorded call kinds: whenever code proves a send, a submit or a
/// payment false, the owner's result carries no card that could be it (a send card naming the recipient or no one, any
/// send card for a submit, any payment card for a payment). So the answer never says "Nothing was sent" or "No form
/// was submitted" beside a Sent card.
// decision: ORCH-661
#[test]
fn a_not_done_line_never_stands_beside_a_card_that_could_be_it() {
    let calls: Vec<(Value, Value)> = vec![
        (
            json!({"tool": "mail.send", "kind": "mailbox_send", "target": {"recipients": ["ana@x.example"]}}),
            ok(),
        ),
        (
            json!({"tool": "mail.send", "kind": "mailbox_send", "target": {"recipients": ["zed@x.example"]}}),
            ok(),
        ),
        (
            json!({"tool": "browser.act", "kind": "send", "target": {"site": SITE}}),
            ok(),
        ),
        (
            json!({"tool": "browser.act", "kind": "submit", "target": {"site": OTHER}}),
            changed(),
        ),
        (
            json!({"tool": "browser.act", "kind": "pay", "target": {"site": OTHER}, "confirmations": ["cf-pay"]}),
            ok(),
        ),
        (
            json!({"tool": "browser.act", "kind": "click", "target": {"site": SITE}}),
            ok(),
        ),
        (
            json!({"tool": "mac.act", "kind": "mac_input", "fact_kinds": ["send"],
                "target": {"site": "com.google.Chrome", "recipients": ["Course grader"]}}),
            ok(),
        ),
    ];
    let checks = [
        json!({"kind": "sent", "expected": "zed@x.example"}),
        json!({"kind": "sent", "expected": "Course grader", "site": SITE}),
        json!({"kind": "submitted", "site": SITE}),
        json!({"kind": "paid", "site": SITE}),
    ];
    let mut proved = 0;
    for mask in 0..(1u32 << calls.len()) {
        let mut w = world();
        w.answer("cf-pay", "floor:pay", true);
        for (i, (input, output)) in calls.iter().enumerate() {
            if mask & (1 << i) != 0 {
                w.call(input.clone(), output.clone(), None);
            }
        }
        let cards = w.cards();
        for check in &checks {
            let (c, r) = w.check(check.clone());
            if r.met != Some(false) {
                continue;
            }
            proved += 1;
            let told = verify::not_done_said(&c);
            let clash = cards
                .iter()
                .find(|card| match check["kind"].as_str().unwrap() {
                    "sent" => {
                        card["type"] == "send"
                            && card.get("recipient").is_none_or(|r| {
                                ferb_policy::confirm::account_key(r.as_str())
                                    == ferb_policy::confirm::account_key(check["expected"].as_str())
                            })
                    }
                    "submitted" => card["type"] == "send",
                    _ => card["type"] == "payment",
                });
            assert!(clash.is_none(), "mask {mask}: {told:?} beside {clash:?}");
        }
    }
    assert!(
        proved > 0,
        "some combination still proves a condition false"
    );
}
