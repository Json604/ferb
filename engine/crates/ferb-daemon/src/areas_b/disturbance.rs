//! Observe desktop changes FERB could have caused; never activate, restore, block or infer authorization. Port of
//! `ferb/daemon/disturbance.py`.
//!
//! DMN-5 (owner 2026-10-30): the desktop is sampled only right before and right after a tool call that can change the
//! owner's screen (its capability declares `on_screen`), never around read-only tools and never on a timer, plus one
//! baseline when a task starts. Only what FERB could have changed is compared and recorded: the front app, the
//! focused window, fullscreen and the Space ([`FERB_KEYS`]). What changed during one call is returned as that call's
//! facts; a change found with no FERB on-screen call in flight is recorded as that fact, with when the last one ended
//! and the processes FERB started that were running (DMN-501). Who did it stays the reader's conclusion. Sound is part
//! of background-first (HAND-207, HAND-213, HAND-230): a tab that isn't FERB's changing its sound near one of FERB's
//! acting calls is a `tab_sound` change, unless the owner's own input was near it. While the owner has paused sensing,
//! nothing about the desktop is recorded.
//!
//! Seams ([`Seams`]): the desktop read ([`DesktopRead`]: the macOS sampler is a platform part, `ferb-platform-macos`;
//! it needs the NSWorkspace Space notification and a pumped run loop, which no ported crate has yet), the record
//! (the daemon's `_record_disturbance`), the owner-input facts, FERB's running processes, the wall clock, the local
//! time zone, the pause and the monotonic clock.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use ferb_core::when::DateTime;
use ferb_py::value::{py_eq, py_str, truthy};
use ferb_trace::lines::{self, CameForward};
use serde_json::{json, Map, Value};

/// The newest plain statements an agent's context carries; every change stays in the records.
pub const NO_ON_SCREEN_KEPT: usize = 10;
/// HAND-207, HAND-230: how long after one of FERB's acting calls a sound change in another tab counts as near it.
pub const SOUND_NEAR_S: f64 = 5.0;
/// HAND-213: a sound change within this long of the owner's own keyboard or mouse input is theirs.
pub const OWNER_NEAR_S: f64 = 3.0;
/// What FERB could change on the owner's screen, and all that is compared (DMN-5): never the cursor.
pub const FERB_KEYS: [&str; 4] = [
    "frontmost_app",
    "key_window",
    "fullscreen",
    "active_space_epoch",
];
pub const PAUSED: &str =
    "sensing paused by the owner (Pause FERB): nothing about the desktop is read or recorded until Resume";
const COVERAGE: &str =
    "sampled right before and after FERB's calls that can change the screen; front app, focused \
                        window, fullscreen and Space only; Space transitions use notifications";

/// The desktop read (`DesktopProbe`): public macOS desktop facts, `unavailable` naming what couldn't be read. Its
/// error is written as Python writes it, `"<ExceptionType>: <text>"`. `start`/`close` bracket the time any task is
/// observed; `pump` runs before each read (the Space notification needs a serviced run loop).
pub trait DesktopRead: Send + Sync + 'static {
    fn start(&self) {}
    fn pump(&self) {}
    fn close(&self) {}
    fn read(&self) -> Result<Map<String, Value>, String>;
}

pub type Record = Arc<dyn Fn(&str, &str, Value) + Send + Sync>;
/// A fact reader that can fail; the error as `"<ExceptionType>: <text>"`.
pub type Facts<T> = Arc<dyn Fn() -> Result<T, String> + Send + Sync>;

