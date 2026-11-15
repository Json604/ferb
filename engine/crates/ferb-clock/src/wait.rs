//! Port of `ferb/clock/wait.py`: `wait.for`, a worker waiting in code, with no model calls, until one of its
//! conditions holds or its time limit passes, then one result saying what code saw (ORCH-631..633, ORCH-639).
//!
//! What belongs to other parts comes in through [`WaitEnv`]: the task's shell commands ([`Shells`], the shell
//! part), its browser tabs ([`Browser`], the browser part), the clock and the loop's rest (TEST-508), the task's
//! limit, its live line, corrections, restarts and the worker's last failed action (the gateway and supervisor),
//! the Mac's zone and home folder, and `urls.parse`'s host (the policy part).
//!
//! Waiting never acts on anything: a page is only read (or reloaded at its own address), a file only looked at.

use std::collections::BTreeSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use ferb_core::errors::FerbError;
use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{ActionSpec, Identity, Target, ToolContext, ToolResult};
use serde_json::{json, Map, Value};

use crate::py::{self, Time};
use crate::when::{self, When};
use crate::zone::Zone;

// Agent rulings (ORCH-632): the default limit, the intervals' floors and how much a result carries.
pub const DEFAULT_LIMIT_MIN: f64 = 30.0;
pub const PAGE_FLOOR_S: f64 = 30.0;
pub const PAGE_DEFAULT_S: f64 = 60.0;
pub const SCRIPT_FLOOR_S: f64 = 5.0;
pub const SCRIPT_DEFAULT_S: f64 = 30.0;
pub const SCRIPT_RUN_S: f64 = 60.0;
pub const FILE_EVERY_S: f64 = 2.0;
pub const TICK_S: f64 = 1.0;
pub const SETTLE_DEFAULT_S: f64 = 10.0;
pub const OUTPUT_TAIL: usize = 2000;
pub const MATCH_LINES: usize = 5;
pub const LINE_CHARS: usize = 300;
pub const PAGE_ERRORS: u32 = 3;
pub const QUOTE_CHARS: usize = 40;
pub const NOT_FOUND: [i64; 2] = [126, 127];
pub const ORIGIN_OUTPUT: &str = "external:unlabeled";
/// `ferb/orchestrator/progress.py:81`.
pub const COMMAND_CHARS_SAID: usize = 40;
pub const COMMAND_CHARS: usize = 120;

pub const DESCRIPTION: &str = "Wait in code, with no AI calls, until any one of your conditions holds or the time limit passes; you get one result saying which condition woke you and what FERB saw (matching output lines, a file's size, the page lines that matched, a script's output). Use it instead of calling shell.wait, a page read or a sleep again and again. Conditions (any_of, any one wakes you): process {id, match?: wake when it exits or its output holds match}; file {path, state: exists | gone | settled (exists and stopped changing for settle_s)}; page {tab, match?: wake when its text holds match, or without match when it changes; every_s >= 30; reload: load its address again before each read}; url {tab, match?: its address holds match, or changes}; mail {tab showing a mailbox, sender / subject / text: a new line naming them appears}; download {tab: it downloads a file}; time {in_minutes or at \"HH:MM\"}; script {command, cwd?, every_s >= 5: runs in FERB's shell sandbox as shell.run does; exit 0 wakes you}. match is plain text, case aside (regex: true for a regular expression). limit_minutes: how long at most (default 30, longer or shorter as the work needs, up to the task's own limit). A message from the owner wakes you too, and so does any of this task's commands ending; right after an action of yours failed it returns at once. While you wait, the task's status shows what you wait for.";

const COMMAND_ENDED: &str = "This task's command {id} ended (you didn't name it in this wait): what you wait for may depend on it. Wait again if it still applies.";
const ACTION_FAILED: &str = "Nothing was waited for: your last action ({tool}) ended with an error, so what you wait for may never come. Check what happened, then act again, or call wait.for again to wait anyway.";

// ---------- the input ----------

/// One condition (`Condition`, `wait.py:91`), with pydantic's defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct Condition {
    pub kind: String,
    pub id: String,
    pub path: String,
    pub state: String,
    pub settle_s: f64,
    pub tab: String,
    pub r#match: String,
    pub regex: bool,
    pub since: usize,
    pub reload: bool,
    pub every_s: f64,
    pub sender: String,
    pub subject: String,
    pub text: String,
    pub in_minutes: f64,
    pub at: String,
    pub command: String,
    pub cwd: String,
}

impl Default for Condition {
    fn default() -> Self {
        Condition {
            kind: String::new(),
            id: String::new(),
            path: String::new(),
            state: String::new(),
            settle_s: SETTLE_DEFAULT_S,
            tab: String::new(),
            r#match: String::new(),
            regex: false,
            since: 0,
            reload: false,
            every_s: 0.0,
            sender: String::new(),
            subject: String::new(),
            text: String::new(),
            in_minutes: 0.0,
            at: String::new(),
            command: String::new(),
            cwd: String::new(),
        }
    }
}

/// `WaitInput`.
#[derive(Clone, Debug, PartialEq)]
pub struct WaitInput {
    pub any_of: Vec<Condition>,
    pub limit_minutes: Option<f64>,
}

impl WaitInput {
    /// The arguments as the gateway checked them against the schema (types, ranges, lengths), then the model's own
    /// checks (`_fields`, `_one_script`): `Err` is the `ValueError`'s words. A regular expression is compiled by
    /// `fancy-regex`, not Python's `re`, so the words after "match isn't a valid regular expression: " differ.
    pub fn from_json(v: &Value) -> Result<WaitInput, String> {
        let mut any_of = Vec::new();
        for c in v
            .get("any_of")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let s = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let f = |k: &str, d: f64| c.get(k).and_then(Value::as_f64).unwrap_or(d);
            let b = |k: &str| c.get(k).and_then(Value::as_bool).unwrap_or(false);
            let cond = Condition {
                kind: s("kind"),
                id: s("id"),
                path: s("path"),
                state: s("state"),
                settle_s: f("settle_s", SETTLE_DEFAULT_S),
                tab: s("tab"),
                r#match: s("match"),
                regex: b("regex"),
                since: c.get("since").and_then(Value::as_u64).unwrap_or(0) as usize,
                reload: b("reload"),
                every_s: f("every_s", 0.0),
                sender: s("sender"),
                subject: s("subject"),
                text: s("text"),
                in_minutes: f("in_minutes", 0.0),
                at: s("at"),
                command: s("command"),
                cwd: s("cwd"),
            };
            cond.check()?;
            any_of.push(cond);
        }
        if any_of.iter().filter(|c| c.kind == "script").count() > 1 {
            return Err("one script condition per wait: put both checks in one command".into());
        }
        Ok(WaitInput {
            any_of,
            limit_minutes: v.get("limit_minutes").and_then(Value::as_f64),
        })
    }

    fn script(&self) -> Option<&Condition> {
        self.any_of.iter().find(|c| c.kind == "script")
    }

    /// `describe`: a wait with a script is a shell command for the gateway (as shell.run); any other only reads.
    pub fn describe(&self) -> ActionSpec {
        match self.script() {
            Some(s) => {
                let mut spec = ActionSpec::new("run_shell");
                spec.target = Target {
                    path: Some(if s.cwd.is_empty() {
                        ".".into()
                    } else {
                        s.cwd.clone()
                    }),
                    ..Target::default()
                };
                spec.content = Some(s.command.clone());
                spec
            }
            None => ActionSpec::new("read"),
        }
    }

    /// `file_paths`: each watched file, for the gateway's protection lists as an fs read is.
    pub fn file_paths(&self) -> Vec<(String, String)> {
        self.any_of
            .iter()
            .filter(|c| c.kind == "file")
            .map(|c| (c.path.clone(), "read".to_string()))
            .collect()
    }
}

