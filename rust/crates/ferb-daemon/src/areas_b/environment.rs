//! What the planner and workers are told about this Mac and FERB right now: environment, availability, facts, a
//! task's tabs and relevant activity. Port of `ferb/daemon/areas/environment.py` (a mixin of the Daemon). Must not
//! select a route: agents interpret it.
//!
//! Every other part of the daemon these functions read comes in through [`Seams`] (the composition implements it
//! over the real engines; tests implement it over recorded state); FERB's home, config, redactor and journal come
//! from [`Host`]. The browser drivers are [`TabDriver`]s, the extension hub a [`Hub`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use ferb_browser::pairing::Binding;
use ferb_browser::profiles::{self, BrowserProfile};
use ferb_core::interfaces::BoxFuture;
use ferb_core::types::Identity;
use ferb_identity::Secret;
use ferb_orchestrator::availability::{self, Availability};
use ferb_rpc::HandlerError;
use ferb_sanitize::untrusted::NoRandomness;
use serde_json::{json, Map, Value};

use super::core::Host;

/// A paired owner profile that isn't connected, where FERB can't start it without a window (no wake service): plain
/// words, not a fault (live 2026-10-24: "extension transport disconnected" read as broken).
pub const IDLE_PROFILE: &str =
    "not connected now — FERB connects once that profile is open in its browser";

/// One registered task as these facts read it (`self.tasks.get(id)`: the live record's `tree_id` and its task).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskRec {
    pub id: String,
    pub tree_id: Option<String>,
    pub role: String,
    pub parent_id: Option<String>,
    pub scopes: BTreeSet<String>,
    pub request_id: Option<String>,
    pub identity: String,
    pub device_id: String,
}

/// A paired profile's extension bridge (`hub.bridges[(browser, folder)]`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bridge {
    /// `bridge.conn` is set: the extension is connected now.
    pub connected: bool,
    /// The version its live hello reported ("" when none).
    pub version: String,
}

/// A secret FERB keeps for the owner, as `secrets.specs()` lists it (never its value).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SecretSpec {
    pub key: String,
    pub account_hint: String,
    pub allowed_origins: Vec<String>,
}

/// Why a browser driver's release of a task's tabs failed: the exception's `"<Type>: <text>"` and the tabs it left
/// behind (`exc.facts["tabs"]`, HAND-419).
#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseError {
    pub error: String,
    pub tabs: Option<Value>,
}

/// One browser driver (`self.router`: the owner's browsers; `self.ferb_identity`: FERB's own), as `_TabDriver`.
pub trait TabDriver: Send + Sync {
    /// The tabs a task holds now, each with the page it held when the task took it over.
    fn task_tabs(&self, task_id: &str) -> Vec<Map<String, Value>>;
    fn urls_of(&self, task_id: &str) -> Vec<String>;
    /// HAND-201: at the task's end its tabs leave the group and stay open.
    fn keep_tabs_open(&self, task_id: &str);
    /// A loop's round starts (`resting` false) or ends (true): the held tab's file picker (L31).
    fn rest<'a>(&'a self, task_id: &'a str, resting: bool) -> BoxFuture<'a, ()>;
    /// The task's tabs let go of: the resources released.
    fn release<'a>(&'a self, task_id: &'a str) -> BoxFuture<'a, Result<Vec<Value>, ReleaseError>>;
    /// `driver.wake is not None`: the wake service can start a profile without a window.
    fn has_wake(&self) -> bool;
}

/// The extension hub (`self.hub`).
pub trait Hub: Send + Sync {
    fn bridge(&self, browser: &str, folder: &str) -> Option<Bridge>;
    /// `hub.unpaired.values()`: the browsers with a connected but unpaired extension.
    fn unpaired(&self) -> Vec<String>;
    /// HAND-406: why the hub refuses this paired profile now (e.g. too old).
    fn problem(&self, binding: &Binding) -> Option<String>;
}

