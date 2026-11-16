//! The `ferb` program's dispatch (exit statuses, where errors go, which setup flags reach the installer) and the
//! socket client's frames, against a fake host and a fake daemon on a temporary socket.
// decision: DMN-3, DMN-537, DMN-24

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;

use ferb_device::cli::client::Client;
use ferb_device::cli::run::{run, Host};
use ferb_device::cli::setup::Env;
use ferb_device::cli::{CliError, Rpc};
use serde_json::{json, Value};

#[derive(Default)]
struct FakeHost {
    out: Vec<u8>,
    err: Vec<u8>,
    sessions: usize,
    salvage_fails: bool,
    envs: Vec<bool>,
    daemon: Option<Vec<(String, Value)>>,
    rpc: Option<Box<dyn Rpc>>,
    extensions: Vec<(String, Option<String>)>,
    stats: Vec<i64>,
}

struct Canned(Vec<(String, Value)>);

impl Rpc for Canned {
    fn call(&mut self, method: &str, _params: Option<Value>) -> Result<Value, CliError> {
        let (m, v) = self.0.remove(0);
        assert_eq!(m, method);
        Ok(v)
    }
}

impl Host for FakeHost {
    fn version(&self) -> String {
        "0.28.0".into()
    }
    fn out(&mut self) -> &mut dyn Write {
        &mut self.out
    }
    fn err(&mut self) -> &mut dyn Write {
        &mut self.err
    }
    fn own_session(&mut self) {
        self.sessions += 1;
    }
    fn salvage(&mut self) -> Result<Vec<String>, CliError> {
        if self.salvage_fails {
            return Err(CliError::ferb(
                "FERB's database is from a newer FERB (schema 63); install that FERB again",
            ));
        }
        Ok(vec!["A damaged ferb.db was set aside.".into()])
    }
    fn env(&mut self, every_browser: bool) -> Result<Env<'_>, CliError> {
        self.envs.push(every_browser);
        Err(CliError::Io("[Errno 13] Permission denied: '/x'".into()))
    }
    fn connect(&mut self) -> Result<Box<dyn Rpc>, CliError> {
        if let Some(rpc) = self.rpc.take() {
            return Ok(rpc);
        }
        match self.daemon.take() {
            Some(answers) => Ok(Box::new(Canned(answers))),
            None => Err(CliError::NotRunning(
                "FERB daemon is not running (start it with `ferb daemon run`)".into(),
            )),
        }
    }
    fn ask(&mut self, _prompt: &str) -> String {
        String::new()
    }
    fn sleep(&mut self, _seconds: f64) {}
    fn copy(&mut self, _text: &str) -> bool {
        true
    }
    fn step_line(&self) -> Option<Box<dyn Fn(&Value) -> String>> {
        None
    }
    fn extension(&mut self, action: &str, out_dir: Option<&str>) -> i32 {
        self.extensions
            .push((action.into(), out_dir.map(String::from)));
        0
    }
    fn stats(&mut self, days: i64) -> i32 {
        self.stats.push(days);
        0
    }
}

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn text(b: &[u8]) -> String {
    String::from_utf8(b.to_vec()).unwrap()
}