impl Condition {
    /// `_fields` (`wait.py:111`).
    pub fn check(&self) -> Result<(), String> {
        let need = match self.kind.as_str() {
            "process" => !self.id.is_empty(),
            "file" => !self.path.is_empty() && !self.state.is_empty(),
            "page" | "url" | "download" => !self.tab.is_empty(),
            "mail" => {
                !self.tab.is_empty()
                    && (!self.sender.is_empty()
                        || !self.subject.is_empty()
                        || !self.text.is_empty())
            }
            "time" => self.in_minutes != 0.0 || !self.at.is_empty(),
            "script" => !py::strip(&self.command).is_empty(),
            other => return Err(format!("unknown condition kind {}", py::repr(other))),
        };
        if !need {
            return Err(match self.kind.as_str() {
                "process" => "a process condition needs id",
                "file" => "a file condition needs path and state",
                "page" => "a page condition needs tab",
                "url" => "a url condition needs tab",
                "mail" => "a mail condition needs tab and one of sender, subject, text",
                "download" => "a download condition needs tab",
                "time" => "a time condition needs in_minutes or at",
                _ => "a script condition needs command",
            }
            .into());
        }
        if self.regex && !self.r#match.is_empty() {
            if let Err(e) = fancy_regex::Regex::new(&self.r#match) {
                return Err(format!("match isn't a valid regular expression: {e}"));
            }
        }
        Ok(())
    }
}

// ---------- the seams ----------

/// A shell command's handle (`ProcHandle`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcHandle {
    pub id: String,
    pub owner_task: String,
}

/// What the shell knows of a command (`ProcInfo`).
#[derive(Clone, Debug, PartialEq)]
pub struct ProcInfo {
    pub id: String,
    pub command: String,
    pub running: bool,
    pub exit_code: Option<i64>,
    /// Why FERB stopped it, if it did.
    pub stopped: Option<String>,
    pub owner_task: String,
}

/// The task's shell commands (`ferb/act/shell/manager.py`'s `ShellManager`, a later part).
pub trait Shells: Send + Sync {
    fn handle(&self, id: &str, task: &str) -> ProcHandle;
    /// `Err(Refused)`: not one of this task's commands.
    fn info(&self, h: &ProcHandle) -> Result<ProcInfo, FerbError>;
    fn list(&self) -> Vec<ProcInfo>;
    /// A tool result already showed this command's end.
    fn told(&self, id: &str) -> bool;
    fn exit_told(&self, h: &ProcHandle);
    /// Output from `since`: (text, its end offset, the total).
    fn read<'a>(&'a self, h: &'a ProcHandle, since: usize)
        -> BoxFuture<'a, (String, usize, usize)>;
    fn tail<'a>(&'a self, h: &'a ProcHandle, limit: usize)
        -> BoxFuture<'a, (String, usize, usize)>;
    /// Its exit code once it exits, or None after `timeout` seconds.
    fn wait<'a>(&'a self, h: &'a ProcHandle, timeout: f64) -> BoxFuture<'a, Option<i64>>;
    /// A check script run exactly as `shell.run` starts a command (the task's sandbox, folders, proxy).
    fn start<'a>(
        &'a self,
        command: &'a str,
        cwd: &'a str,
        task: &'a str,
        folders: &'a [String],
    ) -> BoxFuture<'a, Result<ProcHandle, FerbError>>;
    fn kill<'a>(&'a self, h: &'a ProcHandle) -> BoxFuture<'a, ()>;
    fn release<'a>(&'a self, h: &'a ProcHandle) -> BoxFuture<'a, ()>;
    /// The hosts it reached through FERB's proxy.
    fn network(&self, h: &ProcHandle) -> Vec<String>;
    /// `guard.working_directory(cwd, ctx)`: `Err(Refused)` outside the task's folders.
    fn working_directory(&self, cwd: Option<&str>, ctx: &ToolContext) -> Result<String, FerbError>;
}

/// A page read (`PageSnapshot`'s parts a wait uses).
#[derive(Clone, Debug, PartialEq)]
pub struct Snap {
    pub url: String,
    pub title: Option<String>,
    pub text: Option<String>,
}

/// A browser call that failed: refused (not this task's tab) or failed otherwise (a hung page: tried again).
#[derive(Clone, Debug, PartialEq)]
pub enum PageError {
    Refused(String),
    /// Python's exception class name and its words.
    Failed {
        kind: String,
        message: String,
    },
}

/// The task's tabs (the browser part's driver).
pub trait Browser: Send + Sync {
    /// The tab's reference, or `Err(Refused)` when it isn't this task's tab.
    fn owned_by(&self, tab: &str, task: &str) -> Result<String, PageError>;
    fn tab_url(&self, tab: &str) -> Option<String>;
    fn snapshot<'a>(&'a self, tab_ref: &'a str) -> BoxFuture<'a, Result<Snap, PageError>>;
    fn navigate<'a>(
        &'a self,
        tab_ref: &'a str,
        url: &'a str,
    ) -> BoxFuture<'a, Result<(), PageError>>;
    /// The downloads the tab made that the worker hasn't seen; `None` when this driver has no downloads record.
    fn downloads_for<'a>(
        &'a self,
        tab_ref: &'a str,
    ) -> Option<BoxFuture<'a, Result<Vec<Value>, PageError>>>;
}

