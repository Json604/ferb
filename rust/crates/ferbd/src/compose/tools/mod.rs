//! Every tool the gateway runs, registered through its `Tool` trait in Python's registry order (`ferb/registry.py`
//! `build`, `add_recording`, `add_recovery`, `add_owner_browsers`, `add_mac`, the shell, `add_trail`, the context
//! readers, `add_identity_browser`): the 97 tools of `rust/fixtures/catalog/tools.json`.
//!
//! A tool's schema, description, scopes, identities and facts come from the frozen catalog through the trait's
//! defaults, so a Rust tool can't serve a schema the models were never given. Each family's `describe` and `run`
//! call the crate that holds them; a tool whose part isn't composed in this build answers a refusal that says so
//! (`not available in this build`), never a made-up result.

pub mod browser;
pub mod context;
pub mod mac;
pub mod misc;
pub mod shell;
pub mod waits;
pub mod wrap;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use ferb_core::config::Config;
use ferb_core::interfaces::{BoxFuture, Clock, IdSource};
use ferb_core::types::{ActionSpec, ToolContext};
use ferb_gateway::gateway::Gateway;
use ferb_gateway::tool::{Args, Registry, Tool, ToolError, ToolOutput};
use ferb_sanitize::redact::Redactor;

use super::Options;
use crate::basics::{Events, Journal, SharedDb};

/// What a tool whose part isn't composed in this build answers.
pub fn not_in_this_build(what: &str) -> ToolError {
    ToolError::refused(format!(
        "not available in this build: {what} isn't part of the Rust daemon yet"
    ))
}

/// A run, as a family gives it.
pub type Run = Arc<
    dyn for<'a> Fn(&'a Args, &'a ToolContext) -> BoxFuture<'a, Result<ToolOutput, ToolError>>
        + Send
        + Sync,
>;

/// A tool of the frozen catalog whose behavior is given by functions: `describe` and `run`, everything else the
/// catalog's (the `Tool` trait's defaults).
pub struct Catalogued {
    pub name: &'static str,
    pub describe: Arc<dyn Fn(&Args) -> ActionSpec + Send + Sync>,
    pub run: Run,
}

impl Tool for Catalogued {
    fn name(&self) -> &str {
        self.name
    }
    fn describe(&self, args: &Args) -> ActionSpec {
        (self.describe)(args)
    }
    fn run<'a>(
        &'a self,
        args: &'a Args,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        (self.run)(args, ctx)
    }
}

/// A catalog tool whose part isn't composed in this build: described as the catalog's facts say (a read for an
/// observation, else its kind is the tool's own name), and refused when run.
pub fn unavailable(name: &'static str, what: &'static str) -> Arc<dyn Tool> {
    crate::basics::lock(&REFUSING).insert(name);
    let observation = ferb_gateway::tool::Facts::from_catalog(name)
        .map(|f| f.observation)
        .unwrap_or(false);
    Arc::new(Catalogued {
        name,
        describe: Arc::new(move |_a| ActionSpec::new(if observation { "read" } else { name })),
        run: Arc::new(move |_a, _c| Box::pin(async move { Err(not_in_this_build(what)) })),
    })
}

