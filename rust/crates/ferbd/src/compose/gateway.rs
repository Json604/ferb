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

// ---------------------------------------------------------------------------------------------------------------
// the gateway
// ---------------------------------------------------------------------------------------------------------------

/// What [`build`] takes.
pub struct GatewayParts {
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
    pub owner_contacts: Box<dyn Fn() -> Vec<String> + Send + Sync>,
    pub pay_same_site_forms: Box<dyn Fn() -> bool + Send + Sync>,
    pub native: bool,
    pub hooks: Arc<dyn ferb_gateway::seams::CallHooks>,
}

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
    let deps = Deps {
        db: p.db,
        clock: p.clock,
        ids: p.ids,
        redactor: p.redactor,
        ledger: p.ledger,
        publish: p.publish,
        roots: FsRoots {
            workspace: p.home.join("workspace").to_string_lossy().into_owned(),
            owner_roots: vec![],
            untrusted_roots: vec![p
                .home
                .join("workspace/downloads")
                .to_string_lossy()
                .into_owned()],
            home: p.user_home.to_string_lossy().into_owned(),
            protection: Arc::new(SyncProtection(p.protection)),
        },
        device_id: p.device_id,
        recovery,
        snapshots: None,
        usage: Arc::new(RealUsage),
        apps: Arc::new(MacApps { native: p.native }),
        describe: Arc::new(Words { native: p.native }),
        journal_problem: p.journal_problem,
        profile_name: Box::new(|_b, f| f.to_string()),
        profile_account: Box::new(|_b, _f| String::new()),
        owner_contacts: p.owner_contacts,
        pay_same_site_forms: p.pay_same_site_forms,
        on_path: Box::new(|_p, _t| {}),
        proc_task: Box::new(|_p| None),
        route_question: None,
        hooks: p.hooks,
        made_in_tree: None,
        excluded_host: Some(Box::new(move |h: &str| excluded.excluded_host(h))),
        owner_secrets: p.owner_secrets,
        places: Arc::new(BrowserPlaces),
        username_holds: None,
    };
    Arc::new(Gateway::new(registry, deps, || {
        let mut b = [0u8; 6];
        let _ = getrandom_fill(&mut b);
        format!("ap-{}", hex6(&b))
    }))
}

fn getrandom_fill(buf: &mut [u8]) -> std::io::Result<()> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")?.read_exact(buf)
}

fn hex6(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