/// Everything a wait is given (`WaitDeps`, `wait.py:143`, plus what Python reads from the process: the zone, the
/// home folder and `urls.parse`).
pub trait WaitEnv: Send + Sync {
    fn now(&self) -> Time;
    fn monotonic(&self) -> f64;
    /// The loop's rest: `seconds` pass (a command's exit cuts it short: the wait races it).
    fn pause(&self, seconds: f64) -> BoxFuture<'_, ()>;
    /// The task's own time limit.
    fn limit(&self, task: &str) -> Option<Time>;
    fn shells(&self) -> Option<&dyn Shells>;
    fn browser(&self, identity: Identity) -> Option<&dyn Browser>;
    /// The task's live line while it waits (`line` None clears it).
    fn status(&self, worker: &str, task: &str, until: Option<&Time>, line: Option<&str>);
    /// How many corrections the task's tree has had.
    fn steered(&self, task: &str) -> i64;
    /// "restart" while FERB shuts down.
    fn ending(&self) -> Option<String>;
    /// Journals a wait a restart cut.
    fn ended(&self, task: &str, data: Value);
    /// This worker run's last acting call when it ended with an error and nothing acted since, taken once.
    fn failed_act(&self, worker: &str) -> Option<Map<String, Value>>;
    /// The Mac's zone (Python's `astimezone()`).
    fn zone(&self) -> Zone;
    /// The owner's home (`expanduser`).
    fn home(&self) -> PathBuf;
    /// `urls.parse(url).host`, "" when it doesn't parse.
    fn host(&self, url: &str) -> String;
}

// ---------- text ----------

/// `_clip`: one printable line, cut.
pub fn clip(text: &str, n: usize) -> String {
    let one = py::squeeze(&ferb_sanitize::terminal::printable(text));
    if py::char_len(&one) <= n {
        one
    } else {
        format!("{}…", py::rstrip(py::prefix(&one, n.saturating_sub(1))))
    }
}

fn quoted(text: &str) -> String {
    format!("“{}”", clip(text, QUOTE_CHARS).replace(['“', '”'], ""))
}

fn every(seconds: f64) -> String {
    if seconds < 60.0 {
        format!("every {} s", seconds.trunc() as i64)
    } else {
        format!(
            "every {} min",
            py::fmt_g(py::round_even(seconds / 60.0) as f64)
        )
    }
}

/// `command_said` (`ferb/orchestrator/progress.py:93`): a command as a live line quotes it (ORCH-628).
pub fn command_said(command: &str) -> String {
    let first_line = py::splitlines(command)
        .into_iter()
        .find(|l| !py::strip(l).is_empty())
        .unwrap_or("");
    let mut first = py::squeeze(&ferb_sanitize::terminal::printable(first_line));
    if py::char_len(&first) > COMMAND_CHARS_SAID {
        let cut = py::prefix(&first, COMMAND_CHARS_SAID - 1);
        let before = cut.rsplit_once(' ').map_or(cut, |(a, _)| a);
        first = format!("{} …", py::rstrip(before));
    }
    if first.is_empty() {
        String::new()
    } else {
        format!("`{}`", first.replace('`', ""))
    }
}

/// `PurePosixPath(text)` as `str()` writes it: repeated slashes and `.` parts gone, no trailing slash.
fn posix_norm(text: &str) -> String {
    let root = if text.starts_with("//") && !text.starts_with("///") {
        "//"
    } else if text.starts_with('/') {
        "/"
    } else {
        ""
    };
    let parts: Vec<&str> = text
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    let out = format!("{root}{}", parts.join("/"));
    if out.is_empty() {
        ".".into()
    } else {
        out
    }
}

fn posix_name(text: &str) -> String {
    let n = posix_norm(text);
    if n == "/" || n == "//" || n == "." {
        return String::new();
    }
    n.rsplit('/').next().unwrap_or("").to_string()
}

// ---------- the wait ----------

#[derive(Clone, Debug)]
enum Base {
    None,
    Offset(usize),
    Text(String),
    Lines(Vec<String>),
    File(Option<(u64, i128)>),
}

#[derive(Clone, Debug)]
enum Handle {
    None,
    Proc(ProcHandle),
    Tab,
    Path(String),
}

struct Watch {
    index: i64,
    c: Condition,
    due: f64,
    seen: Map<String, Value>,
    base: Base,
    errors: u32,
    handle: Handle,
    settled_since: Option<f64>,
    at: Option<Time>,
    implicit: bool,
    /// Its exit was seen by the exit watch (Python's `_exit_of` task ran to its end).
    exit_watched: bool,
}

impl Watch {
    fn new(index: i64, c: Condition) -> Watch {
        Watch {
            index,
            c,
            due: 0.0,
            seen: Map::new(),
            base: Base::None,
            errors: 0,
            handle: Handle::None,
            settled_since: None,
            at: None,
            implicit: false,
            exit_watched: false,
        }
    }

    fn proc(&self) -> &ProcHandle {
        match &self.handle {
            Handle::Proc(h) => h,
            _ => unreachable!("a process watch has its handle"),
        }
    }
}

fn err(msg: impl Into<String>) -> ToolResult {
    let mut content = Map::new();
    content.insert("error".into(), Value::String(msg.into()));
    ToolResult {
        content,
        is_error: true,
        ..ToolResult::default()
    }
}

fn refused(msg: impl Into<String>) -> FerbError {
    FerbError::Refused(msg.into())
}

fn merged(a: Map<String, Value>, b: Map<String, Value>) -> Map<String, Value> {
    let mut out = a;
    for (k, v) in b {
        out.insert(k, v);
    }
    out
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

type Sources = BTreeSet<(String, String)>;

fn matcher(c: &Condition) -> Box<dyn Fn(&str) -> bool> {
    if c.regex {
        match fancy_regex::Regex::new(&c.r#match) {
            Ok(re) => Box::new(move |line| re.is_match(line).unwrap_or(false)),
            Err(_) => Box::new(|_| false),
        }
    } else {
        let want = py::casefold(&c.r#match);
        Box::new(move |line| py::casefold(line).contains(&want))
    }
}

/// Undoes the live line when the wait ends however it ends (Python's `finally`), and journals a wait a restart
/// cut (Python's `except CancelledError`): a dropped future is Rust's cancellation.
struct Ending<'a> {
    env: &'a dyn WaitEnv,
    worker: String,
    task: String,
    line: String,
    began: Time,
    done: bool,
}

impl Drop for Ending<'_> {
    fn drop(&mut self) {
        if !self.done && self.env.ending().as_deref() == Some("restart") {
            self.env.ended(
                &self.task,
                json!({"line": self.line, "began": py::iso(&self.began), "why": "restart"}),
            );
        }
        self.env.status(&self.worker, &self.task, None, None);
    }
}

