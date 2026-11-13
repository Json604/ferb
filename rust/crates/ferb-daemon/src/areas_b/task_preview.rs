//! The live preview of a task's work (kb area app): masked frames of the one surface (a task's browser tab, or the
//! app window a call targets) its latest call works on, for FERB.app only. Pixels live in memory only: never
//! journaled, saved, traced or logged. Port of `ferb/daemon/task_preview.py`.
//!
//! Before each call the gateway declares the surface ([`TaskPreviews::declare`]); nothing is resolved then. The first
//! frame request resolves it. A capture never queues on the worker's resource locks: it takes them only when free
//! (`try_hold`, DMN-403, DMN-406), else the last frame is returned with the state. One capture at a time per task,
//! within `limit_s`; a request whose caller left, or that ran out of time, cancels the capture when nobody else waits
//! for it. Each answer carries the state (APP-404), the frame's `seq` and `age_ms`, and leaves the image out when the
//! app already has that frame (`have`, DMN-407). Parallel steps keep their own surface (DMN-404) and take the task's
//! one capture in turn (DMN-584). Frames and capture time are counted per surface kind (APP-405, DMN-408); a capture
//! that runs out of time or fails is noted (preview.slow / preview.failed, bounded).
//!
//! Seams: the surface's capture comes with its [`PreviewTarget`] (`ferb_mac_hands::point`, the same type the Mac and
//! browser capabilities resolve to); the frame sanitizer ([`Sanitize`]), the locks' `try_hold` ([`TryHold`]), the
//! Screen Recording check, the journal note, the task's state ([`Status`], the supervisor's `preview_status`) and the
//! time ([`PreviewClock`]: monotonic readings and the request's time limit).

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use ferb_mac_hands::error::HandError;
use ferb_mac_hands::point::{preview_phase, FrameFn, PreviewCtx, PreviewTarget, PREVIEW};
use serde_json::{json, Map, Value};
use tokio::sync::watch;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// A request's time limit: under the notch's 10 s reply wait.
pub const PREVIEW_LIMIT_S: f64 = 8.0;
pub const ELSEWHERE: &str = "Working on another part of this task";
pub const SLOW_TEXT: &str = "The capture took too long";
pub const LOCKED_TEXT: &str =
    "The screen is locked or asleep, so macOS lets nobody capture windows";
/// DMN-584: a surface turned away while another step's capture ran is captured next, if asked again within this long.
pub const TURN_S: f64 = 1.0;
/// A noted preview problem: the first at once, then one row per window with a count.
pub const NOTE_EVERY_S: f64 = 600.0;

/// The frame sanitizer (text recognition masking, L13): the masked frame, or the error's class name.
pub type Sanitize = Arc<dyn Fn(Vec<u8>) -> BoxFuture<Result<Vec<u8>, String>> + Send + Sync>;
/// `try_hold(keys, holder)` (the daemon's `locks.try_hold(..., during_reads=True)`): a guard holding every lock (they
/// are released when it drops), None when any is busy, or the error's class name (an unknown lock kind: ValueError).
pub type TryHold =
    Arc<dyn Fn(&[String], &str) -> Result<Option<Box<dyn Send>>, String> + Send + Sync>;
/// What the task is doing now by FERB's records (`status(root, worker)`): None while it works, else (state, line).
pub type Status = Arc<dyn Fn(&str, &str) -> Option<(String, String)> + Send + Sync>;
/// A preview problem for the journal (`preview.slow` / `preview.failed`, never a frame, title or URL).
pub type Note = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// Time as the previews read it: `time.monotonic()`, and a request's wait (`asyncio.wait(..., timeout)`).
pub trait PreviewClock: Send + Sync {
    fn monotonic(&self) -> f64;
    fn sleep(&self, seconds: f64) -> BoxFuture<()>;
}

/// The real clock: a monotonic reading since its making, tokio's timer.
pub struct SystemClock(std::time::Instant);

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock(std::time::Instant::now())
    }
}

