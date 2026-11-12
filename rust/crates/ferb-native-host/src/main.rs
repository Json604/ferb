//! The program a browser starts for the FERB extension (Chromium native messaging). A port of
//! `ferb/native_host/main.py`: it relays length-framed messages on stdin/stdout to and from the daemon's Unix
//! socket, checks frame sizes and JSON syntax only, and never inspects, logs or routes a message. The daemon
//! authenticates the origin and every signed message.
//!
//! The browser must start this binary itself (the manifest's `path`, or a launcher that `exec`s it in place):
//! the daemon takes the relay's parent process to be the browser (`ferb/act/browser/ext_bridge.py:249-260`, HAND-223).
//!
//! Forwarding. Python parses each message and writes it again with `json.dumps`. This relay checks the message
//! with Python's grammar (json.rs) and forwards the original bytes: nothing is decoded, so a signed body
//! (`body_json`, `body`; HAND-222) and a lone surrogate escape cannot be altered. The two framings need only this:
//! - browser to daemon: the document without the whitespace around it, raw newlines (legal only between tokens)
//!   turned into spaces, then `\n`;
//! - daemon to browser: the line without the whitespace around it, behind its 4-byte little-endian length.
//!
//! What still differs from the Python relay:
//! 1. Bytes: Python's output is always `json.dumps` form (`, ` and `: `, ASCII with `\uXXXX`, numbers rewritten,
//!    a repeated key written once). Here they are the sender's bytes. They are the same when the sender wrote
//!    `json.dumps` form, as the Python daemon does; otherwise they parse to the same value
//!    (`rust/fixtures/native-host/make.py` checks each case).
//! 2. A browser message never grows: Python's rewrite can push one past the daemon's 1 MiB line limit
//!    (`ferb/daemon/server.py:201,229-232`).
//! 3. A daemon line that is not JSON ends this relay with exit code 1. In Python the reading thread dies and the
//!    process stays, forwarding nothing more to the browser until the browser closes it (main.py:53-57).
//! 4. Refused by Python for reasons outside the grammar, forwarded here: nesting deeper than its recursion limit,
//!    integers of more than 4300 digits. Accepted by Python, refused here (exit 1): UTF-16 and UTF-32 input.
//! 5. A hello reply that is an array or a string never passes here; in Python one that contains `"result"` does
//!    (main.py:49).
//! 6. Nothing is written to stderr except one fixed line per failure; Python prints a traceback.
//!
//! To try it against a real daemon (not run by any test). From a shell the daemon answers the hello and then
//! closes, because the relay's parent is not a browser (HAND-223):
//!
//! ```sh
//! cd rust && cargo build --release -p ferb-native-host
//! FERB_HOME=/path/to/a/test/home target/release/ferb-native-host chrome-extension://<a known extension id>/ </dev/null; echo $?
//! ```
//!
//! With a browser: in a throwaway `--user-data-dir`, write `NativeMessagingHosts/com.ferb.native_host.json` as
//! `tests/chromelab.py:163-184` does, with `"path"` set to `rust/target/release/ferb-native-host`, and start the
//! browser with `FERB_HOME` set to the test daemon's home.

mod json;

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process;

use json::Kind;

/// main.py:17; the daemon's own limit is the same number (`ferb/daemon/server.py:26`).
const MAX_MESSAGE: usize = 1024 * 1024;

fn main() {
    process::exit(run());
}

