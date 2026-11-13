//! Local destinations checked against the owner's executable-pinned ports and FERB's containment
//! (ferb/act/shell/local.py; S2, D28, owner Q1, L24). No command or content is inspected: the listener and the
//! connected TCP peer are kernel socket facts and proc_pidpath names their executable. Unknown identities fail closed.
//!
//! Two routes reach a pinned port: through the proxy (`LocalAccess`: checked before dialling and again against the
//! exact connected peer before a byte is sent), and directly (`direct_ports`, when a shell command's profile is
//! built, with the kernel reporting each connection under the command's own tag: `DirectReports`).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};

use crate::protection::{LocalPort, Protection};
use crate::pynet::ip_address;
use crate::pyos::{int_str, strip};

pub const LOG: &str = "/usr/bin/log";
/// How long the proxy waits for a pinned or contained server to accept a connection it has queued (HAND-227).
pub const ACCEPT_WAIT: f64 = 30.0;
/// HAND-433: a visible listener with nothing queued and no held socket is read this many more times.
pub const SETTLE_READS: u32 = 3;

/// The kernel's own reports (process 0) that carry a FERB tag; a program can't log as the kernel.
pub fn stream_argv() -> Vec<String> {
    [
        LOG,
        "stream",
        "--style",
        "ndjson",
        "--predicate",
        "processID == 0 AND eventMessage CONTAINS \"ferb p-\"",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// The message a command's direct allow rule gives the kernel's reports: its process id and 128 random bits.
pub fn report_tag(process: &str) -> String {
    format!("ferb {process} {}", crate::token_hex(16))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct LocalRefused(pub String);

impl LocalRefused {
    fn new(s: &str) -> LocalRefused {
        LocalRefused(s.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpSocket {
    pub local: (String, u32),
    pub remote: Option<(String, u32)>,
    pub state: String,
    /// None: a process FERB can't inspect (another user's, root's).
    pub pid: Option<i32>,
    /// A listener's connections no process has accepted yet.
    pub queued: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPeer {
    pub address: String,
    pub executable: String,
}

/// Normalize mapped IPv4 and the numeric part of a scoped IPv6 address (local.py `_ip`); None where Python raises.
pub fn ip_norm(address: &str) -> Option<String> {
    let ip = ip_address(address.split('%').next().unwrap_or(""))?;
    Some(ip.unmapped().text())
}

/// The kernel facts the local checks read (a seam: tests script them; `MacFacts` reads this Mac).
pub trait SocketFacts: Send + Sync {
    /// This Mac's TCP sockets: the inspectable ones, or with `complete` every user's (others' with pid None).
    fn tcp_snapshot(&self, complete: bool) -> Option<Vec<TcpSocket>>;
    fn executable_of(&self, pid: i32) -> Option<PathBuf>;
    fn contained(&self, pid: i32, ferb_home: &Path, workspace: &Path) -> bool;
    /// Seconds for the accept wait.
    fn monotonic(&self) -> f64;
    fn sleep(&self, seconds: f64);
}

/// This Mac's facts: libproc, nettop, sandbox_check, the real clock.
pub struct MacFacts;

impl SocketFacts for MacFacts {
    fn tcp_snapshot(&self, complete: bool) -> Option<Vec<TcpSocket>> {
        let found = if complete {
            crate::sockets::complete()?
        } else {
            crate::sockets::inspectable()?.0
        };
        Some(
            found
                .into_iter()
                .filter(|s| s.proto == "tcp")
                .map(|s| TcpSocket {
                    local: s.local,
                    remote: s.remote,
                    state: s.state,
                    pid: s.pid,
                    queued: s.queued,
                })
                .collect(),
        )
    }

    fn executable_of(&self, pid: i32) -> Option<PathBuf> {
        crate::sockets::executable_of(pid)
    }

    fn contained(&self, pid: i32, ferb_home: &Path, workspace: &Path) -> bool {
        crate::listeners::contained(pid, ferb_home, workspace)
    }

    fn monotonic(&self) -> f64 {
        crate::monotonic()
    }

    fn sleep(&self, seconds: f64) {
        std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
    }
}

pub struct LocalAccess {
    protection: Arc<dyn Protection>,
    pins: HashMap<u32, PathBuf>,
    facts: Arc<dyn SocketFacts>,
}

impl LocalAccess {
    pub fn new(protection: Arc<dyn Protection>, facts: Arc<dyn SocketFacts>) -> LocalAccess {
        let pins = protection
            .local_ports()
            .iter()
            .map(|p| (p.port as u32, p.executable.clone()))
            .collect();
        LocalAccess {
            protection,
            pins,
            facts,
        }
    }

    fn sockets(&self) -> Result<Vec<TcpSocket>, LocalRefused> {
        self.facts
            .tcp_snapshot(false)
            .ok_or_else(|| LocalRefused::new("FERB couldn't inspect the local TCP sockets"))
    }

    fn verified(&self, rows: &[TcpSocket], port: u32) -> Result<PathBuf, LocalRefused> {
        if rows.is_empty() || rows.iter().any(|r| r.pid.is_none()) {
            return Err(LocalRefused::new(
                "FERB couldn't identify the local listener or connected peer",
            ));
        }
        let expected = self.pins.get(&port);
        let mut executable = PathBuf::new();
        for row in rows {
            let pid = row.pid.expect("checked above");
            executable = self.facts.executable_of(pid).ok_or_else(|| {
                LocalRefused::new("FERB couldn't identify the local server's executable")
            })?;
            match expected {
                Some(expected) => {
                    if &executable != expected {
                        return Err(LocalRefused::new(
                            "the local server's executable does not match protection.local_ports",
                        ));
                    }
                }
                None => {
                    if !self.facts.contained(
                        pid,
                        self.protection.ferb_home(),
                        self.protection.workspace(),
                    ) {
                        return Err(LocalRefused::new(
                            "this local port is not in protection.local_ports and its server is outside FERB",
                        ));
                    }
                }
            }
        }
        Ok(executable)
    }

    /// Before dialling: every listener on the port (this address or any) runs its pin, or FERB's containment.
    pub fn before_connect(&self, target: (&str, u32)) -> Result<(), LocalRefused> {
        let address = ip_norm(target.0).unwrap_or_else(|| target.0.to_string());
        let port = target.1;
        let rows: Vec<TcpSocket> = self
            .sockets()?
            .into_iter()
            .filter(|r| {
                r.state == "LISTEN"
                    && r.local.1 == port
                    && [address.as_str(), "*", "0.0.0.0", "::"].contains(&r.local.0.as_str())
            })
            .collect();
        self.verified(&rows, port).map(|_| ())
    }

    /// The server end of this exact connected socket, not whichever listener now holds the port (local.py:170).
    pub fn connected(
        &self,
        target: (&str, u32),
        client: (&str, u32),
    ) -> Result<LocalPeer, LocalRefused> {
        let server = (
            ip_norm(target.0).unwrap_or_else(|| target.0.to_string()),
            target.1,
        );
        let source = (
            ip_norm(client.0).unwrap_or_else(|| client.0.to_string()),
            client.1,
        );
        let deadline = self.facts.monotonic() + ACCEPT_WAIT;
        let mut pause = 0.001f64;
        let mut settle = SETTLE_READS;
        let held = loop {
            let rows = self.sockets()?;
            let held: Vec<TcpSocket> = rows
                .iter()
                .filter(|r| {
                    r.state == "ESTABLISHED"
                        && r.local == server
                        && r.remote.as_ref() == Some(&source)
                })
                .cloned()
                .collect();
            let listening: Vec<&TcpSocket> = rows
                .iter()
                .filter(|r| {
                    r.state == "LISTEN"
                        && r.local.1 == server.1
                        && (r.local.0 == server.0 || r.local.0 == "*")
                })
                .collect();
            if !held.is_empty() || self.facts.monotonic() >= deadline {
                break held;
            }
            if !listening.iter().any(|r| r.queued > 0) {
                if listening.is_empty() || settle == 0 {
                    break held; // no visible listener: a server FERB can't see never shows it
                }
                settle -= 1; // HAND-433: a reading taken mid-accept; read again
            }
            self.facts.sleep(pause);
            pause = (pause * 2.0).min(0.05);
        };
        let executable = self.verified(&held, target.1)?;
        Ok(LocalPeer {
            address: server.0,
            executable: executable.to_string_lossy().into_owned(),
        })
    }
}

/// The owner-pinned ports a shell command starting now may reach directly: each port whose every listening TCP socket
/// belongs to a process running its pinned executable, every user's sockets read (local.py:195).
pub fn direct_ports(protection: &dyn Protection, facts: &dyn SocketFacts) -> Vec<LocalPort> {
    if protection.local_ports().is_empty() {
        return Vec::new();
    }
    let Some(rows) = facts.tcp_snapshot(true) else {
        return Vec::new();
    };
    let mut allowed = Vec::new();
    for pin in protection.local_ports() {
        let pids: Vec<Option<i32>> = rows
            .iter()
            .filter(|r| r.state == "LISTEN" && r.local.1 == pin.port as u32)
            .map(|r| r.pid)
            .collect();
        if !pids.is_empty()
            && pids.iter().all(|pid| {
                pid.is_some_and(|p| facts.executable_of(p).as_ref() == Some(&pin.executable))
            })
        {
            allowed.push(pin.clone());
        }
    }
    allowed
}

/// `direct_ports` on this Mac now.
pub fn direct_ports_now(protection: &dyn Protection) -> Vec<LocalPort> {
    direct_ports(protection, &MacFacts)
}

/// One kernel allow report (local.py `_REPORT`, fullmatched on the stripped message): (repeats, program, port, tag).
/// The program can't hold parentheses or a line break, the address no whitespace or parentheses, and the message is
/// a whole tag, so a deny report (whose path a program chooses) never parses as one.
pub fn parse_report(message: &str) -> Option<(Option<i128>, String, i128, String)> {
    let s = strip(message);
    let (repeats, rest) = match s.split_once(" duplicate report") {
        Some((n, after))
            if !n.is_empty()
                && n.chars().all(ferb_py::unicode::is_decimal)
                && (after.starts_with(" for Sandbox: ")
                    || after.starts_with("s for Sandbox: ")) =>
        {
            let after = after.strip_prefix('s').unwrap_or(after);
            (Some(int_str(n)?), after.strip_prefix(" for ")?)
        }
        _ => (None, s),
    };
    let rest = rest.strip_prefix("Sandbox: ")?;
    let open = rest.find('(')?;
    let program = &rest[..open];
    if program.is_empty() || program.contains([')', '\n']) {
        return None;
    }
    let rest = &rest[open + 1..];
    let close = rest.find(')')?;
    let pid = &rest[..close];
    if pid.is_empty() || !pid.chars().all(ferb_py::unicode::is_decimal) {
        return None;
    }
    let rest = rest[close + 1..].strip_prefix(" allow network-outbound ")?;
    let nl = rest.find('\n')?;
    let (address, tag) = (&rest[..nl], &rest[nl + 1..]);
    if address
        .chars()
        .any(|c| crate::pyos::is_space(c) || c == '(' || c == ')')
    {
        return None;
    }
    let colon = address.rfind(':')?;
    let port = &address[colon + 1..];
    if port.is_empty() || !port.chars().all(ferb_py::unicode::is_decimal) {
        return None;
    }
    // (ferb p-[0-9a-f]+ [0-9a-f]{32})
    let body = tag.strip_prefix("ferb p-")?;
    let (id, bits) = body.split_once(' ')?;
    let hex = |t: &str| {
        t.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    if id.is_empty() || !hex(id) || bits.len() != 32 || !hex(bits) {
        return None;
    }
    Some((
        repeats,
        program.to_string(),
        int_str(port)?,
        tag.to_string(),
    ))
}

#[derive(Default)]
struct ReportState {
    /// tag -> process, for the commands FERB expects reports of.
    tags: HashMap<String, String>,
    /// process -> port -> (connections, programs).
    seen: HashMap<String, BTreeMap<i128, (i128, BTreeSet<String>)>>,
    /// When the stream started listening (wall clock).
    since: Option<f64>,
    closing: bool,
    ended: Option<String>,
}

type OnEnd = Arc<dyn Fn(&str) + Send + Sync>;
type WallClock = Arc<dyn Fn() -> f64 + Send + Sync>;

/// Ends the stream with FERB, without Python (ENG-2): the stream's stdin is a pipe only the daemon holds; when it
/// closes (the daemon stopped or crashed) the watcher kills the stream; the script exits with the stream's code.
const WATCH: &str =
    "exec 3<&0 </dev/null; \"$@\" & s=$!; ( cat <&3 >/dev/null; kill \"$s\" 2>/dev/null ) \
>/dev/null 2>&1 & w=$!; exec 3<&-; wait \"$s\"; c=$?; kill \"$w\" 2>/dev/null; exit \"$c\"";

/// The kernel's reports of direct connections to pinned local ports, by FERB process id, read live from the system
/// log while FERB runs (local.py:213). Only a command FERB `expect`s, by its exact tag, is counted.
pub struct DirectReports {
    argv: Vec<String>,
    on_end: Option<OnEnd>,
    now: WallClock,
    state: Arc<Mutex<ReportState>>,
    child: Mutex<Option<(Child, Option<ChildStdin>)>>,
}

impl DirectReports {
    pub fn new(argv: Option<Vec<String>>, on_end: Option<OnEnd>) -> DirectReports {
        DirectReports {
            argv: argv.unwrap_or_else(stream_argv),
            on_end,
            now: Arc::new(crate::wall_time),
            state: Arc::new(Mutex::new(ReportState::default())),
            child: Mutex::new(None),
        }
    }

    pub fn with_clock(mut self, now: WallClock) -> DirectReports {
        self.now = now;
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ReportState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn running(&self) -> bool {
        let st = self.lock();
        st.since.is_some() && st.ended.is_none() && !st.closing
    }

    pub fn ended(&self) -> Option<String> {
        self.lock().ended.clone()
    }

    fn stopped(state: &Mutex<ReportState>, on_end: &Option<OnEnd>, why: String) {
        {
            let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
            if st.closing || st.ended.is_some() {
                return;
            }
            st.ended = Some(why.clone());
        }
        if let Some(f) = on_end {
            f(&why);
        }
    }

    pub fn start(&self) -> bool {
        let spawned = Command::new("/bin/sh")
            .arg("-c")
            .arg(WATCH)
            .arg("ferb-log-stream")
            .args(&self.argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                Self::stopped(
                    &self.state,
                    &self.on_end,
                    format!(
                        "the system log stream couldn't start ({})",
                        crate::py_os_error(&e)
                    ),
                );
                return false;
            }
        };
        let stdout = child.stdout.take().expect("piped");
        let stdin = child.stdin.take();
        let pid = child.id();
        *self.child.lock().unwrap_or_else(|e| e.into_inner()) = Some((child, stdin));
        let state = self.state.clone();
        let on_end = self.on_end.clone();
        let now = self.now.clone();
        let waiter = WaitHandle(pid as i32);
        std::thread::Builder::new()
            .name("ferb-direct-reports".into())
            .spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.split(b'\n') {
                    let Ok(line) = line else { break };
                    let line = String::from_utf8_lossy(&line).into_owned();
                    Self::read_line(&state, &now, &line);
                }
                let code = waiter.wait();
                Self::stopped(
                    &state,
                    &on_end,
                    format!("the system log stream ended (exit {code})"),
                );
            })
            .is_ok()
    }

    fn read_line(state: &Mutex<ReportState>, now: &WallClock, line: &str) {
        {
            let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
            if st.since.is_none() {
                st.since = Some(now()); // `log stream` names its filter once it listens
            }
        }
        let Ok(event) = ferb_py::json::loads(line) else {
            return;
        };
        let Some(obj) = event.as_object() else {
            return;
        };
        let kernel = match obj.get("processID") {
            Some(Value::Number(n)) => n.as_f64() == Some(0.0),
            Some(Value::Bool(b)) => !b,
            _ => false,
        };
        if !kernel {
            return;
        }
        let Some(message) = obj.get("eventMessage").and_then(Value::as_str) else {
            return;
        };
        let Some((repeats, program, port, tag)) = parse_report(message) else {
            return;
        };
        let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(process) = st.tags.get(&tag).cloned() else {
            return; // a tag FERB didn't issue, or a command already forgotten
        };
        let ports = st.seen.entry(process).or_default();
        let entry = ports.entry(port).or_insert((0, BTreeSet::new()));
        entry.0 += repeats.unwrap_or(1);
        entry.1.insert(program);
    }

    /// Count the reports carrying `tag` for `process`.
    pub fn expect(&self, process: &str, tag: &str) {
        self.lock()
            .tags
            .insert(tag.to_string(), process.to_string());
    }

    /// {port: {"reported_connections": n, "clients": [...]}} for a command started at `started` (wall clock); None
    /// when the stream wasn't listening for all of its life.
    pub fn connections(
        &self,
        process: &str,
        started: f64,
    ) -> Option<BTreeMap<i128, Map<String, Value>>> {
        let st = self.lock();
        if st.ended.is_some() || st.closing || st.since.is_none_or(|s| s > started) {
            return None;
        }
        Some(
            st.seen
                .get(process)
                .map(|ports| {
                    ports
                        .iter()
                        .map(|(port, (count, programs))| {
                            let mut m = Map::new();
                            m.insert("reported_connections".into(), json!(count));
                            m.insert("clients".into(), json!(programs.iter().collect::<Vec<_>>()));
                            (*port, m)
                        })
                        .collect()
                })
                .unwrap_or_default(),
        )
    }

    /// The command's record is written: its reports are neither kept nor counted any more.
    pub fn forget(&self, process: &str) {
        let mut st = self.lock();
        st.seen.remove(process);
        st.tags.retain(|_, p| p != process);
    }

    /// Stop the stream (closing its stdin stops it, as a daemon that goes away does).
    pub fn close(&self) {
        self.lock().closing = true;
        let taken = self.child.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some((_child, stdin)) = taken {
            drop(stdin);
            // the reader thread reaps it; it is ours alone
        }
    }

    /// Test seam: whether anything is kept for a process FERB never expected.
    pub fn kept(&self) -> BTreeSet<String> {
        self.lock().seen.keys().cloned().collect()
    }
}

/// Waits for one child of ours by pid (the `Child` stays with `DirectReports` so `close` can drop its stdin).
struct WaitHandle(i32);

impl WaitHandle {
    fn wait(&self) -> i32 {
        let mut status = 0;
        loop {
            // SAFETY: waits only for our own child, by its pid.
            let r = unsafe { libc::waitpid(self.0, &mut status, 0) };
            if r == self.0 {
                break;
            }
            if r == -1 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                return -1;
            }
        }
        if libc::WIFEXITED(status) {
            libc::WEXITSTATUS(status)
        } else if libc::WIFSIGNALED(status) {
            -libc::WTERMSIG(status)
        } else {
            -1
        }
    }
}
