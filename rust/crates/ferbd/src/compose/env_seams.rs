//! What the planner and workers are told about this Mac and FERB (`areas/environment.py`, ported in ferb-daemon's
//! `areas_b::environment`), over this composition's parts: the [`Seams`] that port reads through.
//!
//! Read from the real parts: FERB's own profile and the pairings (the browsers' store), installed and running apps,
//! the macOS version, the registered scopes, the TCC grants (when the Mac's parts are on), the roles' runtimes and
//! models, FERB's secrets, the owner's schedules, reminders and time meanings (group A), today's activity
//! (`activity/today.jsonl`).
//!
//! The browser drivers (`router`, `ferb_identity`) are the composition's ([`Tabs`]). Not read in this build, each
//! answering what Python's daemon answers without that part: the extension hub (`hub`: None, so no live connection
//! line), the
//! browser profiles discovered from Local State (`profiles`, `fresh_profiles`: none), FERB's own browser-free route
//! (`browsing`: not asked), the missing developer tools (none), an unpaired browser's note ("").

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use ferb_browser::pairing::Binding;
use ferb_browser::profiles::BrowserProfile;
use ferb_daemon::areas_b::core::Host;
use ferb_daemon::areas_b::environment::{Hub, Seams, SecretSpec, TabDriver, TaskRec};
use ferb_identity::Secret;
use serde_json::{json, Map, Value};

use super::tools::Parts;

/// One browser driver (the owner's router, FERB's own profile's) as the environment area reads it (`_TabDriver`).
pub struct Tabs(pub Arc<ferb_browser::driver::Driver>);

impl TabDriver for Tabs {
    fn task_tabs(&self, task_id: &str) -> Vec<Map<String, Value>> {
        self.0.task_tabs(task_id)
    }
    fn urls_of(&self, task_id: &str) -> Vec<String> {
        self.0.urls_of(task_id)
    }
    fn keep_tabs_open(&self, task_id: &str) {
        self.0.keep_tabs_open(task_id)
    }
    fn rest<'a>(
        &'a self,
        task_id: &'a str,
        resting: bool,
    ) -> ferb_core::interfaces::BoxFuture<'a, ()> {
        Box::pin(async move {
            // a failure never stops the loop (supervisor.py `_loop_rest`)
            let _ = self.0.rest(task_id, resting).await;
        })
    }
    fn release<'a>(
        &'a self,
        task_id: &'a str,
    ) -> ferb_core::interfaces::BoxFuture<
        'a,
        Result<Vec<Value>, ferb_daemon::areas_b::environment::ReleaseError>,
    > {
        Box::pin(async move {
            self.0
                .release(task_id)
                .await
                .map(|tabs| tabs.into_iter().map(Value::Object).collect())
                .map_err(|e| ferb_daemon::areas_b::environment::ReleaseError {
                    error: e.to_string(),
                    tabs: None,
                })
        })
    }
    fn has_wake(&self) -> bool {
        self.0.wake.is_some()
    }
}

/// The environment area's seams over the daemon's parts.
pub struct EnvSeams {
    pub host: Host,
    pub tools: Parts,
    pub runtimes: Arc<std::sync::OnceLock<ferb_daemon::areas_b::runtimes::Runtimes>>,
    pub a: Arc<std::sync::OnceLock<ferb_daemon::shared::Local<ferb_daemon::areas_a::State>>>,
    pub native: bool,
    pub activity: ferb_sense::daily::DailyActivity,
    pub support: PathBuf,
    pub browser_support: PathBuf,
}

impl EnvSeams {
    /// Group A's answer on its loop thread (None when it isn't running).
    fn on_a<T: Send + 'static>(
        &self,
        f: impl FnOnce(&ferb_daemon::areas_a::State) -> T + Send + 'static,
    ) -> Option<T> {
        let a = self.a.get()?;
        a.call_blocking(move |st| async move { f(&st) }).ok()
    }
}

