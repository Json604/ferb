//! The extension keepalive in the composed daemon (HAND-522; journal 59: a live profile's connection dropped mid-task):
//! through the real program, a scripted fake extension behind a relay started by a fake "Google Chrome.app" says hello
//! listing a tab that live task `task-1` holds. While that tab is open the daemon pings the extension; after the tab
//! closes it stops. The profile's disconnect and reconnect while the task holds the tab are journaled. The keepalive's
//! 15 s period is shortened through the stand-ins' control socket so the test sees it run; no browser is started.
#![cfg(target_os = "macos")]

mod shim;
mod support;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ferb_browser::protocol::{sign, MIN_EXTENSION_VERSION, PROTOCOL_VERSION};
use serde_json::{json, Value};
use support::*;

const ORIGIN: &str = "chrome-extension://jaclnphefmmgmpofnabffahdkbjjompm/";
const INSTALL: &str = "install-0001";
const KEY: [u8; 32] = [7; 32];

/// The relay stand-in, as the browser's native host: it forks; the child (whose parent is this bundled program)
/// connects to the socket, says the native host's hello, then carries lines both ways between its stdin/stdout and
/// the socket until either side ends.
const RELAY_C: &str = r#"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <sys/select.h>
static int all(int fd, const char *b, ssize_t n) {
    while (n > 0) { ssize_t w = write(fd, b, n); if (w <= 0) return -1; b += w; n -= w; }
    return 0;
}
int main(int argc, char **argv) {
    pid_t p = fork();
    if (p == 0) {
        int s = socket(AF_UNIX, SOCK_STREAM, 0);
        struct sockaddr_un a; memset(&a, 0, sizeof a); a.sun_family = AF_UNIX;
        strncpy(a.sun_path, argv[1], sizeof a.sun_path - 1);
        if (connect(s, (struct sockaddr *)&a, sizeof a) != 0) return 2;
        dprintf(s, "{\"jsonrpc\": \"2.0\", \"id\": 1, \"method\": \"auth.hello\", \"params\": {\"role\": \"native_host\", \"origin\": \"%s\"}}\n", argv[2]);
        static char buf[65536];
        for (;;) {
            fd_set r; FD_ZERO(&r); FD_SET(0, &r); FD_SET(s, &r);
            if (select(s + 1, &r, 0, 0, 0) < 0) return 3;
            if (FD_ISSET(0, &r)) {
                ssize_t n = read(0, buf, sizeof buf);
                if (n <= 0 || all(s, buf, n)) { close(s); return 0; }
            }
            if (FD_ISSET(s, &r)) {
                ssize_t n = read(s, buf, sizeof buf);
                if (n <= 0 || all(1, buf, n)) return 0;
            }
        }
    }
    int st = 0; waitpid(p, &st, 0);
    return WIFEXITED(st) ? WEXITSTATUS(st) : 1;
}
"#;