/// Polls the exit watches of the process watches not yet seen exiting, together.
struct FirstExit<'v, 'f> {
    futures: &'v mut Vec<(usize, Option<BoxFuture<'f, Option<i64>>>)>,
}

impl Future for FirstExit<'_, '_> {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        let mut any = false;
        for (_, f) in this.futures.iter_mut() {
            if let Some(fut) = f {
                if fut.as_mut().poll(cx).is_ready() {
                    *f = None;
                    any = true;
                }
            }
        }
        if any {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// `wait.for`.
pub struct WaitFor<'a> {
    env: &'a dyn WaitEnv,
}

impl<'a> WaitFor<'a> {
    pub fn new(env: &'a dyn WaitEnv) -> Self {
        WaitFor { env }
    }

    fn local_hm(&self, t: &Time) -> String {
        py::fmt(&self.env.zone().local(t), "%H:%M")
    }

    /// `run` (`wait.py:252`).
    pub async fn run(&self, args: &WaitInput, ctx: &ToolContext) -> ToolResult {
        let env = self.env;
        let (mut watches, until) = match self.prepare(args, ctx) {
            Ok(x) => x,
            Err(e) => return err(e),
        };
        if let Some(failed) = env.failed_act(&ctx.worker_id) {
            let tool = failed
                .get("tool")
                .filter(|v| ferb_py::value::truthy(v))
                .map(ferb_py::value::py_str)
                .unwrap_or_else(|| "your last action".into());
            let mut content = Map::new();
            content.insert("woke".into(), json!("action_failed"));
            content.insert("after_s".into(), json!(0));
            content.insert(
                "evidence".into(),
                json!({"action": Value::Object(failed), "note": ACTION_FAILED.replace("{tool}", &tool)}),
            );
            return ToolResult {
                content,
                ..ToolResult::default()
            };
        }
        let line = self.line(&watches, &until, ctx);
        let implicit = self.commands(&watches, ctx);
        watches.extend(implicit);
        let started = env.monotonic();
        let began = env.now();
        let steered = env.steered(&ctx.task_id);
        env.status(&ctx.worker_id, &ctx.task_id, Some(&until), Some(&line));
        let mut ending = Ending {
            env,
            worker: ctx.worker_id.clone(),
            task: ctx.task_id.clone(),
            line: line.clone(),
            began,
            done: false,
        };
        let mut sources = Sources::new();
        let out = self
            .wait_loop(
                &mut watches,
                &until,
                &began,
                ctx,
                started,
                steered,
                &mut sources,
            )
            .await;
        ending.done = true;
        drop(ending);
        out
    }

    #[allow(clippy::too_many_arguments)]
    async fn wait_loop(
        &self,
        watches: &mut [Watch],
        until: &Time,
        began: &Time,
        ctx: &ToolContext,
        started: f64,
        steered: i64,
        sources: &mut Sources,
    ) -> ToolResult {
        let env = self.env;
        loop {
            let now = env.monotonic();
            for w in watches.iter_mut() {
                if now < w.due {
                    continue;
                }
                let hit = self.look(w, ctx, sources).await;
                if let Some(hit) = hit {
                    let w = &*w;
                    if hit.get("exited").is_some_and(ferb_py::value::truthy) {
                        if let Some(sh) = env.shells() {
                            sh.exit_told(w.proc());
                        }
                    }
                    if w.implicit {
                        return self.woke(
                            "process_exited",
                            started,
                            merged(self.ended_fact(w), hit),
                            sources,
                        );
                    }
                    let mut ev = Map::new();
                    ev.insert("condition".into(), json!(w.index));
                    return self.woke(&w.c.kind, started, merged(ev, hit), sources);
                }
            }
            if env.steered(&ctx.task_id) > steered {
                return self.woke(
                    "correction",
                    started,
                    obj(json!({"note": "the owner sent a message to this task; read it, then wait again if it still applies"})),
                    sources,
                );
            }
            if env.now() >= *until {
                let last_seen: Vec<Value> = watches
                    .iter()
                    .filter(|w| !w.implicit)
                    .map(|w| {
                        let mut m = Map::new();
                        m.insert("condition".into(), json!(w.index));
                        m.insert("kind".into(), json!(w.c.kind));
                        Value::Object(merged(m, w.seen.clone()))
                    })
                    .collect();
                let mut ev = Map::new();
                ev.insert("limit".into(), json!(self.local_hm(until)));
                ev.insert("began".into(), json!(self.local_hm(began)));
                ev.insert("last_seen".into(), Value::Array(last_seen));
                return self.woke("time_limit", started, ev, sources);
            }
            let mono = env.monotonic();
            let nxt = watches.iter().map(|w| w.due).fold(mono + TICK_S, f64::min);
            let left = py::total_seconds(until, &env.now()).max(0.0);
            let rest = (nxt - mono).min(left).max(0.01);
            self.pause_or_exit(watches, until, rest).await;
        }
    }

    /// The loop's rest, cut short when one of the watched commands exits (Python's `_exit_of` tasks, which set the
    /// watch due at once). After the rest, every exit that already happened counts, as Python's tasks would have run.
    async fn pause_or_exit(&self, watches: &mut [Watch], until: &Time, rest: f64) {
        let env = self.env;
        let Some(shells) = env.shells() else {
            env.pause(rest).await;
            return;
        };
        let left = py::total_seconds(until, &env.now()).max(0.0) + 1.0;
        let handles: Vec<(usize, ProcHandle)> = watches
            .iter()
            .enumerate()
            .filter(|(_, w)| {
                w.c.kind == "process" && !w.exit_watched && matches!(w.handle, Handle::Proc(_))
            })
            .map(|(i, w)| (i, w.proc().clone()))
            .collect();
        let mut futures: Vec<(usize, Option<BoxFuture<'_, Option<i64>>>)> = handles
            .iter()
            .map(|(i, h)| (*i, Some(shells.wait(h, left))))
            .collect();
        {
            let pause = env.pause(rest);
            tokio::pin!(pause);
            let first = FirstExit {
                futures: &mut futures,
            };
            tokio::select! {
                biased;
                _ = &mut pause => {}
                _ = first => {}
            }
        }
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);
        for (i, f) in futures.iter_mut() {
            let ready = match f {
                None => true,
                Some(fut) => fut.as_mut().poll(&mut cx).is_ready(),
            };
            if ready {
                watches[*i].exit_watched = true;
                watches[*i].due = 0.0;
            }
        }
    }

