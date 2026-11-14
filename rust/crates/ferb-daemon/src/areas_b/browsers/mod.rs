//! `ferb/daemon/areas/browsers.py`: pairing, the browser list and the setup window's Browsers step, the extension
//! state FERB reads from each profile's files, FERB's own identity profile, and the human-check handoff. Its 13
//! methods: browsers.list, browsers.pair, browsers.installed, browsers.unpair, setup.browser_pair,
//! setup.browsers_pair_all, setup.browsers_pairing (a stream), setup.browsers, setup.browser_open,
//! browser.show_handoff, identity.find, identity.choose, identity.pair.
//!
//! Must not choose a profile for a task: agents do. Everything native (a browser's launch, Launch Services, the
//! extension hub's connections, the router's tabs, banners) comes through the seams in [`Deps`]; building the Hub,
//! Router, Downloads, Uploads and registry is the daemon's composition (`_build_browsers`), not here.

pub mod steps;

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ferb_browser::autoinstall;
use ferb_browser::pairing::{Binding, Pairings};
use ferb_browser::profiles::{self, Binaries, BrowserProfile, Platform};
use ferb_core::types::Identity;
use ferb_device::extensions::{extension_path, store_page};
use ferb_rpc::{HandlerError, HandlerResult, Params, Server, Session};
use serde_json::{json, Map, Value};

use super::core::{object, refusal, Host, P};
use crate::areas_a::settings as app_settings;

/// DMN-507: what a browser call says while FERB may not look at other apps' data yet.
pub const WAITS_FOR_FULL_DISK_ACCESS: &str =
    "FERB can't look at your browsers until it has Full Disk Access (System \
Settings → Privacy & Security → Full Disk Access → FERB)";
/// HAND-407: why browser tools see only the paired profiles, by folder, while that is so.
pub const PROFILES_UNSEEN: &str = "FERB can't read browser profile names without Full Disk Access (System Settings → \
Privacy & Security → Full Disk Access → FERB), so only the profiles already paired with FERB are known, by their \
folder";
/// HAND-406: a browser connection FERB didn't take, with its reason.
pub const REFUSED: &str = "browser.connection_refused";
/// HAND-410: a secret-helper failure, with its status when known.
pub const HELPER_FAILED: &str = "secret.helper_failed";
/// item 7 / HAND-413: Pair all's skip reasons FERB's Web Store page fixes in one click.
pub const ADDABLE: [&str; 3] = ["removed", "not_added", "requested"];
/// HAND-412: a pairing page nobody answered while the profile's files say FERB is on and new enough.
pub const PAIR_UNANSWERED: &str = "FERB's pairing page got no answer there: {label} may have FERB turned off, \
updating or waiting for you to accept new permissions in that profile. Open chrome://extensions in that profile, make \
sure FERB is on and up to date, then try again";
/// The refusal for a `timeout_s` Python's `float()` refuses (fixed against Python: it was an internal error).
pub const BAD_TIMEOUT: &str = "timeout_s must be a number of seconds";

pub type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// One profile's bridge as the extension hub holds it (`Hub.bridges[(browser, folder)]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeView {
    pub browser: String,
    pub folder: String,
    pub identity: Identity,
    /// Which bridge object this is (Python's `is`): a new bridge for the same profile has another id.
    pub id: usize,
    /// Its connection is live (`br.conn`).
    pub connected: bool,
    /// The version the current connection's hello said ("" before any).
    pub version: String,
    /// Hellos attached so far.
    pub hellos: i64,
}

/// The extension hub's state and its connected profiles (ferb/act/browser/ext_bridge.py `Hub`).
pub trait Extensions: Send + Sync {
    /// Every bridge made so far, in the order made.
    fn bridges(&self) -> Vec<BridgeView>;
    /// `hub.bridges.get((browser, folder))`.
    fn bridge(&self, browser: &str, folder: &str) -> Option<BridgeView> {
        self.bridges()
            .into_iter()
            .find(|b| b.browser == browser && b.folder == folder)
    }
    /// `hub.unpaired.values()`: the browser of each unpaired FERB copy connected now.
    fn unpaired(&self) -> Vec<String>;
    /// `hub.unpaired_copies()` (APP-892).
    fn unpaired_copies(&self) -> BTreeMap<String, String>;
    /// `hub.problem(binding)` (HAND-406).
    fn problem(&self, binding: &Binding) -> Option<String>;
    /// `br.call(op, args, timeout)` on that profile's bridge; `Err` is the bridge's error's words.
    fn call(
        &self,
        browser: &str,
        folder: &str,
        op: &str,
        args: Value,
        timeout: f64,
    ) -> BoxFut<Result<Value, String>>;
}

impl Extensions for ferb_browser::bridge::Hub {
    fn bridges(&self) -> Vec<BridgeView> {
        ferb_browser::bridge::Hub::bridges(self)
            .iter()
            .map(|br| {
                let facts = br.facts();
                BridgeView {
                    browser: br.binding.browser.clone(),
                    folder: br.binding.folder.clone(),
                    identity: br.binding.identity,
                    id: Arc::as_ptr(br) as usize,
                    connected: facts.connected,
                    version: facts.version,
                    hellos: facts.hellos,
                }
            })
            .collect()
    }

    fn unpaired(&self) -> Vec<String> {
        ferb_browser::bridge::Hub::unpaired(self)
            .into_iter()
            .map(|(_, browser)| browser)
            .collect()
    }

    fn unpaired_copies(&self) -> BTreeMap<String, String> {
        ferb_browser::bridge::Hub::unpaired_copies(self)
    }

    fn problem(&self, binding: &Binding) -> Option<String> {
        ferb_browser::bridge::Hub::problem(self, binding)
    }

    fn call(
        &self,
        browser: &str,
        folder: &str,
        op: &str,
        args: Value,
        timeout: f64,
    ) -> BoxFut<Result<Value, String>> {
        match self.existing(browser, folder) {
            Some(br) => {
                let call = br.call(op, args, timeout, None);
                Box::pin(async move { call.await.map_err(|e| e.to_string()) })
            }
            None => Box::pin(async { Err("that profile has no bridge".to_string()) }),
        }
    }
}

/// The router's tabs, for a human check handed to the owner (ferb/act/browser/ext_bridge.py `Router`). A tab is
/// named by the router's own reference, opaque here.
pub trait Tabs: Send + Sync {
    /// `router.shows(ref, message)`: {open, shows, url, title}; `Err` is a DriverError (the extension reconnecting).
    fn shows(&self, tab: &Value, message: &str) -> BoxFut<Result<Value, String>>;
    /// `router.show_tab(ref)`: the tab came forward.
    fn show_tab(&self, tab: &Value) -> BoxFut<bool>;
    /// `router.tab_profile(ref)`: (browser, folder).
    fn tab_profile(&self, tab: &Value) -> (String, String);
}

/// `self._notify(title, text, urgent, category=, target=, ident=)` (areas/notify.py).
pub trait Notifier: Send + Sync {
    fn notify(
        &self,
        title: &str,
        text: &str,
        urgent: bool,
        category: &str,
        target: Value,
        ident: &str,
    ) -> BoxFut<()>;
}

/// FERB's own browser driver as the daemon attaches it (`use_identity_browser`, `_detach_identity_browser`).
pub trait IdentityBrowser: Send + Sync {
    /// `self.ferb_identity` is built.
    fn ready(&self) -> bool;
    /// `self.identity_browser` is set.
    fn attached(&self) -> bool;
    /// `use_identity_browser(self.ferb_identity)`: registers the `ferb_browser.*` tools.
    fn attach(&self);
    /// `_detach_identity_browser`: its tools go and the tabs it opened close; nothing when none is attached.
    fn detach(&self);
}

/// The owner's questions as `_browsing` asks them (`_owner_followups`, `state.approvals`, `_ask_owner`).
pub trait OwnerQuestions: Send + Sync {
    /// The open (pending) owner question of this kind, whichever task asked it.
    fn pending(&self, kind: &str) -> Option<String>;
    /// `_ask_owner(kind, ident, reason, key, options=...)`: the new question's approval id.
    fn ask(&self, kind: &str, ident: &str, reason: &str, key: &str, options: &[String]) -> String;
}

/// `supervisor.activity.waiting(worker_id, on)`.
pub type Activity = Arc<dyn Fn(&str, bool) + Send + Sync>;

/// A profile: (browser, folder).
pub type ProfileKey = (String, String);

/// Runs a browser command (Python's `browser_launch`, else `profiles._detached`).
pub type Launch = Arc<dyn Fn(&[String]) -> std::io::Result<()> + Send + Sync>;

/// FERB's native-host launcher and what it runs (`Installer.native_host_launcher`, `Installer.python_command()` /
/// `native_host_exec`), for the manifests `_write_browser_files` writes.
#[derive(Clone, Debug)]
pub struct NativeHost {
    pub launcher: PathBuf,
    pub exec_part: String,
}

/// What this area keeps between calls (the daemon's `_browser_files_done`, `_browser_files_lock`, `_handoffs`).
#[derive(Default)]
pub struct State {
    files_done: Mutex<Option<FilesKey>>,
    files_lock: tokio::sync::Mutex<()>,
    handoffs: Mutex<HashMap<String, Value>>,
}

