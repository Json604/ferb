//! The setup commands (ferb/cli/setup.py): `setup --service [--update] [--app-service]` (FERB.app's setup), a
//! checkout's `setup [--update]`, `setup --cli`, the Codex and Claude sign-ins and sign-outs, the store's one-time
//! clean-up, `doctor` and `uninstall [--purge [--yes]]`. Every line printed, every exit status and every daemon call
//! is Python's; the machine is reached only through [`Env`]'s seams.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use super::{CliError, Connect, Rpc, Signals};
use crate::install::{
    designated_requirement, trusted_requirement, InstallError, Installer, Output, BUNDLE_CLI,
    MARKER, NATIVE_HOST_MANIFEST, SWITCHED_OFF_HOW, SWITCHED_OFF_STATUS,
};
use crate::profile::{diff, summarize, DeviceProfile, ProfileStore};

pub const CODEX_LOGIN_HOW: &str =
    "choose Sign in to Codex… in FERB's menu bar, or run `ferb setup --codex-login`";
pub const PASTE_PANE: &str = "System Settings → Privacy & Security → Paste from Other Apps";
pub const CERTIFICATE: &str = "FERB Local Code Signing";
/// ferb/device/permissions.py FDA_WAITS.
pub const FDA_WAITS: &str = "FERB's shell sandbox and network proxy must be working first";
/// ferb/decide/decider.py JEV_KEYS_URL.
pub const JEV_KEYS_URL: &str = "https://console.typesafe.ai/keys";
pub const CLAUDE_SIGNED_IN: &str = ".ferb-signed-in";

/// How a program is run (`subprocess.run`'s keywords that matter): environment variables set on top of this
/// process's, whether output is captured, a timeout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunOpts {
    pub env: Vec<(String, String)>,
    pub capture: bool,
    pub timeout: Option<f64>,
}

/// Runs a program; Err when it couldn't run (OSError, a timeout).
pub type Run<'a> = Box<dyn FnMut(&[String], &RunOpts) -> Result<Output, String> + 'a>;

/// ferb/device/permissions.py, ported in ferb-mac-hands: which of the daemon's rows aren't granted, from a
/// `doctor.check` reply, and whether Full Disk Access can be granted yet.
pub trait PermissionFacts {
    fn missing(&mut self, report: &Value) -> Vec<(String, String)>;
    fn fda_ready(&mut self, containment: &Value) -> bool;
}

/// The store's clean-up and size (ferb/store/cleanup.py, ferb/store/db.py; ferb-store).
pub trait StoreFacts {
    fn preview(&mut self, path: &Path) -> Value;
    /// Opens the store, applies the clean-up the owner agreed to (`ok`), closes it: the result, or (type, message).
    fn apply(
        &mut self,
        home: &Path,
        ok: &str,
        stop: &dyn Fn() -> bool,
        say: &mut dyn FnMut(&str),
    ) -> Result<Value, (String, String)>;
    fn pending(&mut self, path: &Path) -> bool;
    fn size(&mut self, path: &Path) -> u64;
}

/// The runtimes' presence checks (ferb/runtimes/catalog.py CATALOG): None for a runtime FERB doesn't know.
pub trait RuntimeFacts {
    fn available(&mut self, name: &str, home: &Path) -> Option<(bool, String)>;
}

/// A checkout's per-browser steps (ferb/act/browser/autoinstall.py): the lines `extension_setup` prints, the
/// install requests uninstall removes, the native-host folders doctor checks.
pub trait BrowserSetup {
    fn setup_lines(
        &mut self,
        home: &Path,
        support: &Path,
        paired: Option<Vec<(String, String)>>,
    ) -> Vec<String>;
    fn remove_requests(&mut self, home: &Path, support: &Path) -> Vec<PathBuf>;
    fn native_host_dirs(&mut self, support: &Path) -> Vec<PathBuf>;
}

/// [protection] as the daemon reads it: (never-read, never-write) counts, or why it fails.
pub type Protection<'a> = Box<dyn FnMut(&Path) -> Result<(usize, usize), String> + 'a>;

/// setup.py:49 `Env`.
pub struct Env<'a> {
    pub inst: Installer,
    pub connect: Connect<'a>,
    pub ask: Box<dyn FnMut(&str) -> String + 'a>,
    pub out: Box<dyn Write + 'a>,
    pub sleep: Box<dyn FnMut(f64) + 'a>,
    pub run: Run<'a>,
    /// FERB's pinned tokenizer copy put in place, in one line (cli/tokenizer.rs `ensure`).
    pub tokenizer: Box<dyn FnMut(&Path) -> String + 'a>,
    /// The device scan for a device id (scan.rs `scan` on this Mac's app folders).
    pub scan: Box<dyn FnMut(&str) -> DeviceProfile + 'a>,
    /// ~/Library/Application Support (a checkout's browser step); None: no browser step.
    pub browser_support: Option<PathBuf>,
    pub browsers: Option<Box<dyn BrowserSetup + 'a>>,
    pub permissions: Box<dyn PermissionFacts + 'a>,
    pub store: Box<dyn StoreFacts + 'a>,
    pub runtimes: Box<dyn RuntimeFacts + 'a>,
    /// [protection] as the daemon reads it: (never-read, never-write) counts, or why it fails.
    pub protection: Protection<'a>,
    /// Codex's model catalog dumped (codex_exec.bundled_catalog): true when it printed one.
    pub codex_catalog: Box<dyn FnMut(&Path) -> bool + 'a>,
    /// The pinned Claude Code (claude_login.cli_binary), or why it isn't there.
    pub claude_cli: Box<dyn FnMut() -> Result<PathBuf, String> + 'a>,
    /// A checkout's code hash, compared with the running daemon's (doctor); None: not compared.
    pub checkout_code_hash: Option<String>,
    pub signals: Box<dyn Signals + 'a>,
}

impl Env<'_> {
    pub fn say(&mut self, line: &str) {
        let _ = writeln!(self.out, "{line}");
        let _ = self.out.flush();
    }

    fn config(&self) -> ferb_core::config::Config {
        ferb_core::config::Config::new(&self.inst.home)
    }

    fn run_quiet(&mut self, argv: &[&str], timeout: Option<f64>) -> Result<Output, String> {
        let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        (self.run)(
            &argv,
            &RunOpts {
                env: Vec::new(),
                capture: true,
                timeout,
            },
        )
    }
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn get_str(v: &Value, key: &str) -> String {
    v.get(key)
        .map(ferb_py::value::py_str)
        .unwrap_or_else(|| "None".into())
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(ferb_py::value::truthy)
}

/// setup.py:72: the daemon once it answers `ping`.
pub fn wait_for_daemon(env: &mut Env<'_>, timeout: f64) -> Result<Box<dyn Rpc>, CliError> {
    let end = env.inst.clock.monotonic() + timeout;
    loop {
        let attempt = (env.connect)().and_then(|mut c| c.call("ping", None).map(|_| c));
        match attempt {
            Ok(c) => return Ok(c),
            Err(CliError::NotRunning(_) | CliError::Rpc { .. } | CliError::Ferb { .. }) => {
                if env.inst.clock.monotonic() > end {
                    return Err(CliError::NotRunning(
                        "the daemon didn't come up; see ~/.ferb/logs/daemon.err.log".into(),
                    ));
                }
                (env.sleep)(0.3);
            }
            Err(e) => return Err(e),
        }
    }
}

