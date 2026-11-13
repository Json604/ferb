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

    // parts not ported yet say so
    for cmd in [
        vec!["trace"],
        vec!["stats"],
        vec!["mcp"],
        vec!["daemon", "run"],
        vec!["runtimes"],
        vec!["memory", "dashboard"],
        vec!["browsers", "extension"],
    ] {
        let mut h = FakeHost::default();
        assert_eq!(run(&argv(&cmd), &mut h), 1, "{cmd:?}");
        assert!(text(&h.err).contains("isn't part of the Rust program yet"));
    }
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
