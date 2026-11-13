//! What the planner and workers are told about this Mac and FERB (`areas/environment.py`, ported in ferb-daemon's
//! `areas_b::environment`), over this composition's parts: the [`Seams`] that port reads through.
//!
//! Read from the real parts: FERB's own profile and the pairings (the browsers' store), installed and running apps,
//! the macOS version, the registered scopes, the TCC grants (when the Mac's parts are on), the roles' runtimes and
//! models, FERB's secrets, the owner's schedules, reminders and time meanings (group A), today's activity
//! (`activity/today.jsonl`).
//!
//! Not read in this build, each answering what Python's daemon answers without that part: the extension hub and the
//! browser drivers' tabs (`hub`, `router`, `ferb_identity`: None, so no live connection line and no task tabs), the
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
        let rec = self.tools.task(task_id)?;
        Some(TaskRec {
            id: rec.task_id,
            tree_id: rec.tree_id,
            request_id: Some(rec.request_id),
            ..TaskRec::default()
        })
    }
    fn readable_tasks(&self, _tree: &str) -> Option<BTreeSet<String>> {
        None
    }
    fn router(&self) -> Option<Arc<dyn TabDriver>> {
        None
    }
    fn ferb_identity(&self) -> Option<Arc<dyn TabDriver>> {
        None
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
