//! FERB's shell (ferb/act/shell): commands run under macOS Seatbelt with a profile FERB builds from its protection
//! lists, in a PTY, with a scrubbed environment, with network only through FERB's egress proxy, owned by a task and
//! stopped with it. Ported from the Python daemon and checked against fixtures the Python code wrote
//! (rust/fixtures/shell, ENG-3).
//!
//! AGENTS.md rule 6: nothing here lets FERB change its own audit records, protection rules, sandbox profile, config,
//! service definition or secrets helper, or bypass its proxy; rule 11: when the sandbox can't run, the shell is
//! unavailable (`Sandbox::available`), never run uncontained.
//!
//! What touches macOS (libproc, `sandbox_check`, `nettop`, `sandbox-exec`) sits behind `cfg(target_os = "macos")`;
//! elsewhere those readings return nothing, so every check fails closed. Seams to other parts: `protection::Protection`
//! (the policy part; implemented over ferb-policy's lists in `policy_lists`), `tools::ResolvePath` (the files part),
//! `local::SocketFacts`, the proxy's `allow`/`resolve` callbacks (the gateway), `manager::ManagerConfig`'s callbacks
//! (the daemon).

pub mod listeners;
pub mod local;
pub mod manager;
#[cfg(feature = "policy")]
pub mod policy_lists;
pub mod protection;
pub mod proxy;
pub mod pty;
pub mod pynet;
pub mod pyos;
pub mod sandbox;
pub mod sockets;
pub mod tools;

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// `n` random bytes from the system (a failure there is fatal: a predictable token is never acceptable).
pub(crate) fn random_bytes(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).expect("the system gave no random bytes");
    b
}

/// `secrets.token_hex(n)`.
pub fn token_hex(n: usize) -> String {
    hex::encode(random_bytes(n))
}

/// Seconds on a monotonic clock (`time.monotonic()`).
pub fn monotonic() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() + 1.0
}

/// Wall-clock seconds (`time.time()`).
pub fn wall_time() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Python's `returncode`: the exit status, or minus the signal that ended it.
pub fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .unwrap_or_else(|| -status.signal().unwrap_or(0))
}

/// The Python OSError subclass an errno raises as.
pub fn py_os_error_class(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(libc::EAGAIN) => "BlockingIOError",
        Some(libc::EPIPE) => "BrokenPipeError",
        Some(libc::ECHILD) => "ChildProcessError",
        Some(libc::ECONNABORTED) => "ConnectionAbortedError",
        Some(libc::ECONNREFUSED) => "ConnectionRefusedError",
        Some(libc::ECONNRESET) => "ConnectionResetError",
        Some(libc::EEXIST) => "FileExistsError",
        Some(libc::ENOENT) => "FileNotFoundError",
        Some(libc::EINTR) => "InterruptedError",
        Some(libc::EISDIR) => "IsADirectoryError",
        Some(libc::ENOTDIR) => "NotADirectoryError",
        Some(libc::EACCES) | Some(libc::EPERM) => "PermissionError",
        Some(libc::ESRCH) => "ProcessLookupError",
        Some(libc::ETIMEDOUT) => "TimeoutError",
        _ => "OSError",
    }
    .to_string()
}

/// `f"{type(e).__name__}: {e}"` for an OSError: `[Errno n] message` (no file name: the callers never pass one).
pub fn py_os_error(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(n) => {
            // SAFETY: strerror returns a static, NUL-terminated message for any errno.
            let msg = unsafe { std::ffi::CStr::from_ptr(libc::strerror(n)) }
                .to_string_lossy()
                .into_owned();
            format!("{}: [Errno {n}] {msg}", py_os_error_class(e))
        }
        None => format!("OSError: {e}"),
    }
}

/// `hmac.compare_digest` on bytes.
pub(crate) fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() != b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).unwrap_or(&0) ^ b.get(i).unwrap_or(&0);
    }
    diff == 0
}

/// The account's home from the user database (`Path.home()` without HOME).
pub(crate) fn pw_home() -> Option<String> {
    // SAFETY: getpwuid returns a pointer into static storage or null; read at once.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        Some(
            std::ffi::CStr::from_ptr((*pw).pw_dir)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// Runs a program FERB names (never a command text), capturing stdout; killed (only it) past `timeout`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn run_capture(argv: &[&str], timeout: Duration) -> Option<(i32, Vec<u8>)> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new(argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let buf = reader.join().ok()?;
    Some((exit_code(status), buf))
}
