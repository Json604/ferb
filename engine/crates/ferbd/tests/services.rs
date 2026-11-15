//! The daemon's services through the real `ferbd daemon` program on temp homes: the shell's egress proxy (a command
//! reaches the network only through it, and the gateway decides each destination), the memory's background work,
//! the Claude tool-list relay, and a hang-up stopping the daemon like SIGTERM.

mod support;

use std::io::Read;
use std::net::TcpListener;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use support::*;

/// A shim connection with a run token for `task-1` (held by the gateway) and `scopes`.
fn shim(run: &Run, scopes: &[&str]) -> Raw {
    let token = run.control(
        json!({"do": "issue", "task_id": "task-1", "worker_id": "w-1",
                                   "identity": "owner", "scopes": scopes}),
    );
    run.control(json!({"do": "hold_task", "task_id": "task-1"}));
    let mut c = Raw::connect(&run.socket()).unwrap();
    c.send(
        &json!({"jsonrpc": "2.0", "id": 0, "method": "auth.hello",
                "params": {"role": "mcp_shim", "token": token}})
        .to_string(),
    );
    let hello = c.recv().unwrap();
    assert!(hello.contains("\"result\""), "{hello}");
    c
}

fn call(c: &mut Raw, id: i64, name: &str, args: Value) -> Value {
    c.send(
        &json!({"jsonrpc": "2.0", "id": id, "method": "tools.call",
                "params": {"name": name, "arguments": args}})
        .to_string(),
    );
    let line = c.recv().expect("an answer");
    serde_json::from_str::<Value>(&line).unwrap()["result"].clone()
}

#[test]
fn a_shell_command_reaches_the_network_only_through_the_proxy() {
    let tmp = root();
    let run = Run::start(tmp.path(), "60", json!({"group_a": true}));
    std::fs::create_dir_all(run.home.join("workspace")).unwrap();
    // a local server the command tries to reach, directly and through the proxy
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    server.set_nonblocking(true).unwrap();
    let port = server.local_addr().unwrap().port();
    let mut c = shim(&run, &["shell"]);
    let env = call(
        &mut c,
        1,
        "shell.run",
        json!({"command": "printenv HTTPS_PROXY", "timeout_s": 20}),
    );
    let text = env.to_string();
    assert_eq!(env["is_error"], false, "{env}");
    assert!(text.contains("127.0.0.1:"), "the proxy's address: {env}");
    // directly: the sandbox closes the network; through the proxy: a local address needs the proxy's own check
    let direct = call(
        &mut c,
        2,
        "shell.run",
        json!({"command": format!("/usr/bin/curl --noproxy '*' -sS -m 5 http://127.0.0.1:{port}/ ; echo exit=$?"),
               "timeout_s": 20}),
    );
    let proxied = call(
        &mut c,
        3,
        "shell.run",
        json!({"command": format!("/usr/bin/curl -sS -m 5 -o /dev/null -w 'status=%{{http_code}}' http://127.0.0.1:{port}/ ; echo ' exit='$?"),
               "timeout_s": 20}),
    );
    eprintln!("direct: {direct}\nproxied: {proxied}");
    assert!(!direct.to_string().contains("exit=0"), "{direct}");
    assert!(!proxied.to_string().contains("status=200"), "{proxied}");
    // nothing reached the server: not directly, and the proxy refused the local address
    let mut accepted = 0;
    while let Ok((mut s, _)) = server.accept() {
        let mut b = [0u8; 16];
        let _ = s.read(&mut b);
        accepted += 1;
    }
    assert_eq!(accepted, 0);
}

