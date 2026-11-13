//! `wait.for` (`ferb/clock/wait.py`, `add_wait`) and `time.sleep_until` (`add_time`, schedules.py `_sleep_until`) over
//! this daemon's shell, browsers, timers and gateway.
//!
//! Without the supervisor (not wired in this build) a wait's live line, the task's corrections and the worker's
//! "waiting" mark have nobody to tell: `status` does nothing and `steered` is 0, as Python's daemon before its
//! supervisor is built.

use std::path::PathBuf;
use std::sync::Arc;

use ferb_clock::py::Time;
use ferb_clock::wait::{Browser, PageError, ProcHandle, ProcInfo, Shells, Snap, WaitEnv};
use ferb_core::errors::FerbError;
use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{ActionSpec, Identity, TabRef, ToolContext};
use ferb_gateway::tool::{Args, Tool, ToolError};
use ferb_shell::manager::ShellManager;
use serde_json::{Map, Value};

use super::{Catalogued, Parts};

fn sh(h: &ProcHandle) -> ferb_shell::manager::ProcHandle {
    ferb_shell::manager::ProcHandle {
        id: h.id.clone(),
        owner_task: h.owner_task.clone(),
    }
}

fn ch(h: ferb_shell::manager::ProcHandle) -> ProcHandle {
    ProcHandle {
        id: h.id,
        owner_task: h.owner_task,
    }
}

fn refused(e: ferb_shell::manager::ShellError) -> FerbError {
    FerbError::Refused(e.to_string())
}

fn info(i: ferb_shell::manager::ProcInfo, stopped: Option<String>) -> ProcInfo {
    ProcInfo {
        id: i.id,
        command: i.command,
        running: i.running,
        exit_code: i.exit_code.map(i64::from),
        stopped,
        owner_task: i.owner_task,
    }
}

/// FERB's shell as a wait reads it.
pub struct WaitShells {
    pub shells: Arc<ShellManager>,
    pub home: String,
}

impl Shells for WaitShells {
    fn handle(&self, id: &str, task: &str) -> ProcHandle {
        ch(self.shells.handle(id, task))
    }
    fn info(&self, h: &ProcHandle) -> Result<ProcInfo, FerbError> {
        let stopped = self.shells.stopped(&h.id);
        self.shells
            .info(&sh(h))
            .map(|i| info(i, stopped))
            .map_err(refused)
    }
    fn list(&self) -> Vec<ProcInfo> {
        self.shells
            .list()
            .into_iter()
            .map(|i| {
                let stopped = self.shells.stopped(&i.id);
                info(i, stopped)
            })
            .collect()
    }
    fn told(&self, id: &str) -> bool {
        self.shells.told(id)
    }
    fn exit_told(&self, h: &ProcHandle) {
        self.shells.exit_told(&sh(h))
    }
    fn read<'a>(
        &'a self,
        h: &'a ProcHandle,
        since: usize,
    ) -> BoxFuture<'a, (String, usize, usize)> {
        Box::pin(async move {
            match self.shells.read(&sh(h), since as i64, None).await {
                Ok((t, o, n)) => (t, o as usize, n as usize),
                Err(_) => (String::new(), since, since),
            }
        })
    }
    fn tail<'a>(
        &'a self,
        h: &'a ProcHandle,
        limit: usize,
    ) -> BoxFuture<'a, (String, usize, usize)> {
        Box::pin(async move {
            match self.shells.tail(&sh(h), limit as u64).await {
                Ok((t, o, n)) => (t, o as usize, n as usize),
                Err(_) => (String::new(), 0, 0),
            }
        })
    }
    fn wait<'a>(&'a self, h: &'a ProcHandle, timeout: f64) -> BoxFuture<'a, Option<i64>> {
        Box::pin(async move {
            self.shells
                .wait(&sh(h), timeout)
                .await
                .ok()
                .flatten()
                .map(i64::from)
        })
    }
    fn start<'a>(
        &'a self,
        command: &'a str,
        cwd: &'a str,
        task: &'a str,
        folders: &'a [String],
    ) -> BoxFuture<'a, Result<ProcHandle, FerbError>> {
        Box::pin(async move {
            self.shells
                .start(
                    command,
                    std::path::Path::new(cwd),
                    task,
                    false,
                    None,
                    None,
                    folders,
                )
                .await
                .map(ch)
                .map_err(refused)
        })
    }
    fn kill<'a>(&'a self, h: &'a ProcHandle) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let _ = self.shells.kill(&sh(h)).await;
        })
    }
    fn release<'a>(&'a self, h: &'a ProcHandle) -> BoxFuture<'a, ()> {
        Box::pin(async move { self.shells.release(&sh(h)).await })
    }
    fn network(&self, h: &ProcHandle) -> Vec<String> {
        let Ok(facts) = self.shells.network(&sh(h)) else {
            return Vec::new();
        };
        facts
            .get("destinations")
            .and_then(Value::as_array)
            .map(|d| {
                d.iter()
                    .filter_map(|x| x.get("host").map(ferb_py::value::py_str))
                    .collect()
            })
            .unwrap_or_default()
    }
    fn working_directory(&self, cwd: Option<&str>, ctx: &ToolContext) -> Result<String, FerbError> {
        let resolver = super::shell::Resolver {
            home: self.home.clone(),
        };
        ferb_shell::tools::working_directory(cwd, ctx, &resolver)
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(refused)
    }
}

