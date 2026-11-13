//! The gateway as the daemon builds it (`_build_stores`, compose.py; `GatewayDeps`, DMN-11): every safety store
//! given, the real call hooks, the orchestrator's ledger behind the gateway's `Ledger` seam, and the adapters from the
//! Mac's parts to the gateway's seams that no crate provides.

use std::path::Path;
use std::sync::{Arc, Mutex};

use ferb_core::interfaces::{BoxFuture, Clock, IdSource};
use ferb_gateway::gateway::{Deps, FsRoots, Gateway, SyncProtection, POSIX_FILES};
use ferb_gateway::seams::{BadTransition, Describe, DuplicateUnresolved, Ledger, Usage};
use ferb_gateway::tool::Registry;
use ferb_gateway::words::LedgerRow;
use ferb_identity::owner_secrets::Places;
use ferb_orchestrator::ledger;
use ferb_policy::confirm::{Apps, AppsUnread, RunningApp};
use ferb_store::Db;
use serde_json::{json, Map, Value};

use crate::basics::{lock, SharedDb};

// ---------------------------------------------------------------------------------------------------------------
// the ledger (ferb-orchestrator) behind the gateway's seam
// ---------------------------------------------------------------------------------------------------------------

/// `ferb_orchestrator::ledger::Ledger` on the daemon's store, behind `ferb_gateway::seams::Ledger` (the shape the
/// gateway's own scenario tests wire, journal 39).
pub struct RealLedger {
    pub db: SharedDb,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdSource>,
}