/// The rest of the daemon as environment.py reads it.
pub trait Seams: Send + Sync {
    /// `_identity_profile()`: FERB's own (browser, folder).
    fn identity_profile(&self) -> Option<(String, String)>;
    /// `self.pairings.all()`; None: no pairings store.
    fn pairings(&self) -> Option<Vec<Binding>>;
    /// `self.pairings.version(install_id)`: the extension version the pairing record holds.
    fn pairing_version(&self, install_id: &str) -> String;
    /// `self.journal.of_running_tasks("security.alignment")`: each row's data.
    fn alignment(&self) -> Vec<Value>;
    /// `_profiles()`: the browser profiles discovered now (Local State; none without Full Disk Access).
    fn profiles(&self) -> Vec<BrowserProfile>;
    /// `_fresh_profiles()`: the profiles as last discovered (kept fresh off the loop).
    fn fresh_profiles(&self) -> Vec<BrowserProfile>;
    /// `_installed_apps()` (desktop): (bundle id, name).
    fn installed_apps(&self) -> Vec<(String, String)>;
    /// The running apps' bundle ids; None when they can't be read.
    fn running_bundle_ids(&self) -> Option<BTreeSet<String>>;
    /// `platform.mac_ver()[0]` ("" when unknown).
    fn macos_version(&self) -> String;
    /// `self.browser_data_dirs[browser]`: a test's or a setting's user-data folder for that browser.
    fn browser_data_dir_override(&self, browser: &str) -> Option<PathBuf>;
    /// `profiles.SUPPORT`: ~/Library/Application Support.
    fn support(&self) -> PathBuf;
    /// `self.browser_support`.
    fn browser_support(&self) -> PathBuf;
    /// `_profile_state(p, extension)` (browsers area): a profile's connection as the setup window shows it.
    fn profile_state(&self, p: &BrowserProfile, extension: Option<&str>) -> String;
    /// `_worker_types()`.
    fn worker_types(&self) -> Value;
    /// `self.tasks.get(task_id)`.
    fn task(&self, task_id: &str) -> Option<TaskRec>;
    /// `self.supervisor.continuity.readable_tasks(tree)`; None: no supervisor.
    fn readable_tasks(&self, tree: &str) -> Option<BTreeSet<String>>;
    /// `self.router`.
    fn router(&self) -> Option<Arc<dyn TabDriver>>;
    /// `self.ferb_identity`.
    fn ferb_identity(&self) -> Option<Arc<dyn TabDriver>>;
    /// `self.daily_activity.search(words)` (its default limit, 8).
    fn activity_search(&self, words: &str) -> Vec<Map<String, Value>>;
    /// `wrap_untrusted(text)`: data, not instructions, in tags with a random id.
    fn wrap_untrusted(&self, text: &str) -> Result<String, NoRandomness> {
        ferb_sanitize::untrusted::wrap_untrusted(text)
    }
    /// The scopes of every registered capability.
    fn registered_scopes(&self) -> BTreeSet<String>;
    /// `self.identity_browser is not None`: FERB's own browser is paired.
    fn identity_browser(&self) -> bool;
    /// `_browsing(False, "")`: where FERB's web work goes while it has no profile of its own (APP-777).
    fn browsing(&self) -> Map<String, Value>;
    /// `self._fresh.get(self._permissions)`: the daemon's TCC grants, kept fresh off the loop.
    fn permissions(&self) -> BTreeMap<String, bool>;
    /// `_runtime_for(role)`: the provider a role runs on.
    fn runtime_for(&self, role: &str) -> String;
    /// runtimes/keys.py `stored_api_keys` through the secrets helper: the API keys of these providers. A problem the
    /// key store reports (`keys.old_item_not_deleted`) is pushed to `problems`. Err: the helper failed.
    fn stored_api_keys(
        &self,
        providers: &[String],
        problems: &mut Vec<Value>,
    ) -> Result<Vec<Secret>, String>;
    /// `_about_summary()`.
    fn about_summary(&self) -> String;
    /// `self.secrets.specs()`.
    fn secret_specs(&self) -> Vec<SecretSpec>;
    /// `self.missing_dev_tools`.
    fn missing_dev_tools(&self) -> Vec<String>;
    /// `self.missing_dev_tools = []` (DMN-512: once installed, the fact is gone for good).
    fn clear_missing_dev_tools(&self);
    /// `dev_tools_installed()`: a folder check.
    fn dev_tools_installed(&self) -> bool;
    /// `_time_meanings()`: (phrase, meaning).
    fn time_meanings(&self) -> Vec<(String, String)>;
    /// `_models()`: (role, model), in order.
    fn models(&self) -> Vec<(String, String)>;
    /// `_schedule_facts(last_runs)`; None: no schedules store.
    fn schedule_facts(&self, last_runs: bool) -> Option<Vec<String>>;
    /// `_reminder_facts()`; None: no timers.
    fn reminder_facts(&self) -> Option<Vec<String>>;
    /// `self.hub`.
    fn hub(&self) -> Option<Arc<dyn Hub>>;
    /// `_unpaired_note(browser)` ("" for none).
    fn unpaired_note(&self, browser: &str) -> String;
}