#[test]
fn the_memory_loops_run_on_a_temp_home() {
    let tmp = root();
    let run = Run::start(
        tmp.path(),
        "61",
        json!({"group_a": true, "memory_loops": true}),
    );
    // the start's re-index is a no-op on a new store; no embedding model in this home: journaled, search runs on
    // keywords; the hourly pass ran once: no day to distill, no card, and the profile pass, due on a new store, had
    // no item to review (no model is called for an empty profile)
    let until = Instant::now() + Duration::from_secs(60);
    let kinds = loop {
        let rows = run.table("journal");
        let kinds: Vec<String> = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r[3].as_str().unwrap_or("").to_string())
            .collect();
        if kinds.iter().any(|k| k == "memory.embedding_model")
            && kinds.iter().any(|k| k == "memory.profile_pass")
        {
            break kinds;
        }
        assert!(
            Instant::now() < until,
            "the memory loops didn't run: {kinds:?}"
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(
        !kinds.iter().any(|k| k == "memory.distill_failed"),
        "{kinds:?}"
    );
    let rows = run.table("journal");
    let model = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r[3] == "memory.embedding_model")
        .unwrap();
    assert!(
        model[4].as_str().unwrap().contains("\"unavailable\""),
        "{model}"
    );
}

#[test]
fn the_claude_relay_forwards_nothing_without_a_live_runs_prefix() {
    use std::io::Write;
    let tmp = root();
    let run = Run::start(tmp.path(), "63", json!({}));
    let base = run.control(json!({"do": "claude_relay", "run_id": "run-1"}));
    let base = base.as_str().unwrap();
    assert!(base.starts_with("http://127.0.0.1:"), "{base}");
    let rest = &base["http://".len()..];
    let (addr, path) = rest.split_once('/').unwrap();
    let secret_path = format!("/{path}");
    assert!(secret_path.starts_with("/run/run-1/"), "{secret_path}");
    // another run's prefix, a wrong secret and no prefix at all: 403, nothing forwarded (no request leaves the Mac)
    for p in [
        "/v1/messages".to_string(),
        "/run/run-1/wrong-secret/v1/messages".to_string(),
        "/run/run-2/x/v1/messages".to_string(),
    ] {
        let mut s = std::net::TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        write!(
            s,
            "POST {p} HTTP/1.1\r\nHost: {addr}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        )
        .unwrap();
        let mut answer = String::new();
        let _ = s.read_to_string(&mut answer);
        assert!(answer.starts_with("HTTP/1.1 403"), "{p}: {answer}");
    }
}

#[test]
fn shutdown_writes_the_resource_logs_partial_window() {
    let tmp = root();
    let mut run = Run::start(tmp.path(), "64", json!({"resources": true}));
    let (status, _) = run.stop();
    assert_eq!(status.code(), Some(0));
    let db = rusqlite::Connection::open(run.home.join("ferb.db")).unwrap();
    let (n, footprint, child_count, work): (i64, Option<i64>, i64, String) = db
        .query_row(
            "SELECT COUNT(*), MAX(footprint), MAX(child_count), MAX(work_json) FROM resource_samples",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(n, 1, "one sample: the window since start-up");
    assert!(
        footprint.unwrap_or(0) > 0,
        "the daemon's own footprint, from libproc"
    );
    assert!(child_count >= 0);
    let work: Value = serde_json::from_str(&work).unwrap();
    let keys: Vec<&str> = work
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "events",
            "perception",
            "jev",
            "models",
            "tool_calls",
            "tasks_running",
            "top",
            "preview"
        ]
    );
}

/// `ferbd <args>` with FERB's home and HOME in `root`, every program it would run recorded (never `launchctl`).
fn ferbd(root: &std::path::Path, args: &[&str]) -> (i32, String, String, Vec<Vec<String>>) {
    let runs = root.join("runs.jsonl");
    let _ = std::fs::remove_file(&runs);
    let out = std::process::Command::new(BIN)
        .args(args)
        .env_clear()
        .env("FERB_HOME", root.join("home"))
        .env("HOME", root.join("user"))
        .env("PATH", "/usr/bin:/bin")
        .env("FERBD_FAKE_RUN", &runs)
        .env("FERBD_APP_BUNDLE", root.join("Applications/FERB.app"))
        .env("FERBD_FAKE_ANSWERS", root.join("answers.json"))
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let ran = std::fs::read_to_string(&runs)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        ran,
    )
}

