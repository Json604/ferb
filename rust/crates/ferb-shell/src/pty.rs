//! A task's process in a PTY, its process group and its owner-only redacted output log
//! (ferb/act/shell/pty_proc.py). Offsets address the full log, never a lossy ring; nothing truncates the retained
//! output; the command is never chosen or rewritten here.
//!
//! The process runs in its own session (its pid is its process group). `kill` ends the whole group, TERM then KILL;
//! once the command's own process has ended, the group id could name an unrelated group (a reused pid), so it is
//! signalled only after `members_ours` confirms every process now in it is the command's (pty_proc.py:242-266).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use ferb_sanitize::{RedactionStream, Redactor};
use tokio::io::unix::AsyncFd;
use tokio::io::Interest;
use tokio::sync::watch;

/// Fixed code only: the approved command and environment are positional argv; sudo's password pipe is discarded
/// before the task gets stdin, and a TERM forwarded by sudo kills the elevated command's group.
pub const ADMIN_SUPERVISOR: &str =
    "exec 0<\"$1\"; shift; trap 'kill -KILL 0' TERM HUP INT; /usr/bin/env -i \"$@\" \
<&0 & child=$!; wait \"$child\"; status=$?; exit \"$status\"";

pub type RedactText = Arc<dyn Fn(&str) -> String + Send + Sync>;
pub type StreamFactory = Arc<dyn Fn() -> RedactionStream<Arc<Redactor>> + Send + Sync>;
type OnExit = Box<dyn FnOnce() + Send>;
/// Whether every process now in the group is the command's (FERB's sandbox).
pub type MembersOurs = Arc<dyn Fn(&[i32]) -> bool + Send + Sync>;

/// The argv an admin command runs as: sudo, then the same sandbox, then the fixed supervisor with the approved
/// environment and command as positional arguments (pty_proc.py:59-64).
pub fn admin_argv(
    argv: &[String],
    env: &[(String, String)],
    tty: &str,
) -> Result<Vec<String>, String> {
    if argv.len() < 3 || argv[0] != crate::sandbox::SANDBOX_EXEC || argv[1] != "-f" {
        return Err("an admin command must keep FERB's sandbox".into());
    }
    let mut out: Vec<String> = ["/usr/bin/sudo", "-S", "-k", "-p", "", "--"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    out.extend(argv[..3].iter().cloned());
    out.extend(
        ["/bin/sh", "-c", ADMIN_SUPERVISOR, "ferb-admin", tty]
            .iter()
            .map(|s| s.to_string()),
    );
    out.extend(env.iter().map(|(k, v)| format!("{k}={v}")));
    out.extend(argv[3..].iter().cloned());
    Ok(out)
}

struct St {
    total: u64,
    ready_ms: Option<f64>,
    pid: Option<i32>,
    exit_code: Option<i32>,
    capture_error: Option<String>,
    master: Option<RawFd>,
    log: Option<File>,
    output_closed: bool,
    redaction: RedactionStream<Arc<Redactor>>,
}

pub struct PtyProc {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub log: PathBuf,
    redact_text: RedactText,
    on_exit: Mutex<Option<OnExit>>,
    admin_secret: Mutex<Option<String>>,
    started_at: Mutex<Option<f64>>,
    st: Mutex<St>,
    done: watch::Sender<bool>,
}

impl std::fmt::Debug for PtyProc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyProc")
            .field("argv", &self.argv)
            .field("log", &self.log)
            .finish_non_exhaustive()
    }
}

/// The master side, registered with the reactor; closed explicitly (never on drop) so writers see it go under lock.
struct MasterFd(RawFd);

impl AsRawFd for MasterFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

fn cloexec(fd: RawFd) {
    // SAFETY: fcntl on a descriptor we own.
    unsafe {
        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
    }
}

fn dup(fd: RawFd) -> std::io::Result<OwnedFd> {
    // SAFETY: duplicates a descriptor we own; the new one is owned by the result.
    let n = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if n < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `n` is a fresh descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(n) })
}

