//! The composition root (`ferb/daemon/main.py`, `areas/compose.py`): every engine crate built from FERB's home, each
//! crate's seams filled with the others, the RPC handlers registered, and the start order, the stop marker and the
//! clean shutdown Python's daemon has.
//!
//! [`Daemon::start`] is Python's `start` + `_start`; [`Daemon::shutdown`] its `shutdown`. What a test or a stripped
//! run changes comes in through [`Options`] (Python's constructor flags), never through a global.

pub mod areas;
pub mod egress;
pub mod env_seams;
pub mod gateway;
pub mod group_c;
pub mod loops;
pub mod memory_loops;
pub mod rpc;
pub mod runtimes;
pub mod supervisor;
pub mod tools;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ferb_clock::clock::ZonedClock;
use ferb_core::config::Config;
use ferb_core::interfaces::{Clock, IdSource};
use ferb_daemon::areas_a::{self, Logins, Owner, Questions, State as AState};
use ferb_daemon::shared::Local;
use ferb_gateway::gateway::Gateway;
use ferb_rpc::{RunTokens, Server};
use ferb_sanitize::redact::Redactor;
use serde_json::{json, Map, Value};
use tokio::task::JoinHandle;

use crate::basics::{lock, open_store, Events, Journal, SharedDb, Stop};

/// FERB's version as `ferb.__version__` gives it.
pub const VERSION: &str = "0.28.0";

/// How the daemon reaches the owner's Keychain (FERB's API keys, saved secrets, Jev's key).
#[derive(Clone)]
pub enum Keychain {
    /// The signed helper (`codesign` against its pin, then the helper process) and `/usr/bin/security`.
    Real,
    /// Nothing reaches the Keychain: every helper call is refused as "unavailable" and `security` finds nothing
    /// (a daemon started for a test).
    Refused,
}

/// Python's `Daemon(...)` flags: what a run without the Mac's own parts turns off.
#[derive(Clone)]
pub struct Options {
    pub home: PathBuf,
    /// `Path.home()`: the owner's home folder.
    pub user_home: PathBuf,
    /// FERB's package folder (`ferb/`): prompts and the other runtime Markdown.
    pub package: PathBuf,
    /// The FERB.app this daemon runs from (`paths.app_bundle()`).
    pub app_bundle: Option<PathBuf>,
    pub clock: Arc<dyn ZonedClock>,
    pub ids: Arc<dyn IdSource>,
    pub keychain: Keychain,
    /// The Mac's own parts: TCC reads, the running-app list, watchers, the owner-input tap, presence, notifications,
    /// the FERB.app peer check. Off: none is read or started (Python's `watchers=False`, test stand-ins).
    pub native: bool,
    /// The Mac hands' own native parts (the pasteboard, Vision's text recognition for tool images, app sound capture,
    /// local snapshots). The real daemon: on. A test daemon: off unless it asks (a real-machine check).
    pub mac: bool,
    /// The retention pass and the start's sweep of stale run folders (`retention`).
    pub retention: bool,
    /// The redactor learns FERB's API keys in use at start (GATE-573, `preload_secrets`).
    pub preload_secrets: bool,
    /// Who may connect (`owner_uid`); None: this process's user.
    pub owner_uid: Option<u32>,
    /// FERB.app-only methods: None checks the peer's signature (`ferb_platform_macos::rpc::attach`); Some(answer)
    /// answers every peer the same (a test's `is_app`).
    pub app_check: Option<bool>,
    /// A timer's claim token (`secrets.token_hex(8)`); None: random.
    pub timer_tokens: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    /// Where browsers keep their user data (`~/Library/Application Support`).
    pub browser_support: PathBuf,
    /// The timer loop runs (`Timers.run`); a test with a fake clock ticks it through its control socket instead.
    pub timer_loop: bool,
    /// A browser step starts browsers (`browser_launch`); false: every launch is refused.
    pub launch_browsers: bool,
    /// Seams replaced after the real ones are built (Python's tests set attributes on the daemon the same way).
    /// Only the `stand-ins` build's test entry sets it; the program's own start never does.
    pub overrides: Option<Arc<dyn Overrides>>,
    /// The shell's egress proxy and what its callbacks read (compose/egress.rs), shared by the shell and the start.
    pub egress: Arc<egress::Egress>,
    /// The memory's background work (compose/memory_loops.rs: re-index, embedder, the hourly pass); Python's
    /// `distill` and `embeddings` flags.
    pub memory_loops: bool,
    /// The resource log (L30, `resources`): a sample every five minutes and at shutdown.
    pub resources: bool,
    /// The questions' ids (`"ap-" + token_hex(6)`; a test build: a counter).
    pub question_ids: gateway::QuestionIds,
}