// decision: DMN-593
#[test]
fn setup_service_uninstall_and_doctor_run_with_a_recording_launchctl() {
    let tmp = root();
    let r = tmp.path();
    std::fs::create_dir_all(r.join("user")).unwrap();
    // an invented FERB.app with the two programs the Rust service names (a copy of this test's ferbd)
    let app = r.join("Applications/FERB.app");
    for (part, from) in [
        ("Contents/Helpers/FERB.app/Contents/MacOS/FERB", BIN),
        // ferb-device BUNDLE_RUST_NATIVE_HOST, beside the agent app
        ("Contents/Helpers/ferb-native-host", BIN),
        ("Contents/Helpers/ferb-secrets", BIN),
        // the pinned Codex (never run: the recording runner answers its --version)
        (
            "Contents/Resources/node/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex",
            BIN,
        ),
    ] {
        let p = app.join(part);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::copy(from, &p).unwrap();
    }
    std::fs::write(
        app.join("Contents/Info.plist"),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
         <key>CFBundleIdentifier</key><string>com.ferb.FERB</string>\
         <key>CFBundleShortVersionString</key><string>0.28.0</string>\
         <key>CFBundleVersion</key><string>202611130000</string></dict></plist>\n",
    )
    .unwrap();
    // its signature's seal of the secrets helper, and what the recording codesign answers about that copy
    let cdhash = "00112233445566778899aabbccddeeff00112233";
    std::fs::create_dir_all(app.join("Contents/_CodeSignature")).unwrap();
    std::fs::write(
        app.join("Contents/_CodeSignature/CodeResources"),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict><key>files2</key><dict>\
         <key>Helpers/ferb-secrets</key><dict><key>cdhash</key><data>ABEiM0RVZneImaq7zN3u/wARIjM=</data>\
         </dict></dict></dict></plist>\n",
    )
    .unwrap();
    std::fs::write(
        r.join("answers.json"),
        json!([
            [["/usr/bin/codesign", "-dv", "--verbose=4"], {"stderr": format!("CDHash={cdhash}\n")}],
            [["/usr/bin/codesign", "-d", "-r-"], {"stdout": format!("designated => cdhash H\"{cdhash}\"\n")}],
            [["*/bin/codex", "--version"], {"stdout": "codex-cli 0.166.0\n"}],
            [["launchctl", "print"], {"code": 113}],
            [["/bin/launchctl", "print"], {"code": 113}],
        ])
        .to_string(),
    )
    .unwrap();
    let (code, out, err, ran) = ferbd(r, &["setup", "--service"]);
    eprintln!("setup --service: {code}\n{out}\n{err}\n{ran:?}");
    // DMN-593: from FERB.app the service is the app's own registration: no LaunchAgent is written or loaded, and
    // the record says so
    let plist = r.join("user/Library/LaunchAgents/com.ferb.daemon.plist");
    assert!(!plist.exists(), "no LaunchAgent from FERB.app");
    let record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(r.join("home/service.json")).unwrap())
            .unwrap();
    assert_eq!(record["service"], "app", "{record}");
    assert!(
        !ran.iter()
            .any(|a| a.iter().any(|w| w == "bootstrap" || w == "load")),
        "nothing loaded: {ran:?}"
    );
    // launchctl was only recorded
    assert!(
        ran.iter()
            .any(|a| a.first().is_some_and(|p| p.ends_with("launchctl"))),
        "{ran:?}"
    );
    let (code, out, err, _) = ferbd(r, &["doctor"]);
    eprintln!("doctor: {code}\n{out}\n{err}");
    assert!(!out.is_empty() || !err.is_empty());
    assert!(!err.contains("not available in this build"), "{err}");
    let (code, out, err, ran) = ferbd(r, &["uninstall"]);
    eprintln!("uninstall: {code}\n{out}\n{err}\n{ran:?}");
    assert_eq!(code, 0, "{out}{err}");
    assert!(!plist.exists(), "no definition is left");
}

#[test]
fn a_hang_up_stops_the_daemon_like_sigterm() {
    let tmp = root();
    let mut run = Run::start(tmp.path(), "62", json!({}));
    let child = run.child.take().unwrap();
    // SAFETY: the pid is our own child's, still unreaped (we hold it).
    unsafe {
        libc::kill(child.id() as i32, libc::SIGHUP);
    }
    let mut child = child;
    let until = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(Instant::now() < until, "the daemon didn't stop");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(0));
    assert!(!run.socket().exists(), "the socket is removed");
}
