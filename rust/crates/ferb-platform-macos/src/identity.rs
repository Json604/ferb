//! The Mac's part of FERB's identity storage: the launcher of the pinned, signed secret helper
//! (`ferb-secrets`, which keeps values in the login Keychain), and where Chrome keeps its profiles. The
//! protocol, the refusals' meaning and the owner-secrets rules are `ferb-identity`'s; this is
//! `ferb/identity/helper.py:29-47` only.
//!
//! - The helper is `<home>/bin/ferb-secrets`, and before every use `/usr/bin/codesign --verify
//!   --strict -R=<pin>` must accept it, where the pin is the requirement text setup wrote to
//!   `<home>/bin/ferb-secrets.requirement` (`ferb/device/install.py:849-923`: a certificate, or
//!   the exact code hash). No pin, no file or a refused signature means no run; there is no
//!   unsigned fallback and no other way to choose the program (`helper.py:29-38`).
//! - A value travels only on the child's stdin and stdout pipes: never in argv, never in the
//!   environment. stderr is discarded. The child gets a scrubbed environment (see [`PASSED_ENV`]).
//! - The program that runs is held to the pin too, not only the file that was checked. Fixed
//!   against Python (ENG-6, `helper.py:29-43`): Python checks the file with `codesign`, then starts
//!   it by its path, so a file swapped in between ran and was handed the request. macOS cannot
//!   start a program from an open file (it has no `fexecve`), so checking an open file and then
//!   running it is not possible; what it can do is check the program the kernel loaded. The helper
//!   is started suspended (`POSIX_SPAWN_START_SUSPENDED`): its program is loaded and its code
//!   signature attached, but it runs no instruction. Its running code is then checked against the
//!   pin by the Security framework (`SecCodeCheckValidity` on the child's pid, which reads the
//!   kernel's record of the loaded code and is "secure against attempts to modify the file system
//!   source"). Only then is it resumed and given the request; otherwise it is killed, unread and
//!   unrun, and the call is refused as a refused signature is. The pid cannot name another process
//!   meanwhile: this process is its parent and has not reaped it.

use std::{
    ffi::{CString, OsString},
    fs,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    ptr,
    sync::Arc,
};

use ferb_identity::{
    helper::{Helper, Launcher},
    owner_secrets::{OwnerSecrets, Places},
    py, Error, Result,
};
use zeroize::Zeroizing;

use crate::security::{running_code_satisfies, SecurityFramework};
use ferb_rpc::app_peer::Security;

/// `helper.py:34`. An absolute path: `PATH` is never searched for the verifier.
const CODESIGN: &str = "/usr/bin/codesign";
/// Where setup writes the pin, beside the helper (`helper.py:32`).
const PIN_FILE: &str = "ferb-secrets.requirement";
const UNAVAILABLE: &str = "the signed secret helper is unavailable; owner setup is required";
const NOT_PINNED: &str = "the secret helper signature does not match the installed identity";

/// The only environment the helper and `codesign` get: where the login keychain is found and
/// what Foundation reads at start. Python's `subprocess.run` hands the daemon's whole environment
/// on (`helper.py:42`); nothing FERB needs from the helper depends on the rest, and a provider
/// key in the daemon's environment has no business in a child (decision: GATE-548).
pub const PASSED_ENV: [&str; 5] = [
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "__CF_USER_TEXT_ENCODING",
];

/// The Mac's [`Launcher`]: `codesign` against the pin, then the helper as a child process.
#[derive(Clone, Copy, Debug, Default)]
pub struct SignedHelper;

/// The helper of the FERB home `home`, verified and run as the Mac does.
pub fn helper(home: impl Into<PathBuf>) -> Helper {
    Helper::new(home, Arc::new(SignedHelper))
}

/// The store Python's `OwnerSecrets(home)` is: values through the signed helper of `home`.
pub fn owner_secrets(home: &Path, places: Arc<dyn Places>) -> OwnerSecrets {
    OwnerSecrets::with_keychain(home, Arc::new(helper(home)), places)
}

