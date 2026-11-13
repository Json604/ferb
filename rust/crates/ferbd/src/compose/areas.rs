//! The handler areas' seams filled from the real parts: group A's `Owner`, `Questions`, `Logins` and the settings
//! host; group B's `Deps` for every area (about, notices, shell processes, sensing, Jev, the intro, runtimes,
//! browsers).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_core::config::Config;
use ferb_core::interfaces::BoxFuture;
use ferb_daemon::areas_a::seams::LoginError;
use ferb_daemon::areas_a::{LocalFuture, Logins, Owner};
use ferb_daemon::areas_b::{
    self, about, banners, browsers, core::Host, egress, intro, jev, notify, runtimes, sensing,
};
use ferb_gateway::gateway::Gateway;
use ferb_identity::owner_secrets::OwnerSecrets;
use ferb_identity::Secret;
use ferb_sanitize::redact::Redactor;
use serde_json::{json, Map, Value};

use super::{Keychain, Options};
use crate::basics::{Events, Journal, SharedDb};

// ---------------------------------------------------------------------------------------------------------------
// the Keychain, as the options allow it
// ---------------------------------------------------------------------------------------------------------------

/// helper.py's words when the signed helper isn't there.
pub const UNAVAILABLE: &str = "the signed secret helper is unavailable; owner setup is required";

/// A launcher that never runs the helper: every call is "unavailable" (a daemon started with the Keychain off).
pub struct NoHelper;

impl ferb_identity::helper::Launcher for NoHelper {
    fn verified(&self, _home: &Path) -> ferb_identity::Result<PathBuf> {
        Err(ferb_identity::Error::Refused(UNAVAILABLE.into()))
    }

    fn run(
        &self,
        _program: &Path,
        _body: &[u8],
    ) -> std::io::Result<(bool, zeroize::Zeroizing<Vec<u8>>)> {
        Err(std::io::Error::other(ferb_identity::Error::Refused(
            UNAVAILABLE.into(),
        )))
    }
}

/// HAND-410 (browsers.py:293-296): each secret-helper failure journaled with what is known of why, bounded
/// (`_Repeats`). Python's `secret_helper.on_failure` is the module's one hook for every caller; here the daemon's start
/// sets it once and every helper this program builds reports to it.
pub static HELPER_FAILED: std::sync::OnceLock<ferb_daemon::shared::Repeats> =
    std::sync::OnceLock::new();

/// The signed helper of `home`, or none, as the options say.
pub fn helper(opts: &Options, home: &Path) -> ferb_identity::Helper {
    let built = match opts.keychain {
        Keychain::Real => ferb_platform_macos::identity::helper(home),
        Keychain::Refused => crate::off::off(
            "keychain.refused",
            ferb_identity::Helper::new(home, Arc::new(NoHelper)),
        ),
    };
    built.on_failure(|facts: &ferb_identity::helper::HelperFacts| {
        let Some(r) = HELPER_FAILED.get() else { return };
        let data = crate::basics::object(facts.to_json());
        let why = match (facts.status, facts.found) {
            (Some(s), _) => s.to_string(),
            (None, Some(f)) => if f { "True" } else { "False" }.to_string(),
            (None, None) => "None".to_string(),
        };
        let kind = ferb_daemon::areas_b::browsers::HELPER_FAILED;
        r.record(kind, &[kind, &facts.operation, &why], data);
    })
}

/// `/usr/bin/security` finds nothing (the Keychain off): exit 44, "not found".
pub struct NoSecurity;

impl runtimes::keys::Security for NoSecurity {
    fn run(&self, _args: &[&str]) -> (i32, Secret) {
        (44, Secret::new(String::new()))
    }
}

/// `OwnerSecrets(home)`: the owner's saved logins, through the helper.
pub fn owner_secrets(opts: &Options, home: &Path) -> OwnerSecrets {
    OwnerSecrets::with_keychain(
        home,
        Arc::new(helper(opts, home)),
        Arc::new(super::gateway::BrowserPlaces),
    )
}

// ---------------------------------------------------------------------------------------------------------------
// group A's seams
// ---------------------------------------------------------------------------------------------------------------

/// The owner-facing side as group A calls it (`_task_result`, `_notify`, `_tell_owner`), on group B's notices.
pub struct OwnerSide {
    events: Arc<Events>,
    notifier: Mutex<Option<Arc<notify::Notifier>>>,
}

impl OwnerSide {
    pub fn new(events: &Arc<Events>) -> OwnerSide {
        OwnerSide {
            events: events.clone(),
            notifier: Mutex::new(None),
        }
    }
}

/// The notifier group B builds, shared with group A's owner side once it exists.
pub static NOTIFIER: std::sync::OnceLock<Arc<notify::Notifier>> = std::sync::OnceLock::new();

