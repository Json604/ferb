//! Group A's handler areas: memory (`areas/memory.py`), memory import (`areas/memory_import.py`), schedules, reminders
//! and timers (`areas/schedules.py`), the retention pass (`areas/retention.py`), FERB's records for the planner
//! (`areas/records.py`), FERB.app's chat history (`areas/chat_history.py`), and the settings the app changes
//! (`ferb/daemon/settings.py`).
//!
//! The state these areas use ([`State`]) is built on the daemon's loop thread ([`crate::shared::LoopThread`]) from
//! [`Deps`], and each handler runs there (Python's one event loop). What belongs to other parts comes in through the
//! seams in [`seams`].

pub mod chat_history;
pub mod memory;
pub mod memory_import;
pub mod persona;
pub mod records;
pub mod retention;
pub mod schedules;
pub mod seams;
pub mod settings;

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use ferb_clock::clock::ZonedClock;
use ferb_clock::py::Time;
use ferb_core::config::Config;
use ferb_core::interfaces::IdSource;
use ferb_learning::items::Memory;
use ferb_learning::journal::{Entry, Journal};
use ferb_learning::learning::Learning;
use ferb_learning::profile::CoreProfile;
use ferb_learning::store::Store;
use ferb_rpc::server::{HandlerError, HandlerResult};
use ferb_rpc::wire::Params;
use ferb_rpc::Server;
use ferb_sanitize::redact::Redactor;
use ferb_store::Db;
use serde_json::{json, Map, Value};

use crate::shared::{fault, refusal_as, Local, LoopThread};
pub use seams::{Logins, Owner, Questions, Tasks};

/// A future that stays on the loop thread.
pub type LocalFuture<T> = Pin<Box<dyn Future<Output = T>>>;

/// `make_runtime` for the learners (the import, the profile pass): `ferb_learning::calls::MakeRuntime`, shareable.
pub type MakeRuntime = dyn Fn(&str, &str, &[String], &Value) -> ferb_learning::Result<ferb_learning::calls::Made>
    + Send
    + Sync;

/// `_publish(topic, data)`: the daemon's events (and through them the bus).
pub type Publish = Arc<dyn Fn(&str, Map<String, Value>) + Send + Sync>;

/// What group A's areas are built from. Every field is required (DMN-11): missing wiring fails here, not later.
pub struct Deps {
    /// FERB's home (`~/.ferb`): config, `memory/` export, persona.json.
    pub home: PathBuf,
    /// The owner's home folder (`Path.home()`): where the import reads other assistants' files.
    pub user_home: PathBuf,
    /// The store (opened and migrated on the loop thread).
    pub db_path: PathBuf,
    /// The loop to live on; `None` starts one.
    pub loop_thread: Option<LoopThread>,
    /// FERB's package folder (`ferb/`): the learners' prompts.
    pub prompts: PathBuf,
    pub clock: Arc<dyn ZonedClock>,
    pub ids: Arc<dyn IdSource>,
    pub config: Arc<Config>,
    pub redactor: Arc<Redactor>,
    /// `_publish`: the daemon's events (and through them the bus).
    pub publish: Publish,
    /// A timer's claim token (`secrets.token_hex(8)`; tests: a counter). `None`: random.
    pub timer_tokens: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    /// The learners' model calls; `None`: none can run (the import and the profile pass refuse).
    pub make_runtime: Option<Arc<MakeRuntime>>,
    /// The FERB.app bundle the daemon runs from (`install.app_bundle()`), for the local model's shipped files.
    pub app_bundle: Option<PathBuf>,
    /// What FERB saw on this Mac: the scan (`persona_scan`); `None`: unavailable (`setup.persona`).
    pub persona_scan: Option<Arc<seams::PersonaScan>>,
    pub tasks: Arc<dyn Tasks>,
    pub owner: Arc<dyn Owner>,
    pub questions: Arc<dyn Questions>,
    pub logins: Arc<dyn Logins>,
    /// The retention pass's other stores and the downloads folder; `None`: not built (the pass doesn't run).
    pub purges: Option<Arc<dyn retention::Purges>>,
}