impl Seams for EnvSeams {
    fn identity_profile(&self) -> Option<(String, String)> {
        ferb_daemon::areas_b::about::identity_profile(&self.host)
            .ok()
            .flatten()
    }
    fn pairings(&self) -> Option<Vec<Binding>> {
        self.tools.browsers().and_then(|b| b.pairings.all().ok())
    }
    fn pairing_version(&self, install_id: &str) -> String {
        self.tools
            .browsers()
            .and_then(|b| b.pairings.version(install_id).ok())
            .unwrap_or_default()
    }
    fn alignment(&self) -> Vec<Value> {
        // journal.of_running_tasks("security.alignment"): no running task writes it in this build's tools
        Vec::new()
    }
    fn profiles(&self) -> Vec<BrowserProfile> {
        Vec::new()
    }
    fn fresh_profiles(&self) -> Vec<BrowserProfile> {
        Vec::new()
    }
    fn installed_apps(&self) -> Vec<(String, String)> {
        ferb_daemon::areas_b::desktop::installed_apps(&self.host).unwrap_or_default()
    }
    fn running_bundle_ids(&self) -> Option<BTreeSet<String>> {
        if !self.native {
            return None;
        }
        Some(
            ferb_mac_hands::native::apps::running()
                .into_iter()
                .map(|a| a.bundle_id)
                .collect(),
        )
    }
    fn macos_version(&self) -> String {
        if self.native {
            crate::areas_c::macos_version()
        } else {
            String::new()
        }
    }
    fn browser_data_dir_override(&self, _browser: &str) -> Option<PathBuf> {
        None
    }
    fn support(&self) -> PathBuf {
        self.support.clone()
    }
    fn browser_support(&self) -> PathBuf {
        self.browser_support.clone()
    }
    fn profile_state(&self, _p: &BrowserProfile, extension: Option<&str>) -> String {
        extension.unwrap_or("missing").to_string()
    }
    fn worker_types(&self) -> Value {
        json!(self
            .runtimes
            .get()
            .map(|r| r.worker_types())
            .unwrap_or_default())
    }
    fn task(&self, task_id: &str) -> Option<TaskRec> {
        task_rec(&self.tools, task_id)
    }
    fn readable_tasks(&self, _tree: &str) -> Option<BTreeSet<String>> {
        None
    }
    fn router(&self) -> Option<Arc<dyn TabDriver>> {
        self.tools
            .owner_driver()
            .map(|d| Arc::new(Tabs(d)) as Arc<dyn TabDriver>)
    }
    fn ferb_identity(&self) -> Option<Arc<dyn TabDriver>> {
        self.tools
            .ferb_driver()
            .map(|d| Arc::new(Tabs(d)) as Arc<dyn TabDriver>)
    }
    fn activity_search(&self, words: &str) -> Vec<Map<String, Value>> {
        self.activity.search(words, 8)
    }
    fn registered_scopes(&self) -> BTreeSet<String> {
        self.tools
            .gateway()
            .map(|g| {
                g.registry()
                    .tools()
                    .iter()
                    .flat_map(|t| t.scopes())
                    .collect()
            })
            .unwrap_or_default()
    }
    fn identity_browser(&self) -> bool {
        self.tools.ferb_driver().is_some()
    }
    fn browsing(&self) -> Map<String, Value> {
        Map::new()
    }
    fn permissions(&self) -> BTreeMap<String, bool> {
        if !self.native {
            return BTreeMap::new();
        }
        let found = ferb_mac_hands::native::permissions::check_all();
        found
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_str() == Some("granted")))
                    .collect()
            })
            .unwrap_or_default()
    }
    fn runtime_for(&self, role: &str) -> String {
        self.runtimes
            .get()
            .map(|r| r.runtime_for(role))
            .unwrap_or_default()
    }
    fn stored_api_keys(
        &self,
        _providers: &[String],
        _problems: &mut Vec<Value>,
    ) -> Result<Vec<Secret>, String> {
        // the start's preload reads them (GroupB::preload_secrets); the facts never do
        Ok(Vec::new())
    }
    fn about_summary(&self) -> String {
        ferb_daemon::areas_b::about::about_summary(&self.host, super::VERSION).unwrap_or_default()
    }
    fn secret_specs(&self) -> Vec<SecretSpec> {
        let section = self.host.config.section("secrets").ok();
        ferb_identity::secrets::specs_from_config(section.as_ref())
            .into_iter()
            .map(|s| SecretSpec {
                key: s.secret.key,
                account_hint: s.account_hint,
                allowed_origins: s.allowed_origins,
            })
            .collect()
    }
    fn missing_dev_tools(&self) -> Vec<String> {
        Vec::new()
    }
    fn clear_missing_dev_tools(&self) {}
    fn dev_tools_installed(&self) -> bool {
        true
    }
    fn time_meanings(&self) -> Vec<(String, String)> {
        self.on_a(ferb_daemon::areas_a::memory::time_meanings)
            .unwrap_or_default()
    }
    fn models(&self) -> Vec<(String, String)> {
        ferb_daemon::areas_b::about::models(&self.host).unwrap_or_default()
    }
    fn schedule_facts(&self, last_runs: bool) -> Option<Vec<String>> {
        self.on_a(move |st| ferb_daemon::areas_a::schedules::schedule_facts(st, last_runs).ok())
            .flatten()
    }
    fn reminder_facts(&self) -> Option<Vec<String>> {
        self.on_a(ferb_daemon::areas_a::records::reminder_facts)
    }
    fn hub(&self) -> Option<Arc<dyn Hub>> {
        None
    }
    fn unpaired_note(&self, _browser: &str) -> String {
        String::new()
    }
}

/// The live record (`self.tasks.get`), with its task's role, parent, identity and scopes from the task store (Python's
/// record holds the `Task` itself); one device: this Mac's.
pub fn task_rec(tools: &Parts, task_id: &str) -> Option<TaskRec> {
    let rec = tools.task(task_id)?;
    let row: Option<(String, Option<String>, String, String)> = {
        let shared = tools.db();
        let db = crate::basics::lock(&shared);
        db.conn()
            .query_row(
                "SELECT role, parent_id, identity, scopes_json FROM tasks WHERE id = ?",
                [task_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .ok()
    };
    let (role, parent_id, identity, scopes) = row.unwrap_or_default();
    let scopes: BTreeSet<String> = serde_json::from_str(&scopes).unwrap_or_default();
    Some(TaskRec {
        id: rec.task_id,
        tree_id: rec.tree_id,
        request_id: Some(rec.request_id),
        role,
        parent_id,
        scopes,
        identity,
        device_id: String::new(),
    })
}

/// environment.py `_inspection_parent`: a registered checker reaches its worker's tabs and evidence.
pub fn inspection_parent(tools: &Parts, task_id: &str) -> Option<String> {
    let child = task_rec(tools, task_id)?;
    if child.role != "verifier" {
        return None;
    }
    let parent = child
        .parent_id
        .as_deref()
        .and_then(|p| task_rec(tools, p))?;
    if parent.role != "worker" || !child.scopes.is_subset(&parent.scopes) {
        return None;
    }
    let same = (
        child.request_id,
        child.identity,
        child.device_id,
        child.tree_id,
    ) == (
        parent.request_id.clone(),
        parent.identity.clone(),
        parent.device_id.clone(),
        parent.tree_id.clone(),
    );
    same.then_some(parent.id)
}