/// Replacements for seams the real composition fills, applied once each area's deps are built.
pub trait Overrides: Send + Sync {
    fn group_a(&self, _deps: &mut areas_a::Deps) {}
    fn group_b(&self, _deps: &mut ferb_daemon::areas_b::Deps) {}
    fn group_c(&self, _deps: &mut crate::areas_c::Deps) {}
    /// What the supervisor reads of the Mac and FERB, and its runtime factory (a recorder's stand-ins).
    fn supervisor_reads(&self, real: Arc<dyn supervisor::Reads>) -> Arc<dyn supervisor::Reads> {
        real
    }
    /// The supervisor's own deps (a test's tokenizer, wrapper id, chat ids).
    fn supervisor_build(&self, _parts: &mut supervisor::BuildParts) {}
    /// Called once the daemon is built (a test build's hold on the parts it drives).
    fn started(&self, _daemon: &Arc<Daemon>) {}
}

impl Options {
    /// The real daemon's options for FERB's home `home`.
    pub fn real(home: PathBuf) -> Options {
        let user_home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .or_else(ferb_core::paths::account_home)
            .unwrap_or_default();
        let zone = ferb_clock::zone::local_zone().unwrap_or_else(|_| ferb_clock::zone::Zone::utc());
        Options {
            browser_support: user_home.join("Library/Application Support"),
            home,
            user_home,
            package: crate::home::package_folder(),
            app_bundle: crate::home::app_bundle(),
            clock: Arc::new(ferb_clock::clock::SystemClock::new(zone)),
            ids: Arc::new(ferb_core::ids::RandomIds),
            keychain: Keychain::Real,
            native: true,
            mac: true,
            retention: true,
            preload_secrets: true,
            owner_uid: None,
            app_check: None,
            timer_tokens: None,
            launch_browsers: true,
            timer_loop: true,
            overrides: None,
            egress: Arc::new(egress::Egress::default()),
            memory_loops: true,
            resources: true,
            question_ids: Arc::new(gateway::random_question_id),
        }
    }
}

/// Why the daemon didn't start.
#[derive(Debug)]
pub enum StartError {
    /// Another daemon answers on this home's socket, or holds its lock (one daemon per home).
    Running(String),
    /// The database was migrated by a newer FERB: nothing was changed (STORE-3). Not a crash: exit 0.
    NewerStore(String),
    Failed(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::Running(m) | StartError::NewerStore(m) | StartError::Failed(m) => {
                f.write_str(m)
            }
        }
    }
}

/// The running daemon: what `start` built, kept for its lifetime and for `shutdown`.
pub struct Daemon {
    pub opts: Options,
    pub config: Arc<Config>,
    pub redactor: Arc<Redactor>,
    pub db: SharedDb,
    pub journal: Arc<Journal>,
    pub events: Arc<Events>,
    /// The in-process bus (`self.bus`): every published event, as `logs.subscribe` reads it.
    pub bus: ferb_core::bus::Bus,
    pub stop: Arc<Stop>,
    pub server: Server,
    pub tokens: Arc<RunTokens>,
    pub gateway: Arc<Gateway>,
    pub ledger: Arc<gateway::RealLedger>,
    /// The planner, its seam and its chat RPCs.
    pub supervisor: supervisor::Built,
    pub a: Local<AState>,
    pub b: areas::GroupB,
    pub tools: tools::Parts,
    /// The Claude runtime's tool-list relay.
    pub claude_proxy: Option<Arc<dyn ferb_runtimes::claude::ToolProxy>>,
    /// Ends the memory's re-index and embedder threads at shutdown.
    pub memory_stop: std::sync::OnceLock<Arc<std::sync::atomic::AtomicBool>>,
    /// The resource log (None: off, or it couldn't start: journaled).
    pub resource_log: Mutex<Option<crate::resources::ResourceLog>>,
    /// Set by SIGTERM, SIGINT or `update.prepare` (`_stop`).
    pub quit: Arc<tokio::sync::Notify>,
    loops: Mutex<Vec<JoinHandle<()>>>,
    _lock: std::fs::File,
}

struct RpcClock(Arc<dyn ZonedClock>);

impl ferb_rpc::Clock for RpcClock {
    fn monotonic(&self) -> f64 {
        self.0.monotonic()
    }
}

/// One daemon per home: an exclusive lock on `run/daemon.lock`, held for the process's life. Python checks only
/// that nobody answers on the socket (`ensure_socket_free`); two starts at the same moment could both pass that
/// check and migrate one database together. Both checks are made here.
fn take_lock(home: &std::path::Path) -> Result<std::fs::File, StartError> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let run = home.join("run");
    std::fs::create_dir_all(&run).map_err(|e| StartError::Failed(e.to_string()))?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(run.join("daemon.lock"))
        .map_err(|e| StartError::Failed(e.to_string()))?;
    // SAFETY: the descriptor is open for the duration of the call; flock takes no pointers.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(StartError::Running(
            "another FERB daemon is already running with this FERB home".into(),
        ));
    }
    Ok(file)
}