/// One journal append (kind, data, origin), for code that only records.
pub type Record = dyn Fn(&str, Value) -> ferb_learning::Result<()>;

/// Group A's state on the loop thread.
pub struct State {
    pub home: PathBuf,
    pub user_home: PathBuf,
    pub app_bundle: Option<PathBuf>,
    pub store: Rc<Store>,
    pub clock: Arc<dyn ZonedClock>,
    pub ids: Arc<dyn IdSource>,
    pub config: Arc<Config>,
    pub redactor: Arc<Redactor>,
    pub publish: Publish,
    pub timer_tokens: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    pub make_runtime: Option<Arc<MakeRuntime>>,
    pub prompts: Rc<ferb_context::prompts::PromptRoot>,
    pub journal: Rc<Journal>,
    pub memory: Rc<Memory>,
    pub learning: Rc<Learning>,
    pub profile: Rc<CoreProfile>,
    pub tasks: Arc<dyn Tasks>,
    pub owner: Arc<dyn Owner>,
    pub questions: Arc<dyn Questions>,
    pub logins: Arc<dyn Logins>,
    pub purges: Option<Arc<dyn retention::Purges>>,
    pub persona: RefCell<persona::PersonaReview>,
    /// `_shutting_down`.
    pub shutting_down: Cell<bool>,
    /// `state.stopped` (the global stop), as the schedules read it.
    pub stopped: Cell<bool>,
    /// Memory: `_forgetting` (the work forgets left behind), `_owner_followups`.
    pub(crate) forgetting: RefCell<Vec<tokio::task::JoinHandle<()>>>,
    pub(crate) followups: RefCell<std::collections::BTreeMap<String, (String, String)>>,
    pub(crate) login_names: Rc<tokio::sync::Mutex<()>>,
    /// Import: `_import_task`, `_import_cancel`.
    pub(crate) import_task: RefCell<Option<tokio::task::JoinHandle<()>>>,
    pub(crate) import_cancel: RefCell<String>,
    /// Schedules: `_runs`, `_schedule_runs`, `_last_runs`, and the timer loop's wake (`Timers.wake`).
    pub(crate) schedule_runs: RefCell<BTreeSet<String>>,
    pub(crate) last_runs: RefCell<BTreeSet<String>>,
    pub(crate) timer_wake: Rc<tokio::sync::Notify>,
    pub(crate) runs: RefCell<Vec<tokio::task::JoinHandle<()>>>,
    pub(crate) sleepers: RefCell<std::collections::HashMap<String, Rc<tokio::sync::Notify>>>,
    me: RefCell<Weak<State>>,
}

// ---------- adapters between the engines' clocks, ids and zones ----------

struct LearnClock(Arc<dyn ZonedClock>);

impl ferb_learning::Clock for LearnClock {
    fn now(&self) -> ferb_learning::DateTime {
        to_py(&self.0.now())
    }
}

/// A chrono time as ferb-py's (`datetime`).
pub fn to_py(t: &Time) -> ferb_learning::DateTime {
    ferb_learning::DateTime::from_isoformat(&ferb_clock::py::iso(t)).expect("an ISO time")
}

/// ferb-py's time as chrono's.
pub fn from_py(t: &ferb_learning::DateTime) -> Time {
    ferb_clock::py::parse(&t.isoformat(ferb_py::time::Timespec::Auto)).expect("an ISO time")
}

pub(crate) struct LearnIds(Arc<dyn IdSource>);

impl ferb_learning::Ids for LearnIds {
    fn new_id(&self) -> String {
        self.0.new_id()
    }
}

struct ZoneOf(ferb_clock::zone::Zone);

impl ferb_memory::times::LocalZone for ZoneOf {
    fn offset_at(&self, unix_seconds: i64) -> i32 {
        let utc = chrono::DateTime::from_timestamp(unix_seconds, 0)
            .unwrap_or_default()
            .fixed_offset();
        self.0.local(&utc).offset().local_minus_utc()
    }
}

