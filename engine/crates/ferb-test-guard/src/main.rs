//! The offline suite's guards (TEST-504's Python guards in Rust, T-1109): `engine/gate.sh test` runs every test
//! binary through this program (cargo's target runner), which
//!
//! - runs it under `sandbox-exec` with [`PROFILE`]: a connection beyond this Mac's loopback (a model provider, any
//!   other host), the Keychain, or a read of a file's contents or a write in FERB's real home, the model programs' own
//!   homes, the Trash or a browser's folders ends the test binary at once (the sandbox's SIGKILL), and the run fails
//!   naming the tests that were running;
//! - ends a test that runs past FERB_TEST_TIMEOUT_S (300 s, runaway.py's) and names it;
//! - ends the test binary when its processes together use more than FERB_TEST_MAX_RSS_MB (4096, runaway.py's) of
//!   memory (the larger of resident size and physical footprint, per process, as runaway.py read it) and names the
//!   tests that were running.
//!
//! A binary listed in `nests.txt` runs without the sandbox, with the caps and a HOME of its own (a fresh temporary
//! folder): its tests apply sandboxes of their own, and macOS refuses a sandbox inside a sandbox that restricts
//! anything.
//!
//! A test's start is known from libtest's own output: "test NAME ... " before it runs with one test thread, and
//! "test NAME has been running for over 60 seconds" with several (libtest's warning). Which tests were running when
//! the binary was ended: the unfinished ones libtest had started, in its order (by name, `--test-threads` at a time).

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const TIMEOUT_ENV: &str = "FERB_TEST_TIMEOUT_S";
pub const MAX_RSS_ENV: &str = "FERB_TEST_MAX_RSS_MB";
/// runaway.py's DEFAULT_TIMEOUT_S, DEFAULT_MAX_RSS_MB, MEMORY_EXIT_CODE.
pub const DEFAULT_TIMEOUT_S: f64 = 300.0;
pub const DEFAULT_MAX_RSS_MB: f64 = 4096.0;
pub const MEMORY_EXIT_CODE: i32 = 3;
/// A test that ran past its limit (timeout(1)'s code).
pub const TIMEOUT_EXIT_CODE: i32 = 124;
/// The sandbox ended the binary: a forbidden place or host.
pub const SANDBOX_EXIT_CODE: i32 = 137;
/// libtest's warning for a test that runs long.
const LONG_RUNNING: &str = " has been running for over ";
const WARN_AFTER_S: f64 = 60.0;

/// The sandbox profile: everything a test may do, but the network beyond loopback and FERB's and the owner's real
/// places. Parameters: HOME (the account's home folder).
pub const PROFILE: &str = include_str!("../offline.sb");

/// The test binaries that apply sandboxes of their own (macOS refuses a sandbox inside a sandbox that restricts
/// anything: `sandbox_apply: Operation not permitted`). They run without [`PROFILE`], with the time and memory caps.
pub const NESTS: &str = include_str!("../nests.txt");

/// A test binary's name as cargo builds it: its file name without the `-<hash>`.
pub fn test_name(binary: &Path) -> String {
    let file = binary
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    match file.rsplit_once('-') {
        Some((name, hash)) if hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()) => {
            name.to_string()
        }
        _ => file,
    }
}

/// A test binary's crate and name ("ferb-shell/live"): the crate from cargo's `build/<crate>/<hash>/out/` folder, when
/// the binary sits in one.
pub fn qualified(binary: &Path) -> String {
    let parts: Vec<String> = binary
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let krate = parts
        .windows(4)
        .rev()
        .find(|w| w[0] == "build" && w[3] == "out")
        .map(|w| w[1].clone());
    match krate {
        Some(k) => format!("{k}/{}", test_name(binary)),
        None => test_name(binary),
    }
}

/// Tests with their own time limit (runaway.py's `ferb_timeout(seconds)` marker): `<crate>/<test target>::<test> |
/// seconds | why`, one per line.
pub const SLOW: &str = include_str!("../slow.txt");

/// [`SLOW`]'s limits for this binary: test name -> seconds.
pub fn slow(binary: &str) -> Vec<(String, f64)> {
    SLOW.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut parts = l.split(" | ");
            let (target, test) = parts.next()?.trim().split_once("::")?;
            let seconds = parts.next()?.trim().parse().ok()?;
            (target == binary).then(|| (test.to_string(), seconds))
        })
        .collect()
}

/// The names [`NESTS`] lists.
pub fn nests() -> Vec<String> {
    NESTS
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split(" | ").next())
        .map(|n| n.trim().to_string())
        .collect()
}