/// A browser driver as a wait reads it (tab refs travel as their JSON text).
pub struct WaitBrowser(pub Arc<ferb_browser::driver::Driver>);

fn page_err(e: ferb_browser::error::Error) -> PageError {
    match e.class() {
        "Refused" | "StaleRef" | "Withheld" => PageError::Refused(e.message()),
        kind => PageError::Failed {
            kind: kind.into(),
            message: e.message(),
        },
    }
}

fn tab_of(r: &str) -> Result<TabRef, PageError> {
    serde_json::from_str(r).map_err(|_| PageError::Refused("not this task's tab".into()))
}

impl Browser for WaitBrowser {
    fn owned_by(&self, tab: &str, task: &str) -> Result<String, PageError> {
        self.0
            .owned_by(tab, task, true)
            .map(|t| serde_json::to_string(&t).unwrap_or_default())
            .map_err(page_err)
    }
    fn tab_url(&self, tab: &str) -> Option<String> {
        self.0.tab_url(tab)
    }
    fn snapshot<'a>(&'a self, tab_ref: &'a str) -> BoxFuture<'a, Result<Snap, PageError>> {
        Box::pin(async move {
            let t = tab_of(tab_ref)?;
            let s = self.0.snapshot(&t).await.map_err(page_err)?;
            Ok(Snap {
                url: s.url,
                title: Some(s.title),
                text: Some(s.text),
            })
        })
    }
    fn navigate<'a>(
        &'a self,
        tab_ref: &'a str,
        url: &'a str,
    ) -> BoxFuture<'a, Result<(), PageError>> {
        Box::pin(async move {
            let t = tab_of(tab_ref)?;
            self.0.navigate(&t, url, None).await.map_err(page_err)
        })
    }
    fn downloads_for<'a>(
        &'a self,
        tab_ref: &'a str,
    ) -> Option<BoxFuture<'a, Result<Vec<Value>, PageError>>> {
        Some(Box::pin(async move {
            let t = tab_of(tab_ref)?;
            self.0
                .downloads_for(&t, 0.0)
                .await
                .map(|l| l.into_iter().map(Value::Object).collect())
                .map_err(page_err)
        }))
    }
}

/// `WaitDeps` over this daemon.
pub struct Env {
    pub parts: Parts,
    pub shells: Option<WaitShells>,
    pub owner: Option<WaitBrowser>,
    pub ferb: Option<WaitBrowser>,
}

impl WaitEnv for Env {
    fn now(&self) -> Time {
        self.parts.clock().now()
    }
    fn monotonic(&self) -> f64 {
        self.parts.clock().monotonic()
    }
    fn pause(&self, seconds: f64) -> BoxFuture<'_, ()> {
        Box::pin(tokio::time::sleep(std::time::Duration::from_secs_f64(
            seconds.max(0.0),
        )))
    }
    fn limit(&self, task: &str) -> Option<Time> {
        self.parts.task(task).and_then(|t| t.deadline)
    }
    fn shells(&self) -> Option<&dyn Shells> {
        self.shells.as_ref().map(|s| s as &dyn Shells)
    }
    fn browser(&self, identity: Identity) -> Option<&dyn Browser> {
        match identity {
            Identity::Ferb => self.ferb.as_ref().map(|b| b as &dyn Browser),
            Identity::Owner => self.owner.as_ref().map(|b| b as &dyn Browser),
        }
    }
    fn status(&self, _worker: &str, _task: &str, _until: Option<&Time>, _line: Option<&str>) {}
    fn steered(&self, _task: &str) -> i64 {
        0
    }
    fn ending(&self) -> Option<String> {
        self.parts.shutting_down().then(|| "restart".to_string())
    }
    fn ended(&self, task: &str, data: Value) {
        // `_wait_ended`: journaled with its tree
        let tree = self
            .parts
            .task(task)
            .and_then(|t| t.tree_id)
            .unwrap_or_else(|| task.split('.').next().unwrap_or(task).to_string());
        let mut m = Map::new();
        m.insert("task_id".into(), Value::String(task.into()));
        m.insert("tree".into(), Value::String(tree));
        if let Value::Object(d) = data {
            m.extend(d);
        }
        self.parts.journal().note("wait.ended", Value::Object(m));
    }
    fn failed_act(&self, worker: &str) -> Option<Map<String, Value>> {
        self.parts.gateway().and_then(|g| g.failed_act(worker))
    }
    fn zone(&self) -> ferb_clock::zone::Zone {
        self.parts.zoned().zone()
    }
    fn home(&self) -> PathBuf {
        self.parts.user_home().clone()
    }
    fn host(&self, url: &str) -> String {
        ferb_policy::urls::parse(url)
            .map(|u| u.host)
            .unwrap_or_default()
    }
}