fn get(host: &Host, section: &str, key: &str) -> Result<Option<Value>, HandlerError> {
    host.get(section, key)
}

fn key_of(b: &Binding) -> (String, String) {
    (b.browser.clone(), b.folder.clone())
}

/// What `_environment_files` reads off the loop.
pub struct EnvironmentFiles {
    pub found: Vec<BrowserProfile>,
    pub apps: Vec<(String, String)>,
    pub running: BTreeSet<String>,
    pub macos: String,
    /// (browser, folder) → "on", "off", "missing" or "removed", for each profile not yet paired.
    pub extension: BTreeMap<(String, String), String>,
}

/// environment.py `_environment_files`: browser profiles, installed apps, running apps, the macOS version, and
/// whether FERB's extension is in each profile not yet paired (its Preferences files). No database. Blocking: run it
/// off the event loop.
pub fn environment_files(
    host: &Host,
    s: &dyn Seams,
    paired: &BTreeSet<(String, String)>,
) -> Result<EnvironmentFiles, HandlerError> {
    let found = s.profiles();
    let running = s.running_bundle_ids().unwrap_or_default();
    let store = ferb_browser::autoinstall::store_extension_id(
        get(host, "browser", "store_extension_id")?.as_ref(),
    );
    // the pinned unpacked id and the store id (L25)
    let ids = ferb_browser::autoinstall::accepted_extension_ids(
        get(host, "browser", "extension_ids")?.as_ref(),
        &store,
    );
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut extension = BTreeMap::new();
    for p in &found {
        let key = (p.browser.clone(), p.folder.clone());
        if paired.contains(&key) {
            continue;
        }
        let data = s.browser_data_dir_override(&p.browser);
        let state_dir = data
            .clone()
            .or_else(|| profiles::data_dir(&p.browser, &s.support()));
        let mut state = match &state_dir {
            Some(d) => profiles::extension_state(&d.join(&p.folder), &ids),
            None => "missing",
        };
        // HAND-449: a missing copy the owner removed (the browser won't add it again), from the same Preferences
        if state == "missing" {
            let removed_dir = data.or_else(|| profiles::data_dir(&p.browser, &s.browser_support()));
            if removed_dir.is_some_and(|d| ferb_browser::autoinstall::owner_removed(p, &d, &store))
            {
                state = "removed";
            }
        }
        extension.insert(key, state.to_owned());
    }
    let macos = s.macos_version();
    Ok(EnvironmentFiles {
        found,
        apps: s.installed_apps(),
        running,
        macos: if macos.is_empty() {
            "unknown".into()
        } else {
            macos
        },
        extension,
    })
}

/// environment.py `_environment`'s answer: the cacheable facts and the ones that change between turns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvironmentFacts {
    pub stable: String,
    pub live: String,
}