impl PreviewClock for SystemClock {
    fn monotonic(&self) -> f64 {
        self.0.elapsed().as_secs_f64()
    }
    fn sleep(&self, seconds: f64) -> BoxFuture<()> {
        Box::pin(tokio::time::sleep(std::time::Duration::from_secs_f64(
            seconds.max(0.0),
        )))
    }
}

/// One answer: the fields in Python's order, and the frame (memory only) when there is one. The image's place is right
/// after `step`; the RPC handler sends it base64 at the end, after redacting the rest.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewAnswer {
    pub fields: Map<String, Value>,
    pub image: Option<Vec<u8>>,
}

impl PreviewAnswer {
    fn new(fields: Value) -> Self {
        PreviewAnswer {
            fields: fields.as_object().cloned().unwrap_or_default(),
            image: None,
        }
    }

    pub fn seq(&self) -> Option<i64> {
        self.fields.get("seq").and_then(Value::as_i64)
    }

    /// The answer as Python's dict, the image (written by `image`) in its place.
    pub fn to_value(&self, image: impl Fn(&[u8]) -> Value) -> Value {
        let mut out = Map::new();
        for (k, v) in &self.fields {
            out.insert(k.clone(), v.clone());
            if k == "step" {
                if let Some(i) = &self.image {
                    out.insert("image".into(), image(i));
                }
            }
        }
        Value::Object(out)
    }
}

/// A count or a time: Python's int and float, kept apart (they print differently).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Num {
    I(i64),
    F(f64),
}

impl Num {
    fn add(self, n: Num) -> Num {
        match (self, n) {
            (Num::I(a), Num::I(b)) => Num::I(a + b),
            (a, b) => Num::F(a.f() + b.f()),
        }
    }
    fn f(self) -> f64 {
        match self {
            Num::I(i) => i as f64,
            Num::F(f) => f,
        }
    }
    /// `round(v, 1)`: an int stays an int.
    fn rounded(self) -> Value {
        match self {
            Num::I(i) => json!(i),
            Num::F(f) => json!(ferb_trace::py::round_digits(f, 1)),
        }
    }
}

#[derive(Default)]
struct Counts {
    requests: i64,
    frames: i64,
    capture_ms: f64,
    cached: i64,
    busy: i64,
    kinds: BTreeMap<String, Num>,
    peaks: BTreeMap<String, Num>,
}

impl Counts {
    fn add(&mut self, key: String, n: Num) {
        let v = self.kinds.get(&key).copied().unwrap_or(Num::I(0)).add(n);
        self.kinds.insert(key, v);
    }
    fn one(&mut self, key: String) {
        self.add(key, Num::I(1));
    }
    /// `max(self.peaks.get(key, 0.0), value)`: the first stays unless the new one is greater.
    fn peak(&mut self, key: String, value: Num) {
        let old = self.peaks.get(&key).copied().unwrap_or(Num::F(0.0));
        if value.f() > old.f() {
            self.peaks.insert(key, value);
        } else {
            self.peaks.insert(key, old);
        }
    }
}

struct Surface {
    target: PreviewTarget,
    locks: Vec<String>,
    capture: Option<FrameFn>,
    frame: Option<Vec<u8>>,
    note: String,
    problem: Option<(String, String)>,
    seq: i64,
    at: f64,
    asked: Option<f64>,
}

impl Surface {
    fn new(target: PreviewTarget, locks: Vec<String>) -> Surface {
        Surface {
            target,
            locks,
            capture: None,
            frame: None,
            note: String::new(),
            problem: None,
            seq: 0,
            at: 0.0,
            asked: None,
        }
    }
    fn kind(&self) -> &'static str {
        if self.target.scope.starts_with("tab:") {
            "tab"
        } else {
            "window"
        }
    }
}

type Sf = Arc<Mutex<Surface>>;

#[derive(Default)]
struct Worker {
    current: Option<String>,
    label: String,
    surface: Option<Sf>,
}