impl Daemon {
    /// `start` (main.py:318): build everything in dependency order, serve as soon as the RPCs are registered
    /// (answering only `ping` until the start is done, DMN-510), then the slow part.
    pub async fn start(opts: Options) -> Result<Arc<Daemon>, StartError> {
        let home = opts.home.clone();
        std::fs::create_dir_all(&home).map_err(|e| StartError::Failed(e.to_string()))?;
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700));
        }
        crate::home::raise_file_limit();
        let socket = ferb_core::paths::socket_path(&home);
        // refuse before touching the database if another daemon runs
        ferb_rpc::server::ensure_socket_free(&socket)
            .map_err(|e| StartError::Running(format!("FERB: {e}")))?;
        let lock_file = take_lock(&home)?;

        // salvage.check_home: a database a newer FERB migrated stops the start here, unchanged
        let build = ferb_store::salvage::this_build(VERSION);
        let now = || ferb_py::time::DateTime::now_local();
        let toml_error = |text: &str| toml_parse_error(text);
        let set_aside = ferb_store::salvage::check_home(
            &home,
            &build,
            &ferb_store::salvage::Seams {
                now: &now,
                toml_error: &toml_error,
            },
        )
        .map_err(|e| {
            if e.to_string().contains("newer") {
                StartError::NewerStore(e.to_string())
            } else {
                StartError::Failed(e.to_string())
            }
        })?;

        let config = Arc::new(Config::new(&home));
        let defaults = ferb_core::config::PrivacyDefaults {
            never_read: ferb_policy::protection::default_never_read()
                .into_iter()
                .map(String::from)
                .collect(),
            never_write: ferb_policy::protection::DEFAULT_NEVER_WRITE
                .iter()
                .map(|s| s.to_string())
                .collect(),
            terminal_apps: ferb_policy::protection::DEFAULT_TERMINAL_APPS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            exclude_paths: ferb_sense::exclusions::DEFAULT_EXCLUDE_PATHS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        };
        config
            .ensure_privacy_defaults(&defaults)
            .map_err(|e| StartError::Failed(format!("config.toml: {e}")))?;
        // _load_config: [protection] and the workspace, once at start, before migrations and browsers
        let protection =
            gateway::protection(&config, &opts.user_home, &home, opts.app_bundle.as_deref())
                .map_err(|e| StartError::Failed(format!("FERB's protection lists: {e}")))?;
        let device_id = match config.get("device", "id") {
            Ok(Some(Value::String(s))) => s,
            Ok(Some(v)) if !v.is_null() => ferb_py::value::py_str(&v),
            _ => "unknown-device".into(),
        };
        let redactor = Arc::new(Redactor::new(&[], &[]));
        apply_privacy(&config, &redactor);

        let db: SharedDb = Arc::new(Mutex::new(
            open_store(&home.join("ferb.db")).map_err(StartError::Failed)?,
        ));
        {
            let d = lock(&db);
            let _ = d
                .conn()
                .execute_batch(ferb_learning::notes::SOURCE_REF_INDEX);
        }
        let clock: Arc<dyn Clock> = opts.clock.clone();
        let journal = Arc::new(Journal {
            db: db.clone(),
            clock: clock.clone(),
            ids: opts.ids.clone(),
            redactor: redactor.clone(),
        });
        for entry in &set_aside {
            journal.note("store.set_aside", Value::Object(entry.clone()));
        }
        // the resource log: its first window starts here, so start-up's own work is in it (L30)
        let resource_log = if opts.resources {
            match crate::resources::MacReadings::new().and_then(|readings| {
                let d = lock(&db);
                crate::resources::ResourceLog::new(
                    &d,
                    Box::new(SampleClock(opts.clock.clone())),
                    Box::new(readings),
                    {
                        let r = redactor.clone();
                        Box::new(move |t: &str| r.text(t))
                    },
                    // perception's batcher is off (Jev is off: notices paused); no live previews without a planner
                    Box::new(|| (0, std::collections::BTreeMap::new())),
                    Box::new(Map::new),
                    std::process::id() as i32,
                )
            }) {
                Ok(l) => Some(l),
                Err(e) => {
                    journal.note("resources.failed", json!({"error": e}));
                    None
                }
            }
        } else {
            None
        };
        let events = Events::new(redactor.clone());
        // the bus as a consumer of every event (compose.py:237): `logs.subscribe` reads it; a topic the bus doesn't
        // declare is not offered (Python's bus raises for it)
        let bus = ferb_core::bus::Bus::new(clock.clone());
        {
            let bus = bus.clone();
            events.events.on(
                None,
                Arc::new(move |topic: &str, data: Map<String, Value>| {
                    let _ = bus.publish(topic, Some(data));
                }),
            );
        }
        let stop = Stop::new(&home);

        // config upgrades, each once and journaled
        for decided in ferb_core::config::UPGRADES {
            if journal
                .of_kind("config.upgraded", "upgrade", decided)
                .is_empty()
            {
                let changes = config.upgrade(decided).unwrap_or_default();
                journal.note(
                    "config.upgraded",
                    json!({"upgrade": decided, "changes": changes}),
                );
            }
        }
        new_protection_defaults(&config, &journal);
        legacy_continuity(&db, &journal, &redactor);

        let ledger = Arc::new(gateway::RealLedger {
            db: db.clone(),
            clock: clock.clone(),
            ids: opts.ids.clone(),
        });

        // the server and its run tokens
        let tokens = Arc::new(RunTokens::new(Arc::new(RpcClock(opts.clock.clone()))));
        let mut server = match opts.owner_uid {
            Some(uid) => Server::with_owner_uid(&socket, tokens.clone(), uid),
            None => Server::new(&socket, tokens.clone()),
        };
        match opts.app_check {
            Some(answer) => server.set_is_app(move |_| answer),
            None => ferb_platform_macos::rpc::attach(&server, &home),
        }
        {
            let j = journal.clone();
            server.on_internal(move |method, e| {
                // a call the gateway cancelled (a stop while it ran) is no internal error: Python's handler task is
                // cancelled and journals nothing
                if e.type_name == "CancelledError" {
                    return;
                }
                j.note(
                    ferb_rpc::server::INTERNAL_ERROR,
                    json!({"method": method, "error": e.type_name, "message": j.redactor.text(&e.message)}),
                )
            });
            let j = journal.clone();
            server.on_slow(move |method, seconds| {
                j.note(
                    ferb_rpc::server::SLOW_CALL,
                    json!({"method": method, "seconds": (seconds * 10.0).round() / 10.0}),
                )
            });
        }

        // the gateway and its tools
        let owner_secrets = Arc::new(areas::owner_secrets(&opts, &home));
        let tool_parts = tools::Parts::new(&opts, &config, &redactor, &db, &journal, &events);
        tool_parts.set_owner_secrets(owner_secrets.clone());
        let registry = tools::registry(&tool_parts);
        // compose.py `_publish` (a task's question names its request), once the supervisor's seam exists
        let publisher: Arc<std::sync::OnceLock<std::sync::Weak<supervisor::SupDaemon>>> =
            Arc::new(std::sync::OnceLock::new());
        let router = Arc::new(supervisor::Router::default());
        let gw = {
            let ev = events.clone();
            let publisher = publisher.clone();
            let j = journal.clone();
            let cfg = config.clone();
            gateway::build(
                registry,
                gateway::GatewayParts {
                    db: db.clone(),
                    clock: clock.clone(),
                    ids: opts.ids.clone(),
                    redactor: redactor.clone(),
                    ledger: ledger.clone(),
                    publish: Box::new(move |topic, data| {
                        let v = crate::basics::object(ferb_gateway::gateway::py_value(&data));
                        match publisher.get().and_then(std::sync::Weak::upgrade) {
                            Some(d) => d.publish_event(topic, v),
                            None => ev.publish(topic, v),
                        }
                    }),
                    journal_problem: Box::new(move |kind, data| j.note(kind, Value::Object(data))),
                    home: home.clone(),
                    user_home: opts.user_home.clone(),
                    device_id: device_id.clone(),
                    protection,
                    owner_secrets: Some(owner_secrets.clone()),
                    owner_contacts: Box::new(Vec::new),
                    pay_same_site_forms: Box::new(move || {
                        !matches!(
                            cfg.get("policy", "pay_same_site_forms"),
                            Ok(Some(Value::Bool(false)))
                        )
                    }),
                    native: opts.native,
                    hooks: tools::mac::hooks(&tool_parts),
                    snapshots: tool_parts.snapshots().map(|s| {
                        Arc::new(gateway::GatewaySnapshots(s))
                            as Arc<dyn ferb_gateway::seams::Snapshots>
                    }),
                    proc_task: {
                        let e = opts.egress.clone();
                        Box::new(move |proc: &str| e.owner_of(proc))
                    },
                    changes: tool_parts.changes(),
                    question_ids: opts.question_ids.clone(),
                    route_question: Some(router.clone()),
                },
            )
        };
        tool_parts.attach_gateway(&gw);
        // `_attach_identity_if_paired`: FERB's own profile's tools join once it is paired as FERB
        if let Some(b) = tool_parts.browsers() {
            if b.identity_paired(&config) {
                let mut reg = gw.registry();
                for t in b.ferb_tools() {
                    reg.set(t);
                }
            }
        }
        // the proxy's destination gate is the gateway from now on
        let _ = opts.egress.gateway.set(Arc::downgrade(&gw));
        let _ = opts.egress.journal.set(journal.clone());
        // the browsers' native-host channel (`Hub.serve` behind ferb-rpc's NativeChannel) and the extension ids a
        // native host's hello is checked against (`accepted_extension_ids`)
        if let Some(channel) = tools::browser::native_channel(&tool_parts) {
            server.set_native(channel);
        }
        {
            let cfg = config.clone();
            server.set_extension_ids(move || tools::browser::extension_ids(&cfg));
        }

        // the learners' model calls go through the runtimes area, built with group B below
        let learning_area: Arc<std::sync::OnceLock<ferb_daemon::areas_b::runtimes::Runtimes>> =
            Arc::new(std::sync::OnceLock::new());

        // the supervisor (compose.py:130): built over the gateway; group A and the runtimes join it once registered
        let a_cell: Arc<std::sync::OnceLock<Local<AState>>> = Arc::new(std::sync::OnceLock::new());
        let sup = {
            let host = ferb_daemon::areas_b::core::Host {
                home: home.clone(),
                config: config.clone(),
                redactor: redactor.clone(),
                journal: journal.clone(),
                bus: events.clone(),
                clock: clock.clone(),
            };
            let seams = Arc::new(env_seams::EnvSeams {
                host: host.clone(),
                tools: tool_parts.clone(),
                runtimes: learning_area.clone(),
                a: a_cell.clone(),
                native: opts.native,
                activity: ferb_sense::daily::DailyActivity::new(
                    home.join("activity").join("today.jsonl"),
                    {
                        let c = clock.clone();
                        move || c.now()
                    },
                    opts.clock.zone(),
                ),
                support: opts.user_home.join("Library/Application Support"),
                browser_support: opts.browser_support.clone(),
            });
            let real: Arc<dyn supervisor::Reads> = Arc::new(supervisor::RealReads {
                host,
                seams,
                runtimes: learning_area.clone(),
                tools: tool_parts.clone(),
                native: opts.native,
            });
            let reads = match &opts.overrides {
                Some(o) => o.supervisor_reads(real),
                None => real,
            };
            let own = Arc::new(supervisor::OwnQuestions::new(opts.question_ids.clone()));
            let seam = supervisor::SupDaemon::new(supervisor::SupParts {
                gateway: gw.clone(),
                tokens: tokens.clone(),
                tools: tool_parts.clone(),
                db: db.clone(),
                clock: clock.clone(),
                ids: opts.ids.clone(),
                redactor: redactor.clone(),
                events: events.clone(),
                journal: journal.clone(),
                stop: stop.clone(),
                ledger: ledger.clone(),
                own,
                reads,
                home: home.to_string_lossy().into_owned(),
                user_home: opts.user_home.to_string_lossy().into_owned(),
                zone: opts.clock.zone(),
                a: a_cell.clone(),
                runtimes: learning_area.clone(),
            });
            let prompts = ferb_context::prompts::PromptRoot::new(&opts.package).map_err(|e| {
                StartError::Failed(format!("FERB's prompts ({}): {e}", opts.package.display()))
            })?;
            let mut parts = supervisor::BuildParts {
                db: db.clone(),
                clock: opts.clock.clone(),
                ids: opts.ids.clone(),
                redactor: redactor.clone(),
                prompts,
                device_id: device_id.clone(),
                home: home.to_string_lossy().into_owned(),
                user_home: opts.user_home.to_string_lossy().into_owned(),
                wrap_id: None,
                new_chat: None,
                shutting_down: {
                    let t = tool_parts.clone();
                    Arc::new(move || t.shutting_down())
                },
                tokenizer: ferb_context::tokens::Tokenizer::default(),
            };
            if let Some(o) = &opts.overrides {
                o.supervisor_build(&mut parts);
            }
            let _ = publisher.set(Arc::downgrade(&seam));
            let built = supervisor::build(seam, parts);
            let _ = router.0.set(built.supervisor.clone());
            built
        };
        // main.py:418-430: what was running didn't finish; the trees that won't resume are told to the owner
        let resuming: std::collections::BTreeSet<String> =
            sup.supervisor.recover_after_restart().into_iter().collect();
        // L39 M1: the records a stopped daemon left go now, except those of trees that will resume
        let stuck = tool_parts.changes().sweep(&|t: &str| resuming.contains(t));
        if !stuck.is_empty() {
            journal.note(
                "changes.cleanup_failed",
                json!({"at": "start", "paths": stuck.len()}),
            );
        }
        // group A's areas, on their loop thread
        let a = {
            let ev = events.clone();
            let deps = areas_a::Deps {
                home: home.clone(),
                user_home: opts.user_home.clone(),
                db_path: home.join("ferb.db"),
                loop_thread: None,
                prompts: opts.package.clone(),
                clock: opts.clock.clone(),
                ids: opts.ids.clone(),
                config: config.clone(),
                redactor: redactor.clone(),
                publish: Arc::new(move |topic: &str, data: Map<String, Value>| {
                    ev.publish(topic, data)
                }),
                timer_tokens: opts.timer_tokens.clone(),
                make_runtime: Some(memory_loops::learning_runtime(learning_area.clone())),
                app_bundle: opts.app_bundle.clone(),
                persona_scan: None,
                tasks: Arc::new(ferb_supervisor::flow::daemon_seams::SupervisorTasks {
                    supervisor: sup.supervisor.clone(),
                    runtime: tokio::runtime::Handle::current(),
                }),
                owner: Arc::new(areas::OwnerSide::new(&events)) as Arc<dyn Owner>,
                questions: Arc::new(ferb_supervisor::flow::daemon_seams::GatewayQuestions(
                    gw.clone(),
                )) as Arc<dyn Questions>,
                logins: Arc::new(areas::SavedLogins(owner_secrets.clone())) as Arc<dyn Logins>,
                purges: None,
            };
            let mut deps = deps;
            if let Some(o) = &opts.overrides {
                o.group_a(&mut deps);
            }
            areas_a::register(&mut server, deps).map_err(StartError::Failed)?
        };
        tool_parts.set_group_a(a.clone());
        let _ = a_cell.set(a.clone());

        // the Claude runtime's tool-list relay: 127.0.0.1, forwarding only a live run's own prefix (proxy.py). Python
        // starts it at a Claude run's first need; here at start, where it can bind (it refuses everything else)
        let claude_proxy: Option<Arc<dyn ferb_runtimes::claude::ToolProxy>> =
            match ferb_runtimes::claude::proxy::ToolListProxy::start(
                ferb_runtimes::claude::proxy::UPSTREAM,
                Arc::new(ferb_runtimes::claude::proxy::token_urlsafe),
            )
            .await
            {
                Ok(p) => Some(Arc::new(p)),
                Err(e) => {
                    journal.note(
                        "runtime.proxy_failed",
                        json!({"error": redactor.text(&e.to_string())}),
                    );
                    None
                }
            };

        // group B's areas (about reads group A's records on its thread)
        let b = areas::group_b(
            &opts,
            &areas::Shared {
                home: home.clone(),
                config: config.clone(),
                redactor: redactor.clone(),
                journal: journal.clone(),
                events: events.clone(),
                db: db.clone(),
                gateway: gw.clone(),
                tokens: tokens.clone(),
                owner_secrets: owner_secrets.clone(),
                tools: tool_parts.clone(),
                claude_proxy: claude_proxy.clone(),
                switch: Arc::new(supervisor::Switch(sup.supervisor.clone())),
            },
            a_cell,
        );
        let mut b = b;
        if let Some(o) = &opts.overrides {
            o.group_b(&mut b.deps);
            // the runtimes area as a test replaced it (its programs) is the one the start prepares
            if let Some(r) = &b.deps.runtimes {
                b.runtimes =
                    ferb_daemon::areas_b::runtimes::Runtimes::new(b.deps.host.clone(), r.clone());
            }
        }
        // about.read's document: the About area's, as FERB.app's About page reads it
        if let Some(about) = b.deps.about.clone() {
            let host = b.deps.host.clone();
            tool_parts.set_about_document(Arc::new(move || {
                ferb_daemon::areas_b::about::document(&host, &about).map_err(|e| e.message)
            }));
        }
        ferb_daemon::areas_b::register(&mut server, &b.deps);
        // browser.pair and browser.handoff run through the browser area (`_pair_for_task`, `_handoff`)
        if let (Some(br), Some(deps)) = (tool_parts.browsers(), b.deps.browsers.as_ref()) {
            br.attach_area(ferb_daemon::areas_b::browsers::Cx::new(&b.deps.host, deps));
        }
        // the runtimes area as registered (its deps after any test's replacement), for the learners
        let _ = learning_area.set(match &b.deps.runtimes {
            Some(r) => {
                ferb_daemon::areas_b::runtimes::Runtimes::new(b.deps.host.clone(), r.clone())
            }
            None => b.runtimes.clone(),
        });

        // the chat RPCs (areas/chat.py); group C's `conversations.*` and `ui.click_through`, and rpc.py's tool bridge
        // below, take the place of the supervisor crate's own
        ferb_supervisor::flow::chat_rpc::register(&server, sup.rpc.clone());

        // the records, permissions and health, logs.subscribe (areas_c)
        {
            let mut c = group_c::deps(group_c::Parts {
                opts: &opts,
                config: config.clone(),
                redactor: redactor.clone(),
                journal: journal.clone(),
                db: db.clone(),
                stop: stop.clone(),
                a: a.clone(),
                gateway: gw.clone(),
                b: &b,
                tools: tool_parts.clone(),
                bus: bus.clone(),
            });
            if let Some(o) = &opts.overrides {
                o.group_c(&mut c);
            }
            crate::areas_c::register(&server, &c);
        }

        // settings, the rest of rpc.py
        {
            let j = journal.clone();
            let r = redactor.clone();
            let c = config.clone();
            areas_a::register_settings(
                &mut server,
                config.clone(),
                Arc::new(areas::Settings {
                    journal: j,
                    redactor: r,
                    config: c,
                }),
            );
        }
        let quit = Arc::new(tokio::sync::Notify::new());
        rpc::register(
            &server,
            &rpc::Parts {
                clock: clock.clone(),
                stop: stop.clone(),
                gateway: gw.clone(),
                tools: tool_parts.clone(),
                journal: journal.clone(),
                events: events.clone(),
                tokens: tokens.clone(),
                quit: quit.clone(),
                supervisor: sup.supervisor.clone(),
            },
        );

        // DMN-510: serve before the slow part; every call but `ping` waits for the start to finish
        server.set_starting();
        server
            .start()
            .await
            .map_err(|e| StartError::Running(format!("FERB: {e}")))?;

        let daemon = Arc::new(Daemon {
            opts,
            config,
            redactor,
            db,
            journal,
            events,
            bus,
            stop,
            server,
            tokens,
            gateway: gw,
            ledger,
            supervisor: sup,
            a,
            b,
            tools: tool_parts,
            claude_proxy,
            memory_stop: std::sync::OnceLock::new(),
            resource_log: Mutex::new(resource_log),
            quit,
            loops: Mutex::new(Vec::new()),
            _lock: lock_file,
        });
        if let Some(o) = &daemon.opts.overrides {
            o.started(&daemon);
        }
        if let Err(e) = daemon.finish_start().await {
            daemon.journal.note(
                "daemon.start_failed",
                json!({"error": daemon.redactor.text(&e)}),
            );
            daemon.shutdown().await;
            return Err(StartError::Failed(e));
        }
        Ok(daemon)
    }

    /// The slow part of `_start` after the socket serves, then the start is done (`server.ready.set()`).
    async fn finish_start(self: &Arc<Self>) -> Result<(), String> {
        if self.opts.preload_secrets {
            self.b.preload_secrets(&self.journal, &self.redactor).await;
        }
        self.b.prepare_codex().await;
        let pid = std::process::id();
        let login = if self.opts.native {
            login_session()
        } else {
            None
        };
        let _new_session =
            self.a
                .call(move |st| async move {
                    areas_a::schedules::record_start(&st, pid, login.as_deref())
                })
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.message)?;
        // L28 (main.py:460-468): an unknown window is said, never guessed
        if let Ok(limits) = self.b.runtimes.runtime_limits() {
            for (role, entry) in limits {
                let Value::Object(entry) = entry else {
                    continue;
                };
                let Some(problem) = entry.get("problem").map(ferb_py::value::py_str) else {
                    continue;
                };
                let mut row = Map::new();
                row.insert("role".into(), json!(role));
                row.extend(entry.clone());
                self.journal
                    .note("runtime.limits_unknown", Value::Object(row));
                let runtime = entry
                    .get("runtime")
                    .map(ferb_py::value::py_str)
                    .unwrap_or_default();
                eprintln!(
                    "FERB: the {role} runtime {runtime} has no documented context window: {}",
                    self.redactor.text(&problem)
                );
            }
        }
        // main.py:466-468: every intent a restart cut off is of unknown outcome (journaled)
        self.supervisor.supervisor.mark_unknown_after_restart();
        // a stop outlives a crash or a restart until the owner lifts it
        if self.stop.marker.exists() {
            self.stop.mark_engaged();
            self.gateway.set_stopped(true);
            self.journal.note("stop.still_engaged", json!({}));
        }
        let engaged = self.stop.engaged();
        self.a
            .call(move |st| async move { st.stopped.set(engaged) })
            .await
            .ok();
        // group A's start: memory, the schedules' timers, then the timer loop
        self.a
            .call(|st| async move {
                let _ = areas_a::memory::start(&st);
                let _ = areas_a::schedules::reconcile_schedules(&st);
                areas_a::memory_import::resume(&st);
            })
            .await
            .map_err(|e| e.to_string())?;
        // DMN-520: Vision's text recognizer is loaded once at start, in the background, so the first image redaction
        // doesn't pay for it (on this Mac its first load took 50-100 s in Python and in Rust alike)
        if self.opts.mac {
            // a plain thread: a shutdown never waits for it
            let _ = std::thread::Builder::new()
                .name("ferb-vision-warm".into())
                .spawn(ferb_mac_hands::native::vision::warm_recognition);
        }
        loops::start(self);
        // main.py:472: the trees a shutdown or crash cut off run again in place, in the background
        self.supervisor.supervisor.start_resume();
        self.server.set_ready();
        Ok(())
    }

    /// The tools' parts (the gateway's registry, the shell, the browsers).
    pub fn b_tools(&self) -> tools::Parts {
        self.tools.clone()
    }

    /// Keeps a background loop for shutdown.
    pub fn keep(&self, job: JoinHandle<()>) {
        lock(&self.loops).push(job);
    }

    /// `shutdown` (main.py:495): producers first, then what they started, then the server and the store.
    pub async fn shutdown(&self) {
        self.tools.set_shutting_down();
        let _ = self
            .a
            .call(|st| async move {
                st.shutting_down.set(true);
                areas_a::memory::settle_forgets(&st).await;
            })
            .await;
        // main.py:522-524: the supervisor suspends its trees (they resume after the restart), cancels its runs and
        // drains every chat turn
        self.supervisor.supervisor.shut_down().await;
        self.supervisor.daemon.reads.shutdown();
        for job in lock(&self.loops).drain(..) {
            job.abort();
        }
        if let Some(s) = self.memory_stop.get() {
            s.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        self.b.shutdown().await;
        self.opts.egress.close();
        self.server.close().await;
        // the minutes since the last sample (L30)
        self.sample_resources();
        self.journal
            .note("daemon.stopped", json!({"pid": std::process::id()}));
    }
}

