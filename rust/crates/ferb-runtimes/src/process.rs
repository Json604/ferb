//! Starting the model programs (Codex's app-server, Claude Code) behind a seam, so tests drive the adapters against
//! scripted stand-ins in this process and the daemon starts real programs. The adapter owns what it starts: the
//! child, its pipes and their readers, and kills and reaps it when the run ends.

use std::collections::BTreeMap;
use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What to start: the whole environment is given (nothing is inherited beyond it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spawn {
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub stdin: bool,
    pub stdout: bool,
    /// Piped (read by the adapter) or inherited (the daemon's own stderr, as the Claude SDK leaves it).
    pub stderr: bool,
    /// Its own session (Python's `start_new_session=True`).
    pub new_session: bool,
}

impl Spawn {
    pub fn new(argv: Vec<String>, env: BTreeMap<String, String>) -> Spawn {
        Spawn {
            argv,
            env,
            cwd: None,
            stdin: true,
            stdout: true,
            stderr: true,
            new_session: false,
        }
    }
}

/// The running program's signals and exit status.
pub trait ProcControl: Send + Sync {
    /// SIGKILL, unless it already ended.
    fn kill(&self);
    /// SIGTERM, unless it already ended.
    fn terminate(&self);
    /// Its exit status once ended (a signal is its negative number, as Python reports it).
    fn returncode(&self) -> Option<i32>;
    /// Waits until it ended and was reaped.
    fn wait(&self) -> BoxFuture<'_, i32>;
}

pub struct Child {
    pub stdin: Option<Box<dyn AsyncWrite + Send + Unpin>>,
    pub stdout: Option<Box<dyn AsyncRead + Send + Unpin>>,
    pub stderr: Option<Box<dyn AsyncRead + Send + Unpin>>,
    pub control: Arc<dyn ProcControl>,
}

pub trait Spawner: Send + Sync {
    fn spawn(&self, spec: Spawn) -> io::Result<Child>;
}

/// The real thing: `tokio::process`, reaped by a task of its own.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsSpawner;

struct OsControl {
    // the child itself, behind one lock: a signal is only sent after `try_wait` said it was not reaped yet, so the
    // pid still names our child (or its zombie) and can't have been reused
    child: Arc<Mutex<tokio::process::Child>>,
    status: watch::Receiver<Option<i32>>,
}

fn code_of(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|n| -n))
        .unwrap_or(-1)
}

impl OsControl {
    fn signal(&self, sig: i32) {
        let Ok(mut child) = self.child.lock() else {
            return;
        };
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        if let Some(pid) = child.id() {
            // SAFETY: kill(2) on our own child, which try_wait (under the same lock) just found not yet reaped.
            unsafe {
                libc::kill(pid as i32, sig);
            }
        }
    }
}

impl ProcControl for OsControl {
    fn kill(&self) {
        self.signal(libc::SIGKILL);
    }

    fn terminate(&self) {
        self.signal(libc::SIGTERM);
    }

    fn returncode(&self) -> Option<i32> {
        *self.status.borrow()
    }

    fn wait(&self) -> BoxFuture<'_, i32> {
        let mut rx = self.status.clone();
        Box::pin(async move {
            loop {
                if let Some(code) = *rx.borrow_and_update() {
                    return code;
                }
                if rx.changed().await.is_err() {
                    return rx.borrow().unwrap_or(-1);
                }
            }
        })
    }
}

impl Spawner for OsSpawner {
    fn spawn(&self, spec: Spawn) -> io::Result<Child> {
        use std::process::Stdio;
        let Some((program, args)) = spec.argv.split_first() else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no program"));
        };
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args).env_clear().envs(&spec.env);
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        let pipe = |on: bool| if on { Stdio::piped() } else { Stdio::inherit() };
        cmd.stdin(pipe(spec.stdin))
            .stdout(pipe(spec.stdout))
            .stderr(pipe(spec.stderr));
        if spec.new_session {
            // SAFETY: setsid() is async-signal-safe and touches nothing of the parent's state.
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let mut child = cmd.spawn()?;
        let stdin = child
            .stdin
            .take()
            .map(|s| Box::new(s) as Box<dyn AsyncWrite + Send + Unpin>);
        let stdout = child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn AsyncRead + Send + Unpin>);
        let stderr = child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn AsyncRead + Send + Unpin>);
        let (tx, rx) = watch::channel(None);
        let child = Arc::new(Mutex::new(child));
        let reaped = child.clone();
        tokio::spawn(async move {
            loop {
                let state = match reaped.lock() {
                    Ok(mut c) => c.try_wait(),
                    Err(_) => return,
                };
                match state {
                    Ok(Some(status)) => {
                        tx.send_replace(Some(code_of(status)));
                        return;
                    }
                    Ok(None) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
                    Err(_) => {
                        tx.send_replace(Some(-1));
                        return;
                    }
                }
            }
        });
        Ok(Child {
            stdin,
            stdout,
            stderr,
            control: Arc::new(OsControl { child, status: rx }),
        })
    }
}
