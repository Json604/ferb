//! `ferb/daemon/reports.py` against `rust/fixtures/daemon-b/reports.json` (made by `reports.py` beside it): every
//! build on a store holding the recorded rows (the report's text byte for byte, the file's mode, the folder after a
//! purge), the Send's preparation, the requests a Send makes to a fake relay (and to a real one on 127.0.0.1), and
//! the pure rules. Nothing leaves this Mac.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ferb_daemon::areas_b::reports::{
    self, BoxFuture, Env, Outgoing, PostError, RelayHttp, ReportError, ReqwestRelay, Versions,
};
use ferb_sanitize::redact::Redactor;
use ferb_store::Db;
use serde_json::{json, Map, Value};

fn fixture() -> &'static Value {
    static F: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    F.get_or_init(|| {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/daemon-b/reports.json");
        let path = if path.exists() {
            path
        } else {
            PathBuf::from("/Users/kartikey/Desktop/FERB/rust/fixtures/daemon-b/reports.json")
        };
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
    })
}

fn dumps(v: &Value) -> String {
    ferb_py::json::dumps(v)
}

fn versions() -> Versions {
    Versions {
        ferb: fixture()["version"].as_str().unwrap().into(),
        macos: fixture()["macos"].as_str().unwrap().into(),
    }
}

fn redactor(sealed: &Value) -> Redactor {
    let r = Redactor::new(&[], &[]);
    for v in sealed.as_array().into_iter().flatten() {
        r.add_secret_value(v.as_str().unwrap());
    }
    r
}

fn now(text: &str) -> ferb_core::when::DateTime {
    ferb_core::when::parse(text).unwrap()
}

/// Written paths as the fixture writes them: the Rust test's folder for the recorder's.
fn fx(v: &Value, ours: &str, theirs: &str) -> Value {
    serde_json::from_str(
        &dumps(v).replace(&dumps(&json!(ours)).trim_matches('"').to_owned(), theirs),
    )
    .unwrap()
}

struct Diffs(Vec<String>, usize);

impl Diffs {
    fn check(&mut self, what: &str, python: &Value, rust: &Value) {
        if dumps(python) == dumps(rust) {
            self.1 += 1;
        } else {
            self.0.push(format!(
                "{what}:\n  python: {}\n  rust:   {}",
                dumps(python),
                dumps(rust)
            ));
        }
    }
    fn done(self, what: &str) {
        assert!(
            self.0.is_empty(),
            "{} difference(s):\n{}",
            self.0.len(),
            self.0.join("\n")
        );
        println!("{what}: {} compared, 0 differ", self.1);
    }
}

fn seed(db: &Db, seed: &Value) {
    for (table, t) in seed.as_object().unwrap() {
        let cols: Vec<&str> = t["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        let sql = format!(
            "INSERT INTO {table} ({}) VALUES ({})",
            cols.join(", "),
            vec!["?"; cols.len()].join(", ")
        );
        for row in t["rows"].as_array().unwrap() {
            let vals: Vec<rusqlite::types::Value> = row
                .as_array()
                .unwrap()
                .iter()
                .map(|v| match v {
                    Value::Null => rusqlite::types::Value::Null,
                    Value::String(s) => rusqlite::types::Value::Text(s.clone()),
                    Value::Number(n) if n.is_i64() => {
                        rusqlite::types::Value::Integer(n.as_i64().unwrap())
                    }
                    Value::Number(n) => rusqlite::types::Value::Real(n.as_f64().unwrap()),
                    other => panic!("cell {other}"),
                })
                .collect();
            db.conn()
                .execute(&sql, rusqlite::params_from_iter(vals))
                .unwrap();
        }
    }
}

fn listing(home: &Path) -> Value {
    use std::os::unix::fs::PermissionsExt;
    let folder = home.join("reports");
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(&folder) {
        let mut names: Vec<_> = rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for n in names {
            let m = std::fs::symlink_metadata(folder.join(&n)).unwrap();
            out.push(json!([
                n,
                format!("0o{:o}", m.permissions().mode() & 0o7777)
            ]));
        }
    }
    Value::Array(out)
}

fn failure_value(f: &reports::Failure) -> Value {
    json!({"task_id": f.task_id, "status": f.status, "reason": f.reason, "request": f.request,
           "answer": f.answer, "started": f.started, "ended": f.ended, "task_status": f.task_status})
}

fn error_value(e: &ReportError) -> Value {
    match e {
        ReportError::Refused(t) => json!({"error": t}),
        ReportError::Unsent(t) => json!({"unsent": t}),
        ReportError::Fault(t) => json!({"fault": t}),
    }
}