/// Everything the sentinel reads or tells, from outside.
#[derive(Clone)]
pub struct Seams {
    pub read: Arc<dyn DesktopRead>,
    /// `record(task_root, kind, data)`: the daemon journals it.
    pub record: Record,
    /// The owner-input tap's facts: `monitored`, `recent`, `last_event_wall` (seconds since the epoch).
    pub owner_input: Facts<Map<String, Value>>,
    /// The processes FERB started that are running now (`id`, `task`, `command`, ...).
    pub processes: Facts<Vec<Value>>,
    /// The daemon's clock.
    pub now: Arc<dyn Fn() -> DateTime + Send + Sync>,
    /// `datetime.astimezone()`: the Mac's local time of an instant (the statements' `%H:%M:%S`).
    pub local: Arc<dyn Fn(&DateTime) -> DateTime + Send + Sync>,
    /// Whether the owner has paused sensing.
    pub paused: Arc<dyn Fn() -> bool + Send + Sync>,
    /// `time.monotonic()`.
    pub monotonic: Arc<dyn Fn() -> f64 + Send + Sync>,
}

impl Seams {
    /// The Python defaults: no owner-input facts, no processes, the system clock and zone, never paused.
    pub fn new(read: Arc<dyn DesktopRead>, record: Record) -> Seams {
        let start = std::time::Instant::now();
        Seams {
            read,
            record,
            owner_input: Arc::new(|| Ok(Map::new())),
            processes: Arc::new(|| Ok(vec![])),
            now: Arc::new(|| chrono::Local::now().fixed_offset()),
            local: Arc::new(local_time),
            paused: Arc::new(|| false),
            monotonic: Arc::new(move || start.elapsed().as_secs_f64()),
        }
    }
}

/// The Mac's local time of an instant (`astimezone()`).
pub fn local_time(dt: &DateTime) -> DateTime {
    dt.with_timezone(&chrono::Local).fixed_offset()
}

/// `screen_changes(before, after)`: what FERB could have changed between two samples (FERB_KEYS), `{key: {before,
/// after}}`; a key unreadable in either sample is left out (unknown is not changed).
pub fn screen_changes(before: Option<&Value>, after: Option<&Value>) -> Map<String, Value> {
    let mut out = Map::new();
    let (Some(b), Some(a)) = (before, after) else {
        return out;
    };
    if !truthy(b) || !truthy(a) {
        return out;
    }
    let unread = |v: &Value| -> Vec<String> {
        match v.get("unavailable") {
            Some(Value::Object(m)) => m.keys().cloned().collect(),
            Some(Value::Array(a)) => a.iter().map(py_str).collect(),
            _ => vec![],
        }
    };
    let mut skip = unread(b);
    skip.extend(unread(a));
    for k in FERB_KEYS {
        if skip.iter().any(|s| s == k) {
            continue;
        }
        let (x, y) = (field(b, k), field(a, k));
        if !py_eq(&x, &y) {
            out.insert(k.into(), json!({"before": x, "after": y}));
        }
    }
    out
}

fn field(v: &Value, k: &str) -> Value {
    v.get(k).cloned().unwrap_or(Value::Null)
}

/// `told(changed)` (HAND-212): what changed on the owner's screen during one call, briefly, for the worker's own
/// tool result; `{}` when nothing did. Facts only.
pub fn told(changed: &Map<String, Value>) -> Map<String, Value> {
    fn name(app: &Value) -> Value {
        match app {
            Value::Object(a) => match a.get("name").filter(|v| truthy(v)) {
                Some(n) => n.clone(),
                None => a.get("bundle_id").cloned().unwrap_or(Value::Null),
            },
            other => other.clone(),
        }
    }
    let mut out = Map::new();
    match changed.get("frontmost_app").filter(|v| truthy(v)) {
        Some(front) => {
            let side = |k: &str| name(front.get(k).unwrap_or(&Value::Null));
            out.insert(
                "front_app".into(),
                json!({"before": side("before"), "after": side("after")}),
            );
        }
        None if changed.contains_key("key_window") => {
            out.insert("focused_window_changed".into(), Value::Bool(true));
        }
        None => {}
    }
    if let Some(f) = changed.get("fullscreen") {
        out.insert("fullscreen".into(), field(f, "after"));
    }
    if changed.contains_key("active_space_epoch") {
        out.insert("space_changed".into(), Value::Bool(true));
    }
    if !out.is_empty() {
        out.insert("put_back".into(), Value::Bool(false));
    }
    out
}