fn ledger_row(r: ledger::LedgerRow) -> LedgerRow {
    LedgerRow {
        id: r.intent.id,
        task_id: r.intent.task_id,
        capability: r.intent.capability,
        target: r.intent.target.as_object().cloned().unwrap_or_default(),
        payload_hash: r.intent.payload_hash,
        state: r.intent.state,
        reconciled: r.reconciled,
        evidence: r
            .evidence
            .iter()
            .map(|e| match e {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect(),
        retry_approved: r.retry_approved,
        retried_by: r.retried_by,
    }
}

fn dup(e: ferb_orchestrator::sql::Error) -> DuplicateUnresolved {
    match e {
        ferb_orchestrator::sql::Error::Duplicate {
            message,
            intent_id,
            state,
        } => DuplicateUnresolved {
            message,
            intent_id,
            state,
        },
        // A failed write must stop the action: it is refused as unresolved, with the store's words.
        other => DuplicateUnresolved {
            message: format!("the action ledger couldn't be written: {other}"),
            intent_id: String::new(),
            state: "unknown".into(),
        },
    }
}

fn bad(e: ferb_orchestrator::sql::Error) -> BadTransition {
    match e {
        ferb_orchestrator::sql::Error::BadTransition(m) => BadTransition(m),
        other => BadTransition(other.to_string()),
    }
}

impl RealLedger {
    pub fn with<T>(&self, f: impl FnOnce(&ledger::Ledger<'_>) -> T) -> T {
        let db = lock(&self.db);
        let clock = self.clock.clone();
        let ids = self.ids.clone();
        let l = ledger::Ledger::new(
            &db,
            Box::new(move || ferb_core::when::isoformat(&clock.now())),
            Box::new(move || ids.new_id()),
        );
        f(&l)
    }

    /// ledger.py:156: what a restart leaves: every row still in flight is unknown.
    pub fn mark_unknown_after_restart(&self) -> Result<Vec<String>, String> {
        self.with(|l| l.mark_unknown_after_restart())
            .map_err(|e| e.to_string())
    }
}

impl Ledger for RealLedger {
    fn intend(
        &self,
        task_id: &str,
        capability: &str,
        target: &Map<String, Value>,
        payload_hash: &str,
    ) -> Result<String, DuplicateUnresolved> {
        let target = Value::Object(target.clone());
        self.with(|l| l.intend(task_id, capability, &target, payload_hash, true))
            .map(|i| i.id)
            .map_err(dup)
    }

    fn mark(&self, intent_id: &str, state: &str, evidence: &[String]) -> Result<(), BadTransition> {
        self.with(|l| l.mark(intent_id, state, evidence))
            .map_err(bad)
    }

    fn refusal(
        &self,
        task_id: &str,
        capability: &str,
        payload_hash: &str,
    ) -> Option<DuplicateUnresolved> {
        match self.with(|l| l.refusal(task_id, capability, payload_hash)) {
            Ok(Some(e)) => Some(dup(e)),
            Ok(None) => None,
            Err(e) => Some(dup(e)),
        }
    }

    fn approved_retry(
        &self,
        task_id: &str,
        capability: &str,
        payload_hash: &str,
    ) -> Option<String> {
        self.with(|l| l.approved_retry(task_id, capability, payload_hash))
            .ok()
            .flatten()
    }

    fn attempts(&self, intent_id: &str) -> Vec<LedgerRow> {
        self.with(|l| l.attempts(intent_id))
            .map(|rows| rows.into_iter().map(ledger_row).collect())
            .unwrap_or_default()
    }

    fn approve_retry(&self, intent_id: &str, ask_span: &str) -> Result<(), BadTransition> {
        self.with(|l| l.approve_retry(intent_id, ask_span))
            .map_err(bad)
    }

    fn get(&self, intent_id: &str) -> Option<LedgerRow> {
        self.with(|l| l.get(intent_id)).ok().map(ledger_row)
    }
}

/// The same ledger as the browser bridge reads it (`state`, `reconcile`).
pub struct BridgeLedger(pub Arc<RealLedger>);

impl ferb_browser::bridge::Ledger for BridgeLedger {
    fn state(&self, intent_id: &str) -> Option<String> {
        self.0
            .with(|l| l.get(intent_id))
            .ok()
            .map(|r| r.intent.state)
    }

    fn reconcile(&self, intent_id: &str, observed: &str, evidence: Vec<String>) {
        let _ = self.0.with(|l| l.reconcile(intent_id, observed, &evidence));
    }
}

// ---------------------------------------------------------------------------------------------------------------
// small seams
// ---------------------------------------------------------------------------------------------------------------

/// S4 (c): a folder's count before a permanent delete (`ferb/act/fs/usage.py`).
pub struct RealUsage;

impl Usage for RealUsage {
    fn count<'a>(
        &'a self,
        path: &'a str,
        closed: ferb_core::types::PathCheck,
    ) -> BoxFuture<'a, Map<String, Value>> {
        Box::pin(async move {
            let check = |p: &str, a: &str| (closed.0)(Path::new(p), a);
            ferb_fs::usage::usage(path, Some(&check), true).unwrap_or_default()
        })
    }
}

/// The Mac's running and installed apps for the confirmations (`apps.candidates`, `installed_bundle`,
/// `is_browser_app`). `native` false: none are read (a daemon started with its Mac parts off).
pub struct MacApps {
    pub native: bool,
}

impl Apps for MacApps {
    fn candidates(&self, name: &str) -> Result<Vec<RunningApp>, AppsUnread> {
        if !self.native {
            return Ok(Vec::new());
        }
        let running = ferb_mac_hands::native::apps::running();
        Ok(ferb_mac_hands::apps::candidates(&running, name)
            .into_iter()
            .map(|a| RunningApp {
                bundle_id: a.bundle_id,
                name: a.name,
            })
            .collect())
    }

    fn installed_bundle(&self, name: &str) -> Result<Option<String>, AppsUnread> {
        if !self.native {
            return Ok(None);
        }
        Ok(ferb_mac_hands::native::apps::installed_bundle(name))
    }

    fn is_browser_app(&self, app: &str) -> bool {
        if !self.native {
            return ferb_mac_hands::apps::known(&ferb_mac_hands::apps::clean(app));
        }
        let handlers = ferb_mac_hands::apps::WebHandlers::default();
        let running = |pid: Option<i32>, name: Option<&str>| -> Vec<String> {
            ferb_mac_hands::native::apps::running()
                .into_iter()
                .filter(|a| {
                    pid.is_some_and(|p| a.pid == p)
                        || name.is_some_and(|n| ferb_mac_hands::apps::clean(&a.name) == n)
                })
                .map(|a| a.bundle_id)
                .collect()
        };
        ferb_mac_hands::apps::is_browser_app(&handlers, app, &running)
    }
}

/// Which apps are web browsers and how a browser reads an address (`is_browser`, `urls.parse`).
pub struct BrowserPlaces;

impl Places for BrowserPlaces {
    fn is_browser(&self, bundle_id: &str) -> bool {
        ferb_mac_hands::apps::known(bundle_id)
    }

    fn url_host(&self, url: &str) -> Option<String> {
        ferb_policy::urls::parse(url)
            .map(|p| p.host)
            .filter(|h| !h.is_empty())
    }
}

/// ferb/policy's words as orchestration reads them (`ferb_orchestrator::ports::Policy`).
pub struct PolicyWords;

impl ferb_orchestrator::ports::Policy for PolicyWords {
    fn site_key(&self, value: Option<&str>) -> String {
        ferb_policy::confirm::site_key(value)
    }
    fn account_key(&self, value: Option<&str>) -> String {
        ferb_policy::confirm::account_key(value)
    }
    fn url_host(&self, url: &str) -> Option<String> {
        ferb_policy::urls::parse(url).map(|p| p.host)
    }
    fn from_task(&self, text: &str) -> String {
        ferb_policy::confirm::from_task(text)
    }
    fn reads_as_ferbs_check(&self, text: &str) -> bool {
        ferb_policy::confirm::reads_as_ferbs_check(text)
    }
}

/// `progress.describe` and `_app_name` for a retry question (orchestration's words).
pub struct Words {
    pub native: bool,
}

impl Describe for Words {
    fn describe(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        kind: &str,
        site: Option<&str>,
        app: Option<&str>,
    ) -> String {
        ferb_orchestrator::progress::describe(&PolicyWords, tool, args, kind, site, app)
    }

    fn app_name(&self, app: &str) -> String {
        if !self.native {
            return "an app".into();
        }
        let running = ferb_mac_hands::native::apps::running();
        ferb_mac_hands::apps::candidates(&running, app)
            .into_iter()
            .next()
            .map(|a| a.name)
            .unwrap_or_else(|| "an app".into())
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the protection lists
// ---------------------------------------------------------------------------------------------------------------

/// `protection.load(config.get, Path.home(), self.home)`: [protection] with its defaults, after checking the
/// workspace. `Err` is BadProtection's reason (the daemon doesn't start).
pub fn protection(
    config: &ferb_core::config::Config,
    user_home: &Path,
    home: &Path,
    app_bundle: Option<&Path>,
) -> Result<ferb_policy::protection::Protection<'static>, String> {
    use ferb_policy::protection as p;
    let get = |key: &str, default: Value| -> Value {
        match config.get("protection", key) {
            Ok(Some(v)) => v,
            _ => default,
        }
    };
    let never_read = get("never_read", json!(p::default_never_read()));
    let never_write = get("never_write", json!(p::DEFAULT_NEVER_WRITE));
    let terminal_apps = get("terminal_apps", json!(p::DEFAULT_TERMINAL_APPS));
    let local_ports = get("local_ports", json!([]));
    let lists = p::Lists {
        never_read: &never_read,
        never_write: &never_write,
        terminal_apps: &terminal_apps,
        local_ports: &local_ports,
    };
    let app = app_bundle
        .map(Path::to_path_buf)
        .or_else(ferb_core::paths::installed_app);
    p::load(
        &lists,
        &user_home.to_string_lossy(),
        &home.to_string_lossy(),
        app.as_deref().map(|a| a.to_str().unwrap_or_default()),
        &POSIX_FILES,
    )
    .map_err(|e| e.0)
}

/// compose.py:111 `Changes(..., closed=lambda path: protection.read_denied(path) is not None)`: L39's change record
/// reads exactly what the shell's sandbox may read (FERB's state and the never-read stores closed), whatever the file
/// tools may. Lists that can't be read close every path (the daemon doesn't start then anyway).
pub fn change_reads_closed(
    opts: &crate::compose::Options,
    config: &ferb_core::config::Config,
) -> ferb_fs::Closed {
    match protection(
        config,
        &opts.user_home,
        &opts.home,
        opts.app_bundle.as_deref(),
    ) {
        Ok(p) => reads_closed(p),
        Err(_) => Arc::new(|_: &Path| true),
    }
}

/// `protection.read_denied(path) is not None` as the change record's check.
pub fn reads_closed(p: ferb_policy::protection::Protection<'static>) -> ferb_fs::Closed {
    let lists = SyncProtection(p);
    Arc::new(move |path: &Path| {
        let lists: &SyncProtection = &lists;
        lists.0.read_denied(&path.to_string_lossy()).is_some()
    })
}

// ---------------------------------------------------------------------------------------------------------------
// what the gateway reads about the owner when it asks (the browser area and group A, built after it)
// ---------------------------------------------------------------------------------------------------------------

/// compose.py:121-127's `profile_name`, `profile_account`, `username_sources`, `owner_contacts` and `on_path`, read
/// (or noted) at use through the tools' parts: the browser area and group A are attached after the gateway is built.
pub struct OwnerFacts(pub crate::compose::tools::WeakParts);

impl OwnerFacts {
    /// compose.py:127 `on_path=self.attribution.ferb_touched`: a path a call of this task touched, so the owner's
    /// trail doesn't count FERB's own change as the owner's.
    pub fn ferb_touched(&self, path: &str, task: &str) {
        if let Some(parts) = self.0.upgrade() {
            parts.attribution().ferb_touched(path, Some(task));
        }
    }

    fn area(&self) -> Option<ferb_daemon::areas_b::browsers::Cx> {
        self.0.upgrade()?.browsers()?.area()
    }

    /// browsers.py:642 `_profile_display_name`: the profile's name as discovered, else its folder.
    pub fn profile_name(&self, browser: &str, folder: &str) -> String {
        match self.area() {
            Some(cx) => cx.profile_display_name(browser, folder),
            None => folder.to_string(),
        }
    }

    /// browsers.py:646 `_profile_account` (HAND-424): the account the profile is signed in to, read fresh.
    pub fn profile_account(&self, browser: &str, folder: &str) -> String {
        self.area()
            .map(|cx| cx.profile_account(browser, folder))
            .unwrap_or_default()
    }

    /// Group A's answer on its loop thread (directly when already there); None while it isn't running.
    fn on_a<T: Send + 'static>(
        &self,
        f: impl FnOnce(&ferb_daemon::areas_a::State) -> T + Send + 'static,
    ) -> Option<T> {
        let a = self.0.upgrade()?.group_a()?;
        if let Some(st) = a.here() {
            return Some(f(&st));
        }
        a.call_blocking(move |st| async move { f(&st) }).ok()
    }

    /// memory.py:494 `_username_sources` (L36 §6): whether a saved username's source still holds: the owner's own
    /// memory items that hold now; any source while memory isn't running (Python's None).
    pub fn username_holds(&self, source: &str) -> bool {
        let current = self.on_a(|st| st.memory.current_ids(Some("owner_stated")).ok());
        source_holds(current.as_ref().map(|c| c.as_ref()), source)
    }

    /// memory.py:502-515 `_owner_contacts` (GATE-538): the owner's own email addresses and phone numbers, by shape:
    /// the accounts their paired owner profiles are signed in to, and what they wrote about themselves in memory.
    pub fn owner_contacts(&self) -> Vec<String> {
        let accounts: Vec<String> = match self.0.upgrade() {
            Some(parts) => {
                let paired = parts
                    .browsers()
                    .and_then(|b| b.pairings.all().ok())
                    .unwrap_or_default();
                paired
                    .into_iter()
                    .filter(|b| b.identity == ferb_core::types::Identity::Owner)
                    .map(|b| self.profile_account(&b.browser, &b.folder))
                    .collect()
            }
            None => Vec::new(),
        };
        let items: Vec<(String, String)> = self
            .on_a(|st| {
                st.memory
                    .items(Some("active"), Some("owner"), true)
                    .map(|items| {
                        items
                            .into_iter()
                            .map(|i| (i.origin, i.content))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        contacts_of(&accounts, &items)
    }
}

/// `source in username_sources()`: a source holds while its memory item does; with no memory, any source holds.
pub fn source_holds(
    current: Option<Option<&std::collections::BTreeSet<String>>>,
    source: &str,
) -> bool {
    match current {
        // memory isn't running: `None`, any source
        None => true,
        Some(ids) => match (ids, source.strip_prefix("memory:")) {
            (Some(ids), Some(id)) => ids.contains(id),
            _ => false,
        },
    }
}

/// GATE-538's set (`contacts_in` over each account, and over each owner-stated item about the owner), sorted.
pub fn contacts_of(accounts: &[String], items: &[(String, String)]) -> Vec<String> {
    let mut found: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for a in accounts {
        found.extend(ferb_policy::confirm::contacts_in(a));
    }
    for (origin, content) in items {
        if origin == "owner_stated" {
            found.extend(ferb_policy::confirm::contacts_in(content));
        }
    }
    found.into_iter().collect()
}

// ---------------------------------------------------------------------------------------------------------------
// the gateway
// ---------------------------------------------------------------------------------------------------------------

/// What [`build`] takes.
/// compose.py:99-100: `[fs] owner_roots` and the untrusted roots, `downloads.untrusted_roots([fs] untrusted_roots
/// (default ["~/Downloads"]), workspace)`: the gateway's `roots`, read once at start as Python does. The same list
/// is the supervisor's and the upload copies' (rule 17).
pub fn fs_roots(
    config: &ferb_core::config::Config,
    home: &std::path::Path,
) -> (Vec<String>, Vec<String>) {
    let list = |key: &str, default: &[&str]| -> Vec<String> {
        match config.get("fs", key) {
            Ok(Some(Value::Array(a))) => a.iter().map(ferb_py::value::py_str).collect(),
            _ => default.iter().map(|s| s.to_string()).collect(),
        }
    };
    let workspace = home.join("workspace").to_string_lossy().into_owned();
    (
        list("owner_roots", &[]),
        ferb_browser::downloads::untrusted_roots(
            &list("untrusted_roots", &["~/Downloads"]),
            &workspace,
        ),
    )
}

pub struct GatewayParts {
    /// `[fs]`'s owner and untrusted roots ([`fs_roots`]).
    pub roots: (Vec<String>, Vec<String>),
    pub db: SharedDb,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdSource>,
    pub redactor: Arc<ferb_sanitize::redact::Redactor>,
    pub ledger: Arc<RealLedger>,
    pub publish: ferb_gateway::gateway::EventSink,
    pub journal_problem: ferb_gateway::gateway::Sink,
    pub home: std::path::PathBuf,
    pub user_home: std::path::PathBuf,
    pub device_id: String,
    pub protection: ferb_policy::protection::Protection<'static>,
    pub owner_secrets: Option<Arc<ferb_identity::owner_secrets::OwnerSecrets>>,
    /// The owner's profiles' names and accounts, saved usernames' sources and own contacts, read at use.
    pub owner_facts: OwnerFacts,
    pub pay_same_site_forms: Box<dyn Fn() -> bool + Send + Sync>,
    pub native: bool,
    pub hooks: Arc<dyn ferb_gateway::seams::CallHooks>,
    /// The task a running shell process belongs to (`shells.live_owner`), for the proxy's destination check.
    pub proc_task: ProcTask,
    /// S1's snapshots before a shell command or a permanent delete (HAND-206).
    pub snapshots: Option<Arc<dyn ferb_gateway::seams::Snapshots>>,
    /// L39's change record: whether a task tree made a file (`changes.made_in_tree`).
    pub changes: Arc<ferb_fs::changes::Changes>,
    /// The questions' ids (Python: `"ap-" + secrets.token_hex(6)`), shared with the supervisor's own questions.
    pub question_ids: QuestionIds,
    /// `route_question`: a worker's question to the orchestrator first (the supervisor, bound once built).
    pub route_question: Option<Arc<dyn ferb_gateway::seams::QuestionRouter>>,
}

/// Where question ids come from: [`random_question_id`] in the daemon, a counter in a test build.
pub type QuestionIds = Arc<dyn Fn() -> String + Send + Sync>;

/// `"ap-" + secrets.token_hex(6)`.
pub fn random_question_id() -> String {
    let mut b = [0u8; 6];
    let _ = getrandom_fill(&mut b);
    format!("ap-{}", hex6(&b))
}

/// S1's `Snapshots` as the gateway asks it (`snapshots.request(task, why, changes=True)`).
pub struct GatewaySnapshots(pub Arc<ferb_fs::recovery::Snapshots>);

impl ferb_gateway::seams::Snapshots for GatewaySnapshots {
    fn request<'a>(
        &'a self,
        task_id: &'a str,
        why: &'a str,
    ) -> ferb_core::interfaces::BoxFuture<'a, serde_json::Map<String, serde_json::Value>> {
        Box::pin(async move { self.0.request(Some(task_id), why, true).await })
    }
}

/// The task a running shell process belongs to.
pub type ProcTask = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// `Gateway(GatewayDeps(...))` with `registry`'s tools. Approval ids are Python's `"ap-" + token_hex(6)`.
pub fn build(registry: Registry, p: GatewayParts) -> Arc<Gateway> {
    let recovery = Arc::new(ferb_gateway::adapters::fs::FsRecovery {
        db: Arc::new(Mutex::new(
            Db::open(lock(&p.db).path()).expect("the store opened once already"),
        )),
        clock: p.clock.clone(),
        ids: p.ids.clone(),
        root: p.home.join("recovery").to_string_lossy().into_owned(),
    });
    let excluded = p.redactor.clone();
    let facts = Arc::new(p.owner_facts);
    let deps = Deps {
        db: p.db,
        clock: p.clock,
        ids: p.ids,
        redactor: p.redactor,
        ledger: p.ledger,
        publish: p.publish,
        roots: FsRoots {
            workspace: p.home.join("workspace").to_string_lossy().into_owned(),
            owner_roots: p.roots.0,
            untrusted_roots: p.roots.1,
            home: p.user_home.to_string_lossy().into_owned(),
            protection: Arc::new(SyncProtection(p.protection)),
        },
        device_id: p.device_id,
        recovery,
        snapshots: p.snapshots,
        usage: Arc::new(RealUsage),
        apps: Arc::new(MacApps { native: p.native }),
        describe: Arc::new(Words { native: p.native }),
        journal_problem: p.journal_problem,
        // browser_pair's question names the profile and its signed-in account (compose.py:121-122)
        profile_name: {
            let f = facts.clone();
            Box::new(move |b, folder| f.profile_name(b, folder))
        },
        profile_account: {
            let f = facts.clone();
            Box::new(move |b, folder| f.profile_account(b, folder))
        },
        // GATE-538 (compose.py:124)
        owner_contacts: {
            let f = facts.clone();
            Box::new(move || f.owner_contacts())
        },
        pay_same_site_forms: p.pay_same_site_forms,
        on_path: {
            let f = facts.clone();
            Box::new(move |path, task| f.ferb_touched(path, task))
        },
        proc_task: p.proc_task,
        route_question: p.route_question.clone(),
        hooks: p.hooks,
        made_in_tree: {
            let changes = p.changes.clone();
            Some(Box::new(move |root: &str, path: &str| {
                changes.made_in_tree(root, path)
            }))
        },
        excluded_host: Some(Box::new(move |h: &str| excluded.excluded_host(h))),
        owner_secrets: p.owner_secrets,
        places: Arc::new(BrowserPlaces),
        // L36 §6 (compose.py:124 `username_sources`)
        username_holds: Some(Box::new(move |source: &str| facts.username_holds(source))),
    };
    let ids = p.question_ids;
    Arc::new(Gateway::new(registry, deps, move || ids()))
}

fn getrandom_fill(buf: &mut [u8]) -> std::io::Result<()> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")?.read_exact(buf)
}

fn hex6(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