fn env(p: &Parts) -> Env {
    Env {
        parts: p.clone(),
        shells: p.shells().map(|s| WaitShells {
            shells: s,
            home: p.user_home().to_string_lossy().into_owned(),
        }),
        owner: p.owner_driver().map(WaitBrowser),
        ferb: p.ferb_driver().map(WaitBrowser),
    }
}

/// `wait.for`.
pub fn wait_for(p: &Parts) -> Arc<dyn Tool> {
    let parts = p.clone();
    Arc::new(Catalogued {
        name: "wait.for",
        describe: Arc::new(|_a: &Args| ActionSpec::new("read")),
        run: Arc::new(move |a: &Args, ctx: &ToolContext| {
            let p = parts.clone();
            let a = a.clone();
            let ctx = ctx.clone();
            Box::pin(async move {
                let input = ferb_clock::wait::WaitInput::from_json(&Value::Object(a))
                    .map_err(|e| ToolError::failed("ValueError", e))?;
                let e = env(&p);
                Ok(ferb_clock::wait::WaitFor::new(&e)
                    .run(&input, &ctx)
                    .await
                    .into())
            })
        }),
    })
}

/// `_sleep_until`: a durable "sleep" timer; the sleep ends when the timer loop marks it fired (it is redelivered
/// after a restart), or when the call is cancelled (the task ended: the timer is cancelled).
///
/// ferb-daemon keeps the timer loop's sleepers private (`State::sleepers`, crate-private), so the sleeper here reads
/// the timer's row once a second instead of being woken: up to a second later than Python's (seam mismatch, listed).
pub struct Sleeper {
    pub parts: Parts,
}

impl ferb_clock::tools::Sleeper for Sleeper {
    fn sleep<'a>(
        &'a self,
        _worker: &'a str,
        task: &'a str,
        at: &'a Time,
        _span: Option<&'a str>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let p = &self.parts;
            let added = {
                let db = p.db();
                let d = crate::basics::lock(&db);
                let ids = p.ids();
                let clock = p.clock();
                let timers =
                    ferb_clock::timers::Timers::new(&d, clock.as_ref(), move || ids.new_id());
                timers.add("sleep", at, "", Some(task), None, false, None, "", None)
            };
            let Ok(timer) = added else {
                return;
            };
            struct Cancel(Parts, String);
            impl Drop for Cancel {
                fn drop(&mut self) {
                    let db = self.0.db();
                    let d = crate::basics::lock(&db);
                    let ids = self.0.ids();
                    let clock = self.0.clock();
                    let timers =
                        ferb_clock::timers::Timers::new(&d, clock.as_ref(), move || ids.new_id());
                    let _ = timers.cancel(&self.1);
                }
            }
            let _cancel = Cancel(p.clone(), timer.id.clone());
            loop {
                let done = {
                    let db = p.db();
                    let d = crate::basics::lock(&db);
                    d.conn()
                        .query_row(
                            "SELECT state != 'pending' FROM timers WHERE id = ?",
                            [&timer.id],
                            |r| r.get::<_, bool>(0),
                        )
                        .unwrap_or(true)
                };
                if done {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        })
    }
}

/// `time.sleep_until`.
pub fn sleep_until(p: &Parts) -> Arc<dyn Tool> {
    let parts = p.clone();
    Arc::new(Catalogued {
        name: ferb_clock::tools::SLEEP_UNTIL,
        describe: Arc::new(|a: &Args| {
            ferb_clock::tools::describe(ferb_clock::tools::SLEEP_UNTIL, &Value::Object(a.clone()))
        }),
        run: Arc::new(move |a: &Args, ctx: &ToolContext| {
            let p = parts.clone();
            let a = Value::Object(a.clone());
            let ctx = ctx.clone();
            Box::pin(async move {
                // its `now` isn't Sync: run on a blocking thread, driven by this runtime (cancelled with the call:
                // the sleep's timer goes when the call's future is dropped)
                // its `now` isn't Sync: on a thread of its own (dropping this call ends the sleep and cancels its timer)
                let job = super::mac::off_thread(move || {
                    Box::pin(async move {
                        let clock = p.clock();
                        let now = move || clock.now();
                        let limit = p.task(&ctx.task_id).and_then(|t| t.deadline);
                        let sleeper = Sleeper { parts: p.clone() };
                        let zone = p.zoned().zone();
                        ferb_clock::tools::sleep_until(Some(&sleeper), &a, &now, limit, &zone, &ctx)
                            .await
                    })
                });
                match job.await {
                    Some(r) => Ok(r.into()),
                    None => Err(ToolError::failed(
                        "RuntimeError",
                        "the sleep's thread ended",
                    )),
                }
            })
        }),
    })
}