#[test]
fn exit_statuses_and_error_lines_are_pythons() {
    let mut h = FakeHost::default();
    assert_eq!(run(&argv(&["--version"]), &mut h), 0);
    assert_eq!(text(&h.out), "ferb 0.28.0\n");

    let mut h = FakeHost::default();
    assert_eq!(run(&argv(&["setup", "--bogus"]), &mut h), 2);
    assert!(text(&h.err).starts_with("ferb: error: "));

    // setup: the salvage lines first; the env's failure is one plain line, exit 1; own session for --service only
    let mut h = FakeHost::default();
    assert_eq!(run(&argv(&["setup", "--service", "--update"]), &mut h), 1);
    assert_eq!(text(&h.out), "A damaged ferb.db was set aside.\n");
    assert_eq!(text(&h.err), "ferb: [Errno 13] Permission denied: '/x'\n");
    assert_eq!((h.sessions, h.envs.clone()), (1, vec![false]));
    let mut h = FakeHost::default();
    run(&argv(&["setup", "--cleanup"]), &mut h);
    run(&argv(&["setup", "--cli"]), &mut h);
    run(&argv(&["uninstall", "--purge", "--yes"]), &mut h);
    run(&argv(&["doctor"]), &mut h);
    assert_eq!(
        (h.sessions, h.envs.clone()),
        (1, vec![false, false, true, false])
    );

    let mut h = FakeHost {
        salvage_fails: true,
        ..FakeHost::default()
    };
    assert_eq!(run(&argv(&["setup"]), &mut h), 1);
    assert_eq!(
        text(&h.err),
        "ferb: FERB's database is from a newer FERB (schema 63); install that FERB again\n"
    );
    assert!(h.envs.is_empty());

    // the toolbox: no daemon is exit 2
    let mut h = FakeHost::default();
    assert_eq!(run(&argv(&["ps"]), &mut h), 2);
    assert_eq!(
        text(&h.err),
        "ferb: FERB daemon is not running (start it with `ferb daemon run`)\n"
    );
    let mut h = FakeHost {
        daemon: Some(vec![("ps.list".into(), json!({"processes": []}))]),
        ..FakeHost::default()
    };
    assert_eq!(run(&argv(&["ps"]), &mut h), 0);
    assert_eq!(text(&h.out), "No processes.\n");

    // parts not ported yet say so; the agent's own commands name the program that runs them
    for cmd in [vec!["runtimes"], vec!["memory", "dashboard"]] {
        let mut h = FakeHost::default();
        assert_eq!(run(&argv(&cmd), &mut h), 1, "{cmd:?}");
        assert!(text(&h.err).contains("isn't part of the Rust program yet"));
    }
    for (cmd, there) in [
        (vec!["mcp"], "ferbd mcp"),
        (vec!["daemon", "run"], "ferbd daemon"),
    ] {
        let mut h = FakeHost::default();
        assert_eq!(run(&argv(&cmd), &mut h), 1, "{cmd:?}");
        assert!(text(&h.err).contains(there), "{}", text(&h.err));
    }

    // `browsers extension`: the host's, with its action (default `path`) and --out
    let mut h = FakeHost::default();
    assert_eq!(run(&argv(&["browsers", "extension"]), &mut h), 0);
    assert_eq!(
        run(
            &argv(&["browsers", "extension", "package", "--out", "/tmp/x"]),
            &mut h
        ),
        0
    );
    assert_eq!(run(&argv(&["browsers", "extension", "install"]), &mut h), 0);
    assert_eq!(
        h.extensions,
        vec![
            ("path".to_string(), None),
            ("package".to_string(), Some("/tmp/x".to_string())),
            ("install".to_string(), None)
        ]
    );
    assert_eq!(h.envs, Vec::<bool>::new()); // no setup env, no daemon

    // `stats`: the host's (the database, no daemon), --days defaulting to 7
    let mut h = FakeHost::default();
    assert_eq!(run(&argv(&["stats"]), &mut h), 0);
    assert_eq!(run(&argv(&["stats", "--days", "30"]), &mut h), 0);
    assert_eq!(h.stats, vec![7, 30]);
    assert_eq!(run(&argv(&["stats", "--days", "x"]), &mut h), 2);
}

fn serve(path: PathBuf, script: Vec<Value>) -> std::thread::JoinHandle<Vec<String>> {
    let listener = UnixListener::bind(&path).unwrap();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        let mut seen = Vec::new();
        for reply in script {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            seen.push(line.clone());
            for frame in reply.as_array().unwrap() {
                writer.write_all(format!("{frame}\n").as_bytes()).unwrap();
            }
        }
        seen
    })
}

