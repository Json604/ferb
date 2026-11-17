//! ORCH-664, ORCH-666, ORCH-667, ORCH-668, ORCH-670: an action check says only what code can see itself: the Mac's
//! state now (a file there or gone, its bytes, the Trash, a taken download) and the receipts FERB's tools wrote when
//! they acted (which bytes, to whom, where). It never concludes "not done" from a missing record of one kind of call,
//! and it says "different" only when two values of the same kind differ. Through the real checks on a real store with
//! recorded calls (invented hosts, names and files; the window route shaped as the live task 0abecb83 of 2026-11-16).

#![cfg(feature = "store")]

mod common;

use common::{real_tempdir, TestPolicy};
use ferb_context::Py;
use ferb_orchestrator::models::Condition;
use ferb_orchestrator::verify::{self, CheckHost, Checks, ConditionResult, PathError};
use ferb_orchestrator::{cards, ports, sql, validate};
use ferb_store::Db;
use serde_json::{json, Value};

const SITE: &str = "study.example.edu";
const OTHER: &str = "seek.example.edu";
const TASK: &str = "t1.worker";

struct Host {
    db: Db,
    home: String,
    trash: String,
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
    fn trash_dir(&self) -> Option<String> {
        Some(self.trash.clone())
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
    let trash = real.join("user/.Trash").to_string_lossy().into_owned();
    std::fs::create_dir_all(&trash).unwrap();
    World {
        _dir: dir,
        host: Host { db, home, trash },
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

impl World {
    /// A file under the invented home, with its bytes; its full path.
    fn file(&self, rel: &str, bytes: &[u8]) -> String {
        let p = format!("{}/{rel}", self.host.home);
        std::fs::create_dir_all(std::path::Path::new(&p).parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        p
    }

    /// FERB's upload tool's receipt as the browser records it (`source: local`, each file's path and SHA-256).
    fn upload(&mut self, files: &[(&str, &str)], bytes_match: Value) {
        let identities: Vec<Value> = files
            .iter()
            .map(|(p, sha)| json!({"path": p, "size": 3, "modified_ns": 1, "sha256": sha, "owner_uid": 501}))
            .collect();
        self.call(
            json!({"tool": "browser.act", "kind": "fill_form", "args": {"tab": "t1", "action": "upload",
                   "files": files.iter().map(|(p, _)| *p).collect::<Vec<_>>()},
                   "target": {"site": SITE}}),
            json!({"ok": true, "source": "local", "files": identities, "bytes_match": bytes_match}),
            None,
        );
    }

    /// A taken browser download of the task.
    fn download(&self, path: &str, sha: &str) {
        self.host
            .db
            .conn()
            .execute(
                "INSERT INTO downloads (id, task_id, profile, identity, state, path, name, bytes, sha256, origin, at) \
                 VALUES (?, ?, 'p', 'ferb', 'taken', ?, 'n', 3, ?, 'o', '2026-01-01T09:00:00+00:00')",
                rusqlite::params![format!("d{path}"), TASK, path, sha],
            )
            .unwrap();
    }
}

fn sha(bytes: &[u8]) -> String {
    let p = std::env::temp_dir().join(format!("ferb-sha-{}", std::process::id()));
    std::fs::write(&p, bytes).unwrap();
    let out = verify::sha256_of(&p.to_string_lossy()).unwrap();
    std::fs::remove_file(&p).unwrap();
    out
}

fn verdict(r: &ConditionResult) -> (Option<bool>, &str, &str) {
    (r.met, r.verdict.as_str(), r.by.as_str())
}

/// The live case: the work went through Chrome's window (Mac tools) and to another host than the condition named.
/// Code gives no verdict (a receipt-less route proves nothing), says what it recorded, and the Sent card stands.
// decision: ORCH-664, ORCH-666
#[test]
fn a_submit_done_through_the_window_is_left_to_the_check() {
    let mut w = world();
    the_live_task(&mut w);
    let (_, r) = w.check(json!({"kind": "submitted", "site": SITE}));
    assert_eq!(verdict(&r), (None, "cant_tell", "receipt"), "{r:?}");
    assert_eq!(r.path, "code");
    assert!(
        r.evidence
            .contains(&format!("FERB acted 1 time(s) on {SITE}")),
        "{r:?}"
    );
    assert!(w
        .cards()
        .iter()
        .any(|c| c["type"] == "send" && c["recipient"] == "Course grader"));
}

/// The Goibibo shape (task d05e8354, 2026-11-17): a script button pressed with plain clicks, one of which loaded a new
/// document and one whose page couldn't be read after it. Code has no verdict; the facts name the clicks.
// decision: ORCH-664, ORCH-666
#[test]
fn plain_clicks_on_a_script_button_are_facts_not_a_verdict() {
    let mut w = world();
    for (changed, note) in [
        (json!(true), "the page loaded a new document"),
        (Value::Null, "nothing changed within 0.5 s"),
        (
            Value::Null,
            "unknown: the page couldn't be read after the act",
        ),
    ] {
        w.call(
            json!({"tool": "browser.act", "kind": "click", "args": {"tab": "t21"},
                   "target": {"site": "www.claims.example", "page": "https://www.claims.example/trips"}}),
            json!({"ok": true, "after": {"read": "page", "changed": changed, "note": note}}),
            None,
        );
    }
    let (_, r) = w.check(json!({"kind": "submitted", "site": "claims.example"}));
    assert_eq!(verdict(&r), (None, "cant_tell", "receipt"), "{r:?}");
    assert!(r.evidence.contains("FERB acted 3 time(s)"), "{r:?}");
    assert!(
        r.evidence
            .contains("unknown: the page couldn't be read after the act"),
        "{r:?}"
    );
}

/// The property behind the redesign, over every combination of recorded calls: code alone never makes a send, a
/// payment or a submit false; it says "different" for a send only when the recipients are addresses of the same kind,
/// never from a name; and a page's own submit event, seen during a click, is reported as a fact.
// decision: ORCH-664, ORCH-668
#[test]
fn code_alone_never_proves_a_send_a_payment_or_a_submit_false() {
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
            json!({"tool": "browser.act", "kind": "click", "declared": "send", "target": {"site": SITE,
                   "recipients": ["bob@x.example"]}}),
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
            json!({"ok": true, "facts": {"submits_form": true}}),
        ),
        (
            json!({"tool": "mac.act", "kind": "mac_input", "fact_kinds": ["send"],
                "target": {"site": "com.google.Chrome", "recipients": ["Course grader"]}}),
            ok(),
        ),
    ];
    let checks = [
        json!({"kind": "sent", "expected": "zed@x.example"}),
        json!({"kind": "sent", "expected": "Zed"}),
        json!({"kind": "sent", "expected": "Course grader", "site": SITE}),
        json!({"kind": "submitted", "site": SITE}),
        json!({"kind": "paid", "site": SITE}),
        json!({"kind": "paid"}),
    ];
    let (mut different, mut submit_seen) = (0, 0);
    for mask in 0..(1u32 << calls.len()) {
        let mut w = world();
        w.answer("cf-pay", "floor:pay", true);
        for (i, (input, output)) in calls.iter().enumerate() {
            if mask & (1 << i) != 0 {
                w.call(input.clone(), output.clone(), None);
            }
        }
        for check in &checks {
            let (_, r) = w.check(check.clone());
            assert_ne!(
                r.met,
                Some(true),
                "mask {mask} {check}: code never proves these done: {r:?}"
            );
            if r.met == Some(false) {
                assert_eq!(r.verdict, "different", "mask {mask} {check}: {r:?}");
                // only an email address against FERB's own sends to other email addresses
                assert_eq!(check["expected"], "zed@x.example", "mask {mask}: {r:?}");
                assert!(mask & 0b10 == 0 && mask & 0b1 != 0, "mask {mask}: {r:?}");
                assert_eq!(r.acted_on, "ana@x.example");
                different += 1;
            }
            if r.evidence.contains("the page reported a form submitted") {
                submit_seen += 1;
            }
        }
    }
    assert!(different > 0 && submit_seen > 0);
}

/// A declared send (the worker's word) is never a receipt; the tool's resolved recipients are. A phone number with a
/// country code is the same number.
// decision: ORCH-668
#[test]
fn recipients_compare_only_as_the_same_kind_of_address() {
    let mut w = world();
    w.call(
        json!({"tool": "whatsapp.send", "kind": "send", "target": {"recipients": ["+91 98765 43210"]}}),
        ok(),
        None,
    );
    let (_, r) = w.check(json!({"kind": "sent", "expected": "98765-43210"}));
    assert_eq!(verdict(&r), (None, "cant_tell", "receipt"), "{r:?}");
    assert!(
        r.evidence.starts_with("FERB's own send to 98765-43210"),
        "{r:?}"
    );
    let (_, r) = w.check(json!({"kind": "sent", "expected": "+91 99999 00000"}));
    assert_eq!(verdict(&r), (Some(false), "different", "receipt"), "{r:?}");
    assert_eq!(r.asked, "+91 99999 00000");
    assert_eq!(r.acted_on, "+91 98765 43210");
    let (_, r) = w.check(json!({"kind": "sent", "expected": "Riya"}));
    assert_eq!(
        verdict(&r),
        (None, "cant_tell", "receipt"),
        "a name is never compared: {r:?}"
    );
}

/// The owner's Drive case (2026-11-17) and its neighbours, by the bytes FERB put on the page's file field.
// decision: ORCH-667, ORCH-668, ORCH-670
#[test]
fn an_upload_is_judged_by_the_bytes_that_left_the_mac() {
    // no call that could carry a file from the Mac: a record fact, not met (the check may still see a file: the
    // supervisor makes that "different")
    let mut w = world();
    let report = w.file("Desktop/report.pdf", b"mac");
    w.call(
        json!({"tool": "browser.act", "kind": "click", "args": {"tab": "t1"}, "target": {"site": SITE}}),
        changed(),
        None,
    );
    let (_, r) = w.check(json!({"kind": "uploaded", "path": "~/Desktop/report.pdf"}));
    assert_eq!(verdict(&r), (Some(false), "not_met", "record"), "{r:?}");
    assert_eq!(r.asked, report);

    // FERB's upload of the named file: a receipt, no verdict
    let mut w = world();
    let report = w.file("Desktop/report.pdf", b"mac");
    w.upload(&[(&report, &sha(b"mac"))], json!(true));
    let (_, r) = w.check(json!({"kind": "uploaded", "path": "~/Desktop/report.pdf"}));
    assert_eq!(verdict(&r), (None, "cant_tell", "receipt"), "{r:?}");
    assert!(
        r.evidence.contains("the page's file field holds its bytes"),
        "{r:?}"
    );

    // ORCH-670: the named file changed since the upload: the same path is the same file, nothing to tell
    let mut w = world();
    let report = w.file("Desktop/report.pdf", b"edited later");
    w.upload(&[(&report, &sha(b"mac"))], json!(true));
    let (_, r) = w.check(json!({"kind": "uploaded", "path": "~/Desktop/report.pdf"}));
    assert_eq!(verdict(&r), (None, "cant_tell", "receipt"), "{r:?}");

    // the same bytes under another path are the same file
    let mut w = world();
    w.file("Desktop/report.pdf", b"mac");
    let copy = w.file("Documents/copy.pdf", b"mac");
    w.upload(&[(&copy, &sha(b"mac"))], json!(true));
    let (_, r) = w.check(json!({"kind": "uploaded", "path": "~/Desktop/report.pdf"}));
    assert_eq!(verdict(&r), (None, "cant_tell", "receipt"), "{r:?}");

    // another file with the same name in another folder: different
    let mut w = world();
    let report = w.file("Desktop/report.pdf", b"mac");
    let other = w.file("Downloads/report.pdf", b"other");
    w.upload(&[(&other, &sha(b"other"))], json!(true));
    let (_, r) = w.check(json!({"kind": "uploaded", "path": "~/Desktop/report.pdf"}));
    assert_eq!(verdict(&r), (Some(false), "different", "receipt"), "{r:?}");
    assert_eq!(
        (r.asked.as_str(), r.acted_on.as_str()),
        (report.as_str(), other.as_str())
    );

    // the page's field holds other bytes than FERB gave it
    let mut w = world();
    let report = w.file("Desktop/report.pdf", b"mac");
    w.upload(&[(&report, &sha(b"mac"))], json!(false));
    let (_, r) = w.check(json!({"kind": "uploaded", "path": "~/Desktop/report.pdf"}));
    assert_eq!(verdict(&r), (Some(false), "different", "receipt"), "{r:?}");

    // input through the Mac tools could have carried it (a drag, a file dialog): no fact either way
    let mut w = world();
    w.file("Desktop/report.pdf", b"mac");
    w.call(
        json!({"tool": "mac.act", "kind": "mac_input", "target": {"site": "com.google.Chrome"}}),
        ok(),
        None,
    );
    let (_, r) = w.check(json!({"kind": "uploaded", "path": "~/Desktop/report.pdf"}));
    assert_eq!(verdict(&r), (None, "cant_tell", "receipt"), "{r:?}");
    assert!(r.evidence.contains("mac.act ran"), "{r:?}");
}

/// Trash and delete are read on the Mac now: still there is not met whatever the records say; gone and in the Trash
/// is met however it got there; the other of the two actions, or the same action on another path, is different.
// decision: ORCH-666, ORCH-668
#[test]
fn trash_and_delete_read_the_mac_now() {
    // a trash call that ran but whose file is still there (restored, locked): not met
    let mut w = world();
    let a = w.file("old/a.txt", b"a");
    w.call(
        json!({"tool": "fs.trash", "kind": "trash", "target": {"path": a}}),
        ok(),
        None,
    );
    let (_, r) = w.check(json!({"kind": "trashed", "path": "~/old/a.txt"}));
    assert_eq!(verdict(&r), (Some(false), "not_met", "mac"), "{r:?}");

    // trashed by FERB's tool: gone, and where the receipt says it went
    let mut w = world();
    let a = format!("{}/old/a.txt", w.host.home);
    let to = w.file(".Trash/a 12-00-00.txt", b"a");
    w.call(
        json!({"tool": "fs.trash", "kind": "trash", "target": {"path": a}}),
        json!({"path": a, "trashed_to": to}),
        None,
    );
    let (_, r) = w.check(json!({"kind": "trashed", "path": "~/old/a.txt"}));
    assert_eq!(verdict(&r), (Some(true), "met", "mac"), "{r:?}");

    // the folder holding it was trashed: its place inside the trashed folder
    let mut w = world();
    let folder = format!("{}/old", w.host.home);
    let to = format!("{}/.Trash/old", w.host.home);
    w.file(".Trash/old/a.txt", b"a");
    w.call(
        json!({"tool": "fs.trash", "kind": "trash", "target": {"path": folder}}),
        json!({"path": folder, "trashed_to": to}),
        None,
    );
    let (_, r) = w.check(json!({"kind": "trashed", "path": "~/old/a.txt"}));
    assert_eq!(verdict(&r), (Some(true), "met", "mac"), "{r:?}");

    // moved by a shell command or Finder: found in the Trash by name
    let mut w = world();
    w.file(".Trash/a.txt", b"a");
    w.call(
        json!({"tool": "shell.run", "kind": "run_shell", "target": {}}),
        ok(),
        None,
    );
    let (_, r) = w.check(json!({"kind": "trashed", "path": "~/old/a.txt"}));
    assert_eq!(verdict(&r), (Some(true), "met", "mac"), "{r:?}");

    // asked to the Trash, deleted for good
    let mut w = world();
    let a = format!("{}/old/a.txt", w.host.home);
    w.call(
        json!({"tool": "fs.delete", "kind": "delete_permanent", "target": {"path": a}}),
        ok(),
        None,
    );
    let (_, r) = w.check(json!({"kind": "trashed", "path": "~/old/a.txt"}));
    assert_eq!(verdict(&r), (Some(false), "different", "receipt"), "{r:?}");
    assert_eq!(r.acted_on, "deleted for good");

    // asked deleted for good, moved to the Trash
    let mut w = world();
    let a = format!("{}/old/a.txt", w.host.home);
    let to = w.file(".Trash/a.txt", b"a");
    w.call(
        json!({"tool": "fs.trash", "kind": "trash", "target": {"path": a}}),
        json!({"path": a, "trashed_to": to}),
        None,
    );
    let (_, r) = w.check(json!({"kind": "deleted", "path": "~/old/a.txt"}));
    assert_eq!(verdict(&r), (Some(false), "different", "receipt"), "{r:?}");

    // deleted for good as asked
    let mut w = world();
    let a = format!("{}/old/a.txt", w.host.home);
    w.call(
        json!({"tool": "fs.delete", "kind": "delete_permanent", "target": {"path": a}}),
        ok(),
        None,
    );
    let (_, r) = w.check(json!({"kind": "deleted", "path": "~/old/a.txt"}));
    assert_eq!(verdict(&r), (Some(true), "met", "mac"), "{r:?}");

    // the wrong file trashed: the named one is still there
    let mut w = world();
    w.file("old/a.txt", b"a");
    let b = format!("{}/old/b.txt", w.host.home);
    w.call(
        json!({"tool": "fs.trash", "kind": "trash", "target": {"path": b}}),
        ok(),
        None,
    );
    let (_, r) = w.check(json!({"kind": "trashed", "path": "~/old/a.txt"}));
    assert_eq!(verdict(&r), (Some(false), "different", "receipt"), "{r:?}");
    assert_eq!(r.acted_on, b);

    // a path that isn't there and that nothing shows was ever there (a mistyped path) is never "done"
    let w = world();
    let (_, r) = w.check(json!({"kind": "deleted", "path": "~/never.txt"}));
    assert_eq!(verdict(&r), (None, "cant_tell", "mac"), "{r:?}");
}

/// Downloads by where they landed and their bytes: a guessed name is a fact, never a failure.
// decision: ORCH-666
#[test]
fn a_download_is_found_by_its_bytes() {
    // landed as "report (1).pdf": the named path isn't there, the fact names where it landed
    let w = world();
    let landed = w.file("Downloads/report (1).pdf", b"pdf");
    w.download(&landed, &sha(b"pdf"));
    let (_, r) = w.check(json!({"kind": "downloaded", "path": "~/Downloads/report.pdf"}));
    assert_eq!(verdict(&r), (None, "cant_tell", "mac"), "{r:?}");
    assert!(r.evidence.contains(&landed), "{r:?}");

    // moved by the worker after the download (HAND-237): the named file has the download's bytes
    let w = world();
    w.download(
        &format!("{}/workspace/downloads/dl-1-report.pdf", w.host.home),
        &sha(b"pdf"),
    );
    w.file("Documents/report.pdf", b"pdf");
    let (_, r) = w.check(json!({"kind": "downloaded", "path": "~/Documents/report.pdf"}));
    assert_eq!(verdict(&r), (Some(true), "met", "mac"), "{r:?}");

    // nothing downloaded and nothing that could fetch a file ran: the file isn't there, not met over any check
    let w = world();
    let (_, r) = w.check(json!({"kind": "downloaded", "path": "~/Downloads/invoice.pdf"}));
    assert_eq!(verdict(&r), (Some(false), "not_met", "mac"), "{r:?}");
    // the same with a shell command that could have fetched it: a fact for the check
    let mut w = world();
    w.call(
        json!({"tool": "shell.run", "kind": "run_shell", "target": {}}),
        ok(),
        None,
    );
    let (_, r) = w.check(json!({"kind": "downloaded", "path": "~/Downloads/invoice.pdf"}));
    assert_eq!(verdict(&r), (None, "cant_tell", "mac"), "{r:?}");

    // there already, not downloaded in this task: a fact for the check (it may be "already done")
    let w = world();
    w.file("Documents/report.pdf", b"old");
    let (_, r) = w.check(json!({"kind": "downloaded", "path": "~/Documents/report.pdf"}));
    assert_eq!(verdict(&r), (None, "cant_tell", "mac"), "{r:?}");
}

/// The ledger's unknown outcomes of a step, never retried without the owner: what the H line names.
// decision: ORCH-666, ORCH-672
#[test]
fn unknown_outcomes_come_from_the_ledger() {
    let w = world();
    for (id, state, retried) in [
        ("l1", "unknown", None),
        ("l2", "done", None),
        ("l3", "unknown", Some("l4")),
    ] {
        w.host
            .db
            .conn()
            .execute(
                "INSERT INTO action_ledger (id, task_id, capability, target_json, payload_hash, state, created_at, \
                 updated_at, retried_by) VALUES (?, ?, 'browser.act', ?, 'h', ?, '2026-01-01T09:00:00+00:00', \
                 '2026-01-01T09:00:00+00:00', ?)",
                rusqlite::params![id, TASK, json!({"site": format!("{id}.example")}).to_string(), state, retried],
            )
            .unwrap();
    }
    assert_eq!(
        verify::unknown_outcomes(&w.host.db, "t1").unwrap(),
        vec!["l1.example".to_string()]
    );
}