// decision: DMN-8, DMN-9, DMN-28, DMN-503, DMN-505
#[test]
fn every_build_matches_python() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture();
    let tmp = tempfile::tempdir().unwrap();
    let mut d = Diffs(vec![], 0);
    for (i, b) in f["builds"].as_array().unwrap().iter().enumerate() {
        let name = b["name"].as_str().unwrap();
        let home = tmp.path().join(format!("{i:02}")).join("home");
        std::fs::create_dir_all(&home).unwrap();
        let theirs = b["built"]["path"]
            .as_str()
            .map(|p| p[..p.find("/reports/").unwrap()].to_owned())
            .unwrap_or_else(|| "/FX/??/home".into());
        let ours = home.to_string_lossy().into_owned();
        let db = Db::open(home.join("ferb.db")).unwrap();
        db.migrate().unwrap();
        seed(&db, &b["seed"]);
        let at = now(b["now"].as_str().unwrap());
        for file in b["files"].as_array().unwrap() {
            let folder = home.join("reports");
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
            let p = folder.join(file[0].as_str().unwrap());
            std::fs::write(&p, "x").unwrap();
            let secs = at.timestamp() as f64 - file[1].as_f64().unwrap() * 86400.0;
            let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs_f64(secs);
            std::fs::File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(when)
                .unwrap();
        }
        let r = redactor(&b["sealed"]);
        let walk = |v: &Value| r.value(v);
        let text = |s: &str| r.text(s);
        let call = &b["call"];
        let v = versions();
        let env = Env {
            home: &home,
            now: at,
            walk: &walk,
            text: &text,
            app_version: call["app_version"].as_str().unwrap(),
            versions: &v,
        };
        let got = if call["fn"] == "report" {
            reports::report(
                &home.join("ferb.db"),
                call["task_id"].as_str().unwrap(),
                call["request_id"].as_str().unwrap(),
                &call["facts"],
                &env,
            )
            .map(|(f, b)| (Some(f), b))
        } else {
            reports::diagnostic(
                &home.join("ferb.db"),
                &call["health"],
                &call["recording"],
                &call["facts"],
                &env,
            )
            .map(|b| (None, b))
        };
        let mut out = Map::new();
        match got {
            Ok((failure, built)) => {
                if let Some(f) = failure {
                    out.insert("failure".into(), failure_value(&f));
                }
                out.insert("built".into(), fx(&built.to_value(), &ours, &theirs));
                out.insert(
                    "text".into(),
                    json!(std::fs::read_to_string(&built.path).unwrap()),
                );
                let mode = std::fs::metadata(&built.path).unwrap().permissions().mode() & 0o7777;
                out.insert("mode".into(), json!(format!("0o{mode:o}")));
            }
            Err(e) => {
                out.insert("error".into(), error_value(&e)["error"].clone());
            }
        }
        out.insert("folder".into(), listing(&home));
        let mut want = Map::new();
        for k in ["failure", "built", "text", "mode", "error", "folder"] {
            if let Some(v) = b.get(k) {
                want.insert(k.into(), v.clone());
            }
        }
        d.check(name, &Value::Object(want), &Value::Object(out));
        if name == "failed_task" {
            for case in f["problems"].as_array().unwrap() {
                let rows = reports::problems(&db, case[0].as_str(), case[1].as_str()).unwrap();
                let rows: Vec<Value> = rows.into_iter().map(|(a, k, v)| json!([a, k, v])).collect();
                d.check(
                    &format!("problems {} {}", case[0], case[1]),
                    &case[2],
                    &Value::Array(rows),
                );
            }
        }
    }
    d.done("builds and problems");
}

// decision: DMN-9, DMN-28, DMN-29
#[test]
fn every_prepare_matches_python() {
    let f = fixture();
    let tmp = tempfile::tempdir().unwrap();
    let mut d = Diffs(vec![], 0);
    for p in f["prepare"].as_array().unwrap() {
        let theirs = p["path"].as_str().unwrap();
        let fname = &theirs[theirs.rfind('/').unwrap() + 1..];
        let folder = tmp.path().join(p["name"].as_str().unwrap()).join("reports");
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join(fname);
        let written = p["written"].as_str().unwrap();
        std::fs::write(&path, written).unwrap();
        let r = redactor(&p["sealed"]);
        let text = |s: &str| r.text(s);
        let call = &p["call"];
        let got = reports::prepare(
            &path,
            call["kind"].as_str().unwrap(),
            call["title"].as_str().unwrap(),
            call["app_version"].as_str().unwrap(),
            &text,
            &versions(),
            call["limit"].as_u64().unwrap() as usize,
        );
        let result = match got {
            Ok(o) => json!({"kind": o.kind, "report_id": o.report_id,
                            "file": String::from_utf8(o.file).unwrap(), "issue": o.issue,
                            "shortened": o.shortened}),
            Err(e) => error_value(&e),
        };
        d.check(&format!("prepare {}", p["name"]), &p["result"], &result);
        let after = std::fs::read_to_string(&path).unwrap();
        let after = if after == written {
            "(unchanged)".to_owned()
        } else {
            after
        };
        d.check(
            &format!("prepare {} file", p["name"]),
            &p["after"],
            &json!(after),
        );
    }
    d.done("prepare");
}