#[test]
fn the_client_speaks_the_daemons_frames() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("ferb.sock");
    match Client::open(&sock) {
        Err(CliError::NotRunning(m)) => assert_eq!(
            m,
            "FERB daemon is not running (start it with `ferb daemon run`)"
        ),
        _ => panic!("no daemon must be NotRunning"),
    }
    let server = serve(
        sock.clone(),
        vec![
            json!([{"jsonrpc": "2.0", "id": 1, "result": {"role": "cli"}}]),
            json!([{"jsonrpc": "2.0", "id": 99, "result": {}}, {"jsonrpc": "2.0", "id": 2, "result": {"pid": 7}}]),
            json!([{"jsonrpc": "2.0", "id": 3, "error": {"code": -32603, "message": "KeyError"}}]),
            json!([{"jsonrpc": "2.0", "method": "logs.subscribe.event", "params": {"topic": "a"}},
                   {"jsonrpc": "2.0", "method": "other.event", "params": {}},
                   {"jsonrpc": "2.0", "method": "logs.subscribe.event", "params": {"topic": "b"}},
                   {"jsonrpc": "2.0", "id": 4, "result": {"ended": true}}]),
        ],
    );
    let mut c = Client::connect(&sock, "cli", None, 10.0).unwrap();
    assert_eq!(c.call("ping", None).unwrap(), json!({"pid": 7}));
    match c.call("tasks.list", Some(json!({"q": "é"}))) {
        Err(e @ CliError::Rpc { code: -32603, .. }) => {
            assert_eq!(
                ferb_device::cli::plain_error(&e),
                "ferb: that failed inside FERB (KeyError); `ferb stats` lists it"
            )
        }
        other => panic!("{other:?}"),
    }
    let mut topics = Vec::new();
    c.stream(
        "logs.subscribe",
        Some(json!({"filters": ["tasks"]})),
        &mut |ev| {
            topics.push(ev["topic"].clone());
            true
        },
    )
    .unwrap();
    assert_eq!(topics, vec![json!("a"), json!("b")]);
    let seen = server.join().unwrap();
    // json.dumps's spacing and \u escapes, the id counting from 1
    assert_eq!(seen[0], "{\"jsonrpc\": \"2.0\", \"id\": 1, \"method\": \"auth.hello\", \"params\": {\"role\": \"cli\"}}\n");
    assert_eq!(
        seen[1],
        "{\"jsonrpc\": \"2.0\", \"id\": 2, \"method\": \"ping\", \"params\": {}}\n"
    );
    assert_eq!(seen[2], "{\"jsonrpc\": \"2.0\", \"id\": 3, \"method\": \"tasks.list\", \"params\": {\"q\": \"\\u00e9\"}}\n");
    // the daemon closed the connection
    match c.call("ping", None) {
        Err(CliError::NotRunning(m)) => assert!(
            m == "FERB daemon closed the connection" || m == "FERB daemon went away",
            "{m}"
        ),
        other => panic!("{other:?}"),
    }
}

/// What `ferb trace` asks the daemon for, the request recorded.
struct Recorded(
    Vec<(String, Value)>,
    std::rc::Rc<std::cell::RefCell<Vec<Value>>>,
);

impl Rpc for Recorded {
    fn call(&mut self, method: &str, params: Option<Value>) -> Result<Value, CliError> {
        let (m, v) = self.0.remove(0);
        assert_eq!(m, method);
        self.1.borrow_mut().push(params.unwrap_or(Value::Null));
        Ok(v)
    }
}

/// `ferb trace [TASK] [--full|--json]` writes what cli/admin.py `trace` wrote for the same `trace.get` answer: every
/// recorded case of engine/fixtures/trace/traces.json (made by the Python build's `admin.trace` on a fake client), each
/// mode, through the program's dispatch.
// decision: ORCH-527
#[test]
fn trace_writes_pythons_bytes_for_every_recorded_case() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/trace/traces.json");
    let fixture = ferb_trace::py::loads(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    let mut compared = 0;
    for c in cases {
        let task_id = c["task_id"].as_str().unwrap();
        let with_episode = json!({
            "task_id": task_id, "spans": c["spans"], "outcome": c["outcome"], "episode": c["episode"]
        });
        // `plain`: the answer when nothing was composed (no episode): the span tree
        let bare = json!({"task_id": task_id, "spans": c["spans"], "outcome": c["outcome"], "episode": null});
        for (flags, key, answer) in [
            (&[][..], "story", &with_episode),
            (&["--full"][..], "full", &with_episode),
            (&["--json"][..], "json", &with_episode),
            (&[][..], "plain", &bare),
        ] {
            // a few large cases keep only their length in the fixture (traces_match_python checks those)
            let Some(want) = c[key].as_str() else {
                continue;
            };
            let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let mut h = FakeHost {
                rpc: Some(Box::new(Recorded(
                    vec![("trace.get".into(), answer.clone())],
                    asked.clone(),
                ))),
                ..FakeHost::default()
            };
            let mut args = vec!["trace", task_id];
            args.extend_from_slice(flags);
            assert_eq!(run(&argv(&args), &mut h), 0, "{} {key}", c["name"]);
            assert_eq!(text(&h.out), want, "{} {key}", c["name"]);
            assert_eq!(asked.borrow().as_slice(), &[json!({"task_id": task_id})]);
            compared += 1;
        }
    }
    assert_eq!(
        compared, 220,
        "every recorded output with its bytes in the fixture"
    );

    // no task: the latest ("last"); --full and --json together: argparse's refusal
    let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut h = FakeHost {
        rpc: Some(Box::new(Recorded(
            vec![("trace.get".into(), json!({"task_id": "t-1", "spans": []}))],
            asked.clone(),
        ))),
        ..FakeHost::default()
    };
    assert_eq!(run(&argv(&["trace"]), &mut h), 0);
    assert_eq!(text(&h.out), "no spans for task t-1\n");
    assert_eq!(asked.borrow().as_slice(), &[json!({"task_id": "last"})]);
    let mut h = FakeHost::default();
    assert_eq!(run(&argv(&["trace", "--full", "--json"]), &mut h), 2);
}