type Wk = Arc<Mutex<Worker>>;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Ended {
    Fresh,
    Busy,
    Failed,
    Cancelled,
}

struct Job {
    id: u64,
    surface: Sf,
    abort: tokio::task::AbortHandle,
    tx: Arc<watch::Sender<Option<Ended>>>,
    waiters: usize,
    phase: Arc<Mutex<String>>,
}

impl Job {
    fn done(&self) -> bool {
        self.tx.borrow().is_some()
    }
    fn cancel(&self) {
        self.abort.abort();
        self.tx.send_if_modified(|v| {
            if v.is_none() {
                *v = Some(Ended::Cancelled);
                true
            } else {
                false
            }
        });
    }
}

#[derive(Default)]
struct State {
    roots: Vec<(String, Vec<(String, Wk)>)>,
    jobs: HashMap<String, Job>,
    turned: HashMap<String, (Sf, f64)>,
    counts: Counts,
    noted: HashMap<String, (f64, i64)>,
    seq: i64,
    next_job: u64,
}

impl State {
    fn workers(&self, root: &str) -> Option<&Vec<(String, Wk)>> {
        self.roots.iter().find(|(r, _)| r == root).map(|(_, w)| w)
    }
    fn has_root(&self, root: &str) -> bool {
        self.roots.iter().any(|(r, _)| r == root)
    }
}

struct Inner {
    sanitize: Sanitize,
    try_hold: TryHold,
    allowed: Arc<dyn Fn() -> bool + Send + Sync>,
    limit_s: f64,
    note: Note,
    clock: Arc<dyn PreviewClock>,
    status: Mutex<Status>,
    state: Mutex<State>,
}

/// The live previews of every running task (`TaskPreviews`).
#[derive(Clone)]
pub struct TaskPreviews {
    inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `_step_of(worker)`: the plan step a step's task id names.
pub fn step_of(worker: &str) -> String {
    let rest = worker.split_once('.').map_or(worker, |(_, r)| r);
    rest.strip_prefix("step.").unwrap_or(rest).to_owned()
}

/// `_error_name(e)`: by type, and a capture error's domain and code (its text can hold a window's title).
pub fn error_name(e: &HandError) -> String {
    match e {
        HandError::Capture {
            domain,
            code: Some(code),
            ..
        } => format!("{} ({domain} {code})", e.class()),
        other => other.class().to_owned(),
    }
}

/// How one capture failed.
enum Fail {
    Busy,
    Locked,
    Withheld(String),
    Gone(String),
    /// (type name, the journal's error name)
    Failed(String, String),
}

impl From<HandError> for Fail {
    fn from(e: HandError) -> Fail {
        match e {
            HandError::CaptureBusy(_) => Fail::Busy,
            HandError::ScreenOff(_) => Fail::Locked,
            HandError::Withheld(t) => Fail::Withheld(t),
            HandError::Refused(t) | HandError::StaleRef(t) | HandError::NoTarget(t) => {
                Fail::Gone(t)
            }
            other => Fail::Failed(other.class().to_owned(), error_name(&other)),
        }
    }
}

/// Adds a capture's phase times to the counters however it ends (Python's `finally`), cancelled included.
struct PhaseTally {
    inner: Arc<Inner>,
    ctx: Arc<PreviewCtx>,
    kind: &'static str,
}

impl Drop for PhaseTally {
    fn drop(&mut self) {
        let phases = lock(&self.ctx.phases).clone();
        let mut st = lock(&self.inner.state);
        for (name, ms) in phases {
            st.counts
                .add(format!("{}_{name}_ms", self.kind), Num::F(ms));
        }
    }
}

/// A request waiting on a capture: when the request is dropped (its caller left), Python's `finally` runs.
struct Waiting {
    inner: Arc<Inner>,
    root: String,
    job: u64,
    armed: bool,
}

impl Waiting {
    fn leave(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        let mut st = lock(&self.inner.state);
        let Some(j) = st.jobs.get_mut(&self.root).filter(|j| j.id == self.job) else {
            return;
        };
        j.waiters -= 1;
        if j.waiters == 0 && !j.done() {
            j.cancel(); // nobody waits for it any more (left, or timed out)
        }
        if j.done() || j.waiters == 0 {
            st.jobs.remove(&self.root);
        }
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        self.leave();
    }
}

impl TaskPreviews {
    pub fn new(
        sanitize: Sanitize,
        try_hold: TryHold,
        allowed: Arc<dyn Fn() -> bool + Send + Sync>,
        limit_s: f64,
        note: Note,
        clock: Arc<dyn PreviewClock>,
    ) -> TaskPreviews {
        TaskPreviews {
            inner: Arc::new(Inner {
                sanitize,
                try_hold,
                allowed,
                limit_s,
                note,
                clock,
                status: Mutex::new(Arc::new(|_: &str, _: &str| None)),
                state: Mutex::new(State::default()),
            }),
        }
    }