fn limit(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .unwrap_or(default)
}

/// The account's home folder (the password database's, whatever HOME says).
fn account_home() -> PathBuf {
    // SAFETY: getpwuid returns a pointer into static storage or null; it is read at once, on this thread only.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_dir.is_null() {
            let dir = std::ffi::CStr::from_ptr((*pw).pw_dir);
            return PathBuf::from(dir.to_string_lossy().into_owned());
        }
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// What libtest said about the tests so far.
#[derive(Default)]
struct Seen {
    finished: BTreeSet<String>,
    /// started and not finished: name -> when it started (as far as is known)
    started: Vec<(String, Instant)>,
    buffer: Vec<u8>,
}

impl Seen {
    fn start(&mut self, name: &str, at: Instant) {
        if !self.finished.contains(name) && !self.started.iter().any(|(n, _)| n == name) {
            self.started.push((name.to_string(), at));
        }
    }

    fn finish(&mut self, name: &str) {
        self.started.retain(|(n, _)| n != name);
        self.finished.insert(name.to_string());
    }

    /// One complete line of libtest's output.
    fn line(&mut self, line: &str, now: Instant) {
        let Some(rest) = line.strip_prefix("test ") else {
            return;
        };
        if let Some((name, _)) = rest.split_once(" ... ") {
            self.finish(name);
        } else if let Some((name, _)) = rest.split_once(LONG_RUNNING) {
            let began = now
                .checked_sub(Duration::from_secs_f64(WARN_AFTER_S))
                .unwrap_or(now);
            self.start(name, began);
        }
    }

    /// Bytes of the binary's output: whole lines, and a line in progress that announces a test's start.
    fn feed(&mut self, bytes: &[u8], now: Instant) {
        self.buffer.extend_from_slice(bytes);
        while let Some(i) = self.buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=i).collect();
            self.line(String::from_utf8_lossy(&line).trim_end(), now);
        }
        let partial = String::from_utf8_lossy(&self.buffer).into_owned();
        if let Some(name) = partial
            .strip_prefix("test ")
            .and_then(|r| r.strip_suffix(" ... "))
        {
            self.start(name, now);
        }
    }

    /// The test that has run longest past its limit (`limit_s`, or its own in `own`; 0: none), with its limit.
    fn overdue(
        &self,
        now: Instant,
        limit_s: f64,
        own: &[(String, f64)],
    ) -> Option<(String, f64, f64)> {
        self.started
            .iter()
            .map(|(n, at)| {
                let limit = own
                    .iter()
                    .find(|(t, _)| t == n)
                    .map_or(limit_s, |(_, s)| *s);
                (n.clone(), now.duration_since(*at).as_secs_f64(), limit)
            })
            .filter(|(_, ran, limit)| *limit > 0.0 && *ran >= *limit)
            .max_by(|a, b| a.1.total_cmp(&b.1))
    }
}

/// The binary's arguments without the ones a `--list` doesn't take.
fn filter_args(args: &[OsString]) -> Vec<OsString> {
    let mut out = Vec::new();
    let mut skip_value = false;
    for a in args {
        if skip_value {
            skip_value = false;
            continue;
        }
        let s = a.to_string_lossy();
        if matches!(
            s.as_ref(),
            "--nocapture" | "--no-capture" | "--show-output" | "-q" | "--quiet"
        ) {
            continue;
        }
        if matches!(
            s.as_ref(),
            "--test-threads" | "--format" | "--color" | "--logfile"
        ) {
            skip_value = true;
            continue;
        }
        if s.starts_with("--test-threads=")
            || s.starts_with("--format=")
            || s.starts_with("--color=")
            || s.starts_with("--logfile=")
        {
            continue;
        }
        out.push(a.clone());
    }
    out
}