enum Output {
    More,
    WouldBlock,
    Over,
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Io(String),
}

impl PtyProc {
    #[allow(clippy::too_many_arguments)] // Python's constructor, argument for argument
    pub fn new(
        argv: Vec<String>,
        cwd: PathBuf,
        env: Vec<(String, String)>,
        log: PathBuf,
        redact_text: RedactText,
        stream_factory: StreamFactory,
        on_exit: Option<OnExit>,
        admin_secret: Option<String>,
    ) -> Arc<PtyProc> {
        let (done, _) = watch::channel(false);
        Arc::new(PtyProc {
            argv,
            cwd,
            env,
            log,
            redact_text,
            on_exit: Mutex::new(on_exit),
            admin_secret: Mutex::new(admin_secret),
            started_at: Mutex::new(None),
            st: Mutex::new(St {
                total: 0,
                ready_ms: None,
                pid: None,
                exit_code: None,
                capture_error: None,
                master: None,
                log: None,
                output_closed: false,
                redaction: stream_factory(),
            }),
            done,
        })
    }

    fn lock(&self) -> MutexGuard<'_, St> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn total(&self) -> u64 {
        self.lock().total
    }
    pub fn exit_code(&self) -> Option<i32> {
        self.lock().exit_code
    }
    pub fn pid(&self) -> Option<i32> {
        self.lock().pid
    }
    pub fn ready_ms(&self) -> Option<f64> {
        self.lock().ready_ms
    }
    pub fn capture_error(&self) -> Option<String> {
        self.lock().capture_error.clone()
    }

    /// Opens the PTY and the log, starts the process in its own session, and starts reading and reaping it.
    pub async fn start(self: &Arc<Self>) -> Result<(), StartError> {
        *self.started_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(crate::monotonic());
        let io = |e: std::io::Error| StartError::Io(crate::py_os_error(&e));
        let (mut master, mut slave) = (0, 0);
        // SAFETY: openpty fills two descriptors; no name, termios or window size given.
        if unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } != 0
        {
            return Err(io(std::io::Error::last_os_error()));
        }
        cloexec(master);
        cloexec(slave);
        // SAFETY: `slave` is ours; closed when this owner drops (Python's `finally: os.close(slave)`).
        let slave = unsafe { OwnedFd::from_raw_fd(slave) };
        self.lock().master = Some(master);
        let result = self.spawn(master, &slave).await;
        drop(slave);
        if let Err(e) = result {
            let pid = self.lock().pid;
            if let Some(pid) = pid {
                // SAFETY: signals the group of the process we just started (its own session).
                unsafe { libc::killpg(pid, libc::SIGKILL) };
            }
            self.close_output();
            return Err(e);
        }
        Ok(())
    }

    async fn spawn(self: &Arc<Self>, master: RawFd, slave: &OwnedFd) -> Result<(), StartError> {
        let io = |e: std::io::Error| StartError::Io(crate::py_os_error(&e));
        let secret = self
            .admin_secret
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        // SAFETY: termios on our own slave descriptor.
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(slave.as_raw_fd(), &mut t) != 0 {
                return Err(io(std::io::Error::last_os_error()));
            }
            t.c_oflag &= !libc::ONLCR; // keep multiline values byte for byte
            if secret.is_some() {
                t.c_lflag &= !libc::ECHO;
            }
            if libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &t) != 0 {
                return Err(io(std::io::Error::last_os_error()));
            }
        }
        if let Some(parent) = self.log.parent() {
            if let Some(grand) = parent.parent() {
                std::fs::create_dir_all(grand).map_err(io)?;
            }
            match std::fs::DirBuilder::new().mode(0o700).create(parent) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(io(e)),
            }
        }
        let log = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&self.log)
            .map_err(io)?;
        log.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(io)?;
        self.lock().log = Some(log);
        let argv = if secret.is_some() {
            // SAFETY: ttyname of our own slave; copied before the descriptor closes.
            let tty = unsafe {
                let p = libc::ttyname(slave.as_raw_fd());
                if p.is_null() {
                    return Err(io(std::io::Error::last_os_error()));
                }
                std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
            };
            admin_argv(&self.argv, &self.env, &tty).map_err(StartError::Refused)?
        } else {
            self.argv.clone()
        };
        let mut cmd = tokio::process::Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .current_dir(&self.cwd)
            .env_clear()
            .envs(self.env.iter().cloned());
        cmd.stdin(if secret.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::from(dup(slave.as_raw_fd()).map_err(io)?)
        });
        cmd.stdout(std::process::Stdio::from(
            dup(slave.as_raw_fd()).map_err(io)?,
        ));
        cmd.stderr(std::process::Stdio::from(
            dup(slave.as_raw_fd()).map_err(io)?,
        ));
        // SAFETY: setsid is async-signal-safe; the child gets its own session and process group.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn().map_err(io)?;
        let pid = child.id().map(|p| p as i32);
        self.lock().pid = pid;
        if let Some(secret) = secret {
            if let Some(mut stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(format!("{secret}\n").as_bytes()).await; // a broken pipe is ignored
                drop(stdin);
            }
            drop(secret);
        }
        // SAFETY: the master is ours; non-blocking for the reactor.
        unsafe {
            let flags = libc::fcntl(master, libc::F_GETFL);
            libc::fcntl(master, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
        // SAFETY: the master stays open until this AsyncFd is dropped: `run` drops it before `close_output` closes
        // the descriptor, and nothing else closes it.
        let afd = unsafe { AsyncFd::register_with_interest(MasterFd(master), Interest::READABLE) }
            .map_err(|e| io(e.into()))?;
        let me = self.clone();
        tokio::spawn(async move { me.run(afd, child).await });
        Ok(())
    }

    /// Reads output as it comes and reaps the process; then drains what was written before exit (never waiting for
    /// a descendant that still holds the terminal), closes the output and tells `on_exit`.
    async fn run(self: Arc<Self>, afd: AsyncFd<MasterFd>, mut child: tokio::process::Child) {
        let mut afd = Some(afd);
        let status = loop {
            let mut exited = None;
            let over = {
                let readable = async {
                    match &afd {
                        Some(a) => a.readable().await.ok(),
                        None => std::future::pending().await,
                    }
                };
                tokio::select! {
                    status = child.wait() => {
                        exited = Some(status);
                        false
                    }
                    guard = readable => match guard {
                        None => false,
                        Some(mut g) => match self.on_output() {
                            Output::WouldBlock => {
                                g.clear_ready();
                                false
                            }
                            Output::More => false,
                            Output::Over => true,
                        },
                    },
                }
            };
            if let Some(status) = exited {
                break status;
            }
            if over {
                drop(afd.take()); // deregister before the descriptor closes
                self.close_output();
            }
        };
        let code = match status {
            Ok(s) => crate::exit_code(s),
            Err(_) => -1,
        };
        self.lock().exit_code = Some(code);
        self.ready();
        drop(afd.take());
        self.drain();
        self.close_output();
        self.done.send_replace(true);
        let callback = self
            .on_exit
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(f) = callback {
            f();
        }
    }

    /// One read of the master: more may come, nothing to read now, or the output is over (EOF, EIO, an error, a
    /// capture failure).
    fn on_output(&self) -> Output {
        let Some(master) = self.lock().master else {
            return Output::Over;
        };
        let mut buf = vec![0u8; 65536];
        // SAFETY: reads into our buffer from our master descriptor (still open: closing needs this task).
        let n = unsafe { libc::read(master, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::WouldBlock {
                return Output::WouldBlock;
            }
            if e.kind() == std::io::ErrorKind::Interrupted {
                return Output::More;
            }
            if e.raw_os_error() != Some(libc::EIO) {
                self.capture_failed(&crate::py_os_error_class(&e), e.raw_os_error());
            }
            return Output::Over; // a PTY commonly signals EOF with EIO
        }
        if n == 0 {
            return Output::Over;
        }
        self.ready();
        if let Err((class, errno)) = self.append(&buf[..n as usize], false) {
            self.capture_failed(&class, errno);
            return Output::Over;
        }
        Output::More
    }

    fn drain(&self) {
        loop {
            if self.lock().output_closed {
                return;
            }
            let Some(master) = self.lock().master else {
                return;
            };
            let mut buf = vec![0u8; 65536];
            // SAFETY: as in `on_output`.
            let n = unsafe { libc::read(master, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n <= 0 {
                return;
            }
            if let Err((class, errno)) = self.append(&buf[..n as usize], false) {
                self.capture_failed(&class, errno);
                return;
            }
        }
    }

    fn ready(&self) {
        let started = *self.started_at.lock().unwrap_or_else(|e| e.into_inner());
        let mut st = self.lock();
        if st.ready_ms.is_none() {
            if let Some(s) = started {
                st.ready_ms = Some((crate::monotonic() - s) * 1000.0);
            }
        }
    }

    /// The production stream redacts registered secrets; `redact_text` keeps callers' contextual redaction and never
    /// sees half a character or a partial record.
    fn append(&self, data: &[u8], last: bool) -> Result<(), (String, Option<i32>)> {
        let mut st = self.lock();
        let out = st.redaction.feed(data, last);
        let text = String::from_utf8(out).map_err(|_| ("UnicodeDecodeError".to_string(), None))?;
        let safe = (self.redact_text)(&text).into_bytes();
        let St { log, total, .. } = &mut *st;
        let Some(log) = log.as_mut() else {
            return Ok(());
        };
        let mut rest: &[u8] = &safe;
        while !rest.is_empty() {
            match log.write(rest) {
                Ok(0) => return Err(("OSError".into(), Some(libc::EIO))),
                Ok(n) => {
                    rest = &rest[n..];
                    *total += n as u64;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err((crate::py_os_error_class(&e), e.raw_os_error())),
            }
        }
        Ok(())
    }

    /// Never the exception's arguments (they may hold raw output): its type and errno.
    fn capture_failed(&self, class: &str, errno: Option<i32>) {
        let code = errno.map(|e| format!(" (errno {e})")).unwrap_or_default();
        self.lock().capture_error = Some(format!(
            "{class}{code}: shell output capture failed; output is incomplete"
        ));
    }

    fn close_output(&self) {
        let (master, flush) = {
            let mut st = self.lock();
            if st.output_closed {
                return;
            }
            st.output_closed = true;
            (
                st.master.take(),
                st.log.is_some() && st.capture_error.is_none(),
            )
        };
        if let Some(m) = master {
            // SAFETY: closes our master; no reactor registration remains (dropped before this call).
            unsafe { libc::close(m) };
        }
        if flush {
            if let Err((class, errno)) = self.append(b"", true) {
                self.capture_failed(&class, errno);
            }
        }
        self.lock().log = None;
    }

    /// Full redacted history by byte offset; returned offsets never split a UTF-8 character (pty_proc.py:178).
    pub fn read(&self, since: i64, limit: Option<u64>) -> (String, u64) {
        let total = self.total();
        read_log(&self.log, total, since, limit)
    }

    /// The longest line, in bytes with its newline, the terminal takes while it reads lines (canonical mode); None
    /// when the command reads raw input or isn't running.
    pub fn line_limit(&self) -> Option<i64> {
        let st = self.lock();
        if st.exit_code.is_some() {
            return None;
        }
        let master = st.master?;
        // SAFETY: termios and pathconf on our open master, under the lock that guards its closing.
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(master, &mut t) != 0 || t.c_lflag & libc::ICANON == 0 {
                return None;
            }
            let n = libc::fpathconf(master, libc::_PC_MAX_CANON);
            (n > 0).then_some(n as i64)
        }
    }

    /// Writes what the terminal takes now; never blocks; 0 once the command ended.
    pub fn write(&self, data: &[u8]) -> std::io::Result<usize> {
        let st = self.lock();
        if st.exit_code.is_some() || data.is_empty() {
            return Ok(0);
        }
        let Some(master) = st.master else {
            return Ok(0);
        };
        // SAFETY: writes our buffer to our open master, under the lock that guards its closing.
        let n = unsafe { libc::write(master, data.as_ptr() as *const libc::c_void, data.len()) };
        if n >= 0 {
            return Ok(n as usize);
        }
        let e = std::io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EAGAIN) | Some(libc::EIO) | Some(libc::EBADF) => Ok(0),
            _ => Err(e),
        }
    }

    pub fn send(&self, text: &str) -> std::io::Result<usize> {
        self.write(text.as_bytes())
    }

    /// The exit code once ended, or None after `timeout` seconds.
    pub async fn wait(&self, timeout: f64) -> Option<i32> {
        let mut rx = self.done.subscribe();
        let ended = tokio::time::timeout(
            Duration::from_secs_f64(timeout.max(0.0)),
            rx.wait_for(|d| *d),
        )
        .await;
        match ended {
            Ok(Ok(_)) => self.exit_code(),
            _ => None,
        }
    }

    /// End the command's whole process group: TERM, then KILL (pty_proc.py:242).
    pub async fn kill(&self, members_ours: Option<MembersOurs>) {
        let Some(pgid) = self.pid() else { return };
        for sig in [libc::SIGTERM, libc::SIGKILL] {
            if self.exit_code().is_some() {
                let members = crate::sockets::group_members(pgid);
                let ours = members_ours.as_ref().is_some_and(|f| f(&members));
                if members.is_empty() || !ours {
                    return;
                }
            }
            // SAFETY: signals the process group this PtyProc started (checked above once its leader is gone).
            if unsafe { libc::killpg(pgid, sig) } != 0 {
                return; // ESRCH or EPERM
            }
            if self.exit_code().is_none() && self.wait(2.0).await.is_none() {
                continue;
            }
            for _ in 0..40 {
                if crate::sockets::group_members(pgid).is_empty() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

/// pty_proc.py `read` on a log file of `total` bytes: (text, the offset after it).
pub fn read_log(log: &Path, total: u64, since: i64, limit: Option<u64>) -> (String, u64) {
    let mut start = (since.max(0) as u64).min(total);
    let Ok(mut f) = File::open(log) else {
        return (String::new(), start);
    };
    let _ = f.seek(SeekFrom::Start(start));
    if start > 0 {
        while start < total {
            let mut b = [0u8; 1];
            match f.read(&mut b) {
                Ok(1) if b[0] & 0xC0 == 0x80 => start += 1,
                _ => break,
            }
        }
        let _ = f.seek(SeekFrom::Start(start));
    }
    let want = match limit {
        Some(l) => l.min(total - start),
        None => total - start,
    };
    let mut chunk = vec![0u8; want as usize];
    let mut got = 0;
    while got < chunk.len() {
        match f.read(&mut chunk[got..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => got += n,
        }
    }
    chunk.truncate(got);
    if !chunk.is_empty() && start + (chunk.len() as u64) < total {
        for _ in 0..3 {
            match std::str::from_utf8(&chunk) {
                Ok(_) => break,
                Err(e) if e.error_len().is_some() => break,
                Err(_) => {
                    let mut b = [0u8; 1];
                    if let Ok(1) = f.read(&mut b) {
                        chunk.push(b[0]);
                    }
                }
            }
        }
    }
    let end = start + chunk.len() as u64;
    (String::from_utf8_lossy(&chunk).into_owned(), end)
}