/// The resource log's clock: the daemon's, its day the Mac's local one (`now.astimezone().date()`).
struct SampleClock(Arc<dyn ZonedClock>);

impl crate::resources::SampleClock for SampleClock {
    fn now_iso(&self) -> String {
        ferb_core::when::isoformat(&self.0.now())
    }
    fn local_day(&self) -> String {
        self.0
            .zone()
            .local(&self.0.now())
            .format("%Y-%m-%d")
            .to_string()
    }
    fn monotonic(&self) -> f64 {
        self.0.monotonic()
    }
}

impl Daemon {
    /// `_sample_resources`: one sample; a failure is journaled and the next sample covers this window too.
    pub fn sample_resources(&self) {
        let mut held = lock(&self.resource_log);
        let Some(log) = held.as_mut() else { return };
        let done = {
            let d = lock(&self.db);
            log.sample(&d)
        };
        if let Err(e) = done {
            self.journal.note("resources.failed", json!({"error": e}));
        }
    }
}

/// `_apply_privacy`: [privacy] exclude_domains / exclude_apps into the one Redactor everything holds.
pub fn apply_privacy(config: &Config, redactor: &Redactor) {
    let domains: Vec<String> = match config.get("privacy", "exclude_domains") {
        Ok(Some(Value::Array(a))) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    };
    let apps: Vec<String> = config
        .exclude_apps()
        .map(|s| s.into_iter().collect())
        .unwrap_or_default();
    let d: Vec<&str> = domains.iter().map(String::as_str).collect();
    let a: Vec<&str> = apps.iter().map(String::as_str).collect();
    redactor.configure(&d, &a);
}

