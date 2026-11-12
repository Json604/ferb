//! The built relay against what the Python relay did (ENG-3). `rust/fixtures/native-host/cases.txt` was recorded by
//! `make.py` from `ferb/native_host/main.py`; each case here plays the same scripted browser and fake daemon
//! (a Unix socket in a temporary folder, found through `FERB_HOME` as in `ferb/paths.py:8-14`) and asserts:
//! - the hello line is Python's, byte for byte;
//! - the exit code is Python's, except where Python lingered after a daemon line it could not parse
//!   (`py_lingered`): there this relay must exit with 1 on its own;
//! - the bytes at the daemon and at the browser are the fixture's `raw_up` / `raw_down`: the sender's own bytes for
//!   exactly the messages Python forwarded. `make.py` refused to write a case unless Python's parser read those
//!   bytes and Python's rewritten bytes as the same value, strings included. Where the sender wrote `json.dumps`
//!   form they are also Python's exact bytes; `the_daemons_own_form_is_python_byte_for_byte` holds that.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/native-host/cases.txt"
);
const LIMIT: Duration = Duration::from_secs(30);

#[derive(Default, Debug)]
struct Case {
    name: String,
    origin: Option<Vec<u8>>,
    no_daemon: bool,
    reply: Option<Vec<u8>>,
    stdin: Vec<u8>,
    up_lines: usize,
    daemon: Vec<u8>,
    end: String,
    exit: i32,
    py_lingered: bool,
    py_hello: Vec<u8>,
    py_up: Vec<u8>,
    py_down: Vec<u8>,
    raw_up: Vec<u8>,
    raw_down: Vec<u8>,
}

/// `h:<hex>` and `r:<hex>*<count>` parts, or `-` (make.py's `enc`).
fn bytes(value: &str) -> Vec<u8> {
    let hex = |h: &str| -> Vec<u8> {
        (0..h.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
            .collect()
    };
    let mut out = Vec::new();
    for part in value.split(' ').filter(|p| *p != "-") {
        match part.split_at(2) {
            ("h:", h) => out.extend(hex(h)),
            ("r:", r) => {
                let (unit, count) = r.split_once('*').unwrap();
                out.extend(hex(unit).repeat(count.parse().unwrap()));
            }
            _ => panic!("bad part {part:?}"),
        }
    }
    out
}

fn cases() -> Vec<Case> {
    let text = std::fs::read_to_string(FIXTURE).expect("run rust/fixtures/native-host/make.py");
    let mut cases: Vec<Case> = Vec::new();
    for line in text
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let (key, value) = line.split_once(' ').unwrap();
        if key == "case" {
            cases.push(Case {
                name: value.into(),
                ..Case::default()
            });
            continue;
        }
        let c = cases.last_mut().unwrap();
        match key {
            "origin" => c.origin = (value != "none").then(|| bytes(value)),
            "no_daemon" => c.no_daemon = value == "1",
            "reply" => c.reply = (value != "close").then(|| bytes(value)),
            "stdin" => c.stdin = bytes(value),
            "up_lines" => c.up_lines = value.parse().unwrap(),
            "daemon" => c.daemon = bytes(value),
            "end" => c.end = value.into(),
            "exit" => c.exit = value.parse().unwrap(),
            "py_lingered" => c.py_lingered = value == "1",
            "py_hello" => c.py_hello = bytes(value),
            "py_up" => c.py_up = bytes(value),
            "py_down" => c.py_down = bytes(value),
            "raw_up" => c.raw_up = bytes(value),
            "raw_down" => c.raw_down = bytes(value),
            _ => panic!("unknown key {key:?}"),
        }
    }
    cases
}

struct Outcome {
    hello: Vec<u8>,
    up: Vec<u8>,
    down: Vec<u8>,
    exit: i32,
    /// The relay ended before the browser closed stdin or the daemon closed its side.
    on_its_own: bool,
}

fn wait(child: &mut Child, limit: Duration) -> Option<i32> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status.code().expect("the relay was killed by a signal"));
        }
        if Instant::now() > deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn accept(listener: &UnixListener) -> UnixStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + LIMIT;
    loop {
        match listener.accept() {
            Ok((conn, _)) => {
                conn.set_nonblocking(false).unwrap();
                return conn;
            }
            Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(2)),
            Err(e) => panic!("the relay never connected: {e}"),
        }
    }
}

