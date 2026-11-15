//! AppleScript target encoding and execution (ferb/act/mac/applescript.py). The script runs as `/usr/bin/osascript`
//! in a child process under FERB's shell sandbox (D27, rule 10), never in-process; Apple Events reach only the app
//! the call names. A stop kills the child and drains it. Script content is never classified by keywords: the only
//! checks are argument validity (the shell command, D31 review N7) and FERB's own apps (rule 8, 9).

use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;

/// tools.py:42.
pub const SHELL_REFUSED: &str = "mac.applescript runs no shell code: `do shell script` would run a program outside \
the shell's own checks. Run the command with shell.run, where FERB's sandbox, terminal question and change record apply";

fn shell_command() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)\bdo\s+shell\s+script\b|(?:\x{ab}|<<)\s*event\s+sysoexec\s*(?:\x{bb}|>>)")
            .expect("fixed")
    })
}

fn continuation() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new("\u{ac}[ \t]*(?:\r\n|\r|\n)").expect("fixed"))
}

/// tools.py:38: `do shell script` in any case, across ¬ continuations or line breaks, or `«event sysoexec»`.
pub fn runs_shell(script: &str) -> bool {
    shell_command().is_match(&continuation().replace_all(script, " "))
}

fn own_app_script() -> &'static fancy_regex::Regex {
    static R: OnceLock<fancy_regex::Regex> = OnceLock::new();
    R.get_or_init(|| {
        fancy_regex::Regex::new(
            r"(?i)(?<![a-z0-9])ferb(?![a-z0-9])|dev\.ferb\.notch|com\.ferb\.(?:ferb|app|daemon)",
        )
        .expect("fixed")
    })
}

/// tools.py:53: a System Events script naming one of FERB's own apps.
pub fn names_own_app(script: &str) -> bool {
    own_app_script().is_match(script).unwrap_or(true)
}

fn quoted() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new("\"([^\"\n]+)\"").expect("fixed"))
}

/// `os.path.expanduser` for "~" and "~/…" (HOME, as Python reads it).
pub fn expanduser(p: &str, home: &str) -> String {
    if p == "~" {
        return home.to_string();
    }
    match p.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", home.trim_end_matches('/')),
        None => p.to_string(),
    }
}

/// applescript.py:23: the paths a script names as quoted literals, absolute.
pub fn named_paths(script: &str, home: &str, volumes: &Path) -> Vec<String> {
    let mut found: Vec<String> = vec![];
    for cap in quoted().captures_iter(script) {
        let literal = &cap[1];
        let (volume, colon, rest) = match literal.find(':') {
            Some(i) => (&literal[..i], true, &literal[i + 1..]),
            None => (literal, false, ""),
        };
        if literal.starts_with('/') || literal == "~" || literal.starts_with("~/") {
            found.push(expanduser(literal, home));
        } else if colon
            && !volume.is_empty()
            && !volume.contains('/')
            && volumes.join(volume).is_dir()
        {
            let mut p = volumes.join(volume);
            for part in rest.split(':').filter(|p| !p.is_empty()) {
                p = p.join(part);
            }
            found.push(p.to_string_lossy().into_owned());
        }
    }
    let mut out: Vec<String> = vec![];
    for f in found {
        if !out.contains(&f) {
            out.push(f);
        }
    }
    out
}

/// applescript.py:37: `tell application id "<bundle>"` around the body; a bad id is refused.
pub fn wrap(bundle_id: &str, body: &str) -> Result<String, String> {
    let ok = !bundle_id.is_empty()
        && bundle_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if !ok {
        return Err("bad app id".into());
    }
    Ok(format!(
        "tell application id \"{bundle_id}\"\n{body}\nend tell"
    ))
}

/// FERB's shell sandbox for osascript (ferb/act/shell/sandbox.py's AppleScript profile, from `ferb-shell`): the
/// command wrapped in it, with the shell's clean environment, and the profile file's cleanup.
pub trait ScriptSandbox: Send + Sync {
    /// (argv to run, environment) for `osascript` with Apple Events to `events_to` and app launches kept.
    fn wrap(&self, argv: &[String], events_to: &str) -> (Vec<String>, Vec<(String, String)>);
    fn cleanup(&self, argv: &[String]);
}

/// applescript.py:44: (ok, output): the script's result, or its error text. A timeout kills and drains the child;
/// dropping this future (a stop) kills it too (`kill_on_drop`), and the sandbox profile is cleaned up either way.
pub async fn run(
    bundle_id: &str,
    body: &str,
    timeout: f64,
    sandbox: &dyn ScriptSandbox,
    cwd: Option<&Path>,
) -> Result<(bool, String), String> {
    if timeout <= 0.0 {
        return Ok((false, "timed out before execution".into()));
    }
    let started = tokio::time::Instant::now();
    let script = wrap(bundle_id, body)?;
    let plain = vec!["/usr/bin/osascript".to_string(), "-e".to_string(), script];
    let (argv, env) = sandbox.wrap(&plain, bundle_id);
    struct Cleanup<'a>(&'a dyn ScriptSandbox, Vec<String>);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            self.0.cleanup(&self.1);
        }
    }
    let _cleanup = Cleanup(sandbox, argv.clone());
    let left = timeout - started.elapsed().as_secs_f64();
    if left <= 0.0 {
        return Ok((false, "timed out before execution".into()));
    }
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .env_clear()
        .envs(env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let read = async {
        use tokio::io::AsyncReadExt;
        let mut out = vec![];
        let mut err = vec![];
        let (a, b) = tokio::join!(
            async {
                if let Some(mut s) = stdout {
                    let _ = s.read_to_end(&mut out).await;
                }
            },
            async {
                if let Some(mut s) = stderr {
                    let _ = s.read_to_end(&mut err).await;
                }
            }
        );
        let _ = (a, b);
        (out, err)
    };
    let waited = tokio::time::timeout(Duration::from_secs_f64(left.max(0.0)), async {
        let (out, err) = read.await;
        let status = child.wait().await;
        (out, err, status)
    })
    .await;
    match waited {
        Ok((out, err, status)) => {
            let ok = status.map(|s| s.success()).unwrap_or(false);
            let bytes = if ok { out } else { err };
            Ok((ok, String::from_utf8_lossy(&bytes).into_owned()))
        }
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            Ok((false, "timed out".into()))
        }
    }
}