/// environment.py `_environment`: the macOS version and browser connection facts (no installed-apps list, ORCH-514);
/// the model interprets the owner's requested route. `_own_words` is not read (as in Python). The files are read in
/// [`environment_files`]; the composition runs this off the event loop.
pub fn environment(
    host: &Host,
    s: &dyn Seams,
    _own_words: &str,
) -> Result<EnvironmentFacts, HandlerError> {
    let ferb = s.identity_profile();
    let bindings = s.pairings().unwrap_or_default();
    let alignment = s.alignment();
    let paired_keys: BTreeSet<(String, String)> = bindings.iter().map(key_of).collect();
    let files = environment_files(host, s, &paired_keys)?;
    let paired: Vec<Vec<String>> = bindings
        .iter()
        .filter(|b| b.identity == Identity::Owner && Some(key_of(b)) != ferb)
        .map(|b| {
            let p = profiles::named(&files.found, &b.browser, &b.folder);
            vec![b.browser.clone(), b.folder.clone(), p.display_name, p.email]
        })
        .collect();
    let env = ferb_policy::small::Environment {
        apps: files.apps.clone(),
        paired,
    };
    let (mut stable, mut live) =
        ferb_policy::small::services_summary(&env, &files.macos, &files.running);
    let is_ferb =
        |p: &BrowserProfile| ferb.as_ref() == Some(&(p.browser.clone(), p.folder.clone()));
    let inventory: Vec<Value> = files
        .found
        .iter()
        .map(|p| {
            json!({"browser": p.browser, "profile": p.folder, "name": p.display_name, "account_hint": p.email,
                   "identity": if is_ferb(p) { "ferb" } else { "owner" }})
        })
        .collect();
    // each profile's connection changes between turns: with the live facts, after the conversation (ORCH-325)
    let connections: Vec<Value> = files
        .found
        .iter()
        .map(|p| {
            let ext = files
                .extension
                .get(&(p.browser.clone(), p.folder.clone()))
                .map(String::as_str);
            json!({"browser": p.browser, "profile": p.folder, "connection": s.profile_state(p, ext)})
        })
        .collect();
    if !alignment.is_empty() {
        live.push_str("\nNon-enforcing alignment advice for active tasks: ");
        live.push_str(&ferb_py::json::dumps(&alignment));
    }
    if !connections.is_empty() {
        live.push_str("\nBrowser profile connections now: ");
        live.push_str(&ferb_py::json::dumps(&connections));
    }
    if connections.iter().any(|c| c["connection"] == "removed") {
        // HAND-449: as Pair all says it
        let url =
            ferb_device::extensions::store_page(&ferb_browser::autoinstall::store_extension_id(
                get(host, "browser", "store_extension_id")?.as_ref(),
            ));
        live.push_str(
            "\nConnection \"removed\": the owner removed FERB's extension in that profile, so the browser won't \
             add it again by itself; FERB can't pair or use that profile until the owner adds FERB ",
        );
        if url.is_empty() {
            live.push_str("there again");
        } else {
            live.push_str(&format!("from its Chrome Web Store page there ({url})"));
        }
        live.push_str(", then FERB pairs it.");
    }
    stable.push_str("\nBrowser/profile inventory (facts, not a selected route): ");
    stable.push_str(&ferb_py::json::dumps(&inventory));
    // in the planner's fixed Layer 1 (never dropped for room, unlike today's activity): what worker_type may name
    stable.push_str("\nWorker types: ");
    stable.push_str(&ferb_py::json::dumps(&s.worker_types()));
    Ok(EnvironmentFacts { stable, live })
}

/// environment.py `_task_root`: the request's root task a worker step belongs to (download budgets and the tab cap
/// are per task tree).
pub fn task_root(s: &dyn Seams, task_id: &str) -> String {
    match s.task(task_id).and_then(|r| r.tree_id) {
        Some(tree) if !tree.is_empty() => tree,
        _ => task_id.split('.').next().unwrap_or(task_id).to_owned(),
    }
}

/// environment.py `_inspection_parent`: a registered checker borrows its worker's tabs and evidence, without
/// changing their ownership.
pub fn inspection_parent(s: &dyn Seams, task_id: &str) -> Option<String> {
    let child = s.task(task_id)?;
    if child.role != "verifier" {
        return None;
    }
    let parent = child.parent_id.as_deref().and_then(|p| s.task(p))?;
    if parent.role != "worker" || !child.scopes.is_subset(&parent.scopes) {
        return None;
    }
    let mine = (
        &child.request_id,
        &child.identity,
        &child.device_id,
        &child.tree_id,
    );
    let theirs = (
        &parent.request_id,
        &parent.identity,
        &parent.device_id,
        &parent.tree_id,
    );
    (mine == theirs).then_some(parent.id)
}

/// environment.py `_conversation_tasks`: the tasks of the conversation a running task (or the planner's lookup turn)
/// belongs to, what task.evidence's task_id may read (ORCH-593). From the registered record's tree, never
/// from a name.
pub fn conversation_tasks(s: &dyn Seams, task_id: &str) -> BTreeSet<String> {
    let Some(rec) = s.task(task_id) else {
        return BTreeSet::new();
    };
    let tree = rec
        .tree_id
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| task_id.to_owned());
    s.readable_tasks(&tree).unwrap_or_default()
}