/// The relay as the recorder's server answered, and what each request carried.
struct FakeRelay {
    answers: Mutex<Vec<Value>>,
    timeout: bool,
    refused: bool,
    seen: Mutex<Vec<Value>>,
}

impl RelayHttp for FakeRelay {
    fn post<'a>(
        &'a self,
        url: &'a str,
        body: &'a [u8],
        headers: &'a [(&'a str, String)],
    ) -> BoxFuture<'a, Result<(u16, Vec<u8>), PostError>> {
        Box::pin(async move {
            if self.refused {
                return Err(PostError::Failed("ConnectError".into()));
            }
            let path = &url[url.find("://").unwrap() + 3..];
            let path = &path[path.find('/').unwrap()..];
            let h = |k: &str| {
                headers
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.clone())
            };
            self.seen
                .lock()
                .unwrap()
                .push(json!({"path": path, "content_type": h("Content-Type"),
                "user_agent": h("User-Agent"), "content_length": body.len().to_string(),
                "body": String::from_utf8(body.to_vec()).unwrap()}));
            if self.timeout {
                return Err(PostError::Timeout);
            }
            let mut q = self.answers.lock().unwrap();
            let (status, answer) = if q.is_empty() {
                (200, json!({"ok": true, "ticket": "t-x"}))
            } else {
                let a = q.remove(0);
                (a[0].as_u64().unwrap() as u16, a[1].clone())
            };
            let bytes = match answer {
                Value::String(s) => s.into_bytes(),
                other => dumps(&other).into_bytes(),
            };
            Ok((status, bytes))
        })
    }
}

fn outgoing(o: &Value) -> Outgoing {
    Outgoing {
        kind: o["kind"].as_str().unwrap().into(),
        report_id: o["report_id"].as_str().unwrap().into(),
        file: o["file"].as_str().unwrap().as_bytes().to_vec(),
        issue: o["issue"].as_object().unwrap().clone(),
        shortened: o["shortened"].as_bool().unwrap(),
    }
}

// decision: DMN-8, DMN-28, DMN-30, DMN-31
#[tokio::test]
async fn every_send_makes_pythons_requests() {
    let s = &fixture()["send"][0];
    let out = outgoing(&s["outgoing"]);
    let version = s["version"].as_str().unwrap();
    let mut d = Diffs(vec![], 0);
    for c in s["cases"].as_array().unwrap() {
        let relay = FakeRelay {
            answers: Mutex::new(c["answers"].as_array().unwrap().clone()),
            timeout: c["timeout"].as_bool().unwrap(),
            refused: c.get("connect_error").is_some(),
            seen: Mutex::new(vec![]),
        };
        let got = reports::send("http://127.0.0.1:9", &out, &relay, version, 540.0, 0.0).await;
        let result = match got {
            Ok(id) => json!({"report_id": id}),
            Err(e) => error_value(&e),
        };
        let name = c["name"].as_str().unwrap();
        d.check(&format!("send {name}"), &c["result"], &result);
        d.check(
            &format!("send {name} requests"),
            &c["requests"],
            &Value::Array(relay.seen.into_inner().unwrap()),
        );
    }
    d.done("send");
}

#[tokio::test]
async fn a_send_past_its_deadline_is_unsent() {
    struct Never;
    impl RelayHttp for Never {
        fn post<'a>(
            &'a self,
            _u: &'a str,
            _b: &'a [u8],
            _h: &'a [(&'a str, String)],
        ) -> BoxFuture<'a, Result<(u16, Vec<u8>), PostError>> {
            Box::pin(std::future::pending())
        }
    }
    let out = outgoing(&fixture()["send"][0]["outgoing"]);
    let got = reports::send("https://relay.example", &out, &Never, "0.27.3", 0.0, 0.0).await;
    assert_eq!(
        got,
        Err(ReportError::Unsent(
            "FERB couldn't send the report: it didn't finish in time".into()
        ))
    );
}