/// `seed.py:9`: `~/Library/Application Support/Google/Chrome`.
pub fn chrome_data() -> PathBuf {
    std::env::home_dir()
        .unwrap_or_default()
        .join("Library/Application Support/Google/Chrome")
}

impl Launcher for SignedHelper {
    /// `helper.py:29-38`: the helper's path, only once `codesign` accepted it against the pin.
    fn verified(&self, home: &Path) -> Result<PathBuf> {
        let binary = home.join("bin").join("ferb-secrets");
        let requirement = home.join("bin").join(PIN_FILE);
        if !binary.is_file() || !requirement.is_file() {
            return Err(Error::Refused(UNAVAILABLE.into()));
        }
        let pin = fs::read_to_string(&requirement)?;
        let mut check = Command::new(CODESIGN);
        check
            .args(codesign_args(&pin, &binary))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        scrub_env(&mut check);
        if !check.status()?.success() {
            return Err(Error::Refused(NOT_PINNED.into()));
        }
        Ok(binary)
    }

    /// The helper `verified` accepted, held to the same pin once it is loaded and before it runs
    /// (see the module's notes). A program that is not the pinned code is killed before it runs
    /// and the answer is `Error::Refused`, carried in the `io::Error` (`ferb_identity::Error`'s
    /// `From<io::Error>` gives it back as the refusal).
    fn run(&self, program: &Path, body: &[u8]) -> io::Result<(bool, Zeroizing<Vec<u8>>)> {
        run(program, body)
    }
}

fn refused(message: &str) -> io::Error {
    io::Error::other(Error::Refused(message.into()))
}

/// `codesign`'s arguments (`helper.py:34`): the pin as one argument, never through a shell.
fn codesign_args(pin: &str, binary: &Path) -> Vec<OsString> {
    vec![
        "--verify".into(),
        "--strict".into(),
        format!("-R={}", py::strip(pin)).into(),
        binary.as_os_str().to_owned(),
    ]
}

fn scrub_env(command: &mut Command) {
    command.env_clear();
    for name in PASSED_ENV {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
}

/// `helper.py:41-47`: run the helper with the request on its stdin; whether it failed (any exit
/// but 0, a signal included) and what it wrote on stdout. No argument, private pipes, stderr
/// discarded, only the three standard descriptors inherited (Python's `close_fds=True`). The
/// request is written only once the started program is known to be the pinned code.
fn run(binary: &Path, body: &[u8]) -> io::Result<(bool, Zeroizing<Vec<u8>>)> {
    // the pin as it is now, read beside the program, as `verified` read it
    let pin = binary
        .parent()
        .map(|bin| bin.join(PIN_FILE))
        .and_then(|file| fs::read_to_string(file).ok())
        .ok_or_else(|| refused(UNAVAILABLE))?;
    let requirement = SecurityFramework
        .requirement(py::strip(&pin))
        .ok_or_else(|| refused(NOT_PINNED))?;
    let mut child = Suspended::spawn(binary)?;
    if !running_code_satisfies(child.pid, &requirement) {
        return Err(refused(NOT_PINNED)); // `child` is killed and reaped as it drops, never resumed
    }
    child.resume()?;
    let mut stdin = fs::File::from(child.stdin.take().expect("stdin is piped"));
    let mut stdout = fs::File::from(child.stdout.take().expect("stdout is piped"));
    let reply = std::thread::scope(|scope| {
        scope.spawn(move || {
            // A helper that exits without reading closes the pipe: as in Python's
            // `communicate`, that is not an error of its own; the exit code says it.
            let _ = stdin.write_all(body);
        });
        read_all(&mut stdout)
    });
    let status = child.wait()?;
    Ok((!status, reply?))
}

/// A child started suspended: it has run no instruction until `resume`. Killed and reaped if it is
/// dropped before `wait`.
struct Suspended {
    pid: libc::pid_t,
    stdin: Option<OwnedFd>,
    stdout: Option<OwnedFd>,
    reaped: bool,
}

/// A descriptor pair from `pipe`, both close-on-exec (only the dups the spawn makes reach the
/// child).
fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: `fds` is a valid place for two descriptors.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `pipe` succeeded, so both are open descriptors this code alone owns.
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    for fd in [&read, &write] {
        // SAFETY: an open descriptor; F_SETFD only sets its close-on-exec flag.
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((read, write))
}

/// `posix_spawn`'s answer: 0, or the error number.
fn spawned(code: libc::c_int) -> io::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code))
    }
}

