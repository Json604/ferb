//! The composition root (`ferb/daemon/main.py`, `areas/compose.py`): every engine crate built from FERB's home, each
//! crate's seams filled with the others, the RPC handlers registered, and the start order, the stop marker and the
//! clean shutdown Python's daemon has.
//!
//! [`Daemon::start`] is Python's `start` + `_start`; [`Daemon::shutdown`] its `shutdown`. What a test or a stripped
//! run changes comes in through [`Options`] (Python's constructor flags), never through a global.

pub mod areas;
pub mod checks;
pub mod consumers;
pub mod desktop;
pub mod desktop_seams;
pub mod device;
pub mod egress;
pub mod env_seams;
pub mod front;
pub mod gateway;
pub mod group_c;
pub mod guard;
pub mod insomnia;
pub mod link;
pub mod loops;
pub mod mail_check;
pub mod memory_loops;
pub mod phone;
pub mod presence;
pub mod previews;
pub mod rpc;
pub mod runtimes;
pub mod supervisor;
pub mod tools;

use std::path::{Path, PathBuf};
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
    /// Each run token's text (`secrets.token_urlsafe(32)`); None: the system's random bytes. Only a test sets it.
    pub run_tokens: Option<Arc<ferb_rpc::auth::TokenSource>>,
    /// Where the chat entries' trace goes (ferb-supervisor `ChatLog::set_trace`); None: none is kept. Only a test sets
    /// it (the stand-ins' `chat_trace`).
    pub chat_trace: Option<ferb_supervisor::flow::chat_log::Trace>,
    /// How a failed background loop waits before it starts again (compose/guard.rs; a test's recorder).
    pub pause: Arc<dyn guard::Pause>,
    /// The phone link's helper (stage 3 S4, compose/link.rs).
    pub link: LinkOptions,
    /// Where a temporary chat's files from a phone are moved at send (ORCH-717): ferb-core's `FILES_FOLDER` in the
    /// account's own temporary folder; the supervisor removes it when the chat ends and at every start. A test daemon's
    /// is in its own home.
    pub temporary_files: PathBuf,
}