type FilesKey = (Vec<String>, Vec<String>, String);

/// The seams of the browser area.
#[derive(Clone)]
pub struct Deps {
    /// The pairing records and tokens (ferb-browser `Pairings`).
    pub pairings: Arc<Pairings>,
    /// The extension hub's state.
    pub hub: Arc<dyn Extensions>,
    /// DMN-507: the daemon's one gate, Full Disk Access granted (`data_access`).
    pub data_access: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Launch Services and processes (`has_app`, `locate`, the https handlers).
    pub platform: Arc<dyn Platform>,
    /// Each browser's executable, remembered.
    pub binaries: Arc<Binaries>,
    /// Where browsers keep their user data (~/Library/Application Support).
    pub browser_support: PathBuf,
    /// browser -> user-data folder replacing discovery (tests; `browser_data_dirs`).
    pub browser_data_dirs: Option<Vec<(String, PathBuf)>>,
    /// Runs a browser command.
    pub launch: Launch,
    /// The router's tabs; None before the router is built.
    pub tabs: Option<Arc<dyn Tabs>>,
    /// The owner's banners.
    pub notifier: Arc<dyn Notifier>,
    /// `supervisor.activity.waiting(worker_id, on)`; None without a supervisor.
    pub activity: Option<Activity>,
    /// FERB's own browser driver.
    pub identity_browser: Arc<dyn IdentityBrowser>,
    /// The native host the manifests name.
    pub native_host: NativeHost,
    /// seed.py's `CHROME_DATA`: Chrome's folder when FERB's browser has no discovered folder.
    pub chrome_data: PathBuf,
    /// A new handoff id (`uuid.uuid4().hex[:12]`).
    pub new_handoff_id: Arc<dyn Fn() -> String + Send + Sync>,
    /// A profile opened just to pair gets this long to load.
    pub pair_load_wait_s: f64,
    /// How long a task's pairing page may take (`pair_timeout_s`).
    pub pair_timeout_s: f64,
    /// How often a handed-off check's page is looked at.
    pub handoff_poll_s: f64,
    pub state: Arc<State>,
}

/// The host and the seams, as every helper takes them.
#[derive(Clone)]
pub struct Cx {
    pub host: Host,
    pub deps: Deps,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn store_err(e: ferb_browser::pairing::StoreError) -> HandlerError {
    HandlerError::fault("DatabaseError", e.0)
}

fn os_err(e: impl std::fmt::Display) -> HandlerError {
    HandlerError::fault("OSError", e.to_string())
}

/// Python's `float(value)` for `timeout_s`; a value it refuses is [`BAD_TIMEOUT`] (Python: an internal error).
pub fn timeout_s(params: &Params, default: f64) -> Result<f64, HandlerError> {
    let refused = || refusal(BAD_TIMEOUT);
    match params.get("timeout_s") {
        None => Ok(default),
        Some(Value::Number(n)) => n.as_f64().ok_or_else(refused),
        Some(Value::Bool(b)) => Ok(if *b { 1.0 } else { 0.0 }),
        Some(Value::String(s)) => {
            let t = ferb_py::value::strip(s);
            let chars: Vec<char> = t.chars().collect();
            let mut cleaned = String::new();
            for (i, c) in chars.iter().enumerate() {
                if *c == '_' {
                    let digit = |j: Option<&char>| j.is_some_and(|c| c.is_ascii_digit());
                    if i == 0 || !digit(chars.get(i - 1)) || !digit(chars.get(i + 1)) {
                        return Err(refused());
                    }
                    continue;
                }
                cleaned.push(*c);
            }
            cleaned.parse::<f64>().map_err(|_| refused())
        }
        Some(_) => Err(refused()),
    }
}

/// `{**a, **b}` as Python's dict `|`.
fn merge(mut a: Map<String, Value>, b: Value) -> Map<String, Value> {
    for (k, v) in object(b) {
        a.insert(k, v);
    }
    a
}

fn label_of(browser: &str) -> String {
    profiles::browser(browser).map_or_else(|| browser.to_string(), |b| b.label.to_string())
}

impl Cx {
    pub fn new(host: &Host, deps: &Deps) -> Cx {
        Cx {
            host: host.clone(),
            deps: deps.clone(),
        }
    }

    /// Blocking file work in a worker thread (DMN-13).
    async fn blocking<T: Send + 'static>(&self, f: impl FnOnce(&Cx) -> T + Send + 'static) -> T {
        let cx = self.clone();
        match tokio::task::spawn_blocking(move || f(&cx)).await {
            Ok(v) => v,
            Err(e) => std::panic::resume_unwind(e.into_panic()),
        }
    }

    fn config_text(&self, section: &str, key: &str) -> Result<String, HandlerError> {
        Ok(match self.host.get(section, key)? {
            Some(v) if ferb_py::value::truthy(&v) => ferb_py::value::py_str(&v),
            _ => String::new(),
        })
    }

    /// `autoinstall.store_extension_id(config)`.
    pub fn store_id(&self) -> Result<String, HandlerError> {
        Ok(autoinstall::store_extension_id(
            self.host.get("browser", "store_extension_id")?.as_ref(),
        ))
    }

    /// `autoinstall.accepted_extension_ids(config)`.
    pub fn accepted_ids(&self) -> Result<Vec<String>, HandlerError> {
        let store = self.store_id()?;
        Ok(autoinstall::accepted_extension_ids(
            self.host.get("browser", "extension_ids")?.as_ref(),
            &store,
        ))
    }

    /// `_other_app_data` (DMN-507): FERB may read or write another app's data now.
    pub fn other_app_data(&self) -> bool {
        (self.deps.data_access)()
    }

    /// `_browser_data`: each installed browser's user-data folder; none before Full Disk Access.
    pub fn browser_data(&self) -> Vec<(String, PathBuf)> {
        if !self.other_app_data() {
            return vec![];
        }
        let platform = self.deps.platform.as_ref();
        match &self.deps.browser_data_dirs {
            Some(dirs) => dirs
                .iter()
                .filter(|(b, _)| profiles::app_installed(platform, b))
                .cloned()
                .collect(),
            None => profiles::installed(platform, &self.deps.browser_support)
                .into_iter()
                .filter_map(|b| {
                    profiles::data_dir(b, &self.deps.browser_support).map(|d| (b.to_string(), d))
                })
                .collect(),
        }
    }

    /// `_profiles`: every discovered profile (none before Full Disk Access).
    pub fn profiles(&self) -> Vec<BrowserProfile> {
        if !self.other_app_data() {
            return vec![];
        }
        let platform = self.deps.platform.as_ref();
        self.browser_data()
            .iter()
            .filter(|(b, _)| profiles::app_installed(platform, b))
            .flat_map(|(b, d)| profiles::discover(b, d))
            .collect()
    }

    /// `_fresh_profiles`: the profiles as last discovered. The composition's `_Fresh` cache is not applied here:
    /// this reads them now.
    pub fn fresh_profiles(&self) -> Vec<BrowserProfile> {
        self.profiles()
    }

    /// A browser's folder for reading one profile's files: the test mapping's, else under the support folder.
    fn data_dir_of(&self, browser: &str) -> PathBuf {
        self.deps
            .browser_data_dirs
            .as_ref()
            .and_then(|d| d.iter().find(|(b, _)| b == browser).map(|(_, p)| p.clone()))
            .or_else(|| profiles::data_dir(browser, &self.deps.browser_support))
            .unwrap_or_else(|| self.deps.browser_support.join(browser))
    }

    /// `_identity_profile`: FERB's own profile (browser, folder).
    pub fn identity_profile(&self) -> Result<Option<(String, String)>, HandlerError> {
        let folder = self.host.get("identity", "profile")?;
        match folder {
            Some(f) if ferb_py::value::truthy(&f) => {
                let browser = self.host.get_or("identity", "browser", json!("chrome"))?;
                Ok(Some((
                    ferb_py::value::py_str(&browser),
                    ferb_py::value::py_str(&f),
                )))
            }
            _ => Ok(None),
        }
    }

    /// `_owner_browsing_profile` (APP-1): the owner's profile FERB browses in while it has none of its own.
    pub fn owner_browsing_profile(&self) -> Result<Option<(String, String)>, HandlerError> {
        let text = self.config_text("browsing", "owner_profile")?;
        let (browser, folder) = text.split_once('/').unwrap_or((text.as_str(), ""));
        Ok((!browser.is_empty() && !folder.is_empty())
            .then(|| (browser.to_string(), folder.to_string())))
    }