    fn woke(
        &self,
        kind: &str,
        started: f64,
        evidence: Map<String, Value>,
        sources: &Sources,
    ) -> ToolResult {
        let after = py::round_even(self.env.monotonic() - started);
        let is_error = evidence.contains_key("error");
        let mut content = Map::new();
        content.insert("woke".into(), json!(kind));
        content.insert("after_s".into(), json!(after));
        content.insert("evidence".into(), Value::Object(evidence));
        ToolResult {
            content,
            is_error,
            read_sources: sources.iter().cloned().collect(),
            ..ToolResult::default()
        }
    }

    /// `_commands` (ORCH-639): the task's own commands the worker didn't name, each watched for its end.
    fn commands(&self, named: &[Watch], ctx: &ToolContext) -> Vec<Watch> {
        let Some(shells) = self.env.shells() else {
            return vec![];
        };
        if !ctx.scopes.contains("shell") {
            return vec![];
        }
        let ids: BTreeSet<&str> = named
            .iter()
            .filter(|w| w.c.kind == "process")
            .map(|w| w.c.id.as_str())
            .collect();
        let mut out = Vec::new();
        for info in shells.list() {
            if info.owner_task != ctx.task_id
                || ids.contains(info.id.as_str())
                || (!info.running && shells.told(&info.id))
            {
                continue;
            }
            let mut w = Watch::new(
                -1,
                Condition {
                    kind: "process".into(),
                    id: info.id.clone(),
                    ..Condition::default()
                },
            );
            w.handle = Handle::Proc(shells.handle(&info.id, &ctx.task_id));
            w.implicit = true;
            out.push(w);
        }
        out
    }

    fn ended_fact(&self, w: &Watch) -> Map<String, Value> {
        let command = self
            .env
            .shells()
            .and_then(|s| s.info(w.proc()).ok())
            .map(|i| clip(&i.command, COMMAND_CHARS))
            .unwrap_or_default();
        obj(
            json!({"id": w.c.id, "command": command, "note": COMMAND_ENDED.replace("{id}", &w.c.id)}),
        )
    }

    // ---------- before the wait ----------

    fn prepare(&self, a: &WaitInput, ctx: &ToolContext) -> Result<(Vec<Watch>, Time), String> {
        let env = self.env;
        let now = env.now();
        let limit = env.limit(&ctx.task_id);
        let until = if let Some(minutes) = a.limit_minutes {
            let us = py::timedelta_us(minutes, py::US_PER_MINUTE).map_err(|o| o.0)?;
            let until = py::plus_us(&now, us).map_err(|o| o.0)?;
            if let Some(l) = limit {
                if until > l {
                    let longest = (py::total_seconds(&l, &now) / 60.0).floor().max(0.0) as i64;
                    return Err(format!(
                        "that's past this task's time limit ({}); the longest wait now is {} minutes",
                        self.local_hm(&l),
                        longest.max(0)
                    ));
                }
            }
            until
        } else {
            let until = py::plus_us(&now, (DEFAULT_LIMIT_MIN as i64) * py::US_PER_MINUTE)
                .map_err(|o| o.0)?;
            match limit {
                Some(l) if l < until => l,
                _ => until,
            }
        };
        let mut watches = Vec::new();
        for (i, c) in a.any_of.iter().enumerate() {
            let mut w = Watch::new(i as i64, c.clone());
            self.admit(&mut w, ctx, &now).map_err(|e| e.to_string())?;
            if c.kind == "time" {
                if let Some(at) = w.at {
                    if at > until {
                        return Err(format!(
                            "condition {i}'s time ({}) is after this wait's limit ({}); set limit_minutes to cover it",
                            self.local_hm(&at),
                            self.local_hm(&until)
                        ));
                    }
                }
            }
            watches.push(w);
        }
        Ok((watches, until))
    }

    fn scope(&self, ctx: &ToolContext, kind: &str) -> Result<(), FerbError> {
        let need = match kind {
            "process" | "script" => Some("shell"),
            "file" => Some("fs:read"),
            "page" | "url" | "mail" | "download" => Some(if ctx.identity == Identity::Ferb {
                "identity:browse"
            } else {
                "browser:read"
            }),
            _ => None,
        };
        match need {
            Some(n) if !ctx.scopes.contains(n) => Err(refused(format!(
                "this task can't use a {kind} condition (no {n})"
            ))),
            _ => Ok(()),
        }
    }

    fn admit(&self, w: &mut Watch, ctx: &ToolContext, now: &Time) -> Result<(), FerbError> {
        let env = self.env;
        let c = w.c.clone();
        self.scope(ctx, &c.kind)?;
        match c.kind.as_str() {
            "process" => {
                let shells = env
                    .shells()
                    .ok_or_else(|| refused("FERB's shell isn't available"))?;
                let h = shells.handle(&c.id, &ctx.task_id);
                shells.info(&h)?;
                w.handle = Handle::Proc(h);
            }
            "page" | "url" | "mail" | "download" => {
                let driver = env
                    .browser(ctx.identity)
                    .ok_or_else(|| refused("no browser is connected for this task"))?;
                let r = driver.owned_by(&c.tab, &ctx.task_id).map_err(|e| match e {
                    PageError::Refused(m) => refused(m),
                    PageError::Failed { message, .. } => FerbError::Refused(message),
                })?;
                let _ = r;
                w.handle = Handle::Tab;
                if matches!(c.kind.as_str(), "page" | "mail")
                    && c.every_s != 0.0
                    && c.every_s < PAGE_FLOOR_S
                {
                    let f = py::fmt_g(PAGE_FLOOR_S);
                    return Err(refused(format!(
                        "a page is read at most every {f} s (every_s >= {f})"
                    )));
                }
            }
            "script" => {
                let shells = env
                    .shells()
                    .ok_or_else(|| refused("FERB's shell isn't available"))?;
                if c.every_s != 0.0 && c.every_s < SCRIPT_FLOOR_S {
                    let f = py::fmt_g(SCRIPT_FLOOR_S);
                    return Err(refused(format!(
                        "a script runs at most every {f} s (every_s >= {f})"
                    )));
                }
                shells.working_directory(Some(c.cwd.as_str()).filter(|s| !s.is_empty()), ctx)?;
            }
            "time" => {
                let when = When {
                    in_minutes: Some(c.in_minutes).filter(|m| *m != 0.0),
                    time: Some(c.at.clone()).filter(|a| !a.is_empty()),
                    ..When::default()
                };
                w.at =
                    Some(when::resolve(&when, now, &env.zone()).map_err(|e| refused(e.message()))?);
            }
            "file" => w.handle = Handle::Path(self.path(&c.path, ctx)),
            _ => {}
        }
        Ok(())
    }