/// How this daemon runs the phone link's helper (compose/link.rs).
#[derive(Clone)]
pub struct LinkOptions {
    /// The helper program: FERB.app's `Contents/Helpers/ferb-link`, a checkout's beside `ferbd`, a test's own; None:
    /// this build has none and the phone feature can't run.
    pub program: Option<PathBuf>,
    /// Under its sandbox profile (the real daemon); a test daemon runs inside the test guard's, and sandboxes don't nest.
    pub sandbox: bool,
    /// More arguments for the helper (a test's relays, `--no-portmap`).
    pub args: Vec<String>,
    /// Where its one Keychain item is kept; None: through the secrets helper, as the daemon's `keychain` says.
    pub keys: Option<Arc<dyn link::LinkKeys>>,
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
    /// The owner's banners' desk (osascript, presence): a test's recorder in place of the Mac's.
    fn desk(&self) -> Option<Arc<dyn ferb_daemon::areas_b::notify::Desk>> {
        None
    }
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
            run_tokens: None,
            chat_trace: None,
            pause: Arc::new(guard::RealPause),
            link: LinkOptions {
                program: link::program(crate::home::app_bundle().as_deref()),
                sandbox: true,
                args: Vec::new(),
                keys: None,
            },
            temporary_files: std::env::temp_dir().join(ferb_core::temporary::FILES_FOLDER),
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
    /// A problem that keeps happening, journaled bounded (`_repeats`); flushed at shutdown.
    pub repeats: ferb_daemon::shared::Repeats,
    /// Set by SIGTERM, SIGINT or `update.prepare` (`_stop`).
    pub quit: Arc<tokio::sync::Notify>,
    /// The phone area (PHN-585: the hourly pass gives the 30-day unused warning).
    pub phones: ferb_daemon::phone::Phones,
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
        let pause = |wait: std::time::Duration| std::thread::sleep(wait);
        // decision: STORE-431: full-text indexes rebuilt in place, journaled once the journal is open
        let rebuilt: Mutex<Vec<Value>> = Mutex::default();
        let told_rebuilt = |tables: &[String], why: &str| {
            lock(&rebuilt).push(serde_json::json!({"tables": tables, "why": why}));
        };
        let set_aside = ferb_store::salvage::check_home(
            &home,
            &build,
            &ferb_store::salvage::Seams {
                now: &now,
                toml_error: &toml_error,
                pause: &pause,
                rebuilt: &told_rebuilt,
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
        crate::log::set_redactor(redactor.clone());

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
        let rebuilt_entries = std::mem::take(&mut *lock(&rebuilt));
        for entry in rebuilt_entries {
            crate::log::line(&format!(
                "store: search indexes rebuilt in place {}",
                ferb_py::json::dumps(&entry)
            ));
            journal.note("store.index_rebuilt", entry);
        }
        // `_repeats` (main.py:279): a problem that keeps happening, journaled bounded; each row also one log line
        let repeats = ferb_daemon::shared::Repeats::new(
            {
                let j = journal.clone();
                move |kind: &str, data: Map<String, Value>| {
                    crate::log::line(&format!(
                        "{kind} {}",
                        ferb_py::json::dumps(&Value::Object(data.clone()))
                    ));
                    j.note(kind, Value::Object(data));
                }
            },
            clock.clone(),
        );
        let _ = areas::HELPER_FAILED.set(repeats.clone());
        // a panic is journaled `daemon.unhandled` (Python's `_loop_exception`): handed over a channel, so a panic on
        // a thread that holds the store's lock never waits for it
        {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Map<String, Value>>();
            crate::log::set_unhandled(move |row| {
                let _ = tx.send(row);
            });
            let r = repeats.clone();
            // supervised (T-1123); the channel outlives a failure
            let rx = Arc::new(tokio::sync::Mutex::new(rx));
            let g = guard::Guard::of(&journal, &redactor, &opts.pause);
            tokio::spawn(async move {
                g.run("unhandled", |_| {
                    let (rx, r) = (rx.clone(), r.clone());
                    async move {
                        let mut rx = rx.lock().await;
                        while let Some(row) = rx.recv().await {
                            let last = row
                                .get("frames")
                                .and_then(Value::as_array)
                                .and_then(|f| f.last())
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            r.record(
                                ferb_daemon::shared::UNHANDLED,
                                &[ferb_daemon::shared::UNHANDLED, "panic", &last],
                                row,
                            );
                        }
                        Ok(())
                    }
                })
                .await
            });
        }
        // the resource log: its first window starts here, so start-up's own work is in it (L30); the previews'
        // counts join it once the tools are built (`hooks.previews.snapshot()`, main.py:399)
        let preview_counts: Arc<
            std::sync::OnceLock<ferb_daemon::areas_b::task_preview::TaskPreviews>,
        > = Arc::new(std::sync::OnceLock::new());
        // perception's batcher counts (`_perception_counts`), once sensing builds perception
        let perception_counts: Arc<std::sync::OnceLock<loops::Perception>> =
            Arc::new(std::sync::OnceLock::new());
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
                    {
                        let pc = perception_counts.clone();
                        Box::new(move || pc.get().map(|p| p.counts()).unwrap_or_default())
                    },
                    {
                        let counts = preview_counts.clone();
                        Box::new(move || counts.get().map(|p| p.snapshot()).unwrap_or_default())
                    },
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
        // the consumers Python runs before the bus (compose.py `_wire_events`: plans, the day's activity, conversations)
        let first = consumers::wire_first(&events);
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
        retired_sessions(&home, &opts.user_home, &config, &journal);

        let ledger = Arc::new(gateway::RealLedger {
            db: db.clone(),
            clock: clock.clone(),
            ids: opts.ids.clone(),
        });

        // the server and its run tokens
        let rpc_clock = Arc::new(RpcClock(opts.clock.clone()));
        let tokens = Arc::new(match &opts.run_tokens {
            Some(source) => RunTokens::with_source(rpc_clock, source.clone()),
            None => RunTokens::new(rpc_clock),
        });
        let mut server = match opts.owner_uid {
            Some(uid) => Server::with_owner_uid(&socket, tokens.clone(), uid),
            None => Server::new(&socket, tokens.clone()),
        };
        match opts.app_check {
            Some(answer) => server.set_is_app(move |_| answer),
            None => {
                ferb_platform_macos::rpc::attach(&server, &home);
                // DMN-402: the same check, each verdict also a log line (the first FERB.app peer accepted, every one
                // refused), so "FERB isn't running" can be told apart from FERB.app's calls being refused
                let check = ferb_platform_macos::rpc::system(&home).into_check();
                let seen_app = std::sync::atomic::AtomicBool::new(false);
                server.set_is_app(move |token| {
                    let pid = token.map(|t| u32::from_ne_bytes([t[20], t[21], t[22], t[23]]));
                    let verdict = check(token);
                    if !verdict {
                        crate::log::line(&format!(
                            "a FERB.app-only call was refused: the caller (pid {}) isn't this FERB.app",
                            pid.map_or("unknown".into(), |p| p.to_string())
                        ));
                    } else if !seen_app.swap(true, std::sync::atomic::Ordering::SeqCst) {
                        crate::log::line(&format!(
                            "FERB.app (pid {}) is answered as FERB.app",
                            pid.map_or("unknown".into(), |p| p.to_string())
                        ));
                    }
                    verdict
                });
            }
        }
        {
            let j = journal.clone();
            server.on_internal(move |method, e| {
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
        // GATE-602: the identifiers the owner let the AI see, read from the Keychain off the start-up path; until
        // they are read they stay masked
        {
            let (store, r) = (owner_secrets.clone(), redactor.clone());
            let g = guard::Guard::of(&journal, &redactor, &opts.pause);
            let _ = guard::thread_once(
                g,
                "ferb-shown-identifiers",
                "shown identifiers",
                move || {
                    if let Ok(values) = store.shown_values() {
                        if !values.is_empty() {
                            r.show_identifiers(values.iter().map(|v| v.expose()));
                        }
                    }
                    Ok(())
                },
            );
        }
        let tool_parts = tools::Parts::new(&opts, &config, &redactor, &db, &journal, &events);
        tool_parts.set_owner_secrets(owner_secrets.clone());
        let _ = preview_counts.set(tool_parts.previews().clone());
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
                    roots: gateway::fs_roots(&config, &home),
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
                    owner_facts: gateway::OwnerFacts(tool_parts.downgrade()),
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
        // HAND-406: every native-host connection or extension message FERB doesn't take is journaled
        tools::browser::wire_refusals(&tool_parts, &server, &repeats);

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
                dev_tools: std::sync::Mutex::new(Vec::new()),
            });
            // decision: ORCH-756 — the tool core is off in FERB.app (every tool in full) until a real-model replay
            // shows workers still find and call what they need; the stand-ins' daemon (tests) has it on
            gw.set_tool_core(opts.overrides.is_some());
            let real: Arc<dyn supervisor::Reads> = Arc::new(supervisor::RealReads {
                host,
                seams: seams.clone(),
                runtimes: learning_area.clone(),
                tools: tool_parts.clone(),
                native: opts.native,
            });
            let reads = match &opts.overrides {
                Some(o) => o.supervisor_reads(real),
                None => real,
            };
            // the inventory walk's protection check (`roots.protection`), the same lists the gateway was given
            let sup_protection = Arc::new(ferb_gateway::gateway::SyncProtection(
                gateway::protection(&config, &opts.user_home, &home, opts.app_bundle.as_deref())
                    .map_err(|e| StartError::Failed(format!("FERB's protection lists: {e}")))?,
            ));
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
                env: seams,
                protection: sup_protection,
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
                notice_hex: None,
                shutting_down: {
                    let t = tool_parts.clone();
                    Arc::new(move || t.shutting_down())
                },
                tokenizer: ferb_context::tokens::Tokenizer::default(),
                temporary_files: opts.temporary_files.clone(),
            };
            if let Some(o) = &opts.overrides {
                o.supervisor_build(&mut parts);
            }
            let _ = publisher.set(Arc::downgrade(&seam));
            let _ = first.daemon.set(Arc::downgrade(&seam));
            let built = supervisor::build(seam, parts);
            let _ = router.0.set(built.supervisor.clone());
            built
        };
        // main.py:418-430: what was running didn't finish; the trees that won't resume are told to the owner
        let resuming: std::collections::BTreeSet<String> =
            sup.supervisor.recover_after_restart().into_iter().collect();
        // stage 2: the chat entries' journal backstop and start-up pass, before anything is served (chat_log.rs)
        if let Some(trace) = &opts.chat_trace {
            sup.supervisor.inner().chat_log.set_trace(trace.clone());
        }
        sup.supervisor.chat_entries_after_restart(&resuming);
        // L39 M1: the records a stopped daemon left go now, except those of trees that will resume
        let stuck = tool_parts.changes().sweep(&|t: &str| resuming.contains(t));
        if !stuck.is_empty() {
            journal.note(
                "changes.cleanup_failed",
                json!({"at": "start", "paths": stuck.len()}),
            );
        }
        // MEM-456: the memory import's live card as group A last published it, for the phones' `chat.status`
        let import_card: Arc<std::sync::Mutex<Option<Value>>> = Arc::default();
        // group A's areas, on their loop thread
        let a = {
            let ev = events.clone();
            let card = import_card.clone();
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
                    if topic == areas_a::memory_import::LIVE_TOPIC {
                        *card.lock().unwrap_or_else(|e| e.into_inner()) =
                            data.get("card").filter(|c| c.is_object()).cloned();
                    }
                    ev.publish(topic, data)
                }),
                timer_tokens: opts.timer_tokens.clone(),
                make_runtime: Some(memory_loops::learning_runtime(learning_area.clone())),
                app_bundle: opts.app_bundle.clone(),
                // `discover_persona` on the Mac (main.py:105): a test daemon (no Mac parts) has none, as Python's
                // recorder (`setup.persona` answers unavailable)
                persona_scan: opts
                    .native
                    .then(|| crate::persona::scan(&opts.user_home, &opts.browser_support)),
                tasks: Arc::new(ferb_supervisor::flow::daemon_seams::SupervisorTasks {
                    supervisor: sup.supervisor.clone(),
                    runtime: tokio::runtime::Handle::current(),
                }),
                owner: Arc::new(areas::OwnerSide::new(&events, sup.supervisor.clone()))
                    as Arc<dyn Owner>,
                questions: Arc::new(ferb_supervisor::flow::daemon_seams::GatewayQuestions(
                    gw.clone(),
                )) as Arc<dyn Questions>,
                logins: Arc::new(areas::SavedLogins(owner_secrets.clone())) as Arc<dyn Logins>,
                // the retention pass's other stores (the downloads cap's Yes deletes through them)
                purges: Some(loops::purges(&opts, &tool_parts, &redactor)),
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
        // the rest of compose.py `_wire_events` (the supervisor's seam registered its three), and the browsers' hooks
        // and drivers' seams (areas/browsers.py `_build_browsers`, areas/sensing.py:61-64), now that every part exists
        {
            let desktop = Arc::new(desktop_seams::DesktopSeams {
                parts: tool_parts.clone(),
                tokens: tokens.clone(),
            });
            consumers::wire(
                &events,
                &first,
                b.notifier.clone(),
                sup.supervisor.clone(),
                sup.daemon.env.clone(),
                b.deps.host.clone(),
                device_id.clone(),
                desktop.clone(),
            );
            let helper = matches!(opts.keychain, Keychain::Real)
                .then(|| Arc::new(areas::helper(&opts, &home)));
            tools::browser::wire_parts(&tool_parts, desktop, helper);
            // stage 2: the chat's entries read the bus last, after every consumer above has done its own work (a
            // question's record among them), so queuing adds nothing before them (ferb-supervisor's chat_log.rs)
            let chat = sup.supervisor.clone();
            events.events.on(
                Some(&ferb_supervisor::flow::chat_log::TOPICS),
                Arc::new(move |topic: &str, data: Map<String, Value>| {
                    chat.chat_bus(topic, Value::Object(data));
                }),
            );
        }
        // about.read's document: the About area's, as FERB.app's About page reads it
        if let Some(about) = b.deps.about.clone() {
            let host = b.deps.host.clone();
            // about.diagnostics' permissions (DMN-595): the About area's rows, as about.read's `permissions`
            let grants = about.clone();
            tool_parts.set_diagnostics_permissions(Arc::new(move || {
                ferb_mac_hands::permissions::rows(&(grants.grants)(), &(grants.containment)(), &[])
                    .iter()
                    .map(|r| serde_json::json!({"id": r["id"], "status": r["status"]}))
                    .collect()
            }));
            tool_parts.set_about_document(Arc::new(move || {
                ferb_daemon::areas_b::about::document(&host, &about).map_err(|e| e.message)
            }));
        }
        ferb_daemon::areas_b::register(&mut server, &b.deps);
        // browser.pair and browser.handoff run through the browser area (`_pair_for_task`, `_handoff`)
        if let (Some(br), Some(deps)) = (tool_parts.browsers(), b.deps.browsers.as_ref()) {
            let cx = ferb_daemon::areas_b::browsers::Cx::new(&b.deps.host, deps);
            br.attach_area(cx.clone());
            // the owner's answer to FERB's browsing question (APP-777) is the browser area's
            let hook: areas_a::BrowsingAnswered = Arc::new(move |state, choice, ident| {
                ferb_daemon::areas_b::browsers::browsing_answered(&cx, state, choice, ident)
                    .unwrap_or_default()
            });
            // decision: MEM-476 — a website watch's code check of the export mail, in the daemon's runtime (the tab and
            // the page's markup through the owner's router), never a model
            let (hub, router) = (br.hub.clone(), br.router.clone());
            let runtime = tokio::runtime::Handle::current();
            let mail: areas_a::MailCheck = Arc::new(move |q| {
                let job = runtime.spawn(mail_check::check(hub.clone(), router.clone(), q));
                Box::pin(async move {
                    job.await.unwrap_or_else(|e| {
                        areas_a::MailSeen::Unchecked(format!("the check stopped: {e}"))
                    })
                })
            });
            let _ = a.spawn(move |st| async move {
                *st.browsing_answered.borrow_mut() = Some(hook);
                *st.mail_check.borrow_mut() = Some(mail);
            });
        }
        if let Some(p) = tool_parts.perception() {
            let _ = perception_counts.set(p);
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

        // the records, permissions and health, logs.subscribe (areas_c); the phone area comes later (PHN-454's check)
        let phone_slot: Arc<std::sync::OnceLock<ferb_daemon::phone::Phones>> =
            Arc::new(std::sync::OnceLock::new());
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
            c.phones = phone_slot.clone();
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

        // APP-1138, PHN-828: FERB.app's sign-in to the owner's FERB account, kept in the Keychain through FERB's signed
        // helper (the runtimes area's, as the API keys are); the phone area matches each pairing against it
        let account = ferb_daemon::account::Account::new(ferb_daemon::account::Seams {
            helper: b.runtimes.deps.helper.clone(),
            redactor: redactor.clone(),
            journal: {
                let j = journal.clone();
                Arc::new(move |kind: &str, data: Value| j.note(kind, data))
            },
        });
        account.register(&server);

        // stage 3 S3 (T-1202): the phone area and the phone link helper's check
        let phones = phone::register(
            &server,
            phone::Parts {
                // PHN-828: the site's keys this build pins (none until the owner makes the pair) and the Mac's account
                account: ferb_daemon::phone::account::AccountCheck::pinned({
                    let a = account.clone();
                    Arc::new(move || a.mac_account_read())
                }),
                db: db.clone(),
                clock: clock.clone(),
                redactor: redactor.clone(),
                gateway: gw.clone(),
                journal: journal.clone(),
                events: events.clone(),
                home: home.clone(),
                real_peers: opts.app_check.is_none(),
                native: opts.native,
                // stage 3 S3b (T-1203): the phone's files, judged by the lists the gateway was given
                protection: Arc::new(ferb_gateway::gateway::SyncProtection(
                    gateway::protection(
                        &config,
                        &opts.user_home,
                        &home,
                        opts.app_bundle.as_deref(),
                    )
                    .map_err(|e| StartError::Failed(format!("FERB's protection lists: {e}")))?,
                )),
                user_home: opts.user_home.clone(),
                config: config.clone(),
                temporary_files: opts.temporary_files.clone(),
            },
        );
        let _ = phone_slot.set(phones.clone());
        // PHN-828: an account change removes the phones of another account
        account.set_phones(phones.clone());
        phones.set_memory_import(Arc::new(move || {
            import_card
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }));
        // GATE-615: what the gateway sees of the owner at the Mac, for the phone's "until I'm back at my Mac" (the Mac's
        // parts off: never offered)
        if tool_parts.owner().is_some() {
            presence::OwnerAtMac::wire(&tool_parts);
            gw.set_at_mac(Arc::new(presence::OwnerAtMac(tool_parts.clone())));
        }
        // T-1214: the paired phones' own records in the facts line and device.facts
        tool_parts.device().set_phones(phones.clone());
        // T-1274: insomnia mode (APP-980..APP-984): its RPCs; its status for the phones' `chat.status`; its one Yes for the
        // gateway's screen-use question; its cover for the Mac hands' foreground steps
        let ins = insomnia::build(
            &server,
            insomnia::Parts {
                db: db.clone(),
                journal: journal.clone(),
                events: events.clone(),
                clock: clock.clone(),
                ids: opts.ids.clone(),
                config: config.clone(),
                notifier: b.notifier.clone(),
                native: opts.native,
            },
        );
        {
            let i = ins.clone();
            phones.set_insomnia(Arc::new(move || i.status()));
            let i = ins.clone();
            gw.set_standing_screen(Arc::new(move || i.screen_yes_since()));
        }
        tool_parts.set_insomnia(ins);

        // DMN-510: serve before the slow part; every call but `ping` waits for the start to finish
        server.set_starting();
        server
            .start()
            .await
            .map_err(|e| StartError::Running(format!("FERB: {e}")))?;
        crate::log::line(&format!(
            "serving on its socket {} ms after the process started (only ping until the start is done)",
            crate::log::since_start_ms()
        ));

        // stage 3 S4 (T-1204): the phone link's helper, run while the phone feature is on
        let link_runner = link::Runner::new(
            opts.link.program.clone(),
            home.clone(),
            opts.user_home.clone(),
            ferb_core::paths::socket_path(&home),
            config.clone(),
            opts.link.keys.clone().unwrap_or_else(|| {
                Arc::new(link::HelperKeys(areas::helper(&opts, &home))) as Arc<dyn link::LinkKeys>
            }),
            redactor.clone(),
            opts.link.sandbox,
            opts.link.args.clone(),
        );
        link::attach(&phones, &link_runner);

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
            repeats,
            quit,
            phones: phones.clone(),
            loops: Mutex::new(Vec::new()),
            _lock: lock_file,
        });
        if let Some(o) = &daemon.opts.overrides {
            o.started(&daemon);
        }
        link_runner.start(&daemon);
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
        let step = |what: &str, began: std::time::Instant| {
            crate::log::line(&format!(
                "start: {what} took {} ms",
                began.elapsed().as_millis()
            ))
        };
        if self.opts.preload_secrets {
            let began = std::time::Instant::now();
            self.b.preload_secrets(&self.journal, &self.redactor).await;
            step("FERB's API keys for the redactor", began);
        }
        // DMN-511 (main.py:453): the developer tools a command would reach only as Apple's stub, read once
        if self.opts.native {
            let env = self.supervisor.daemon.env.clone();
            let _ = tokio::task::spawn_blocking(move || env.read_dev_tools()).await;
        }
        let began = std::time::Instant::now();
        self.b.prepare_codex().await;
        step("Codex's catalog and version check", began);
        let pid = std::process::id();
        let login = if self.opts.native {
            login_session()
        } else {
            None
        };
        let new_session =
            self.a
                .call(move |st| async move {
                    areas_a::schedules::record_start(&st, pid, login.as_deref())
                })
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.message)?;
        // DMN-514 (main.py:455): started by launchd under FERB's label, an edited LaunchAgent is put back
        {
            let under_launchd = std::os::unix::process::parent_id() == 1
                && std::env::var("XPC_SERVICE_NAME").ok().as_deref()
                    == Some(ferb_device::install::LABEL);
            let (home, journal) = (self.opts.home.clone(), self.journal.clone());
            let _ = tokio::task::spawn_blocking(move || {
                if under_launchd {
                    restore_service_definition(
                        &|kind, data| journal.note(kind, data),
                        &crate::setup_env::installer(&home),
                    );
                }
            })
            .await;
        }
        // DMN-10 (main.py:456): what was set aside because it couldn't be read is told once
        self.b.notifier.tell_set_aside().await;
        self.mac_started(new_session);
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
        // main.py:435 `_sweep_stale_runs` (review M1): Codex run folders a crash left in the temporary folder, each
        // holding a copy of FERB's Codex login, removed at start; a test daemon leaves the real temporary folder alone
        // unless its stand-ins turn retention on. Ported but never called until the audit of 2026-11-16.
        if self.opts.retention {
            if let Err(e) = self.b.runtimes.sweep_stale_runs().await {
                crate::log::line(&format!("the stale run folders weren't swept: {e:?}"));
            }
        }
        // DMN-520: Vision's text recognizer is loaded once at start, in the background, so the first image redaction
        // doesn't pay for it (on this Mac its first load took 50-100 s in Python and in Rust alike)
        if self.opts.mac {
            // a plain thread: a shutdown never waits for it
            let _ = guard::thread_once(self.guard(), "ferb-vision-warm", "vision warm-up", || {
                ferb_mac_hands::native::vision::warm_recognition();
                Ok(())
            });
        }
        // main.py:478 `_with_full_disk_access` (DMN-507, DMN-508): once FERB has Full Disk Access, FERB's files in
        // every installed browser, and the browser part of what FERB saw on this Mac when that was gathered before
        // access; without it, nothing. In the background: the start doesn't wait for it.
        if self.opts.native {
            let cx = self.tools.browsers().and_then(|b| b.area());
            let a = self.a.clone();
            // `_spawn(..., "browser files")`: a failure is journaled `daemon.background_failed`
            self.spawn_background("browser files", async move {
                let Some(cx) = cx else { return Ok(()) };
                let probe = cx.clone();
                let readable = tokio::task::spawn_blocking(move || probe.other_app_data())
                    .await
                    .unwrap_or(false);
                if !readable {
                    return Ok(());
                }
                cx.connect_browsers()
                    .await
                    .map_err(|e| format!("{}: {}", e.type_name, e.message))?;
                let _ = a
                    .call(|st| async move {
                        // `persona_scan is not None and needs_browser_part()`: refresh reads the scan's presence
                        let needs = st.persona.borrow().needs_browser_part();
                        if needs {
                            let _ = areas_a::persona::refresh(&st).await;
                        }
                    })
                    .await;
                Ok(())
            });
        }
        // main.py:477 `self._spawn(self._fresh.prime(self._permissions), "permission check")`: the planner's first
        // turn won't wait for the TCC reads (T-1124); none without the Mac's parts
        if self.opts.native {
            let fresh = self.tools.fresh();
            self.spawn_background("permission check", async move {
                tokio::task::spawn_blocking(move || {
                    fresh.prime(env_seams::PERMISSIONS, "", env_seams::read_permissions)
                })
                .await
                .map_err(|e| format!("JoinError: {e}"))
            });
        }
        loops::start(self);
        // main.py:472: the trees a shutdown or crash cut off run again in place, in the background
        self.supervisor.supervisor.start_resume();
        self.server.set_ready();
        crate::log::line(&format!(
            "started: every call answered {} ms after the process started",
            crate::log::since_start_ms()
        ));
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

    /// The guard over this daemon's journal (compose/guard.rs, `_supervise` and `_spawn`).
    pub fn guard(&self) -> guard::Guard {
        guard::Guard::of(&self.journal, &self.redactor, &self.opts.pause)
    }

    /// A loop for the daemon's life on the shared runtime (`_supervise`): a failure journaled and the loop started
    /// again; aborted at shutdown. `body` is told how many times it failed before.
    pub fn spawn_loop<F, Fut>(&self, name: &'static str, body: F)
    where
        F: FnMut(u32) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        let g = self.guard();
        self.keep(tokio::spawn(async move { g.run(name, body).await }));
    }

    /// One-off work in the background (`_spawn`): a failure journaled; aborted at shutdown.
    pub fn spawn_background<Fut>(&self, what: &'static str, work: Fut)
    where
        Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        let g = self.guard();
        self.keep(tokio::spawn(async move { g.background(what, work).await }));
    }

    /// A loop for the daemon's life on group A's thread, where its state lives (`_supervise`).
    pub fn spawn_group_a_loop<F, Fut>(&self, name: &'static str, body: F)
    where
        F: FnMut(std::rc::Rc<AState>, u32) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), String>> + 'static,
    {
        let g = self.guard();
        let _ = self.a.spawn(move |st| async move {
            let mut body = body;
            g.run(name, |failures| body(st.clone(), failures)).await
        });
    }

    /// `shutdown` (main.py:495): producers first, then what they started, then the server and the store. Each step
    /// that takes longer than a moment is a log line, so a slow stop says what it waited on.
    pub async fn shutdown(&self) {
        let step = |what: &str, began: std::time::Instant| {
            let ms = began.elapsed().as_millis();
            if ms >= 100 {
                crate::log::line(&format!("stop: {what} took {ms} ms"));
            }
        };
        self.tools.set_shutting_down();
        let began = std::time::Instant::now();
        let _ = self
            .a
            .call(|st| async move {
                st.shutting_down.set(true);
                areas_a::memory::settle_forgets(&st).await;
            })
            .await;
        step("the forgets settling", began);
        // main.py:517: the producers (timers, retention, held banners, resources, memory) stop first
        for job in lock(&self.loops).drain(..) {
            job.abort();
        }
        if let Some(s) = self.memory_stop.get() {
            s.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        // main.py:522-524: the supervisor suspends its trees (they resume after the restart), cancels its runs and
        // drains every chat turn
        let began = std::time::Instant::now();
        self.supervisor.supervisor.shut_down().await;
        step("the supervisor's trees and turns", began);
        // main.py:543-554: hearing, recordings, FERB's browser then the owner's, the shell's processes
        let began = std::time::Instant::now();
        if let Some(m) = self.tools.mac() {
            m.hearing.shutdown().await;
            m.recordings.shutdown().await;
        }
        step("hearing and recordings", began);
        let began = std::time::Instant::now();
        for d in [self.tools.ferb_driver(), self.tools.owner_driver()]
            .into_iter()
            .flatten()
        {
            d.shutdown().await;
        }
        step("the browser drivers", began);
        let began = std::time::Instant::now();
        if let Some(s) = self.tools.shells() {
            s.kill_all().await;
        }
        step("the shell's processes", began);
        self.opts.egress.close();
        // main.py:543: after the proxy's last records went out
        if let Some(r) = self.tools.shells().and_then(|s| s.reports().cloned()) {
            r.close();
        }
        // main.py:557-561: sensing (its watchers, the owner-input tap), then the desktop observer's summaries
        let began = std::time::Instant::now();
        self.b.shutdown().await;
        step("sensing", began);
        self.supervisor.daemon.reads.shutdown();
        let began = std::time::Instant::now();
        if let Some(s) = self.tools.sentinel() {
            s.shutdown().await;
        }
        step("the desktop observer", began);
        let began = std::time::Instant::now();
        self.server.close().await;
        step("the socket's connections", began);
        // the minutes since the last sample (L30)
        self.sample_resources();
        self.repeats.flush();
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
    /// schedules.py `_mac_started` (ORCH-317): a start in a new login session (a restart, a login) is the Mac coming
    /// back into use, unless it is out of use right now (its unlock is the return then). A start in the same session
    /// (an update, a crash) is no return: the current stretch began at the last return journaled (`mac.back_in_use`).
    fn mac_started(&self, new_session: bool) {
        let Some(o) = self.tools.owner() else { return };
        self.mac_started_on(&o.presence, new_session);
    }

    /// `_mac_started` on this presence.
    pub fn mac_started_on(
        &self,
        mac: &ferb_sense::watchers::presence::MacPresence,
        new_session: bool,
    ) {
        if !mac.running() {
            return;
        }
        if new_session {
            if !mac.away() {
                mac.returned("login");
            }
            return;
        }
        let last: Option<String> = {
            let d = lock(&self.db);
            d.conn()
                .query_row(
                    "SELECT at FROM journal WHERE kind = 'mac.back_in_use' ORDER BY seq DESC LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .ok()
        };
        if let Some(ts) = last.as_deref().and_then(back_in_use_since) {
            if mac.present_since().is_none() {
                mac.set_present_since(Some(ts));
            }
        }
    }

    /// `_sample_resources`: one sample; a failure is journaled and the next sample covers this window too.
    pub fn sample_resources(&self) {
        let mut held = lock(&self.resource_log);
        let Some(log) = held.as_mut() else { return };
        let done = log.sample(|| lock(&self.db));
        if let Err(e) = done {
            self.journal.note("resources.failed", json!({"error": e}));
        }
    }
}

/// `datetime.fromisoformat(at).timestamp()` of a `mac.back_in_use` row (an aware time; None when it isn't one).
pub fn back_in_use_since(at: &str) -> Option<f64> {
    let t = ferb_clock::py::parse(at)?;
    Some(t.timestamp() as f64 + f64::from(t.timestamp_subsec_micros()) / 1e6)
}

/// main.py:341 `_restore_service_definition` (DMN-514, rule 6), for a daemon launchd started under FERB's label: an
/// existing LaunchAgent that isn't exactly what setup writes is written again and journaled; FERB.app's own
/// registration has none to put back (the installer's `in_app`). A failure is journaled with its type.
pub fn restore_service_definition(
    note: &dyn Fn(&str, Value),
    inst: &ferb_device::install::Installer,
) {
    match inst.restore_definition() {
        Ok(true) => note(
            "service.definition_restored",
            json!({"path": inst.plist_path().to_string_lossy()}),
        ),
        Ok(false) => {}
        Err(e) => note(
            "service.definition_restore_failed",
            json!({"error": format!("{}: {e}", e.type_name())}),
        ),
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

/// decision: HAND-718 — what the archived sessions left on this Mac (FERB's hooks in Claude Code's settings, the
/// sessions' folder, config.toml's `[sessions]`) is taken away at every start; journaled when something was there, and a
/// failure that stays (say, a settings file that isn't JSON) only the first time.
fn retired_sessions(home: &Path, user_home: &Path, config: &Config, journal: &Journal) {
    let Some(mut done) = ferb_device::claude_hooks::retire_sessions(home, user_home, config) else {
        return;
    };
    let only_failed = done
        .as_object()
        .is_some_and(|m| m.len() == 1 && m.contains_key("errors"));
    if only_failed {
        if !journal
            .of_kind(RETIRED_SESSIONS, "only", "errors")
            .is_empty()
        {
            return;
        }
        done["only"] = json!("errors");
    }
    journal.note(RETIRED_SESSIONS, done);
}

/// The journal kind of [`retired_sessions`].
pub const RETIRED_SESSIONS: &str = "sessions.retired";

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

#[cfg(test)]
mod restore_tests {
    use std::sync::Mutex;

    use serde_json::Value;

    /// main.py:341 (DMN-514): an edited LaunchAgent is written back to exactly what setup writes and journaled once;
    /// one already right, or none at all (FERB.app's own registration has none), is left alone.
    // decision: DMN-514
    #[test]
    fn an_edited_launch_agent_is_put_back() {
        let tmp = tempfile::tempdir().unwrap();
        let (home, user) = (tmp.path().join("ferb"), tmp.path().join("user"));
        std::fs::create_dir_all(user.join("Library/LaunchAgents")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let inst = crate::setup_env::installer_at(&home, user.clone());
        let rows: Mutex<Vec<(String, Value)>> = Mutex::new(Vec::new());
        let note = |k: &str, v: Value| rows.lock().unwrap().push((k.to_string(), v));
        // none there: nothing written
        super::restore_service_definition(&note, &inst);
        assert!(!inst.plist_path().exists());
        assert!(rows.lock().unwrap().is_empty());
        // edited: put back, journaled
        std::fs::write(inst.plist_path(), b"<plist>edited</plist>").unwrap();
        super::restore_service_definition(&note, &inst);
        assert_eq!(
            std::fs::read(inst.plist_path()).unwrap(),
            inst.plist_bytes().unwrap()
        );
        let got = rows.lock().unwrap().clone();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "service.definition_restored");
        assert_eq!(
            got[0].1["path"],
            inst.plist_path().to_string_lossy().as_ref()
        );
        // right already: nothing more
        super::restore_service_definition(&note, &inst);
        assert_eq!(rows.lock().unwrap().len(), 1);
    }
}

#[cfg(test)]
mod mac_started_tests {
    /// schedules.py:80 `_mac_started`, a start in the same login session: the current stretch dates from the last
    /// `mac.back_in_use` (`datetime.fromisoformat(at).timestamp()`: Python's 1767238200.25 for this row), set only
    /// when the presence has none, and nothing is told.
    // decision: ORCH-590
    #[test]
    fn a_start_in_the_same_session_dates_the_stretch_from_the_last_return() {
        assert_eq!(
            super::back_in_use_since("2026-01-01T09:00:00.250000+05:30"),
            Some(1767238200.25)
        );
        assert_eq!(super::back_in_use_since("not a time"), None);
        let mac = ferb_sense::watchers::presence::MacPresence::new(
            || 1800000000.0,
            ferb_sense::watchers::presence::Session::default,
            || None,
            None,
        );
        assert_eq!(mac.present_since(), None);
        mac.set_present_since(super::back_in_use_since("2026-01-01T09:00:00+00:00"));
        assert_eq!(mac.present_since(), Some(1767258000.0));
    }
}