    /// `_profile_state`: connected / paired / installed / off / removed / missing.
    pub fn profile_state(
        &self,
        p: &BrowserProfile,
        extension: Option<&str>,
    ) -> Result<&'static str, HandlerError> {
        if self
            .deps
            .pairings
            .for_profile(&p.browser, &p.folder)
            .map_err(store_err)?
            .is_some()
        {
            let br = self.deps.hub.bridge(&p.browser, &p.folder);
            return Ok(if br.is_some_and(|b| b.connected) {
                "connected"
            } else {
                "paired"
            });
        }
        let state = match extension {
            Some(e) => e.to_string(),
            None => {
                let ids = self.accepted_ids()?;
                let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
                profiles::extension_state(&self.data_dir_of(&p.browser).join(&p.folder), &refs)
                    .to_string()
            }
        };
        Ok(match state.as_str() {
            "on" => "installed",
            "off" => "off",
            "removed" => "removed",
            _ => "missing",
        })
    }

    /// `_extension_stands` (HAND-412, HAND-449): why this profile's FERB copy can't answer a pairing page, from its
    /// files now; None: on and new enough as far as its files say. Blocking.
    pub fn extension_stands(
        &self,
        p: &BrowserProfile,
    ) -> Result<Option<(String, String)>, HandlerError> {
        let data = self.data_dir_of(&p.browser);
        let store = self.store_id()?;
        let ids = self.accepted_ids()?;
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let st = autoinstall::profile_status(p, &refs, &data, None, &store);
        let requested = autoinstall::valid_id(&store)
            && data
                .join("External Extensions")
                .join(format!("{store}.json"))
                .is_file();
        Ok(steps::not_paired_step(
            &st,
            &label_of(&p.browser),
            requested,
            &store_page(&store),
            &extension_path(&self.host.home),
        ))
    }

    /// `_identity_paired`: FERB's own profile is paired as FERB.
    pub fn identity_paired(&self) -> Result<bool, HandlerError> {
        let Some((b, f)) = self.identity_profile()? else {
            return Ok(false);
        };
        Ok(self
            .deps
            .pairings
            .for_profile(&b, &f)
            .map_err(store_err)?
            .is_some_and(|b| b.identity == Identity::Ferb))
    }

    /// `_attach_identity_if_paired`: FERB's tools exist only once its identity profile is paired as FERB.
    pub fn attach_identity_if_paired(&self) -> Result<bool, HandlerError> {
        if !self.identity_paired()? || !self.deps.identity_browser.ready() {
            return Ok(false);
        }
        if !self.deps.identity_browser.attached() {
            self.deps.identity_browser.attach();
        }
        Ok(true)
    }

    /// `_paired_problem` (HAND-406): a paired profile that isn't connected and that FERB knows why.
    pub fn paired_problem(
        &self,
        browser: &str,
        folder: &str,
    ) -> Result<Option<(String, String)>, HandlerError> {
        let Some(b) = self
            .deps
            .pairings
            .for_profile(browser, folder)
            .map_err(store_err)?
        else {
            return Ok(None);
        };
        if self
            .deps
            .hub
            .bridge(browser, folder)
            .is_some_and(|br| br.connected)
        {
            return Ok(None);
        }
        Ok(self
            .deps
            .hub
            .problem(&b)
            .map(|why| ("problem".to_string(), why)))
    }

    /// `_profile_problems` (HAND-406): why each paired profile that isn't connected isn't, when FERB knows.
    pub fn profile_problems(&self) -> Result<Vec<(ProfileKey, String)>, HandlerError> {
        let mut out: Vec<(ProfileKey, String)> = vec![];
        for b in self.deps.pairings.all().map_err(store_err)? {
            let connected = self
                .deps
                .hub
                .bridge(&b.browser, &b.folder)
                .is_some_and(|br| br.connected);
            if !connected {
                if let Some(why) = self.deps.hub.problem(&b) {
                    let key = (b.browser.clone(), b.folder.clone());
                    match out.iter_mut().find(|(k, _)| *k == key) {
                        Some(slot) => slot.1 = why,
                        None => out.push((key, why)),
                    }
                }
            }
        }
        Ok(out)
    }

    /// `_extension_problems`: the same, in the owner's words for setup's message.
    async fn extension_problems(&self) -> Result<Vec<String>, HandlerError> {
        let why = self.profile_problems()?;
        if why.is_empty() {
            return Ok(vec![]);
        }
        let found = self.blocking(|c| c.profiles()).await;
        Ok(why
            .iter()
            .map(|((b, f), text)| {
                format!(
                    "{}: {text}.",
                    profiles::shown(&profiles::named(&found, b, f), None)
                )
            })
            .collect())
    }

    /// `_owner_extension_connected`: a paired owner profile of this browser is connected.
    pub fn owner_extension_connected(&self, browser: &str) -> bool {
        self.deps
            .hub
            .bridges()
            .iter()
            .any(|br| br.connected && br.browser == browser && br.identity == Identity::Owner)
    }

    /// `_unpaired_note`: the FERB extension is installed and connected in this browser but not paired.
    pub fn unpaired_note(&self, browser: &str) -> Option<String> {
        if !self.deps.hub.unpaired().iter().any(|b| b == browser)
            || self.owner_extension_connected(browser)
        {
            return None;
        }
        let name = match browser {
            "brave" => "Brave",
            "chrome" => "Chrome",
            other => other,
        };
        Some(format!(
            "The FERB extension is installed in {name} but not paired: a task pairs the profile it needs with \
             browser_pair (FERB asks the owner once); the owner can also pair it in Settings → Browsers."
        ))
    }

    /// `_profile_display_name`.
    pub fn profile_display_name(&self, browser: &str, folder: &str) -> String {
        self.profiles()
            .into_iter()
            .find(|p| p.browser == browser && p.folder == folder)
            .map_or_else(|| folder.to_string(), |p| p.display_name)
    }

    /// `_profile_account` (HAND-424): the account that profile is signed in to, read fresh; "" when none.
    pub fn profile_account(&self, browser: &str, folder: &str) -> String {
        self.profiles()
            .into_iter()
            .find(|p| p.browser == browser && p.folder == folder)
            .map(|p| p.email)
            .unwrap_or_default()
    }

    /// `_browsing_profiles`: the owner's profiles FERB knows now (discovered, plus paired ones it can't read),
    /// without FERB's own.
    pub fn browsing_profiles(&self) -> Result<Vec<BrowserProfile>, HandlerError> {
        let ferb = self.identity_profile()?;
        let paired: Vec<(String, String)> = self
            .deps
            .pairings
            .all()
            .map_err(store_err)?
            .into_iter()
            .map(|b| (b.browser, b.folder))
            .collect();
        Ok(
            profiles::with_paired(self.deps.platform.as_ref(), &self.fresh_profiles(), &paired)
                .into_iter()
                .filter(|p| Some((p.browser.clone(), p.folder.clone())) != ferb)
                .collect(),
        )
    }

    /// `browser_profiles.open_known_profile(browser, folder, url, known, background=False, window=…)` through the
    /// launcher; `Err` is Python's ValueError's words.
    fn open_known(
        &self,
        browser: &str,
        folder: &str,
        url: Option<&str>,
        known: &[BrowserProfile],
        window: bool,
    ) -> Result<Result<(), String>, HandlerError> {
        let binary = profiles::browser(browser)
            .map(|b| self.deps.binaries.binary(self.deps.platform.as_ref(), b))
            .unwrap_or_default();
        match profiles::open_command(browser, folder, url, known, &binary, false, false, window) {
            Err(why) => Ok(Err(why)),
            Ok(args) => {
                (self.deps.launch)(&args).map_err(os_err)?;
                Ok(Ok(()))
            }
        }
    }

    /// `_open_pairing`: the extension's pairing page in that profile, which closes itself once it has handed over
    /// its token (pair.js). A profile that isn't connected is loaded first with no window (`--no-startup-window`, as
    /// the drivers' wake loads one: HAND-403), so its extension is running when the page arrives.
    ///
    /// Fixed against Python (ENG-6): Python loaded it with a blank local page (`WakeServer`) and closed that page
    /// only after a pairing that succeeded (`wake_close`), so every failed, refused, timed-out or abandoned pairing
    /// left a "FERB" tab behind; on the owner's first Rust setup (2026-11-14) every profile kept one. FERB now opens
    /// no tab but the self-closing pairing page.
    async fn open_pairing(
        &self,
        browser: &str,
        folder: &str,
        token: &str,
    ) -> Result<(), HandlerError> {
        let value_error = |why: String| HandlerError::fault("ValueError", why);
        let known = self.blocking(|c| c.profiles()).await;
        if !self
            .deps
            .hub
            .bridge(browser, folder)
            .is_some_and(|b| b.connected)
        {
            self.open_known(browser, folder, None, &known, false)?
                .map_err(value_error)?;
            sleep_s(self.deps.pair_load_wait_s).await;
        }
        let ids = self.accepted_ids()?;
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let found = known
            .iter()
            .find(|p| p.browser == browser && p.folder == folder)
            .and_then(|p| {
                profiles::extension_entry(&self.data_dir_of(browser).join(&p.folder), &refs)
            });
        let ext = found.map_or_else(|| ids[0].clone(), |f| f.0);
        let page = format!("chrome-extension://{ext}/pair.html#token={token}");
        self.open_known(browser, folder, Some(&page), &known, false)?
            .map_err(value_error)
    }

    /// `_answered_but_refused` (HAND-412): the pairing page was answered and FERB then refused that profile's hello.
    fn answered_but_refused(
        &self,
        p: &BrowserProfile,
        token: &str,
    ) -> Result<Option<(String, String)>, HandlerError> {
        if !self.deps.pairings.was_paired(token) {
            return Ok(None);
        }
        let b = self
            .deps
            .pairings
            .for_profile(&p.browser, &p.folder)
            .map_err(store_err)?;
        Ok(b.and_then(|b| self.deps.hub.problem(&b))
            .map(|why| ("problem".to_string(), why)))
    }

    /// `_exact_profile`: the discovered profile with exactly that browser and folder, or None when no folder is named.
    fn exact_profile(&self, params: &Params) -> Result<Option<BrowserProfile>, HandlerError> {
        let p = P(params);
        let (browser, folder) = (p.text_or_empty("browser"), p.text_or_empty("folder"));
        if folder.is_empty() {
            return Ok(None);
        }
        match self
            .profiles()
            .into_iter()
            .find(|p| p.browser == browser && p.folder == folder)
        {
            Some(p) => Ok(Some(p)),
            None => Err(refusal(format!(
                "no {browser} profile in folder {} on this Mac",
                ferb_identity::py::repr(&folder)
            ))),
        }
    }

    /// `browser_profiles.resolve(str(params.get("profile", "")), known, browser=params.get("browser"))`, one hit or
    /// the refusal in its words.
    fn resolved(
        &self,
        params: &Params,
        known: &[BrowserProfile],
    ) -> Result<BrowserProfile, HandlerError> {
        let name = P(params).text("profile", "");
        let pool: Vec<BrowserProfile> = match params.get("browser") {
            None | Some(Value::Null) => known.to_vec(),
            Some(Value::String(b)) => known.iter().filter(|p| p.browser == *b).cloned().collect(),
            Some(_) => vec![],
        };
        let r = profiles::resolve(&name, &pool, &[], None);
        if r.kind != "one" {
            return Err(refusal(r.message(&name, known)));
        }
        Ok(r.candidates[0].clone())
    }

    /// `_pair_one`: the extension's own pairing page, with a one-time token, opened in that profile; done once that
    /// token is used and the profile's extension is connected. A page left unanswered revokes its token (so does a
    /// caller that leaves: the token is revoked when this is dropped).
    pub async fn pair_one(
        &self,
        p: BrowserProfile,
        timeout: f64,
    ) -> Result<Map<String, Value>, HandlerError> {
        let base = object(
            json!({"browser": p.browser, "folder": p.folder, "name": p.display_name,
                                 "email": p.email}),
        );
        if Some((p.browser.clone(), p.folder.clone())) == self.identity_profile()? {
            return Ok(merge(
                base,
                json!({"paired": false, "why": "FERB's own profile is paired as FERB"}),
            ));
        }
        let state = self.profile_state(&p, None)?;
        let flag = match state {
            "missing" => json!({"needs_install": true}),
            "off" => json!({"needs_enable": true}),
            _ => json!({}),
        };
        let q = p.clone();
        if let Some((reason, why)) = self.blocking(move |c| c.extension_stands(&q)).await? {
            return Ok(merge(
                merge(base, flag),
                json!({"paired": false, "reason": reason, "why": why}),
            ));
        }
        if state == "missing" {
            return Ok(merge(
                merge(base, flag),
                json!({"paired": false, "why": "FERB's extension isn't in that profile"}),
            ));
        }
        if state == "off" {
            return Ok(merge(
                merge(base, flag),
                json!({"paired": false,
                       "why": "FERB's extension is in that profile but turned off: the owner turns it on there \
                               (Chrome shows “Action Required”, or the switch on its extensions page), then pair"}),
            ));
        }
        let token = self
            .deps
            .pairings
            .issue_token(&p.browser, &p.folder, Identity::Owner);
        let _revoke = Revoke(self.deps.pairings.clone(), token.clone());
        self.open_pairing(&p.browser, &p.folder, &token).await?;
        let clock = self.host.clock.clone();
        let end = clock.monotonic() + timeout;
        let mut recheck = clock.monotonic() + self.deps.pair_load_wait_s;
        while clock.monotonic() < end {
            let connected = self
                .deps
                .hub
                .bridge(&p.browser, &p.folder)
                .is_some_and(|b| b.connected);
            if self.deps.pairings.was_paired(&token) && connected {
                self.host.journal(
                    "browsers.paired",
                    json!({"browser": p.browser, "folder": p.folder}),
                    None,
                )?;
                return Ok(merge(base, json!({"paired": true})));
            }
            let mut why = self.answered_but_refused(&p, &token)?;
            if why.is_none() && clock.monotonic() >= recheck {
                recheck = clock.monotonic() + self.deps.pair_load_wait_s;
                let q = p.clone();
                why = self.blocking(move |c| c.extension_stands(&q)).await?;
            }
            if let Some((reason, text)) = why {
                self.deps.pairings.revoke(&token);
                self.host.journal(
                    "browsers.pair_failed",
                    json!({"browser": p.browser, "folder": p.folder, "reason": reason}),
                    None,
                )?;
                return Ok(merge(
                    base,
                    json!({"paired": false, "reason": reason, "why": text}),
                ));
            }
            sleep_s(0.2).await;
        }
        self.deps.pairings.revoke(&token);
        let q = p.clone();
        let stands = self.blocking(move |c| c.extension_stands(&q)).await?;
        let mut failed = object(json!({"browser": p.browser, "folder": p.folder,
                                       "why": "the pairing page wasn't answered"}));
        if let Some((reason, _)) = &stands {
            failed.insert("reason".into(), json!(reason));
        }
        self.host
            .journal("browsers.pair_failed", Value::Object(failed), None)?;
        if let Some((reason, why)) = stands {
            return Ok(merge(
                base,
                json!({"paired": false, "reason": reason, "why": why}),
            ));
        }
        Ok(merge(
            base,
            json!({"paired": false, "reason": "no_answer",
                   "hint": "is the FERB extension loaded and enabled in that profile?",
                   "why": PAIR_UNANSWERED.replace("{label}", &label_of(&p.browser))}),
        ))
    }

    /// `_pair_for_task`: a task's browser_pair, after the owner's Yes: that exact profile.
    pub async fn pair_for_task(
        &self,
        browser: &str,
        folder: &str,
    ) -> Result<Map<String, Value>, HandlerError> {
        let mut params = Params::new();
        params.insert("browser".into(), json!(browser));
        params.insert("folder".into(), json!(folder));
        let found = self.exact_profile(&params)?;
        // Python passed None on to `_pair_one` (AttributeError) when no folder was named
        let p = found.ok_or_else(|| refusal("name the profile's browser and folder"))?;
        self.pair_one(p, self.deps.pair_timeout_s).await
    }

    /// `_connect_browsers` (DMN-507, DMN-508): FERB's native host and the Web Store install request in every
    /// installed browser; written again only when the installed browsers or the accepted ids changed, or the last
    /// try failed. A browser folder that refused is reported and journaled.
    pub async fn connect_browsers(&self) -> Result<Map<String, Value>, HandlerError> {
        if !self.blocking(|c| c.other_app_data()).await {
            return Ok(object(json!({"state": "waiting"})));
        }
        let state = self.deps.state.clone();
        let result = {
            let _held = state.files_lock.lock().await;
            self.blocking(|c| c.write_browser_files()).await?
        };
        if result.get("written") == Some(&Value::Bool(true)) {
            let mut data = Map::new();
            for k in ["hosts", "requests", "failed"] {
                data.insert(k.into(), result.get(k).cloned().unwrap_or(Value::Null));
            }
            self.host
                .journal("browser.files_written", Value::Object(data), None)?;
        }
        Ok(result)
    }

    /// `_write_browser_files`: the file work of `connect_browsers`, in a worker thread.
    fn write_browser_files(&self) -> Result<Map<String, Value>, HandlerError> {
        let support = &self.deps.browser_support;
        let (ids, store) = match (self.accepted_ids(), self.store_id()) {
            (Ok(ids), Ok(store)) => (ids, store),
            (Err(e), _) | (_, Err(e)) => {
                return Ok(object(json!({"state": "failed", "written": false,
                    "problem": format!("FERB couldn't set up your browsers: {}", e.message)})));
            }
        };
        let installed = profiles::installed(self.deps.platform.as_ref(), support);
        let key: FilesKey = (
            installed.iter().map(|s| s.to_string()).collect(),
            ids.clone(),
            store.clone(),
        );
        if lock(&self.deps.state.files_done).as_ref() == Some(&key) {
            return Ok(object(json!({"state": "connected", "written": false})));
        }
        let home = &self.host.home;
        let dirs = autoinstall::native_host_dirs(support, &installed);
        let (_, failed_hosts) = ferb_device::install::write_native_hosts(
            home,
            &self.deps.native_host.launcher,
            &self.deps.native_host.exec_part,
            &ids,
            &dirs,
        )
        .map_err(|e| HandlerError::fault(e.type_name(), e.to_string()))?;
        let keep = if autoinstall::valid_id(&store) {
            store.as_str()
        } else {
            ""
        };
        autoinstall::withdraw(home, keep).map_err(os_err)?;
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let states = autoinstall::prepare(
            self.deps.platform.as_ref(),
            support,
            &store,
            &refs,
            None,
            Some(home),
        )
        .map_err(os_err)?;
        let label_for_dir = |d: &Path| -> String {
            profiles::BROWSERS
                .iter()
                .find(|b| profiles::data_dir(b.key, support).as_deref() == Some(d))
                .map(|b| b.label.to_string())
                .unwrap_or_else(|| {
                    d.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                })
        };
        let mut failed: Vec<Value> = failed_hosts
            .iter()
            .map(|(d, why)| {
                let parent = d.parent().unwrap_or(d);
                json!({"browser": label_for_dir(parent),
                       "file": ferb_device::install::NATIVE_HOST_MANIFEST, "error": why})
            })
            .collect();
        failed.extend(states.iter().filter(|s| !s.error.is_empty()).map(|s| {
            json!({"browser": label_of(&s.browser), "file": "install request", "error": s.error})
        }));
        *lock(&self.deps.state.files_done) = if failed.is_empty() { Some(key) } else { None };
        let problem = if failed.is_empty() {
            String::new()
        } else {
            let unique = |k: &str| {
                let mut seen: Vec<String> = vec![];
                for f in &failed {
                    let v = f[k].as_str().unwrap_or_default().to_string();
                    if !seen.contains(&v) {
                        seen.push(v);
                    }
                }
                seen
            };
            format!(
                "FERB couldn't connect {} ({}); it tries again the next time it starts or you open Browsers.",
                unique("browser").join(", "),
                unique("error").join("; ")
            )
        };
        Ok(object(json!({
            "state": if failed.is_empty() { "connected" } else { "failed" },
            "written": true,
            "hosts": dirs.len() - failed_hosts.len(),
            "requests": states.iter().filter(|s| s.request.is_some()).count(),
            "failed": failed,
            "problem": problem,
        })))
    }

    /// `_identity_candidates`: every browser profile signed into `address`, and whether it holds that account alone.
    fn identity_candidates(&self, address: &str) -> Vec<Value> {
        let mut out = vec![];
        for (browser, data) in self.browser_data() {
            let Ok(found) = ferb_identity::seed::profiles(&data) else {
                continue;
            };
            let signed: Vec<(String, String)> = profiles::discover(&browser, &data)
                .into_iter()
                .map(|p| (p.folder, p.email))
                .collect();
            for p in found {
                if p.accounts.iter().any(|a| a == address) {
                    let email = signed
                        .iter()
                        .find(|(f, _)| *f == p.folder)
                        .map(|(_, e)| e.clone())
                        .unwrap_or_default();
                    out.push(
                        json!({"browser": browser, "browser_name": label_of(&browser),
                                    "folder": p.folder, "name": p.name, "email": email,
                                    "only": p.accounts == [address.to_string()],
                                    "other_accounts": p.accounts.len() as i64 - 1}),
                    );
                }
            }
        }
        out
    }

    /// `_web_secrets` (L13): ask every connected profile of this browser for the mask state of its active tab of
    /// its window at these bounds; exactly one must own such a window.
    pub async fn web_secrets(
        &self,
        browser: &str,
        bounds: (f64, f64, f64, f64),
        title: &str,
    ) -> ferb_mac_hands::mask::WebSecrets {
        use ferb_mac_hands::mask::{same_title, WebSecrets};
        let none = |why: &str, late: bool| WebSecrets {
            state: None,
            why: why.to_string(),
            late,
            tab: None,
        };
        let bridges: Vec<BridgeView> = self
            .deps
            .hub
            .bridges()
            .into_iter()
            .filter(|b| b.connected && b.browser == browser)
            .collect();
        if bridges.is_empty() {
            return none(
                &format!("no paired profile of {browser} is connected"),
                false,
            );
        }
        let args = json!({"bounds": [bounds.0, bounds.1, bounds.2, bounds.3], "title": title});
        let asks: Vec<_> = bridges
            .iter()
            .map(|b| {
                self.deps
                    .hub
                    .call(&b.browser, &b.folder, "window_secrets", args.clone(), 4.0)
            })
            .collect();
        let mut answers: Vec<Option<Value>> = vec![];
        let handles: Vec<_> = asks.into_iter().map(tokio::spawn).collect();
        for h in handles {
            answers.push(h.await.ok().and_then(Result::ok));
        }
        let matched = |r: &Option<Value>| -> bool {
            matches!(r, Some(Value::Object(o)) if o.get("matched").is_some_and(ferb_py::value::truthy))
        };
        let mut owners: Vec<&Value> = answers
            .iter()
            .filter(|r| matched(r))
            .filter_map(Option::as_ref)
            .collect();
        if owners.len() > 1 && !title.is_empty() {
            let same: Vec<&Value> = owners
                .iter()
                .copied()
                .filter(|r| {
                    r.get("tab")
                        .and_then(|t| t.get("title"))
                        .and_then(Value::as_str)
                        .is_some_and(|t| same_title(t, title))
                })
                .collect();
            if !same.is_empty() {
                owners = same;
            }
        }
        if owners.len() == 1 {
            return WebSecrets::from_reply(owners[0]);
        }
        if !owners.is_empty() {
            return none(
                "more than one paired profile has a window at this window's place",
                false,
            );
        }
        if answers.iter().any(|r| {
            matches!(r, Some(Value::Object(o)) if o.get("reason") == Some(&json!("ambiguous_window")))
        }) {
            return none(
                "a paired profile has more than one window at this window's place",
                false,
            );
        }
        if answers.iter().any(|r| !matches!(r, Some(Value::Object(_)))) {
            return none("a paired profile didn't answer", true);
        }
        none(
            "no paired profile has a window at this window's place (another profile, or a private window)",
            false,
        )
    }
}