impl OwnerSide {
    fn notifier(&self) -> Option<Arc<notify::Notifier>> {
        let mut held = crate::basics::lock(&self.notifier);
        if held.is_none() {
            *held = NOTIFIER.get().cloned();
        }
        held.clone()
    }
}

impl Owner for OwnerSide {
    fn task_result(&self, event: Map<String, Value>, shown: bool, alert: bool) -> LocalFuture<()> {
        let n = self.notifier();
        Box::pin(async move {
            if let Some(n) = n {
                let _ = n
                    .task_result(&Value::Object(event), shown, alert, true)
                    .await;
            }
        })
    }

    fn notify(
        &self,
        title: &str,
        text: &str,
        ident: &str,
        target: Value,
        card: Value,
    ) -> LocalFuture<Result<(), String>> {
        let n = self.notifier();
        let banner = banners::Banner {
            title: title.into(),
            text: text.into(),
            // schedules.py's reminder banner (`_notify(..., category="reminder")`); group A's other notices have none
            category: if ident.starts_with("reminder:") {
                "reminder".into()
            } else {
                String::new()
            },
            target: crate::basics::object(target),
            when_hidden: false,
            ident: ident.into(),
            card: crate::basics::object(card),
        };
        Box::pin(async move {
            if let Some(n) = n {
                n.notify(banner, false).await;
            }
            Ok(())
        })
    }

    fn tell_owner(
        &self,
        ident: &str,
        refs: Vec<String>,
        text: &str,
        origin: &str,
        cards: Option<Value>,
    ) -> LocalFuture<()> {
        let n = self.notifier();
        let (ident, text, origin) = (ident.to_string(), text.to_string(), origin.to_string());
        let _ = &self.events;
        Box::pin(async move {
            if let Some(n) = n {
                let cards: Vec<Value> = match cards {
                    Some(Value::Array(a)) => a,
                    Some(other) if !other.is_null() => vec![other],
                    _ => vec![],
                };
                let _ = n
                    .tell_owner(
                        &ident,
                        &refs,
                        &text,
                        &origin,
                        "",
                        "",
                        &cards,
                        &Value::Null,
                        None,
                    )
                    .await;
            }
        })
    }
}

/// FERB's own questions outside a task (group A's `_ask_owner`), as the browser area asks its one question (APP-777).
pub struct GroupAQuestions(pub ferb_daemon::shared::Local<ferb_daemon::areas_a::State>);

impl ferb_daemon::areas_b::browsers::OwnerQuestions for GroupAQuestions {
    fn pending(&self, kind: &str) -> Option<String> {
        let kind = kind.to_string();
        self.0
            .call_blocking(move |st| async move {
                ferb_daemon::areas_a::memory::pending_own(&st, &kind)
            })
            .ok()
            .flatten()
    }
    fn ask(&self, kind: &str, ident: &str, reason: &str, key: &str, options: &[String]) -> String {
        let (kind, ident, reason, key, options) = (
            kind.to_string(),
            ident.to_string(),
            reason.to_string(),
            key.to_string(),
            options.to_vec(),
        );
        self.0
            .call_blocking(move |st| async move {
                ferb_daemon::areas_a::memory::ask_owner(
                    &st,
                    &kind,
                    &ident,
                    &reason,
                    &key,
                    None,
                    Some(&options),
                )
                .unwrap_or_default()
            })
            .unwrap_or_default()
    }
}

/// The owner's saved logins (`OwnerSecrets(home).clear_usernames`).
pub struct SavedLogins(pub Arc<OwnerSecrets>);

impl Logins for SavedLogins {
    fn clear_usernames(
        &self,
        sources: &std::collections::BTreeSet<String>,
    ) -> Result<Vec<String>, LoginError> {
        self.0.clear_usernames(sources).map_err(|e| match e {
            ferb_identity::Error::Refused(m) => LoginError::Refused(m),
            other => LoginError::Failed(format!("{other:?}")),
        })
    }

    /// memory.py:1010 `write`: the username saved for its place (when `save`), then every record whose username
    /// came from an older version of the fact takes the new spelling.
    fn save_username(
        &self,
        place: &str,
        name: &str,
        source: &str,
        save: bool,
        older: &std::collections::BTreeSet<String>,
    ) -> Result<Vec<String>, LoginError> {
        let said = |e: ferb_identity::Error| match e {
            ferb_identity::Error::Refused(m) => LoginError::Refused(m),
            other => LoginError::Failed(format!("{other:?}")),
        };
        if save {
            self.0.set_username(place, name, source).map_err(said)?;
        }
        self.0
            .replace_usernames(older, Some(name), Some(source))
            .map_err(said)
    }
}