    /// `_path`: `~` expanded, relative to the workspace, as `str(Path)` writes it.
    fn path(&self, given: &str, ctx: &ToolContext) -> String {
        let expanded = if given == "~" {
            self.env.home().to_string_lossy().into_owned()
        } else if let Some(rest) = given.strip_prefix("~/") {
            format!(
                "{}/{rest}",
                self.env.home().to_string_lossy().trim_end_matches('/')
            )
        } else {
            given.to_string()
        };
        if expanded.starts_with('/') {
            posix_norm(&expanded)
        } else {
            posix_norm(&format!("{}/{expanded}", ctx.workspace))
        }
    }

    fn interval(c: &Condition) -> f64 {
        let or = |x: f64, d: f64| if x != 0.0 { x } else { d };
        match c.kind.as_str() {
            "page" | "mail" => PAGE_FLOOR_S.max(or(c.every_s, PAGE_DEFAULT_S)),
            "script" => SCRIPT_FLOOR_S.max(or(c.every_s, SCRIPT_DEFAULT_S)),
            "file" | "url" | "download" => FILE_EVERY_S,
            _ => TICK_S,
        }
    }

    // ---------- the live line ----------

    fn line(&self, watches: &[Watch], until: &Time, ctx: &ToolContext) -> String {
        let phrases: Vec<String> = watches.iter().map(|w| self.phrase(w, ctx)).collect();
        let mut first = phrases[0].clone();
        if phrases.len() > 1 {
            let n = phrases.len() - 1;
            first.push_str(&format!(
                ", or {n} other {}",
                if n == 1 { "thing" } else { "things" }
            ));
        }
        let only_time = watches.len() == 1 && watches[0].c.kind == "time";
        if only_time {
            first
        } else {
            format!("{first} (up to {})", self.local_hm(until))
        }
    }

    fn phrase(&self, w: &Watch, ctx: &ToolContext) -> String {
        let env = self.env;
        let c = &w.c;
        match c.kind.as_str() {
            "process" => {
                let cmd = env
                    .shells()
                    .and_then(|s| s.info(w.proc()).ok())
                    .map(|i| command_said(&i.command))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "a command".into());
                if c.r#match.is_empty() {
                    format!("Waiting for {cmd} to finish")
                } else {
                    format!("Waiting for {cmd} to print {}", quoted(&c.r#match))
                }
            }
            "file" => {
                let trimmed = c.path.trim_end_matches('/');
                let name = posix_name(trimmed);
                let name = clip(if name.is_empty() { &c.path } else { &name }, QUOTE_CHARS);
                match c.state.as_str() {
                    "exists" => format!("Waiting for {name} to appear"),
                    "gone" => format!("Waiting for {name} to go away"),
                    _ => {
                        let handle = match &w.handle {
                            Handle::Path(p) => p.as_str(),
                            _ => "",
                        };
                        let downloads = Path::new(handle).components().any(|p| {
                            matches!(p.as_os_str().to_str(), Some("Downloads" | "downloads"))
                        });
                        format!(
                            "Waiting for {name} to finish {}",
                            if downloads { "downloading" } else { "saving" }
                        )
                    }
                }
            }
            "time" => format!(
                "Waiting until {}",
                self.local_hm(w.at.as_ref().expect("a time"))
            ),
            "script" => {
                let cmd = Some(command_said(&c.command))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "a command".into());
                format!("Checking {cmd} {}", every(Self::interval(c)))
            }
            kind => {
                let url = env.browser(ctx.identity).and_then(|b| b.tab_url(&c.tab));
                let host = url
                    .filter(|u| !u.is_empty())
                    .map(|u| env.host(&u))
                    .filter(|h| !h.is_empty())
                    .unwrap_or_else(|| "a page".into());
                match kind {
                    "page" => {
                        let what = if c.r#match.is_empty() {
                            "a change".to_string()
                        } else {
                            quoted(&c.r#match)
                        };
                        format!("Watching {host} for {what} {}", every(Self::interval(c)))
                    }
                    "url" => {
                        if c.r#match.is_empty() {
                            format!("Waiting for the {host} tab to go to another page")
                        } else {
                            format!(
                                "Waiting for the {host} tab's address to show {}",
                                quoted(&c.r#match)
                            )
                        }
                    }
                    "mail" => {
                        let who = if !c.sender.is_empty() {
                            format!("from {}", quoted(&c.sender))
                        } else if !c.subject.is_empty() {
                            format!("about {}", quoted(&c.subject))
                        } else {
                            format!("mentioning {}", quoted(&c.text))
                        };
                        format!(
                            "Watching {host} for a new message {who} {}",
                            every(Self::interval(c))
                        )
                    }
                    _ => format!("Waiting for a download in the {host} tab"),
                }
            }
        }
    }

    // ---------- one look ----------

    async fn look(
        &self,
        w: &mut Watch,
        ctx: &ToolContext,
        sources: &mut Sources,
    ) -> Option<Map<String, Value>> {
        let env = self.env;
        w.due = env.monotonic() + Self::interval(&w.c);
        match w.c.kind.as_str() {
            "time" => {
                let at = w.at.expect("a time");
                let ahead = py::total_seconds(&at, &env.now());
                w.due = env.monotonic() + ahead.clamp(0.01, TICK_S);
                (env.now() >= at).then(|| obj(json!({"at": self.local_hm(&at)})))
            }
            "process" => self.process(w, sources).await,
            "file" => self.file(w),
            "script" => self.run_script(w, ctx, sources).await,
            _ => self.tab(w, ctx, sources).await,
        }
    }