/// `<dir>/Google Chrome.app` (bundle id com.google.Chrome) with the relay stand-in as its executable.
fn fake_chrome(dir: &Path) -> PathBuf {
    let contents = dir.join("Google Chrome.app/Contents");
    std::fs::create_dir_all(contents.join("MacOS")).unwrap();
    std::fs::write(
        contents.join("Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>com.google.Chrome</string></dict></plist>"#,
    )
    .unwrap();
    let src = dir.join("relay.c");
    std::fs::write(&src, RELAY_C).unwrap();
    let exe = contents.join("MacOS/relay-parent");
    let built = Command::new("cc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .status()
        .expect("cc runs");
    assert!(built.success(), "cc built the relay stand-in");
    exe
}

/// One connection of the fake extension: what it was sent (op, when), and its relay.
struct Ext {
    child: Child,
    /// Taken out (closed) when the connection ends.
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    seq: Arc<AtomicI64>,
    sent: Arc<Mutex<Vec<(String, Instant)>>>,
}

fn frame(seq: &AtomicI64, mtype: &str, body: &Value) -> String {
    let body_json = ferb_py::json::dumps(body);
    let n = seq.fetch_add(1, Ordering::SeqCst) + 1;
    let mac = sign(&KEY, INSTALL, n, mtype, &body_json);
    format!(
        "{}\n",
        json!({"install_id": INSTALL, "type": mtype, "body_json": body_json, "seq": n, "mac": mac})
    )
}

impl Ext {
    /// Connects through the relay and says hello listing tab 11 of `task-1`; every command is answered as the
    /// extension does when it succeeds.
    fn connect(relay: &Path, socket: &Path, seq: Arc<AtomicI64>) -> Ext {
        let mut child = Command::new(relay)
            .arg(socket)
            .arg(ORIGIN)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = Arc::new(Mutex::new(child.stdin.take()));
        let stdout = child.stdout.take().unwrap();
        let sent: Arc<Mutex<Vec<(String, Instant)>>> = Arc::default();
        let hello = json!({"version": MIN_EXTENSION_VERSION, "protocol": PROTOCOL_VERSION, "browser": "chrome",
                           "paired": true, "nonce": format!("nonce-{}", seq.load(Ordering::SeqCst)),
                           "tabs": [{"tab_id": 11, "task": "task-1", "url": "https://example.com/", "borrowed": false}]});
        write(&stdin, &frame(&seq, "hello", &hello)).unwrap();
        let (w, s, q) = (stdin.clone(), sent.clone(), seq.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if v["type"] != "cmd" {
                    continue;
                }
                let cmd: Value = serde_json::from_str(v["body"].as_str().unwrap()).unwrap();
                s.lock()
                    .unwrap()
                    .push((cmd["op"].as_str().unwrap().to_string(), Instant::now()));
                let result =
                    json!({"id": cmd["id"], "gen": cmd["gen"], "status": "ok", "value": {}});
                if write(&w, &frame(&q, "result", &result)).is_err() {
                    break;
                }
            }
        });
        Ext {
            child,
            stdin,
            seq,
            sent,
        }
    }

    fn pings(&self) -> usize {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|(op, _)| op == "ping")
            .count()
    }

    fn wait_pings(&self, n: usize) {
        let until = Instant::now() + Duration::from_secs(10);
        while self.pings() < n {
            assert!(
                Instant::now() < until,
                "only {} pings: {:?}",
                self.pings(),
                self.sent.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The browser says the tab closed (the owner closed it).
    fn tab_removed(&self, tab: i64) {
        let body = json!({"event": "removed", "tab_id": tab});
        write(&self.stdin, &frame(&self.seq, "tab_event", &body)).unwrap();
    }

    /// The extension's connection ends (the relay closes the socket and exits).
    fn end(mut self) {
        self.stdin.lock().unwrap().take();
        let _ = self.child.wait();
    }
}

impl Drop for Ext {
    /// A failed test leaves no relay running.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One line to the relay, while the connection is open.
fn write(stdin: &Mutex<Option<ChildStdin>>, line: &str) -> std::io::Result<()> {
    match stdin.lock().unwrap().as_mut() {
        Some(w) => w.write_all(line.as_bytes()),
        None => Err(std::io::Error::other("closed")),
    }
}

fn journal(run: &Run, kind: &str) -> Vec<Value> {
    let db = rusqlite::Connection::open(run.home.join("ferb.db")).unwrap();
    let mut q = db
        .prepare("SELECT data_json FROM journal WHERE kind = ? ORDER BY seq")
        .unwrap();
    let rows: Vec<String> = q
        .query_map([kind], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    rows.iter()
        .map(|d| serde_json::from_str(d).unwrap())
        .collect()
}

fn wait_journal(run: &Run, kind: &str, n: usize) -> Vec<Value> {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let rows = journal(run, kind);
        if rows.len() >= n || Instant::now() > until {
            return rows;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// decision: HAND-522
#[test]
fn the_composed_daemon_pings_a_profile_while_a_task_tab_is_open_and_journals_its_drops() {
    let tmp = root();
    let (run, _shim) = shim::started(tmp.path(), "01");
    run.control(json!({"do": "keepalive", "seconds": 0.2}));
    let key: String = KEY.iter().map(|b| format!("{b:02x}")).collect();
    run.control(json!({"do": "sql", "statements": [[format!(
        "INSERT INTO ext_installs (install_id, browser, folder, identity, key, last_seq, paired_at) \
         VALUES ('{INSTALL}', 'chrome', 'Default', 'owner', X'{key}', 0, '2026-01-01T09:00:00+00:00')"
    ), []]]}));
    let apps = tmp.path().join("apps");
    std::fs::create_dir_all(&apps).unwrap();
    let relay = fake_chrome(&apps);
    let seq = Arc::new(AtomicI64::new(0));

    // task-1's tab is open: the keepalive pings, again and again
    let first = Ext::connect(&relay, &run.socket(), seq.clone());
    first.wait_pings(3);

    // the profile's connection drops while task-1 holds its tab: journaled
    first.end();
    let down = wait_journal(&run, "browser.task_profile_disconnected", 1);
    assert_eq!(
        down,
        vec![json!({"profile": "chrome/Default", "tasks": ["task-1"], "tabs": 1, "gen": 1})]
    );

    // it connects again: journaled, and the keepalive runs again for the same tab
    let second = Ext::connect(&relay, &run.socket(), seq.clone());
    let up = wait_journal(&run, "browser.task_profile_reconnected", 1);
    assert_eq!(
        up,
        vec![json!({"profile": "chrome/Default", "tasks": ["task-1"], "tabs": 1, "gen": 2})]
    );
    second.wait_pings(3);

    // the tab closes: no task tab is open there any more, so the pings stop
    second.tab_removed(11);
    std::thread::sleep(Duration::from_millis(400));
    let after = second.pings();
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(second.pings(), after, "pings went on after the tab closed");
    // and the drop of a profile no task holds a tab in is no journal row
    second.end();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(journal(&run, "browser.task_profile_disconnected").len(), 1);
}