    /// Sets what the task is doing now (the supervisor's `preview_status`). It must not call back into the previews.
    pub fn set_status(&self, status: Status) {
        *lock(&self.inner.status) = status;
    }

    fn status(&self) -> Status {
        lock(&self.inner.status).clone()
    }

    // ---------- the gateway's side: what each call works on ----------

    /// Before a call of `worker` (a step's task id) runs: the surface it works on (None: no surface). Nothing is
    /// resolved or captured here. A new surface replaces the worker's last one only once it has a frame of its own;
    /// until then the last frame stays, frozen.
    pub fn declare(
        &self,
        root: &str,
        worker: &str,
        target: Option<PreviewTarget>,
        locks: &[String],
    ) {
        let mut st = lock(&self.inner.state);
        let at = match st.roots.iter().position(|(r, _)| r == root) {
            Some(at) => at,
            None => {
                st.roots.push((root.to_owned(), vec![]));
                st.roots.len() - 1
            }
        };
        let workers = &mut st.roots[at].1;
        let w = match workers.iter().position(|(k, _)| k == worker) {
            Some(i) => workers.remove(i).1,
            None => Arc::new(Mutex::new(Worker::default())),
        };
        workers.push((worker.to_owned(), w.clone())); // most recent last
        let mut w = lock(&w);
        match &target {
            Some(t) => {
                w.current = Some(t.scope.clone());
                w.label = t.label.clone();
            }
            None => {
                w.current = None;
                w.label = String::new();
            }
        }
        let Some(mut target) = target else { return };
        let same = w
            .surface
            .as_ref()
            .is_some_and(|s| lock(s).target.scope == target.scope);
        if same {
            let s = w.surface.clone().expect("a surface");
            let mut s = lock(&s);
            if target.resolve.is_none() && s.target.resolve.is_some() {
                // a call naming only the app: this step's last window of it stays the picture (DMN-407)
                target.resolve = s.target.resolve.clone();
            }
            s.target = target;
            s.locks = locks.to_vec();
            s.capture = None; // resolved again, lazily
        } else if target.resolve.is_some() || w.surface.is_none() {
            let frozen = w.surface.take();
            let mut fresh = Surface::new(target, locks.to_vec());
            if let Some(f) = frozen {
                // the last frame stays until this surface has its own. Fixed against Python: it keeps its seq and
                // capture time too (Python's copy kept neither: seq 0 and an age counted from the clock's zero)
                let f = lock(&f);
                fresh.frame = f.frame.clone();
                fresh.note = f.note.clone();
                fresh.seq = f.seq;
                fresh.at = f.at;
            }
            w.surface = Some(Arc::new(Mutex::new(fresh)));
        }
    }