    async fn process(&self, w: &mut Watch, sources: &mut Sources) -> Option<Map<String, Value>> {
        let shells = self.env.shells()?;
        let h = w.proc().clone();
        let info = match shells.info(&h) {
            Ok(i) => i,
            Err(e) => return Some(obj(json!({"error": e.to_string()}))),
        };
        if !w.c.r#match.is_empty() {
            let start = match w.base {
                Base::Offset(o) => o,
                _ => w.c.since,
            };
            let (text, offset, _total) = shells.read(&h, start).await;
            w.base = Base::Offset(offset);
            if !text.is_empty() {
                sources.insert(("origin".into(), ORIGIN_OUTPUT.into()));
                let m = matcher(&w.c);
                let hits: Vec<String> = py::splitlines(&text)
                    .into_iter()
                    .filter(|l| m(l))
                    .map(|l| clip(l, LINE_CHARS))
                    .take(MATCH_LINES)
                    .collect();
                if !hits.is_empty() {
                    return Some(obj(json!({"matched": hits, "running": info.running,
                                           "exit_code": info.exit_code, "output_offset": offset})));
                }
            }
        }
        w.seen = obj(json!({"running": info.running}));
        if !info.running {
            let (text, _offset, total) = shells.tail(&h, OUTPUT_TAIL).await;
            if !text.is_empty() {
                sources.insert(("origin".into(), ORIGIN_OUTPUT.into()));
            }
            let mut out = obj(
                json!({"exited": true, "exit_code": info.exit_code, "output_tail": text,
                                     "output_total": total}),
            );
            if let Some(s) = info.stopped.filter(|s| !s.is_empty()) {
                out.insert("stopped_by_ferb".into(), json!(s));
            }
            return Some(out);
        }
        None
    }

    fn file(&self, w: &mut Watch) -> Option<Map<String, Value>> {
        let path = match &w.handle {
            Handle::Path(p) => p.clone(),
            _ => return None,
        };
        let st = match std::fs::symlink_metadata(&path) {
            Ok(m) => Some(m),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                let text = e.to_string();
                let strerror = text.split(" (os error").next().unwrap_or(&text).to_string();
                return Some(obj(json!({"error": format!("{strerror}: {path}")})));
            }
        };
        let mut facts = obj(json!({"path": path, "exists": st.is_some()}));
        let key = st.as_ref().map(|m| {
            let (secs, nanos) = mtime(m);
            (
                m.len(),
                i128::from(secs) * 1_000_000_000 + i128::from(nanos),
            )
        });
        if let Some(m) = &st {
            let (secs, nanos) = mtime(m);
            facts.insert("bytes".into(), json!(m.len()));
            facts.insert("modified".into(), json!(self.modified(secs, nanos)));
        }
        w.seen = facts.clone();
        match w.c.state.as_str() {
            "exists" => return st.is_some().then_some(facts),
            "gone" => return st.is_none().then_some(facts),
            _ => {}
        }
        let same = matches!((&w.base, key), (Base::File(Some(b)), Some(k)) if *b == k);
        if key.is_none() || !same {
            w.base = Base::File(key);
            w.settled_since = key.map(|_| self.env.monotonic());
            return None;
        }
        // Fixed against Python: `w.settled_since or monotonic()` reads a stretch that began at monotonic 0.0 as
        // not begun, so a file first seen then never settles; an unset stretch is None here.
        let mono = self.env.monotonic();
        let held = mono - w.settled_since.unwrap_or(mono);
        if held >= w.c.settle_s {
            facts.insert("unchanged_s".into(), json!(py::round_even(held)));
            Some(facts)
        } else {
            None
        }
    }

    /// `datetime.fromtimestamp(st_mtime).astimezone().isoformat(timespec="seconds")`.
    fn modified(&self, secs: i64, nanos: u32) -> String {
        let f = secs as f64 + f64::from(nanos) * 1e-9;
        let whole = f.floor();
        let mut us = ((f - whole) * 1e6).round_ties_even() as i64;
        let mut s = whole as i64;
        if us >= 1_000_000 {
            s += 1;
            us -= 1_000_000;
        }
        let t = chrono::DateTime::from_timestamp(s, (us * 1000) as u32)
            .unwrap_or_default()
            .fixed_offset();
        py::isoformat(&self.env.zone().local(&t), py::Spec::Seconds)
    }

    async fn run_script(
        &self,
        w: &mut Watch,
        ctx: &ToolContext,
        sources: &mut Sources,
    ) -> Option<Map<String, Value>> {
        let shells = self.env.shells()?;
        let c = w.c.clone();
        let cwd =
            match shells.working_directory(Some(c.cwd.as_str()).filter(|s| !s.is_empty()), ctx) {
                Ok(d) => d,
                Err(e) => return Some(obj(json!({"error": e.to_string()}))),
            };
        let h = match shells
            .start(&c.command, &cwd, &ctx.task_id, &ctx.folders)
            .await
        {
            Ok(h) => h,
            Err(e) => return Some(obj(json!({"error": e.to_string()}))),
        };
        let code = shells.wait(&h, SCRIPT_RUN_S).await;
        if code.is_none() {
            shells.kill(&h).await;
        }
        let (text, _, _total) = shells.tail(&h, OUTPUT_TAIL).await;
        for host in shells.network(&h) {
            sources.insert(("web".into(), format!("https://{host}")));
        }
        if !text.is_empty() {
            sources.insert(("origin".into(), ORIGIN_OUTPUT.into()));
        }
        let info = shells.info(&h);
        // finally: its end is this wait's, the next run counts from this one's end, the run goes with it
        shells.exit_told(&h);
        w.due = self.env.monotonic() + Self::interval(&c);
        if let Ok(i) = shells.info(&h) {
            if i.running {
                shells.kill(&h).await;
            }
        }
        shells.release(&h).await;
        let info = match info {
            Ok(i) => i,
            Err(e) => return Some(obj(json!({"error": e.to_string()}))),
        };
        let runs = w.seen.get("runs").and_then(Value::as_i64).unwrap_or(0) + 1;
        w.seen = obj(json!({"runs": runs, "exit_code": code, "output_tail": text}));
        if code == Some(0) {
            return Some(obj(
                json!({"exit_code": 0, "output_tail": text, "runs": runs}),
            ));
        }
        if code.is_some_and(|c| NOT_FOUND.contains(&c)) && runs == 1 {
            let code = code.unwrap_or_default();
            return Some(obj(json!({
                "error": format!("the script couldn't run (the shell's exit code {code}: not found or not executable)"),
                "exit_code": code, "output_tail": text})));
        }
        if let Some(stopped) = info.stopped.filter(|s| !s.is_empty()) {
            return Some(obj(
                json!({"error": "FERB stopped the script", "stopped_by_ferb": stopped,
                                   "output_tail": text}),
            ));
        }
        if code.is_none() {
            w.seen.insert(
                "timed_out".into(),
                json!(format!(
                    "a run took over {} s and was stopped",
                    py::fmt_g(SCRIPT_RUN_S)
                )),
            );
        }
        None
    }

    async fn tab(
        &self,
        w: &mut Watch,
        ctx: &ToolContext,
        sources: &mut Sources,
    ) -> Option<Map<String, Value>> {
        let env = self.env;
        let c = w.c.clone();
        let driver = env.browser(ctx.identity)?;
        let r = match driver.owned_by(&c.tab, &ctx.task_id) {
            Ok(r) => r,
            Err(e) => return self.page_failed(w, e),
        };
        if c.kind == "url" {
            let url = driver.tab_url(&c.tab).unwrap_or_default();
            w.seen = obj(json!({"url": url}));
            if matches!(w.base, Base::None) {
                w.base = Base::Text(url.clone());
            }
            let base = match &w.base {
                Base::Text(b) => b.clone(),
                _ => String::new(),
            };
            let hit = if c.r#match.is_empty() {
                url != base
            } else {
                matcher(&c)(&url)
            };
            if hit {
                sources.insert(("web".into(), url.clone()));
                return Some(obj(json!({"url": url})));
            }
            return None;
        }
        if c.kind == "download" {
            let found = match driver.downloads_for(&r) {
                None => Ok(vec![]),
                Some(f) => f.await,
            };
            return match found {
                Ok(found) if !found.is_empty() => {
                    for f in &found {
                        let p = f
                            .get("path")
                            .filter(|v| ferb_py::value::truthy(v))
                            .or_else(|| f.get("source_url").filter(|v| ferb_py::value::truthy(v)))
                            .map(ferb_py::value::py_str)
                            .unwrap_or_default();
                        sources.insert(("download".into(), p));
                    }
                    Some(obj(json!({"downloads": found})))
                }
                Ok(_) => None,
                Err(e) => self.page_failed(w, e),
            };
        }
        if c.reload {
            if let Some(url) = driver.tab_url(&c.tab).filter(|u| !u.is_empty()) {
                if let Err(e) = driver.navigate(&r, &url).await {
                    return self.page_failed(w, e);
                }
            }
        }
        let snap = match driver.snapshot(&r).await {
            Ok(s) => s,
            Err(e) => return self.page_failed(w, e),
        };
        w.errors = 0;
        sources.insert(("web".into(), snap.url.clone()));
        if ctx.identity == Identity::Owner {
            sources.insert(("owner_account".into(), format!("{}|", env.host(&snap.url))));
        }
        let text = snap.text.clone().unwrap_or_default();
        let lines: Vec<String> = py::splitlines(&text)
            .into_iter()
            .filter(|l| !py::strip(l).is_empty())
            .map(str::to_string)
            .collect();
        w.seen = obj(
            json!({"url": snap.url, "title": clip(snap.title.as_deref().unwrap_or(""), LINE_CHARS)}),
        );
        if c.kind == "page" {
            if !c.r#match.is_empty() {
                let m = matcher(&c);
                let hits: Vec<String> = lines
                    .iter()
                    .filter(|l| m(l))
                    .map(|l| clip(l, LINE_CHARS))
                    .take(MATCH_LINES)
                    .collect();
                return (!hits.is_empty()).then(|| obj(json!({"url": snap.url, "matched": hits})));
            }
            let Base::Lines(base) = &w.base else {
                w.base = Base::Lines(lines);
                return None;
            };
            if lines != *base {
                let new: Vec<String> = lines
                    .iter()
                    .filter(|l| !base.contains(l))
                    .map(|l| clip(l, LINE_CHARS))
                    .take(MATCH_LINES)
                    .collect();
                return Some(obj(
                    json!({"url": snap.url, "changed": true, "new_lines": new}),
                ));
            }
            return None;
        }
        // mail
        let wanted: Vec<String> = [&c.sender, &c.subject, &c.text]
            .into_iter()
            .filter(|s| !s.is_empty())
            .map(|s| py::casefold(s))
            .collect();
        let naming: Vec<String> = lines
            .into_iter()
            .filter(|l| {
                let f = py::casefold(l);
                wanted.iter().all(|s| f.contains(s.as_str()))
            })
            .collect();
        let Base::Lines(base) = &w.base else {
            w.base = Base::Lines(naming);
            return None;
        };
        let mut before = base.clone();
        let mut new = Vec::new();
        for line in naming {
            if let Some(i) = before.iter().position(|b| *b == line) {
                before.remove(i);
            } else {
                new.push(clip(&line, LINE_CHARS));
            }
        }
        if new.is_empty() {
            None
        } else {
            new.truncate(MATCH_LINES);
            Some(obj(json!({"url": snap.url, "new_lines": new})))
        }
    }

    /// A browser call that failed: refused ends the wait with its words; any other failure (a hung page) is
    /// tried again next time, and three in a row end the wait.
    fn page_failed(&self, w: &mut Watch, e: PageError) -> Option<Map<String, Value>> {
        let (kind, message) = match e {
            PageError::Refused(m) => return Some(obj(json!({"error": m}))),
            PageError::Failed { kind, message } => (kind, message),
        };
        w.errors += 1;
        w.seen = obj(json!({"error": format!("{kind}: {message}")}));
        (w.errors >= PAGE_ERRORS).then(|| {
            obj(json!({"error": format!("the page couldn't be read {PAGE_ERRORS} times in a row: {message}")}))
        })
    }
}