/// `ferb/paths.py:8-14`: `$FERB_HOME` (tests, a second FERB side by side) or `~/.ferb`, then `run/ferb.sock`.
/// Not restricted in a shipping build, as in Python: the relay holds no secret, and a process that can set the
/// browser's environment already runs as the owner (HAND-223).
fn socket_path() -> PathBuf {
    let home = std::env::var_os("FERB_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::home_dir().unwrap_or_default().join(".ferb"));
    home.join("run").join("ferb.sock")
}

fn run() -> i32 {
    // The browser passes the caller's origin (chrome-extension://<id>/) as the first argument (main.py:67-68).
    let origin: OsString = std::env::args_os().nth(1).unwrap_or_default();
    let Ok(sock) = UnixStream::connect(socket_path()) else {
        return 1; // daemon not running: the extension retries with backoff (main.py:44)
    };
    let Ok(read_half) = sock.try_clone() else {
        return fail("cannot read the daemon socket");
    };
    let mut rfile = BufReader::new(read_half);

    // main.py:46-47, byte for byte what json.dumps writes.
    let mut hello = br#"{"jsonrpc": "2.0", "id": 1, "method": "auth.hello", "params": {"role": "native_host", "origin": "#.to_vec();
    push_json_string(&mut hello, origin.as_bytes());
    hello.extend_from_slice(b"}}\n");
    let mut reply = Vec::new();
    if (&sock).write_all(&hello).is_err() || rfile.read_until(b'\n', &mut reply).is_err() {
        return fail("the daemon closed during the hello");
    }
    // main.py:48-50: no line reads as `{}`; anything without a `result` is a refusal.
    if !reply.is_empty() {
        match json::check(&reply).map(|c| c.kind) {
            Some(Kind::Object { has_result: true }) => {}
            Some(Kind::Object { has_result: false } | Kind::Array | Kind::String) => return 2,
            Some(Kind::Scalar) | None => return fail("the daemon's hello reply is not JSON"),
        }
    } else {
        return 2; // refused (e.g. an extension id FERB doesn't know)
    }

    // main.py:52-59: the daemon going away ends the whole process, so the extension reconnects cleanly.
    std::thread::spawn(move || process::exit(daemon_to_browser(rfile)));
    browser_to_daemon(&sock)
}

/// main.py:20-31 and 60-62. Returns the exit code.
fn browser_to_daemon(mut sock: &UnixStream) -> i32 {
    let mut stdin = io::stdin().lock();
    let mut body = Vec::new();
    loop {
        let mut head = [0u8; 4];
        if stdin.read_exact(&mut head).is_err() {
            return 0; // the browser closed the port
        }
        let n = u32::from_le_bytes(head) as usize;
        if n > MAX_MESSAGE {
            return 0; // oversized: end the connection rather than buffer it (main.py:25-26)
        }
        body.resize(n, 0);
        if stdin.read_exact(&mut body).is_err() {
            return 0;
        }
        let Some(doc) = json::check(&body) else {
            return fail("a browser message is not JSON"); // json.loads raises: exit code 1
        };
        if !matches!(doc.kind, Kind::Object { .. }) {
            return 0; // main.py:31
        }
        // One line for the daemon: after the check, a newline byte can only be whitespace between tokens.
        body.truncate(doc.end);
        for byte in &mut body[doc.start..] {
            if *byte == b'\n' {
                *byte = b' ';
            }
        }
        body.push(b'\n');
        if sock.write_all(&body[doc.start..]).is_err() {
            return fail("the daemon socket closed"); // sendall raises: exit code 1
        }
    }
}

/// main.py:52-57. Returns the exit code.
fn daemon_to_browser(rfile: BufReader<UnixStream>) -> i32 {
    let mut stdout = io::stdout().lock();
    let mut rfile = rfile;
    let mut line = Vec::new();
    loop {
        line.clear();
        // One byte past the cap is enough to know the line is too long; never buffer more.
        let mut capped = (&mut rfile).take(MAX_MESSAGE as u64 + 1);
        match capped.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return 0, // daemon gone
            Ok(_) => {}
        }
        if line.len() > MAX_MESSAGE {
            return 0; // main.py:54-55 (the newline counts)
        }
        let Some(doc) = json::check(&line) else {
            return fail("a daemon line is not JSON"); // difference 3 in the module text
        };
        let body = &line[doc.start..doc.end];
        let mut frame = Vec::with_capacity(4 + body.len());
        frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
        frame.extend_from_slice(body);
        if stdout.write_all(&frame).is_err() || stdout.flush().is_err() {
            return 0; // the browser is gone
        }
    }
}

/// A JSON string as `json.dumps` writes it (ASCII only; `Lib/json/encoder.py` ESCAPE_ASCII), from a program
/// argument. Python reads arguments with `surrogateescape`: a byte that is not UTF-8 becomes U+DC00 + the byte.
fn push_json_string(out: &mut Vec<u8>, raw: &[u8]) {
    out.push(b'"');
    for chunk in raw.utf8_chunks() {
        for ch in chunk.valid().chars() {
            match ch {
                '"' => out.extend_from_slice(b"\\\""),
                '\\' => out.extend_from_slice(b"\\\\"),
                '\n' => out.extend_from_slice(b"\\n"),
                '\r' => out.extend_from_slice(b"\\r"),
                '\t' => out.extend_from_slice(b"\\t"),
                '\u{8}' => out.extend_from_slice(b"\\b"),
                '\u{c}' => out.extend_from_slice(b"\\f"),
                ' '..='~' => out.push(ch as u8),
                _ => {
                    for unit in ch.encode_utf16(&mut [0u16; 2]) {
                        out.extend_from_slice(format!("\\u{unit:04x}").as_bytes());
                    }
                }
            }
        }
        for byte in chunk.invalid() {
            out.extend_from_slice(format!("\\udc{byte:02x}").as_bytes());
        }
    }
    out.push(b'"');
}

/// Exit code 1 with a fixed reason; message contents are never written anywhere.
fn fail(why: &str) -> i32 {
    eprintln!("ferb-native-host: {why}");
    1
}