/// The same script as `run` in make.py, with the Rust binary.
fn run(c: &Case) -> Outcome {
    static N: AtomicUsize = AtomicUsize::new(0);
    let home: PathBuf = std::env::temp_dir().join(format!(
        "fnh-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(home.join("run")).unwrap();
    let listener =
        (!c.no_daemon).then(|| UnixListener::bind(home.join("run").join("ferb.sock")).unwrap());

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ferb-native-host"));
    if let Some(origin) = &c.origin {
        cmd.arg(OsStr::from_bytes(origin));
    }
    let mut child = cmd
        .env_clear()
        .env("FERB_HOME", &home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut child_in = child.stdin.take().unwrap();
    let mut child_out = child.stdout.take().unwrap();

    let mut hello = Vec::new();
    let mut conn = None;
    if let Some(listener) = &listener {
        let mut stream = accept(listener);
        stream.set_read_timeout(Some(LIMIT)).unwrap();
        let mut byte = [0u8; 1];
        while hello.last() != Some(&b'\n') {
            stream.read_exact(&mut byte).expect("no hello line");
            hello.push(byte[0]);
        }
        stream.set_read_timeout(None).unwrap();
        match &c.reply {
            Some(reply) => {
                stream.write_all(reply).unwrap();
                conn = Some(stream);
            }
            None => drop(stream),
        }
    }

    let stdin_bytes = c.stdin.clone();
    let writer = thread::spawn(move || {
        let _ = child_in.write_all(&stdin_bytes); // the relay may have gone
        let _ = child_in.flush();
        child_in
    });
    let down_reader = thread::spawn(move || {
        let mut down = Vec::new();
        child_out.read_to_end(&mut down).unwrap();
        down
    });
    let up = Arc::new(Mutex::new(Vec::new()));
    let up_reader = conn.as_ref().map(|stream| {
        let (mut stream, up) = (stream.try_clone().unwrap(), Arc::clone(&up));
        thread::spawn(move || {
            let mut chunk = [0u8; 65536];
            while let Ok(n @ 1..) = stream.read(&mut chunk) {
                up.lock().unwrap().extend_from_slice(&chunk[..n]);
            }
        })
    });

    let deadline = Instant::now() + LIMIT;
    while up.lock().unwrap().iter().filter(|b| **b == b'\n').count() < c.up_lines {
        assert!(
            Instant::now() < deadline,
            "{}: the daemon never got {} lines",
            c.name,
            c.up_lines
        );
        thread::sleep(Duration::from_millis(2));
    }
    if let Some(stream) = &mut conn {
        let _ = stream.write_all(&c.daemon); // the relay may have gone
    }

    let mut on_its_own = false;
    let exit = match c.end.as_str() {
        "stdin_eof" => {
            drop(writer.join().unwrap());
            wait(&mut child, LIMIT)
        }
        "daemon_eof" if c.py_lingered => {
            // Python stayed until the browser closed stdin; this relay must leave by itself, daemon side open.
            on_its_own = true;
            let code = wait(&mut child, LIMIT);
            drop(writer.join().unwrap());
            code
        }
        "daemon_eof" => {
            conn.as_ref().unwrap().shutdown(Shutdown::Write).unwrap();
            let code = wait(&mut child, LIMIT);
            drop(writer.join().unwrap());
            code
        }
        "self" => {
            on_its_own = true;
            let code = wait(&mut child, LIMIT);
            drop(writer.join().unwrap());
            code
        }
        other => panic!("unknown end {other:?}"),
    };
    let Some(exit) = exit else {
        let _ = child.kill();
        panic!("{}: the relay did not exit", c.name);
    };
    let down = down_reader.join().unwrap();
    if let Some(reader) = up_reader {
        reader.join().unwrap();
    }
    drop(conn);
    let _ = std::fs::remove_dir_all(&home);
    let up = up.lock().unwrap().clone();
    Outcome {
        hello,
        up,
        down,
        exit,
        on_its_own,
    }
}

/// Long byte strings compared without printing megabytes.
fn same(case: &str, what: &str, got: &[u8], want: &[u8]) {
    if got != want {
        let at = got
            .iter()
            .zip(want)
            .position(|(a, b)| a != b)
            .unwrap_or(got.len().min(want.len()));
        let show = |b: &[u8]| {
            String::from_utf8_lossy(&b[at.saturating_sub(40).min(b.len())..(at + 40).min(b.len())])
                .into_owned()
        };
        panic!(
            "{case}: {what} differs at byte {at} (got {} bytes, want {}): got …{:?}… want …{:?}…",
            got.len(),
            want.len(),
            show(got),
            show(want)
        );
    }
}

#[test]
fn the_relay_does_what_the_python_relay_did() {
    let cases = cases();
    assert!(cases.len() >= 40, "the fixture lost cases");
    for c in &cases {
        let got = run(c);
        same(&c.name, "the hello", &got.hello, &c.py_hello);
        same(&c.name, "bytes at the daemon", &got.up, &c.raw_up);
        same(&c.name, "bytes at the browser", &got.down, &c.raw_down);
        // The one chosen difference: a daemon line that is not JSON ends this relay (main.rs, difference 3).
        let want = if c.py_lingered { 1 } else { c.exit };
        assert_eq!(got.exit, want, "{}: exit code", c.name);
        assert!(got.on_its_own == (c.py_lingered || c.end == "self"));
    }
}

/// Where the sender wrote `json.dumps` form, forwarding the bytes is byte for byte what Python's rewrite produced.
/// Everything the Python daemon sends is in that form (`ferb/daemon/server.py:452`).
#[test]
fn the_daemons_own_form_is_python_byte_for_byte() {
    let cases = cases();
    let named = |name: &str| cases.iter().find(|c| c.name == name).expect(name);
    for name in [
        "both_directions",
        "down_canonical",
        "down_last_line_without_newline",
        "down_at_the_cap_then_over",
    ] {
        let c = named(name);
        assert!(!c.raw_down.is_empty(), "{name}");
        same(name, "raw_down against py_down", &c.raw_down, &c.py_down);
    }
    // A browser writes compact JSON, so Python's rewrite differs in spacing there, and can outgrow the daemon's
    // 1 MiB line limit (ferb/daemon/server.py:201): a frame at the cap became a line three times as long.
    let c = named("up_at_the_cap_non_ascii");
    assert!(c.py_up.len() > 3 * 1024 * 1024 && c.raw_up.len() < 1024 * 1024 + 64);
    assert_ne!(
        named("both_directions").raw_up,
        named("both_directions").py_up
    );
}
