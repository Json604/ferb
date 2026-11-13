//! The native host's connection through the real program (HAND-223, HAND-406, HAND-438): the daemon reads who started
//! the relay (its parent, from the socket) and that process's app (its executable's bundle), takes a connection whose
//! parent runs a browser FERB knows, and refuses and journals every other one, plus a hello the socket refuses itself.
//! The relay's parent is a small program compiled into a fake `.app` here (an Apple binary copied into a bundle is
//! killed by macOS), so the bundle id the daemon reads is the fake bundle's.
#![cfg(target_os = "macos")]

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use support::*;

const ORIGIN: &str = "chrome-extension://jaclnphefmmgmpofnabffahdkbjjompm/";

/// The relay stand-in: it forks; the child (whose parent is this bundled program) connects to the socket, says the
/// native host's hello and prints whatever the daemon writes within 1.5 s.
const RELAY_C: &str = r#"
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <sys/time.h>
int main(int argc, char **argv) {
    pid_t p = fork();
    if (p == 0) {
        int s = socket(AF_UNIX, SOCK_STREAM, 0);
        struct sockaddr_un a; memset(&a, 0, sizeof a); a.sun_family = AF_UNIX;
        strncpy(a.sun_path, argv[1], sizeof a.sun_path - 1);
        if (connect(s, (struct sockaddr *)&a, sizeof a) != 0) return 2;
        dprintf(s, "{\"jsonrpc\": \"2.0\", \"id\": 1, \"method\": \"auth.hello\", \"params\": {\"role\": \"native_host\", \"origin\": \"%s\"}}\n", argv[2]);
        struct timeval tv = {1, 500000};
        setsockopt(s, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv);
        char buf[4096]; ssize_t n;
        while ((n = read(s, buf, sizeof buf)) > 0) fwrite(buf, 1, n, stdout);
        fflush(stdout);
        return 0;
    }
    int st = 0; waitpid(p, &st, 0);
    return WIFEXITED(st) ? WEXITSTATUS(st) : 1;
}
"#;

/// `<dir>/<name>.app` with the relay stand-in as its executable and `id` as its bundle id.
fn fake_app(dir: &Path, name: &str, id: &str) -> PathBuf {
    let contents = dir.join(format!("{name}.app/Contents"));
    std::fs::create_dir_all(contents.join("MacOS")).unwrap();
    std::fs::write(
        contents.join("Info.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>{id}</string></dict></plist>"#
        ),
    )
    .unwrap();
    let src = dir.join(format!("{name}.c"));
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

fn relay_through(exe: &Path, socket: &Path, origin: &str) -> String {
    let out = Command::new(exe).arg(socket).arg(origin).output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn refusals(run: &Run) -> Vec<Value> {
    let db = rusqlite::Connection::open(run.home.join("ferb.db")).unwrap();
    let mut q = db
        .prepare(
            "SELECT data_json FROM journal WHERE kind = 'browser.connection_refused' ORDER BY seq",
        )
        .unwrap();
    let rows: Vec<String> = q
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    rows.iter()
        .map(|d| serde_json::from_str(d).unwrap())
        .collect()
}

// decision: HAND-223, HAND-406, HAND-438
#[test]
fn a_browsers_relay_is_taken_and_every_other_one_is_refused_and_journaled() {
    let tmp = root();
    let run = Run::start(tmp.path(), "01", json!({}));
    let apps = tmp.path().join("apps");
    std::fs::create_dir_all(&apps).unwrap();

    // started by Chrome: the hello is answered and nothing refuses it
    let chrome = fake_app(&apps, "Google Chrome", "com.google.Chrome");
    let said = relay_through(&chrome, &run.socket(), ORIGIN);
    assert!(said.contains("native_host"), "{said}");
    assert!(!said.contains("refused"), "{said}");

    // started by an app that isn't a browser: refused, naming it (read from its bundle)
    let other = fake_app(&apps, "Other", "com.example.other");
    let said = relay_through(&other, &run.socket(), ORIGIN);
    assert!(
        said.contains("started by com.example.other, not a web browser FERB knows"),
        "{said}"
    );

    // started by no app at all (this test's process): refused as FERB can't tell
    let mut direct = Raw::connect(&run.socket()).unwrap();
    direct.send(
        &json!({"jsonrpc": "2.0", "id": 1, "method": "auth.hello",
                "params": {"role": "native_host", "origin": ORIGIN}})
        .to_string(),
    );
    let hello = direct.recv().expect("the hello's answer");
    assert!(hello.contains("native_host"), "{hello}");
    let refused = direct.recv().expect("the refusal");
    assert!(refused.contains("couldn't tell which app"), "{refused}");

    // a hello the socket refuses itself (an origin that isn't FERB's extension): a "host" refusal
    let mut stranger = Raw::connect(&run.socket()).unwrap();
    stranger.send(
        &json!({"jsonrpc": "2.0", "id": 1, "method": "auth.hello",
                "params": {"role": "native_host", "origin": "chrome-extension://abcdefghijklmnopabcdefghijklmnop/"}})
        .to_string(),
    );
    let _ = stranger.recv();

    // each journaled once, with its reason code, never a message's contents
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let rows = loop {
        let rows = refusals(&run);
        if rows.len() >= 3 || std::time::Instant::now() > deadline {
            break rows;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(rows[0]["reason"], "peer");
    assert_eq!(rows[0]["app"], "com.example.other");
    assert_eq!(rows[1]["reason"], "peer");
    assert!(rows[1].get("app").is_none(), "{rows:?}");
    assert_eq!(rows[2]["reason"], "host");
    assert_eq!(
        rows[2]["origin"],
        "chrome-extension://abcdefghijklmnopabcdefghijklmnop/"
    );
}