fn mtime(m: &std::fs::Metadata) -> (i64, u32) {
    match m.modified() {
        Ok(t) => match t.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => (d.as_secs() as i64, d.subsec_nanos()),
            Err(e) => {
                let d = e.duration();
                let (s, n) = (d.as_secs() as i64, d.subsec_nanos());
                if n == 0 {
                    (-s, 0)
                } else {
                    (-s - 1, 1_000_000_000 - n)
                }
            }
        },
        Err(_) => (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_read_as_pathlib_writes_them() {
        assert_eq!(posix_norm("/w/./out//old.txt/"), "/w/out/old.txt");
        assert_eq!(posix_norm("//x"), "//x");
        assert_eq!(posix_norm("///x"), "/x");
        assert_eq!(posix_name("/"), "");
        assert_eq!(posix_name("a/b.pdf"), "b.pdf");
    }

    #[test]
    fn a_command_is_said_as_its_first_line_cut_at_a_word() {
        assert_eq!(command_said("   \n  build   all\t`x`  \n"), "`build all x`");
        assert_eq!(
            command_said("codex exec -s danger-full-access -C ~/.ferb/workspace 'fix'"),
            "`codex exec -s danger-full-access -C …`"
        );
        assert_eq!(command_said("\n\n"), "");
    }
}