struct Inner {
    home: PathBuf,
    user_home: PathBuf,
    package: PathBuf,
    native: bool,
    clock: Arc<dyn ferb_clock::clock::ZonedClock>,
    ids: Arc<dyn IdSource>,
    redactor: Arc<Redactor>,
    config: Arc<Config>,
    db: SharedDb,
    journal: Arc<Journal>,
    events: Arc<Events>,
    shells: Option<Arc<ferb_shell::manager::ShellManager>>,
    shell_protection: Option<Arc<dyn ferb_shell::protection::Protection>>,
    /// The thread gateway calls run on: a call's future holds what can't move between threads.
    calls: ferb_daemon::shared::LoopThread,
    browsers: OnceLock<browser::Browsers>,
    gateway: OnceLock<Weak<Gateway>>,
    secrets: Mutex<Vec<ferb_gateway::catalog::SavedSecret>>,
    /// Group A's state (memory, learning), once registered.
    group_a: OnceLock<ferb_daemon::shared::Local<ferb_daemon::areas_a::State>>,
    /// The About area's document (`_about_document`), once group B is built.
    about: OnceLock<AboutDocument>,
    owner_secrets: OnceLock<Arc<ferb_identity::owner_secrets::OwnerSecrets>>,
    /// The live task records the gateway holds (`self.tasks`), mirrored for the tools that read a task's tree or
    /// deadline (the gateway keeps its own private).
    tasks: Mutex<std::collections::HashMap<String, ferb_gateway::gateway::TaskRecord>>,
    shutting_down: std::sync::atomic::AtomicBool,
    mac: OnceLock<mac::Mac>,
    snapshots: OnceLock<Arc<ferb_fs::recovery::Snapshots>>,
    /// L39's change record (`self.changes`): the gateway's hooks write it before a file call, the checker reads it.
    changes: Arc<ferb_fs::changes::Changes>,
    /// What a waiting worker tells the supervisor (`_wait_status`, `supervisor.corrections`), once it is built.
    waits: OnceLock<Arc<dyn WaitHooks>>,
    /// The disturbance sentinel (DMN-5): the gateway's hooks sample around on-screen calls; the supervisor starts and
    /// finishes each tree's observation.
    sentinel: OnceLock<Arc<ferb_daemon::areas_b::disturbance::DisturbanceSentinel>>,
    /// The owner-input tap and the Mac's presence (main.py:201-203), when the Mac's parts are on.
    owner: Option<super::presence::MacOwner>,
    /// The supervisor, once built: the browser drivers read a loop's rest and a task's pause from it.
    supervisor: OnceLock<ferb_supervisor::flow::Supervisor>,
    /// A task's live previews (`hooks.previews`): the gateway's hooks declare surfaces, FERB.app asks for frames.
    previews: OnceLock<ferb_daemon::areas_b::task_preview::TaskPreviews>,
}

/// `CallHooks.previews` (call_hooks.py:79) over these parts: the gateway's locks once it is attached, the Mac's image
/// masking, the Screen Recording check, the journal.
fn previews_of(opts: &Options, p: &Parts) -> ferb_daemon::areas_b::task_preview::TaskPreviews {
    let me = Arc::downgrade(&p.0);
    let images: Arc<dyn ferb_gateway::call_hooks::ImageSanitizer> = match p.mac() {
        Some(m) => m.images.clone(),
        None => Arc::new(mac::Images {
            redactor: p.redactor(),
            native: opts.native,
        }),
    };
    let native = opts.native;
    let journal = p.journal();
    super::previews::build(super::previews::Over {
        locks: Arc::new(move || Some(me.upgrade()?.gateway.get()?.upgrade()?.locks().clone())),
        images,
        // `_screen_recording_allowed`: read only when a capture failed, to say why (a daemon without the Mac's
        // parts can't read it: allowed, as Python's answer when the read raises)
        screen_recording: Arc::new(move || {
            !native
                || ferb_mac_hands::native::permissions::screen_recording()
                    == ferb_mac_hands::permissions::Status::Granted
        }),
        note: Arc::new(move |kind: &str, data: serde_json::Value| journal.note(kind, data)),
    })
}

/// [`Parts`] without keeping them alive.
#[derive(Clone)]
pub struct WeakParts(Weak<Inner>);

impl WeakParts {
    pub fn upgrade(&self) -> Option<Parts> {
        self.0.upgrade().map(Parts)
    }
}

/// The change record (L39) behind the gateway's hooks: `Changes.before` for the files a call names.
pub struct ChangeHook(pub Arc<ferb_fs::changes::Changes>);

