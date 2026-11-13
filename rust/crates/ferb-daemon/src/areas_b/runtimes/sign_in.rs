//! FERB's own Claude sign-in (ferb/runtimes/claude_login.py; L34 C, owner 2026-10-25: "Build a Claude sign-in like
//! Codex's"). The pinned Claude Code runs its non-interactive `auth login --claudeai` with FERB's own Claude home
//! (CLAUDE_CONFIG_DIR=<home>/claude-home), opens the sign-in page itself and keeps the credential in the login
//! Keychain itself. FERB reads its output only for the page's address, passes a pasted code to its stdin, and
//! records only that a sign-in finished (the SIGNED_IN marker, no secret) after `auth status --json` said so. Never
//! types into a terminal; nothing Claude Code prints is kept, logged or journaled.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

use ferb_core::errors::FerbError;
use ferb_rpc::HandlerError;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::super::core::refusal;

pub const SIGNED_IN: &str = ".ferb-signed-in";

pub fn home_dir(ferb_home: &Path) -> PathBuf {
    ferb_home.join("claude-home")
}

/// claude_login.py `claude_home`: FERB's own Claude home once a sign-in there finished (never the owner's
/// ~/.claude).
pub fn claude_home(ferb_home: &Path) -> Option<PathBuf> {
    let home = home_dir(ferb_home);
    home.join(SIGNED_IN).is_file().then_some(home)
}

/// claude_login.py `prepare`: the folder, owner-only.
pub fn prepare(ferb_home: &Path) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let home = home_dir(ferb_home);
    std::fs::create_dir_all(&home)?;
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    Ok(home)
}

/// claude_login.py `env`: FERB's Claude home; no key or token from the environment can take over the login;
/// telemetry and error reports off.
pub fn env(ferb_home: &Path, base: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut out = base.clone();
    out.insert(
        "CLAUDE_CONFIG_DIR".into(),
        home_dir(ferb_home).to_string_lossy().into_owned(),
    );
    for k in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ] {
        out.insert(k.into(), String::new());
    }
    out.insert("DISABLE_TELEMETRY".into(), "1".into());
    out.insert("DISABLE_ERROR_REPORTING".into(), "1".into());
    out
}

pub fn login_argv(cli: &Path) -> Vec<String> {
    vec![
        cli.to_string_lossy().into_owned(),
        "auth".into(),
        "login".into(),
        "--claudeai".into(),
    ]
}

pub fn logout_argv(cli: &Path) -> Vec<String> {
    vec![
        cli.to_string_lossy().into_owned(),
        "auth".into(),
        "logout".into(),
    ]
}

pub fn status_argv(cli: &Path) -> Vec<String> {
    vec![
        cli.to_string_lossy().into_owned(),
        "auth".into(),
        "status".into(),
        "--json".into(),
    ]
}

/// claude_login.py `signed_in`: `auth status --json` says a Claude subscription login is saved (an API key
/// doesn't count).
pub fn signed_in(status_output: &str) -> bool {
    let Ok(Value::Object(status)) = ferb_py::json::loads(status_output) else {
        return false;
    };
    let provider_ok = match status.get("apiProvider") {
        None => true,
        Some(v) => v == "firstParty",
    };
    let method = status
        .get("authMethod")
        .map(ferb_py::value::py_str)
        .unwrap_or_default();
    status.get("loggedIn") == Some(&Value::Bool(true))
        && provider_ok
        && !method.to_lowercase().contains("api")
}

/// claude_login.py `mark`.
pub fn mark(ferb_home: &Path) -> std::io::Result<()> {
    std::fs::write(
        prepare(ferb_home)?.join(SIGNED_IN),
        "FERB's own Claude sign-in finished; Claude Code keeps the credential in the login Keychain.\n",
    )
}