/// setup.py:107: the browsers step. From FERB.app nothing here runs (DMN-507).
pub fn extension_setup(env: &mut Env<'_>) -> Result<(), CliError> {
    let Some(support) = env.browser_support.clone() else {
        return Ok(());
    };
    if env.inst.bundle.is_some() {
        env.say("Browsers: FERB connects them once it has Full Disk Access (the setup tour's Pair browsers step).");
        return Ok(());
    }
    let paired = match (env.connect)().and_then(|mut c| {
        let listed = c.call("browsers.list", None)?;
        c.close();
        Ok(listed)
    }) {
        Ok(listed) => Some(
            listed["profiles"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter(|p| truthy(p.get("paired")))
                .map(|p| (get_str(p, "browser"), get_str(p, "folder")))
                .collect(),
        ),
        Err(
            CliError::NotRunning(_)
            | CliError::Rpc { .. }
            | CliError::Ferb { .. }
            | CliError::Io(_),
        ) => None,
        Err(e) => return Err(e),
    };
    let home = env.inst.home.clone();
    let lines = match env.browsers.as_mut() {
        Some(b) => b.setup_lines(&home, &support, paired),
        None => Vec::new(),
    };
    for line in lines {
        env.say(&line);
    }
    Ok(())
}

/// setup.py:139: FERB's device id, made once.
pub fn device_id(config: &ferb_core::config::Config) -> Result<String, CliError> {
    if let Some(v) = config.get("device", "id")? {
        if ferb_py::value::truthy(&v) {
            return Ok(ferb_py::value::py_str(&v));
        }
    }
    use ferb_core::interfaces::IdSource;
    let new = ferb_core::ids::RandomIds.new_id();
    config.set("device", "id", &json!(new))?;
    Ok(new)
}

/// `with _finish_install(): body`: a stop meanwhile is held, then reported as Interrupted.
fn finishing<T>(
    env: &mut Env<'_>,
    body: impl FnOnce(&mut Env<'_>) -> Result<T, CliError>,
) -> Result<T, CliError> {
    env.signals.hold();
    let out = body(env);
    let caught = env.signals.release();
    let out = out?;
    if caught {
        return Err(CliError::Interrupted);
    }
    Ok(out)
}

/// setup.py:170.
fn install(env: &mut Env<'_>, update: bool) -> Result<(), CliError> {
    let before = env.inst.active_program();
    finishing(env, |env| {
        env.inst.install(update, None).map_err(CliError::from)
    })?;
    let now = env.inst.active_program();
    if let (Some(b), Some(n)) = (&before, &now) {
        if b != n {
            env.say(&format!(
                "     The background service now runs from {n} (it ran from {b})."
            ));
        }
    }
    for note in env.inst.notes.clone() {
        env.say(&format!("     {note}"));
    }
    app_installed(env);
    Ok(())
}

/// setup.py:191: a FERB daemon answering outside launchd.
fn foreign_daemon(env: &mut Env<'_>) -> Result<Option<String>, CliError> {
    let pid = match (env.connect)().and_then(|mut c| {
        let r = c.call("ping", None)?;
        c.close();
        Ok(r)
    }) {
        Ok(r) => r.get("pid").cloned().unwrap_or(Value::Null),
        Err(CliError::NotRunning(_)) => return Ok(None),
        Err(e) => return Err(e),
    };
    let agent = env.inst.agent_pid()?;
    let same = match agent {
        Some(a) => pid.as_i64() == Some(a),
        None => pid.is_null(),
    };
    if !same {
        return Ok(Some(format!(
            "A FERB daemon (pid {}) is already running outside launchd — probably `ferb daemon run` in a terminal. Stop it (Ctrl-C) and run `ferb setup` again.",
            ferb_py::value::py_str(&pid)
        )));
    }
    Ok(None)
}

/// setup.py:206: FERB.app's setup window.
pub fn service_setup(env: &mut Env<'_>, update: bool) -> Result<i32, CliError> {
    if !update {
        if let Some(refusal) = foreign_daemon(env)? {
            env.say(&refusal);
            return Ok(1);
        }
    }
    env.say(if update {
        "Updating FERB's background service…"
    } else {
        "Setting up FERB's background service…"
    });
    let home = env.inst.home.clone();
    let line = (env.tokenizer)(&home);
    env.say(&line);
    match install(env, update) {
        Ok(()) => {}
        Err(CliError::Install(InstallError::SwitchedOff(m))) => {
            env.say(&m);
            return Ok(SWITCHED_OFF_STATUS);
        }
        Err(e) => return Err(e),
    }
    if !update {
        let id = device_id(&env.config())?;
        let profile = (env.scan)(&id);
        ProfileStore::new(&home).save(&profile, "setup scan")?;
    }
    if env.inst.bundle.is_some() || home.join("FERB.app").exists() {
        voice_helper(env)?;
    }
    let started = !env.inst.in_app() || env.inst.loaded();
    if started {
        wait_for_daemon(env, 30.0)?.close();
    }
    extension_setup(env)?;
    if !update {
        filevault_line(env);
    }
    env.say(if started {
        "FERB's background service is running."
    } else {
        "FERB's background service is ready for FERB.app to turn on."
    });
    Ok(0)
}

/// setup.py:240: a checkout's `ferb setup [--update]` (from FERB.app's link it sets up from that app).
pub fn setup(env: &mut Env<'_>, update: bool) -> Result<i32, CliError> {
    let home = env.inst.home.clone();
    if update {
        let line = match &env.inst.bundle {
            Some(b) => format!(
                "Updating FERB from {} and restarting the daemon…",
                b.display()
            ),
            None => "Updating FERB in a new venv and restarting the daemon…".into(),
        };
        env.say(&line);
        let t = (env.tokenizer)(&home);
        env.say(&t);
        install(env, true)?;
        if env.inst.bundle.is_some() || home.join("FERB.app").exists() {
            voice_helper(env)?;
        }
        let mut client = wait_for_daemon(env, 30.0)?;
        permissions_state(env, client.as_mut())?;
        review_persona(env, client.as_mut(), false)?;
        client.close();
        if codex_needed(&home)? {
            env.say(&format!(
                "FERB's own Codex isn't signed in, so FERB can't answer yet: {CODEX_LOGIN_HOW}."
            ));
        }
        extension_setup(env)?;
        env.say("Done.");
        return Ok(0);
    }
    if let Some(refusal) = foreign_daemon(env)? {
        env.say(&refusal);
        return Ok(1);
    }
    let first = match &env.inst.bundle {
        Some(b) => format!(
            "1/4  Setting up FERB's background service from {}…",
            b.display()
        ),
        None => "1/4  Installing FERB's dedicated Python, its venv and the LaunchAgent…".into(),
    };
    env.say(&first);
    let t = (env.tokenizer)(&home);
    env.say(&format!("     {t}"));
    install(env, false)?;
    let mut client = wait_for_daemon(env, 30.0)?;
    let binary = env.inst.grant_target();
    let shown = binary.map(|b| s(&b)).unwrap_or_else(|| "None".into());
    let line = if env.inst.bundle.is_some() {
        format!("     Daemon running. Permissions go to FERB ({shown}); macOS lists it as FERB.")
    } else {
        format!("     Daemon running. Permissions must be granted to: {shown}")
    };
    env.say(&line);
    extension_setup(env)?;
    env.say("2/4  Scanning this Mac's apps (Info.plist only)…");
    let id = device_id(&env.config())?;
    let profile = (env.scan)(&id);
    ProfileStore::new(&home).save(&profile, "setup scan")?;
    let sm = summarize(&profile);
    let browsers: Vec<String> = sm["browsers"]
        .as_array()
        .unwrap()
        .iter()
        .map(ferb_py::value::py_str)
        .collect();
    let browsers = if browsers.is_empty() {
        "none".to_string()
    } else {
        browsers.join(", ")
    };
    env.say(&format!(
        "     {} apps, {} scriptable; browsers: {browsers}",
        sm["apps"], sm["scriptable"]
    ));
    filevault_line(env);
    env.say("3/4  Permissions (checked by the daemon itself)");
    if voice_helper(env)?.is_none() {
        env.say("     Hearing isn't set up; `ferb doctor` will remind you.");
    }
    permissions_state(env, client.as_mut())?;
    env.say("4/4  Self-test through the daemon");
    let report = client.call("doctor.check", None)?;
    let mut ok = print_selftest(env, &report);
    review_persona(env, client.as_mut(), false)?;
    client.close();
    ok &= offer_codex_login(env)?;
    env.say("     FERB uses the installed action registry without per-action permission prompts.");
    env.say("     Connect the accounts/profiles you want available; macOS permissions and logins still apply.");
    env.say(if ok {
        "Setup complete."
    } else {
        "Setup finished with problems — `ferb doctor` shows what to fix."
    });
    Ok(if ok { 0 } else { 1 })
}

/// setup.py:303.
fn permissions_state(env: &mut Env<'_>, client: &mut dyn Rpc) -> Result<(), CliError> {
    let report = client.call(
        "doctor.check",
        Some(json!({"selftest": false, "hearing": true})),
    )?;
    let missing = env.permissions.missing(&report);
    if !missing.is_empty() {
        let rows: Vec<String> = missing.iter().map(|(r, s)| format!("{r}={s}")).collect();
        env.say(&format!(
            "     Not granted yet: {}. Grant them in FERB's setup window (Set Up FERB… in FERB's menu bar); `ferb doctor` lists them.",
            rows.join(", ")
        ));
        return Ok(());
    }
    env.say("     All of the daemon's macOS permissions are granted (checked by the running daemon); FERB's setup window checks FERB.app's own and records setup as done.");
    Ok(())
}

// ---------- the store's one-time clean-up (STORE-402) ----------

fn daemon_answers(env: &mut Env<'_>) -> bool {
    (env.connect)()
        .and_then(|mut c| {
            c.call("ping", None)?;
            c.close();
            Ok(())
        })
        .is_ok()
}

/// setup.py:330: `setup --cleanup`, one JSON object for FERB.app.
pub fn cleanup_preview(env: &mut Env<'_>) -> Result<i32, CliError> {
    let path = env.inst.home.join("ferb.db");
    if !path.exists() {
        env.say("{\"steps\": [], \"rows\": 0, \"compact\": false, \"ok\": null}");
        return Ok(0);
    }
    let plan = env.store.preview(&path);
    env.say(&ferb_py::json::dumps(&plan));
    Ok(0)
}

/// setup.py:343: `setup --cleanup --ok DIGEST`.
pub fn cleanup_apply(env: &mut Env<'_>, ok: &str) -> Result<i32, CliError> {
    if daemon_answers(env) && !env.inst.loaded() {
        env.say("The clean-up didn't run: a FERB daemon outside launchd answers (`ferb daemon run` in a terminal?). Stop it, then try again.");
        return Ok(1);
    }
    let result = finishing(env, |env| {
        let mut result = Map::new();
        if env.inst.loaded() || env.inst.labelled_plist() {
            // Installer::while_stopped, with the clean-up (which needs the rest of the env) as its work
            let mut got = None;
            let outcome = env.inst.pause_service().and_then(|_| {
                let done = env
                    .inst
                    .check_exited()
                    .map(|_| got = Some(run_cleanup(env, ok)));
                let resumed = env.inst.resume_service();
                resumed.and(done)
            });
            if let Some(g) = got {
                result.extend(g);
            }
            match outcome {
                Ok(()) => {}
                Err(e @ (InstallError::Install(_) | InstallError::SwitchedOff(_))) => {
                    let m = e.to_string();
                    env.say(&format!(
                        "The clean-up didn't run: {m}. Try again in a moment."
                    ));
                    result.insert("not_run".into(), json!(m));
                }
                Err(other) => return Err(CliError::Install(other)),
            }
        } else {
            result.extend(run_cleanup(env, ok));
        }
        env.say(&ferb_py::json::dumps(&Value::Object(result.clone())));
        Ok(result)
    })?;
    Ok(if truthy(result.get("refused")) {
        2
    } else if truthy(result.get("error")) || truthy(result.get("not_run")) {
        1
    } else {
        0
    })
}

/// setup.py:371.
fn run_cleanup(env: &mut Env<'_>, ok: &str) -> Map<String, Value> {
    let mut out = Map::new();
    let loaded = env.inst.loaded();
    let exiting = if loaded { None } else { env.inst.exiting() };
    let running = if loaded {
        "launchd hasn't stopped FERB's daemon yet".to_string()
    } else if let Some(pid) = exiting {
        format!("FERB's daemon (process {pid}) hasn't exited since launchd stopped it")
    } else if daemon_answers(env) {
        "a FERB daemon still answers".to_string()
    } else {
        String::new()
    };
    if !running.is_empty() {
        env.say(&format!(
            "The clean-up didn't run: {running}. Try again in a moment."
        ));
        out.insert("not_run".into(), json!(running));
        return out;
    }
    let home = env.inst.home.clone();
    let mut said: Vec<String> = Vec::new();
    let signals = &env.signals;
    let stop = || signals.caught();
    let applied = env
        .store
        .apply(&home, ok, &stop, &mut |line| said.push(line.to_string()));
    for line in said {
        env.say(&line);
    }
    let result = match applied {
        Ok(Value::Object(m)) => m,
        Ok(_) => Map::new(),
        Err((kind, message)) => {
            env.say(&format!(
                "The clean-up stopped ({kind}: {message}); what it removed stays removed, and running it again finishes it."
            ));
            out.insert("error".into(), json!(format!("{kind}: {message}")));
            return out;
        }
    };
    let removed: i64 = result
        .get("removed")
        .and_then(Value::as_object)
        .map(|m| m.values().filter_map(Value::as_i64).sum())
        .unwrap_or(0);
    let count = group_thousands(removed);
    if truthy(result.get("refused")) {
        env.say("The clean-up didn't run: what it would remove changed since you saw it. Look at it again.");
    } else if truthy(result.get("stopped")) {
        env.say(&format!(
            "The clean-up stopped before it finished ({count} rows removed so far); running it again finishes it."
        ));
    } else if result.get("compacted") == Some(&Value::Bool(false)) && result.contains_key("needed")
    {
        env.say(&format!(
            "Removed {count} rows. Not compacted: that needs {} free and the disk has {}; FERB reuses the freed space inside the file, and running the clean-up again compacts it.",
            super::admin::shown_bytes(result["needed"].as_f64().unwrap_or(0.0)),
            super::admin::shown_bytes(result.get("free").and_then(Value::as_f64).unwrap_or(0.0))
        ));
    } else if truthy(result.get("compacted")) {
        env.say(&format!(
            "Removed {count} rows; FERB's database: {} → {}.",
            super::admin::shown_bytes(
                result
                    .get("bytes_before")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
            ),
            super::admin::shown_bytes(
                result
                    .get("bytes_after")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
            )
        ));
    } else {
        env.say(&format!("Removed {count} rows."));
    }
    result
}

/// Python's `f"{n:,}"`.
pub fn group_thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// setup.py:419.
fn app_installed(env: &mut Env<'_>) {
    if env.inst.bundle.is_none() {
        return;
    }
    if let Some(status) = env.inst.cli_status() {
        env.say(&format!("     {status}"));
    }
}

/// setup.py:430 (MEM-13): what FERB saw on this Mac, shown once.
fn review_persona(env: &mut Env<'_>, client: &mut dyn Rpc, refresh: bool) -> Result<i32, CliError> {
    let mut result = client.call("setup.persona", Some(json!({"refresh": refresh})))?;
    if result["state"] == "scanning" {
        env.say("Getting to know this Mac from local app-use metadata…");
    }
    while result["state"] == "scanning" {
        (env.sleep)(0.1);
        result = client.call("setup.persona", Some(json!({"status": true})))?;
    }
    if result["state"] == "failed" {
        let error = result
            .get("error")
            .map(ferb_py::value::py_str)
            .unwrap_or_else(|| "unknown error".into());
        env.say(&format!(
            "App-use scan failed: {}",
            ferb_sanitize::terminal::printable(&error)
        ));
        env.say("FERB's setup window can look again (Set Up FERB… in FERB's menu bar).");
        return Ok(1);
    }
    if result["state"] == "seen" && truthy(result.get("summary")) {
        let summary = ferb_py::value::py_str(&result["summary"]);
        env.say(&ferb_sanitize::terminal::printable(&summary));
    }
    Ok(0)
}

fn print_selftest(env: &mut Env<'_>, report: &Value) -> bool {
    let mut ok = true;
    for r in report["selftest"].as_array().cloned().unwrap_or_default() {
        let good = truthy(r.get("ok"));
        env.say(&format!(
            "     {} {}: {}",
            if good { "✓" } else { "✗" },
            get_str(&r, "name"),
            get_str(&r, "detail")
        ));
        ok &= good;
    }
    ok
}

// ---------- doctor ----------

/// setup.py:459 `Check`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    /// "ok" | "fail" | "warn".
    pub status: String,
    pub detail: String,
    pub hint: String,
}

impl Check {
    pub fn new(
        name: impl Into<String>,
        status: &str,
        detail: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Check {
            name: name.into(),
            status: status.into(),
            detail: detail.into(),
            hint: hint.into(),
        }
    }

    pub fn json(&self) -> Value {
        json!({"name": self.name, "status": self.status, "detail": self.detail, "hint": self.hint})
    }
}

/// setup.py:469.
pub fn context_window_check(role: &str, lim: &Value) -> Check {
    let rm = format!("{} {}", get_str(lim, "runtime"), get_str(lim, "model"));
    if let Some(problem) = lim.get("problem") {
        return Check::new(
            format!("context window {role}"),
            "fail",
            format!("{rm}: {}", ferb_py::value::py_str(problem)),
            "set a documented model in config.toml [models]",
        );
    }
    let cap = if truthy(lim.get("max_input_chars")) {
        format!(
            ", at most {} characters in one input",
            get_str(lim, "max_input_chars")
        )
    } else {
        String::new()
    };
    Check::new(
        format!("context window {role}"),
        "ok",
        format!(
            "{rm}: {} tokens, {} for the reply{cap}",
            get_str(lim, "context_window"),
            get_str(lim, "max_output")
        ),
        "",
    )
}

/// setup.py:479: FERB's runtimes use Codex and FERB's own Codex home has no sign-in.
pub fn codex_needed(home: &Path) -> Result<bool, CliError> {
    let config = ferb_core::config::Config::new(home);
    let get = |key: &str, default: &str| -> Result<String, CliError> {
        Ok(config
            .get("runtimes", key)?
            .map(|v| ferb_py::value::py_str(&v))
            .unwrap_or_else(|| default.into()))
    };
    let worker = get("worker", "codex")?;
    let roles = [
        get("orchestrator", "codex")?,
        worker.clone(),
        get("checker", &worker)?,
    ];
    Ok(roles.iter().any(|r| r == "codex") && !home.join("codex-home/auth.json").exists())
}

/// setup.py:490 (C2): setup offers FERB's own Codex sign-in when it's missing.
fn offer_codex_login(env: &mut Env<'_>) -> Result<bool, CliError> {
    if !codex_needed(&env.inst.home)? {
        return Ok(true);
    }
    let answer = (env.ask)("     FERB answers through Codex, which isn't signed in for FERB. Sign in to ChatGPT for FERB now? [Y/n] ");
    let answer = crate::py::strip(&answer).to_lowercase();
    if matches!(answer.as_str(), "" | "y" | "yes") && codex_login(env)? == 0 {
        return Ok(true);
    }
    env.say(&format!(
        "     FERB can't answer until its Codex is signed in: {CODEX_LOGIN_HOW}."
    ));
    Ok(false)
}

/// claude_login.signed_in: `auth status --json` says a Claude subscription login is saved.
pub fn claude_signed_in(status_output: &str) -> bool {
    let Ok(Value::Object(status)) = serde_json::from_str::<Value>(status_output) else {
        return false;
    };
    status.get("loggedIn") == Some(&Value::Bool(true))
        && status
            .get("apiProvider")
            .map(ferb_py::value::py_str)
            .as_deref()
            .unwrap_or("firstParty")
            == "firstParty"
        && !status
            .get("authMethod")
            .map(ferb_py::value::py_str)
            .unwrap_or_default()
            .to_lowercase()
            .contains("api")
}

/// claude_login.env: FERB's Claude home, no key or token that could take over the login, telemetry off.
fn claude_env(home: &Path) -> Vec<(String, String)> {
    vec![
        (
            "CLAUDE_CONFIG_DIR".into(),
            s(&home.join(crate::install::CLAUDE_HOME)),
        ),
        ("ANTHROPIC_API_KEY".into(), String::new()),
        ("ANTHROPIC_AUTH_TOKEN".into(), String::new()),
        ("CLAUDE_CODE_OAUTH_TOKEN".into(), String::new()),
        ("DISABLE_TELEMETRY".into(), "1".into()),
        ("DISABLE_ERROR_REPORTING".into(), "1".into()),
    ]
}

/// setup.py:503: `setup --claude-logout`.
pub fn claude_logout(env: &mut Env<'_>) -> Result<i32, CliError> {
    let marker = env
        .inst
        .home
        .join(crate::install::CLAUDE_HOME)
        .join(CLAUDE_SIGNED_IN);
    if !marker.is_file() {
        env.say("FERB's Claude isn't signed in; nothing to sign out of.");
        return Ok(0);
    }
    let cli = match (env.claude_cli)() {
        Ok(c) => s(&c),
        Err(e) => {
            env.say(&format!("FERB's pinned Claude Code isn't installed: {e}"));
            return Ok(1);
        }
    };
    let cenv = claude_env(&env.inst.home);
    let _ = (env.run)(
        &[cli.clone(), "auth".into(), "logout".into()],
        &RunOpts {
            env: cenv.clone(),
            capture: false,
            timeout: None,
        },
    );
    let status = (env.run)(
        &[cli, "auth".into(), "status".into(), "--json".into()],
        &RunOpts {
            env: cenv,
            capture: true,
            timeout: None,
        },
    )
    .map_err(CliError::Io)?;
    if claude_signed_in(&status.stdout) {
        env.say("Claude didn't sign out; nothing changed.");
        return Ok(1);
    }
    let _ = std::fs::remove_file(&marker);
    env.say("Signed out: FERB can't use Claude until you sign in again.");
    Ok(0)
}

/// codex_exec.codex_binary: the pinned native binary under FERB's node folder.
fn codex_binary(inst: &Installer) -> Result<PathBuf, CliError> {
    let p = inst.home.join("node").join(&inst.codex.native_bin);
    if p.exists() {
        return Ok(p);
    }
    Err(CliError::Ferb {
        kind: "Unsupported".into(),
        message: "FERB's pinned Codex isn't installed: set FERB up again".into(),
    })
}

/// setup.py:526: `setup --codex-logout`.
pub fn codex_logout(env: &mut Env<'_>) -> Result<i32, CliError> {
    let home = env.inst.home.join("codex-home");
    if !home.join("auth.json").exists() {
        env.say("FERB's Codex isn't signed in; nothing to sign out of.");
        return Ok(0);
    }
    let binary = match codex_binary(&env.inst) {
        Ok(b) => b,
        Err(e) => {
            env.say(&format!("FERB's pinned Codex isn't installed: {e}"));
            return Ok(1);
        }
    };
    let rc = (env.run)(
        &[s(&binary), "logout".into()],
        &RunOpts {
            env: vec![("CODEX_HOME".into(), s(&home))],
            capture: false,
            timeout: None,
        },
    )
    .map_err(CliError::Io)?
    .code;
    let gone = !home.join("auth.json").exists();
    let ok = rc == 0 && gone;
    env.say(if ok {
        "Signed out: FERB can't answer through Codex until you sign in again."
    } else {
        "Codex didn't sign out; nothing changed."
    });
    Ok(if ok { 0 } else { 1 })
}

/// setup.py:547: `setup --cli`.
pub fn cli_setup(env: &mut Env<'_>) -> Result<i32, CliError> {
    let Some(bundle) = env.inst.bundle.clone() else {
        env.say("The command-line tool is linked from FERB.app; a checkout runs `uv run ferb`.");
        return Ok(1);
    };
    env.inst.link_cli()?;
    let ok = env.inst.cli_link().is_some_and(|link| {
        std::fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink())
            && crate::py::resolve(&link) == crate::py::resolve(&bundle.join(BUNDLE_CLI))
    });
    let status = env.inst.cli_status().unwrap_or_default();
    env.say(&status);
    if ok {
        env.say(
            "To use it in a terminal, add  export PATH=\"$HOME/.local/bin:$PATH\"  to ~/.zprofile.",
        );
    }
    Ok(if ok { 0 } else { 1 })
}