fn drivers(s: &dyn Seams) -> Vec<(&'static str, Arc<dyn TabDriver>)> {
    let mut out = Vec::new();
    if let Some(d) = s.router() {
        out.push(("browser", d));
    }
    if let Some(d) = s.ferb_identity() {
        out.push(("ferb_browser", d));
    }
    out
}

/// environment.py `_loop_rest`: a loop's round starts or ends (L31); each driver gives the owner's held tab its file
/// picker back between rounds, or takes it for the round.
pub async fn loop_rest(s: &dyn Seams, task_id: &str, resting: bool) {
    for (_, d) in drivers(s) {
        d.rest(task_id, resting).await;
    }
}

/// environment.py `_task_tabs`: the tabs a task holds now, by tab id, with the page each held when the task took it
/// over. A model reads them: redacted as every tool result is.
pub fn task_tabs(host: &Host, s: &dyn Seams, task_id: &str) -> Vec<Value> {
    let mut tabs = Vec::new();
    for (prefix, d) in drivers(s) {
        for tab in d.task_tabs(task_id) {
            let mut m = Map::new();
            m.insert("tool_prefix".into(), Value::from(prefix));
            for (k, v) in tab {
                m.insert(k, v);
            }
            tabs.push(Value::Object(m));
        }
    }
    if tabs.is_empty() {
        return tabs;
    }
    match host.redact(&Value::Array(tabs)) {
        Value::Array(a) => a,
        _ => Vec::new(),
    }
}

/// environment.py `_browser_context`: a checker's view of its worker's tabs ("" for anyone else, or no tabs).
pub fn browser_context(host: &Host, s: &dyn Seams, task_id: &str) -> String {
    let Some(parent) = inspection_parent(s, task_id) else {
        return String::new();
    };
    let tabs = task_tabs(host, s, &parent);
    if tabs.is_empty() {
        return String::new();
    }
    format!(
        "\n\nExisting task tabs available for inspection (reuse these aliases; do not reopen the page or guess tab \
         ids). A tab is known by its id: one you navigated elsewhere is still the same tab, and taken_over_from is \
         the page it held when the task took it over: {}",
        ferb_py::json::dumps(&tabs)
    )
}

/// environment.py `_urls_of`.
pub fn urls_of(s: &dyn Seams, task_id: &str) -> Vec<String> {
    drivers(s)
        .into_iter()
        .flat_map(|(_, d)| d.urls_of(task_id))
        .collect()
}

/// environment.py `_keep_tabs_open` (HAND-201).
pub fn keep_tabs_open(s: &dyn Seams, task_id: &str) {
    for (_, d) in drivers(s) {
        d.keep_tabs_open(task_id);
    }
}

/// environment.py `_relevant_context`'s answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelevantContext {
    /// The prompt's part: today's matching activity, wrapped as data.
    pub text: String,
    /// Its origin: FERB's, joined with the trail labels of the rows kept.
    pub origin: String,
    /// The rows' data, as JSON (non-ASCII kept).
    pub data: String,
    /// The search's source refs (`trail:<source_id>`).
    pub refs: Vec<String>,
}