/// `settings.set`'s host: the privacy exclusions reach the Redactor at once, and the change is journaled.
pub struct Settings {
    pub journal: Arc<Journal>,
    pub redactor: Arc<Redactor>,
    pub config: Arc<Config>,
}

impl ferb_daemon::areas_a::settings::SettingsHost for Settings {
    fn apply_privacy(&self) {
        super::apply_privacy(&self.config, &self.redactor);
    }

    fn journal(&self, kind: &str, data: Value, origin: &str) {
        if let Err(e) = self.journal.append(kind, data, Some(origin)) {
            eprintln!("FERB: couldn't journal {kind}: {e}");
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// group B
// ---------------------------------------------------------------------------------------------------------------

/// What every group B area is built from.
pub struct Shared {
    pub home: PathBuf,
    pub config: Arc<Config>,
    pub redactor: Arc<Redactor>,
    pub journal: Arc<Journal>,
    pub events: Arc<Events>,
    pub db: SharedDb,
    pub gateway: Arc<Gateway>,
    pub tokens: Arc<ferb_rpc::RunTokens>,
    pub owner_secrets: Arc<OwnerSecrets>,
    pub tools: super::tools::Parts,
    /// The Claude runtime's tool-list relay (`_claude_proxy`), None when it couldn't bind.
    pub claude_proxy: Option<Arc<dyn ferb_runtimes::claude::ToolProxy>>,
    /// The supervisor's runtime offer and switch (`runtime_offer`, `answer_runtime_switch`).
    pub switch: Arc<dyn runtimes::Supervisor>,
}

/// Group B as built: its areas' deps and the parts the daemon's start and shutdown use.
pub struct GroupB {
    pub deps: areas_b::Deps,
    pub runtimes: runtimes::Runtimes,
    pub notifier: Arc<notify::Notifier>,
    pub sense: Option<Arc<ferb_sense::manager::Sense>>,
    pub native: bool,
}

/// `_osascript_notify` and Mac presence (`Desk`); nothing posts and the Mac counts as in use when the Mac's parts
/// are off.
struct MacDesk {
    native: bool,
    /// The Mac's presence (`owner_idle`, ORCH-317); None: the Mac counts as in use.
    presence: Option<Arc<ferb_sense::watchers::presence::MacPresence>>,
}

impl notify::Desk for MacDesk {
    fn osascript(&self, script: &str) -> BoxFuture<'static, Result<(), String>> {
        let script = script.to_string();
        let native = self.native;
        Box::pin(async move {
            if !native {
                return Ok(());
            }
            let run = tokio::process::Command::new("/usr/bin/osascript")
                .arg("-e")
                .arg(script)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            match tokio::time::timeout(Duration::from_secs(10), run).await {
                Ok(Ok(s)) if s.success() => Ok(()),
                Ok(Ok(s)) => Err(format!("osascript exited {s}")),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err("osascript took too long".into()),
            }
        })
    }

    /// notify.py `owner_idle`: the Mac is out of use (asleep, displays asleep, locked, switched away); never input
    /// idleness; unknown counts as present.
    fn away(&self) -> bool {
        self.presence.as_ref().is_some_and(|p| p.away())
    }
}

/// Who shows what the notices would banner (notify.py): the open `logs.subscribe` streams (`_client_shows`, L9), the
/// trees a chat is attached to (`_attached_trees`), and a task's request (`self.tasks.column`); perception's hooks
/// (areas/notify.py:86-94, 233-238) once sensing builds perception (areas/sensing.py:65).
pub struct Listeners {
    pub tools: super::tools::Parts,
    pub db: SharedDb,
}

impl notify::Listeners for Listeners {
    fn client_shows(&self, topic: &str) -> bool {
        self.tools.log_watchers().wants(topic)
    }
    fn attached(&self, tree: &str) -> bool {
        self.tools
            .chat()
            .is_some_and(|c| c.attached_trees().contains(tree))
    }
    /// `self.tasks.column(task_id, "request_id")`: None for a task the store doesn't hold.
    fn request_of(&self, task_id: &str) -> Value {
        let db = crate::basics::lock(&self.db);
        db.conn()
            .query_row(
                "SELECT request_id FROM tasks WHERE id = ?",
                [task_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
            .map_or(Value::Null, Value::String)
    }
    /// notify.py:86-94 `perception.banner_posted`: FERB knows its own banners (L10), so the Notification Center row
    /// a posted one becomes is FERB's, not an arrival.
    fn banner_posted(&self, shown: &Value, poster: &str) -> Option<Value> {
        let text = |k: &str| shown.get(k).map(ferb_py::value::py_str).unwrap_or_default();
        let p = self.tools.perception()?;
        p.banner_posted(&text("title"), &text("body"), poster)
            .map(|b| super::loops::banner_json(&b))
    }
    /// That banner was never posted: no row of it will come back.
    fn banner_failed(&self, banner: &Value) {
        if let (Some(p), Some(b)) = (self.tools.perception(), super::loops::banner_of(banner)) {
            p.banner_failed(b);
        }
    }
    /// notify.py:233-238 `perception.policy.told(...)`: the receipt that FERB told the owner about a notification
    /// a notice cites (its text redacted).
    fn told(&self, bundle_id: &str, conversation: &str, row_id: &str, text: &str) {
        let db = crate::basics::lock(&self.db);
        let (clock, ids, r) = (self.tools.clock(), self.tools.ids(), self.tools.redactor());
        let ids = super::tools::IdsRef(ids);
        let policy = ferb_sense::notifications::NotificationPolicy::new(
            &db,
            clock.as_ref(),
            &ids,
            r.as_ref(),
        );
        let _ = policy.told(bundle_id, conversation, row_id, text);
    }
    /// `trail.row(id)`: the row's source and data as stored.
    fn trail_row(&self, id: &str) -> Option<(String, String)> {
        let db = crate::basics::lock(&self.db);
        db.conn()
            .query_row(
                "SELECT source, data_json FROM trail WHERE id = ?",
                [id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .ok()
    }
}

/// Jev stays off (notices are paused, rust/PLAN.md part 11): its key is never read and no decider is built.
struct JevOff;

impl jev::JevSeams for JevOff {
    fn key(&self) -> BoxFuture<'static, Option<String>> {
        crate::off::off("jev.key", Box::pin(async { None }))
    }
    fn build(&self, _key: String) -> Arc<dyn jev::Decider> {
        unreachable!("Jev's key is never read in this build, so no decider is built")
    }
    fn wire(&self, _jev: Option<Arc<dyn jev::Decider>>) {}
}

/// The runtimes area's journal reads.
struct Reads(Arc<Journal>);

impl runtimes::JournalReads for Reads {
    fn latest_kind(&self, kinds: &[&str], field: &str, equals: &str) -> Option<String> {
        self.0.latest_kind(kinds, field, equals)
    }
    fn of_kind(&self, kind: &str, field: &str, equals: &str) -> Vec<(String, Value)> {
        self.0.of_kind(kind, field, equals)
    }
}

/// The two check tools in the gateway's registry while a check runs.
struct CheckTools(Arc<Gateway>);

impl runtimes::ToolRegistry for CheckTools {
    fn add(&self, tool: Arc<dyn runtimes::check::CheckTool>) {
        self.0
            .registry()
            .set(Arc::new(super::tools::misc::CheckToolAdapter(tool)));
    }
    fn remove(&self, name: &str) {
        self.0.registry().remove(name);
    }
}

/// What one runtime check holds while it runs.
struct Checks {
    gateway: Arc<Gateway>,
    tools: super::tools::Parts,
    tokens: Arc<ferb_rpc::RunTokens>,
    db: SharedDb,
    redactor: Arc<Redactor>,
    clock: Arc<dyn ferb_core::interfaces::Clock>,
    ids: Arc<dyn ferb_core::interfaces::IdSource>,
    /// The spans a check started and hasn't finished (`spans.finish` takes the span itself).
    open: Mutex<std::collections::HashMap<String, ferb_store::spans::Span>>,
}

impl Checks {
    fn spans<T>(&self, f: impl FnOnce(&ferb_store::spans::SpanStore<'_>) -> T) -> T {
        let db = crate::basics::lock(&self.db);
        let r = self.redactor.clone();
        let s = ferb_store::spans::SpanStore::new(
            &db,
            Box::new(ferb_gateway::gateway::StoreTime(self.clock.clone())),
            Box::new(ferb_gateway::gateway::StoreIds(self.ids.clone())),
            Box::new(move |v: &Value| r.value(v)),
            Box::new(|kind: &str| ferb_policy::rules::REVERSIBLE.contains(&kind)),
        );
        f(&s)
    }
}

impl runtimes::CheckHost for Checks {
    fn new_id(&self) -> String {
        self.ids.new_id()
    }

    fn span_start(&self, kind: &str, input: Value) -> String {
        let Ok(span) = self.spans(|s| s.start(kind, &input, None, None, None)) else {
            return String::new();
        };
        let id = span.id.clone();
        crate::basics::lock(&self.open).insert(id.clone(), span);
        id
    }

    fn span_finish(&self, span_id: &str, output: Value) {
        let Some(mut span) = crate::basics::lock(&self.open).remove(span_id) else {
            return;
        };
        self.spans(|s| {
            let _ = s.finish(
                &mut span,
                ferb_store::spans::Finish {
                    output: Some(&output),
                    error: None,
                    cost_usd: None,
                    origin: None,
                    summary: None,
                },
            );
        });
    }

    fn hold_task(
        &self,
        task: &ferb_core::types::Task,
        started_at: &ferb_core::when::DateTime,
        span_id: &str,
    ) {
        self.tools.hold_task(
            &self.gateway,
            ferb_gateway::gateway::TaskRecord {
                task_id: task.id.clone(),
                request_id: task.request_id.clone(),
                started_at: *started_at,
                deadline: None,
                span_id: span_id.to_string(),
                tree_id: Some(task.id.clone()),
            },
        );
    }

    fn remove_task(&self, task_id: &str) {
        self.tools.release_task(&self.gateway, task_id);
    }

    fn issue_token(
        &self,
        task_id: &str,
        holder: &str,
        scopes: &std::collections::BTreeSet<String>,
        identity: ferb_core::types::Identity,
        ttl_s: f64,
        observe_only: bool,
        tools: &std::collections::BTreeSet<String>,
        max_calls: u32,
    ) -> String {
        let identity = match identity {
            ferb_core::types::Identity::Owner => ferb_rpc::auth::Identity::Owner,
            ferb_core::types::Identity::Ferb => ferb_rpc::auth::Identity::Ferb,
        };
        self.tokens.issue(
            task_id,
            holder,
            ferb_rpc::auth::GrantSpec {
                scopes: scopes.clone(),
                identity,
                ttl_s,
                observe_only,
                tools: tools.clone(),
                max_calls: u64::from(max_calls),
            },
        )
    }

    fn revoke_token(&self, holder: &str) {
        self.tokens.revoke_worker(holder);
    }

    fn forget_calls(&self, holder: &str) {
        self.gateway.forget_calls(holder);
    }
}

/// `_notify` as the browser area calls it.
pub struct BrowserBanners(pub Arc<notify::Notifier>);

impl browsers::Notifier for BrowserBanners {
    fn notify(
        &self,
        title: &str,
        text: &str,
        urgent: bool,
        category: &str,
        target: Value,
        ident: &str,
    ) -> browsers::BoxFut<()> {
        let n = self.0.clone();
        let banner = banners::Banner {
            title: title.into(),
            text: text.into(),
            category: category.into(),
            target: crate::basics::object(target),
            when_hidden: false,
            ident: ident.into(),
            card: Map::new(),
        };
        Box::pin(async move { n.notify(banner, urgent).await })
    }
}

/// The router's tabs (`Router.shows`, `show_tab`, `tab_profile`) as the browser area's handoff reads them.
pub struct RouterTabs(pub Arc<ferb_browser::driver::Driver>);

fn tab_ref(v: &Value) -> Option<ferb_core::types::TabRef> {
    serde_json::from_value(v.clone()).ok()
}

impl browsers::Tabs for RouterTabs {
    fn shows(&self, tab: &Value, message: &str) -> browsers::BoxFut<Result<Value, String>> {
        let d = self.0.clone();
        let (tab, message) = (tab_ref(tab), message.to_string());
        Box::pin(async move {
            let tab = tab.ok_or_else(|| "no such tab".to_string())?;
            d.shows(&tab, &message)
                .await
                .map(Value::Object)
                .map_err(|e| e.to_string())
        })
    }
    fn show_tab(&self, tab: &Value) -> browsers::BoxFut<bool> {
        let d = self.0.clone();
        let tab = tab_ref(tab);
        Box::pin(async move {
            match tab {
                Some(t) => d.show_tab(&t).await.unwrap_or(false),
                None => false,
            }
        })
    }
    fn tab_profile(&self, tab: &Value) -> (String, String) {
        tab_ref(tab)
            .and_then(|t| self.0.tab_profile(&t).ok())
            .unwrap_or_default()
    }
}

/// Group B's areas over the real parts.
pub fn group_b(
    opts: &Options,
    s: &Shared,
    a_state: Arc<std::sync::OnceLock<ferb_daemon::shared::Local<ferb_daemon::areas_a::State>>>,
) -> GroupB {
    let host = Host {
        home: s.home.clone(),
        config: s.config.clone(),
        redactor: s.redactor.clone(),
        journal: s.journal.clone(),
        bus: s.events.clone(),
        clock: opts.clock.clone(),
    };
    let mut d = areas_b::Deps::new(host.clone());
    let posters = banners::Posters::default();
    let notifier = Arc::new(notify::Notifier {
        host: host.clone(),
        posters: posters.clone(),
        desk: opts
            .overrides
            .as_ref()
            .and_then(|o| o.desk())
            .unwrap_or_else(|| {
                Arc::new(MacDesk {
                    native: opts.native,
                    presence: s.tools.owner().map(|o| o.presence.clone()),
                })
            }),
        listeners: Arc::new(Listeners {
            tools: s.tools.clone(),
            db: s.db.clone(),
        }),
        notices: Some(s.db.clone()),
        held: Mutex::new(Vec::new()),
        uid: Arc::new(|| ferb_runtimes::codex::token_hex(6)),
    });
    let _ = NOTIFIER.set(notifier.clone());
    d.notify = Some(notify::Deps {
        posters,
        notices: Some(s.db.clone()),
    });
    d.jev = Some(jev::Deps {
        seams: Arc::new(JevOff),
        jev: Arc::new(Mutex::new(None)),
    });
    d.egress = Some(egress::Deps {
        shells: s.tools.shells(),
        poll: Duration::from_millis(100),
    });
    let sense = Arc::new(super::loops::sense(opts, s));
    s.tools.set_sense(sense.clone());
    // sensing.py:151-159 `_sense_status`: the tap's state, the watch folders skipped, perception's status
    let perception = s
        .tools
        .perception()
        .expect("loops::sense builds perception");
    d.sensing = Some(sensing::Deps {
        sense: Some(sense.clone()),
        owner_input_running: {
            let owner = s.tools.owner().cloned();
            Arc::new(move || owner.as_ref().is_some_and(|o| o.input.is_running()))
        },
        watch_problems: perception.watch_problems.clone(),
        perception_status: Some(Arc::new(move || perception.status())),
    });
    // the intro's reads of group A (below) take their own handle: `about` moves `a_state`
    let intro_a = a_state.clone();

    // about
    let native = opts.native;
    let grants: Arc<dyn Fn() -> Map<String, Value> + Send + Sync> = Arc::new(move || {
        if native {
            crate::basics::object(ferb_mac_hands::native::permissions::check_all())
        } else {
            Map::new()
        }
    });
    let shell_ok = s.tools.shells().is_some();
    // rpc.py:487-491 `_setup_containment`: the containment facts this daemon started with (the sandbox profile
    // checked once at start, the shell, the proxy), as group C's setup reads them
    let egress = opts.egress.clone();
    let containment: Arc<dyn Fn() -> Map<String, Value> + Send + Sync> = Arc::new(move || {
        let contained = egress.containment.get().is_some_and(|(ok, _)| *ok);
        crate::basics::object(
            json!({"profile_ok": contained, "shell": shell_ok && contained,
                                     "proxy": egress.running() && contained}),
        )
    });
    let db_path = crate::basics::lock(&s.db).path().to_path_buf();
    let gw = s.gateway.clone();
    let tree_db = s.db.clone();
    d.about = Some(Arc::new(about::Deps {
        version: super::VERSION.into(),
        guide: Arc::new(Mutex::new(None)),
        grants,
        containment,
        account: Arc::new(Accounts {
            tools: s.tools.clone(),
            config: s.config.clone(),
        }),
        store: Some(s.db.clone()),
        store_facts: Arc::new(move || {
            (
                ferb_store::store_size(&db_path),
                ferb_store::cleanup::pending(&db_path),
            )
        }),
        // records.py `_permissions_in_force` on group A's thread (its store), with the gateway's "don't ask" window
        permissions_in_force: Arc::new(move || {
            let until = gw.allow_until();
            let a = a_state.get().cloned().ok_or("the daemon isn't started")?;
            a.call_blocking(move |st| async move {
                ferb_daemon::areas_a::records::permissions_in_force(&st, until.as_ref())
            })
            .map_err(|e| e.to_string())?
        }),
        // records.py `_saved_logins`: the owner's saved entries and FERB's own secrets, names and sites only
        saved_logins: {
            let owner = s.owner_secrets.clone();
            let cfg = s.config.clone();
            Arc::new(move || {
                let mine =
                    ferb_identity::secrets::specs_from_config(cfg.section("secrets").ok().as_ref())
                        .into_iter()
                        .map(|sp| (sp.secret.key, sp.account_hint, sp.allowed_origins))
                        .collect();
                let listed = owner.inventory().map_err(|e| {
                    match e {
                        ferb_identity::Error::Refused(_) => "Refused",
                        ferb_identity::Error::HelperFailed { .. } => "HelperFailed",
                        ferb_identity::Error::Io(_) => "OSError",
                    }
                    .to_string()
                });
                Ok(ferb_daemon::areas_a::records::saved_logins(listed, mine))
            })
        },
        owner_secrets: s.owner_secrets.clone(),
        evidence: Some(Arc::new(about::Evidence {
            root: s.home.join("evidence"),
        })),
        tree_of: Arc::new(move |task_id: &str| {
            let db = crate::basics::lock(&tree_db);
            // a read: the tree's clock is never asked
            let tree = ferb_orchestrator::tree::TaskTree::new(
                Some(&db),
                crate::off::off("tree.read_clock", Box::new(String::new)),
            );
            tree.tree_of(task_id).ok().flatten()
        }),
    }));

    // intro: the suggestion reads what FERB saw on this Mac (MEM-13, group A's persona) and asks the model on group
    // A's thread (DMN-500); the preview is the supervisor's `preview_events`
    let shutting: Arc<dyn Fn() -> bool + Send + Sync> = {
        let parts = s.tools.downgrade();
        Arc::new(move || parts.upgrade().is_some_and(|p| p.shutting_down()))
    };
    let tools = s.tools.clone();
    let preview: intro::PreviewStart = Arc::new(move |session: &str, text: &str| {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let sup = tools.supervisor().cloned();
        let (session, text) = (session.to_string(), text.to_string());
        tokio::spawn(async move {
            let Some(sup) = sup else {
                let _ = tx
                    .send(Err(intro::PreviewEnd::Ferb("daemon not started".into())))
                    .await;
                return;
            };
            match sup.preview_events(&session, &text).await {
                Ok(events) => {
                    for e in events {
                        let ev = match e {
                            Value::Object(m) => m,
                            _ => continue,
                        };
                        if tx.send(Ok(ev)).await.is_err() {
                            return;
                        }
                    }
                }
                // intro.py: a FERB error ends the stream with its words; anything else as `couldnt(...)`
                Err(ferb_supervisor::flow::Failure::Raised { class, message })
                    if class == "FerbError" || class == "Refused" =>
                {
                    let _ = tx.send(Err(intro::PreviewEnd::Ferb(message))).await;
                }
                Err(ferb_supervisor::flow::Failure::Cancelled) => {}
                Err(f) => {
                    let line = format!("I couldn't show that plan: {}", f.message());
                    let _ = tx.send(Err(intro::PreviewEnd::Other(line))).await;
                }
            }
        });
        rx
    });
    d.intro = Some(Arc::new(intro::Deps {
        // memory.py:1127-1153 `_mac_profile_service().status()`, on group A's thread where the persona lives
        scan: {
            let a = intro_a.clone();
            Arc::new(move || match a.get().cloned() {
                Some(a) => a
                    .call_blocking(|st| async move { st.persona.borrow().status() })
                    .unwrap_or_else(|_| json!({"state": "closed"})),
                None => json!({"state": "not_started"}),
            })
        },
        // intro.py:107-119 `_suggest`'s model call (role intro_suggest)
        ask: {
            let a = intro_a;
            Arc::new(move |system: String, ctx: String| {
                let a = a.get().cloned();
                Box::pin(async move {
                    let a = a.ok_or_else(|| intro::CallFailed("RuntimeError".into()))?;
                    a.call(move |st| async move {
                        super::memory_loops::intro_suggest(&st, &system, &ctx).await
                    })
                    .await
                    .map_err(|_| intro::CallFailed("CancelledError".into()))?
                    .map_err(intro::CallFailed)
                })
            })
        },
        // intro.py:112-113: `load_prompt("orchestrator/prompts/intro.md", {"capabilities": availability().text})`
        system: {
            let package = opts.package.clone();
            let tools = s.tools.clone();
            Arc::new(move || {
                let capabilities = tools
                    .supervisor()
                    .map(|sup| sup.inner().deps.daemon.availability())
                    .unwrap_or_default();
                ferb_context::prompts::PromptRoot::new(&package)
                    .map_err(|e| e.to_string())?
                    .load(
                        "orchestrator/prompts/intro.md",
                        &[("capabilities", capabilities.as_str())],
                    )
                    .map_err(|e| e.to_string())
            })
        },
        limit: Duration::from_secs(55),
        shutting_down: shutting,
        preview: Some(preview),
    }));

    // runtimes
    let helper = Arc::new(helper(opts, &s.home));
    let security: Arc<dyn runtimes::keys::Security> = match opts.keychain {
        Keychain::Real => Arc::new(runtimes::keys::SystemSecurity),
        Keychain::Refused => crate::off::off("keychain.refused_security", Arc::new(NoSecurity)),
    };
    let programs = Arc::new(super::runtimes::MacPrograms {
        home: s.home.clone(),
        app_bundle: opts.app_bundle.clone(),
    });
    let rt_state = Arc::new(runtimes::State::default());
    let bridge = Arc::new(super::runtimes::GatewayBridge {
        gateway: s.gateway.clone(),
        tokens: s.tokens.clone(),
        tools: s.tools.clone(),
    });
    let rt_deps_partial = |factory: Arc<dyn runtimes::Factory>| runtimes::Deps {
        supervisor: s.switch.clone(),
        factory,
        programs: programs.clone(),
        helper: helper.clone(),
        security: security.clone(),
        tools: Arc::new(CheckTools(s.gateway.clone())),
        checks: Arc::new(Checks {
            gateway: s.gateway.clone(),
            tools: s.tools.clone(),
            tokens: s.tokens.clone(),
            db: s.db.clone(),
            redactor: s.redactor.clone(),
            clock: opts.clock.clone(),
            ids: opts.ids.clone(),
            open: Mutex::new(std::collections::HashMap::new()),
        }),
        journal_reads: Arc::new(Reads(s.journal.clone())),
        model_lists: Arc::new(runtimes::ProviderLists),
        state: rt_state.clone(),
    };
    // the API loop reads its key through the area's own key store (keys.py `api_key`)
    let keys_area: Arc<std::sync::OnceLock<runtimes::Runtimes>> =
        Arc::new(std::sync::OnceLock::new());
    let k = keys_area.clone();
    let factory = Arc::new(super::runtimes::RealFactory {
        redactor: s.redactor.clone(),
        bridge,
        keys: Arc::new(move |provider: String| {
            let k = k.clone();
            Box::pin(async move {
                match k.get() {
                    Some(rt) => rt.api_key(&provider).await.ok().flatten(),
                    None => None,
                }
            })
        }),
        refusals: Arc::new(ferb_runtimes::forms::Refusals::new(Box::new({
            let c = opts.clock.clone();
            move || c.monotonic()
        }))),
        claude_cli: ferb_daemon::areas_b::runtimes::Programs::claude_cli(programs.as_ref()).ok(),
        proxy: s.claude_proxy.clone(),
    });
    let rt_deps = rt_deps_partial(factory);
    let rt = runtimes::Runtimes::new(host.clone(), rt_deps.clone());
    let _ = keys_area.set(rt.clone());
    d.runtimes = Some(rt_deps);

    // browsers
    d.browsers = Some(super::tools::browsers_deps(opts, s, &notifier));

    GroupB {
        deps: d,
        runtimes: rt,
        notifier,
        sense: Some(sense),
        native: opts.native,
    }
}

/// The browser facts FERB's About reads (`Account`, about.py:59): the browser area's profiles found now (none before
/// Full Disk Access, DMN-507) and `_identity_paired()`: FERB's own profile paired as FERB.
struct Accounts {
    tools: super::tools::Parts,
    config: Arc<Config>,
}

impl about::Account for Accounts {
    fn profiles(&self) -> Vec<ferb_browser::profiles::BrowserProfile> {
        self.tools
            .browsers()
            .and_then(|b| b.area())
            .map(|cx| cx.profiles())
            .unwrap_or_default()
    }
    fn identity_paired(&self) -> bool {
        self.tools
            .browsers()
            .is_some_and(|b| b.identity_paired(&self.config))
    }
}

impl GroupB {
    /// `_preload_secrets` (environment.py:229): the API keys of the providers this home's roles use, known to the
    /// redactor before this run first uses them (GATE-573); only counts are journaled.
    pub async fn preload_secrets(&self, journal: &Journal, redactor: &Redactor) {
        let providers: Vec<String> = ferb_runtimes::catalog::ROLES
            .iter()
            .map(|r| self.runtimes.runtime_for(r))
            .collect();
        let rt = self.runtimes.clone();
        let found = tokio::task::spawn_blocking(move || {
            let keys = runtimes::keys::Keys {
                helper: rt.deps.helper.clone(),
                security: rt.deps.security.clone(),
                cache: Arc::new(runtimes::keys::KeyCache::default()),
            };
            let mut problems = Vec::new();
            let r = keys.stored_api_keys(&providers, &mut problems);
            (r, problems)
        })
        .await;
        let Ok((found, problems)) = found else { return };
        for p in problems {
            journal.note("keys.old_item_not_deleted", p);
        }
        let (values, unreadable) = match found {
            Ok(v) => (v, 0),
            Err(_) => (Vec::new(), 1),
        };
        if !values.is_empty() || unreadable > 0 {
            let n = redactor.preload(values.iter().map(|s| s.expose()));
            journal.note(
                "secret.registry_preloaded",
                json!({"values": n, "unreadable": unreadable}),
            );
        }
    }

    /// `_prepare_codex` (DMN-510).
    pub async fn prepare_codex(&self) {
        let _ = self.runtimes.prepare_codex().await;
    }

    pub async fn shutdown(&self) {
        if let Some(s) = &self.sense {
            s.stop().await;
            for (part, ms) in s.slow_stop() {
                crate::log::line(&format!("stop: sensing's {part} took {ms} ms"));
            }
        }
    }
}