/// `_new_protection_defaults` (main.py:357): a never-read default config.toml was never offered is added once and
/// journaled with every default offered so far.
fn new_protection_defaults(config: &Config, journal: &Journal) {
    use ferb_policy::protection as p;
    let mut offered: std::collections::BTreeSet<String> = p::never_read_offered()
        .into_iter()
        .map(String::from)
        .collect();
    for (_, data) in journal.of_kind(p::NEW_DEFAULTS, "key", "protection.never_read") {
        if let Some(a) = data.get("offered").and_then(Value::as_array) {
            offered.extend(a.iter().filter_map(Value::as_str).map(String::from));
        }
    }
    let new: Vec<&str> = p::default_never_read()
        .into_iter()
        .filter(|e| !offered.contains(*e))
        .collect();
    if new.is_empty() {
        return;
    }
    let added = config
        .add_defaults("protection", "never_read", &new)
        .unwrap_or_default();
    journal.note(
        p::NEW_DEFAULTS,
        json!({"key": "protection.never_read", "added": added, "offered": p::default_never_read()}),
    );
}

/// main.py:406-409: chat continuity written by builds before 2026-10-03 upgraded once, journaled (step 8c); it also
/// makes the owner's daily conversation.
fn legacy_continuity(db: &SharedDb, journal: &Journal, redactor: &Arc<Redactor>) {
    use ferb_orchestrator::continuity::{upgrade_legacy, Continuity, LEGACY_UPGRADE};
    if !journal
        .of_kind("store.upgraded", "upgrade", LEGACY_UPGRADE)
        .is_empty()
    {
        return;
    }
    let changes = {
        let d = lock(db);
        let r = redactor.clone();
        let c = Continuity::new(
            &d,
            Arc::new(move |v: &Value| r.value(v)),
            Arc::new(|_: &str| std::collections::BTreeSet::new()),
            Arc::new(ferb_learning::journal::journal_origin),
            Arc::new(|| ferb_runtimes::codex::token_hex(6)),
        );
        upgrade_legacy(&c)
    };
    match changes {
        Ok(changes) => journal.note(
            "store.upgraded",
            json!({"upgrade": LEGACY_UPGRADE, "changes": changes}),
        ),
        Err(e) => journal.note(
            "store.upgrade_failed",
            json!({"upgrade": LEGACY_UPGRADE, "error": e.to_string()}),
        ),
    }
}

/// Why config.toml's text isn't TOML (the parser's words), for the salvage check.
fn toml_parse_error(text: &str) -> Option<String> {
    ferb_core::config::Document::parse(text)
        .err()
        .map(|e| e.to_string())
}

/// `_login_session` (schedules.py:59): a digest of this login session's unique id (CoreGraphics), None when it can't
/// be read. Not a secret: it only tells one login session from another.
fn login_session() -> Option<String> {
    use sha2::Digest;
    let id = ferb_sense::mac::system::session().id?;
    let digest = sha2::Sha256::digest(id.as_bytes());
    Some(
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()[..16]
            .to_string(),
    )
}
