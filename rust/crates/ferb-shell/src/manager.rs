//! Task process lifetimes, reads, signals and cleanup (ferb/act/shell/manager.py). Commands are never rewritten or
//! interpreted here.
//!
//! Processes belong to the task that started them; a worker only sees its own task's. When the task ends its
//! processes' whole groups are killed unless started as long-lived (servers), even after the command's own process has
//! exited (children it left keep its group). Commands run through /bin/sh in a clean environment (no FERB tokens, no
//! API keys), always under FERB's sandbox: a `ShellManager` can't be built without one, and when the sandbox can't
//! run the daemon leaves the shell tools out (rule 11; there is no uncontained mode, unlike Python's test-only
//! `sandbox=None`). Each process gets its own credential for FERB's egress proxy, so what it reached is a recorded
//! fact; a finished process's record is reported once its last connection closes.
//!
//! Downloads (HAND-210, HAND-232): while any command runs, every WATCH_S the bytes each received through the proxy go
//! to `on_received`; when it answers why, that command's whole group is stopped and its results say so.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use ferb_core::types::EscapeGrant;
use serde_json::{json, Map, Value};

use crate::local::{report_tag, DirectReports};
use crate::protection::{LocalPort, ADMIN};
use crate::proxy::EgressProxy;
use crate::pty::{MembersOurs, PtyProc, RedactText, StreamFactory};
use crate::sandbox::{ProfileError, Sandbox};

pub const CLEAN_ENV_KEYS: [&str; 8] = [
    "PATH", "HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "SHELL", "TMPDIR",
];
/// How long shell.send keeps typing into a command that reads slowly.
pub const SEND_WAIT_S: f64 = 2.0;
/// How often the download budget and the free-disk floor are checked while a command runs.
pub const WATCH_S: f64 = 1.0;

/// The environment of a process FERB contains: no FERB tokens, no API keys (manager.py:52). `environ` reads the
/// daemon's environment; `with_cert` adds the sandbox's certificate bundle.
pub fn clean_env_from(
    environ: &dyn Fn(&str) -> Option<String>,
    with_cert: bool,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = CLEAN_ENV_KEYS
        .iter()
        .filter_map(|k| environ(k).map(|v| (k.to_string(), v)))
        .collect();
    // launchd gives the daemon a bare PATH; the owner's tools (brew, node, uv, gh) live in these
    let home = environ("HOME").unwrap_or_else(|| crate::pw_home().unwrap_or_default());
    let local_bin = format!("{home}/.local/bin");
    let extras = [
        "/opt/homebrew/bin",
        "/opt/homebrew/sbin",
        "/usr/local/bin",
        local_bin.as_str(),
    ];
    let path = env
        .iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| "/usr/bin:/bin:/usr/sbin:/sbin".to_string());
    let mut parts: Vec<String> = extras
        .iter()
        .filter(|e| Path::new(e).is_dir())
        .map(|e| e.to_string())
        .collect();
    parts.extend(path.split(':').map(str::to_string));
    let joined = crate::pyos::dedupe(parts).join(":");
    set(&mut env, "PATH", joined);
    set(&mut env, "TERM", "dumb".into());
    if with_cert {
        for (k, v) in Sandbox::env() {
            set(&mut env, &k, v);
        }
    }
    env
}

fn set(env: &mut Vec<(String, String)>, key: &str, value: String) {
    match env.iter_mut().find(|(k, _)| k == key) {
        Some(e) => e.1 = value,
        None => env.push((key.to_string(), value)),
    }
}