/// `ferb logs` prints a `tool.finished` event as ferb/trace/lines.py's step line (it printed a placeholder until the
/// traces part was wired in). Each expected line is what ferb/cli/logs.py `format_event` printed for the same event.
#[test]
fn a_tool_step_in_the_logs_is_pythons_step_line() {
    let cases: Value = serde_json::from_str(STEP_LINES).unwrap();
    for case in cases.as_array().unwrap() {
        let (ev, want) = (&case[0], case[1].as_str().unwrap());
        let got = ferb_device::cli::logs::format_event(ev, &ferb_device::cli::logs::tool_step_line)
            .unwrap();
        assert_eq!(got, want, "{ev}");
    }
}

/// Recorded from the Python build: `[event, format_event(event)]`.
const STEP_LINES: &str = r##"[
 [
  {
   "topic": "tool.finished",
   "at": "2026-11-15T10:02:03.5+05:30",
   "class": "info",
   "data": {
    "tool": "browser.open",
    "args": {
     "url": "https://example.com/a",
     "tab": 2,
     "x": null
    },
    "why": "to read the page",
    "summary": "opened",
    "seconds": 1.25
   }
  },
  "10:02:03  browser.open  url=https://example.com/a  tab=2  (why: to read the page)  → opened  [1.2s]"
 ],
 [
  {
   "topic": "tool.finished",
   "at": "2026-11-15T10:02:04+00:00",
   "class": "info",
   "data": {
    "tool": "shell.run",
    "args": {
     "command": "ls -la"
    },
    "error": "exit 2: no such file",
    "seconds": 0
   }
  },
  "10:02:04  shell.run  command=ls -la  ✗ exit 2: no such file  [0.0s]"
 ],
 [
  {
   "topic": "tool.finished",
   "at": "2026-11-15T23:59:59",
   "class": "info",
   "data": {}
  },
  "23:59:59  ?  → "
 ],
 [
  {
   "topic": "tool.finished",
   "at": "2026-11-15T01:00:00",
   "class": "info",
   "data": {
    "tool": "fs.read",
    "args": null,
    "why": "",
    "summary": "",
    "seconds": null,
    "error": ""
   }
  },
  "01:00:00  fs.read  → "
 ],
 [
  {
   "topic": "tool.finished",
   "at": "2026-11-15T01:00:00",
   "class": "info",
   "data": {
    "tool": "fs.read",
    "args": {
     "path": "/tmp/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    },
    "summary": "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    "seconds": 12
   }
  },
  "01:00:00  fs.read  path=/tmp/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa…  → xxxxxxxxxxxxxxxxxxxxxxxxxxxxxx  [12.0s]"
 ],
 [
  {
   "topic": "task.started",
   "at": "2026-11-15T01:00:00",
   "class": "notice",
   "data": {
    "id": "t-1",
    "text": "é"
   }
  },
  "01:00:00 * task.started             {\"id\":\"t-1\",\"text\":\"é\"}"
 ]
]"##;

/// T-1128: `-h` / `--help` prints argparse's help (Python's recorded text) for the command before it and exits 0,
/// before any setup, salvage or connection; `--version` first still wins.
// decision: DMN-601
#[test]
fn help_is_printed_and_exits_zero() {
    let recorded: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/device/help.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let mut h = FakeHost::default();
    assert_eq!(run(&argv(&["--help"]), &mut h), 0);
    let top = text(&h.out);
    assert!(top.starts_with("usage: ferb [-h] [--version]\n"), "{top}");
    assert!(!top.contains("venv"), "the named difference: {top}");
    assert!(h.err.is_empty());
    let mut h = FakeHost::default();
    assert_eq!(run(&argv(&["memory", "add", "-h"]), &mut h), 0);
    assert_eq!(text(&h.out), recorded["memory add"].as_str().unwrap());
    let mut h = FakeHost::default();
    assert_eq!(run(&argv(&["setup", "--service", "-h"]), &mut h), 0);
    assert!(text(&h.out).starts_with("usage: ferb setup [-h]"));
    assert_eq!((h.sessions, h.envs.clone()), (0, vec![]), "nothing set up");
    let mut h = FakeHost::default();
    assert_eq!(run(&argv(&["--version", "--help"]), &mut h), 0);
    assert_eq!(text(&h.out), "ferb 0.28.0\n");
}