    /// A task ended (or was cancelled): its surfaces, frames and captures go.
    pub fn remove(&self, roots: &[String]) {
        let mut st = lock(&self.inner.state);
        let gone: Vec<String> = st
            .roots
            .iter()
            .map(|(r, _)| r.clone())
            .filter(|r| {
                roots
                    .iter()
                    .any(|t| r == t || r.starts_with(&format!("{t}.")))
            })
            .collect();
        for root in gone {
            st.roots.retain(|(r, _)| *r != root);
            st.turned.remove(&root);
            if let Some(job) = st.jobs.remove(&root) {
                job.cancel();
            }
        }
    }

    // ---------- the app's side: one answer per request ----------

    /// The preview now: `state`, a short `line`, the frame (`fresh` when captured for this request; `seq` and
    /// `age_ms`, and no image when it is the frame `have` names) and the step it shows.
    pub async fn frame(
        &self,
        root: &str,
        worker: Option<&str>,
        have: Option<i64>,
    ) -> PreviewAnswer {
        lock(&self.inner.state).counts.requests += 1;
        let mut answer = self.frame_inner(root, worker).await;
        if have.is_some() && answer.seq() == have {
            answer.image = None; // the app already shows this very frame
        }
        answer
    }

    async fn frame_inner(&self, root: &str, worker: Option<&str>) -> PreviewAnswer {
        let inner = &self.inner;
        let status = self.status();
        let mono = inner.clock.monotonic();
        // the first part, as Python runs it with no await
        let (key, w, surface, line, kind, job_id, mut rx, phase) = {
            let mut st = lock(&inner.state);
            let doing = status(root, worker.unwrap_or(""));
            if let Some((s, l)) = &doing {
                if s == "done" {
                    return PreviewAnswer::new(json!({"state": "done", "line": l}));
                }
            }
            let Some(workers) = st.workers(root).filter(|w| !w.is_empty()) else {
                return PreviewAnswer::new(
                    json!({"state": "none", "line": "No window to show yet"}),
                );
            };
            let (key, w) = match worker.and_then(|k| workers.iter().find(|(x, _)| x == k)) {
                Some((k, w)) => (k.clone(), w.clone()),
                None => workers
                    .iter()
                    .rev()
                    .find(|(_, w)| {
                        let w = lock(w);
                        w.current.is_some() && w.surface.is_some()
                    })
                    .or_else(|| workers.last())
                    .map(|(k, w)| (k.clone(), w.clone()))
                    .expect("a worker"),
            };
            if let Some((s, l)) = status(root, &key) {
                // asking, waiting, paused, or this step isn't running: frozen
                return self.answer(&mut st, &key, &w, &s, &l, false, false, false);
            }
            let (current, label, surface) = {
                let wl = lock(&w);
                (wl.current.clone(), wl.label.clone(), wl.surface.clone())
            };
            let (Some(current), Some(surface)) = (current, surface) else {
                return self.answer(
                    &mut st,
                    &key,
                    &w,
                    "working_elsewhere",
                    ELSEWHERE,
                    false,
                    false,
                    false,
                );
            };
            let line = format!("Working in {label}");
            let (kind, scope, resolvable) = {
                let s = lock(&surface);
                (s.kind(), s.target.scope.clone(), s.target.resolve.is_some())
            };
            st.counts.one(format!("{kind}_requests"));
            if scope != current || !resolvable {
                if scope == current {
                    st.counts.one(format!("{kind}_no_picture"));
                }
                let state = if scope != current {
                    "working_elsewhere"
                } else {
                    "working"
                };
                return self.answer(&mut st, &key, &w, state, &line, false, false, false);
            }
            {
                let mut s = lock(&surface);
                if s.asked.is_none() {
                    s.asked = Some(mono);
                }
            }
            if st.jobs.get(root).is_some_and(Job::done) {
                st.jobs.remove(root);
            }
            let mut turned = st.turned.get(root).map(|(s, at)| (s.clone(), *at));
            if turned.as_ref().is_some_and(|(_, at)| mono - at > TURN_S) {
                st.turned.remove(root); // its row stopped asking: nobody is waiting for a turn
                turned = None;
            }
            let no_job = !st.jobs.contains_key(root);
            if no_job
                && turned
                    .as_ref()
                    .is_none_or(|(s, _)| Arc::ptr_eq(s, &surface))
            {
                // DMN-584: one turned away goes first
                st.turned.remove(root);
                st.next_job += 1;
                let id = st.next_job;
                let (tx, _) = watch::channel(None);
                let tx = Arc::new(tx);
                let phase = Arc::new(Mutex::new("start".to_owned()));
                let task = tokio::spawn(Self::capture(
                    inner.clone(),
                    root.to_owned(),
                    surface.clone(),
                    phase.clone(),
                    tx.clone(),
                ));
                st.jobs.insert(
                    root.to_owned(),
                    Job {
                        id,
                        surface: surface.clone(),
                        abort: task.abort_handle(),
                        tx,
                        waiters: 0,
                        phase,
                    },
                );
            }
            let ours = st
                .jobs
                .get(root)
                .is_some_and(|j| Arc::ptr_eq(&j.surface, &surface));
            if !ours {
                // one capture at a time per task: another step's is running
                if st.jobs.contains_key(root) && turned.is_none() {
                    st.turned.insert(root.to_owned(), (surface.clone(), mono));
                }
                st.counts.busy += 1;
                st.counts.one(format!("{kind}_busy"));
                return self.answer(&mut st, &key, &w, "working", &line, false, true, false);
            }
            let job = st.jobs.get_mut(root).expect("the job");
            job.waiters += 1;
            (
                key,
                w,
                surface,
                line,
                kind,
                job.id,
                job.tx.subscribe(),
                job.phase.clone(),
            )
        };
        let mut waiting = Waiting {
            inner: inner.clone(),
            root: root.to_owned(),
            job: job_id,
            armed: true,
        };
        let timeout = inner.clock.sleep(inner.limit_s);
        let finished = tokio::select! {
            _ = rx.wait_for(Option::is_some) => true,
            _ = timeout => false,
        };
        let slow = !finished;
        let mut notes = vec![];
        if slow {
            let mut st = lock(&inner.state);
            st.counts.one(format!("{kind}_slow"));
            let phase = lock(&phase).clone();
            notes.extend(self.journal(
                &mut st,
                "preview.slow",
                kind,
                json!({"phase": phase, "seconds": inner.limit_s}),
            ));
        }
        waiting.leave();
        self.tell(notes);
        let ended = *rx.borrow();
        let mut st = lock(&inner.state);
        if !st.has_root(root) {
            // the task ended during the capture: its pixels are discarded
            return PreviewAnswer::new(json!({"state": "done", "line": "Done"}));
        }
        let ended = match ended {
            Some(Ended::Cancelled) | None => Ended::Failed,
            Some(e) => e,
        };
        let _ = surface;
        self.answer(
            &mut st,
            &key,
            &w,
            "working",
            &line,
            ended == Ended::Fresh,
            ended == Ended::Busy,
            slow,
        )
    }