/// environment.py `_relevant_context`: today's matching activity, with its origin (S3). The turn's one search (L28);
/// within the activity limit, best first, never a row cut; the rest counted (ORCH-307).
pub fn relevant_context(s: &dyn Seams, words: &str) -> Result<RelevantContext, NoRandomness> {
    let found = Value::Array(
        s.activity_search(words)
            .into_iter()
            .map(Value::Object)
            .collect(),
    );
    // addresses without query strings (live 2026-10-28)
    let activity = match ferb_sense::daily::without_queries(&found) {
        Value::Array(a) => a,
        _ => Vec::new(),
    };
    let limit = ferb_memory::search::WORKER.activity_tokens;
    let (mut kept, mut used) = (Vec::new(), 0usize);
    let total = activity.len();
    for row in activity {
        let cost = ferb_memory::search::tokens(&ferb_py::json::dumps_utf8(&row));
        if used + cost <= limit {
            kept.push(row);
            used += cost;
        }
    }
    let left_out = total - kept.len();
    let trail: BTreeSet<String> = kept
        .iter()
        .filter_map(|r| r.get("source"))
        .filter(|v| ferb_py::value::truthy(v))
        .map(ferb_py::value::py_str)
        .collect();
    let trail: Vec<String> = trail.into_iter().collect();
    let labels: BTreeSet<String> = if trail.is_empty() {
        BTreeSet::new()
    } else {
        ferb_sense::tools::trail_sources(&trail)
            .into_iter()
            .map(|(k, i)| ferb_sanitize::origin::from_read_source(&k, &i))
            .collect()
    };
    let mut origins: Vec<&str> = vec![ferb_sanitize::origin::FERB];
    origins.extend(labels.iter().map(String::as_str));
    let mut text = format!(
        "\nRelevant activity today (data): {}\n",
        s.wrap_untrusted(&ferb_py::json::dumps(&kept))?
    );
    if left_out > 0 {
        text.push_str(&format!(
            "({left_out} more matching activity row(s) today left out to stay within the limit; trail.query finds \
             them.)\n"
        ));
    }
    let data: Vec<&Value> = kept
        .iter()
        .map(|r| match r.get("data") {
            Some(d) if r.is_object() => d,
            _ => r,
        })
        .collect();
    let refs = kept
        .iter()
        .filter(|r| {
            r.get("source").is_some_and(ferb_py::value::truthy)
                && r.get("source_id").is_some_and(ferb_py::value::truthy)
        })
        .map(|r| format!("trail:{}", ferb_py::value::py_str(&r["source_id"])))
        .collect();
    Ok(RelevantContext {
        text,
        origin: ferb_sanitize::origin::join(origins),
        data: ferb_py::json::dumps_utf8(&data),
        refs,
    })
}