/// claude_login.py `unmark`: a sign-out.
pub fn unmark(ferb_home: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(home_dir(ferb_home).join(SIGNED_IN)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// claude_login.py `keychain_service`: the Keychain service of the credential Claude Code saves for FERB's own
/// Claude home (its name, never its value).
pub fn keychain_service(ferb_home: &Path) -> String {
    let digest = hex::encode(Sha256::digest(
        home_dir(ferb_home).to_string_lossy().as_bytes(),
    ));
    format!("Claude Code-credentials-{}", &digest[..8])
}

/// claude_login.py `sign_in_url`: the first https address in a line, without trailing `.,)`.
pub fn sign_in_url(line: &str) -> Option<String> {
    static URL: OnceLock<regex::Regex> = OnceLock::new();
    // Python's \S: anything but str.isspace() (Unicode White_Space plus U+001C..U+001F)
    let re =
        URL.get_or_init(|| regex::Regex::new(r"https://[^\s\x1c-\x1f]+").expect("url pattern"));
    re.find(line)
        .map(|m| m.as_str().trim_end_matches(['.', ',', ')']).to_string())
}

#[derive(Default)]
struct Inner {
    state: String,
    url: String,
    why: String,
    /// The login process while it runs (Python's `_proc`): its pid and stdin.
    pid: Option<i32>,
    running: bool,
    cancelled: bool,
}

/// One sign-in at a time, run by the daemon for FERB.app (`runtimes.sign_in`).
pub struct ClaudeSignIn {
    inner: Arc<Mutex<Inner>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    follow: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Default for ClaudeSignIn {
    fn default() -> Self {
        ClaudeSignIn {
            inner: Arc::new(Mutex::new(Inner {
                state: "idle".into(),
                ..Inner::default()
            })),
            stdin: Arc::new(Mutex::new(None)),
            follow: tokio::sync::Mutex::new(None),
        }
    }
}

/// What a sign-in uses from the daemon.
pub struct Hooks {
    pub ferb_home: PathBuf,
    /// claude_login.py `cli_binary`: the pinned Claude Code, or why it is missing.
    pub cli: Result<PathBuf, FerbError>,
    /// The daemon's own environment (`os.environ`).
    pub environ: BTreeMap<String, String>,
    /// Joins the redactor's known values (a pasted code).
    pub add_secret: Arc<dyn Fn(&str) + Send + Sync>,
    /// Told when a sign-in ends (not when it is cancelled).
    pub done: Arc<dyn Fn(bool) + Send + Sync>,
}

fn status_of(i: &Inner) -> Map<String, Value> {
    match json!({"state": i.state, "url": i.url, "why": i.why}) {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

impl ClaudeSignIn {
    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn status(&self) -> Map<String, Value> {
        status_of(&self.inner())
    }

    /// claude_login.py `start`: starts it, or reports the one waiting. A program that can't be started is the
    /// caller's error (Python lets the OSError through).
    pub async fn start(&self, hooks: Hooks) -> Result<Map<String, Value>, HandlerError> {
        if self.inner().state == "waiting" {
            return Ok(self.status());
        }
        let cli = match hooks.cli {
            Ok(cli) => cli,
            Err(FerbError::Unsupported(_)) => {
                let mut i = self.inner();
                i.state = "failed".into();
                i.url = String::new();
                i.why = "FERB's copy of Claude Code is missing, so it can't sign in to Claude. Download FERB again."
                    .into();
                return Ok(status_of(&i));
            }
            Err(e) => return Err(HandlerError::fault(e.class(), e.to_string())),
        };
        prepare(&hooks.ferb_home).map_err(os_fault)?;
        {
            let mut i = self.inner();
            i.state = "waiting".into();
            i.url = String::new();
            i.why = String::new();
            i.cancelled = false;
        }
        let (reader, writer) = std::io::pipe().map_err(os_fault)?;
        let writer2 = writer.try_clone().map_err(os_fault)?;
        let argv = login_argv(&cli);
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .env_clear()
            .envs(env(&hooks.ferb_home, &hooks.environ))
            .stdin(Stdio::piped())
            .stdout(writer)
            .stderr(writer2);
        // SAFETY: setsid() is async-signal-safe and touches no memory of this process; it runs in the child between
        // fork and exec (Python's start_new_session=True).
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let spawned = cmd.spawn();
        drop(cmd); // the pipe's write ends now live only in the child, so its exit ends the reader
        let mut child = spawned.map_err(os_fault)?;
        *self.stdin.lock().unwrap_or_else(|e| e.into_inner()) = child.stdin.take();
        let status = {
            let mut i = self.inner();
            i.pid = i32::try_from(child.id()).ok();
            i.running = true;
            status_of(&i)
        };
        let (inner, stdin, home, done) = (
            self.inner.clone(),
            self.stdin.clone(),
            hooks.ferb_home,
            hooks.done,
        );
        let environ = hooks.environ;
        let task = tokio::spawn(async move {
            let inner2 = inner.clone();
            let code = tokio::task::spawn_blocking(move || {
                for line in BufReader::new(reader).split(b'\n').map_while(Result::ok) {
                    if let Some(url) = sign_in_url(&String::from_utf8_lossy(&line)) {
                        let mut i = inner2.lock().unwrap_or_else(|e| e.into_inner());
                        if i.url.is_empty() {
                            i.url = url;
                        }
                    }
                }
                child.wait().ok().and_then(|s| s.code())
            })
            .await
            .ok()
            .flatten();
            *stdin.lock().unwrap_or_else(|e| e.into_inner()) = None;
            {
                let mut i = inner.lock().unwrap_or_else(|e| e.into_inner());
                i.running = false;
                i.pid = None;
                if i.cancelled {
                    return;
                }
            }
            let ok = code == Some(0) && confirmed(&cli, &home, &environ).await;
            if ok {
                let _ = mark(&home);
            }
            {
                let mut i = inner.lock().unwrap_or_else(|e| e.into_inner());
                i.state = if ok { "signed_in" } else { "failed" }.into();
                i.url = String::new();
                i.why = if ok {
                    String::new()
                } else {
                    "Claude didn't confirm the sign-in. Try again.".into()
                };
            }
            done(ok);
        });
        *self.follow.lock().await = Some(task);
        Ok(status)
    }

    /// claude_login.py `send_code`: the code the sign-in page shows, to Claude Code's stdin only.
    pub fn send_code(
        &self,
        code: &str,
        add_secret: &dyn Fn(&str),
    ) -> Result<Map<String, Value>, HandlerError> {
        let code = ferb_py::value::strip(code);
        let waiting = {
            let i = self.inner();
            i.state == "waiting" && i.running
        };
        let mut stdin = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
        let Some(pipe) = stdin.as_mut().filter(|_| waiting) else {
            return Err(refusal(
                "No Claude sign-in is waiting for a code; start it again",
            ));
        };
        if code.is_empty() || code.chars().any(ferb_py::value::is_py_space) {
            return Err(refusal("Paste the code the sign-in page shows"));
        }
        add_secret(code); // rule 14: redacted wherever anything echoes it
        pipe.write_all(format!("{code}\n").as_bytes())
            .and_then(|()| pipe.flush())
            .map_err(os_fault)?;
        drop(stdin);
        Ok(self.status())
    }

    /// claude_login.py `cancel`: the login process is told to stop (SIGTERM) and the sign-in is idle again.
    pub async fn cancel(&self) -> Map<String, Value> {
        let pid = {
            let mut i = self.inner();
            i.cancelled = true;
            i.running.then_some(i.pid).flatten()
        };
        if let Some(pid) = pid {
            // SAFETY: kill() only sends a signal to the login process this sign-in started (still running: its pid
            // is cleared once it was waited for, so it can't have been reused).
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
        if let Some(task) = self.follow.lock().await.take() {
            let _ = task.await;
        }
        let mut i = self.inner();
        i.state = "idle".into();
        i.url = String::new();
        i.why = String::new();
        status_of(&i)
    }

    /// claude_login.py `close`: the daemon's shutdown stops a sign-in still waiting.
    pub async fn close(&self) {
        if self.inner().state == "waiting" {
            self.cancel().await;
        }
    }
}

/// claude_login.py `_confirmed`: `auth status --json` in FERB's Claude home says the login is saved.
async fn confirmed(cli: &Path, home: &Path, environ: &BTreeMap<String, String>) -> bool {
    let argv = status_argv(cli);
    let env = env(home, environ);
    tokio::task::spawn_blocking(move || {
        Command::new(&argv[0])
            .args(&argv[1..])
            .env_clear()
            .envs(env)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map(|out| signed_in(&String::from_utf8_lossy(&out.stdout)))
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

fn os_fault(e: std::io::Error) -> HandlerError {
    let r = ferb_runtimes::error::os_error(&e);
    HandlerError::fault(&r.class, r.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_subscription_login_counts() {
        assert!(signed_in(
            r#"{"loggedIn": true, "authMethod": "claude.ai"}"#
        ));
        assert!(!signed_in(r#"{"loggedIn": true, "authMethod": "api_key"}"#));
        assert!(!signed_in(
            r#"{"loggedIn": true, "authMethod": "claude.ai", "apiProvider": "bedrock"}"#
        ));
        assert!(!signed_in("not json"));
        assert_eq!(
            sign_in_url("visit: https://claude.example.test/x?a=b).").as_deref(),
            Some("https://claude.example.test/x?a=b")
        );
        assert_eq!(sign_in_url("no address"), None);
    }

    #[test]
    fn the_credentials_item_is_named_by_the_home() {
        let name = keychain_service(Path::new("/FX/home"));
        assert!(name.starts_with("Claude Code-credentials-") && name.len() == 32);
    }
}
