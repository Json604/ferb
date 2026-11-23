//! Each guard of the offline suite trips (TEST-504, T-1109): a deliberately bad test, ignored in every normal run and
//! doing nothing unless FERB_TEST_GUARD_PROOF is set, is run through `ferb-test-guard` on this very test binary, and
//! the run must fail naming it. The bad tests only read: a connection to TEST-NET-1 (192.0.2.1, never routed), the
//! listing of FERB's home and of the Trash, Chrome's Local State, a Keychain lookup of an item that doesn't exist.

use std::process::Command;
use std::time::{Duration, Instant};

const GUARD: &str = env!("CARGO_BIN_EXE_ferb-test-guard");
const PROOF: &str = "FERB_TEST_GUARD_PROOF";

fn proof() -> bool {
    std::env::var_os(PROOF).is_some()
}

/// The account's own home (the password database's): this binary may run with a HOME of its own (nests.txt).
fn home() -> std::path::PathBuf {
    // SAFETY: getpwuid returns a pointer into static storage or null; it is read at once on this thread.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_dir.is_null() {
            return std::ffi::CStr::from_ptr((*pw).pw_dir)
                .to_string_lossy()
                .into_owned()
                .into();
        }
    }
    std::env::var_os("HOME").map(Into::into).unwrap_or_default()
}

/// Runs one bad test of this binary through the guard: (exit code, stderr, seconds).
fn through_guard(test: &str, env: &[(&str, &str)]) -> (Option<i32>, String, f64) {
    let me = std::env::current_exe().unwrap();
    let began = Instant::now();
    let out = Command::new(GUARD)
        .arg(&me)
        .args(["--ignored", "--exact", test, "--test-threads", "1"])
        .env(PROOF, "1")
        .env("FERB_TEST_GUARD_SANDBOX", "1")
        .envs(env.iter().copied())
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        began.elapsed().as_secs_f64(),
    )
}

#[test]
#[ignore = "a bad test: run only through the guard by its proof"]
fn bad_network() {
    if proof() {
        let _ = std::net::TcpStream::connect_timeout(
            &"192.0.2.1:9".parse().unwrap(),
            Duration::from_secs(3),
        );
    }
}

#[test]
#[ignore = "a bad test: run only through the guard by its proof"]
fn bad_ferb_home() {
    if proof() {
        let _ = std::fs::read_dir(home().join(".ferb")).map(|d| d.count());
    }
}

#[test]
#[ignore = "a bad test: run only through the guard by its proof"]
fn bad_trash() {
    if proof() {
        let _ = std::fs::read_dir(home().join(".Trash")).map(|d| d.count());
    }
}

#[test]
#[ignore = "a bad test: run only through the guard by its proof"]
fn bad_browser_folder() {
    if proof() {
        let _ = std::fs::read(home().join("Library/Application Support/Google/Chrome/Local State"));
    }
}

/// A program the test starts that reaches the Keychain is ended by the sandbox (it can't fail the test binary
/// itself: the test sees its child end, as here).
#[test]
#[ignore = "a bad test: run only through the guard by its proof"]
fn bad_keychain() {
    if proof() {
        use std::os::unix::process::ExitStatusExt;
        let status = Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                "ferb-test-guard-no-such-item",
            ])
            .output()
            .unwrap()
            .status;
        println!("the Keychain lookup ended by signal {:?}", status.signal());
    }
}

#[test]
#[ignore = "a bad test: run only through the guard by its proof"]
fn bad_runaway_time() {
    if proof() {
        std::thread::sleep(Duration::from_secs(60));
    }
}

#[test]
#[ignore = "a bad test: run only through the guard by its proof"]
fn bad_runaway_memory() {
    if proof() {
        // 512 MB, every page touched, held until the guard ends the binary
        let mut held: Vec<Vec<u8>> = Vec::new();
        for _ in 0..8 {
            held.push(vec![1u8; 64 << 20]);
        }
        std::thread::sleep(Duration::from_secs(30));
        assert!(!held.is_empty());
    }
}

#[test]
#[ignore = "a bad test: run only through the guard by its proof"]
fn bad_crash() {
    if proof() {
        // SIGTERM: a death by signal that leaves no crash report behind
        // SAFETY: raise takes a signal number, no pointers.
        unsafe { libc::raise(libc::SIGTERM) };
        std::thread::sleep(Duration::from_secs(5));
    }
}

/// What a test may do: bind and reach this Mac's loopback (the daemon's sockets, the fake servers).
#[test]
fn loopback_is_allowed() {
    for addr in ["127.0.0.1:0", "[::1]:0"] {
        let l = std::net::TcpListener::bind(addr).unwrap();
        let s = std::net::TcpStream::connect(l.local_addr().unwrap()).unwrap();
        drop(s);
    }
}

// decision: TEST-504
// decision: TEST-511
#[test]
fn the_network_beyond_loopback_ends_the_run_naming_the_test() {
    let (code, err, _) = through_guard("bad_network", &[]);
    assert_eq!(code, Some(137), "{err}");
    assert!(err.contains("the sandbox ended"), "{err}");
    assert!(err.contains("bad_network"), "{err}");
}

