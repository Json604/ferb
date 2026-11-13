//! A stand-in for `ferb-secrets` in this crate's tests: it speaks the helper's framing (one
//! request on stdin, one reply on stdout, an exit code) and touches no Keychain. Never shipped:
//! an example is built only by `cargo test` / `cargo build --examples`.
//!
//! It runs from `<home>/bin/ferb-secrets` and keeps everything beside itself:
//! - `calls.log`: one line per run: its arguments, the names of its environment variables and its
//!   stdin, each hex, so a test can hold the caller to Python's exact invocation;
//! - `replies.txt`: the scripted replies, one `<exit code> <stdout hex>` per line, taken in order
//!   (`replies.pos` counts). A negative code ends the run by a signal. No reply left: exit 99.

use std::{
    env, fs,
    io::{Read, Write},
    process,
};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

fn main() {
    let beside = env::current_exe().unwrap().parent().unwrap().to_owned();
    let mut stdin = Vec::new();
    std::io::stdin().read_to_end(&mut stdin).unwrap();
    let args: Vec<String> = env::args_os().map(|a| hex(a.as_encoded_bytes())).collect();
    let mut names: Vec<String> = env::vars_os()
        .map(|(name, _)| hex(name.as_encoded_bytes()))
        .collect();
    names.sort();
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(beside.join("calls.log"))
        .unwrap();
    writeln!(
        log,
        "{}\t{}\t{}",
        args.join(","),
        names.join(","),
        hex(&stdin)
    )
    .unwrap();
    drop(log);

    let replies = fs::read_to_string(beside.join("replies.txt")).unwrap_or_default();
    let pos: usize = fs::read_to_string(beside.join("replies.pos"))
        .ok()
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or(0);
    let Some(line) = replies.lines().nth(pos) else {
        process::exit(99)
    };
    fs::write(beside.join("replies.pos"), (pos + 1).to_string()).unwrap();
    let (code, out) = line.split_once(' ').unwrap();
    let code: i32 = code.parse().unwrap();
    let mut stdout = std::io::stdout();
    stdout.write_all(&unhex(out)).unwrap();
    stdout.flush().unwrap();
    if code < 0 {
        process::abort();
    }
    process::exit(code);
}