/// setup.py:563: `setup --codex-login`.
pub fn codex_login(env: &mut Env<'_>) -> Result<i32, CliError> {
    let home = env.inst.home.join("codex-home");
    std::fs::create_dir_all(&home)?;
    std::fs::set_permissions(&home, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    let binary = match codex_binary(&env.inst) {
        Ok(b) => Ok(b),
        Err(_) => env
            .inst
            .ensure_codex()
            .map_err(CliError::from)
            .and_then(|_| codex_binary(&env.inst)),
    };
    let binary = match binary {
        Ok(b) => b,
        Err(
            e @ (CliError::Ferb { .. }
            | CliError::Install(InstallError::Install(_) | InstallError::SwitchedOff(_))),
        ) => {
            env.say(&format!("FERB's pinned Codex couldn't be installed: {e}"));
            return Ok(1);
        }
        Err(e) => return Err(e),
    };
    env.say("Signing in FERB's own Codex (a browser window opens)…");
    let rc = (env.run)(
        &[s(&binary), "login".into()],
        &RunOpts {
            env: vec![("CODEX_HOME".into(), s(&home))],
            capture: false,
            timeout: None,
        },
    )
    .map_err(CliError::Io)?
    .code;
    let ok = home.join("auth.json").exists();
    if ok {
        let ferb_home = env.inst.home.clone();
        if !(env.codex_catalog)(&ferb_home) {
            env.say("Codex didn't print its model catalog; FERB reads it again when its background service restarts.");
        }
    }
    env.say(if rc == 0 && ok {
        "Done: FERB now uses its own Codex home."
    } else {
        "Not signed in; nothing changed."
    });
    Ok(if ok { 0 } else { 1 })
}

/// setup.py:594 (I3).
pub fn clipboard_check(state: Option<&str>) -> Option<Check> {
    let state = state?;
    if matches!(state, "allow" | "unrestricted") {
        return Some(Check::new(
            "clipboard",
            "ok",
            "FERB may read what you copy (kept on this Mac for 7 days)",
            "",
        ));
    }
    let why = match state {
        "default" => "macOS hasn't asked you yet".to_string(),
        "ask" => "macOS would ask you every time".to_string(),
        "deny" => "you set FERB to Deny".to_string(),
        other => format!("macOS reports {other}"),
    };
    let fix = if state == "default" {
        format!(
            "let macOS ask once from FERB's setup window, then set FERB to Allow in {PASTE_PANE}"
        )
    } else {
        format!("set FERB to Allow in {PASTE_PANE}")
    };
    Some(Check::new(
        "clipboard",
        "warn",
        format!("FERB doesn't record what you copy: {why}"),
        fix,
    ))
}

/// setup.py:608: the first output line of a status command; None when it can't run or fails.
fn first_line(run: &mut Run<'_>, argv: &[&str]) -> Option<String> {
    let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    let done = run(
        &argv,
        &RunOpts {
            env: Vec::new(),
            capture: true,
            timeout: Some(10.0),
        },
    )
    .ok()?;
    let out = crate::py::strip(&done.stdout).to_string();
    let lines = crate::py::splitlines(&out);
    if done.code == 0 && !lines.is_empty() {
        Some(crate::py::strip(lines[0]).to_string())
    } else {
        None
    }
}

/// setup.py:618 (DMN-506).
pub fn sip_check(run: &mut Run<'_>) -> Check {
    let line = first_line(run, &["/usr/bin/csrutil", "status"]);
    if line
        .as_ref()
        .is_some_and(|l| l.to_lowercase().ends_with("status: enabled."))
    {
        return Check::new("system integrity protection", "ok", "on", "");
    }
    let what = match &line {
        None => "couldn't be read",
        Some(l) if l.to_lowercase().contains("disabled") => "is off",
        Some(_) => "is partly off",
    };
    Check::new(
        "system integrity protection",
        "warn",
        format!("System Integrity Protection {what}: any process can then read a running command's proxy credential from its environment, and no sandbox rule stops it"),
        "turn it back on from macOS Recovery (`csrutil enable`)",
    )
}

/// setup.py:631 (DMN-502).
pub fn filevault_check(run: &mut Run<'_>) -> Check {
    let line = first_line(run, &["/usr/bin/fdesetup", "status"]);
    if line
        .as_ref()
        .is_some_and(|l| l.starts_with("FileVault is On"))
    {
        return Check::new("filevault", "ok", "on", "");
    }
    let what = if line.is_none() {
        "couldn't be read"
    } else {
        "is off"
    };
    Check::new(
        "filevault",
        "warn",
        format!("FileVault {what}: what FERB keeps in ~/.ferb (its database, traces and screenshots) sits unencrypted on this Mac's disk"),
        "turn it on in System Settings → Privacy & Security → FileVault",
    )
}

fn filevault_line(env: &mut Env<'_>) {
    let check = filevault_check(&mut env.run);
    if check.status != "ok" {
        env.say(&format!("     {}; {}.", check.detail, check.hint));
    }
}

/// setup.py:650.
fn protection_check(env: &mut Env<'_>) -> Check {
    let home = env.inst.home.clone();
    match (env.protection)(&home) {
        Err(e) => Check::new("protection", "fail", e, "fix [protection] in ~/.ferb/config.toml (or the workspace), then `ferb setup --update`"),
        Ok((reads, writes)) => Check::new(
            "protection",
            "ok",
            format!("{reads} never-read and {writes} never-write entries; FERB's shell and file tools follow both (a task asks you before it opens one)"),
            "",
        ),
    }
}

/// setup.py:663.
fn database_check(env: &mut Env<'_>) -> Option<Check> {
    let path = env.inst.home.join("ferb.db");
    if !path.exists() {
        return None;
    }
    let size = super::admin::shown_bytes(env.store.size(&path) as f64);
    if env.store.pending(&path) {
        return Some(Check::new(
            "database",
            "warn",
            format!("{size}; a one-time clean-up of data FERB no longer uses waits for your OK"),
            "FERB's app shows what it removes (`ferb setup --cleanup` lists it)",
        ));
    }
    Some(Check::new("database", "ok", size, ""))
}

fn or_none(v: Option<&Value>) -> String {
    match v {
        Some(x) if ferb_py::value::truthy(x) => ferb_py::value::py_str(x),
        _ => "none".into(),
    }
}

/// setup.py:677.
pub fn containment_check(c: Option<&Value>) -> Check {
    let Some(c) = c.filter(|c| ferb_py::value::truthy(c)) else {
        return Check::new(
            "containment",
            "fail",
            "the daemon didn't report it",
            "run `ferb setup --update`",
        );
    };
    if truthy(c.get("profile_ok")) && truthy(c.get("shell")) {
        let mut detail =
            "FERB's shell and AppleScript run inside its sandbox (sandbox-exec)".to_string();
        let alive = c
            .get("proxy_alive")
            .map(ferb_py::value::truthy)
            .unwrap_or(true);
        if truthy(c.get("proxy")) && !alive {
            return Check::new(
                "containment",
                "fail",
                format!("{detail}; FERB's egress proxy didn't answer, so the shell has no network (last error: {})", or_none(c.get("proxy_last_error"))),
                "restart the daemon (`ferb setup --update`)",
            );
        }
        if truthy(c.get("proxy_restarted")) {
            detail += &format!(
                "; FERB's egress proxy had stopped and was restarted (last error: {})",
                or_none(c.get("proxy_last_error"))
            );
        }
        return Check::new("containment", "ok", detail, "");
    }
    let why = if !truthy(c.get("profile_ok")) {
        get_str(c, "reason")
    } else {
        get_str(c, "shell_reason")
    };
    Check::new(
        "containment",
        "fail",
        format!("FERB's shell is unavailable until sandbox-exec works: {why}"),
        "restart the daemon after fixing it (`ferb setup --update`)",
    )
}

/// setup.py:697.
pub fn local_reports_check(c: Option<&Value>) -> Option<Check> {
    let status = c
        .and_then(|c| c.get("local_reports"))
        .filter(|v| !v.is_null())?;
    if truthy(status.get("running")) {
        return Some(Check::new(
            "local connections",
            "ok",
            "direct connections to pinned local ports are recorded from the kernel's reports",
            "",
        ));
    }
    let why = match status.get("stopped") {
        Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
        _ => "not started".into(),
    };
    Some(Check::new(
        "local connections",
        "warn",
        format!("the kernel's reports stopped ({why}): direct connections to pinned local ports are unrecorded (null) until FERB restarts"),
        "restart the daemon (`ferb setup --update`); reading the system log may need an admin account",
    ))
}

/// setup.py:712. `local` turns a recorded time into this Mac's local time.
pub fn snapshot_check(c: Option<&Value>) -> Check {
    let Some(c) = c.filter(|c| ferb_py::value::truthy(c)) else {
        return Check::new(
            "snapshots",
            "warn",
            "the daemon didn't report them",
            "run `ferb setup --update`",
        );
    };
    let failure = c
        .get("snapshot_failure")
        .filter(|f| ferb_py::value::truthy(f))
        .cloned()
        .unwrap_or(json!({}));
    if c.get("snapshots") == Some(&Value::Bool(false)) || truthy(failure.get("off")) {
        return Check::new(
            "snapshots",
            "warn",
            "snapshots are turned off: shell commands and permanent deletes run without a recovery snapshot",
            "",
        );
    }
    if failure.as_object().is_some_and(|m| !m.is_empty()) {
        let when = failure
            .get("at")
            .map(ferb_py::value::py_str)
            .and_then(|at| local_time(&at, "%Y-%m-%d %H:%M"))
            .map(|t| format!(" at {t}"))
            .unwrap_or_default();
        return Check::new(
            "snapshots",
            "warn",
            format!(
                "the last snapshot attempt failed{when} ({}); shell commands since then ran without a recovery snapshot",
                get_str(&failure, "why")
            ),
            "check that Time Machine local snapshots work (`tmutil localsnapshot`)",
        );
    }
    Check::new("snapshots", "ok", "a local snapshot comes before shell work and permanent deletes (one per 15 minutes, reused)", "")
}

/// `datetime.fromisoformat(at).astimezone().strftime(fmt)` in this process's time zone (TZ; a naive time is
/// taken as local already); None when `at` isn't a time Python reads.
pub fn local_time(at: &str, fmt: &str) -> Option<String> {
    use chrono::{NaiveDate, TimeZone};
    let t = ferb_py::time::DateTime::from_isoformat(at).ok()?;
    let naive = NaiveDate::from_ymd_opt(t.year, t.month, t.day)?.and_hms_micro_opt(
        t.hour,
        t.minute,
        t.second,
        t.microsecond,
    )?;
    match t.offset {
        None => Some(naive.format(fmt).to_string()),
        Some(_) => {
            let u = t.to_utc().ok()?;
            let utc = NaiveDate::from_ymd_opt(u.year, u.month, u.day)?.and_hms_micro_opt(
                u.hour,
                u.minute,
                u.second,
                u.microsecond,
            )?;
            Some(
                chrono::Local
                    .from_utc_datetime(&utc)
                    .format(fmt)
                    .to_string(),
            )
        }
    }
}

/// setup.py:734 (L26).
pub fn add_hearing_check(add: &mut dyn FnMut(Check), hearing: Option<&Value>) {
    let Some(h) = hearing.filter(|h| ferb_py::value::truthy(h)) else {
        return;
    };
    let fix = "run `ferb setup`";
    if !truthy(h.get("installed")) {
        add(Check::new(
            "hearing",
            "warn",
            "FERB.app isn't installed, so FERB can't turn sound into text",
            fix,
        ));
    } else if h.get("signed").is_none() {
    } else if !truthy(h.get("signed")) {
        add(Check::new(
            "hearing",
            "warn",
            "FERB.app's signature isn't the one recorded at setup",
            fix,
        ));
    } else if truthy(h.get("error")) {
        add(Check::new(
            "hearing",
            "warn",
            format!("FERB.app didn't answer: {}", get_str(h, "error")),
            fix,
        ));
    } else if h.get("speech") != Some(&json!("granted")) {
        add(Check::new(
            "hearing",
            "warn",
            format!("FERB.app's Speech Recognition: {}", get_str(h, "speech")),
            format!("{fix} and allow Speech Recognition for FERB"),
        ));
    } else if !truthy(h.get("on_device")) {
        add(Check::new(
            "hearing",
            "warn",
            format!("on-device speech recognition isn't available for {}; FERB never uses a cloud recognizer", get_str(h, "locale")),
            "on-device support follows the Mac's Dictation languages (System Settings → Keyboard → Dictation)",
        ));
    } else {
        add(Check::new(
            "hearing",
            "ok",
            format!(
                "FERB.app may use on-device Speech Recognition ({})",
                get_str(h, "locale")
            ),
            "",
        ));
    }
}

/// setup.py:758.
pub fn service_check(inst: &mut Installer) -> Result<Check, CliError> {
    let fix = if inst.bundle.is_some() {
        "open FERB.app: it sets its background service up again"
    } else {
        "run `ferb setup --update`"
    };
    let built = inst
        .bundle_info()?
        .get("CFBundleVersion")
        .map(crate::plistlib::py_str)
        .unwrap_or_default();
    let recorded_build = inst.recorded().get("build").cloned();
    let same_build = recorded_build == Some(json!(built));
    let plist = inst.plist_path();
    if inst.in_app() {
        if inst.labelled_plist() {
            return Ok(Check::new(
                "service definition",
                "fail",
                format!("an old {} is still there", plist.display()),
                fix,
            ));
        }
        if !same_build {
            return Ok(Check::new(
                "service definition",
                "warn",
                format!("FERB.app build {built} isn't the one set up"),
                fix,
            ));
        }
        return Ok(Check::new(
            "service definition",
            "ok",
            "registered by FERB.app (System Settings → General → Login Items)",
            "",
        ));
    }
    if !plist.exists() {
        return Ok(Check::new(
            "LaunchAgent",
            "fail",
            format!("{} is missing", plist.display()),
            fix,
        ));
    }
    if !inst.definition_current()? {
        return Ok(Check::new(
            "LaunchAgent",
            "fail",
            format!("{} differs from what setup writes", plist.display()),
            fix,
        ));
    }
    if inst.bundle.is_some() && !same_build {
        return Ok(Check::new(
            "LaunchAgent",
            "warn",
            format!("FERB.app build {built} isn't the one set up"),
            fix,
        ));
    }
    Ok(Check::new("LaunchAgent", "ok", s(&plist), ""))
}

fn call_once(env: &mut Env<'_>, method: &str, params: Option<Value>) -> Result<Value, CliError> {
    let mut c = (env.connect)()?;
    let r = c.call(method, params)?;
    c.close();
    Ok(r)
}

fn soft(e: &CliError) -> bool {
    matches!(
        e,
        CliError::NotRunning(_) | CliError::Rpc { .. } | CliError::Ferb { .. }
    )
}

/// setup.py:778: `ferb doctor`.
pub fn doctor(env: &mut Env<'_>) -> Result<i32, CliError> {
    let mut checks: Vec<Check> = Vec::new();
    let home = env.inst.home.clone();
    checks.push(Check::new(
        "home",
        if home.join(MARKER).exists() {
            "ok"
        } else {
            "fail"
        },
        s(&home),
        "run `ferb setup`",
    ));
    if home.join("codex-home/auth.json").exists() {
        checks.push(Check::new(
            "codex home",
            "ok",
            "FERB's own (~/.ferb/codex-home)",
            "",
        ));
    } else if codex_needed(&home)? {
        checks.push(Check::new(
            "codex home",
            "fail",
            "FERB's own Codex isn't signed in, so FERB can't answer (it never uses your personal Codex)",
            CODEX_LOGIN_HOW,
        ));
    } else {
        checks.push(Check::new(
            "codex home",
            "ok",
            "not signed in; FERB's runtimes don't use Codex",
            "",
        ));
    }
    match env.config().load() {
        Ok(_) => checks.push(Check::new("config permissions", "ok", "owner-only", "")),
        Err(ferb_core::config::ConfigError::Insecure(m)) => checks.push(Check::new(
            "config permissions",
            "fail",
            m,
            "chmod 600 ~/.ferb/config.toml; chmod 700 ~/.ferb",
        )),
        Err(e) => return Err(e.into()),
    }
    checks.push(protection_check(env));
    checks.push(sip_check(&mut env.run));
    checks.push(filevault_check(&mut env.run));
    if let Some(d) = database_check(env) {
        checks.push(d);
    }
    if let Some(bundle) = env.inst.bundle.clone() {
        checks.push(Check::new("app", "ok", s(&bundle), ""));
        let dp = env.inst.daemon_python();
        checks.push(Check::new(
            "interpreter",
            if dp.is_file() { "ok" } else { "fail" },
            s(&dp),
            "download FERB again",
        ));
    } else {
        let interp = env.inst.interpreter();
        checks.push(Check::new(
            "interpreter",
            if interp.is_some() { "ok" } else { "fail" },
            interp.map(|p| s(&p)).unwrap_or_else(|| "missing".into()),
            "run `ferb setup`",
        ));
        checks.push(Check::new(
            "venv",
            if env.inst.venv_python().exists() {
                "ok"
            } else {
                "fail"
            },
            s(&env.inst.venv()),
            "run `ferb setup`",
        ));
    }
    checks.push(service_check(&mut env.inst)?);
    let off = env.inst.disabled();
    let loaded = env.inst.loaded();
    let label = env.inst.label.clone();
    let has_bundle = env.inst.bundle.is_some();
    checks.push(Check::new(
        "background service",
        if loaded && !off { "ok" } else { "fail" },
        if off {
            format!("{label} is switched off in Login Items")
        } else {
            label
        },
        if off {
            SWITCHED_OFF_HOW
        } else if has_bundle {
            "open FERB.app"
        } else {
            "run `ferb setup`"
        },
    ));
    let report = match call_once(env, "doctor.check", None) {
        Ok(r) => Some(r),
        Err(e) if soft(&e) => {
            checks.push(Check::new(
                "daemon",
                "fail",
                e.to_string(),
                "check ~/.ferb/logs/daemon.err.log, then `ferb setup`",
            ));
            None
        }
        Err(e) => return Err(e),
    };
    if let Some(report) = report.filter(ferb_py::value::truthy) {
        doctor_report(env, &report, &mut checks)?;
    }
    let config = env.config();
    let did = if home.join("config.toml").exists() {
        config.get("device", "id")?
    } else {
        None
    };
    let stored = match did.filter(ferb_py::value::truthy) {
        Some(d) => ProfileStore::new(&home).load(&ferb_py::value::py_str(&d))?,
        None => None,
    };
    match stored {
        None => checks.push(Check::new(
            "device profile",
            "fail",
            "missing",
            "run `ferb setup`",
        )),
        Some(stored) => {
            let now = (env.scan)(&stored.device_id);
            let d = diff(&stored, &now);
            checks.push(Check::new(
                "device profile",
                if d.empty() { "ok" } else { "warn" },
                if d.empty() {
                    "up to date".to_string()
                } else {
                    format!(
                        "{} added, {} removed, {} changed since last scan",
                        d.added.len(),
                        d.removed.len(),
                        d.changed.len()
                    )
                },
                if d.empty() { "" } else { "run `ferb setup`" },
            ));
        }
    }
    for ch in &checks {
        let mark = match ch.status.as_str() {
            "ok" => "✓",
            "warn" => "!",
            _ => "✗",
        };
        let hint = if !ch.hint.is_empty() && ch.status != "ok" {
            format!("  → {}", ch.hint)
        } else {
            String::new()
        };
        let line = format!("{mark} {}: {}{hint}", ch.name, ch.detail);
        env.say(&line);
    }
    Ok(if checks.iter().any(|c| c.status == "fail") {
        1
    } else {
        0
    })
}

/// doctor's checks from the daemon's own report.
fn doctor_report(
    env: &mut Env<'_>,
    report: &Value,
    checks: &mut Vec<Check>,
) -> Result<(), CliError> {
    let dedicated = env.inst.granted_binary();
    let executable = get_str(report, "executable");
    let shown_dedicated = dedicated
        .as_ref()
        .map(|p| s(p))
        .unwrap_or_else(|| "None".into());
    checks.push(Check::new(
        "daemon interpreter",
        if dedicated
            .as_ref()
            .is_some_and(|d| Path::new(&executable) == d)
        {
            "ok"
        } else {
            "warn"
        },
        executable.clone(),
        if executable != shown_dedicated {
            "this is a dev daemon (`ferb daemon run`), not the installed one"
        } else {
            ""
        },
    ));
    if let Some(here) = &env.checkout_code_hash {
        if env.inst.bundle.is_none()
            && env.inst.sources.repo_root.join("pyproject.toml").exists()
            && get_str(report, "code_hash") != *here
        {
            checks.push(Check::new(
                "installed code",
                "warn",
                "differs from this checkout",
                "run `ferb setup --update`",
            ));
        }
    }
    let containment = report
        .get("containment")
        .filter(|c| ferb_py::value::truthy(c))
        .cloned()
        .unwrap_or(json!({}));
    let ready = env.permissions.fda_ready(&containment);
    if let Some(perms) = report.get("permissions").and_then(Value::as_object) {
        for (perm, status) in perms {
            let status = ferb_py::value::py_str(status);
            let waits = perm == "full_disk_access" && status != "granted" && !ready;
            let granted = status == "granted";
            checks.push(Check::new(
                format!("permission {perm}"),
                if granted { "ok" } else { "fail" },
                if waits {
                    format!("{status} ({FDA_WAITS})")
                } else {
                    status.clone()
                },
                if granted {
                    String::new()
                } else if waits {
                    "fix containment first, then grant it".into()
                } else {
                    format!("grant it to {shown_dedicated} (System Settings → Privacy & Security)")
                },
            ));
        }
    }
    for r in report["selftest"].as_array().cloned().unwrap_or_default() {
        checks.push(Check::new(
            format!("self-test {}", get_str(&r, "name")),
            if truthy(r.get("ok")) { "ok" } else { "fail" },
            get_str(&r, "detail"),
            "",
        ));
    }
    let c = report.get("containment");
    checks.push(containment_check(c));
    if let Some(l) = local_reports_check(c) {
        checks.push(l);
    }
    checks.push(snapshot_check(c));
    add_hearing_check(&mut |ch| checks.push(ch), report.get("hearing"));
    if let Some(cl) = clipboard_check(report.get("clipboard").and_then(Value::as_str)) {
        checks.push(cl);
    }
    if let Some(t) = report.get("tokenizer") {
        let exact = ferb_py::value::py_str(t).starts_with("o200k");
        checks.push(Check::new(
            "tokenizer",
            if exact { "ok" } else { "warn" },
            if exact {
                "o200k_base, pinned copy verified"
            } else {
                "missing: token counts (the planner's budget) use a byte ceiling"
            },
            if exact {
                ""
            } else {
                "run `ferb setup --update`"
            },
        ));
    }
    let rs = report.get("runtimes").cloned().unwrap_or(json!({}));
    for name in ["codex", "claude"] {
        if let Some(v) = rs.get(name) {
            let text = ferb_py::value::py_str(v);
            let ok = !text.starts_with("problem");
            let used = rs.get("orchestrator") == Some(&json!(name))
                || rs.get("worker") == Some(&json!(name));
            checks.push(Check::new(
                format!("runtime {name}"),
                if ok {
                    "ok"
                } else if used {
                    "fail"
                } else {
                    "warn"
                },
                text,
                if ok { "" } else { "run `ferb setup --update`" },
            ));
        }
    }
    let config = env.config();
    let home = env.inst.home.clone();
    for role in ["orchestrator", "worker", "checker"] {
        let default = if role == "checker" {
            config
                .get("runtimes", "worker")?
                .map(|v| ferb_py::value::py_str(&v))
                .unwrap_or_else(|| "codex".into())
        } else {
            "codex".into()
        };
        let rname = config
            .get("runtimes", role)?
            .map(|v| ferb_py::value::py_str(&v))
            .unwrap_or(default);
        let (usable, fix) = env
            .runtimes
            .available(&rname, &home)
            .unwrap_or((false, "check config.toml [runtimes]".into()));
        checks.push(if usable {
            Check::new(format!("runtime {role}"), "ok", rname, "")
        } else {
            Check::new(
                format!("runtime {role}"),
                "warn",
                format!("{rname} not usable"),
                fix,
            )
        });
    }
    if let Some(limits) = rs.get("limits").and_then(Value::as_object) {
        for (role, lim) in limits {
            checks.push(context_window_check(role, lim));
        }
    }
    if rs.as_object().is_some_and(|m| !m.is_empty()) {
        checks.push(Check::new(
            "roles",
            "ok",
            format!(
                "supervisor {}, worker {}",
                get_str(&rs, "orchestrator"),
                get_str(&rs, "worker")
            ),
            "",
        ));
    }
    match call_once(env, "identity.check", Some(json!({"session": false}))) {
        Ok(ident) if ferb_py::value::truthy(&ident) => {
            if !truthy(ident.get("profile")) {
                checks.push(Check::new(
                    "identity profile",
                    "warn",
                    "not set up",
                    "FERB's own account is set up in FERB.app → Settings → Browsers → FERB's own account",
                ));
            } else {
                let ok = truthy(ident.get("profile_ok"));
                checks.push(Check::new(
                    "identity profile",
                    if ok { "ok" } else { "fail" },
                    if ok {
                        format!(
                            "{} ({}) holds only {}",
                            get_str(&ident, "profile_name"),
                            get_str(&ident, "profile"),
                            get_str(&ident, "address")
                        )
                    } else {
                        get_str(&ident, "error")
                    },
                    "FERB's Chrome profile must hold only FERB's account; fix it in Chrome",
                ));
                if ok {
                    let paired = truthy(ident.get("paired"));
                    checks.push(Check::new(
                        "identity session",
                        "warn",
                        if paired {
                            "session not checked (observational doctor; no browser opened)"
                        } else {
                            "not paired; session not checked"
                        },
                        if paired { "" } else { "pair it from FERB.app" },
                    ));
                }
            }
            for sec in ident["secrets"].as_array().cloned().unwrap_or_default() {
                let key = get_str(&sec, "key");
                let there = truthy(sec.get("in_keychain"));
                checks.push(Check::new(
                    format!("secret {key}"),
                    if there { "ok" } else { "warn" },
                    if there {
                        "in Keychain"
                    } else {
                        "missing from Keychain"
                    },
                    format!(
                        "in your own terminal: security add-generic-password -s ferb -a {key} -w"
                    ),
                ));
            }
        }
        Ok(_) => {}
        Err(e) if soft(&e) => {
            checks.push(Check::new(
                "identity session",
                "warn",
                format!("couldn't check: {e}"),
                "run `ferb doctor` again",
            ));
        }
        Err(e) => return Err(e),
    }
    let hosts_dirs = if !env.inst.native_hosts_dirs.is_empty() {
        env.inst.native_hosts_dirs.clone()
    } else {
        match (env.browser_support.clone(), env.browsers.as_mut()) {
            (Some(support), Some(b)) => b.native_host_dirs(&support),
            _ => Vec::new(),
        }
    };
    let launcher = env.inst.native_host_launcher();
    for dir in hosts_dirs {
        let m = dir.join(NATIVE_HOST_MANIFEST);
        let parent = dir
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        checks.push(Check::new(
            format!("native host ({parent})"),
            if m.exists() && launcher.exists() {
                "ok"
            } else {
                "fail"
            },
            if m.exists() { s(&m) } else { "missing".into() },
            "run `ferb setup --update`",
        ));
    }
    let profiles = match call_once(env, "browsers.list", None) {
        Ok(r) => r["profiles"].as_array().cloned().unwrap_or_default(),
        Err(e) if soft(&e) => Vec::new(),
        Err(e) => return Err(e),
    };
    for p in profiles.iter().filter(|p| truthy(p.get("paired"))) {
        let name = format!("browser {}/{}", get_str(p, "browser"), get_str(p, "name"));
        if truthy(p.get("refused")) {
            checks.push(Check::new(
                name,
                "fail",
                get_str(p, "refused"),
                "reload the FERB extension in that profile",
            ));
        } else if !truthy(p.get("connected")) && p.get("identity") == Some(&json!("ferb")) {
            checks.push(Check::new(name, "ok", "FERB's own profile: paired, not open right now (FERB opens it when a task needs it)", ""));
        } else {
            let connected = truthy(p.get("connected"));
            checks.push(Check::new(
                name,
                if connected { "ok" } else { "warn" },
                if connected {
                    format!("connected, extension {}", get_str(p, "version"))
                } else {
                    "paired, not connected".into()
                },
                "open that profile; FERB reconnects within 30 s",
            ));
        }
    }
    if truthy(report.get("stopped")) {
        checks.push(Check::new(
            "emergency stop",
            "warn",
            "engaged",
            "FERB.app's menu lifts it",
        ));
    }
    let learning = report
        .get("learning")
        .filter(|l| ferb_py::value::truthy(l))
        .cloned()
        .unwrap_or(json!({}));
    for day in learning
        .get("undistilled")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        checks.push(Check::new(
            format!("learning {}", get_str(&day, "day")),
            "warn",
            format!(
                "not learned from yet ({} tries): {}",
                get_str(&day, "attempts"),
                get_str(&day, "error")
            ),
            "FERB tries that day again every hour; sign in to its AI service if it says signed out",
        ));
    }
    for card in learning
        .get("stuck_cards")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        checks.push(Check::new(
            format!("episode card {}", get_str(&card, "task_id")),
            "warn",
            format!("couldn't be built twice: {}", get_str(&card, "error")),
            "FERB tries it again when that task changes (a new verdict or a rebuilt episode)",
        ));
    }
    let sense = match call_once(env, "sense.status", None) {
        Ok(s) => Some(s),
        Err(e) if soft(&e) => None,
        Err(e) => return Err(e),
    };
    if let Some(sense) = sense {
        if truthy(sense.get("paused")) {
            checks.push(Check::new(
                "watchers",
                "warn",
                "paused: nothing is recorded, no real mouse / keyboard input",
                "FERB.app's menu resumes them",
            ));
        } else {
            let watchers: Vec<String> = sense["watchers"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(ferb_py::value::py_str)
                .collect();
            checks.push(Check::new(
                "watchers",
                "ok",
                if watchers.is_empty() {
                    "none running".into()
                } else {
                    watchers.join(", ")
                },
                "",
            ));
            if truthy(sense.get("owner_input_why")) {
                checks.push(Check::new(
                    "owner input",
                    "warn",
                    format!("{}: FERB can't tell whether you're using the Mac, so it makes no typing or clicking in front of you", get_str(&sense, "owner_input_why")),
                    "turn FERB on in Settings → Privacy & Security → Input Monitoring; FERB starts watching by itself within a minute",
                ));
            }
        }
        let jev = match call_once(env, "jev.stats", None) {
            Ok(j) => Some(j),
            Err(e) if soft(&e) => None,
            Err(e) => return Err(e),
        };
        if let Some(jev) = jev {
            if truthy(jev.get("configured")) {
                if truthy(jev.get("recent_total")) && jev.get("recent_answered") == Some(&json!(0))
                {
                    checks.push(Check::new(
                        "jev",
                        "warn",
                        format!(
                            "not answering ({}); FERB uses its own decisions",
                            get_str(&jev, "last_error")
                        ),
                        "check the Jev key in the Keychain",
                    ));
                } else {
                    let on: Vec<String> = jev["stats"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .filter(|s| truthy(s.get("enabled")))
                        .map(|s| get_str(s, "question"))
                        .collect();
                    let tail = if on.is_empty() {
                        String::new()
                    } else {
                        format!("; enabled: {}", on.join(", "))
                    };
                    checks.push(Check::new("jev", "ok", format!("shadow mode{tail}"), ""));
                }
            } else {
                checks.push(Check::new("jev", "ok", format!("off: activity notices are paused (SENSE-2); a key comes from {JEV_KEYS_URL}"), ""));
            }
        }
        if let Some(failed) = sense.get("failed").and_then(Value::as_object) {
            for (source, why) in failed {
                checks.push(Check::new(
                    format!("watcher {source}"),
                    "warn",
                    format!("failing, trying again: {}", ferb_py::value::py_str(why)),
                    "FERB retries it by itself; if it keeps failing, check its permission in Settings",
                ));
            }
        }
        for problem in sense["folder_problems"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            checks.push(Check::new(
                "watch folder",
                "warn",
                ferb_py::value::py_str(&problem),
                "fix [sense] watch_folders in ~/.ferb/config.toml",
            ));
        }
    }
    Ok(())
}

// ---------- uninstall ----------

/// setup.py:988: `uninstall [--purge [--yes]]`.
pub fn uninstall(env: &mut Env<'_>, purge: bool, confirmed: bool) -> Result<i32, CliError> {
    let home = env.inst.home.clone();
    let mut purge = purge;
    if purge {
        if !home.join(MARKER).exists() {
            env.say(&format!(
                "Refusing to purge {}: it has no {MARKER} marker.",
                home.display()
            ));
            return Ok(1);
        }
        if !confirmed {
            let answer = (env.ask)(&format!(
                "This deletes {} entirely (database, journal, config, FERB's Chrome profile) and every FERB item in your Keychain (API keys, saved secrets and cards, FERB's sign-ins). Type 'delete' to confirm: ",
                home.display()
            ));
            if crate::py::strip(&answer) != "delete" {
                env.say("Not purged.");
                purge = false;
            }
        }
    }
    let binary = env.inst.granted_binary();
    let requests = match (env.browser_support.clone(), env.browsers.as_mut()) {
        (Some(support), Some(b)) => b.remove_requests(&home, &support),
        _ => Vec::new(),
    };
    for path in &requests {
        env.say(&format!("removed {}", path.display()));
    }
    let removed = match env.inst.uninstall(purge) {
        Ok(r) => r,
        Err(e @ (InstallError::Install(_) | InstallError::SwitchedOff(_))) => {
            env.say(&e.to_string());
            return Ok(1);
        }
        Err(e) => return Err(e.into()),
    };
    for r in &removed {
        env.say(&format!("removed {r}"));
    }
    if removed.is_empty() && requests.is_empty() {
        env.say("Nothing to remove.");
    }
    if let Some(binary) = binary {
        let what = if env.inst.bundle.is_some() {
            format!(
                "FERB ({})",
                env.inst
                    .grant_target()
                    .map(|p| s(&p))
                    .unwrap_or_else(|| "None".into())
            )
        } else {
            s(&binary)
        };
        env.say(&format!(
            "macOS can't remove permissions for you: in System Settings → Privacy & Security, remove {what} from Accessibility, Screen Recording (and Input Monitoring / Automation if granted)."
        ));
    }
    Ok(0)
}

fn tail(text: &str, n: usize) -> String {
    crate::py::last_chars(crate::py::strip(text), n).to_string()
}

/// setup.py:1035: a checkout builds the notch app into FERB's home and signs it with the local certificate.
fn build_app(env: &mut Env<'_>) -> Result<Option<String>, CliError> {
    let found = env
        .run_quiet(&["security", "find-identity", "-p", "codesigning"], None)
        .map_err(CliError::Io)?;
    if !found.stdout.contains(CERTIFICATE) {
        env.say(&format!("The signing certificate \"{CERTIFICATE}\" isn't in your login keychain. Create it in Keychain Access →"));
        env.say("Certificate Assistant → Create a Certificate (name it exactly that, type Code Signing), then run this again.");
        return Ok(None);
    }
    let app = env.inst.home.join("FERB.app");
    env.say(
        "Building FERB.app (the notch app, which also turns an app's sound into text for hearing)…",
    );
    let notch = env.inst.sources.repo_root.join("ui/mac/FERBNotch");
    let built = env
        .run_quiet(&[&s(&notch.join("build.sh")), "release"], None)
        .map_err(CliError::Io)?;
    if built.code != 0 {
        let said = if built.stderr.is_empty() {
            &built.stdout
        } else {
            &built.stderr
        };
        env.say(&format!("Build failed:\n{}", tail(said, 1500)));
        return Ok(None);
    }
    let staged = env.inst.home.join(".FERB.app.new");
    let _ = std::fs::remove_dir_all(&staged);
    let copied = env
        .run_quiet(
            &[
                "/usr/bin/ditto",
                &s(&notch.join("build/FERB Notch.app")),
                &s(&staged),
            ],
            None,
        )
        .map_err(CliError::Io)?;
    if copied.code != 0 {
        let said = if copied.stderr.is_empty() {
            &copied.stdout
        } else {
            &copied.stderr
        };
        env.say(&format!("Copying the build failed:\n{}", tail(said, 1500)));
        let _ = std::fs::remove_dir_all(&staged);
        return Ok(None);
    }
    let signed = env
        .run_quiet(
            &[
                "/usr/bin/codesign",
                "--force",
                "--deep",
                "--sign",
                CERTIFICATE,
                &s(&staged),
            ],
            None,
        )
        .map_err(CliError::Io)?;
    if signed.code != 0 {
        let said = if signed.stderr.is_empty() {
            &signed.stdout
        } else {
            &signed.stderr
        };
        env.say(&format!("Signing failed:\n{}", tail(said, 1500)));
        let _ = std::fs::remove_dir_all(&staged);
        return Ok(None);
    }
    let _ = std::fs::remove_dir_all(&app);
    std::fs::rename(&staged, &app)?;
    let shown = env
        .run_quiet(&["/usr/bin/codesign", "-d", "-r-", &s(&app)], None)
        .map_err(CliError::Io)?;
    let req = designated_requirement(&shown.both());
    if !trusted_requirement(&req, false) {
        env.say("FERB.app isn't signed with the local certificate, so FERB won't use it.");
        return Ok(None);
    }
    Ok(Some(req))
}

/// setup.py:1075.
fn shipped_app(env: &mut Env<'_>) -> Result<Option<String>, CliError> {
    match env
        .inst
        .check_bundle()
        .and_then(|_| env.inst.own_requirement())
    {
        Ok(r) => Ok(Some(r)),
        Err(e @ (InstallError::Install(_) | InstallError::SwitchedOff(_))) => {
            env.say(&format!("FERB won't use FERB.app for hearing: {e}"));
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}

/// setup.py:1086 (L26): the app hearing runs, its signature recorded for the daemon's checks.
fn voice_helper(env: &mut Env<'_>) -> Result<Option<String>, CliError> {
    let req = if env.inst.bundle.is_some() {
        shipped_app(env)?
    } else {
        build_app(env)?
    };
    let Some(req) = req else {
        return Ok(None);
    };
    let config = env.config();
    config.set("hearing", "requirement", &json!(req))?;
    let voice = config.get("voice", "requirement")?;
    if voice.is_some_and(|v| v != json!(req)) {
        config.set("voice", "requirement", &json!(req))?;
    }
    Ok(Some(req))
}