// decision: TEST-504
// decision: TEST-511
// decision: TEST-508
#[test]
fn ferbs_home_the_trash_a_browser_and_the_keychain_end_the_run() {
    for test in ["bad_ferb_home", "bad_trash", "bad_browser_folder"] {
        let (code, err, _) = through_guard(test, &[]);
        assert_eq!(code, Some(137), "{test}: {err}");
        assert!(err.contains(test), "{test} named: {err}");
    }
    let me = std::env::current_exe().unwrap();
    let out = Command::new(GUARD)
        .arg(&me)
        .args(["--ignored", "--exact", "bad_keychain", "--nocapture"])
        .env(PROOF, "1")
        .env("FERB_TEST_GUARD_SANDBOX", "1")
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains("the Keychain lookup ended by signal Some(9)"),
        "{said}"
    );
}

// decision: TEST-504
// decision: TEST-511
#[test]
fn a_test_past_its_time_is_ended_and_named() {
    let (code, err, took) = through_guard("bad_runaway_time", &[("FERB_TEST_TIMEOUT_S", "1")]);
    assert_eq!(code, Some(124), "{err}");
    assert!(
        err.contains("bad_runaway_time ran for") && err.contains("FERB_TEST_TIMEOUT_S"),
        "{err}"
    );
    assert!(took < 30.0, "ended at its limit, not at its end ({took} s)");
}

// decision: TEST-504
// decision: TEST-511
#[test]
fn a_test_past_the_memory_cap_is_ended_and_named() {
    let (code, err, took) = through_guard("bad_runaway_memory", &[("FERB_TEST_MAX_RSS_MB", "128")]);
    assert_eq!(code, Some(3), "{err}");
    assert!(err.contains("FERB_TEST_MAX_RSS_MB"), "{err}");
    assert!(err.contains("bad_runaway_memory"), "{err}");
    assert!(took < 25.0, "ended at the cap ({took} s)");
}

/// FERB.app builds before HAND-650 ask the kernel about every process holding a listening port whether its sandbox
/// lets it list FERB's run folder and its workspace (`old_question`), and the kernel runs a sandbox rule's action on
/// such a question as if the process had tried. That question never ends a guarded binary (nor does today's check,
/// ferb-shell's `listeners::contained`, which asks no sandbox); reading a file in those folders still does, and listing
/// them stays refused. In a home of the test's own (the profile's HOME).
/// The containment question FERB.app builds before HAND-650 ask (ferb-shell's old `listeners::contained`): whether the
/// process's sandbox denies listing `run` and allows listing `ws`.
fn old_question(pid: i32, run: &std::path::Path, ws: &std::path::Path) -> bool {
    use std::ffi::{c_char, CString};
    use std::os::unix::ffi::OsStrExt;
    extern "C" {
        fn sandbox_check(pid: libc::pid_t, operation: *const c_char, type_: i32, ...) -> i32;
    }
    let (run, ws) = (
        CString::new(run.as_os_str().as_bytes()).unwrap(),
        CString::new(ws.as_os_str().as_bytes()).unwrap(),
    );
    let (op, flags) = (c"file-read-data", 1 | 0x4000_0000); // SANDBOX_FILTER_PATH | SANDBOX_CHECK_NO_REPORT
                                                            // SAFETY: NUL-terminated strings that outlive the call.
    unsafe {
        sandbox_check(pid, std::ptr::null(), 0) == 1
            && sandbox_check(pid, op.as_ptr(), flags, run.as_ptr()) == 1
            && sandbox_check(pid, op.as_ptr(), flags, ws.as_ptr()) == 0
    }
}