impl ferb_gateway::call_hooks::ChangeRecord for ChangeHook {
    fn before<'a>(
        &'a self,
        task: &'a str,
        tree: &'a str,
        paths: Vec<String>,
    ) -> ferb_core::interfaces::BoxFuture<
        'a,
        Result<serde_json::Map<String, serde_json::Value>, String>,
    > {
        Box::pin(async move {
            Ok(match self.0.before(task, tree, &paths).to_json() {
                serde_json::Value::Object(m) => m,
                _ => serde_json::Map::new(),
            })
        })
    }
}

/// The supervisor's side of a worker's `wait.for` (schedules.py `_wait_status`, desktop.py's `steered`).
pub trait WaitHooks: Send + Sync {
    fn status(
        &self,
        worker: &str,
        task: &str,
        until: Option<&ferb_clock::py::Time>,
        line: Option<&str>,
    );
    fn steered(&self, task: &str) -> i64;
    /// `_sleep_until`'s side (ORCH-598): the worker waits (not stalled) and its task's status says until when and
    /// why; `until` None: it woke.
    fn sleeping(&self, worker: &str, task: &str, until: Option<&ferb_clock::py::Time>, why: &str) {
        let _ = why;
        self.status(worker, task, until, until.map(|_| ""));
    }
}

/// Tools built as refusing "not available in this build" (none, once every part is composed): the catalog test
/// reads it.
static REFUSING: Mutex<std::collections::BTreeSet<&'static str>> =
    Mutex::new(std::collections::BTreeSet::new());

