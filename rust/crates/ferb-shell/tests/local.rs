//! The local-peer checks against Python's decisions (rust/fixtures/shell/local.json): address normalization, kernel
//! report lines, LocalAccess on scripted socket readings with a fake clock (including the 30 s accept wait, without
//! waiting), direct_ports and local_listeners. Unknown identities fail closed.
// decision: HAND-226, HAND-227, HAND-433, HAND-214

mod common;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use common::{fixture, Tally};
use ferb_shell::local::{
    direct_ports, ip_norm, parse_report, report_tag, LocalAccess, SocketFacts, TcpSocket,
};
use ferb_shell::protection::{Entry, LocalPort, Protection};
use ferb_shell::sockets::Socket;
use serde_json::{json, Value};

struct Pins {
    pins: Vec<LocalPort>,
    home: PathBuf,
}

impl Protection for Pins {
    fn home(&self) -> &Path {
        &self.home
    }
    fn ferb_home(&self) -> &Path {
        &self.home
    }
    fn workspace(&self) -> &Path {
        &self.home
    }
    fn uploads(&self) -> &Path {
        &self.home
    }
    fn reads(&self) -> &[Entry] {
        &[]
    }
    fn writes(&self) -> &[Entry] {
        &[]
    }
    fn app_entries(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn service_files(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn guarded_parents(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn forms_now(&self, p: &Path) -> Vec<PathBuf> {
        vec![p.to_path_buf()]
    }
    fn app_data(&self, _: &Path) -> bool {
        false
    }
    fn local_ports(&self) -> &[LocalPort] {
        &self.pins
    }
    fn rules(&self) -> Vec<Value> {
        vec![]
    }
}

fn pins(v: &Value) -> Arc<Pins> {
    Arc::new(Pins {
        pins: v
            .as_array()
            .unwrap()
            .iter()
            .map(|p| LocalPort {
                port: p[0].as_u64().unwrap() as u16,
                executable: PathBuf::from(p[1].as_str().unwrap()),
            })
            .collect(),
        home: PathBuf::from("/nonexistent-ferb-home"),
    })
}

fn rows(v: &Value) -> Option<Vec<TcpSocket>> {
    let a = v.as_array()?;
    Some(
        a.iter()
            .map(|r| TcpSocket {
                local: (
                    r[0][0].as_str().unwrap().into(),
                    r[0][1].as_u64().unwrap() as u32,
                ),
                remote: r[1]
                    .as_array()
                    .map(|x| (x[0].as_str().unwrap().into(), x[1].as_u64().unwrap() as u32)),
                state: r[2].as_str().unwrap().into(),
                pid: r[3].as_i64().map(|p| p as i32),
                queued: r[4].as_i64().unwrap(),
            })
            .collect(),
    )
}

/// Scripted kernel facts: readings in order (the last repeats), executables and containment by pid, a fake clock.
struct Script {
    readings: Vec<Option<Vec<TcpSocket>>>,
    exes: HashMap<String, String>,
    contained: HashMap<String, bool>,
    reads: Mutex<usize>,
    clock: Mutex<f64>,
    sleeps: Mutex<Vec<f64>>,
}

impl SocketFacts for Script {
    fn tcp_snapshot(&self, _complete: bool) -> Option<Vec<TcpSocket>> {
        let mut n = self.reads.lock().unwrap();
        *n += 1;
        self.readings[(*n).min(self.readings.len()) - 1].clone()
    }
    fn executable_of(&self, pid: i32) -> Option<PathBuf> {
        self.exes.get(&pid.to_string()).map(PathBuf::from)
    }
    fn contained(&self, pid: i32, _: &Path, _: &Path) -> bool {
        self.contained
            .get(&pid.to_string())
            .copied()
            .unwrap_or(false)
    }
    fn monotonic(&self) -> f64 {
        *self.clock.lock().unwrap()
    }
    fn sleep(&self, s: f64) {
        self.sleeps.lock().unwrap().push(s);
        *self.clock.lock().unwrap() += s;
    }
}

fn script(case: &Value, readings: Vec<Option<Vec<TcpSocket>>>) -> Arc<Script> {
    let strmap = |v: &Value| {
        v.as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<HashMap<_, _>>()
    };
    Arc::new(Script {
        readings,
        exes: strmap(&case["executables"])
            .into_iter()
            .map(|(k, v)| (k, v.as_str().unwrap().to_string()))
            .collect(),
        contained: case
            .get("contained")
            .map(|c| {
                strmap(c)
                    .into_iter()
                    .map(|(k, v)| (k, v.as_bool().unwrap()))
                    .collect()
            })
            .unwrap_or_default(),
        reads: Mutex::new(0),
        clock: Mutex::new(1000.0),
        sleeps: Mutex::new(Vec::new()),
    })
}

#[test]
fn addresses_and_kernel_reports_read_as_python_reads_them() {
    let fx = fixture("local.json");
    let mut tally = Tally::default();
    for pair in fx["ip"].as_array().unwrap() {
        tally.check(
            pair[0].as_str().unwrap(),
            ip_norm(pair[0].as_str().unwrap()),
            pair[1].as_str().map(str::to_string),
        );
    }
    for pair in fx["reports"].as_array().unwrap() {
        let got = parse_report(pair[0].as_str().unwrap())
            .map(|(rep, prog, port, tag)| json!([rep.map(|r| r as i64), prog, port as i64, tag]));
        tally.check(
            &format!("{:?}", pair[0]),
            got.unwrap_or(Value::Null),
            pair[1].clone(),
        );
    }
    tally.check(
        "ACCEPT_WAIT",
        ferb_shell::local::ACCEPT_WAIT,
        fx["accept_wait"].as_f64().unwrap(),
    );
    tally.check(
        "SETTLE_READS",
        ferb_shell::local::SETTLE_READS as u64,
        fx["settle_reads"].as_u64().unwrap(),
    );
    tally.check(
        "STREAM",
        ferb_shell::local::stream_argv(),
        common::strs(&fx["stream"]),
    );
    tally.done("ip/reports");
}

#[test]
fn local_access_decides_as_python_on_every_scripted_reading() {
    let fx = fixture("local.json");
    let mut tally = Tally::default();
    for case in fx["access"].as_array().unwrap() {
        let readings: Vec<Option<Vec<TcpSocket>>> = case["readings"]
            .as_array()
            .unwrap()
            .iter()
            .map(rows)
            .collect();
        let facts = script(case, readings);
        let access = LocalAccess::new(pins(&case["pins"]), facts.clone());
        let t = &case["target"];
        let target = (t[0].as_str().unwrap(), t[1].as_u64().unwrap() as u32);
        let got = if case["op"] == "before" {
            match access.before_connect(target) {
                Ok(()) => json!({"ok": null}),
                Err(e) => json!({"refused": e.0}),
            }
        } else {
            let c = &case["client"];
            match access.connected(
                target,
                (c[0].as_str().unwrap(), c[1].as_u64().unwrap() as u32),
            ) {
                Ok(peer) => json!({"ok": [peer.address, peer.executable]}),
                Err(e) => json!({"refused": e.0}),
            }
        };
        let name = case["name"].as_str().unwrap();
        tally.check(name, got, case["result"].clone());
        tally.check(
            &format!("{name} reads"),
            *facts.reads.lock().unwrap() as u64,
            case["reads"].as_u64().unwrap(),
        );
        let sleeps: Vec<f64> = case["sleeps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_f64().unwrap())
            .collect();
        tally.check(
            &format!("{name} sleeps"),
            facts.sleeps.lock().unwrap().clone(),
            sleeps,
        );
    }
    tally.done("access");
}

#[test]
fn direct_ports_and_listeners_decide_as_python() {
    let fx = fixture("local.json");
    let mut tally = Tally::default();
    for case in fx["direct"].as_array().unwrap() {
        let facts = script(case, vec![rows(&case["rows"])]);
        let got: Vec<Value> = direct_ports(&*pins(&case["pins"]), &*facts)
            .iter()
            .map(|p| json!([p.port, p.executable.to_string_lossy()]))
            .collect();
        tally.check(
            case["name"].as_str().unwrap(),
            Value::from(got),
            case["allowed"].clone(),
        );
    }
    for case in fx["listeners"].as_array().unwrap() {
        let socks: Option<Vec<Socket>> = case["sockets"].as_array().map(|a| {
            a.iter()
                .map(|s| Socket {
                    proto: s[0].as_str().unwrap().into(),
                    local: (
                        s[1][0].as_str().unwrap().into(),
                        s[1][1].as_u64().unwrap() as u32,
                    ),
                    remote: s[2]
                        .as_array()
                        .map(|r| (r[0].as_str().unwrap().into(), r[1].as_u64().unwrap() as u32)),
                    state: s[3].as_str().unwrap().into(),
                    pid: s[4].as_i64().map(|p| p as i32),
                    queued: s[5].as_i64().unwrap(),
                })
                .collect()
        });
        let contained = case["contained"].clone();
        let got = ferb_shell::listeners::local_listeners(socks.as_deref(), |pid| {
            contained
                .get(pid.to_string())
                .and_then(Value::as_bool)
                .unwrap_or(false)
        });
        let got = got.map(|(o, i)| {
            json!([
                o.into_iter().collect::<Vec<_>>(),
                i.into_iter().collect::<Vec<_>>()
            ])
        });
        tally.check(
            case["name"].as_str().unwrap(),
            got.unwrap_or(Value::Null),
            case["result"].clone(),
        );
    }
    tally.done("direct/listeners");
}

#[test]
fn each_command_gets_its_own_unguessable_report_tag() {
    let (a, b) = (report_tag("p-1234abcd"), report_tag("p-1234abcd"));
    assert_ne!(a, b);
    let bits = a.strip_prefix("ferb p-1234abcd ").unwrap();
    assert!(
        bits.len() == 32
            && bits
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
}

// ---------- the kernel's reports (DirectReports), with a stand-in for `log stream` ----------

fn kernel_line(message: &str, pid: i64) -> String {
    json!({"processID": pid, "processImagePath": "/kernel", "eventMessage": message}).to_string()
}

fn allow(program: &str, port: u32, tag: &str, repeats: u32) -> String {
    let head = if repeats > 0 {
        format!("{repeats} duplicate reports for ")
    } else {
        String::new()
    };
    kernel_line(
        &format!("{head}Sandbox: {program}(4242) allow network-outbound remote:*:{port}\n{tag}"),
        0,
    )
}

/// A stand-in `log stream`: a banner, the given lines, then it keeps listening (or `then`).
fn fake_stream(dir: &Path, lines: &[String], then: &str) -> Vec<String> {
    let file = dir.join(format!("lines-{}", ferb_shell::token_hex(4)));
    let mut body = String::from("Filtering the log data using \"...\"\n");
    for l in lines {
        body.push_str(l);
        body.push('\n');
    }
    std::fs::write(&file, body).unwrap();
    vec![
        "/bin/sh".into(),
        "-c".into(),
        format!("cat '{}'; {then}", file.display()),
    ]
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !f() {
        assert!(std::time::Instant::now() < deadline, "{what}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

const TAG_A: &str = "ferb p-aaaa1111 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TAG_B: &str = "ferb p-bbbb2222 bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[test]
fn kernel_reports_count_each_programs_direct_connections_for_its_command_only() {
    let dir = tempfile::tempdir().unwrap();
    let lines = vec![
        allow("psql", 8765, TAG_A, 0),
        allow("psql", 8765, TAG_A, 3),
        allow("redis-cli", 8765, TAG_A, 0),
        allow("psql", 8765, TAG_B, 0),
        kernel_line(&format!("Sandbox: curl(4245) deny(1) network-outbound remote:*:8765\n{TAG_A}"), 0),
        kernel_line(&format!("Sandbox: psql(4246) allow network-outbound remote:*:8765\n{TAG_A}"), 4246),
        "not json".into(),
        kernel_line(&format!("Sandbox: python3(1) deny(1) file-write-create /x(1) allow network-outbound remote:*:5432\n{TAG_A}"), 0),
        allow("psql", 5432, "ferb p-cccc3333 cccccccccccccccccccccccccccccccc", 0),
    ];
    let started = ferb_shell::wall_time() + 1.0;
    let reports = ferb_shell::local::DirectReports::new(
        Some(fake_stream(dir.path(), &lines, "exec sleep 60")),
        None,
    );
    reports.expect("p-aaaa1111", TAG_A);
    reports.expect("p-bbbb2222", TAG_B);
    assert!(reports.start());
    wait_for("the kernel's reports were never counted", || {
        reports
            .connections("p-aaaa1111", started)
            .and_then(|m| m.get(&8765).cloned())
            .is_some_and(|m| m["reported_connections"] == json!(5))
    });
    let a = reports.connections("p-aaaa1111", started).unwrap();
    assert_eq!(
        Value::Object(a[&8765].clone()),
        json!({"reported_connections": 5, "clients": ["psql", "redis-cli"]})
    );
    let b = reports.connections("p-bbbb2222", started).unwrap();
    assert_eq!(
        Value::Object(b[&8765].clone()),
        json!({"reported_connections": 1, "clients": ["psql"]})
    );
    assert!(reports.connections("p-aaaa1111", 0.0).is_none()); // it started before the stream listened
    assert_eq!(
        reports.kept(),
        BTreeSet::from(["p-aaaa1111".to_string(), "p-bbbb2222".to_string()])
    );
    reports.forget("p-aaaa1111");
    assert!(reports
        .connections("p-aaaa1111", started)
        .unwrap()
        .is_empty());
    reports.close();
    assert!(reports.connections("p-bbbb2222", started).is_none()); // a stopped stream: unknown from now on
}

#[test]
fn a_stream_that_ends_is_reported_and_closing_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let ended: Arc<Mutex<Vec<String>>> = Arc::default();
    let e = ended.clone();
    let dying = ferb_shell::local::DirectReports::new(
        Some(fake_stream(dir.path(), &[], "exit 3")),
        Some(Arc::new(move |why: &str| {
            e.lock().unwrap().push(why.to_string())
        })),
    );
    assert!(dying.start());
    wait_for("the stream's end was never reported", || {
        !ended.lock().unwrap().is_empty()
    });
    assert_eq!(
        ended.lock().unwrap()[0],
        "the system log stream ended (exit 3)"
    );
    assert!(!dying.running());
    let closing: Arc<Mutex<Vec<String>>> = Arc::default();
    let c = closing.clone();
    let healthy = ferb_shell::local::DirectReports::new(
        Some(fake_stream(dir.path(), &[], "exec sleep 60")),
        Some(Arc::new(move |why: &str| {
            c.lock().unwrap().push(why.to_string())
        })),
    );
    assert!(healthy.start());
    wait_for("the stream never started listening", || healthy.running());
    healthy.close();
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(closing.lock().unwrap().is_empty() && healthy.ended().is_none());
}

#[test]
fn the_stream_stops_when_its_input_closes() {
    // what a daemon's death does to the pipe: the watcher kills the stream (no Python in between, ENG-2)
    let dir = tempfile::tempdir().unwrap();
    let ended: Arc<Mutex<Vec<String>>> = Arc::default();
    let e = ended.clone();
    let pidfile = dir.path().join("pid");
    let argv = vec![
        "/bin/sh".into(),
        "-c".into(),
        format!(
            "echo $$ > '{}'; echo banner; exec sleep 60",
            pidfile.display()
        ),
    ];
    let reports = ferb_shell::local::DirectReports::new(
        Some(argv),
        Some(Arc::new(move |w: &str| e.lock().unwrap().push(w.into()))),
    );
    assert!(reports.start());
    wait_for("the stand-in stream never started", || reports.running());
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    reports.close(); // closes its stdin, as a daemon going away does
    wait_for("the stream outlived its input", || {
        // SAFETY: probes only the stand-in this test started.
        unsafe { libc::kill(pid, 0) != 0 }
    });
}