    fn tell(&self, notes: Vec<(String, Value)>) {
        for (kind, data) in notes {
            (self.inner.note)(&kind, data);
        }
    }

    /// One capture of a surface, under its locks only if they are free (never queued behind the worker), then masked
    /// again by text recognition. A failure keeps the last frame and says why. The capture's own waits get only the
    /// time the preview has left (`PREVIEW`'s deadline), and each phase's time is counted per surface kind.
    async fn capture(
        inner: Arc<Inner>,
        root: String,
        surface: Sf,
        phase: Arc<Mutex<String>>,
        tx: Arc<watch::Sender<Option<Ended>>>,
    ) {
        let started = inner.clock.monotonic();
        let ctx = Arc::new(PreviewCtx {
            deadline: started + inner.limit_s,
            phases: Mutex::new(BTreeMap::new()),
        });
        let ended = PREVIEW
            .scope(
                ctx.clone(),
                Self::capture_in(inner.clone(), root, surface, phase, ctx, started),
            )
            .await;
        tx.send_if_modified(|v| {
            if v.is_none() {
                *v = Some(ended);
                true
            } else {
                false
            }
        });
    }

    async fn capture_in(
        inner: Arc<Inner>,
        root: String,
        surface: Sf,
        phase: Arc<Mutex<String>>,
        ctx: Arc<PreviewCtx>,
        started: f64,
    ) -> Ended {
        let (kind, locks, capture, resolve) = {
            let s = lock(&surface);
            (
                s.kind(),
                s.locks.clone(),
                s.capture.clone(),
                s.target.resolve.clone(),
            )
        };
        let tally = PhaseTally {
            inner: inner.clone(),
            ctx,
            kind,
        };
        let clock = inner.clock.clone();
        let now = move || clock.monotonic();
        let got: Result<(Vec<u8>, String), Fail> = async {
            let held = (inner.try_hold)(&locks, &format!("preview:{root}"))
                .map_err(|class| Fail::Failed(class.clone(), class))?;
            let Some(held) = held else {
                return Err(Fail::Busy);
            };
            let capture = match capture {
                Some(c) => c,
                None => {
                    let Some(resolve) = resolve else {
                        let class = "AssertionError".to_owned();
                        return Err(Fail::Failed(class.clone(), class));
                    };
                    *lock(&phase) = "resolve".into();
                    let (_, c) = preview_phase("resolve", &now, resolve()).await?;
                    lock(&surface).capture = Some(c.clone());
                    c
                }
            };
            *lock(&phase) = "capture".into();
            let raw = capture().await?;
            drop(held);
            *lock(&phase) = "ocr".into();
            let frame = preview_phase("ocr", &now, (inner.sanitize)(raw.0))
                .await
                .map_err(|class| Fail::Failed(class.clone(), class))?;
            Ok((frame, raw.1))
        }
        .await;
        drop(tally);
        let mut st = lock(&inner.state);
        let (frame, note) = match got {
            Ok(x) => x,
            Err(fail) => {
                let mut s = lock(&surface);
                return match fail {
                    // the worker's own screenshot is capturing (DMN-406), or the worker holds the surface
                    Fail::Busy => {
                        st.counts.busy += 1;
                        st.counts.one(format!("{kind}_busy"));
                        Ended::Busy
                    }
                    Fail::Locked => {
                        s.problem = Some(("screen_locked".into(), LOCKED_TEXT.into()));
                        st.counts.one(format!("{kind}_locked"));
                        Ended::Failed
                    }
                    Fail::Withheld(t) => {
                        s.problem = Some(("withheld".into(), t));
                        st.counts.one(format!("{kind}_withheld"));
                        Ended::Failed
                    }
                    Fail::Gone(t) => {
                        s.capture = None;
                        s.problem = Some(("gone".into(), t));
                        st.counts.one(format!("{kind}_gone"));
                        Ended::Failed
                    }
                    Fail::Failed(class, name) => {
                        s.capture = None;
                        s.problem = Some(if (inner.allowed)() {
                            ("failed".into(), class)
                        } else {
                            (
                                "not_allowed".into(),
                                "Screen Recording isn't allowed for FERB".into(),
                            )
                        });
                        st.counts.one(format!("{kind}_failed"));
                        drop(s);
                        let previews = TaskPreviews {
                            inner: inner.clone(),
                        };
                        let notes = previews.journal(
                            &mut st,
                            "preview.failed",
                            kind,
                            json!({"error": name}),
                        );
                        drop(st);
                        previews.tell(notes);
                        Ended::Failed
                    }
                };
            }
        };
        if !st.has_root(&root) {
            return Ended::Failed;
        }
        let now = inner.clock.monotonic();
        st.seq += 1;
        let seq = st.seq;
        let mut s = lock(&surface);
        s.frame = Some(frame);
        s.note = note;
        s.problem = None;
        s.seq = seq;
        s.at = now;
        st.counts.frames += 1;
        st.counts.capture_ms += (now - started) * 1000.0;
        st.counts.one(format!("{kind}_frames"));
        st.counts.add(
            format!("{kind}_capture_ms"),
            Num::F((now - started) * 1000.0),
        );
        if let Some(asked) = s.asked.take() {
            st.counts
                .peak(format!("max_{kind}_gap_ms"), Num::F((now - asked) * 1000.0));
        }
        Ended::Fresh
    }