impl Suspended {
    fn spawn(binary: &Path) -> io::Result<Suspended> {
        let path = CString::new(binary.as_os_str().as_bytes())?;
        let env: Vec<CString> = PASSED_ENV
            .iter()
            .filter_map(|name| {
                let value = std::env::var_os(name)?;
                let mut entry = format!("{name}=").into_bytes();
                entry.extend_from_slice(value.as_bytes());
                CString::new(entry).ok()
            })
            .collect();
        let argv = [path.as_ptr().cast_mut(), ptr::null_mut()];
        let mut envp: Vec<*mut libc::c_char> = env.iter().map(|e| e.as_ptr().cast_mut()).collect();
        envp.push(ptr::null_mut());
        let (stdin_read, stdin_write) = pipe()?;
        let (stdout_read, stdout_write) = pipe()?;
        let null = c"/dev/null";
        let mut actions: libc::posix_spawn_file_actions_t = ptr::null_mut();
        let mut attributes: libc::posix_spawnattr_t = ptr::null_mut();
        // SAFETY: each init gets a valid place for its object; each is destroyed below, once.
        spawned(unsafe { libc::posix_spawn_file_actions_init(&mut actions) })?;
        // SAFETY: as above.
        if let Err(e) = spawned(unsafe { libc::posix_spawnattr_init(&mut attributes) }) {
            // SAFETY: `actions` was initialised above and is destroyed once.
            unsafe { libc::posix_spawn_file_actions_destroy(&mut actions) };
            return Err(e);
        }
        let mut pid: libc::pid_t = 0;
        // SAFETY: `actions` and `attributes` are initialised; the descriptors are open and owned
        // above until after the spawn; `path`, `argv`, `envp` and `null` are NUL-terminated and
        // outlive the call, and `argv`/`envp` end in a null pointer. The signal sets are local.
        let outcome = unsafe {
            let mut no_signals: libc::sigset_t = 0;
            let mut sigpipe: libc::sigset_t = 0;
            libc::sigemptyset(&mut no_signals);
            libc::sigemptyset(&mut sigpipe);
            libc::sigaddset(&mut sigpipe, libc::SIGPIPE);
            let flags = libc::POSIX_SPAWN_START_SUSPENDED
                | libc::POSIX_SPAWN_CLOEXEC_DEFAULT
                | libc::POSIX_SPAWN_SETSIGMASK
                | libc::POSIX_SPAWN_SETSIGDEF;
            spawned(libc::posix_spawn_file_actions_adddup2(
                &mut actions,
                stdin_read.as_raw_fd(),
                0,
            ))
            .and_then(|()| {
                spawned(libc::posix_spawn_file_actions_adddup2(
                    &mut actions,
                    stdout_write.as_raw_fd(),
                    1,
                ))
            })
            .and_then(|()| {
                spawned(libc::posix_spawn_file_actions_addopen(
                    &mut actions,
                    2,
                    null.as_ptr(),
                    libc::O_WRONLY,
                    0,
                ))
            })
            // like Rust's own `Command`: the child gets no blocked signal and SIGPIPE's default
            .and_then(|()| {
                spawned(libc::posix_spawnattr_setflags(
                    &mut attributes,
                    flags as libc::c_short,
                ))
            })
            .and_then(|()| {
                spawned(libc::posix_spawnattr_setsigmask(
                    &mut attributes,
                    &no_signals,
                ))
            })
            .and_then(|()| {
                spawned(libc::posix_spawnattr_setsigdefault(
                    &mut attributes,
                    &sigpipe,
                ))
            })
            .and_then(|()| {
                spawned(libc::posix_spawn(
                    &mut pid,
                    path.as_ptr(),
                    &actions,
                    &attributes,
                    argv.as_ptr(),
                    envp.as_ptr(),
                ))
            })
        };
        // SAFETY: both were initialised above and are destroyed exactly once.
        unsafe {
            libc::posix_spawn_file_actions_destroy(&mut actions);
            libc::posix_spawnattr_destroy(&mut attributes);
        }
        outcome?;
        drop((stdin_read, stdout_write)); // the child's ends: only the child holds them now
        Ok(Suspended {
            pid,
            stdin: Some(stdin_write),
            stdout: Some(stdout_read),
            reaped: false,
        })
    }