pub fn refusing() -> Vec<String> {
    crate::basics::lock(&REFUSING)
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// The About area's whole document, built off the loop.
pub type AboutDocument =
    Arc<dyn Fn() -> Result<serde_json::Map<String, serde_json::Value>, String> + Send + Sync>;

/// An `IdSource` behind an `Arc`, by reference.
pub struct IdsRef(pub Arc<dyn IdSource>);
impl IdSource for IdsRef {
    fn new_id(&self) -> String {
        self.0.new_id()
    }
}

/// The owner-input tap and the Mac's presence, when the Mac's parts are on.
fn mac_owner(opts: &Options) -> Option<super::presence::MacOwner> {
    #[cfg(target_os = "macos")]
    if opts.native {
        let clock: Arc<dyn Clock> = opts.clock.clone();
        return Some(super::presence::MacOwner::build(clock));
    }
    let _ = opts;
    None
}

/// The desktop read the sentinel samples: the Mac's own (`DesktopProbe`: Accessibility reads, as the Mac hands make)
/// when the Mac hands' native parts are on.
fn desktop_read(opts: &Options) -> Arc<dyn ferb_daemon::areas_b::disturbance::DesktopRead> {
    #[cfg(target_os = "macos")]
    if opts.mac {
        return Arc::new(super::desktop::MacDesktop::default());
    }
    let _ = opts;
    Arc::new(super::desktop::Unread)
}

/// The parts the tools are built over, shared with the areas that drive the same parts (shell processes, browsers).
#[derive(Clone)]
pub struct Parts(Arc<Inner>);

impl Parts {
    pub fn new(
        opts: &Options,
        config: &Arc<Config>,
        redactor: &Arc<Redactor>,
        db: &SharedDb,
        journal: &Arc<Journal>,
        events: &Arc<Events>,
    ) -> Parts {
        let (shells, shell_protection) = match shell::manager(opts, config, redactor) {
            Some((m, p)) => (Some(m), Some(p)),
            None => (None, None),
        };
        let p = Parts(Arc::new(Inner {
            home: opts.home.clone(),
            user_home: opts.user_home.clone(),
            package: opts.package.clone(),
            native: opts.native,
            clock: opts.clock.clone(),
            ids: opts.ids.clone(),
            redactor: redactor.clone(),
            config: config.clone(),
            db: db.clone(),
            journal: journal.clone(),
            events: events.clone(),
            shells,
            shell_protection,
            calls: ferb_daemon::shared::LoopThread::start("ferb-calls"),
            browsers: OnceLock::new(),
            gateway: OnceLock::new(),
            secrets: Mutex::new(Vec::new()),
            group_a: OnceLock::new(),
            about: OnceLock::new(),
            owner_secrets: OnceLock::new(),
            tasks: Mutex::new(std::collections::HashMap::new()),
            shutting_down: std::sync::atomic::AtomicBool::new(false),
            mac: OnceLock::new(),
            snapshots: OnceLock::new(),
            changes: Arc::new(ferb_fs::changes::Changes::new(
                &opts.home.join("changes").to_string_lossy(),
                None,
                &opts.user_home.to_string_lossy(),
            )),
            waits: OnceLock::new(),
            sentinel: OnceLock::new(),
            owner: mac_owner(opts),
            supervisor: OnceLock::new(),
            previews: OnceLock::new(),
        }));
        let _ = p.0.mac.set(mac::Mac::build(opts, &p));
        let _ = p.0.previews.set(previews_of(opts, &p));
        let _ =
            p.0.sentinel
                .set(super::desktop::sentinel(&p, desktop_read(opts)));
        let _ = p.0.browsers.set(browser::Browsers::build(opts, &p));
        p
    }

    /// A handle that doesn't keep the parts alive (for the parts' own members that call back into them).
    pub fn downgrade(&self) -> WeakParts {
        WeakParts(Arc::downgrade(&self.0))
    }

    /// The owner-input tap and the Mac's presence (None when the Mac's parts are off).
    pub fn owner(&self) -> Option<&super::presence::MacOwner> {
        self.0.owner.as_ref()
    }

    /// The supervisor, once built: the previews read what each task is doing from it (`preview_status`).
    pub fn set_supervisor(&self, s: ferb_supervisor::flow::Supervisor) {
        let _ = self.0.supervisor.set(s);
        let me = Arc::downgrade(&self.0);
        self.previews()
            .set_status(Arc::new(move |root: &str, worker: &str| {
                me.upgrade()?.supervisor.get()?.preview_status(root, worker)
            }));
    }

    /// A task's live previews (built with the parts).
    pub fn previews(&self) -> &ferb_daemon::areas_b::task_preview::TaskPreviews {
        self.0.previews.get().expect("built with the parts")
    }
    pub fn supervisor(&self) -> Option<&ferb_supervisor::flow::Supervisor> {
        self.0.supervisor.get()
    }

    /// The disturbance sentinel (built with the parts).
    pub fn sentinel(&self) -> Option<Arc<ferb_daemon::areas_b::disturbance::DisturbanceSentinel>> {
        self.0.sentinel.get().cloned()
    }

    pub fn home(&self) -> &PathBuf {
        &self.0.home
    }
    /// FERB's package folder (`ferb/`): the runtime Markdown (abilities' procedures, prompts).
    pub fn package(&self) -> &PathBuf {
        &self.0.package
    }
    pub fn set_group_a(&self, a: ferb_daemon::shared::Local<ferb_daemon::areas_a::State>) {
        let _ = self.0.group_a.set(a);
    }
    pub fn group_a(&self) -> Option<ferb_daemon::shared::Local<ferb_daemon::areas_a::State>> {
        self.0.group_a.get().cloned()
    }
    pub fn set_about_document(&self, f: AboutDocument) {
        let _ = self.0.about.set(f);
    }
    pub fn about_document(&self) -> Option<AboutDocument> {
        self.0.about.get().cloned()
    }
    pub fn set_owner_secrets(&self, s: Arc<ferb_identity::owner_secrets::OwnerSecrets>) {
        let _ = self.0.owner_secrets.set(s);
    }
    pub fn owner_secrets(&self) -> Option<Arc<ferb_identity::owner_secrets::OwnerSecrets>> {
        self.0.owner_secrets.get().cloned()
    }
    /// A live task for the gateway (`tasks.hold`): kept here too, for the tools that read its tree or deadline.
    pub fn hold_task(&self, gateway: &Gateway, rec: ferb_gateway::gateway::TaskRecord) {
        crate::basics::lock(&self.0.tasks).insert(rec.task_id.clone(), rec.clone());
        gateway.add_task(rec);
    }
    pub fn release_task(&self, gateway: &Gateway, task_id: &str) {
        crate::basics::lock(&self.0.tasks).remove(task_id);
        gateway.remove_task(task_id);
    }
    /// `self.tasks.get(task_id)`.
    pub fn task(&self, task_id: &str) -> Option<ferb_gateway::gateway::TaskRecord> {
        crate::basics::lock(&self.0.tasks).get(task_id).cloned()
    }
    /// S1's snapshots (`recovery.snapshot`, the gateway's before shell commands and permanent deletes).
    pub fn set_snapshots(&self, s: Arc<ferb_fs::recovery::Snapshots>) -> bool {
        self.0.snapshots.set(s).is_ok()
    }
    pub fn snapshots(&self) -> Option<Arc<ferb_fs::recovery::Snapshots>> {
        self.0.snapshots.get().cloned()
    }
    /// The Mac's parts (driver, hearing, recordings, the image sanitizer).
    /// L39's change record.
    pub fn changes(&self) -> Arc<ferb_fs::changes::Changes> {
        self.0.changes.clone()
    }
    /// The supervisor's side of `wait.for`, once it is built.
    pub fn set_wait_hooks(&self, hooks: Arc<dyn WaitHooks>) {
        let _ = self.0.waits.set(hooks);
    }
    pub fn wait_hooks(&self) -> Option<Arc<dyn WaitHooks>> {
        self.0.waits.get().cloned()
    }
    pub fn mac(&self) -> Option<&mac::Mac> {
        self.0.mac.get()
    }
    /// A job on the calls' thread (what a gateway future needs: it isn't `Send`); None when that thread stopped.
    pub async fn on_calls_thread<T, F, Fut>(&self, f: F) -> Option<T>
    where
        T: Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = T> + 'static,
    {
        self.0.calls.run(f).await.ok()
    }
    pub fn set_shutting_down(&self) {
        self.0
            .shutting_down
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn shutting_down(&self) -> bool {
        self.0
            .shutting_down
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn user_home(&self) -> &PathBuf {
        &self.0.user_home
    }
    pub fn native(&self) -> bool {
        self.0.native
    }
    pub fn clock(&self) -> Arc<dyn Clock> {
        self.0.clock.clone()
    }
    pub fn zoned(&self) -> Arc<dyn ferb_clock::clock::ZonedClock> {
        self.0.clock.clone()
    }
    pub fn ids(&self) -> Arc<dyn IdSource> {
        self.0.ids.clone()
    }
    pub fn redactor(&self) -> Arc<Redactor> {
        self.0.redactor.clone()
    }
    pub fn config(&self) -> Arc<Config> {
        self.0.config.clone()
    }
    pub fn db(&self) -> SharedDb {
        self.0.db.clone()
    }
    pub fn journal(&self) -> Arc<Journal> {
        self.0.journal.clone()
    }
    pub fn events(&self) -> Arc<Events> {
        self.0.events.clone()
    }
    pub fn shells(&self) -> Option<Arc<ferb_shell::manager::ShellManager>> {
        self.0.shells.clone()
    }
    pub fn shell_protection(&self) -> Option<Arc<dyn ferb_shell::protection::Protection>> {
        self.0.shell_protection.clone()
    }

    /// One gateway call (its future is `Send` since journal 44: no thread of its own).
    pub async fn call(
        &self,
        gateway: Arc<Gateway>,
        grant: Arc<ferb_rpc::auth::RunGrant>,
        name: String,
        args: ferb_gateway::tool::Args,
        caller_left: ferb_gateway::gateway::Left,
    ) -> Result<ferb_gateway::reply::Reply, ferb_gateway::gateway::CallError> {
        gateway
            .call_as(
                &grant,
                &name,
                &args,
                ferb_gateway::gateway::Cancel::new(),
                caller_left,
            )
            .await
    }

    pub fn browsers(&self) -> Option<&browser::Browsers> {
        self.0.browsers.get()
    }
    pub fn owner_driver(&self) -> Option<Arc<ferb_browser::driver::Driver>> {
        self.browsers().map(|b| b.router.clone())
    }
    pub fn ferb_driver(&self) -> Option<Arc<ferb_browser::driver::Driver>> {
        self.browsers().and_then(|b| b.identity.clone())
    }
    /// The `ferb_browser.*` tools (FERB's own profile), for when it attaches.
    pub fn ferb_browser_tools(&self) -> Vec<Arc<dyn Tool>> {
        self.browsers().map(|b| b.ferb_tools()).unwrap_or_default()
    }

    /// The gateway, once built (the tools that read it at run time: secrets.list's sites).
    pub fn attach_gateway(&self, gw: &Arc<Gateway>) {
        let _ = self.0.gateway.set(Arc::downgrade(gw));
    }

    pub fn gateway(&self) -> Option<Arc<Gateway>> {
        self.0.gateway.get().and_then(Weak::upgrade)
    }

    /// What a description reads when the listing is made (saved secrets, the downloads total).
    pub fn live(&self) -> ferb_gateway::catalog::Live {
        // `_accounts(driver)`: FERB's own saved secrets (names, accounts, origins: never a value), read now
        let mut secrets = crate::basics::lock(&self.0.secrets).clone();
        if secrets.is_empty() {
            secrets = ferb_identity::secrets::specs_from_config(
                self.0.config.section("secrets").ok().as_ref(),
            )
            .into_iter()
            .map(|s| ferb_gateway::catalog::SavedSecret {
                key: s.secret.key,
                account_hint: s.account_hint,
                allowed_origins: s.allowed_origins,
            })
            .collect();
        }
        ferb_gateway::catalog::Live {
            secrets,
            downloads_total_bytes: None,
        }
    }
}

/// The registry in Python's order (see the module's text). FERB's own browser's tools come last, as when its
/// profile is attached.
pub fn registry(p: &Parts) -> Registry {
    let mut r = Registry::new();
    let mut add = |t: Arc<dyn Tool>| {
        r.add(t).expect("each tool once");
    };
    // build(): actions.search, abilities, the file tools, owner.ask
    for t in misc::first(p) {
        add(t);
    }
    // add_recording, add_recovery
    add(mac::recording_tool(p, "recording.stop"));
    add(mac::recording_tool(p, "recording.inspect"));
    add(misc::recovery_snapshot(p));
    // _build_browsers: the owner's browsers
    if let Some(b) = p.browsers() {
        for t in b.owner_tools() {
            add(t);
        }
    }
    // _build_mac_and_shell: Mac tools, hearing, the shell
    for t in mac::mac_tools(p) {
        add(t);
    }
    add(mac::listen_tool(p));
    for t in shell::tools(p) {
        add(t);
    }
    // add_trail, the task's evidence, time, skills, the context readers, secrets, about, conversations, learning
    for t in misc::rest(p) {
        add(t);
    }
    // add_identity_browser: FERB's own profile's tools join once its profile is paired as FERB (the start's
    // `_attach_identity_if_paired`, compose/mod.rs)
    // the file tools' work in a worker thread, as files.py runs it
    for name in wrap::THREADED {
        if let Some(t) = r.get(name).cloned() {
            r.set(wrap::threaded(t));
        }
    }
    r
}

pub use browser::browsers_deps;