struct LearningSeams;

impl ferb_learning::learning::Seams for LearningSeams {
    fn episode(&self, store: &Store, task_id: &str) -> ferb_learning::Result<Option<Value>> {
        ferb_orchestrator::episodes::get(&store.db, task_id)
            .map_err(|e| ferb_learning::Error::Sql("OperationalError".into(), e.to_string()))
    }

    fn trail_sources(&self, source: &str) -> Vec<(String, String)> {
        ferb_sense::tools::trail_sources(&[source])
    }
}

struct SummarySeams;

impl ferb_learning::summary::Seams for SummarySeams {
    fn test_task(&self, store: &Store, task_id: &str) -> ferb_learning::Result<bool> {
        ferb_orchestrator::test_runs::TestRuns::new(&store.db)
            .task(Some(task_id))
            .map_err(|e| ferb_learning::Error::Sql("OperationalError".into(), e.to_string()))
    }

    fn security_digest(&self, store: &Store, day: &str) -> ferb_learning::Result<Value> {
        ferb_policy::stores::security_digest(&store.db, day).map_err(ferb_learning::Error::from)
    }
}

/// A learning error as the handler answers it: FERB's refusals (`MemoryRefused`, `LearningRefused`, the import's,
/// another part's `FerbError`) with their words; anything else as an internal error (its class only).
pub fn learning_error(e: ferb_learning::Error) -> HandlerError {
    use ferb_learning::Error as E;
    match &e {
        E::MemoryRefused(m) | E::LearningRefused(m) | E::ImportUnreadable(m) | E::ImportBusy(m) => {
            refusal_as(e.class(), m.clone())
        }
        E::Ferb(f) => refusal_as(f.class(), f.to_string()),
        _ => fault(e.class(), e.to_string()),
    }
}

/// A store error as the handler answers it.
pub fn store_error(e: impl std::fmt::Display) -> HandlerError {
    fault("OperationalError", e.to_string())
}