/// `clean_env_from` on the daemon's own environment.
pub fn clean_env(with_cert: bool) -> Vec<(String, String)> {
    clean_env_from(&|k| std::env::var(k).ok(), with_cert)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProcHandle {
    pub id: String,
    pub owner_task: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcInfo {
    pub id: String,
    pub owner_task: String,
    pub command: String,
    pub cwd: String,
    /// Wall-clock seconds.
    pub started: f64,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub long_lived: bool,
    /// It reached at least one host through FERB's proxy.
    pub network: bool,
    pub capture_error: Option<String>,
    pub sandboxed: bool,
    /// The one rule an approved escape lifted for this run (D31).
    pub lifted: Option<String>,
    pub read_path: Option<String>,
    pub sandbox_ms: Option<f64>,
    /// Owner-pinned local ports it may reach directly, as checked when it started (L24).
    pub direct: Vec<LocalPort>,
    /// FERB stopped it: why (the download budget or the free-disk floor).
    pub stopped: Option<String>,
}

/// Why a shell call didn't run: Refused is a safety check's words (safe to show a worker).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShellError {
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Profile(#[from] ProfileError),
    #[error("{0}")]
    Start(String),
}

pub type OnNetwork = Arc<dyn Fn(&ProcInfo, Map<String, Value>) + Send + Sync>;
pub type OnEscape = Arc<dyn Fn(&str, &EscapeGrant) + Send + Sync>;
pub type OnReceived = Arc<dyn Fn(&str, i64) -> Option<String> + Send + Sync>;
pub type Contained = Arc<dyn Fn(i32) -> bool + Send + Sync>;
pub type LocalRefusedFn = Arc<dyn Fn(&str) -> bool + Send + Sync>;
/// A built argv, its direct ports and the report tag it was built under.
type Built = Result<(Vec<String>, Vec<LocalPort>, Option<String>), ProfileError>;
type WallClock = Arc<dyn Fn() -> f64 + Send + Sync>;

/// What the daemon wires into the manager (manager.py:93's arguments). `contained` is `listeners::contained` over
/// the protection lists' FERB home and workspace; `now` the wall clock.
pub struct ManagerConfig {
    pub log_dir: PathBuf,
    pub redact_text: RedactText,
    pub stream_factory: StreamFactory,
    pub sandbox: Arc<Sandbox>,
    pub proxy: Option<Arc<EgressProxy>>,
    pub on_network: Option<OnNetwork>,
    /// Present when the daemon can take an admin command's hidden password.
    pub admin_ready: bool,
    pub on_escape: Option<OnEscape>,
    pub reports: Option<Arc<DirectReports>>,
    /// Whether a task's tree said No to one of this Mac's addresses.
    pub local_refused: Option<LocalRefusedFn>,
    pub contained: Contained,
    pub now: WallClock,
}

impl ManagerConfig {
    pub fn new(
        log_dir: PathBuf,
        redact_text: RedactText,
        stream_factory: StreamFactory,
        sandbox: Arc<Sandbox>,
    ) -> ManagerConfig {
        let prot = sandbox.protection.clone();
        ManagerConfig {
            log_dir,
            redact_text,
            stream_factory,
            sandbox,
            proxy: None,
            on_network: None,
            admin_ready: false,
            on_escape: None,
            reports: None,
            local_refused: None,
            contained: Arc::new(move |pid| {
                crate::listeners::contained(pid, prot.ferb_home(), prot.workspace())
            }),
            now: Arc::new(crate::wall_time),
        }
    }
}

#[derive(Default)]
struct State {
    /// Insertion-ordered, as Python's dict.
    procs: Vec<(String, Arc<PtyProc>, ProcInfo)>,
    owner_input: HashMap<String, String>,
    report_on_exit: HashSet<String>,
    retiring: HashSet<String>,
    received: HashMap<String, i64>,
    stopped: HashMap<String, String>,
    exit_told: HashSet<String>,
}

impl State {
    fn get(&self, id: &str) -> Option<&(String, Arc<PtyProc>, ProcInfo)> {
        self.procs.iter().find(|(k, _, _)| k == id)
    }
}

pub struct ShellManager {
    cfg: ManagerConfig,
    state: Mutex<State>,
    on_received: Mutex<Option<OnReceived>>,
    watcher: Mutex<Option<tokio::task::JoinHandle<()>>>,
    me: Weak<ShellManager>,
}

fn refused(s: &str) -> ShellError {
    ShellError::Refused(s.to_string())
}

impl ShellManager {
    /// The proxy's final records come back through `report` (Python sets `proxy.on_report = self._report`).
    pub fn new(cfg: ManagerConfig) -> Arc<ShellManager> {
        let m = Arc::new_cyclic(|me| ShellManager {
            cfg,
            state: Mutex::new(State::default()),
            on_received: Mutex::new(None),
            watcher: Mutex::new(None),
            me: me.clone(),
        });
        if let Some(proxy) = &m.cfg.proxy {
            let weak = Arc::downgrade(&m);
            proxy.set_on_report(Arc::new(move |proc_id: &str, facts: Map<String, Value>| {
                if let Some(m) = weak.upgrade() {
                    m.report(proc_id, facts);
                }
            }));
        }
        m
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Downloads (HAND-210, daemon supplied): (task, bytes received since the last look) -> why the command must stop.
    pub fn set_on_received(&self, f: Option<OnReceived>) {
        *self.on_received.lock().unwrap_or_else(|e| e.into_inner()) = f;
    }

    fn env(&self, proc_id: &str) -> Vec<(String, String)> {
        let mut env = clean_env(true);
        if let Some(proxy) = &self.cfg.proxy {
            for (k, v) in proxy.env(proc_id) {
                set(&mut env, &k, v);
            }
        }
        env
    }

    /// Run the command text through /bin/sh under the sandbox profile, with `lift`'s one rule lifted and the task's
    /// `folders` the only visible folders it may change (manager.py:130).
    #[allow(clippy::too_many_arguments)] // Python's signature: each is a separate fact of the call
    pub async fn start(
        &self,
        command: &str,
        cwd: &Path,
        owner_task: &str,
        long_lived: bool,
        lift: Option<&str>,
        escape: Option<&EscapeGrant>,
        folders: &[String],
    ) -> Result<ProcHandle, ShellError> {
        let pid = format!("p-{}", crate::token_hex(4));
        if let Some(esc) = escape {
            let resolved = crate::pyos::realpath(cwd).to_string_lossy().into_owned();
            if esc.command != command || esc.cwd != resolved || Some(esc.rule.as_str()) != lift {
                return Err(refused(
                    "the escape grant does not cover this exact command and working folder",
                ));
            }
        }
        if lift == Some(ADMIN)
            && !(escape.is_some_and(|e| e.secret.as_deref().is_some_and(|s| !s.is_empty()))
                && self.cfg.admin_ready)
        {
            return Err(refused(
                "admin requires the approved command's hidden password",
            ));
        }
        let rule = if lift == Some(ADMIN) { None } else { lift };
        let sandbox = self.cfg.sandbox.clone();
        let use_proxy = self.cfg.proxy.is_some()
            && !self
                .cfg
                .local_refused
                .as_ref()
                .is_some_and(|f| f(owner_task));
        let (argv, direct) = {
            let (command, folders, rule, proc_id) = (
                command.to_string(),
                folders.to_vec(),
                rule.map(str::to_string),
                pid.clone(),
            );
            let built = tokio::task::spawn_blocking(move || -> Built {
                // building the profile reads the sockets and runs sandbox_check: off the event loop
                if use_proxy {
                    let tag = report_tag(&proc_id); // in its profile only: the command can't read it there
                    let (argv, direct) =
                        sandbox.contain(&command, &tag, rule.as_deref(), &folders)?;
                    Ok((argv, direct, Some(tag)))
                } else {
                    Ok((
                        sandbox.argv(&command, rule.as_deref(), &folders)?,
                        Vec::new(),
                        None,
                    ))
                }
            })
            .await
            .map_err(|e| ShellError::Start(e.to_string()))??;
            if let (Some(tag), false, Some(reports)) =
                (&built.2, built.1.is_empty(), &self.cfg.reports)
            {
                reports.expect(&pid, tag);
            }
            (built.0, built.1)
        };
        let profile = Sandbox::profile_path(&argv);
        let secret = if lift == Some(ADMIN) {
            escape.and_then(|e| e.secret.clone())
        } else {
            None
        };
        let me = self.me.clone();
        let finished_id = pid.clone();
        let profile_for_exit = profile.clone();
        let finished = move || {
            Sandbox::cleanup(profile_for_exit.as_deref());
            if let Some(m) = me.upgrade() {
                let due = m.lock().report_on_exit.remove(&finished_id);
                if due {
                    m.retire(&finished_id); // outlived its task: its record is due now, then it goes
                }
            }
        };
        let p = PtyProc::new(
            argv,
            cwd.to_path_buf(),
            self.env(&pid),
            self.cfg.log_dir.join(format!("{pid}.log")),
            self.cfg.redact_text.clone(),
            self.cfg.stream_factory.clone(),
            Some(Box::new(finished)),
            secret,
        );
        let entry = lift.and_then(|l| sandbox_entry(&self.cfg.sandbox, l));
        let info = ProcInfo {
            id: pid.clone(),
            owner_task: owner_task.to_string(),
            command: command.to_string(),
            cwd: cwd.to_string_lossy().into_owned(),
            started: (self.cfg.now)(),
            running: true,
            exit_code: None,
            long_lived,
            network: false,
            capture_error: None,
            sandboxed: true,
            lifted: lift.map(str::to_string),
            read_path: entry.filter(|e| e.0.starts_with("read:")).map(|e| e.1),
            sandbox_ms: None,
            direct: direct.clone(),
            stopped: None,
        };
        self.lock().procs.push((pid.clone(), p.clone(), info));
        if let Err(e) = p.start().await {
            Sandbox::cleanup(profile.as_deref());
            self.lock().procs.retain(|(k, _, _)| *k != pid);
            if let Some(proxy) = &self.cfg.proxy {
                proxy.forget(&pid);
            }
            if !direct.is_empty() {
                if let Some(r) = &self.cfg.reports {
                    r.forget(&pid); // it never ran: no reports to expect under its tag
                }
            }
            return Err(match e {
                crate::pty::StartError::Refused(s) => ShellError::Refused(s),
                crate::pty::StartError::Io(s) => ShellError::Start(s),
            });
        }
        if let (Some(esc), Some(f)) = (escape, &self.cfg.on_escape) {
            f(owner_task, esc);
        }
        self.watch_downloads();
        Ok(ProcHandle {
            id: pid,
            owner_task: owner_task.to_string(),
        })
    }

    // ---------- downloads: the task tree's budget and the free-disk floor (HAND-210) ----------

    fn watch_downloads(&self) {
        let has = self
            .on_received
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        if !has || self.cfg.proxy.is_none() {
            return;
        }
        let mut w = self.watcher.lock().unwrap_or_else(|e| e.into_inner());
        if w.as_ref().is_some_and(|h| !h.is_finished()) {
            return;
        }
        let me = self.me.clone();
        *w = Some(tokio::spawn(async move {
            loop {
                if me.upgrade().is_none_or(|m| m.lock().procs.is_empty()) {
                    return;
                }
                tokio::time::sleep(Duration::from_secs_f64(WATCH_S)).await;
                let Some(m) = me.upgrade() else { return };
                let procs: Vec<(String, Arc<PtyProc>, String)> = m
                    .lock()
                    .procs
                    .iter()
                    .map(|(k, p, i)| (k.clone(), p.clone(), i.owner_task.clone()))
                    .collect();
                for (pid, p, task) in procs {
                    let why = m.count_received(&pid, &task);
                    if let Some(why) = why {
                        let first = {
                            let mut st = m.lock();
                            if st.stopped.contains_key(&pid) || p.exit_code().is_some() {
                                false
                            } else {
                                st.stopped.insert(pid.clone(), why);
                                true
                            }
                        };
                        if first {
                            p.kill(Some(m.ours(&pid))).await; // it may end meanwhile; `stopped` still says why
                        }
                    }
                }
            }
        }));
    }

    fn received_total(facts: &Map<String, Value>) -> i64 {
        facts
            .get("destinations")
            .and_then(Value::as_array)
            .map(|d| {
                d.iter()
                    .map(|x| x.get("bytes_in").and_then(Value::as_i64).unwrap_or(0))
                    .sum()
            })
            .unwrap_or(0)
    }

    /// The bytes this process received through the proxy since the last look go to `on_received`; its answer only
    /// when it received some. A broken count stops nothing.
    fn count_received(&self, pid: &str, task: &str) -> Option<String> {
        let f = self
            .on_received
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()?;
        let proxy = self.cfg.proxy.as_ref()?;
        let got = Self::received_total(&proxy.facts(pid));
        let new = {
            let mut st = self.lock();
            let new = got - st.received.get(pid).copied().unwrap_or(0);
            if new <= 0 {
                return None;
            }
            st.received.insert(pid.to_string(), got);
            new
        };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(task, new))).unwrap_or(None)
    }

    /// A tool result showed its task that this command ended, so wait.for doesn't wake on that end again (ORCH-639).
    pub fn exit_told(&self, h: &ProcHandle) {
        let mut st = self.lock();
        if st
            .get(&h.id)
            .is_some_and(|(_, _, i)| i.owner_task == h.owner_task)
        {
            st.exit_told.insert(h.id.clone());
        }
    }

    pub fn told(&self, proc_id: &str) -> bool {
        self.lock().exit_told.contains(proc_id)
    }

    pub fn stopped(&self, proc_id: &str) -> Option<String> {
        self.lock().stopped.get(proc_id).cloned()
    }

    fn get(&self, h: &ProcHandle) -> Result<Arc<PtyProc>, ShellError> {
        let st = self.lock();
        match st.get(&h.id) {
            Some((_, p, info)) if info.owner_task == h.owner_task => Ok(p.clone()),
            _ => Err(ShellError::Refused(format!(
                "no process {} for this task",
                h.id
            ))),
        }
    }

    /// (text, the offset after it, total bytes so far): page forward with `since` = the returned offset.
    pub async fn read(
        &self,
        h: &ProcHandle,
        since: i64,
        limit: Option<u64>,
    ) -> Result<(String, u64, u64), ShellError> {
        let p = self.get(h)?;
        let (text, offset) = p.read(since, limit);
        Ok((text, offset, p.total()))
    }

    /// Type `data` into the command's terminal: (bytes that went in, bytes given) (manager.py:250).
    pub async fn send(
        &self,
        h: &ProcHandle,
        data: &str,
        wait_s: f64,
    ) -> Result<(usize, usize), ShellError> {
        if self.lock().owner_input.contains_key(&h.id) {
            return Err(refused(
                "You have taken over this process. Worker input is paused until you detach.",
            ));
        }
        let p = self.get(h)?;
        let lifted = self
            .lock()
            .get(&h.id)
            .and_then(|(_, _, i)| i.lifted.clone());
        if let Some(lifted) = lifted {
            return Err(ShellError::Refused(format!(
                "This command runs with the owner's one-time lift of {lifted}; FERB doesn't type into it, because the \
                 owner's Yes covered that exact command, not later input. Put the input in the approved command \
                 itself (a here-document or a file), and ask again if it needs the lift."
            )));
        }
        let raw = data.as_bytes();
        if let Some(limit) = p.line_limit() {
            if line_too_long(raw, limit) {
                return Err(ShellError::Refused(format!(
                    "A line longer than {} bytes can't go to this command: its terminal reads line by line and drops \
                     a longer line whole. Nothing was sent. Write the text to a file in the workspace and pass the \
                     file, or send shorter lines.",
                    limit - 1
                )));
            }
        }
        let deadline = crate::monotonic() + wait_s;
        let mut sent = 0usize;
        loop {
            sent += p
                .write(&raw[sent..])
                .map_err(|e| ShellError::Start(crate::py_os_error(&e)))?;
            if sent >= raw.len() || p.exit_code().is_some() || crate::monotonic() >= deadline {
                return Ok((sent, raw.len()));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    pub fn take_input(&self, proc_id: &str) -> Result<String, ShellError> {
        let mut st = self.lock();
        let running = st
            .get(proc_id)
            .is_some_and(|(_, p, _)| p.exit_code().is_none());
        if !running {
            return Err(refused("That process is not running."));
        }
        if st.owner_input.contains_key(proc_id) {
            return Err(refused(
                "Another attachment already owns input to this process.",
            ));
        }
        let lease = crate::proxy::token_urlsafe(32);
        st.owner_input.insert(proc_id.to_string(), lease.clone());
        Ok(lease)
    }

    /// The owner's own typing into an attached process: what the terminal took (bytes).
    pub fn owner_send(&self, proc_id: &str, lease: &str, data: &str) -> Result<usize, ShellError> {
        let p = {
            let st = self.lock();
            let held = st.owner_input.get(proc_id).cloned().unwrap_or_default();
            if lease.is_empty() || !crate::constant_eq(held.as_bytes(), lease.as_bytes()) {
                return Err(refused("This attachment does not own process input."));
            }
            st.get(proc_id).map(|(_, p, _)| p.clone())
        };
        let p = p.ok_or_else(|| refused("This attachment does not own process input."))?;
        p.send(data)
            .map_err(|e| ShellError::Start(crate::py_os_error(&e)))
    }

    pub fn release_input(&self, proc_id: &str, lease: &str) {
        let mut st = self.lock();
        let held = st.owner_input.get(proc_id).cloned().unwrap_or_default();
        if !lease.is_empty() && crate::constant_eq(held.as_bytes(), lease.as_bytes()) {
            st.owner_input.remove(proc_id);
        }
    }

    pub async fn tail(&self, h: &ProcHandle, limit: u64) -> Result<(String, u64, u64), ShellError> {
        let p = self.get(h)?;
        let total = p.total();
        let (text, offset) = p.read(total.saturating_sub(limit) as i64, Some(limit));
        Ok((text, offset, p.total()))
    }

    pub async fn wait(&self, h: &ProcHandle, timeout: f64) -> Result<Option<i32>, ShellError> {
        Ok(self.get(h)?.wait(timeout).await)
    }

    pub async fn kill(&self, h: &ProcHandle) -> Result<(), ShellError> {
        let p = self.get(h)?;
        p.kill(Some(self.ours(&h.id))).await;
        Ok(())
    }

    /// For killing a group whose first process has ended: every process now in it runs inside FERB's sandbox, and
    /// its id isn't another running command's own (manager.py:309).
    pub fn ours(&self, proc_id: &str) -> MembersOurs {
        let st = self.lock();
        let leader = st.get(proc_id).and_then(|(_, p, _)| p.pid());
        let others: HashSet<i32> = st
            .procs
            .iter()
            .filter(|(k, p, _)| k != proc_id && p.exit_code().is_none())
            .filter_map(|(_, p, _)| p.pid())
            .collect();
        let contained = self.cfg.contained.clone();
        Arc::new(move |members: &[i32]| match leader {
            None => false,
            Some(l) if others.contains(&l) => false,
            Some(_) => members.iter().all(|m| contained(*m)),
        })
    }

    /// What this process reached through FERB's proxy and what FERB refused it, so far.
    pub fn network(&self, h: &ProcHandle) -> Result<Map<String, Value>, ShellError> {
        self.get(h)?;
        Ok(self.facts(&h.id))
    }

    fn facts(&self, proc_id: &str) -> Map<String, Value> {
        let mut facts = match &self.cfg.proxy {
            Some(p) => p.facts(proc_id),
            None => {
                let mut m = Map::new();
                m.insert("destinations".into(), json!([]));
                m.insert("refused".into(), json!([]));
                m
            }
        };
        let info = self.lock().get(proc_id).map(|(_, _, i)| i.clone());
        if let Some(info) = info.filter(|i| !i.direct.is_empty()) {
            facts.insert("local_direct".into(), Value::Array(self.direct(&info)));
        }
        facts
    }

    /// Each pinned local port this process may reach directly, with the connections the kernel reported so far;
    /// None for both when the reports couldn't be read (manager.py:340).
    fn direct(&self, info: &ProcInfo) -> Vec<Value> {
        let seen = self
            .cfg
            .reports
            .as_ref()
            .and_then(|r| r.connections(&info.id, info.started));
        info.direct
            .iter()
            .map(|d| {
                let mut m = Map::new();
                m.insert("host".into(), json!("localhost"));
                m.insert("port".into(), json!(d.port));
                m.insert("executable".into(), json!(d.executable.to_string_lossy()));
                m.insert("local".into(), json!(true));
                m.insert("direct".into(), json!(true));
                match &seen {
                    None => {
                        m.insert("reported_connections".into(), Value::Null);
                        m.insert("clients".into(), Value::Null);
                    }
                    Some(seen) => match seen.get(&(d.port as i128)) {
                        Some(found) => {
                            for (k, v) in found {
                                m.insert(k.clone(), v.clone());
                            }
                        }
                        None => {
                            m.insert("reported_connections".into(), json!(0));
                            m.insert("clients".into(), json!([]));
                        }
                    },
                }
                Value::Object(m)
            })
            .collect()
    }

    pub fn list(&self) -> Vec<ProcInfo> {
        let snapshot: Vec<(Arc<PtyProc>, ProcInfo, Option<String>)> = {
            let st = self.lock();
            st.procs
                .iter()
                .map(|(k, p, i)| (p.clone(), i.clone(), st.stopped.get(k).cloned()))
                .collect()
        };
        snapshot
            .into_iter()
            .map(|(p, info, stopped)| {
                let facts = self.facts(&info.id);
                let network = facts
                    .get("destinations")
                    .and_then(Value::as_array)
                    .is_some_and(|d| !d.is_empty());
                ProcInfo {
                    running: p.exit_code().is_none(),
                    exit_code: p.exit_code(),
                    capture_error: p.capture_error(),
                    sandbox_ms: if info.sandboxed { p.ready_ms() } else { None },
                    network,
                    stopped,
                    ..info
                }
            })
            .collect()
    }

    pub fn info(&self, h: &ProcHandle) -> Result<ProcInfo, ShellError> {
        self.get(h)?;
        self.list()
            .into_iter()
            .find(|p| p.id == h.id)
            .ok_or_else(|| ShellError::Refused(format!("no process {} for this task", h.id)))
    }

    pub fn handle(&self, proc_id: &str, owner_task: &str) -> ProcHandle {
        ProcHandle {
            id: proc_id.to_string(),
            owner_task: owner_task.to_string(),
        }
    }

    pub fn owner_of(&self, proc_id: &str) -> Option<String> {
        self.lock()
            .get(proc_id)
            .map(|(_, _, i)| i.owner_task.clone())
    }

    /// The task of a process still running (the gateway's network gate, L2); None once it has ended.
    pub fn live_owner(&self, proc_id: &str) -> Option<String> {
        self.lock()
            .get(proc_id)
            .filter(|(_, p, _)| p.exit_code().is_none())
            .map(|(_, _, i)| i.owner_task.clone())
    }

    pub async fn kill_id(&self, proc_id: &str) -> bool {
        let p = self.lock().get(proc_id).map(|(_, p, _)| p.clone());
        match p {
            None => false,
            Some(p) => {
                p.kill(Some(self.ours(proc_id))).await;
                true
            }
        }
    }

    fn procs_of(&self, which: impl Fn(&ProcInfo) -> bool) -> Vec<(String, Arc<PtyProc>, ProcInfo)> {
        self.lock()
            .procs
            .iter()
            .filter(|(_, _, i)| which(i))
            .cloned()
            .collect()
    }

    /// Task finished: kill its processes' whole groups except long-lived ones, whether or not the command's own
    /// process has ended; report and drop the finished ones. Returns how many were still running.
    pub async fn end_task(&self, task_id: &str) -> usize {
        let mut n = 0;
        for (pid, p, _) in self.procs_of(|i| i.owner_task == task_id && !i.long_lived) {
            n += p.exit_code().is_none() as usize;
            p.kill(Some(self.ours(&pid))).await;
        }
        self.settle(|i| i.owner_task == task_id);
        n
    }

    /// A finished command whose record its task no longer needs: its group ended as its task's end would, then it
    /// is retired. A command still running, or another task's, is left alone.
    pub async fn release(&self, h: &ProcHandle) {
        let hit = self.lock().get(&h.id).cloned();
        let Some((_, p, info)) = hit else { return };
        if info.owner_task != h.owner_task || p.exit_code().is_none() || info.long_lived {
            return;
        }
        p.kill(Some(self.ours(&h.id))).await;
        self.retire(&h.id);
    }

    /// A task paused (ORCH-312): the still-running commands of the tasks `owned` names stop, except long-lived
    /// ones; their records stay. Returns how many were stopped.
    pub async fn stop_running(&self, owned: impl Fn(&str) -> bool) -> usize {
        let mut n = 0;
        for (pid, p, _) in self.procs_of(|i| owned(&i.owner_task) && !i.long_lived) {
            if p.exit_code().is_none() {
                n += 1;
                p.kill(Some(self.ours(&pid))).await;
            }
        }
        n
    }

    pub async fn kill_all(&self) -> usize {
        if let Some(w) = self
            .watcher
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            w.abort();
        }
        let mut n = 0;
        for (pid, p, _) in self.procs_of(|_| true) {
            n += p.exit_code().is_none() as usize;
            p.kill(Some(self.ours(&pid))).await;
        }
        self.settle(|_| true);
        n
    }

    /// Processes whose task ended: a finished one is retired now; a running one is retired when it exits.
    fn settle(&self, which: impl Fn(&ProcInfo) -> bool) {
        for info in self.list() {
            if which(&info) && info.running {
                self.lock().report_on_exit.insert(info.id.clone());
            } else if which(&info) {
                self.retire(&info.id);
            }
        }
    }

    /// A finished process whose task ended: its credential is forgotten and its entry goes once its network record
    /// is out (manager.py:434).
    fn retire(&self, proc_id: &str) {
        if let Some(proxy) = &self.cfg.proxy {
            proxy.forget(proc_id);
            if proxy.pending(proc_id) {
                self.lock().retiring.insert(proc_id.to_string());
                return;
            }
        }
        self.drop_entry(proc_id);
    }

    fn drop_entry(&self, proc_id: &str) {
        let mut st = self.lock();
        st.retiring.remove(proc_id);
        st.received.remove(proc_id);
        st.stopped.remove(proc_id);
        st.exit_told.remove(proc_id);
        st.owner_input.remove(proc_id);
        st.procs.retain(|(k, _, _)| k != proc_id);
    }

    /// Output logs last written before `before` (Unix time), of processes FERB no longer tracks. Returns how many.
    pub fn purge_logs(&self, before: f64) -> usize {
        let tracked: HashSet<String> = self
            .lock()
            .procs
            .iter()
            .map(|(k, _, _)| k.clone())
            .collect();
        let Ok(entries) = std::fs::read_dir(&self.cfg.log_dir) else {
            return 0;
        };
        let mut n = 0;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".log") else {
                continue;
            };
            if tracked.contains(stem) {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64())
                .unwrap_or(f64::MAX);
            if meta.is_file() && mtime < before && std::fs::remove_file(entry.path()).is_ok() {
                n += 1;
            }
        }
        n
    }

    /// The proxy's final record for a forgotten process (from any thread: a tunnel may close last).
    pub fn report(&self, proc_id: &str, facts: Map<String, Value>) {
        let (hit, retiring) = {
            let st = self.lock();
            (
                st.get(proc_id).map(|(_, _, i)| i.clone()),
                st.retiring.contains(proc_id),
            )
        };
        let Some(info) = hit.filter(|_| self.cfg.on_network.is_some()) else {
            if retiring {
                self.drop_entry(proc_id);
            }
            return;
        };
        // the bytes it received since the watch last looked still count
        if let Some(count) = self
            .on_received
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            let got = Self::received_total(&facts);
            let new = {
                let mut st = self.lock();
                let new = got - st.received.get(proc_id).copied().unwrap_or(0);
                if new > 0 {
                    st.received.insert(proc_id.to_string(), got);
                }
                new
            };
            if new > 0 {
                let task = info.owner_task.clone();
                let _ =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| count(&task, new)));
            }
        }
        let used: Vec<Value> = self
            .direct(&info)
            .into_iter()
            .filter(|d| {
                d.get("reported_connections")
                    .is_some_and(|r| r.is_null() || r.as_i64().unwrap_or(0) > 0)
            })
            .collect();
        if !info.direct.is_empty() {
            if let Some(r) = &self.cfg.reports {
                r.forget(proc_id);
            }
        }
        let mut facts = facts;
        if !used.is_empty() {
            facts.insert("local_direct".into(), Value::Array(used.clone()));
        }
        let any = |k: &str| {
            facts
                .get(k)
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
        };
        if any("destinations") || any("refused") || !used.is_empty() {
            if let Some(f) = &self.cfg.on_network {
                f(&info, facts.clone());
            }
        }
        if retiring {
            self.drop_entry(proc_id); // its record was what its entry waited for
        }
    }

    /// Test seam: whether an entry is still tracked.
    pub fn tracked(&self, proc_id: &str) -> bool {
        self.lock().get(proc_id).is_some()
    }

    /// Test seam: the PTY process of an entry.
    pub fn process(&self, proc_id: &str) -> Option<Arc<PtyProc>> {
        self.lock().get(proc_id).map(|(_, p, _)| p.clone())
    }
}

fn sandbox_entry(sandbox: &Sandbox, lift: &str) -> Option<(String, String)> {
    sandbox
        .protection
        .entry(lift)
        .map(|e| (e.id.clone(), e.path.to_string_lossy().into_owned()))
}

/// manager.py:266: a line longer than the terminal takes while it reads lines (`limit` bytes with its newline).
pub fn line_too_long(raw: &[u8], limit: i64) -> bool {
    let parts: Vec<&[u8]> = raw.split(|&b| b == b'\n').collect();
    let (full, last) = parts.split_at(parts.len() - 1);
    full.iter().any(|line| line.len() as i64 + 1 > limit) || (last[0].len() as i64) > limit - 1
}