/// `_elapsed(s)`: seconds since an on-screen call ended: tenths under 10 s (rounded down), whole seconds from 10 s.
pub fn elapsed(s: f64) -> f64 {
    let s = s.max(0.0);
    if s >= 10.0 {
        s.trunc()
    } else {
        (s * 10.0).floor() / 10.0
    }
}

/// `_for_tree`: FERB-started processes as one task tree's record keeps them: another tree's lose their command text.
fn for_tree(processes: &Option<Vec<Value>>, tree: &str) -> Value {
    let Some(list) = processes else {
        return Value::Null;
    };
    Value::Array(
        list.iter()
            .map(|p| {
                let task = p.get("task").map(py_str).unwrap_or_default();
                if task.split('.').next().unwrap_or("") == tree {
                    p.clone()
                } else {
                    match p {
                        Value::Object(m) => Value::Object(
                            m.iter()
                                .filter(|(k, _)| k.as_str() != "command")
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect(),
                        ),
                        other => other.clone(),
                    }
                }
            })
            .collect(),
    )
}

fn root_of(task: &str) -> &str {
    task.split('.').next().unwrap_or("")
}

/// One task's observation (`_Observation`).
#[derive(Default)]
struct Observation {
    last: Option<Map<String, Value>>,
    samples: i64,
    changes: i64,
    errors: i64,
    unavailable: Map<String, Value>,
    /// None: Python's int 0 (never raised by a sample).
    max_sample_ms: Option<f64>,
    on_screen_calls: i64,
    screen_changes: Vec<Value>,
    no_on_screen_action: VecDeque<String>,
    no_on_screen_action_count: i64,
    forward: CameForward,
    paused: bool,
}

impl Observation {
    fn max_sample(&self) -> Value {
        self.max_sample_ms.map_or(json!(0), |v| json!(v))
    }
}

struct Action {
    task_id: String,
    span_id: String,
    tool: String,
    foreground_argument: Value,
    started_monotonic: f64,
    before: Option<Map<String, Value>>,
}

impl Action {
    fn public(&self) -> Value {
        json!({"task_id": self.task_id, "span_id": self.span_id, "tool": self.tool, "on_screen": true,
               "foreground_argument": self.foreground_argument, "started_monotonic": self.started_monotonic,
               "state": "in_flight"})
    }
}

#[derive(Default)]
struct State {
    on_screen_ended: Option<DateTime>,
    active: Vec<(String, Observation)>,
    actions: Vec<(String, Action)>,
    acting: Vec<(String, Value)>,
    ended: VecDeque<(f64, Value)>,
}

impl State {
    fn obs(&mut self, root: &str) -> Option<&mut Observation> {
        self.active
            .iter_mut()
            .find(|(k, _)| k == root)
            .map(|(_, o)| o)
    }
}