/// How many tests libtest runs at once: `--test-threads`, RUST_TEST_THREADS, else the CPUs.
fn concurrency(args: &[OsString]) -> usize {
    let mut it = args.iter().map(|a| a.to_string_lossy().into_owned());
    while let Some(a) = it.next() {
        if a == "--test-threads" {
            if let Some(n) = it.next().and_then(|v| v.parse().ok()) {
                return n;
            }
        } else if let Some(v) = a.strip_prefix("--test-threads=") {
            if let Ok(n) = v.parse() {
                return n;
            }
        }
    }
    std::env::var("RUST_TEST_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
}

/// The tests that were running: the ones known to have started, else the first `threads` unfinished tests in
/// libtest's order (it starts them by name, that many at a time).
fn running(binary: &Path, args: &[OsString], seen: &Seen, home: &Path) -> Vec<String> {
    let mut out: Vec<String> = seen.started.iter().map(|(n, _)| n.clone()).collect();
    let listed = Command::new("/usr/bin/sandbox-exec")
        .arg("-D")
        .arg(format!("HOME={}", home.display()))
        .arg("-p")
        .arg(PROFILE)
        .arg(binary)
        .args(filter_args(args))
        .args(["--list", "--format", "terse"])
        .stderr(Stdio::null())
        .output();
    if let Ok(o) = listed {
        let mut names: Vec<String> = String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|l| l.strip_suffix(": test").map(str::to_string))
            .filter(|n| !seen.finished.contains(n))
            .collect();
        names.sort();
        for n in names.into_iter().take(concurrency(args)) {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out
}

/// The processes of the test binary's process group (the binary and what its tests started).
fn group(pgid: i32) -> Vec<i32> {
    let mut pids = vec![0i32; 4096];
    // SAFETY: the buffer is ours and its size in bytes is given; the call writes at most that many.
    let n = unsafe {
        libc::proc_listpgrppids(
            pgid,
            pids.as_mut_ptr().cast(),
            (pids.len() * std::mem::size_of::<i32>()) as i32,
        )
    };
    if n <= 0 {
        return vec![pgid];
    }
    // the count of pids written (libproc's `proc_listpgrppids` returns pids, not bytes)
    pids.truncate((n as usize).min(pids.len()));
    pids.retain(|p| *p > 0);
    pids
}

/// One process's memory: the larger of its resident size and physical footprint (runaway.py `_darwin_memory`).
fn memory(pid: i32) -> u64 {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v0>::zeroed();
    // SAFETY: proc_pid_rusage fills a rusage_info_v0 for flavor RUSAGE_INFO_V0; the pointer is to one we own.
    let rc = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V0, info.as_mut_ptr().cast()) };
    if rc != 0 {
        return 0;
    }
    // SAFETY: filled by the call above (it returned 0).
    let info = unsafe { info.assume_init() };
    info.ri_resident_size.max(info.ri_phys_footprint)
}

fn say(text: &str) {
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "\n\nFERB test guard: {text}");
}