/// Revokes a pairing token when the wait for it ends however it ends (Python's revocations, and a caller that left:
/// `CancelledError`). Revoking a token already used changes nothing.
struct Revoke(Arc<Pairings>, String);

impl Drop for Revoke {
    fn drop(&mut self) {
        self.0.revoke(&self.1);
    }
}

async fn sleep_s(seconds: f64) {
    let d = if seconds.is_finite() && seconds > 0.0 {
        Duration::from_secs_f64(seconds)
    } else {
        Duration::ZERO
    };
    tokio::time::sleep(d).await;
}

/// `urlsplit(url).hostname`: lower-cased, without user info or port; None when there is none.
fn hostname(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let netloc = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = netloc.rsplit_once('@').map_or(netloc, |(_, h)| h);
    let host = if let Some(inner) = host.strip_prefix('[') {
        inner.split(']').next().unwrap_or("")
    } else {
        host.split(':').next().unwrap_or("")
    };
    (!host.is_empty()).then(|| host.to_lowercase())
}

/// `_handoff`: a human check handed to the owner (owner 2026-10-27): one notification whose click brings this tab
/// forward (`browser.show_handoff`), then a wait outside the stall clock until the page no longer shows the check's
/// words on two looks in a row. `Err` is the router's error on the first look.
pub async fn handoff(
    cx: &Cx,
    tab: Value,
    message: &str,
    task_id: &str,
    worker_id: &str,
) -> Result<Result<Value, String>, HandlerError> {
    let Some(tabs) = cx.deps.tabs.clone() else {
        return Ok(Err("the router isn't built".into()));
    };
    let first = match tabs.shows(&tab, message).await {
        Ok(v) => v,
        Err(e) => return Ok(Err(e)),
    };
    let truthy = |v: &Value, k: &str| v.get(k).is_some_and(ferb_py::value::truthy);
    if !truthy(&first, "open") {
        return Ok(Ok(
            json!({"passed": false, "error": "that tab isn't open any more"}),
        ));
    }
    let text = |v: &Value, k: &str| v.get(k).cloned().unwrap_or(json!(""));
    if first.get("shows") == Some(&Value::Bool(false)) {
        return Ok(Ok(json!({"passed": true, "url": text(&first, "url"),
                            "title": text(&first, "title"),
                            "next": "the page doesn't show those words now: take a fresh snapshot and carry on"})));
    }
    let hid = (cx.deps.new_handoff_id)();
    lock(&cx.deps.state.handoffs).insert(hid.clone(), tab.clone());
    let url = first.get("url").and_then(Value::as_str).unwrap_or("");
    let host = hostname(url).unwrap_or_else(|| "a page".into());
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    let (browser, folder) = tabs.tab_profile(&tab);
    struct Done(Cx, String, String);
    impl Drop for Done {
        fn drop(&mut self) {
            if let Some(waiting) = &self.0.deps.activity {
                waiting(&self.2, false);
            }
            lock(&self.0.deps.state.handoffs).remove(&self.1);
        }
    }
    let notified = cx.deps.notifier.notify(
        "FERB needs you for a moment",
        &format!(
            "{host} wants to check you're human. Click to open the tab; FERB carries on by itself once it lets \
             you through."
        ),
        true,
        "question",
        json!({"kind": "handoff", "handoff_id": hid, "task_id": task_id}),
        &format!("handoff:{hid}"),
    );
    notified.await;
    cx.host.journal(
        "browser.handoff",
        json!({"task_id": task_id, "handoff_id": hid, "host": host, "browser": browser, "folder": folder}),
        None,
    )?;
    if let Some(waiting) = &cx.deps.activity {
        waiting(worker_id, true);
    }
    let _done = Done(cx.clone(), hid.clone(), worker_id.to_string());
    let (mut clear, mut now) = (0, first);
    while clear < 2 {
        sleep_s(cx.deps.handoff_poll_s).await;
        now = match tabs.shows(&tab, message).await {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !truthy(&now, "open") {
            cx.host.journal(
                "browser.handoff_ended",
                json!({"handoff_id": hid, "passed": false}),
                None,
            )?;
            return Ok(Ok(
                json!({"passed": false, "error": "the tab was closed before the check let the owner through"}),
            ));
        }
        clear = if now.get("shows") == Some(&Value::Bool(false)) {
            clear + 1
        } else {
            0
        };
    }
    cx.host.journal(
        "browser.handoff_ended",
        json!({"handoff_id": hid, "passed": true}),
        None,
    )?;
    Ok(Ok(
        json!({"passed": true, "url": text(&now, "url"), "title": text(&now, "title"),
                 "next": "the check let the owner through: take a fresh snapshot of this tab and carry on"}),
    ))
}

/// `_browsing` (APP-777): where a task FERB runs as itself browses: {} (FERB's own profile, or nothing to offer),
/// {as_owner, label}, {declined}, {note} for a run nobody attends, or {ask: approval id}.
pub fn browsing(
    cx: &Cx,
    questions: &dyn OwnerQuestions,
    attended: bool,
) -> Result<Value, HandlerError> {
    use ferb_orchestrator::browsing as words;
    if cx.identity_profile()?.is_some() {
        return Ok(json!({}));
    }
    let (chosen, known) = (cx.owner_browsing_profile()?, cx.browsing_profiles()?);
    let shown_of =
        |c: &(String, String)| profiles::shown(&profiles::named(&known, &c.0, &c.1), None);
    if let Some(c) = &chosen {
        if known.is_empty() || known.iter().any(|p| p.browser == c.0 && p.folder == c.1) {
            return Ok(json!({"as_owner": true, "label": shown_of(c)}));
        }
    }
    let gone = chosen.as_ref().map(shown_of).unwrap_or_default();
    let asked = cx.host.get_or("browsing", words::ASKED, json!(false))?;
    if gone.is_empty() && ferb_py::value::truthy(&asked) {
        return Ok(json!({"declined": true}));
    }
    if known.is_empty() {
        return Ok(json!({}));
    }
    if !attended {
        return Ok(json!({"note": if gone.is_empty() {
            words::NO_BROWSER_RUN.to_string()
        } else {
            words::GONE_RUN.replace("{label}", &gone)
        }}));
    }
    if let Some(open) = questions.pending(words::KIND) {
        return Ok(json!({"ask": open}));
    }
    let mut options: Vec<(String, String)> = vec![];
    for p in &known {
        let label = profiles::shown(p, None);
        let value = format!("{}/{}", p.browser, p.folder);
        match options.iter_mut().find(|(k, _)| *k == label) {
            Some(slot) => slot.1 = value,
            None => options.push((label, value)),
        }
    }
    let mut map = Map::new();
    for (k, v) in &options {
        map.insert(k.clone(), json!(v));
    }
    let gone_line = if gone.is_empty() {
        String::new()
    } else {
        words::GONE.replace("{label}", &gone)
    };
    let reason = format!(
        "{}\n{gone_line}{}",
        words::QUESTION_TITLE,
        words::QUESTION_DETAIL
    );
    let key = format!("browsing-{}", cx.host.now().format("%Y%m%dT%H%M%S"));
    let mut labels: Vec<String> = options.iter().map(|(k, _)| k.clone()).collect();
    labels.push(words::NONE.to_string());
    let ident = ferb_py::json::dumps(&json!({"options": map}));
    Ok(json!({"ask": questions.ask(words::KIND, &ident, &reason, &key, &labels)}))
}

/// `_browsing_answered`: the owner's answer to the browsing question (`state`, `choice`): a profile is stored as
/// `browsing.owner_profile` through the settings path, None stores none; either way FERB doesn't ask again unless
/// that profile goes. A line for the owner ("" when nothing was stored).
pub fn browsing_answered(
    cx: &Cx,
    state: &str,
    choice: &str,
    ident: &str,
) -> Result<String, HandlerError> {
    use ferb_orchestrator::browsing as words;
    if state != "granted" || choice.is_empty() {
        return Ok(String::new());
    }
    let options = ferb_py::json::loads(ident)
        .ok()
        .and_then(|v| v.get("options").and_then(Value::as_object).cloned())
        .unwrap_or_default();
    let (profile, line) = if choice == words::NONE {
        (String::new(), words::NONE_CHOSEN.to_string())
    } else if let Some(v) = options.get(choice) {
        (
            ferb_py::value::py_str(v),
            words::CHOSEN.replace("{label}", choice),
        )
    } else {
        return Ok(String::new());
    };
    app_settings::write(&cx.host.config, "browsing.owner_profile", &json!(profile))
        .map_err(write_error)?;
    cx.host.set("browsing", words::ASKED, &json!(true))?;
    cx.host.journal(
        "browsing.chosen",
        json!({"profile": profile}),
        Some("owner"),
    )?;
    Ok(line)
}

fn write_error(e: app_settings::WriteError) -> HandlerError {
    match e {
        app_settings::WriteError::Bad(b) => refusal(b.0),
        app_settings::WriteError::Config(c) => refusal(c.to_string()),
    }
}

// ---------- the methods ----------

async fn browsers_list(cx: Cx) -> HandlerResult {
    let ferb = cx.identity_profile()?;
    let bindings = cx.deps.pairings.all().map_err(store_err)?;
    let paired_keys: Vec<(String, String)> = bindings
        .iter()
        .map(|b| (b.browser.clone(), b.folder.clone()))
        .collect();
    let discovered = cx.profiles();
    let mut out = vec![];
    for p in profiles::with_paired(cx.deps.platform.as_ref(), &discovered, &paired_keys) {
        let key = (p.browser.clone(), p.folder.clone());
        let b = bindings
            .iter()
            .find(|b| b.browser == p.browser && b.folder == p.folder);
        let br = cx.deps.hub.bridge(&p.browser, &p.folder);
        let connected = br.as_ref().is_some_and(|b| b.connected);
        let said = br
            .as_ref()
            .filter(|b| !b.version.is_empty())
            .map(|b| b.version.clone())
            .unwrap_or_default();
        let last = if !said.is_empty() {
            said
        } else if let Some(b) = b {
            cx.deps.pairings.version(&b.install_id).map_err(store_err)?
        } else {
            String::new()
        };
        out.push(json!({"browser": p.browser, "folder": p.folder, "name": p.display_name, "email": p.email,
                        "ferb_identity": Some(&key) == ferb.as_ref(),
                        "paired": b.is_some(), "identity": b.map(|b| b.identity.as_str()),
                        "connected": connected,
                        "version": if connected { br.as_ref().map(|b| b.version.clone()).unwrap_or_default() }
                                   else { String::new() },
                        "last_version": last,
                        "refused": if connected { None } else { b.and_then(|b| cx.deps.hub.problem(b)) }}));
    }
    let unpaired = cx.deps.hub.unpaired();
    let mut browsers: Vec<&String> = unpaired.iter().collect();
    browsers.sort();
    browsers.dedup();
    let mut counts = Map::new();
    for b in browsers {
        counts.insert(
            b.clone(),
            json!(unpaired.iter().filter(|x| *x == b).count()),
        );
    }
    let mut ids = cx.accepted_ids()?;
    ids.sort();
    ids.dedup();
    Ok(object(
        json!({"profiles": out, "extension_ids": ids, "unpaired_connections": counts}),
    ))
}

async fn browsers_pair(cx: Cx, params: Params) -> HandlerResult {
    let p = match cx.exact_profile(&params)? {
        Some(p) => p,
        None => {
            let known = cx.profiles();
            cx.resolved(&params, &known)?
        }
    };
    if Some((p.browser.clone(), p.folder.clone())) == cx.identity_profile()? {
        return Err(refusal(format!(
            "{} is FERB's own profile; it's paired as FERB, not as yours",
            profiles::shown(&p, None)
        )));
    }
    if cx.profile_state(&p, None)? == "missing" {
        let known = cx.profiles();
        if let Err(why) = cx.open_known(
            &p.browser,
            &p.folder,
            Some("chrome://extensions"),
            &known,
            false,
        )? {
            return Err(HandlerError::fault("ValueError", why));
        }
        return Ok(object(
            json!({"paired": false, "needs_install": true, "browser": p.browser,
            "folder": p.folder, "name": p.display_name, "email": p.email,
            "extension_dir": extension_path(&cx.host.home).to_string_lossy()}),
        ));
    }
    let timeout = timeout_s(&params, 60.0)?;
    cx.pair_one(p, timeout).await
}

async fn browsers_installed(cx: Cx, params: Params) -> HandlerResult {
    let known = cx.profiles();
    let p = cx.resolved(&params, &known)?;
    Ok(object(
        json!({"installed": cx.profile_state(&p, None)? != "missing"}),
    ))
}

async fn browsers_unpair(cx: Cx, params: Params) -> HandlerResult {
    let known = cx.profiles();
    let p = cx.resolved(&params, &known)?;
    let done = cx
        .deps
        .pairings
        .unpair(&p.browser, &p.folder)
        .map_err(store_err)?;
    Ok(object(json!({"unpaired": done})))
}

async fn setup_browser_pair(cx: Cx, params: Params) -> HandlerResult {
    let Some(p) = cx.exact_profile(&params)? else {
        return Err(refusal("name the profile's browser and folder"));
    };
    let timeout = timeout_s(&params, 60.0)?;
    cx.pair_one(p, timeout).await
}

/// Where Pair all's events go: the stream's channel, or (one reply) only the last kept.
enum Sink {
    Stream(tokio::sync::mpsc::Sender<HandlerResult>),
    Last(Option<Map<String, Value>>),
}

impl Sink {
    /// False when the stream's caller left.
    async fn emit(&mut self, event: Map<String, Value>) -> bool {
        match self {
            Sink::Stream(tx) => tx.send(Ok(event)).await.is_ok(),
            Sink::Last(last) => {
                *last = Some(event);
                true
            }
        }
    }

    async fn left(&self) {
        match self {
            Sink::Stream(tx) => tx.closed().await,
            Sink::Last(_) => std::future::pending().await,
        }
    }
}

/// `_pair_all_events` (HAND-411, HAND-412): `plan`, a `result` per profile as it lands, then `done`. Leaving the
/// stream stops the pairings still waiting (their keys revoked).
async fn pair_all_events(cx: Cx, params: Params, sink: &mut Sink) -> Result<(), HandlerError> {
    let store = cx.store_id()?;
    let ids = cx.accepted_ids()?;
    let paired: Vec<(String, String)> = cx
        .deps
        .pairings
        .all()
        .map_err(store_err)?
        .into_iter()
        .map(|b| (b.browser, b.folder))
        .collect();
    let (states, known) = {
        let (store, ids, paired) = (store.clone(), ids.clone(), paired.clone());
        cx.blocking(move |c| {
            let data = c.browser_data();
            let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            let states = autoinstall::read_states(
                c.deps.platform.as_ref(),
                &c.deps.browser_support,
                &store,
                &refs,
                Some(&paired),
                Some(&data),
            );
            (states, c.profiles())
        })
        .await
    };
    let url = store_page(&store);
    let ext_dir = extension_path(&cx.host.home);
    let ferb = cx.identity_profile()?;
    let mut todo: Vec<BrowserProfile> = vec![];
    let mut skipped: Vec<Value> = vec![];
    let mut labels: Vec<((String, String), String)> = vec![];
    let mut owner = 0;
    for s in &states {
        let label = label_of(&s.browser);
        for st in &s.profiles {
            let key = (st.browser.clone(), st.folder.clone());
            if Some(&key) == ferb.as_ref() {
                continue;
            }
            owner += 1;
            labels.push((key.clone(), label.clone()));
            let mut why = steps::not_paired_step(st, &label, s.request.is_some(), &url, &ext_dir);
            if why.is_none() && st.paired == Some(true) {
                why = cx.paired_problem(&st.browser, &st.folder)?;
                if why.is_none() {
                    continue;
                }
            }
            if let Some((reason, step)) = why {
                skipped.push(
                    json!({"browser": st.browser, "label": label, "folder": st.folder,
                    "name": st.name, "email": st.email, "reason": reason, "step": step,
                    "can_add": !url.is_empty() && ADDABLE.contains(&reason.as_str())}),
                );
            } else if let Some(p) = known
                .iter()
                .find(|p| p.browser == st.browser && p.folder == st.folder)
            {
                todo.push(p.clone());
            }
        }
    }
    if !skipped.is_empty() {
        let short: Vec<Value> = skipped
            .iter()
            .map(|s| json!({"browser": s["browser"], "folder": s["folder"], "reason": s["reason"]}))
            .collect();
        cx.host
            .journal("browsers.pair_all_skipped", json!({"skipped": short}), None)?;
    }
    let label_for = |b: &str, f: &str| {
        labels
            .iter()
            .find(|(k, _)| k.0 == b && k.1 == f)
            .map_or_else(|| b.to_string(), |(_, l)| l.clone())
    };
    let store_url = if url.is_empty() {
        Value::Null
    } else {
        json!(url)
    };
    let plan = json!({"type": "plan", "profiles": owner, "skipped": skipped, "store_url": store_url,
        "todo": todo.iter().map(|p| json!({"browser": p.browser, "folder": p.folder,
            "name": p.display_name, "email": p.email, "label": label_for(&p.browser, &p.folder)}))
            .collect::<Vec<_>>()});
    if !sink.emit(object(plan)).await {
        return Ok(());
    }
    let timeout = timeout_s(&params, 60.0)?;
    let mut jobs = tokio::task::JoinSet::new();
    for p in todo {
        let c = cx.clone();
        jobs.spawn(async move { c.pair_one(p, timeout).await });
    }
    let mut results: Vec<Value> = vec![];
    loop {
        let next = tokio::select! {
            next = jobs.join_next() => next,
            _ = sink.left() => return Ok(()),      // the caller left: dropping `jobs` stops the waits
        };
        let Some(done) = next else { break };
        let mut r = match done {
            Ok(r) => r?,
            Err(e) => std::panic::resume_unwind(e.into_panic()),
        };
        let b = r
            .get("browser")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let f = r
            .get("folder")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        r.insert("label".into(), json!(label_for(&b, &f)));
        results.push(Value::Object(r.clone()));
        let mut event = object(json!({"type": "result"}));
        event.extend(r);
        if !sink.emit(event).await {
            return Ok(());
        }
    }
    let paired_n = results
        .iter()
        .filter(|r| r.get("paired").is_some_and(ferb_py::value::truthy))
        .count();
    sink.emit(object(
        json!({"type": "done", "profiles": owner, "skipped": skipped, "store_url": store_url,
        "results": results, "paired": paired_n}),
    ))
    .await;
    Ok(())
}

async fn setup_browsers_pair_all(cx: Cx, params: Params) -> HandlerResult {
    let mut sink = Sink::Last(None);
    pair_all_events(cx, params, &mut sink).await?;
    let Sink::Last(last) = sink else {
        unreachable!()
    };
    let mut reply = last.unwrap_or_default();
    reply.shift_remove("type");
    Ok(reply)
}

async fn setup_browsers(cx: Cx) -> HandlerResult {
    let store = cx.store_id()?;
    let ext_dir = extension_path(&cx.host.home);
    let ferb = cx.identity_profile()?;
    if !cx.blocking(|c| c.other_app_data()).await {
        let rows = steps::browser_rows(
            &[],
            &store,
            &ext_dir,
            ferb.as_ref(),
            &[],
            &[],
            &BTreeMap::new(),
        );
        return Ok(merge(
            rows,
            json!({"waiting": "full_disk_access", "problem": WAITS_FOR_FULL_DISK_ACCESS}),
        ));
    }
    let files = cx.connect_browsers().await?;
    let ids = cx.accepted_ids()?;
    let paired: Vec<(String, String)> = cx
        .deps
        .pairings
        .all()
        .map_err(store_err)?
        .into_iter()
        .map(|b| (b.browser, b.folder))
        .collect();
    let (states, others) = {
        let (store, ids, paired) = (store.clone(), ids.clone(), paired.clone());
        cx.blocking(move |c| {
            let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            let states = autoinstall::read_states(
                c.deps.platform.as_ref(),
                &c.deps.browser_support,
                &store,
                &refs,
                Some(&paired),
                c.deps.browser_data_dirs.as_deref(),
            );
            (states, profiles::unsupported(c.deps.platform.as_ref()))
        })
        .await
    };
    let connected: Vec<((String, String), String)> = cx
        .deps
        .hub
        .bridges()
        .into_iter()
        .filter(|b| b.connected)
        .map(|b| ((b.browser, b.folder), b.version))
        .collect();
    let mut rows = steps::browser_rows(
        &states,
        &store,
        &ext_dir,
        ferb.as_ref(),
        &connected,
        &others,
        &cx.deps.hub.unpaired_copies(),
    );
    let why = cx.profile_problems()?;
    if let Some(Value::Array(browsers)) = rows.get_mut("browsers") {
        for browser in browsers {
            let b = browser["browser"].as_str().unwrap_or("").to_string();
            if let Some(Value::Array(rows)) = browser.get_mut("profiles") {
                for row in rows {
                    let f = row["folder"].as_str().unwrap_or("").to_string();
                    if let Some((_, text)) = why.iter().find(|(k, _)| k.0 == b && k.1 == f) {
                        row["problem"] = json!(text);
                    }
                }
            }
        }
    }
    let mut problems: Vec<String> = vec![];
    if let Some(p) = files.get("problem").filter(|p| ferb_py::value::truthy(p)) {
        problems.push(ferb_py::value::py_str(p));
    }
    problems.extend(cx.extension_problems().await?);
    if !problems.is_empty() {
        rows.insert("problem".into(), json!(problems.join(" ")));
    }
    Ok(rows)
}

async fn setup_browser_open(cx: Cx, params: Params) -> HandlerResult {
    let p = P(&params);
    let (browser, folder) = (p.text("browser", ""), p.text("folder", ""));
    let page = store_page(&cx.store_id()?);
    let url = if page.is_empty() {
        "chrome://extensions".to_string()
    } else {
        page
    };
    let opened = {
        let (b, f, u) = (browser.clone(), folder.clone(), url.clone());
        cx.blocking(move |c| {
            let known = c.profiles();
            c.open_known(&b, &f, Some(&u), &known, false)
        })
        .await?
    };
    opened.map_err(refusal)?;
    cx.host.journal(
        "setup.browser_opened",
        json!({"browser": browser, "folder": folder, "url": url}),
        None,
    )?;
    Ok(object(json!({"opened": url})))
}

async fn show_handoff(cx: Cx, params: Params) -> HandlerResult {
    let id = P(&params).text_or_empty("handoff_id");
    let tab = lock(&cx.deps.state.handoffs).get(&id).cloned();
    let (Some(tab), Some(tabs)) = (tab, cx.deps.tabs.clone()) else {
        return Ok(object(
            json!({"shown": false, "why": "that check is already over"}),
        ));
    };
    if tabs.show_tab(&tab).await {
        return Ok(object(json!({"shown": true, "tab": true})));
    }
    let (browser, folder) = tabs.tab_profile(&tab);
    let known = cx.profiles();
    if let Err(why) = cx.open_known(&browser, &folder, None, &known, true)? {
        return Err(HandlerError::fault("ValueError", why));
    }
    Ok(object(json!({"shown": true, "tab": false})))
}

async fn identity_find(cx: Cx, params: Params) -> HandlerResult {
    let configured = cx.config_text("mailbox", "address")?;
    let given = P(&params).text_or_empty("address");
    let address = ferb_py::value::strip(if given.is_empty() {
        &configured
    } else {
        &given
    })
    .to_lowercase();
    let (prof, owner) = (cx.identity_profile()?, cx.owner_browsing_profile()?);
    let found = if prof.is_some() || owner.is_some() {
        cx.blocking(|c| c.profiles()).await
    } else {
        vec![]
    };
    let mine = prof.as_ref().map(|(b, f)| profiles::named(&found, b, f));
    let theirs = owner.as_ref().map(|(b, f)| profiles::named(&found, b, f));
    let current = json!({"address": configured,
        "browser": prof.as_ref().map(|p| p.0.clone()).unwrap_or_default(),
        "profile": prof.as_ref().map(|p| p.1.clone()).unwrap_or_default(),
        "profile_name": mine.as_ref().map(|p| p.display_name.clone()).unwrap_or_default(),
        "profile_email": mine.as_ref().map(|p| p.email.clone()).unwrap_or_default(),
        "paired": cx.identity_paired()?,
        "owner_profile": cx.config_text("browsing", "owner_profile")?,
        "owner_profile_name": theirs.as_ref().map(|p| p.display_name.clone()).unwrap_or_default(),
        "owner_profile_email": theirs.as_ref().map(|p| p.email.clone()).unwrap_or_default()});
    let candidates = if address.is_empty() {
        vec![]
    } else {
        let a = address.clone();
        cx.blocking(move |c| c.identity_candidates(&a)).await
    };
    Ok(object(
        json!({"current": current, "address": address, "candidates": candidates}),
    ))
}

async fn identity_choose(cx: Cx, params: Params) -> HandlerResult {
    let p = P(&params);
    let stored = if p.is_true("clear") {
        let stored =
            app_settings::write_identity(&cx.host.config, "", "", "").map_err(write_error)?;
        cx.deps.identity_browser.detach();
        stored
    } else {
        let address = ferb_py::value::strip(&p.text_or_empty("address")).to_lowercase();
        let (browser, folder) = (p.text_or_empty("browser"), p.text_or_empty("folder"));
        if !cx.other_app_data() {
            return Err(refusal(WAITS_FOR_FULL_DISK_ACCESS));
        }
        let Some((_, data)) = cx.browser_data().into_iter().find(|(b, _)| *b == browser) else {
            let shown = if browser.is_empty() {
                "That browser"
            } else {
                browser.as_str()
            };
            return Err(refusal(format!("{shown} isn't installed")));
        };
        let (a, f) = (address.clone(), folder.clone());
        cx.blocking(move |_| ferb_identity::seed::verify(&a, &f, &data))
            .await
            .map_err(|e| refusal(e.0))?;
        app_settings::write_identity(&cx.host.config, &address, &browser, &folder)
            .map_err(write_error)?
    };
    cx.host.journal(
        "settings.changed",
        json!({"key": "identity", "value": stored}),
        Some("owner"),
    )?;
    Ok(stored)
}

async fn identity_pair(cx: Cx, params: Params) -> HandlerResult {
    let prof = cx.identity_profile()?;
    let address = cx.config_text("mailbox", "address")?;
    let Some((browser, folder)) = prof.filter(|_| !address.is_empty()) else {
        return Err(refusal("FERB's identity profile isn't chosen yet"));
    };
    if !cx.other_app_data() {
        return Err(refusal(WAITS_FOR_FULL_DISK_ACCESS));
    }
    let data = cx
        .browser_data()
        .into_iter()
        .find(|(b, _)| *b == browser)
        .map_or_else(|| cx.deps.chrome_data.clone(), |(_, d)| d);
    {
        let (a, f) = (address.clone(), folder.clone());
        cx.blocking(move |_| ferb_identity::seed::verify(&a, &f, &data))
            .await
            .map_err(|e| refusal(format!("refusing to pair: {}", e.0)))?;
    }
    let current = cx
        .deps
        .pairings
        .for_profile(&browser, &folder)
        .map_err(store_err)?;
    if current.is_some() && !P(&params).truthy("confirm") {
        return Err(refusal(
            "FERB's identity profile is already paired; re-pairing needs your confirmation",
        ));
    }
    let (b, f) = (browser.clone(), folder.clone());
    let stands = cx
        .blocking(move |c| {
            match c
                .profiles()
                .into_iter()
                .find(|p| p.browser == b && p.folder == f)
            {
                Some(p) => c.extension_stands(&p),
                None => Ok(None),
            }
        })
        .await?;
    if let Some((_, why)) = stands {
        return Err(refusal(format!(
            "FERB's own profile can't be paired yet: {why}"
        )));
    }
    let before = cx.deps.hub.bridge(&browser, &folder);
    let hellos0 = before.as_ref().map_or(0, |b| b.hellos);
    let token = cx
        .deps
        .pairings
        .issue_token(&browser, &folder, Identity::Ferb);
    let _revoke = Revoke(cx.deps.pairings.clone(), token.clone());
    cx.open_pairing(&browser, &folder, &token).await?;
    let clock = cx.host.clock.clone();
    let end = clock.monotonic() + timeout_s(&params, 60.0)?;
    while clock.monotonic() < end {
        let b = cx
            .deps
            .pairings
            .for_profile(&browser, &folder)
            .map_err(store_err)?;
        let br = cx.deps.hub.bridge(&browser, &folder);
        if let Some(br) = br.filter(|br| br.connected) {
            let fresh = before.as_ref().map(|b| b.id) != Some(br.id) || br.hellos > hellos0;
            if cx.deps.pairings.was_paired(&token)
                && b.is_some_and(|b| b.identity == Identity::Ferb)
                && fresh
            {
                cx.attach_identity_if_paired()?;
                return Ok(object(json!({"paired": true, "profile": folder})));
            }
        }
        sleep_s(0.2).await;
    }
    cx.deps.pairings.revoke(&token);
    Ok(object(json!({"paired": false, "profile": folder,
                     "hint": "is the FERB extension loaded in FERB's profile?"})))
}

/// The browser area's 13 methods (`areas/rpc.py`'s entries for them).
pub fn register(server: &Server, host: &Host, deps: &Deps) {
    let cx = Cx::new(host, deps);
    macro_rules! unary {
        ($name:expr, |$c:ident, $p:ident| $body:expr) => {{
            let cx = cx.clone();
            server.register_unary($name, move |_s: Session, $p: Params| {
                let $c = cx.clone();
                async move { $body.await }
            });
        }};
    }
    unary!("browsers.list", |c, _p| browsers_list(c));
    unary!("browsers.pair", |c, p| browsers_pair(c, p));
    unary!("browsers.installed", |c, p| browsers_installed(c, p));
    unary!("browsers.unpair", |c, p| browsers_unpair(c, p));
    unary!("setup.browser_pair", |c, p| setup_browser_pair(c, p));
    unary!("setup.browsers_pair_all", |c, p| setup_browsers_pair_all(
        c, p
    ));
    unary!("setup.browsers", |c, _p| setup_browsers(c));
    unary!("setup.browser_open", |c, p| setup_browser_open(c, p));
    unary!("browser.show_handoff", |c, p| show_handoff(c, p));
    unary!("identity.find", |c, p| identity_find(c, p));
    unary!("identity.choose", |c, p| identity_choose(c, p));
    unary!("identity.pair", |c, p| identity_pair(c, p));
    let stream_cx = cx.clone();
    server.register_stream(
        "setup.browsers_pairing",
        move |_s: Session, params: Params| -> Box<dyn ferb_rpc::EventStream> {
            let (tx, rx) = tokio::sync::mpsc::channel::<HandlerResult>(16);
            let c = stream_cx.clone();
            tokio::spawn(async move {
                let mut sink = Sink::Stream(tx.clone());
                if let Err(e) = pair_all_events(c, params, &mut sink).await {
                    let _ = tx.send(Err(e)).await;
                }
            });
            Box::new(rx)
        },
    );
}