    #[allow(clippy::too_many_arguments)]
    fn answer(
        &self,
        st: &mut State,
        key: &str,
        w: &Wk,
        state: &str,
        line: &str,
        fresh: bool,
        busy: bool,
        slow: bool,
    ) -> PreviewAnswer {
        let surface = lock(w).surface.clone();
        if !fresh {
            st.counts.cached += 1;
        }
        let mut out = PreviewAnswer::new(
            json!({"state": state, "line": line, "worker": key, "step": step_of(key)}),
        );
        let s = surface.as_ref().map(|s| lock(s));
        if let Some(s) = &s {
            if let Some(frame) = &s.frame {
                let age = (((self.inner.clock.monotonic() - s.at) * 1000.0) as i64).max(0);
                out.image = Some(frame.clone());
                out.fields.insert("fresh".into(), json!(fresh));
                out.fields.insert("seq".into(), json!(s.seq));
                out.fields.insert("age_ms".into(), json!(age));
                if state == "working" {
                    st.counts
                        .peak(format!("max_{}_age_ms", s.kind()), Num::I(age));
                }
            }
            if !s.note.is_empty() {
                out.fields.insert("masking".into(), json!(s.note));
            }
        }
        // a problem the surface has stays until its next frame; "too long" belongs to the answer that ran out only
        let problem = if slow {
            Some(("slow".to_owned(), SLOW_TEXT.to_owned()))
        } else {
            s.as_ref().and_then(|s| s.problem.clone())
        };
        if let Some((kind, text)) = problem {
            if !fresh {
                out.fields
                    .insert("problem".into(), json!({"kind": kind, "text": text}));
            }
        }
        if busy {
            out.fields.insert("busy".into(), json!(true));
        }
        out
    }