// decision: TEST-511
// decision: TEST-514
#[test]
fn ferbs_containment_question_never_ends_a_guarded_binary() {
    use std::io::BufRead;
    use std::os::unix::process::ExitStatusExt;
    let home = std::env::temp_dir().join(format!(
        "ferb-test-guard-probe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(home.join(".ferb/run")).unwrap();
    std::fs::create_dir_all(home.join(".ferb/workspace")).unwrap();
    // the sandbox matches resolved paths (the temporary folder is behind /var -> /private/var)
    let home = home.canonicalize().unwrap();
    let ferb = home.join(".ferb");
    std::fs::write(ferb.join("run/x"), "x").unwrap();
    let guarded = |argv: &[&str]| {
        let mut c = Command::new("/usr/bin/sandbox-exec");
        c.arg("-D")
            .arg(format!("HOME={}", home.display()))
            .arg("-p")
            .arg(include_str!("../offline.sb"))
            .args(argv);
        c
    };
    // a guarded process, sandboxed once it says so
    let mut child = guarded(&["/bin/sh", "-c", "echo ready; exec /bin/sleep 30"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line.trim(), "ready");
    let pid = child.id() as i32;
    assert!(
        !ferb_shell::listeners::contained(pid),
        "the guard's sandbox is not FERB's containment"
    );
    old_question(pid, &ferb.join("run"), &ferb.join("workspace"));
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        child.try_wait().unwrap().is_none(),
        "FERB's question ended the guarded process"
    );
    child.kill().unwrap();
    let _ = child.wait();
    // a file in FERB's run folder still ends a guarded program; listing the folder is refused without ending it
    let read = guarded(&["/bin/cat", &ferb.join("run/x").to_string_lossy()])
        .output()
        .unwrap();
    assert_eq!(read.status.signal(), Some(9), "{read:?}");
    let list = guarded(&["/bin/ls", &ferb.join("run").to_string_lossy()])
        .output()
        .unwrap();
    assert_eq!(list.status.signal(), None, "{list:?}");
    assert!(!list.status.success(), "{list:?}");
    assert!(list.stdout.is_empty(), "{list:?}");
    std::fs::remove_dir_all(&home).unwrap();
}

/// A test binary that dies by a signal (a crash) is named with the signal and the test running then: libtest itself
/// prints nothing, and the gate once showed ferb-platform-macos's unit tests failing with no message at all.
// decision: TEST-504
#[test]
fn a_binary_that_dies_by_a_signal_is_named_with_its_test() {
    let (code, err, _) = through_guard("bad_crash", &[]);
    assert_eq!(code, Some(128 + libc::SIGTERM), "{err}");
    assert!(
        err.contains("ended by signal 15 (SIGTERM)") && err.contains("bad_crash"),
        "{err}"
    );
}

/// A good test passes through the guard unchanged, its output passed on.
#[test]
fn a_good_test_passes_through() {
    let me = std::env::current_exe().unwrap();
    let out = Command::new(GUARD)
        .arg(&me)
        .args(["--exact", "loopback_is_allowed"])
        .env("FERB_TEST_GUARD_SANDBOX", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("test loopback_is_allowed ... ok"));
}

/// The sandbox covers every browser folder FERB knows (ferb-browser's BROWSERS).
#[test]
fn every_browser_folder_is_covered() {
    let profile = include_str!("../offline.sb");
    for b in ferb_browser::profiles::BROWSERS {
        let top = b.data.split('/').next().unwrap();
        assert!(
            profile.contains(&format!("/Library/Application Support/{top}\"")),
            "{} ({})",
            b.label,
            b.data
        );
    }
}

/// Every test binary nests.txt runs without the sandbox is a test target of the workspace (`<crate>/tests/<name>.rs`)
/// that reaches sandbox-exec.
#[test]
fn every_unsandboxed_binary_is_a_test_target() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let text = include_str!("../nests.txt");
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, why) = line.split_once(" | ").expect("crate/name | why");
        assert!(!why.trim().is_empty(), "{line}");
        let (krate, test) = name.trim().split_once('/').expect("crate/name");
        let text =
            std::fs::read_to_string(crates.join(krate).join("tests").join(format!("{test}.rs")))
                .unwrap_or_else(|_| panic!("{name}: no such test target"));
        // it reaches sandbox-exec: a sandbox of its own, or the daemon's shell (shell.run, wait.for's scripts)
        assert!(
            ["sandbox", "shell.run", "wait.for"]
                .iter()
                .any(|w| text.contains(w)),
            "{name}: nothing in it reaches sandbox-exec; it belongs in the sandbox"
        );
    }
}

/// Every test slow.txt gives its own limit exists (`fn <test>(` in `<crate>/tests/<target>.rs`) and has seconds.
#[test]
fn every_test_with_its_own_limit_exists() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for line in include_str!("../slow.txt").lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split(" | ").collect();
        assert_eq!(parts.len(), 3, "target::test | seconds | why: {line}");
        let (target, test) = parts[0].split_once("::").expect("target::test");
        let (krate, file) = target.split_once('/').expect("crate/target");
        let seconds: f64 = parts[1].parse().expect("seconds");
        assert!(seconds > 0.0);
        let text =
            std::fs::read_to_string(crates.join(krate).join("tests").join(format!("{file}.rs")))
                .unwrap_or_else(|_| panic!("{target}: no such test target"));
        assert!(
            text.contains(&format!("fn {test}(")),
            "{line}: no such test"
        );
    }
}

#[test]
#[ignore = "a probe: run only through the guard by its proof"]
fn home_probe() {
    println!("HOME={}", std::env::var("HOME").unwrap_or_default());
}

/// A binary nests.txt lists (this one) runs without the sandbox but in a HOME of its own, a fresh temporary folder
/// removed after it: what it reaches through HOME is never the owner's.
// decision: TEST-511
#[test]
fn a_binary_without_the_sandbox_gets_a_home_of_its_own() {
    let me = std::env::current_exe().unwrap();
    let out = Command::new(GUARD)
        .arg(&me)
        .args(["--ignored", "--exact", "home_probe", "--nocapture"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let said = String::from_utf8_lossy(&out.stdout);
    let home = said
        .lines()
        .find_map(|l| l.strip_prefix("HOME="))
        .expect("the probe ran")
        .to_string();
    assert!(home.contains("ferb-test-home-"), "{home}");
    assert_ne!(std::path::Path::new(&home), self::home());
    assert!(
        !std::path::Path::new(&home).exists(),
        "removed after the binary"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("in a home of its own"));
}