    /// Let the suspended child run (`POSIX_SPAWN_START_SUSPENDED` waits for SIGCONT).
    fn resume(&self) -> io::Result<()> {
        // SAFETY: `pid` is this process's own unreaped child.
        if unsafe { libc::kill(self.pid, libc::SIGCONT) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Wait for the child to end; whether it exited with 0 (a signal is a failure).
    fn wait(&mut self) -> io::Result<bool> {
        let mut status = 0;
        loop {
            // SAFETY: `pid` is this process's own child and `status` a valid place for its status.
            if unsafe { libc::waitpid(self.pid, &mut status, 0) } == self.pid {
                self.reaped = true;
                return Ok(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }
}

impl Drop for Suspended {
    fn drop(&mut self) {
        // its pipes close first, so a killed child is never left with a reader or writer here
        self.stdin.take();
        self.stdout.take();
        if !self.reaped {
            // SAFETY: `pid` is this process's own unreaped child: no other process can hold it.
            unsafe { libc::kill(self.pid, libc::SIGKILL) };
            let _ = self.wait();
        }
    }
}

/// Everything the child writes, in a buffer that is zeroed whenever it is left behind.
fn read_all(from: &mut impl Read) -> io::Result<Zeroizing<Vec<u8>>> {
    let mut out = Zeroizing::new(Vec::with_capacity(4096));
    let mut chunk = Zeroizing::new([0u8; 4096]);
    loop {
        let n = match from.read(&mut chunk[..]) {
            Ok(0) => return Ok(out),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if out.len() + n > out.capacity() {
            let mut bigger =
                Zeroizing::new(Vec::with_capacity((out.capacity() * 2).max(out.len() + n)));
            bigger.extend_from_slice(&out);
            out = bigger; // the smaller buffer is zeroed as it drops
        }
        out.extend_from_slice(&chunk[..n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn fixture() -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/identity/cases.json");
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    /// The verifier's argv is Python's (`helper.py:34`), for the pin file as setup writes it
    /// (with its newline). The fixture's first helper case starts with Python's codesign call.
    #[test]
    fn codesign_is_asked_as_python_asks_it() {
        let fixture = fixture();
        let call = &fixture["helper"][0]["calls"][0];
        let want: Vec<String> = call["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap().to_owned())
            .collect();
        let pin = format!("{}\n", fixture["requirement"].as_str().unwrap());
        let mut got = vec![CODESIGN.to_owned()];
        got.extend(
            codesign_args(&pin, Path::new("$HOME/bin/ferb-secrets"))
                .into_iter()
                .map(|a| a.into_string().unwrap()),
        );
        assert_eq!(got, want);
        assert_eq!(call["stdout"], "null");
        assert_eq!(call["stderr"], "null");
    }

    #[test]
    fn a_reply_buffer_grows_without_losing_bytes() {
        let data: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
        assert_eq!(*read_all(&mut &data[..]).unwrap(), data);
    }
}