    /// `_journal`: the first of its kind per surface kind at once, then at most one row per NOTE_EVERY_S with how
    /// many there were since. Returns what to note (told after the state's lock is released).
    fn journal(
        &self,
        st: &mut State,
        kind: &str,
        surface_kind: &str,
        data: Value,
    ) -> Vec<(String, Value)> {
        let key = format!("{kind}:{surface_kind}");
        let now = self.inner.clock.monotonic();
        let (due, since) = st.noted.get(&key).copied().unwrap_or((0.0, 0));
        if now < due {
            st.noted.insert(key, (due, since + 1));
            return vec![];
        }
        st.noted.insert(key, (now + NOTE_EVERY_S, 0));
        let mut out = Map::new();
        out.insert("surface".into(), json!(surface_kind));
        if let Value::Object(d) = data {
            out.extend(d);
        }
        if since != 0 {
            out.insert("repeats".into(), json!(since));
        }
        vec![(kind.to_owned(), Value::Object(out))]
    }

    /// The counters, cumulative, and the peaks since the last read (taken as they are, then started afresh).
    pub fn snapshot(&self) -> Map<String, Value> {
        let mut st = lock(&self.inner.state);
        let peaks = std::mem::take(&mut st.counts.peaks);
        let c = &st.counts;
        let mut out = Map::new();
        out.insert("requests".into(), json!(c.requests));
        out.insert("frames".into(), json!(c.frames));
        out.insert("capture_ms".into(), Num::F(c.capture_ms).rounded());
        out.insert("cached".into(), json!(c.cached));
        out.insert("busy".into(), json!(c.busy));
        for (k, v) in &c.kinds {
            out.insert(k.clone(), v.rounded());
        }
        for (k, v) in peaks {
            out.insert(k, v.rounded());
        }
        out
    }
}