fn main() {
    let mut argv = std::env::args_os().skip(1);
    let Some(binary) = argv.next().map(PathBuf::from) else {
        eprintln!("usage: ferb-test-guard <test binary> [its arguments]");
        std::process::exit(2);
    };
    let args: Vec<OsString> = argv.collect();
    let timeout_s = limit(TIMEOUT_ENV, DEFAULT_TIMEOUT_S);
    let max_bytes = (limit(MAX_RSS_ENV, DEFAULT_MAX_RSS_MB) * 1024.0 * 1024.0) as u64;
    let home = account_home();
    // FERB_TEST_GUARD_SANDBOX: sandboxed even when listed (the guard's own proofs on their own binary); it can only add
    // the sandbox, never take it away
    let sandboxed = std::env::var_os("FERB_TEST_GUARD_SANDBOX").is_some()
        || !nests().contains(&qualified(&binary));
    // a binary without the sandbox gets a HOME of its own (removed after it): what it reaches through HOME (~/.ferb,
    // the Trash, the browsers' folders, the Keychain's files) is a fresh empty folder, never the owner's
    let own_home = std::env::temp_dir().join(format!("ferb-test-home-{}", std::process::id()));
    if !sandboxed {
        let _ = std::fs::create_dir_all(&own_home);
    }
    let mut command = if sandboxed {
        let mut c = Command::new("/usr/bin/sandbox-exec");
        c.arg("-D")
            .arg(format!("HOME={}", home.display()))
            .arg("-p")
            .arg(PROFILE)
            .arg(&binary);
        c
    } else {
        eprintln!(
            "FERB test guard: {} runs without the offline sandbox, in a home of its own: its tests apply sandboxes of \
             their own (nests.txt)",
            qualified(&binary)
        );
        let mut c = Command::new(&binary);
        c.env("HOME", &own_home);
        c
    };
    let mut child = match command
        .args(&args)
        .stdout(Stdio::piped())
        .process_group(0)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("FERB test guard: couldn't start {}: {e}", binary.display());
            std::process::exit(2);
        }
    };
    let pgid = child.id() as i32;
    let own_limits = slow(&qualified(&binary));
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut out = child.stdout.take().expect("piped");
    let reader = {
        let seen = seen.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            let mut stdout = std::io::stdout();
            loop {
                match out.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        let _ = stdout.write_all(&buf[..n]);
                        let _ = stdout.flush();
                        seen.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .feed(&buf[..n], Instant::now());
                    }
                }
            }
        })
    };
    let mut ended_by_us: Option<i32> = None;
    // FERB_TEST_GUARD_PEAK: the binary's largest memory, said at its end (to see the cap's headroom)
    let report_peak = std::env::var_os("FERB_TEST_GUARD_PEAK").is_some();
    let mut peak = 0u64;
    // the binary's end is seen at once (a waiting thread), the checks every quarter second meanwhile
    let (ended, end) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = ended.send(child.wait());
    });
    let status = loop {
        match end.recv_timeout(Duration::from_millis(250)) {
            Ok(Ok(s)) => break s,
            Ok(Err(e)) => {
                eprintln!("FERB test guard: lost the test binary: {e}");
                std::process::exit(2);
            }
            Err(_) => {}
        }
        let now = Instant::now();
        let overdue = if timeout_s > 0.0 {
            seen.lock()
                .unwrap_or_else(|e| e.into_inner())
                .overdue(now, timeout_s, &own_limits)
        } else {
            None
        };
        if let Some((name, ran, limit)) = overdue {
            let whose = if limit == timeout_s {
                TIMEOUT_ENV
            } else {
                "its own in slow.txt"
            };
            say(&format!(
                "{name} ran for {ran:.0} s, over the {limit:.0} s limit ({whose}); the test binary {} was ended",
                binary.display()
            ));
            ended_by_us = Some(TIMEOUT_EXIT_CODE);
        } else if max_bytes > 0 {
            let used: u64 = group(pgid).into_iter().map(memory).sum();
            peak = peak.max(used);
            if used > max_bytes {
                let s = seen.lock().unwrap_or_else(|e| e.into_inner());
                say(&format!(
                    "{} MB is over the {} MB limit ({MAX_RSS_ENV}) in {}; the tests running then: {}",
                    used >> 20,
                    max_bytes >> 20,
                    binary.display(),
                    running(&binary, &args, &s, &home).join(", ")
                ));
                ended_by_us = Some(MEMORY_EXIT_CODE);
            }
        }
        if ended_by_us.is_some() {
            // SAFETY: killpg takes a process group id and a signal, no pointers; the group is the binary's own.
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
            let _ = end.recv();
            break std::process::ExitStatus::from_raw(0);
        }
    };
    let _ = reader.join();
    if !sandboxed {
        let _ = std::fs::remove_dir_all(&own_home);
    }
    if report_peak {
        eprintln!(
            "FERB test guard: peak {} MB in {}",
            peak >> 20,
            test_name(&binary)
        );
    }
    if let Some(code) = ended_by_us {
        std::process::exit(code);
    }
    if sandboxed && status.signal() == Some(libc::SIGKILL) {
        let s = seen.lock().unwrap_or_else(|e| e.into_inner());
        say(&format!(
            "the sandbox ended {}: a test reached the network beyond this Mac's loopback, or FERB's real home, the \
             Keychain, the Trash, a model program's home or a browser's folders (TEST-504, TEST-508); the tests running \
             then: {}",
            binary.display(),
            running(&binary, &args, &s, &home).join(", ")
        ));
        std::process::exit(SANDBOX_EXIT_CODE);
    }
    // Ended by a signal (a crash: SIGSEGV, SIGBUS, SIGABRT, or "Rust cannot catch foreign exceptions"): libtest prints
    // nothing then, so the gate showed a failed binary with no message; it is named here, with the tests running then.
    if let Some(signal) = status.signal() {
        let s = seen.lock().unwrap_or_else(|e| e.into_inner());
        say(&format!(
            "{} ended by signal {signal} ({}); the tests running then: {}",
            binary.display(),
            signal_name(signal),
            running(&binary, &args, &s, &home).join(", ")
        ));
        std::process::exit(128 + signal);
    }
    std::process::exit(status.code().unwrap_or(1));
}

/// The usual name of a signal that ends a process.
fn signal_name(signal: i32) -> &'static str {
    match signal {
        libc::SIGABRT => "SIGABRT: an abort, a failed check or an uncaught exception",
        libc::SIGSEGV => "SIGSEGV: a bad memory access",
        libc::SIGBUS => "SIGBUS: a bad memory access",
        libc::SIGILL => "SIGILL",
        libc::SIGTRAP => "SIGTRAP: a Swift or Rust trap",
        libc::SIGTERM => "SIGTERM",
        libc::SIGKILL => "SIGKILL",
        _ => "another signal",
    }
}