/// Records made under the state lock, told after it is released, in order.
type Pending = Vec<(String, &'static str, Value)>;

/// One sampler shared by all active tasks (`DisturbanceSentinel`).
pub struct DisturbanceSentinel {
    seams: Seams,
    state: Mutex<State>,
    lock: tokio::sync::Mutex<()>,
}

impl DisturbanceSentinel {
    pub fn new(seams: Seams) -> DisturbanceSentinel {
        DisturbanceSentinel {
            seams,
            state: Mutex::new(State::default()),
            lock: tokio::sync::Mutex::new(()),
        }
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn tell(&self, pending: Pending) {
        for (task, kind, data) in pending {
            (self.seams.record)(&task, kind, data);
        }
    }

    /// The task roots observed now, in the order they started.
    pub fn active(&self) -> Vec<String> {
        self.st().active.iter().map(|(k, _)| k.clone()).collect()
    }

    pub async fn start(&self, task_id: &str) {
        {
            let mut st = self.st();
            if st.active.iter().any(|(k, _)| k == task_id) {
                return;
            }
            let first = st.active.is_empty();
            // registered and the probe started with no await between, so a finishing tree can't close the probe
            // under a starting one
            st.active.push((task_id.to_owned(), Observation::default()));
            if first {
                self.seams.read.start();
            }
        }
        self.sample().await; // the task's baseline
    }

    pub async fn finish(&self, task_id: &str) {
        let mut pending = Pending::new();
        {
            let mut st = self.st();
            let Some(at) = st.active.iter().position(|(k, _)| k == task_id) else {
                return; // finished meanwhile: the tree's own end and shutdown's both ran
            };
            let (_, o) = st.active.remove(at);
            pending.push((
                task_id.to_owned(),
                "disturbance.summary",
                json!({"samples": o.samples, "changes": o.changes, "errors": o.errors,
                       "unavailable": o.unavailable, "max_sample_ms": o.max_sample(),
                       "on_screen_calls": o.on_screen_calls, "screen_changes": o.screen_changes,
                       "coverage": COVERAGE, "permission_assessment": "not_inferred",
                       "attribution": "temporal_correlation_only"}),
            ));
            if st.active.is_empty() {
                self.seams.read.close();
            }
        }
        self.tell(pending);
    }

    pub async fn shutdown(&self) {
        for task_id in self.active() {
            self.finish(&task_id).await;
        }
    }

    /// A call is about to run: only one that can change the screen is sampled.
    pub async fn before_action(
        &self,
        task_id: &str,
        span_id: &str,
        tool: &str,
        args: &Map<String, Value>,
        on_screen: bool,
    ) {
        if !on_screen {
            return;
        }
        let before = self.sample().await;
        let mut st = self.st();
        if let Some(o) = st.obs(root_of(task_id)) {
            o.on_screen_calls += 1;
        }
        let action = Action {
            task_id: task_id.to_owned(),
            span_id: span_id.to_owned(),
            tool: tool.to_owned(),
            foreground_argument: args.get("allow_foreground").cloned().unwrap_or(Value::Null),
            started_monotonic: (self.seams.monotonic)(),
            before,
        };
        match st.actions.iter_mut().find(|(k, _)| k == span_id) {
            Some(slot) => slot.1 = action, // a dict keeps an updated key where it was
            None => st.actions.push((span_id.to_owned(), action)),
        }
    }

    /// The call ended: what FERB could have changed during it, as the call's facts (`{}` when nothing, or when it
    /// wasn't sampled). Idempotent per call.
    pub async fn after_action(&self, span_id: &str) -> Map<String, Value> {
        if !self.st().actions.iter().any(|(k, _)| k == span_id) {
            return Map::new();
        }
        let after = self.sample().await;
        let mut st = self.st();
        let action = st
            .actions
            .iter()
            .position(|(k, _)| k == span_id)
            .map(|at| st.actions.remove(at).1);
        st.on_screen_ended = Some((self.seams.now)());
        let Some(action) = action else {
            return Map::new();
        };
        let before = action.before.map(Value::Object);
        let after = after.map(Value::Object);
        let changed = screen_changes(before.as_ref(), after.as_ref());
        if !changed.is_empty() {
            if let Some(o) = st.obs(root_of(&action.task_id)) {
                o.screen_changes
                    .push(json!({"tool": action.tool, "span_id": span_id,
                                             "changes": changed}));
            }
        }
        changed
    }

    /// One of FERB's acting calls (not an observation) starts (HAND-207). Nothing is read.
    pub fn call_started(&self, task_id: &str, span_id: &str, tool: &str) {
        let call = json!({"task_id": task_id, "span_id": span_id, "tool": tool});
        let mut st = self.st();
        match st.acting.iter_mut().find(|(k, _)| k == span_id) {
            Some(slot) => slot.1 = call,
            None => st.acting.push((span_id.to_owned(), call)),
        }
    }

    pub fn call_ended(&self, span_id: &str) {
        let mut st = self.st();
        if let Some(at) = st.acting.iter().position(|(k, _)| k == span_id) {
            let (_, call) = st.acting.remove(at);
            if st.ended.len() == 64 {
                st.ended.pop_front();
            }
            st.ended.push_back(((self.seams.monotonic)(), call));
        }
    }

    /// A tab that isn't FERB's changed its sound (`change`: `{audible|muted: {before, after}}`): a `tab_sound`
    /// change for each active task with an acting call in flight or just ended; nothing when no FERB call is near,
    /// the owner's own input was (HAND-213), or sensing is paused.
    pub fn sound_changed(&self, tab: &Value, change: &Map<String, Value>) {
        if self.st().active.is_empty()
            || change.is_empty()
            || (self.seams.paused)()
            || self.owner_input_near()
        {
            return;
        }
        // SENSE-406: with the input tap down the owner's input can't be told apart; the record says so
        let monitored = self.owner_input_monitored();
        let now = (self.seams.monotonic)();
        let mut pending = Pending::new();
        {
            let mut st = self.st();
            let mut near: Vec<Value> = st.acting.iter().map(|(_, c)| c.clone()).collect();
            near.extend(
                st.ended
                    .iter()
                    .filter(|(at, _)| now - at <= SOUND_NEAR_S)
                    .map(|(_, c)| c.clone()),
            );
            let mut by_tree: Vec<(String, Vec<Value>)> = vec![];
            for call in near {
                let tree = root_of(&py_str(&field(&call, "task_id"))).to_owned();
                match by_tree.iter_mut().find(|(t, _)| *t == tree) {
                    Some((_, calls)) => calls.push(call),
                    None => by_tree.push((tree, vec![call])),
                }
            }
            for (tree, calls) in by_tree {
                let Some(o) = st.obs(&tree) else { continue };
                if o.paused {
                    continue;
                }
                let mut sound = Map::new();
                sound.insert("tab".into(), tab.clone());
                for (k, v) in change {
                    sound.insert(k.clone(), v.clone());
                }
                let changes = json!({"tab_sound": sound});
                o.changes += 1;
                let last = calls.last().cloned().unwrap_or(Value::Null);
                o.screen_changes.push(json!({"tool": field(&last, "tool"),
                                             "span_id": field(&last, "span_id"),
                                             "changes": changes}));
                let mut data = json!({"changes": changes, "actions": calls,
                                      "permission_assessment": "not_inferred",
                                      "attribution": "temporal_correlation_only"});
                if !monitored {
                    data["owner_input_monitored"] = Value::Bool(false);
                }
                pending.push((tree, "disturbance.change", data));
            }
        }
        self.tell(pending);
    }

    /// Whether the owner-input tap is running (a reader that doesn't say counts as running).
    fn owner_input_monitored(&self) -> bool {
        match (self.seams.owner_input)() {
            Ok(o) => o.get("monitored").is_none_or(truthy),
            Err(_) => false,
        }
    }

    /// The owner's own input is under way or ended within OWNER_NEAR_S; false when the tap can't say.
    fn owner_input_near(&self) -> bool {
        let Ok(o) = (self.seams.owner_input)() else {
            return false;
        };
        if o.get("recent").is_some_and(truthy) {
            return true;
        }
        let last = match o.get("last_event_wall") {
            None | Some(Value::Null) => return false,
            Some(v) => match py_float(v) {
                Some(f) => f,
                None => return false, // float() raised
            },
        };
        timestamp(&(self.seams.now)()) - last <= OWNER_NEAR_S
    }

    /// What FERB's calls in this task changed on the screen so far (for the merged checker).
    pub fn facts(&self, task_id: &str) -> Vec<Value> {
        self.st()
            .obs(root_of(task_id))
            .map(|o| o.screen_changes.clone())
            .unwrap_or_default()
    }

    /// One read, compared for every active task; returns it (None when paused, failed or no task is active).
    pub async fn sample(&self) -> Option<Map<String, Value>> {
        if self.st().active.is_empty() {
            return None;
        }
        let _one = self.lock.lock().await;
        let mut pending = Pending::new();
        let out = self.sample_locked(&mut pending).await;
        self.tell(pending);
        out
    }

    async fn sample_locked(&self, pending: &mut Pending) -> Option<Map<String, Value>> {
        if self.held(pending) {
            return None;
        }
        let mono = &self.seams.monotonic;
        let started = mono();
        let read = self.seams.read.clone();
        let got = async {
            read.pump();
            let r = read.clone();
            let snapshot = tokio::task::spawn_blocking(move || r.read())
                .await
                .map_err(|e| format!("JoinError: {e}"))??;
            let sample_ms = (mono() - started) * 1000.0;
            let owner = (self.seams.owner_input)()?;
            Ok::<_, String>((snapshot, sample_ms, owner))
        }
        .await;
        let (raw, sample_ms, owner) = match got {
            Ok(x) => x,
            Err(error) => {
                if self.held(pending) {
                    return None; // paused while reading: the sample never happened
                }
                let mut st = self.st();
                for (task_id, o) in st.active.iter_mut() {
                    o.errors += 1;
                    pending.push((
                        task_id.clone(),
                        "disturbance.error",
                        json!({"error": error}),
                    ));
                }
                return None;
            }
        };
        if self.held(pending) {
            return None; // paused while reading: drop what was read
        }
        let mut snapshot: Map<String, Value> = raw
            .iter()
            .filter(|(k, _)| FERB_KEYS.contains(&k.as_str()) || k.as_str() == "unavailable")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let unavailable: Map<String, Value> = match snapshot.get("unavailable") {
            Some(Value::Object(m)) => m
                .iter()
                .filter(|(k, _)| FERB_KEYS.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            _ => Map::new(),
        };
        snapshot.insert("unavailable".into(), Value::Object(unavailable.clone()));
        let snapshot_v = Value::Object(snapshot.clone());
        let mut st = self.st();
        let actions: Vec<Value> = st.actions.iter().map(|(_, a)| a.public()).collect();
        let near = !actions.is_empty(); // every tracked action is an on-screen one, of any task
        let mut procs: Option<Vec<Value>> = None;
        let mut listed = false;
        let on_screen_ended = st.on_screen_ended;
        for i in 0..st.active.len() {
            let task_id = st.active[i].0.clone();
            let o = &mut st.active[i].1;
            o.samples += 1;
            if o.max_sample_ms.map_or(sample_ms > 0.0, |m| sample_ms > m) {
                o.max_sample_ms = Some(sample_ms);
            }
            for (k, v) in &unavailable {
                o.unavailable.insert(k.clone(), v.clone());
            }
            match &o.last {
                None => {
                    o.forward.baseline(&snapshot_v);
                    pending.push((
                        task_id.clone(),
                        "disturbance.baseline",
                        json!({"snapshot": snapshot_v, "space_identity": "transition_epoch_only"}),
                    ));
                }
                Some(last) => {
                    let mut changes = Map::new();
                    for key in FERB_KEYS {
                        let (b, a) = (
                            last.get(key).cloned().unwrap_or(Value::Null),
                            snapshot.get(key).cloned().unwrap_or(Value::Null),
                        );
                        if !py_eq(&b, &a) {
                            changes.insert(key.into(), json!({"before": b, "after": a}));
                        }
                    }
                    if !changes.is_empty() {
                        o.changes += 1;
                        let mut data = json!({
                            "changes": changes, "actions": actions, "owner_input": owner,
                            "sample_duration_ms": sample_ms, "permission_assessment": "not_inferred",
                            "attribution": "temporal_correlation_only"});
                        if changes.contains_key("frontmost_app")
                            && lines::front_pid(&field(&snapshot_v, "frontmost_app")).is_none()
                        {
                            data["front_app_unreadable"] = Value::Bool(true);
                        }
                        let what = o.forward.change(&Value::Object(changes));
                        if !what.is_empty() {
                            if !listed {
                                procs = (self.seams.processes)().ok();
                                listed = true;
                            }
                            data["processes"] = for_tree(&procs, &task_id);
                            if !near {
                                self.no_on_screen_action(
                                    &task_id,
                                    o,
                                    &what,
                                    &mut data,
                                    on_screen_ended,
                                );
                            }
                        }
                        pending.push((task_id.clone(), "disturbance.change", data));
                    }
                }
            }
            o.last = Some(snapshot.clone());
        }
        Some(snapshot)
    }

    /// `_held`: while the owner has paused sensing each task's record says so once and nothing is read; the next
    /// sample after resuming starts from a fresh baseline.
    fn held(&self, pending: &mut Pending) -> bool {
        let paused = (self.seams.paused)();
        let mut st = self.st();
        if !paused {
            for (task_id, o) in st.active.iter_mut() {
                if o.paused {
                    o.paused = false;
                    pending.push((task_id.clone(), "disturbance.resumed", json!({})));
                }
            }
            return false;
        }
        for (task_id, o) in st.active.iter_mut() {
            if !o.paused {
                o.paused = true;
                o.last = None;
                o.forward = CameForward::new();
                pending.push((
                    task_id.clone(),
                    "disturbance.paused",
                    json!({"reason": PAUSED}),
                ));
            }
        }
        true
    }

    /// `_no_on_screen_action`: the facts of a change no FERB on-screen call was near, and the plain statement.
    fn no_on_screen_action(
        &self,
        task_id: &str,
        o: &mut Observation,
        what: &str,
        data: &mut Value,
        ended: Option<DateTime>,
    ) {
        let now = (self.seams.now)();
        let in_task = o.on_screen_calls > 0;
        let last = match ended {
            None => Value::Null,
            Some(e) => {
                let gap = (now - e).num_microseconds().unwrap_or(0) as f64 / 1e6;
                json!(elapsed(gap))
            }
        };
        data["on_screen_in_task"] = Value::Bool(in_task);
        data["last_on_screen_ended_s"] = last.clone();
        o.no_on_screen_action_count += 1;
        if o.no_on_screen_action.len() == NO_ON_SCREEN_KEPT {
            o.no_on_screen_action.pop_front();
        }
        let stamp = (self.seams.local)(&now).format("%H:%M:%S").to_string();
        o.no_on_screen_action.push_back(format!(
            "{stamp} {}",
            lines::no_on_screen_action(
                what,
                &field(data, "processes"),
                task_id,
                &Value::Bool(in_task),
                &last
            )
        ));
    }

    /// What an agent's context carries about the desktop ("" when not observed or paused).
    pub fn context(&self, task_id: &str) -> String {
        let mut st = self.st();
        if (self.seams.paused)() {
            return String::new();
        }
        let Some(o) = st.obs(root_of(task_id)) else {
            return String::new();
        };
        let facts = json!({
            "samples": o.samples, "changes": o.changes, "errors": o.errors, "unavailable": o.unavailable,
            "front_or_space_changes_with_no_ferb_on_screen_action": o.no_on_screen_action_count,
            "latest": o.no_on_screen_action.iter().collect::<Vec<_>>(),
            "read_full": "task.evidence query=disturbance on this task root"});
        format!(
            "\nDesktop observations (facts, not inferred permission or causation; FERB's actions that can change \
             the screen are marked on_screen): {}",
            ferb_py::json::dumps_utf8(&facts)
        )
    }
}

/// `float(value)` of a recorded fact: a number, or a text Python's float() reads.
fn py_float(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::String(s) => {
            let t = s
                .trim_matches(|c: char| ferb_py::value::is_py_space(c))
                .replace('_', "");
            match t.to_ascii_lowercase().as_str() {
                "inf" | "+inf" | "infinity" | "+infinity" => Some(f64::INFINITY),
                "-inf" | "-infinity" => Some(f64::NEG_INFINITY),
                "nan" | "+nan" | "-nan" => Some(f64::NAN),
                _ => t.parse().ok().filter(|_| !t.is_empty()),
            }
        }
        _ => None,
    }
}

fn timestamp(dt: &DateTime) -> f64 {
    dt.timestamp() as f64 + f64::from(dt.timestamp_subsec_micros()) / 1e6
}