impl State {
    /// `_build_stores` and the memory half of `_build_memory` (compose.py:65-80, memory.py:47-85), on the loop thread.
    pub fn build(deps: Deps) -> Result<Rc<State>, String> {
        let db = Db::open(&deps.db_path).map_err(|e| e.to_string())?;
        db.migrate().map_err(|e| e.to_string())?;
        let store = Store::new(Rc::new(db));
        let clock: Rc<dyn ferb_learning::Clock> = Rc::new(LearnClock(deps.clock.clone()));
        let ids: Rc<dyn ferb_learning::Ids> = Rc::new(LearnIds(deps.ids.clone()));
        let zone: Rc<dyn ferb_memory::times::LocalZone> = Rc::new(ZoneOf(deps.clock.zone()));
        let redactor = deps.redactor.clone();
        let walk = {
            let r = redactor.clone();
            Box::new(move |v: &Value| r.value(v)) as Box<dyn Fn(&Value) -> Value>
        };
        let journal = Rc::new(Journal::new(
            store.clone(),
            clock.clone(),
            ids.clone(),
            walk,
        ));
        let publish_fn = |p: Publish| {
            Box::new(move |topic: &str, data: Value| {
                p(topic, crate::shared::object(data));
            }) as Box<dyn Fn(&str, Value)>
        };
        let text =
            |r: Arc<Redactor>| Box::new(move |s: &str| r.text(s)) as Box<dyn Fn(&str) -> String>;
        let memory = Rc::new(Memory::new(
            store.clone(),
            clock.clone(),
            ids.clone(),
            zone.clone(),
            publish_fn(deps.publish.clone()),
            text(redactor.clone()),
        ));
        let mut learning = Learning::new(
            store.clone(),
            clock,
            zone,
            publish_fn(deps.publish.clone()),
            text(redactor.clone()),
            Box::new(LearningSeams),
        );
        {
            let r = redactor.clone();
            learning.redact_id = Some(Box::new(move |s: &str| r.ident(s)));
            let r = redactor.clone();
            learning.redact_value = Some(Box::new(move |v: &Value| r.value(v)));
            let r = redactor.clone();
            learning.secret_values =
                Box::new(move || r.secret_values().iter().map(str::to_owned).collect());
        }
        let learning = Rc::new(learning);
        let j = journal.clone();
        let profile = Rc::new(CoreProfile::new(
            memory.clone(),
            Box::new(move |k, d| j.append(k, Some(d), None).map(|_| ())),
        ));
        let prompts = Rc::new(
            ferb_context::prompts::PromptRoot::new(&deps.prompts).map_err(|e| e.to_string())?,
        );
        let persona = persona::PersonaReview::new(&deps.home, deps.persona_scan.clone());
        let state = Rc::new(State {
            home: deps.home,
            user_home: deps.user_home,
            app_bundle: deps.app_bundle,
            store,
            clock: deps.clock,
            ids: deps.ids,
            config: deps.config,
            redactor,
            publish: deps.publish,
            timer_tokens: deps.timer_tokens,
            make_runtime: deps.make_runtime,
            prompts,
            journal,
            memory,
            learning,
            profile,
            tasks: deps.tasks,
            owner: deps.owner,
            questions: deps.questions,
            logins: deps.logins,
            purges: deps.purges,
            persona: RefCell::new(persona),
            shutting_down: Cell::new(false),
            stopped: Cell::new(false),
            forgetting: RefCell::default(),
            followups: RefCell::default(),
            login_names: Rc::new(tokio::sync::Mutex::new(())),
            import_task: RefCell::new(None),
            import_cancel: RefCell::new(String::new()),
            schedule_runs: RefCell::default(),
            last_runs: RefCell::default(),
            timer_wake: Rc::new(tokio::sync::Notify::new()),
            runs: RefCell::default(),
            sleepers: RefCell::default(),
            me: RefCell::new(Weak::new()),
        });
        *state.me.borrow_mut() = Rc::downgrade(&state);
        memory::wire(&state);
        Ok(state)
    }

    /// This state, as an `Rc` (for work it spawns).
    pub fn rc(&self) -> Rc<State> {
        self.me.borrow().upgrade().expect("the state is alive")
    }

    /// The store's one connection.
    pub fn db(&self) -> &Db {
        &self.store.db
    }

    /// `self._journal().append(kind, data, origin=...)`.
    pub fn journal(
        &self,
        kind: &str,
        data: Value,
        origin: Option<&str>,
    ) -> Result<Entry, HandlerError> {
        self.journal
            .append(kind, Some(data), origin)
            .map_err(learning_error)
    }

    /// `_publish`.
    pub fn publish(&self, topic: &str, data: Value) {
        (self.publish)(topic, crate::shared::object(data));
    }

    /// `_redact_text`.
    pub fn redact_text(&self, text: &str) -> String {
        self.redactor.text(text)
    }

    pub fn now(&self) -> Time {
        self.clock.now()
    }

    /// `self.config.get(section, key, default)`.
    pub fn config_get(&self, section: &str, key: &str) -> Option<Value> {
        self.config.get(section, key).ok().flatten()
    }