/// A relay on 127.0.0.1 answering each request from a queue; returns the raw requests (head and body).
async fn local_relay(answers: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<(String, Vec<u8>)>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(vec![]));
    let kept = seen.clone();
    tokio::spawn(async move {
        let mut answers = answers.into_iter();
        while let Ok((mut sock, _)) = listener.accept().await {
            let Some((status, body)) = answers.next() else {
                break;
            };
            let mut buf = vec![];
            let mut chunk = [0u8; 65536];
            let (head, len) = loop {
                let n = sock.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8(buf[..i].to_vec()).unwrap();
                    let len = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    buf.drain(..i + 4);
                    break (head, len);
                }
            };
            while buf.len() < len {
                let n = sock.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
            }
            kept.lock().unwrap().push((head, buf.clone()));
            let reply = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(reply.as_bytes()).await.unwrap();
            let _ = sock.shutdown().await;
        }
    });
    (url, seen)
}

// decision: DMN-28, DMN-30, DMN-31
#[tokio::test]
async fn the_real_client_sends_the_relays_bytes() {
    let s = &fixture()["send"][0];
    let out = outgoing(&s["outgoing"]);
    let case = s["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "conflicts_then_ok")
        .unwrap();
    let answers = case["answers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a[0].as_u64().unwrap() as u16, dumps(&a[1])))
        .collect();
    let (url, seen) = local_relay(answers).await;
    let relay = ReqwestRelay::new(10.0).unwrap();
    let got = reports::send(
        &url,
        &out,
        &relay,
        s["version"].as_str().unwrap(),
        30.0,
        0.0,
    )
    .await;
    assert_eq!(got, Ok("r3".to_owned()));
    let seen = seen.lock().unwrap().clone();
    let want = case["requests"].as_array().unwrap();
    assert_eq!(seen.len(), want.len());
    for ((head, body), w) in seen.iter().zip(want) {
        let first = head.lines().next().unwrap();
        assert_eq!(
            first,
            format!("POST {} HTTP/1.1", w["path"].as_str().unwrap())
        );
        let header = |k: &str| {
            head.lines().find_map(|l| {
                l.split_once(": ")
                    .filter(|(n, _)| n.eq_ignore_ascii_case(k))
                    .map(|(_, v)| v.to_owned())
            })
        };
        assert_eq!(
            header("content-type").as_deref(),
            w["content_type"].as_str()
        );
        assert_eq!(header("user-agent").as_deref(), w["user_agent"].as_str());
        assert_eq!(
            header("content-length").as_deref(),
            w["content_length"].as_str()
        );
        assert_eq!(
            String::from_utf8(body.clone()).unwrap(),
            w["body"].as_str().unwrap()
        );
    }
    // nothing listens on a closed port: ConnectError, as httpx names it
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let got = reports::send(
        &format!("http://127.0.0.1:{port}"),
        &out,
        &relay,
        "0.27.3",
        30.0,
        0.0,
    )
    .await;
    assert_eq!(
        got,
        Err(ReportError::Unsent(
            "FERB couldn't reach the report service (ConnectError)".into()
        ))
    );
}

// decision: DMN-31
#[test]
fn the_relay_address_rules_match_python() {
    let mut d = Diffs(vec![], 0);
    for case in fixture()["relay_url"].as_array().unwrap() {
        let got = match reports::relay_url(&case[0]) {
            Ok(u) => json!({"url": u}),
            Err(e) => error_value(&e),
        };
        d.check(&format!("relay_url {}", case[0]), &case[1], &got);
    }
    d.done("relay_url");
}

// decision: DMN-28
#[test]
fn send_takes_only_a_file_ferb_wrote_in_its_reports_folder() {
    let s = &fixture()["saved"];
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("saved");
    let ours = root.parent().unwrap().to_string_lossy().into_owned();
    let home = root.join("home");
    std::fs::create_dir_all(home.join("reports")).unwrap();
    std::fs::create_dir_all(root.join("elsewhere")).unwrap();
    let setup = &s["setup"];
    for (rel, text) in setup["files"].as_object().unwrap() {
        std::fs::write(home.join(rel), text.as_str().unwrap()).unwrap();
    }
    std::fs::write(home.join(setup["outside"].as_str().unwrap()), "not FERB's").unwrap();
    for (rel, to) in setup["links"].as_object().unwrap() {
        std::os::unix::fs::symlink(home.join(to.as_str().unwrap()), home.join(rel)).unwrap();
    }
    for rel in setup["dirs"].as_array().unwrap() {
        std::fs::create_dir(home.join(rel.as_str().unwrap())).unwrap();
    }
    let mut d = Diffs(vec![], 0);
    for case in s["cases"].as_array().unwrap() {
        let path = case[0].as_str().unwrap().replace("/FX", &ours);
        let got = match reports::saved(&home, &path) {
            Ok((p, kind)) => {
                json!({"path": p.to_string_lossy().replace(&ours, "/FX"), "kind": kind})
            }
            Err(e) => error_value(&e),
        };
        d.check(&format!("saved {}", case[0]), &case[1], &got);
    }
    d.done("saved");
}

// decision: DMN-9, DMN-29
#[test]
fn shortening_and_the_summary_match_python() {
    let f = fixture();
    let bodies = &f["bodies"];
    let mut d = Diffs(vec![], 0);
    for c in f["shorten"].as_array().unwrap() {
        let (text, note) = reports::shorten(
            bodies[c["body"].as_str().unwrap()].as_str().unwrap(),
            c["limit"].as_u64().unwrap() as usize,
            c["name"].as_str().unwrap(),
        );
        d.check(
            &format!("shorten {} {}", c["body"], c["limit"]),
            &json!([c["text"], c["note"]]),
            &json!([text, note]),
        );
    }
    for c in f["summarize"].as_array().unwrap() {
        let got = reports::summarize(
            bodies[c["body"].as_str().unwrap()].as_str().unwrap(),
            c["note"].as_str().unwrap(),
            c["limit"].as_u64().unwrap() as usize,
        );
        d.check(
            &format!("summarize {} {}", c["body"], c["limit"]),
            &c["summary"],
            &json!(got),
        );
    }
    let rt = &f["redacted_text"];
    let r = redactor(&rt["sealed"]);
    for c in rt["cases"].as_array().unwrap() {
        let pieces = Mutex::new(vec![]);
        let text = |s: &str| {
            pieces.lock().unwrap().push(s.to_owned());
            r.text(s)
        };
        let got = reports::redacted_text(
            rt["input"].as_str().unwrap(),
            &text,
            c["piece_chars"].as_u64().unwrap() as usize,
        );
        d.check(
            &format!("redacted_text {}", c["piece_chars"]),
            &json!([c["pieces"], c["result"]]),
            &json!([pieces.into_inner().unwrap(), got]),
        );
    }
    d.done("shorten, summarize, redacted_text");
}

/// tests/test_failure_report.py's 30 MB case without a 30 MB fixture: a generated report over 25 MB is shortened
/// to between 24 and 25 MB with its summary, its error and its last step kept.
// decision: DMN-9, DMN-29
#[test]
fn a_report_over_25_mb_is_cut_to_fit() {
    let word = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor ";
    let filler: String = word.repeat(130).chars().take(10_000).collect();
    let mut body = String::from(
        "FERB report for t\n\n## Summary\nStatus: failed\n\n## Request\nYour request:\nx\n\n## Steps\ntask t\n",
    );
    body.push_str(
        "09:00:00  tool.call  fs.list  [1.0s]\n    {\n      \"error\": \"denied: ~/Downloads is protected\"\n    }\n",
    );
    for i in 0..2_700 {
        body.push_str(&format!(
            "09:00:01  tool.call  browser.read  [0.1s]\n    {{\n      \"error\": null,\n      \"output\": \"{i} {filler}\"\n    }}\n"
        ));
    }
    body.push_str("09:00:02  tool.call  browser.read  [0.1s]\n    the last step\n");
    body.push_str("\n## FERB's recorded problems\nFERB's own records.\n(none recorded)\n");
    assert!(body.len() > 27_000_000);
    let (text, note) = reports::shorten(&body, reports::MAX_REPORT_BYTES, "FERB-report-x.txt");
    assert!(
        text.len() <= 25_000_000 && text.len() > 24_000_000,
        "{}",
        text.len()
    );
    assert!(text.starts_with(&note) && note.starts_with("[Shortened to fit 25 MB: "));
    for kept in [
        "## Summary",
        "denied: ~/Downloads is protected",
        "the last step",
        "## FERB's recorded problems",
        "[FERB cut ",
    ] {
        assert!(text.contains(kept), "{kept}");
    }
    let summary = reports::summarize(&text, &note, reports::SUMMARY_CHARS);
    assert!(summary.starts_with("[Shortened") && summary.contains("Errors (1)"));
}