/// environment.py `_availability`: what the planner may use, and what's missing with how to fix it (owner decision,
/// 2026-09-30). Only facts that hold across turns: it sits in every role's instructions, the planner's cached prefix
/// included (ORCH-303); whether a paired profile is connected now is a live fact ([`live_facts`]).
pub fn availability(s: &dyn Seams) -> Availability {
    let registered = s.registered_scopes();
    let mut problems: Vec<(String, String)> = Vec::new();
    let mut add = |k: &str, v: String| problems.push((k.to_owned(), v));
    let mut shown_profiles = String::new();
    if registered.contains("browser:act") {
        let ferb = s.identity_profile();
        let owner: Vec<Binding> = s
            .pairings()
            .unwrap_or_default()
            .into_iter()
            .filter(|b| b.identity == Identity::Owner && Some(key_of(b)) != ferb)
            .collect();
        let found = if owner.is_empty() {
            Vec::new()
        } else {
            s.fresh_profiles()
        };
        if owner.is_empty() {
            add(
                "owner_browsers",
                "none of the owner's browser profiles is paired — browser_pair pairs one (FERB asks the owner once)"
                    .into(),
            );
        }
        // HAND-424: each with the account it is signed in to its browser with (cached prefix: it changes rarely)
        shown_profiles = owner
            .iter()
            .map(|b| {
                profiles::shown(
                    &profiles::named(&found, &b.browser, &b.folder),
                    Some(&profiles::capitalize(&b.browser)),
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        if !shown_profiles.is_empty() {
            shown_profiles =
                format!("paired: {shown_profiles} (whether each is connected now: the live facts)");
        }
    }
    let prof = s.identity_profile();
    if prof.is_some() && !s.identity_browser() {
        // FERB's own browser is part of this FERB
        add("ferb_browser", "not paired yet".into());
    } else if prof.is_none() && registered.contains("browser:act") {
        // APP-777: where FERB's web work goes instead
        use ferb_orchestrator::browsing;
        let found = s.browsing();
        let flag = |k: &str| found.get(k).is_some_and(ferb_py::value::truthy);
        let why = if flag("as_owner") {
            let label = found
                .get("label")
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            browsing::with_label(browsing::CHOSEN_FACT, &label)
        } else if flag("declined") {
            browsing::NONE_FACT.to_owned()
        } else {
            browsing::UNASKED_FACT.to_owned()
        };
        add("ferb_browser", why);
    }
    // Not open right now is fine: FERB opens its own profile when a task needs it (the owner's report, 2026-09-30)
    let perms = s.permissions();
    let granted = |k: &str| perms.get(k).copied().unwrap_or(true);
    if (registered.contains("mac:read") || registered.contains("mac:act"))
        && !granted("accessibility")
    {
        add("mac", "FERB doesn't have the Accessibility permission — the owner grants it in FERB's Settings → Permissions".into());
    }
    if registered.contains("screen") && !granted("screen") {
        add("screen", "FERB doesn't have the Screen Recording permission — the owner grants it in FERB's Settings → Permissions".into());
    }
    if registered.contains("audio") && !granted("screen") {
        // app sound is captured under Screen Recording
        add("hearing", "FERB doesn't have the Screen Recording permission, which covers app sound — the owner grants it in FERB's Settings → Permissions".into());
    }
    availability::build(&registered, &problems, &shown_profiles)
}

/// environment.py `_add_secret_value`: one of FERB's own credentials, hidden by its value wherever it appears.
pub fn add_secret_value(host: &Host, value: &Secret) {
    host.redactor.add_secret_value(value.expose());
}

/// environment.py `_keychain_values`: the API keys of the providers this home's runtime table uses, read through
/// the key store (each read is a helper call: run it off the event loop). Saved secrets are never read here: their
/// values aren't masked (GATE-572). (values, 1 when the store couldn't be read).
pub fn keychain_values(s: &dyn Seams, problems: &mut Vec<Value>) -> (Vec<Secret>, u32) {
    let providers: BTreeSet<String> = ferb_runtimes::catalog::ROLES
        .iter()
        .map(|r| s.runtime_for(r))
        .collect();
    let providers: Vec<String> = providers.into_iter().collect();
    match s.stored_api_keys(&providers, problems) {
        Ok(v) => (v, 0),
        Err(_) => (Vec::new(), 1),
    }
}

/// environment.py `_preload_secrets`: the known-value registry, preloaded at start (step 6a): the API keys in use
/// are known before this run first uses them (GATE-573). Values stay in memory; only counts are journaled. Blocking
/// (helper calls): the composition runs it off the event loop.
pub fn preload_secrets(host: &Host, s: &dyn Seams) -> Result<(), HandlerError> {
    let mut problems = Vec::new(); // GATE-547: journaled here
    let (values, unreadable) = keychain_values(s, &mut problems);
    for problem in problems {
        host.journal("keys.old_item_not_deleted", problem, None)?;
    }
    if !values.is_empty() || unreadable > 0 {
        let added = host.redactor.preload(values.iter().map(Secret::expose));
        host.journal(
            "secret.registry_preloaded",
            json!({"values": added, "unreadable": unreadable}),
            None,
        )?;
    }
    Ok(())
}

/// environment.py `_facts`: facts about this FERB that hold across turns (in every role's instructions, a cacheable
/// prefix).
pub fn facts(host: &Host, s: &dyn Seams) -> Result<String, HandlerError> {
    let address = get(host, "mailbox", "address")?.unwrap_or_else(|| Value::from(""));
    let mut lines = vec![s.about_summary()];
    if ferb_py::value::truthy(&address) {
        lines.push(format!(
            "- FERB's own email (its identity, never the owner's): {}",
            ferb_py::value::py_str(&address)
        ));
    } else {
        // APP-1: the owner skipped FERB's own account
        let chosen_cfg = get(host, "browsing", "owner_profile")?.unwrap_or_else(|| Value::from(""));
        let chosen_cfg = if ferb_py::value::truthy(&chosen_cfg) {
            ferb_py::value::py_str(&chosen_cfg)
        } else {
            String::new()
        };
        let (browser, folder) = chosen_cfg.split_once('/').unwrap_or((&chosen_cfg, ""));
        let tail = if !browser.is_empty() && !folder.is_empty() {
            let chosen = profiles::shown(
                &profiles::named(&s.fresh_profiles(), browser, folder),
                Some(""),
            );
            format!("in the owner's {browser} profile {chosen} (folder “{folder}”), which they chose for it.")
        } else {
            "in a profile the owner names (none chosen for it).".to_owned()
        };
        lines.push(format!(
            "- FERB has no Google account or browser profile of its own (the owner can add one in FERB.app → \
             Settings → Browsers): what needs FERB's own inbox (one-time codes for FERB's own sign-ups, sending \
             from FERB's address) isn't set up; web work uses identity \"owner\" {tail}"
        ));
    }
    for spec in s.secret_specs() {
        lines.push(format!(
            "- FERB secret {}: {}, usable on {}",
            spec.key,
            spec.account_hint,
            spec.allowed_origins.join(", ")
        ));
    }
    // DMN-511, DMN-512: the worker asks the owner once to install the tools when a task needs them; once they are
    // installed (a folder check) the fact is gone for good
    if !s.missing_dev_tools().is_empty() && s.dev_tools_installed() {
        s.clear_missing_dev_tools();
    }
    let fact = ferb_device::devtools::missing_fact(&s.missing_dev_tools());
    if !fact.is_empty() {
        lines.push(fact);
    }
    for (phrase, meaning) in s.time_meanings() {
        lines.push(format!(
            "- When the owner says “{phrase}”, they mean: {meaning}"
        ));
    }
    Ok(lines.join("\n"))
}

/// environment.py `_live_facts`: facts that change between turns (ORCH-325: after the conversation in the planner's
/// context, so the prefix before it stays cacheable). `owner_state` false (the intro's preview): only the browser
/// profiles. `last_runs` (the planner's turn, ORCH-592): each schedule's line ends with its last run.
pub fn live_facts(s: &dyn Seams, owner_state: bool, last_runs: bool) -> String {
    // ORCH-557: the AI FERB uses now, per role, from its settings
    let shown = s
        .models()
        .iter()
        .map(|(role, model)| format!("{role} {model}"))
        .collect::<Vec<_>>()
        .join("; ");
    let mut lines = vec![format!(
        "- The AI FERB uses now, from its settings (answer questions about FERB's AI or model from this, never \
         from memory): {shown}"
    )];
    if owner_state {
        // T-1066 and ORCH-577: the owner's schedules, reminders and timers as FERB's records hold them
        lines.extend(s.schedule_facts(last_runs).unwrap_or_default());
        lines.extend(s.reminder_facts().unwrap_or_default());
    }
    if let (Some(bindings), Some(hub)) = (s.pairings(), s.hub()) {
        let ferb = s.identity_profile();
        let mut found: Option<Vec<BrowserProfile>> = None;
        for b in &bindings {
            if Some(key_of(b)) == ferb || b.identity != Identity::Owner {
                continue;
            }
            let br = hub.bridge(&b.browser, &b.folder);
            // one profile discovery per call
            let found = found.get_or_insert_with(|| s.fresh_profiles());
            let name = profiles::shown(&profiles::named(found, &b.browser, &b.folder), Some(""));
            // ORCH-578: the extension's version as it last reported it (its live hello, else the pairing record)
            let live = br
                .as_ref()
                .filter(|br| br.connected)
                .map(|br| br.version.clone())
                .unwrap_or_default();
            let version = if live.is_empty() {
                s.pairing_version(&b.install_id)
            } else {
                live
            };
            let mut line = format!(
                "- The owner's {} profile {name}: paired{}",
                b.browser,
                paired_state(s, Some(hub.as_ref()), b, br.as_ref())
            );
            if !version.is_empty() {
                line.push_str(&format!(" (FERB extension {version})"));
            }
            lines.push(line);
        }
        let unpaired: BTreeSet<String> = hub.unpaired().into_iter().collect();
        for browser in unpaired {
            let note = s.unpaired_note(&browser);
            if !note.is_empty() {
                lines.push(format!("- {note}"));
            }
        }
    }
    lines.join("\n")
}

/// environment.py `_paired_state`: how the planner may use one paired owner profile now, in plain words.
pub fn paired_state(
    s: &dyn Seams,
    hub: Option<&dyn Hub>,
    binding: &Binding,
    bridge: Option<&Bridge>,
) -> String {
    if bridge.is_some_and(|b| b.connected) {
        return ", connected".into();
    }
    // HAND-406: e.g. too old
    if let Some(refused) = hub
        .and_then(|h| h.problem(binding))
        .filter(|r| !r.is_empty())
    {
        return format!(
            "; not connected: {refused} (only the owner can fix this; browser tools can't use it until then)"
        );
    }
    if s.router().is_some_and(|r| r.has_wake()) {
        return "; not connected now (usually the profile just isn't running) — FERB's browser tools start it in \
                the background and open its tab there themselves, nothing to ask"
            .into();
    }
    format!("; {IDLE_PROFILE}")
}