    /// The timer store on the store's connection (`self.timers`), telling `timer.changed` (DMN-548).
    pub fn timers(&self) -> ferb_clock::timers::Timers<'_> {
        let ids = self.ids.clone();
        let mut t = ferb_clock::timers::Timers::new(self.db(), &*self.clock, move || ids.new_id());
        if let Some(tokens) = self.timer_tokens.clone() {
            t = t.tokens(move || tokens());
        }
        let me = self.me.borrow().clone();
        t.on_change = Some(Box::new(move |timer, state| {
            if let Some(st) = me.upgrade() {
                schedules::timer_changed(&st, timer, state);
            }
            Ok(())
        }));
        let me = self.me.borrow().clone();
        t.on_error = Some(Box::new(move |timer, error| {
            if let Some(st) = me.upgrade() {
                schedules::timer_failed(&st, timer, error);
            }
        }));
        t
    }

    /// The timer store without the change and error reports (for reads, and inside those reports).
    pub fn timers_plain(&self) -> ferb_clock::timers::Timers<'_> {
        let ids = self.ids.clone();
        ferb_clock::timers::Timers::new(self.db(), &*self.clock, move || ids.new_id())
    }

    /// The schedule store (`self.schedules`).
    pub fn schedules(&self) -> ferb_clock::schedule_store::Schedules<'_> {
        let ids = self.ids.clone();
        ferb_clock::schedule_store::Schedules::new(
            self.db(),
            &*self.clock,
            move || ids.new_id(),
            self.clock.zone(),
        )
    }

    /// The device id the learners name (`config.get("device", "id", "dev")`).
    pub fn device_id(&self) -> String {
        match self.config_get("device", "id") {
            Some(v) if !v.is_null() => ferb_py::value::py_str(&v),
            _ => "dev".into(),
        }
    }
}

/// Every group A method, as `ferb/daemon/areas/rpc.py:130-186` maps it.
pub const METHODS: [&str; 26] = [
    "memory.learning",
    "memory.list",
    "memory.show",
    "memory.add",
    "memory.profile",
    "memory.edit",
    "memory.retract",
    "memory.forget",
    "memory.restore",
    "memory.repeats",
    "timeline.get",
    "setup.persona",
    "memory.import.estimate",
    "memory.import.start",
    "memory.import.status",
    "memory.import.cancel",
    "schedules.list",
    "schedules.change",
    "reminders.list",
    "reminders.cancel",
    "timers.repeat",
    "timers.states",
    "chat.history.put",
    "chat.history.import",
    "chat.history.page",
    "chat.history.days",
];

type Handler = fn(Rc<State>, Params) -> LocalFuture<HandlerResult>;

fn unary(server: &Server, local: &Local<State>, method: &str, handler: Handler) {
    let local = local.clone();
    server.register_unary(method, move |_session, params: Params| {
        let local = local.clone();
        async move {
            local
                .call(move |state| handler(state, params))
                .await
                .unwrap_or_else(|gone| Err(gone.into()))
        }
    });
}

/// Build group A's state on its loop (from `deps`) and register its 26 methods. Returns the handle the composition
/// keeps (start-up, the timer loop, the retention pass, shutdown).
pub fn register(server: &mut Server, deps: Deps) -> Result<Local<State>, String> {
    let thread = deps
        .loop_thread
        .clone()
        .unwrap_or_else(|| LoopThread::start("ferb-daemon-a"));
    let state = thread.try_install_rc(move || State::build(deps))?;
    for (method, handler) in handlers() {
        unary(server, &state, method, handler);
    }
    Ok(state)
}

/// `settings.get` and `settings.set` (`areas/rpc.py:36-51`), for the composition to register (they read
/// `ferb/daemon/settings.py`, ported here; the rest of rpc.py is group B's).
pub fn register_settings(
    server: &mut Server,
    config: Arc<Config>,
    host: Arc<dyn settings::SettingsHost>,
) {
    settings::handlers(config, host, server);
}

fn handlers() -> Vec<(&'static str, Handler)> {
    let mut out: Vec<(&'static str, Handler)> = Vec::new();
    out.extend(memory::handlers());
    out.extend(memory_import::handlers());
    out.extend(schedules::handlers());
    out.extend(chat_history::handlers());
    out
}

/// `{}` as a result.
pub fn empty() -> Params {
    Map::new()
}

/// A result built from `json!`.
pub fn ok(value: Value) -> HandlerResult {
    Ok(crate::shared::object(value))
}

/// The JSON of an optional time (`.isoformat()` or None).
pub fn iso_or_null(t: Option<&Time>) -> Value {
    t.map_or(Value::Null, |t| json!(ferb_clock::py::iso(t)))
}
