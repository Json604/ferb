//! Task-owned tabs over the bridges: a port of the core of `ext_bridge.py`'s `_TabDriver`, `Router`
//! (the owner's profiles, `browser.*`) and `FerbIdentity` (FERB's own profile, `ferb_browser.*`).
//!
//! Ported here: opening a tab (blank first, in the task's collapsed group, muted, within the task's
//! tab cap, a window chosen when the profile has none of FERB's: HAND-201, HAND-207, HAND-234), the
//! one connection check every tool goes through (HAND-403), navigate, close, keep open, release at
//! task end, re-adoption after a reconnect, the owner closing or taking a tab, popups and links a
//! task tab opens, borrowing the owner's tab, snapshots with generation-scoped element ids and the
//! saved full snapshot, `read`, dialogs, and the sealed fills: FERB's secret and the owner's secret
//! or login code, each after the element's generation and the frame's exact origin are checked and
//! before anything secret is read (AGENTS.md rule 7).
//!
//! The page operations are in [`crate::ops`] (act, find, html, scroll, media, wait, terminal
//! input, the cursor's geometry), [`crate::capture`] (screenshots, points, PDF, the owner's view,
//! recordings), [`crate::upload`] and [`crate::owner`] (the owner's tab list and profiles, FERB's
//! inbox codes, cleanup leftovers).
//!
//! Fixed against Python: the tab cap counts every driver's tabs through the hub (HAND-234), as
//! Python's does; a cleanup leftover names the browser, profile and account (HAND-419).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{
    ActionResult, Dict, ElementRef, Identity, PageElement, PageSnapshot, TabRef, ToolContext,
};
use ferb_identity::secrets::{SecretRef, SecretSpec};
use ferb_identity::Secret;
use indexmap::IndexMap;
use serde_json::{json, Map, Value};

use crate::bridge::{Adopted, Bridge, Hub, TabDriver, TaskTab};
use crate::error::{Error, Result};
use crate::rules;
use crate::seal::{self, SealError, Sealed};
use crate::snapshot;

/// `WINDOWLESS_WAIT_S`: a profile loaded with no window.
pub const WINDOWLESS_WAIT_S: f64 = 35.0;
/// `WINDOW_WAIT_S`: then the profile's window.
pub const WINDOW_WAIT_S: f64 = 15.0;
/// A task tab opens blank, then loads its URL (D35).
pub const BLANK: &str = "about:blank";
/// A media element's state fields (L26).
pub const MEDIA_FIELDS: [&str; 7] = [
    "playing",
    "paused",
    "current_time",
    "duration",
    "muted",
    "volume",
    "in_view",
];

/// D31: a secret never goes into a web terminal.
pub const SECRET_TERMINAL: &str =
    "that's a terminal in the page: FERB puts no password or code into a terminal (D31); \
                                   it would run as the owner's input there";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

type TextFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
type CheckUrl = Arc<dyn Fn(&str) -> std::result::Result<(), String> + Send + Sync>;
/// `Resolver.choose`: the profile a name means, `(browser, folder)`.
pub type Choose = Arc<dyn Fn(Option<&str>) -> Result<(String, String)> + Send + Sync>;

/// What the daemon tells the driver about tasks (the `task_*` callables `_TabDriver` is given).
#[derive(Clone)]
pub struct Tasks {
    pub running: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    pub paused: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    /// L31: a loop holds this task's tabs between its rounds.
    pub resting: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    /// The task tree (tab cap, group).
    pub root: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// A task tree's words for its group's label.
    pub title: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// browser.task_max_tabs.
    pub max_tabs: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// A registered checker reaches its worker's tabs.
    pub inspection_parent: TextFn,
    /// HAND-511: the owner's own input is under way or just ended (`None`: the input tap isn't running).
    pub owner_input_near: Arc<dyn Fn() -> Option<bool> + Send + Sync>,
    /// The daemon's redactor (rule 14).
    pub redact: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// `tools.check_url` (policy's URL parse, S4): `Err(why)` refuses a link's new tab.
    pub check_url: CheckUrl,
}

impl Default for Tasks {
    fn default() -> Tasks {
        Tasks {
            running: Arc::new(|_| true),
            paused: Arc::new(|_| false),
            resting: Arc::new(|_| false),
            root: Arc::new(|t| t.to_string()),
            title: Arc::new(|_| String::new()),
            max_tabs: Arc::new(|| 50),
            inspection_parent: Arc::new(|_| None),
            owner_input_near: Arc::new(|| None),
            redact: Arc::new(|t| t.to_string()),
            check_url: Arc::new(|_| Ok(())),
        }
    }
}

/// Starting a profile that isn't connected (`open_known_profile`, the platform's part): with no
/// window first, then (`window`) its own window (HAND-403).
pub trait Wake: Send + Sync {
    fn start(&self, browser: &str, folder: &str, window: bool);
}

/// FERB's own secrets (`KeychainSecretStore`): names and specs, and the value, read only inside a
/// sealed fill (the identity crate's helper).
pub trait Secrets: Send + Sync {
    /// Every saved secret's spec (names, accounts, origins: never a value), for the tools' descriptions.
    fn specs(&self) -> Vec<SecretSpec>;
    fn has(&self, secret: &SecretRef) -> bool;
    fn spec(&self, secret: &SecretRef) -> Option<SecretSpec>;
    /// `Err(reason)`: unavailable ("" when the helper said nothing, HAND-410).
    fn reveal(&self, secret: &SecretRef) -> BoxFuture<'_, std::result::Result<Secret, String>>;
}

/// FERB's own configured secrets (`ferb_identity::secrets::KeychainSecretStore`, behind the signed helper) as the
/// FERB-identity driver reads them; a value is revealed in a worker thread (the helper is a blocking call).
pub struct KeychainSecrets(pub Arc<ferb_identity::secrets::KeychainSecretStore>);

impl Secrets for KeychainSecrets {
    fn specs(&self) -> Vec<SecretSpec> {
        self.0.specs()
    }
    fn has(&self, secret: &SecretRef) -> bool {
        self.0.has(secret)
    }
    fn spec(&self, secret: &SecretRef) -> Option<SecretSpec> {
        self.0.spec(secret)
    }
    fn reveal(&self, secret: &SecretRef) -> BoxFuture<'_, std::result::Result<Secret, String>> {
        let store = Arc::clone(&self.0);
        let secret = secret.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || store.reveal(&secret).map_err(|e| e.reason))
                .await
                .unwrap_or_else(|_| Err(String::new()))
        })
    }
}

/// Seals a value for one command and origin under a pairing key (production:
/// [`seal::seal_fill`], a fresh nonce each time; the daemon's helper may seal instead).
pub type Sealer =
    Arc<dyn Fn(&[u8], &Secret, &str, &str) -> std::result::Result<Sealed, SealError> + Send + Sync>;

/// The owner's hidden input for a secret or a login code (`ask_secret` / `ask_code`): the value,
/// or `None` when declined.
pub trait OwnerInput: Send + Sync {
    fn ask_code(&self, task: &str, intent: &str, origin: &str) -> BoxFuture<'_, Option<Secret>>;
    fn ask_secret(
        &self,
        task: &str,
        intent: &str,
        origin: &str,
        ask: SecretAsk,
    ) -> BoxFuture<'_, Option<Secret>>;

    /// `ask_code` with the whole call's context (Python's gateway reads the calling context: its worker, grant and
    /// intent), which the gateway's hidden input needs. By default it forwards to [`OwnerInput::ask_code`] with the
    /// context's task.
    fn ask_code_for<'a>(
        &'a self,
        ctx: &'a ToolContext,
        intent: &'a str,
        origin: &'a str,
    ) -> BoxFuture<'a, Option<Secret>> {
        self.ask_code(&ctx.task_id, intent, origin)
    }

    /// `ask_secret` with the whole call's context; by default [`OwnerInput::ask_secret`] with the context's task.
    fn ask_secret_for<'a>(
        &'a self,
        ctx: &'a ToolContext,
        intent: &'a str,
        origin: &'a str,
        ask: SecretAsk,
    ) -> BoxFuture<'a, Option<Secret>> {
        self.ask_secret(&ctx.task_id, intent, origin, ask)
    }
}

/// What the owner's secret question carries besides the origin.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SecretAsk {
    pub handle: Option<String>,
    pub remember: Option<String>,
    pub field: Option<String>,
    pub kind: Option<String>,
}

/// Which profiles a driver takes.
pub enum Kind {
    /// `Router`: the owner's profiles, chosen by name (`Resolver.choose`: `Ok((browser, folder))`).
    Owner {
        choose: Choose,
        input: Option<Arc<dyn OwnerInput>>,
    },
    /// `FerbIdentity`: FERB's own profile.
    Ferb {
        profile: Arc<dyn Fn() -> Option<(String, String)> + Send + Sync>,
        secrets: Arc<dyn Secrets>,
    },
}

/// One task tab (`_Tab`).
#[derive(Clone)]
pub struct Tab {
    pub tab: TabRef,
    pub bridge: Arc<Bridge>,
    pub chrome_id: i64,
    pub owner: Option<String>,
    pub url: String,
    pub snapshot_json: String,
    pub snap_gen: i64,
    pub snap_nav: i64,
    /// The owner's own tab, taken over for a task: forgotten after, never closed.
    pub attached: bool,
    /// Explicit handoff requested; an uncertain answer must not cause a later close.
    pub keep_open: bool,
    /// New tabs this tab started, not yet reported.
    pub opened: Vec<Map<String, Value>>,
    /// id -> (frame, snapshot generation, the frame's own element id, the element).
    pub elements: IndexMap<String, (i64, i64, i64, PageElement)>,
    pub containers: HashMap<String, (i64, String)>,
    pub media: HashMap<i64, (i64, i64)>,
    pub media_ids: HashMap<String, i64>,
    pub carried: HashSet<String>,
    pub closed: bool,
    pub left: String,
    pub closing: bool,
    pub closed_news: Vec<Map<String, Value>>,
    pub download_only: bool,
    pub loading: bool,
    pub held: bool,
    pub frame_urls: BTreeMap<i64, String>,
    pub commerce: Map<String, Value>,
    pub muted: bool,
    pub typed_at: Option<i64>,
    pub opener: Option<String>,
    pub title: String,
    pub http_status: Option<i64>,
    pub taken_over_from: Map<String, Value>,
    pub stuck: bool,
    pub timing: Option<Map<String, Value>>,
    /// find: the saved result of each search (payload, matches, lines, element matches, current).
    pub searches: HashMap<String, (String, i64, i64, usize, bool)>,
    /// The latest screenshot that resolves points.
    pub visual: Option<Visual>,
    /// Element id -> the extension's pointer for a screenshot point.
    pub points: HashMap<String, String>,
    /// How the latest screenshot's secret fields were found, when worth saying (L13).
    pub masking: String,
    /// A screenshot no point resolves on (a page the extension can't read, I1).
    pub unread_shot: Option<String>,
    /// The latest screenshot is the whole browser window.
    pub native_shot: bool,
    /// The latest capture went on although the tab's input readiness failed (round 5).
    pub input_note: String,
    pub shot_timing: Option<Map<String, Value>>,
    /// GATE-578: every money amount the latest snapshot shows.
    pub amounts: Vec<String>,
    /// Where the latest snapshot saw the tab (L33).
    pub page: Option<Page>,
    /// Element id -> its scroll facts (L33).
    pub motion: HashMap<String, (Option<String>, bool)>,
    /// FERB scrolled the page since the snapshot (L33).
    pub moved: bool,
    /// D31: the web terminal element FERB just typed approved text into, and that Yes.
    pub armed: Option<(String, String)>,
    /// HAND-488: what the last screenshot's snapshot did to earlier element ids.
    pub ids_note: Map<String, Value>,
    /// HAND-496: the last html read (its key, its text, skipped frames).
    pub markup: (String, String, Vec<i64>),
}

/// A screenshot whose points resolve: its id, the snapshot and load it goes with, when it was taken
/// (the loop's clock), its pixel size.
#[derive(Clone, Debug, PartialEq)]
pub struct Visual {
    pub shot: String,
    pub snap_gen: i64,
    pub snap_nav: i64,
    pub at: tokio::time::Instant,
    pub width: i64,
    pub height: i64,
}

/// `_Page` (L33): the tab's window as the extension reported it, the top frame's layout viewport
/// (w, h), visual viewport (x, y, w, h, scale) and own scroll (top, height, viewport height, left,
/// width), and whether its body scrolls by itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub window: Option<Map<String, Value>>,
    pub layout: Option<(f64, f64)>,
    pub visual: Option<[f64; 5]>,
    pub scroll: Option<[f64; 5]>,
    pub body_scrolls: bool,
}

/// `_numbers`: `n` finite numbers (never booleans).
pub fn numbers(v: &[Option<&Value>]) -> Option<Vec<f64>> {
    v.iter()
        .map(|x| x.and_then(Value::as_f64).filter(|f| f.is_finite()))
        .collect()
}

fn numbers_of(v: Option<&Value>, n: usize) -> Option<Vec<f64>> {
    let a = v?.as_array()?;
    if a.len() != n {
        return None;
    }
    numbers(&a.iter().map(Some).collect::<Vec<_>>())
}

impl Page {
    /// `_Page.of`.
    pub fn of(window: Option<&Value>, top: &Value) -> Page {
        let scroll = top.get("scroll").and_then(Value::as_object).and_then(|s| {
            let n = numbers(
                &["y", "height", "viewport_h", "x", "width"]
                    .iter()
                    .map(|k| s.get(*k))
                    .collect::<Vec<_>>(),
            )?;
            Some([n[0], n[1], n[2], n[3], n[4]])
        });
        let layout = numbers_of(top.get("viewport"), 2)
            .filter(|l| l[0] > 0.0 && l[1] > 0.0)
            .map(|l| (l[0], l[1]));
        let visual = numbers_of(top.get("visual"), 5)
            .filter(|v| v[2] > 0.0 && v[3] > 0.0 && v[4] > 0.0)
            .map(|v| [v[0], v[1], v[2], v[3], v[4]]);
        Page {
            window: window.and_then(Value::as_object).cloned(),
            layout,
            visual,
            scroll,
            body_scrolls: top.get("body_scrolls") == Some(&Value::Bool(true)),
        }
    }

    /// `mapping`: the page's part the screen shows, in CSS px.
    pub fn mapping(&self) -> Option<crate::mask::Rect> {
        let ((lw, lh), v) = (self.layout?, self.visual?);
        Some((v[0], v[1], lw / v[4], lh / v[4]))
    }

    /// `centred`: where an act that centres its element will have scrolled it.
    pub fn centred(&self, rect: crate::mask::Rect) -> Option<crate::mask::Rect> {
        if self.body_scrolls {
            return None;
        }
        let ((lw, _), v, s) = (self.layout?, self.visual?, self.scroll?);
        let [top, height, view, _, width] = s;
        let scale = v[4];
        let mid = rect.0 + rect.2 / 2.0;
        if (scale - 1.0).abs() > 1e-3 || width > lw + 1.0 || !(0.0..=lw).contains(&mid) {
            return None;
        }
        let to = (top + rect.1 + rect.3 / 2.0 - view / 2.0)
            .max(0.0)
            .min((height - view).max(0.0));
        Some((rect.0, rect.1 - (to - top), rect.2, rect.3))
    }
}

/// `_motion`: an element's scroll facts: fixed, sticky or "unknown", and whether a box other than
/// the page scrolls it.
pub fn motion(raw: &Map<String, Value>) -> (Option<String>, bool) {
    let pinned = raw
        .get("pinned")
        .and_then(Value::as_str)
        .filter(|p| matches!(*p, "fixed" | "sticky" | "unknown"))
        .map(String::from);
    let boxed = raw.get("scroll_box").is_some_and(ferb_py::value::truthy)
        || raw.get("container").is_some_and(ferb_py::value::truthy);
    (pinned, boxed)
}

impl Tab {
    fn new(
        tab: TabRef,
        bridge: Arc<Bridge>,
        chrome_id: i64,
        owner: Option<String>,
        url: String,
    ) -> Tab {
        Tab {
            tab,
            bridge,
            chrome_id,
            owner,
            url,
            snapshot_json: String::new(),
            snap_gen: 0,
            snap_nav: -1,
            attached: false,
            keep_open: false,
            opened: vec![],
            elements: IndexMap::new(),
            containers: HashMap::new(),
            media: HashMap::new(),
            media_ids: HashMap::new(),
            carried: HashSet::new(),
            closed: false,
            left: String::new(),
            closing: false,
            closed_news: vec![],
            download_only: false,
            loading: false,
            held: false,
            frame_urls: BTreeMap::new(),
            commerce: Map::new(),
            muted: false,
            typed_at: None,
            opener: None,
            title: String::new(),
            http_status: None,
            taken_over_from: Map::new(),
            stuck: false,
            timing: None,
            searches: HashMap::new(),
            visual: None,
            points: HashMap::new(),
            masking: String::new(),
            unread_shot: None,
            native_shot: false,
            input_note: String::new(),
            shot_timing: None,
            amounts: vec![],
            page: None,
            motion: HashMap::new(),
            moved: false,
            armed: None,
            ids_note: Map::new(),
            markup: (String::new(), String::new(), vec![]),
        }
    }
}

#[derive(Default)]
pub(crate) struct State {
    /// By alias number ("t3" is 3), in the order they were made.
    pub(crate) tabs: BTreeMap<u64, Tab>,
    n: u64,
    /// Task tree -> tabs being opened now (tab cap).
    opening: HashMap<String, i64>,
    /// Tasks that let go of their tabs.
    pub(crate) released: HashSet<String>,
    keep_roots: HashSet<String>,
}

fn alias_number(alias: &str) -> Option<u64> {
    alias.strip_prefix('t')?.parse().ok()
}

/// `_status`: a reply's `http_status`, only as a plain HTTP status number.
fn status(v: &Value) -> Option<i64> {
    v.get("http_status")
        .and_then(Value::as_i64)
        .filter(|n| (100..=599).contains(n) && !v["http_status"].is_boolean())
}

fn text(v: &Value, key: &str) -> String {
    rules::text_or_empty(v.get(key))
}

fn facts_of(v: &Value) -> Dict {
    v.as_object().cloned().unwrap_or_default()
}

/// `_TabDriver` with its `Router` / `FerbIdentity` half.
pub struct Driver {
    pub hub: Arc<Hub>,
    pub identity: Identity,
    pub kind: Kind,
    pub tasks: Tasks,
    pub wake: Option<Arc<dyn Wake>>,
    sealer: Mutex<Sealer>,
    seams: Mutex<crate::ops::Seams>,
    otp: Mutex<crate::owner::Otp>,
    random: Arc<dyn crate::pairing::Random>,
    state: Mutex<State>,
    me: Weak<Driver>,
}

impl Driver {
    pub fn new(
        hub: Arc<Hub>,
        kind: Kind,
        tasks: Tasks,
        wake: Option<Arc<dyn Wake>>,
        random: Arc<dyn crate::pairing::Random>,
    ) -> Arc<Driver> {
        let identity = match kind {
            Kind::Owner { .. } => Identity::Owner,
            Kind::Ferb { .. } => Identity::Ferb,
        };
        let d = Arc::new_cyclic(|me| Driver {
            hub: Arc::clone(&hub),
            identity,
            kind,
            tasks,
            wake,
            sealer: Mutex::new(Arc::new(
                |key: &[u8], value: &Secret, cid: &str, origin: &str| {
                    seal::seal_fill(key, value, cid, origin)
                },
            )),
            seams: Mutex::default(),
            otp: Mutex::default(),
            random,
            state: Mutex::default(),
            me: me.clone(),
        });
        hub.add_driver(Arc::clone(&d) as Arc<dyn TabDriver>);
        d
    }

    /// Replaces how fills are sealed (production: [`seal::seal_fill`], a fresh nonce each time; the
    /// daemon's helper may seal instead; the fixtures replay fixed nonces).
    pub fn set_sealer(&self, sealer: Sealer) {
        *lock(&self.sealer) = sealer;
    }

    /// What the daemon supplies besides the tasks (downloads, uploads, the clipboard, the image
    /// library, native captures, upload files).
    pub fn set_seams(&self, seams: crate::ops::Seams) {
        *lock(&self.seams) = seams;
    }

    /// The seams set now.
    pub fn seams_now(&self) -> crate::ops::Seams {
        lock(&self.seams).clone()
    }

    pub(crate) fn seams_lock(&self) -> MutexGuard<'_, crate::ops::Seams> {
        lock(&self.seams)
    }

    pub(crate) fn otp_lock(&self) -> MutexGuard<'_, crate::owner::Otp> {
        lock(&self.otp)
    }

    pub(crate) fn me(&self) -> Option<Arc<Driver>> {
        self.me.upgrade()
    }

    pub(crate) fn random(&self) -> &dyn crate::pairing::Random {
        self.random.as_ref()
    }

    pub(crate) fn random_arc(&self) -> Arc<dyn crate::pairing::Random> {
        Arc::clone(&self.random)
    }

    fn seal(&self, key: &[u8], value: &Secret, cid: &str, origin: &str) -> Result<Sealed> {
        let sealer = Arc::clone(&lock(&self.sealer));
        sealer(key, value, cid, origin).map_err(|_| Error::refused("the value couldn't be sealed"))
    }

    pub(crate) fn st(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// A tab by its alias, cloned.
    pub fn tab(&self, alias: &str) -> Option<Tab> {
        alias_number(alias).and_then(|n| self.st().tabs.get(&n).cloned())
    }

    pub(crate) fn update(&self, alias: &str, f: impl FnOnce(&mut Tab)) {
        if let Some(n) = alias_number(alias) {
            if let Some(t) = self.st().tabs.get_mut(&n) {
                f(t);
            }
        }
    }

    pub(crate) fn new_ref(&self, br: &Bridge) -> TabRef {
        let mut s = self.st();
        s.n += 1;
        TabRef {
            device_id: br.device_id.clone(),
            browser: br.binding.browser.clone(),
            profile: br.binding.folder.clone(),
            generation: br.gen(),
            tab_id: format!("t{}", s.n),
        }
    }

    pub(crate) fn insert(&self, t: Tab) {
        let n = alias_number(&t.tab.tab_id).expect("an alias this driver made");
        self.st().tabs.insert(n, t);
    }

    /// `_connected`: the one check every browser tool's profile goes through (HAND-403).
    pub async fn connected(&self, br: Arc<Bridge>) -> Result<Arc<Bridge>> {
        if !br.is_connected() {
            if let Some(wake) = &self.wake {
                let (browser, folder) = (br.binding.browser.clone(), br.binding.folder.clone());
                wake.start(&browser, &folder, false);
                if !br.came(Duration::from_secs_f64(WINDOWLESS_WAIT_S)).await {
                    let reason = self.hub.refused(&br.binding.install_id);
                    if reason.is_none() && self.hub.unpaired().iter().any(|(_, b)| *b == browser) {
                        return Err(Error::driver(format!(
                            "{}'s FERB extension is connected but not paired with FERB (a reinstall gives it a new \
                             identity): pair it with browser_pair profile={browser}/{folder} (FERB asks the owner once). \
                             Never work around this through Mac tools (the browser's menus, address bar or clicks): that \
                             takes over the owner's screen.",
                            br.name()
                        )));
                    }
                    if let Some(reason) = reason {
                        return Err(Error::driver(format!(
                            "{}'s FERB extension was refused: {reason}.",
                            br.name()
                        )));
                    }
                    wake.start(&browser, &folder, true);
                    if !br.came(Duration::from_secs_f64(WINDOW_WAIT_S)).await {
                        if let Some(known) = self.hub.problem(&br.binding) {
                            return Err(Error::driver(format!(
                                "{} didn't connect, even with its window open: {known}. Tell the owner; only they can fix this.",
                                br.name()
                            )));
                        }
                        return Err(Error::driver(format!(
                            "{} didn't connect, even with its window open: its FERB extension may be turned off there; ask \
                             the owner to reload the FERB extension in that profile.",
                            br.name()
                        )));
                    }
                }
            }
        }
        if !br.is_connected() {
            return Err(Error::driver(format!("{} isn't connected", br.name())));
        }
        Ok(br)
    }

    /// `_group`: the extension's group record for a task's tab.
    fn group(&self, owner: &str) -> Value {
        let root = (self.tasks.root)(owner);
        json!({"root": root, "task": owner, "title": (self.tasks.redact)(&rules::group_title(&(self.tasks.title)(&root)))})
    }

    fn hold(&self, alias: &str) {
        let mut bridge = None;
        self.update(alias, |t| {
            if !t.attached && !t.held {
                t.held = true;
                bridge = Some(Arc::clone(&t.bridge));
            }
        });
        if let Some(br) = bridge {
            br.hold();
        }
    }

    fn unhold(&self, t: &mut Tab) {
        if t.held {
            t.held = false;
            t.bridge.unhold();
        }
    }

    /// `_drop`: FERB no longer drives this tab.
    fn drop_tab(&self, t: &mut Tab, why: &str) {
        t.closed = true;
        if t.left.is_empty() {
            t.left = why.into();
        }
        self.unhold(t);
    }

    /// `_open_tabs`: tabs FERB opened for this task tree still open, and those being opened now, in
    /// every browser driver of the hub (HAND-234).
    fn open_tab_count(&self, root: &str) -> i64 {
        let drivers = self.hub.drivers();
        let me = self.me.upgrade();
        let listed = me.as_ref().is_some_and(|me| {
            drivers
                .iter()
                .any(|d| std::ptr::addr_eq(Arc::as_ptr(d), Arc::as_ptr(me)))
        });
        if !listed {
            return self.open_count(root);
        }
        drivers.iter().map(|d| d.open_count(root)).sum()
    }

    /// This driver's part of `_open_tabs`.
    pub fn open_count(&self, root: &str) -> i64 {
        let s = self.st();
        let open = s
            .tabs
            .values()
            .filter(|t| {
                t.owner
                    .as_deref()
                    .is_some_and(|o| (self.tasks.root)(o) == root)
                    && !t.closed
                    && !t.keep_open
                    && (!t.attached || t.opener.is_some())
            })
            .count() as i64;
        open + s.opening.get(root).copied().unwrap_or(0)
    }

    /// `_tab_cap`: why this task tree may not open another tab now.
    fn tab_cap(&self, owner: &str) -> Option<String> {
        let cap = (self.tasks.max_tabs)();
        (self.open_tab_count(&(self.tasks.root)(owner)) >= cap).then(|| {
            format!(
                "this task already has {cap} tabs open (browser.task_max_tabs), so FERB didn't open another; close tabs \
                 it no longer needs (close) and open it again"
            )
        })
    }

    fn reserve(&self, owner: &str) -> Result<String> {
        if let Some(why) = self.tab_cap(owner) {
            return Err(Error::refused(why));
        }
        let root = (self.tasks.root)(owner);
        *self.st().opening.entry(root.clone()).or_insert(0) += 1;
        Ok(root)
    }

    fn unreserve(&self, reserved: &mut Option<String>) {
        if let Some(root) = reserved.take() {
            let mut s = self.st();
            let n = s.opening.get(&root).copied().unwrap_or(1) - 1;
            if n <= 0 {
                s.opening.remove(&root);
            } else {
                s.opening.insert(root, n);
            }
        }
    }

    /// `_open_in`: a task tab opens blank (armed, grouped, muted), becomes the task's, then loads its URL.
    #[allow(clippy::too_many_arguments)] // Python's `_open_in` keywords
    pub async fn open_in(
        &self,
        br: Arc<Bridge>,
        url: &str,
        owner: Option<&str>,
        window_id: Option<i64>,
        allow_foreground: bool,
        attached: bool,
        opener: Option<&str>,
        abandon: Option<&(dyn Fn() -> bool + Send + Sync)>,
    ) -> Result<TabRef> {
        let mut reserved = match owner.filter(|o| !o.is_empty()) {
            Some(o) => Some(self.reserve(o)?),
            None => None,
        };
        let r = self
            .open_reserved(
                br,
                url,
                owner,
                &mut reserved,
                window_id,
                allow_foreground,
                attached,
                opener,
                abandon,
            )
            .await;
        self.unreserve(&mut reserved);
        r
    }

    #[allow(clippy::too_many_arguments)]
    async fn open_reserved(
        &self,
        br: Arc<Bridge>,
        url: &str,
        owner: Option<&str>,
        reserved: &mut Option<String>,
        window_id: Option<i64>,
        allow_foreground: bool,
        attached: bool,
        opener: Option<&str>,
        abandon: Option<&(dyn Fn() -> bool + Send + Sync)>,
    ) -> Result<TabRef> {
        let mut args = Map::new();
        args.insert("url".into(), BLANK.into());
        if let Some(w) = window_id {
            args.insert("window_id".into(), w.into());
        }
        if allow_foreground {
            args.insert("allow_foreground".into(), true.into());
        }
        let owner = owner.filter(|o| !o.is_empty());
        if let (Some(o), false) = (owner, attached) {
            args.insert("group".into(), self.group(o)); // the task's collapsed group (HAND-201)
        }
        if !attached {
            args.insert("muted".into(), true.into()); // HAND-207
        }
        let mut v = br
            .call("open", Value::Object(args.clone()), 30.0, None)
            .await?;
        if v.get("opened") == Some(&Value::Bool(false))
            && v.get("reason").and_then(Value::as_str) == Some("window_selection_required")
            && window_id.is_none()
            && !allow_foreground
        {
            // No FERB window in this profile: an open window takes a background tab first; only a profile with no
            // window at all gets FERB's minimized window (owner 2026-10-26).
            let mut windows: Vec<Value> = v
                .get("windows")
                .and_then(Value::as_array)
                .map(|w| {
                    w.iter()
                        .filter(|w| {
                            w.get("type").and_then(Value::as_str).unwrap_or("normal") == "normal"
                                && !w.get("incognito").is_some_and(ferb_py::value::truthy)
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            let chosen = br.window();
            windows.sort_by_key(|w| {
                (
                    w.get("id").and_then(Value::as_i64) != chosen,
                    w.get("focused").is_some_and(ferb_py::value::truthy),
                )
            });
            let mut again = args.clone();
            match windows.first() {
                Some(w) => {
                    again.insert("window_id".into(), w["id"].clone());
                }
                None => {
                    again.insert("allow_foreground".into(), true.into());
                }
            }
            v = br.call("open", Value::Object(again), 30.0, None).await?;
        }
        if v.get("opened") == Some(&Value::Bool(false)) {
            return Err(Error::driver_with(text(&v, "detail"), facts_of(&v)));
        }
        if let Some(w) = v.get("window_id").and_then(Value::as_i64) {
            br.set_window(Some(w)); // remembered for this profile
        }
        let chrome_id = v
            .get("tab_id")
            .and_then(Value::as_i64)
            .ok_or_else(|| Error::driver("the browser opened no tab"))?;
        let tab = self.new_ref(&br);
        let mut t = Tab::new(
            tab.clone(),
            Arc::clone(&br),
            chrome_id,
            owner.map(String::from),
            BLANK.into(),
        );
        t.attached = attached;
        t.muted = v.get("muted") == Some(&Value::Bool(true));
        t.opener = opener.map(String::from);
        self.insert(t);
        self.unreserve(reserved); // it counts as an open tab of the task from here
        self.hold(&tab.tab_id);
        if abandon.is_some_and(|a| a()) {
            let _ = self.close(&tab, None).await;
            return Err(Error::driver(
                "the task had already let go of its tabs; the new tab was closed before it loaded",
            ));
        }
        self.update(&tab.tab_id, |t| t.loading = true);
        let loaded = br
            .call(
                "navigate",
                json!({"tab_id": chrome_id, "url": url}),
                30.0,
                None,
            )
            .await;
        self.update(&tab.tab_id, |t| t.loading = false);
        let loaded = match loaded {
            Ok(v) if v.get("navigated") == Some(&Value::Bool(false)) => {
                let detail = rules::text_or_empty(v.get("detail"));
                Err(Error::driver_with(
                    if detail.is_empty() {
                        "the tab didn't load".into()
                    } else {
                        detail
                    },
                    facts_of(&v),
                ))
            }
            other => other,
        };
        match loaded {
            Err(mut e) => {
                // never a blank task tab the worker doesn't know about
                if let Err(cleanup) = self.close(&tab, None).await {
                    if let Some(facts) = e.facts_mut() {
                        facts.insert("tab".into(), tab.tab_id.clone().into());
                        facts.insert(
                            "tab_left_open".into(),
                            format!("the blank tab couldn't be closed: {cleanup}").into(),
                        );
                    }
                }
                Err(e)
            }
            Ok(loaded) => {
                let new_url = loaded
                    .get("url")
                    .and_then(Value::as_str)
                    .filter(|u| !u.is_empty())
                    .unwrap_or(url)
                    .to_string();
                self.update(&tab.tab_id, |t| {
                    t.url = new_url;
                    t.http_status = status(&loaded);
                });
                Ok(tab)
            }
        }
    }

    /// `keep_tabs_open`: at the task's end its tabs leave the group and stay open (HAND-201).
    pub fn keep_tabs_open(&self, task: &str) {
        let root = (self.tasks.root)(task);
        self.st().keep_roots.insert(root);
    }

    /// `_tab`: a tab still FERB's, on its current connection.
    pub fn live_tab(&self, tab: &TabRef) -> Result<Tab> {
        let t = self
            .tab(&tab.tab_id)
            .ok_or_else(|| Error::stale(format!("tab {} is gone", tab.tab_id)))?;
        if t.closed {
            let left = if t.left.is_empty() {
                "it was closed in the browser"
            } else {
                &t.left
            };
            return Err(Error::stale(format!(
                "tab {} is no longer FERB's: {left}",
                tab.tab_id
            )));
        }
        if t.bridge.gen() != tab.generation || !t.bridge.is_connected() {
            return Err(Error::stale(format!(
                "tab {} belongs to an older browser connection",
                tab.tab_id
            )));
        }
        Ok(t)
    }

    /// `element_ref`: an element id of a snapshot ("s3e12") as a reference.
    pub fn element_ref(&self, tab: &TabRef, element_id: &str) -> ElementRef {
        let head = element_id
            .strip_prefix('s')
            .map(|r| r.split('e').next().unwrap_or(""))
            .unwrap_or("");
        let gen = if !head.is_empty() && head.chars().all(lax_digit) {
            py_int(head)
        } else {
            -1
        };
        ElementRef {
            tab: tab.clone(),
            frame_id: String::new(),
            snapshot_gen: gen,
            element_id: element_id.into(),
        }
    }

    /// `_resolve`: the element of the latest snapshot (or carried into it), on a page that hasn't loaded since.
    pub fn resolve(&self, el: &ElementRef) -> Result<(Tab, i64, i64, i64, PageElement)> {
        let t = self.live_tab(&el.tab)?;
        let hit = t.elements.get(&el.element_id).cloned();
        let Some((frame, snap, local, info)) =
            hit.filter(|_| el.snapshot_gen == t.snap_gen || t.carried.contains(&el.element_id))
        else {
            return Err(Error::stale(format!(
                "element {} is from an older snapshot; take a new one",
                el.element_id
            )));
        };
        if t.bridge.navs(t.chrome_id) != t.snap_nav {
            return Err(Error::stale(
                "the page changed since the snapshot; take a new one",
            ));
        }
        Ok((t, frame, snap, local, info))
    }

    pub(crate) fn accessible(&self, t: &Tab, task: &str) -> bool {
        t.owner.as_deref().is_some_and(|o| {
            o == task || Some(o.to_string()) == (self.tasks.inspection_parent)(task)
        })
    }

    /// `owned_by`: `owner`'s tab (with `borrow`, a registered checker reaches its worker's tabs too).
    pub fn owned_by(&self, tab_id: &str, owner: &str, borrow: bool) -> Result<TabRef> {
        let ok = |t: &Tab| {
            if borrow {
                self.accessible(t, owner)
            } else {
                t.owner.as_deref() == Some(owner)
            }
        };
        if let Some(t) = self.tab(tab_id).filter(|t| ok(t)) {
            return Ok(t.tab);
        }
        let open: Vec<String> = self
            .st()
            .tabs
            .values()
            .filter(|t| !t.closed && ok(t))
            .map(|t| t.tab.tab_id.clone())
            .collect();
        let listed = if open.is_empty() {
            "none".to_string()
        } else {
            open.join(", ")
        };
        Err(Error::refused(format!(
            "no tab {tab_id} open for this task; its open tabs: {listed} (browser.tabs lists them)"
        )))
    }

    /// `tab_chain`: the first task tab of the chain this tab was opened from (GATE-204).
    pub fn tab_chain(&self, tab_id: &str) -> String {
        let mut seen: HashSet<String> = HashSet::from([tab_id.to_string()]);
        let mut at = tab_id.to_string();
        while let Some(opener) = self
            .tab(&at)
            .and_then(|t| t.opener)
            .filter(|o| !o.is_empty())
        {
            if !seen.insert(opener.clone()) {
                break;
            }
            at = opener;
        }
        at
    }

    /// `_live`: this driver's tab with that browser id, driven by a task still running (not a loop's
    /// borrowed tab resting between rounds).
    pub(crate) fn live(&self, br: &Arc<Bridge>, chrome_id: i64) -> Option<Tab> {
        self.st()
            .tabs
            .values()
            .find(|t| {
                Arc::ptr_eq(&t.bridge, br)
                    && t.chrome_id == chrome_id
                    && !t.closed
                    && t.owner.as_deref().is_some_and(|o| {
                        !o.is_empty()
                            && (self.tasks.running)(o)
                            && !(t.attached && (self.tasks.resting)(o))
                    })
            })
            .cloned()
    }

    /// `_closed_news` (HAND-511): told in the next result of the tab that opened it, else of the task's first tab.
    pub(crate) fn closed_news(&self, t: &Tab, why: &str) {
        let entry: Map<String, Value> = [
            ("tab".to_string(), Value::from(t.tab.tab_id.clone())),
            ("closed".into(), why.into()),
        ]
        .into_iter()
        .collect();
        let mut s = self.st();
        let to = t
            .opener
            .as_deref()
            .and_then(alias_number)
            .filter(|n| s.tabs.get(n).is_some_and(|o| !o.closed))
            .or_else(|| {
                s.tabs
                    .iter()
                    .find(|(_, o)| o.owner == t.owner && !o.closed && o.tab.tab_id != t.tab.tab_id)
                    .map(|(n, _)| *n)
            });
        if let Some(n) = to {
            s.tabs.get_mut(&n).expect("found").closed_news.push(entry);
        }
    }

    /// `closed_for`: tabs of this task that closed themselves, not yet reported.
    pub fn closed_for(&self, tab: &TabRef) -> Vec<Map<String, Value>> {
        let mut out = vec![];
        self.update(&tab.tab_id, |t| out = std::mem::take(&mut t.closed_news));
        out
    }

    /// `opened_for`: new tabs this tab started, now this task's, not yet reported.
    pub fn opened_for(&self, tab: &TabRef) -> Vec<Map<String, Value>> {
        let mut out = vec![];
        self.update(&tab.tab_id, |t| out = std::mem::take(&mut t.opened));
        out
    }

    /// `_note_input` (HAND-447).
    pub(crate) fn note_input(&self, t: &Tab) {
        let at = t.bridge.navs(t.chrome_id);
        self.update(&t.tab.tab_id, |x| x.typed_at = Some(at));
    }

    /// `typed_since_load` (HAND-447).
    pub fn typed_since_load(&self, tab: &TabRef) -> bool {
        self.tab(&tab.tab_id)
            .is_some_and(|t| t.typed_at.is_some_and(|n| n == t.bridge.navs(t.chrome_id)))
    }

    /// `closes`: whether `close` would close this tab rather than only let go of it.
    pub fn closes(&self, tab: &TabRef) -> bool {
        self.tab(&tab.tab_id)
            .is_some_and(|t| !t.attached && !t.keep_open && !t.closed)
    }

    pub fn http_status(&self, tab: &TabRef) -> Option<i64> {
        self.tab(&tab.tab_id).and_then(|t| t.http_status)
    }

    pub fn tabs(&self) -> Vec<TabRef> {
        self.st().tabs.values().map(|t| t.tab.clone()).collect()
    }

    pub fn urls_of(&self, owner: &str) -> Vec<String> {
        self.st()
            .tabs
            .values()
            .filter(|t| t.owner.as_deref() == Some(owner))
            .map(|t| t.url.clone())
            .collect()
    }

    /// `task_tabs`: the task's tabs by tab id.
    pub fn task_tabs(&self, owner: &str) -> Vec<Map<String, Value>> {
        self.st()
            .tabs
            .values()
            .filter(|t| t.owner.as_deref() == Some(owner))
            .map(|t| {
                let mut m = Map::new();
                m.insert("tab".into(), t.tab.tab_id.clone().into());
                if t.closed {
                    let left = if t.left.is_empty() {
                        "the owner closed this tab"
                    } else {
                        &t.left
                    };
                    m.insert("closed".into(), left.into());
                }
                if !t.taken_over_from.is_empty() {
                    m.insert(
                        "tab_ref".into(),
                        format!("{}:{}:{}", t.tab.browser, t.tab.profile, t.chrome_id).into(),
                    );
                    m.insert(
                        "taken_over_from".into(),
                        Value::Object(t.taken_over_from.clone()),
                    );
                }
                m
            })
            .collect()
    }

    /// `navigate` (`leave_page`, HAND-442: the worker's choice for a page that asks before it is left).
    pub async fn navigate(&self, tab: &TabRef, url: &str, leave_page: Option<&str>) -> Result<()> {
        let t = self.live_tab(tab)?;
        let mut args = json!({"tab_id": t.chrome_id, "url": url});
        if let Some(l) = leave_page.filter(|l| !l.is_empty()) {
            args["leave_page"] = l.into();
        }
        let v = t.bridge.call("navigate", args, 30.0, None).await?;
        if v.get("navigated") == Some(&Value::Bool(false)) {
            let detail = text(&v, "detail");
            return Err(Error::driver_with(
                if detail.is_empty() {
                    "the tab didn't navigate".into()
                } else {
                    detail
                },
                facts_of(&v),
            ));
        }
        let new_url = v
            .get("url")
            .and_then(Value::as_str)
            .filter(|u| !u.is_empty())
            .unwrap_or(url)
            .to_string();
        self.update(&tab.tab_id, |t| {
            t.url = new_url;
            t.http_status = status(&v);
        });
        Ok(())
    }

    fn lifecycle(t: &Tab, result: &Value) -> Map<String, Value> {
        let flag = |k: &str| result.get(k).is_some_and(ferb_py::value::truthy);
        let url = result
            .get("url")
            .filter(|u| ferb_py::value::truthy(u))
            .cloned()
            .unwrap_or_else(|| t.url.clone().into());
        let mut m = Map::new();
        m.insert("tab".into(), t.tab.tab_id.clone().into());
        m.insert(
            "tab_ref".into(),
            format!("{}:{}:{}", t.tab.browser, t.tab.profile, t.chrome_id).into(),
        );
        m.insert(
            "profile".into(),
            format!("{}/{}", t.tab.browser, t.tab.profile).into(),
        );
        m.insert("url".into(), url);
        m.insert("borrowed".into(), t.attached.into());
        m.insert("closed".into(), flag("closed").into());
        m.insert("kept_open".into(), flag("kept_open").into());
        m.insert(
            "released".into(),
            (flag("released") || flag("closed")).into(),
        );
        m
    }

    /// `keep_open`: keep the tab open after the task (a borrowed tab: held until its task ends).
    pub async fn keep_open(&self, tab: &TabRef) -> Result<Map<String, Value>> {
        let t = self.live_tab(tab)?;
        if t.attached {
            let listed = t.bridge.call("list_tabs", json!({}), 10.0, None).await?;
            let open = listed
                .get("tabs")
                .and_then(Value::as_array)
                .is_some_and(|tabs| {
                    tabs.iter()
                        .any(|x| x.get("tab_id").and_then(Value::as_i64) == Some(t.chrome_id))
                });
            if !open {
                return Err(Error::driver(format!(
                    "tab {} isn't open in the browser any more, so it can't be kept open",
                    tab.tab_id
                )));
            }
            self.update(&tab.tab_id, |x| x.keep_open = true);
            let mut out = Self::lifecycle(&t, &json!({"kept_open": true}));
            out.insert(
                "held_until_task_end".into(),
                "the browser has the tab open now; FERB keeps holding it while the task runs and lets go of it, open, \
                 when the task ends"
                    .into(),
            );
            return Ok(out);
        }
        self.update(&tab.tab_id, |x| x.keep_open = true);
        let v = t
            .bridge
            .call(
                "release_tab",
                json!({"tab_id": t.chrome_id, "keep_open": true}),
                10.0,
                None,
            )
            .await?;
        if !v.get("released").is_some_and(ferb_py::value::truthy)
            || !v.get("kept_open").is_some_and(ferb_py::value::truthy)
        {
            return Err(Error::driver(
                "browser did not confirm keeping the tab open and releasing control",
            ));
        }
        Ok(Self::lifecycle(&t, &v))
    }

    /// `close`: closes a tab FERB opened, lets go of a borrowed or kept one.
    pub async fn close(
        &self,
        tab: &TabRef,
        leave_page: Option<&str>,
    ) -> Result<Map<String, Value>> {
        let Some(t) = self.tab(&tab.tab_id) else {
            let mut m = Map::new();
            m.insert("tab".into(), tab.tab_id.clone().into());
            m.insert("released".into(), true.into());
            m.insert("closed".into(), false.into());
            m.insert("already_released".into(), true.into());
            return Ok(m);
        };
        let v = if t.closed {
            json!({"closed": true, "released": true})
        } else if !t.bridge.is_connected() {
            return Err(Error::driver(format!(
                "cannot confirm cleanup of {}: {} is disconnected",
                tab.tab_id,
                t.bridge.name()
            )));
        } else if t.attached || t.keep_open {
            let v = t
                .bridge
                .call(
                    "release_tab",
                    json!({"tab_id": t.chrome_id, "keep_open": t.keep_open}),
                    10.0,
                    None,
                )
                .await?;
            let truthy = |k: &str| v.get(k).is_some_and(ferb_py::value::truthy);
            if !truthy("released") || (t.keep_open && !truthy("kept_open")) {
                return Err(Error::driver(format!(
                    "browser did not confirm releasing {}",
                    tab.tab_id
                )));
            }
            v
        } else {
            if t.bridge.gen() != t.tab.generation {
                return Err(Error::driver(format!(
                    "cannot confirm cleanup of {}: browser connection changed",
                    tab.tab_id
                )));
            }
            self.update(&tab.tab_id, |x| x.closing = true);
            let mut args = json!({"tab_id": t.chrome_id});
            if let Some(l) = leave_page.filter(|l| !l.is_empty()) {
                args["leave_page"] = l.into();
            }
            let v = match t.bridge.call("close", args, 10.0, None).await {
                Ok(v) => v,
                Err(e) => return Err(e),
            };
            let reason = v.get("reason").and_then(Value::as_str);
            if matches!(reason, Some("dialog_pending" | "stayed")) {
                self.update(&tab.tab_id, |x| x.closing = false);
                let detail = text(&v, "detail");
                return Err(Error::driver_with(
                    if detail.is_empty() {
                        "the page asked before closing".into()
                    } else {
                        detail
                    },
                    facts_of(&v),
                ));
            }
            let truthy = |k: &str| v.get(k).is_some_and(ferb_py::value::truthy);
            if reason == Some("not_ferbs") {
                let mut v = v.clone();
                v["released"] = true.into();
                v["left_open"] = true.into();
                v
            } else if !truthy("closed") && !(truthy("kept_open") && truthy("released")) {
                self.update(&tab.tab_id, |x| x.closing = false);
                return Err(Error::driver(format!(
                    "browser did not confirm closing {}",
                    tab.tab_id
                )));
            } else {
                v
            }
        };
        if let Some(n) = alias_number(&tab.tab_id) {
            if let Some(mut gone) = self.st().tabs.remove(&n) {
                self.unhold(&mut gone);
            }
        }
        if v.get("closed").is_some_and(ferb_py::value::truthy) {
            if let Some(uploads) = self.seams().uploads {
                // rule 17: its upload copies go with its tree
                uploads.closed(&crate::upload::tab_key(&t.bridge.name(), t.chrome_id));
            }
        }
        Ok(Self::lifecycle(&t, &v))
    }

    /// `release`: the task's end (HAND-201). Each tab retried once; one that still can't be let go
    /// of becomes no task's, named in the error's `tabs` by browser, profile, account and title
    /// (HAND-419). FERB's own profile's inbox inventories of the task go too.
    pub async fn release(&self, owner: &str) -> Result<Vec<Map<String, Value>>> {
        let r = self.release_tabs(owner).await;
        if matches!(self.kind, Kind::Ferb { .. }) {
            self.forget_otp(owner);
        }
        r
    }

    async fn release_tabs(&self, owner: &str) -> Result<Vec<Map<String, Value>>> {
        if (self.tasks.paused)(owner) {
            return Ok(vec![]);
        }
        let keep_all = self.st().keep_roots.contains(&(self.tasks.root)(owner));
        {
            let mut s = self.st();
            let running = &self.tasks.running;
            s.released.retain(|o| running(o));
            s.released.insert(owner.to_string());
        }
        let aliases: Vec<String> = self
            .st()
            .tabs
            .values()
            .filter(|t| t.owner.as_deref() == Some(owner))
            .map(|t| t.tab.tab_id.clone())
            .collect();
        let (mut results, mut errors, mut left) = (vec![], vec![], vec![]);
        for alias in aliases {
            if keep_all {
                self.update(&alias, |t| {
                    if !t.attached && !t.closed {
                        t.keep_open = true;
                    }
                });
            }
            for attempt in [1, 2] {
                let Some(t) = self.tab(&alias) else { break };
                match self.close(&t.tab, None).await {
                    Ok(r) => {
                        results.push(r);
                        break;
                    }
                    Err(e) if e.is_driver_error() => {
                        if attempt == 2 || e.reason() == Some("dialog_pending") {
                            self.update(&alias, |t| t.owner = None);
                            errors.push(format!("{alias}: {e}"));
                            let now = self.tab(&alias).unwrap_or(t);
                            left.push(self.leftover(&now, &e.to_string()));
                            break;
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        {
            let s = self.st();
            let live: HashSet<String> = s
                .tabs
                .values()
                .filter_map(|t| t.owner.as_deref().map(|o| (self.tasks.root)(o)))
                .collect();
            drop(s);
            let running = &self.tasks.running;
            self.st()
                .keep_roots
                .retain(|r| live.contains(r) || running(r));
        }
        if !errors.is_empty() {
            let mut facts = Map::new();
            facts.insert("tabs".into(), Value::Array(left));
            return Err(Error::driver_with(
                format!("browser cleanup incomplete: {}", errors.join("; ")),
                facts,
            ));
        }
        Ok(results)
    }

    /// `snapshot`: the page's elements with ids of this snapshot ("s<gen>e<n>"), its text and frames;
    /// the full form is saved for `read`.
    pub async fn snapshot(&self, tab: &TabRef) -> Result<PageSnapshot> {
        let t = self.live_tab(tab)?;
        let nav = t.bridge.navs(t.chrome_id);
        let gen = t.snap_gen + 1;
        let v = match t
            .bridge
            .call(
                "snapshot",
                json!({"tab_id": t.chrome_id, "snap": gen}),
                30.0,
                None,
            )
            .await
        {
            Ok(v) => v,
            Err(e) => {
                let stuck = e.reason() == Some("page_not_responding");
                self.update(&tab.tab_id, |x| {
                    x.stuck = stuck;
                    x.timing = None;
                    x.shot_timing = None;
                });
                return Err(e);
            }
        };
        let frames: Vec<Value> = v
            .get("frames")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| Error::driver("a snapshot without frames"))?;
        let mut elements: Vec<(String, (i64, i64, i64, PageElement))> = vec![];
        let mut containers = HashMap::new();
        let (mut text, mut title, mut url, mut truncated) =
            (String::new(), String::new(), t.url.clone(), false);
        let mut scroll: Option<Dict> = None;
        let mut top = Value::Object(Map::new());
        let (mut scrollers, mut media) = (vec![], vec![]);
        let (mut media_refs, mut media_ids) = (HashMap::new(), HashMap::new());
        let mut motions = HashMap::new();
        for f in &frames {
            let fid = rules::frame_id(f);
            for m in f
                .get("media")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let index = media.len() as i64;
                media_refs.insert(
                    index,
                    (fid, m.get("index").and_then(Value::as_i64).unwrap_or(0)),
                );
                let mut entry = Map::new();
                entry.insert("index".into(), index.into());
                if let Some(id) = m
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| media_id(id))
                {
                    let id = if fid == 0 {
                        id.to_string()
                    } else {
                        format!("f{fid}:{id}")
                    };
                    media_ids.insert(id.clone(), index);
                    entry.insert("id".into(), id.into());
                }
                for k in MEDIA_FIELDS {
                    if let Some(x) = m.get(k) {
                        entry.insert(k.into(), x.clone());
                    }
                }
                if fid != 0 {
                    entry.insert("frame".into(), fid.into());
                }
                media.push(entry);
            }
            for sc in f
                .get("scrollers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let local = rules::text_or_empty(sc.get("id"));
                let local = if local.is_empty() {
                    ferb_py::value::py_str(sc.get("id").unwrap_or(&Value::Null))
                } else {
                    local
                };
                let alias = if fid == 0 {
                    local.clone()
                } else {
                    format!("f{fid}:{local}")
                };
                containers.insert(alias.clone(), (fid, local));
                let mut entry = Map::new();
                for k in ["label", "axis", "y", "height", "viewport_h", "at_end"] {
                    if let Some(x) = sc.get(k) {
                        entry.insert(k.into(), x.clone());
                    }
                }
                if matches!(sc.get("axis").and_then(Value::as_str), Some("x" | "both")) {
                    for k in ["x", "width", "viewport_w"] {
                        if let Some(x) = sc.get(k) {
                            entry.insert(k.into(), x.clone());
                        }
                    }
                }
                entry.insert("id".into(), alias.into());
                entry.insert("frame".into(), fid.into());
                scrollers.push(entry);
            }
            truncated = truncated || f.get("truncated").is_some_and(ferb_py::value::truthy);
            for raw in f
                .get("elements")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let eid = format!("s{gen}e{}", elements.len() + 1);
                let mut raw = raw.as_object().cloned().unwrap_or_default();
                if fid != 0 {
                    if let Some(c) = raw.get("container").filter(|c| ferb_py::value::truthy(c)) {
                        let c = format!("f{fid}:{}", ferb_py::value::py_str(c));
                        raw.insert("container".into(), c.into());
                    }
                }
                let local = raw
                    .get("id")
                    .and_then(|i| crate::lax::int(i).ok())
                    .ok_or_else(|| Error::driver("a snapshot element without an id"))?;
                let e = snapshot::element(&eid, &fid.to_string(), &raw)?;
                let m = motion(&raw);
                if m.0.is_some() || m.1 {
                    motions.insert(eid.clone(), m);
                }
                elements.push((eid, (fid, gen, local, e)));
            }
            if fid == 0 {
                text = f
                    .get("text")
                    .map(|x| {
                        x.as_str()
                            .map_or_else(|| ferb_py::value::py_str(x), String::from)
                    })
                    .unwrap_or_default();
                title = f
                    .get("title")
                    .map(|x| {
                        x.as_str()
                            .map_or_else(|| ferb_py::value::py_str(x), String::from)
                    })
                    .unwrap_or_default();
                if let Some(u) = f.get("url") {
                    url = u
                        .as_str()
                        .map_or_else(|| ferb_py::value::py_str(u), String::from);
                }
                scroll = f
                    .get("scroll")
                    .and_then(Value::as_object)
                    .map(|s| {
                        s.iter()
                            .filter(|(k, _)| *k != "x" && *k != "width")
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect::<Map<_, _>>()
                    })
                    .filter(|s| !s.is_empty());
                top = f.clone();
            }
        }
        let child_text: Vec<String> = frames
            .iter()
            .filter(|f| {
                rules::frame_id(f) != 0 && f.get("text").is_some_and(ferb_py::value::truthy)
            })
            .map(|f| rules::text_or_empty(f.get("text")))
            .collect();
        if !child_text.is_empty() {
            text = format!("{text}\n[Embedded frames]\n{}", child_text.join("\n"));
        }
        let dialogs: Vec<String> = v
            .get("dialogs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .map(|d| {
                let shown = |k: &str| {
                    d.get(k).map_or_else(
                        || "None".into(),
                        |x| {
                            x.as_str()
                                .map_or_else(|| ferb_py::value::py_str(x), String::from)
                        },
                    )
                };
                format!(
                    "{}: {} ({})",
                    shown("type"),
                    rules::text_or_empty(d.get("message")),
                    shown("handled")
                )
            })
            .collect();
        let redacted_title = if title.is_empty() {
            t.title.clone()
        } else {
            (self.tasks.redact)(&title)
        };
        let frame_urls: BTreeMap<i64, String> = frames
            .iter()
            .map(|f| (rules::frame_id(f), rules::text_or_empty(f.get("url"))))
            .collect();
        let commerce = frames
            .iter()
            .find(|f| rules::frame_id(f) == 0 && f.get("commerce").is_some_and(Value::is_object))
            .and_then(|f| f["commerce"].as_object().cloned())
            .unwrap_or_default();
        let kind = top
            .get("content_type")
            .and_then(Value::as_str)
            .filter(|k| !k.is_empty())
            .map(|k| ferb_py::value::prefix(k, 100).to_string());
        let result = PageSnapshot {
            tab: tab.clone(),
            generation: gen,
            url: url.clone(),
            title,
            elements: elements.iter().map(|(_, h)| h.3.clone()).collect(),
            text,
            truncated,
            dialogs,
            scroll,
            scrollers,
            media,
            frame_urls: rules::frame_urls(&frames),
            loading: v.get("loading") == Some(&Value::Bool(true)),
            content_type: kind,
        };
        let mut full = snapshot::for_model(&result, true, false);
        full.insert(
            "frames".into(),
            Value::Array(
                frames
                    .iter()
                    .map(|f| {
                        Value::Object(
                            ["frame_id", "url", "text", "truncated"]
                                .iter()
                                .filter_map(|k| f.get(*k).map(|x| (k.to_string(), x.clone())))
                                .collect(),
                        )
                    })
                    .collect(),
            ),
        );
        let saved = ferb_py::json::dumps_utf8(&Value::Object(full));
        let timing = rules::steps(v.get("steps"));
        let amounts = crate::money::page_amounts(
            std::iter::once(Some(result.text.as_str()))
                .chain(elements.iter().map(|(_, h)| Some(h.3.label.as_str()))),
        );
        let page = Page::of(v.get("window"), &top);
        self.update(&tab.tab_id, |x| {
            x.stuck = false;
            x.timing = timing;
            x.shot_timing = None;
            x.visual = None;
            x.points.clear();
            x.ids_note = Map::new();
            x.amounts = amounts;
            x.page = Some(page);
            x.motion = motions;
            x.moved = false;
            x.snap_gen = gen;
            x.snap_nav = nav;
            x.elements = elements.into_iter().collect();
            x.url = url;
            x.carried.clear();
            x.title = redacted_title;
            x.frame_urls = frame_urls;
            x.commerce = commerce;
            x.containers = containers;
            x.media = media_refs;
            x.media_ids = media_ids;
            x.snapshot_json = saved;
        });
        Ok(result)
    }

    /// `read`: the latest snapshot's saved full form, paged by characters.
    pub fn read(
        &self,
        tab: &TabRef,
        offset: usize,
        max_chars: usize,
    ) -> Result<Map<String, Value>> {
        let t = self.live_tab(tab)?;
        if t.snapshot_json.is_empty() {
            return Err(Error::refused(
                "take a browser snapshot before reading its full evidence",
            ));
        }
        let chars: Vec<char> = t.snapshot_json.chars().collect();
        let end = (offset + max_chars).min(chars.len());
        let text: String = chars
            .get(offset.min(end)..end)
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        let mut m = Map::new();
        m.insert("tab".into(), tab.tab_id.clone().into());
        m.insert("snapshot".into(), t.snap_gen.into());
        m.insert("text".into(), text.into());
        m.insert("offset".into(), offset.into());
        m.insert(
            "next_offset".into(),
            if end < chars.len() {
                Value::from(end)
            } else {
                Value::Null
            },
        );
        m.insert("total_chars".into(), chars.len().into());
        Ok(m)
    }

    /// `dialog`: the tab's pending dialog (a read), or the worker's decision on it.
    pub async fn dialog(
        &self,
        tab: &TabRef,
        decision: Option<&str>,
        text: Option<&str>,
    ) -> Result<Value> {
        let t = self.live_tab(tab)?;
        let mut args = json!({"tab_id": t.chrome_id, "decision": decision});
        if let Some(x) = text {
            args["text"] = x.into();
        }
        t.bridge.call("dialog", args, 10.0, None).await
    }

    /// `open_for`: the task's new tab in the profile this driver takes.
    pub async fn open_for(
        &self,
        url: &str,
        owner: &str,
        profile: Option<&str>,
        window_id: Option<i64>,
        allow_foreground: bool,
    ) -> Result<TabRef> {
        let br = match &self.kind {
            Kind::Owner { choose, .. } => {
                let (browser, folder) = choose(profile)?;
                let br = self.hub.bridge(&browser, &folder)?.ok_or_else(|| {
                    Error::refused(format!("{browser}/{folder} isn't paired with FERB"))
                })?;
                if br.binding.identity != Identity::Owner {
                    return Err(Error::refused(
                        "that profile belongs to FERB's identity, not the owner",
                    ));
                }
                br
            }
            Kind::Ferb { .. } => self.ferb_bridge()?,
        };
        let br = self.connected(br).await?;
        self.open_in(
            br,
            url,
            Some(owner),
            window_id,
            allow_foreground,
            false,
            None,
            None,
        )
        .await
    }

    pub(crate) fn ferb_bridge(&self) -> Result<Arc<Bridge>> {
        let Kind::Ferb { profile, .. } = &self.kind else {
            return Err(Error::refused("FERB's identity profile isn't paired yet"));
        };
        let br = match profile() {
            Some((b, f)) => self.hub.bridge(&b, &f)?,
            None => None,
        };
        br.filter(|br| br.binding.identity == Identity::Ferb)
            .ok_or_else(|| Error::refused("FERB's identity profile isn't paired yet"))
    }

    /// `Router.attach`: take over one of the owner's open tabs for this task (tab borrowing).
    pub async fn attach(&self, tab_ref: &str, owner: &str) -> Result<TabRef> {
        let parts: Vec<&str> = tab_ref.rsplitn(3, ':').collect();
        let bad = || {
            Error::refused(format!(
                "{} isn't a tab reference from browser.tabs",
                ferb_identity::py::repr(tab_ref)
            ))
        };
        if parts.len() != 3 {
            return Err(bad());
        }
        let (folder, browser) = (parts[1], parts[2]);
        let cid = py_int_strict(parts[0]).ok_or_else(bad)?;
        let br = self
            .hub
            .bridge(browser, folder)?
            .filter(|b| b.binding.identity == Identity::Owner);
        let br = br.ok_or_else(|| {
            Error::refused("that profile isn't one of the owner's paired profiles")
        })?;
        let br = self.connected(br).await?;
        let existing = self
            .st()
            .tabs
            .values()
            .find(|t| Arc::ptr_eq(&t.bridge, &br) && t.chrome_id == cid && !t.closed)
            .cloned();
        if let Some(t) = existing {
            if !self.accessible(&t, owner) {
                return Err(Error::refused("another task is using that tab"));
            }
            return Ok(t.tab);
        }
        let v = br.call("list_tabs", json!({}), 10.0, None).await?;
        let found = v
            .get("tabs")
            .and_then(Value::as_array)
            .and_then(|tabs| {
                tabs.iter()
                    .find(|x| x.get("tab_id").and_then(|i| crate::lax::int(i).ok()) == Some(cid))
                    .cloned()
            })
            .ok_or_else(|| Error::refused("that tab isn't open any more"))?;
        let tab = self.new_ref(&br);
        let (url, title) = (text(&found, "url"), text(&found, "title"));
        let mut t = Tab::new(
            tab.clone(),
            Arc::clone(&br),
            cid,
            Some(owner.into()),
            url.clone(),
        );
        t.attached = true;
        t.title = (self.tasks.redact)(&title);
        // stored redacted, so every later reader is safe
        t.taken_over_from
            .insert("url".into(), (self.tasks.redact)(&url).into());
        t.taken_over_from
            .insert("title".into(), (self.tasks.redact)(&title).into());
        self.insert(t);
        // recorded (never grouped) so a reconnect re-adopts it; best effort
        let record = json!({"tab_id": cid, "task": owner, "root": (self.tasks.root)(owner), "borrowed": true});
        let _ = br.call("adopt_tab", record, 10.0, None).await;
        Ok(tab)
    }

    /// The frame's origin, as the extension reads it in the page (`frame_origin`).
    pub(crate) async fn frame_origin(&self, t: &Tab, frame: i64) -> Result<String> {
        let v = t
            .bridge
            .call(
                "frame_origin",
                json!({"tab_id": t.chrome_id, "frame_id": frame}),
                10.0,
                None,
            )
            .await?;
        Ok(text(&v, "origin"))
    }

    /// `FerbIdentity.fill_secret`: one of FERB's own secrets into a field, sealed. The element is
    /// checked first (this snapshot's, the page unchanged), then the frame's exact origin against
    /// the secret's allowed origins, and only then is the Keychain read.
    pub async fn fill_secret(
        &self,
        el: &ElementRef,
        secret: &SecretRef,
        intent_id: &str,
    ) -> Result<ActionResult> {
        let Kind::Ferb { secrets, .. } = &self.kind else {
            return Err(Error::refused(
                "FERB's secrets are never used in the owner's profiles",
            ));
        };
        if !secrets.has(secret) {
            return Err(Error::refused(format!(
                "no secret {} is set up",
                secret.key
            )));
        }
        let (t, frame, snap, local, info) = self.resolve(el)?;
        no_secret_into_terminal(&info)?;
        let origin = self.checked_origin(&t, frame, secret).await?;
        let value = secrets.reveal(secret).await.map_err(|reason| {
            Error::refused(if reason.is_empty() {
                format!("secret {} isn't in the Keychain", secret.key)
            } else {
                format!(
                    "secret {} couldn't be read from the Keychain: {reason}",
                    secret.key
                )
            })
        })?;
        let r = self
            .sealed_fill(&t, frame, snap, local, &origin, value, intent_id)
            .await?;
        Ok(ActionResult {
            ok: r.ok,
            detail: format!("{} ({})", r.detail, secret.key),
            facts: Dict::new(),
            observed: None,
        })
    }

    /// `_checked_origin`: the frame's origin, refused unless the secret allows it exactly.
    pub(crate) async fn checked_origin(
        &self,
        t: &Tab,
        frame: i64,
        secret: &SecretRef,
    ) -> Result<String> {
        let Kind::Ferb { secrets, .. } = &self.kind else {
            unreachable!("FERB's identity only")
        };
        let origin = self.frame_origin(t, frame).await?;
        let allowed = secrets.spec(secret).is_some_and(|spec| {
            ferb_identity::secrets::origin_allowed(&origin, &spec).unwrap_or(false)
        });
        if !allowed {
            let shown = if origin.is_empty() {
                "unknown origin"
            } else {
                &origin
            };
            return Err(Error::refused(format!(
                "refused: this field's page ({shown}) isn't allowed for {}",
                secret.key
            )));
        }
        ferb_identity::origin::normalize_origin(&origin).map_err(|_| {
            Error::refused(format!(
                "refused: this field's page ({origin}) isn't allowed for {}",
                secret.key
            ))
        })
    }

    /// `_sealed_fill`: the value sealed to this command and origin; the extension opens it only
    /// there, re-checks the origin in the same in-page call that sets it, and never echoes it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn sealed_fill(
        &self,
        t: &Tab,
        frame: i64,
        snap: i64,
        local: i64,
        origin: &str,
        value: Secret,
        intent_id: &str,
    ) -> Result<ActionResult> {
        let key = self
            .hub
            .pairings
            .key(&t.bridge.binding.install_id)
            .map_err(|e| Error::driver(e.to_string()))?
            .ok_or_else(|| Error::refused("FERB's identity profile isn't paired"))?;
        let cid = if intent_id.is_empty() {
            format!("f-{}", crate::pairing::token_hex(self.random.as_ref(), 8))
        } else {
            intent_id.to_string()
        };
        let sealed = self.seal(key.expose(), &value, &cid, origin)?;
        drop(value);
        self.note_input(t);
        let v = t
            .bridge
            .call(
                "fill",
                json!({"tab_id": t.chrome_id, "frame_id": frame, "snap": snap, "el": local, "sealed": seal::wire(&sealed), "origin": origin}),
                15.0,
                Some(cid),
            )
            .await?;
        if v.get("detail").and_then(Value::as_str) == Some("stale") {
            return Err(Error::stale("the element is gone; take a new snapshot"));
        }
        Ok(ActionResult {
            ok: v.get("ok").is_some_and(ferb_py::value::truthy),
            detail: text(&v, "detail"),
            facts: Dict::new(),
            observed: None,
        })
    }

    /// `Router.owner_code`: the owner's login code into a one-time-code field, asked through
    /// hidden input, sealed to an HTTPS origin; the element is checked again after the owner answers.
    pub async fn owner_code(
        &self,
        el: &ElementRef,
        task: &str,
        intent_id: &str,
    ) -> Result<ActionResult> {
        self.owner_code_as(el, task, intent_id, None).await
    }

    /// [`Driver::owner_code`] for one tool call: the owner's hidden input is asked with the call's whole context
    /// ([`OwnerInput::ask_code_for`]).
    pub async fn owner_code_for(
        &self,
        el: &ElementRef,
        ctx: &ToolContext,
        intent_id: &str,
    ) -> Result<ActionResult> {
        self.owner_code_as(el, &ctx.task_id, intent_id, Some(ctx))
            .await
    }

    async fn owner_code_as(
        &self,
        el: &ElementRef,
        task: &str,
        intent_id: &str,
        ctx: Option<&ToolContext>,
    ) -> Result<ActionResult> {
        let (t, frame, snap, local, info) = self.resolve(el)?;
        self.owned_by(&el.tab.tab_id, task, true)?;
        no_secret_into_terminal(&info)?;
        if !info.code_fill {
            return Err(Error::refused(
                "that field takes no login code: select a visible one-time-code field, not a password or an ordinary text field",
            ));
        }
        let Kind::Owner {
            input: Some(input), ..
        } = &self.kind
        else {
            return Err(Error::refused("hidden login-code input is unavailable"));
        };
        let origin = normalize(&self.frame_origin(&t, frame).await?);
        if !origin.starts_with("https://") {
            return Err(Error::refused("login codes require an HTTPS destination"));
        }
        let asked = match ctx {
            Some(ctx) => input.ask_code_for(ctx, intent_id, &origin),
            None => input.ask_code(task, intent_id, &origin),
        };
        let Some(value) = asked.await.filter(|v| !v.expose().is_empty()) else {
            return Ok(ActionResult {
                ok: false,
                detail: "login code declined".into(),
                facts: Dict::new(),
                observed: None,
            });
        };
        self.resolve(el)?; // same generation, element and tab after waiting for the owner
        let mut r = self
            .sealed_fill_owner(
                &t,
                frame,
                snap,
                local,
                &origin,
                value,
                intent_id,
                json!({"code_only": true}),
            )
            .await?;
        r.facts = Dict::new();
        Ok(r)
    }

    /// `Router.owner_secret`: the owner's secret (or a split code across several boxes) into a text
    /// field, asked through hidden input, sealed to an HTTPS origin.
    pub async fn owner_secret(
        &self,
        el: &ElementRef,
        task: &str,
        intent_id: &str,
        ask: SecretAsk,
        split_elements: Option<&[String]>,
    ) -> Result<ActionResult> {
        self.owner_secret_as(el, task, intent_id, ask, split_elements, None)
            .await
    }

    /// [`Driver::owner_secret`] for one tool call: the owner's hidden input is asked with the call's whole context
    /// ([`OwnerInput::ask_secret_for`]).
    pub async fn owner_secret_for(
        &self,
        el: &ElementRef,
        ctx: &ToolContext,
        intent_id: &str,
        ask: SecretAsk,
        split_elements: Option<&[String]>,
    ) -> Result<ActionResult> {
        self.owner_secret_as(el, &ctx.task_id, intent_id, ask, split_elements, Some(ctx))
            .await
    }

    async fn owner_secret_as(
        &self,
        el: &ElementRef,
        task: &str,
        intent_id: &str,
        ask: SecretAsk,
        split_elements: Option<&[String]>,
        ctx: Option<&ToolContext>,
    ) -> Result<ActionResult> {
        let (t, frame, snap, local, info) = self.resolve(el)?;
        self.owned_by(&el.tab.tab_id, task, true)?;
        no_secret_into_terminal(&info)?;
        if !["textbox", "searchbox", "combobox"].contains(&info.role.as_str())
            && info.input_type.as_deref().is_none_or(str::is_empty)
        {
            return Err(Error::refused(
                "select an exact text field from a fresh snapshot",
            ));
        }
        let mut refs = vec![el.clone()];
        let mut locals: Option<Vec<i64>> = None;
        if let Some(split) = split_elements {
            let distinct: HashSet<&String> = split.iter().collect();
            if split.len() < 2 || distinct.len() != split.len() || split[0] != el.element_id {
                return Err(Error::refused(
                    "select every distinct split-code textbox in order, with element first",
                ));
            }
            refs = split
                .iter()
                .map(|id| self.element_ref(&el.tab, id))
                .collect();
            let mut ls = vec![];
            for r in &refs {
                let (other, of, os, ol, field) = self.resolve(r)?;
                if other.tab.tab_id != t.tab.tab_id || (of, os) != (frame, snap) {
                    return Err(Error::refused(
                        "split-code fields must share one tab, frame and snapshot",
                    ));
                }
                if field.role != "textbox"
                    || !matches!(
                        field.input_type.as_deref(),
                        None | Some("" | "text" | "tel" | "number")
                    )
                {
                    return Err(Error::refused(
                        "split-code fields must all be textboxes, not passwords or other controls",
                    ));
                }
                no_secret_into_terminal(&field)?;
                ls.push(ol);
            }
            locals = Some(ls);
        }
        let Kind::Owner {
            input: Some(input), ..
        } = &self.kind
        else {
            return Err(Error::refused("hidden secret input is unavailable"));
        };
        let origin = normalize(&self.frame_origin(&t, frame).await?);
        if !origin.starts_with("https://") {
            return Err(Error::refused("secret input requires an HTTPS destination"));
        }
        let ask = SecretAsk {
            field: Some(info.label.clone()).filter(|l| !l.is_empty()),
            kind: info.field_kind.clone().filter(|k| !k.is_empty()),
            ..ask
        };
        let asked = match ctx {
            Some(ctx) => input.ask_secret_for(ctx, intent_id, &origin, ask),
            None => input.ask_secret(task, intent_id, &origin, ask),
        };
        let Some(value) = asked.await.filter(|v| !v.expose().is_empty()) else {
            return Ok(ActionResult {
                ok: false,
                detail: "secret input declined".into(),
                facts: Dict::new(),
                observed: None,
            });
        };
        for r in &refs {
            no_secret_into_terminal(&self.resolve(r)?.4)?;
        }
        let extra = match locals {
            Some(ls) => json!({"split_els": ls}),
            None => json!({}),
        };
        self.sealed_fill_owner(&t, frame, snap, local, &origin, value, intent_id, extra)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn sealed_fill_owner(
        &self,
        t: &Tab,
        frame: i64,
        snap: i64,
        local: i64,
        origin: &str,
        value: Secret,
        intent_id: &str,
        extra: Value,
    ) -> Result<ActionResult> {
        let key = self
            .hub
            .pairings
            .key(&t.bridge.binding.install_id)
            .map_err(|e| Error::driver(e.to_string()))?
            .ok_or_else(|| Error::refused("the browser is no longer paired"))?;
        let cid = if intent_id.is_empty() {
            format!("f-{}", crate::pairing::token_hex(self.random.as_ref(), 8))
        } else {
            intent_id.to_string()
        };
        let sealed = self.seal(key.expose(), &value, &cid, origin)?;
        drop(value);
        self.note_input(t);
        let mut args = json!({"tab_id": t.chrome_id, "frame_id": frame, "snap": snap, "el": local, "sealed": seal::wire(&sealed), "origin": origin});
        for (k, v) in extra.as_object().into_iter().flatten() {
            args[k] = v.clone();
        }
        let v = t.bridge.call("fill", args, 15.0, Some(cid)).await?;
        Ok(ActionResult {
            ok: v.get("ok").is_some_and(ferb_py::value::truthy),
            detail: text(&v, "detail"),
            facts: Dict::new(),
            observed: None,
        })
    }
}

impl Driver {
    /// `open_from`: a task tab's link to a new tab, opened by FERB in the opener's window as that
    /// task's tab; the opener's next result names it (`opened_for`). Past the tab cap it isn't opened.
    /// Difference: when the open fails after the tab was registered, the entry names the error only
    /// (Python's also names the tab, then closed).
    pub async fn open_link(
        &self,
        br: Arc<Bridge>,
        opener_id: i64,
        url: String,
        window_id: Option<i64>,
    ) {
        let Some(opener) = self.live(&br, opener_id) else {
            return;
        };
        let Some(owner) = opener.owner.clone() else {
            return;
        };
        let mut entry = Map::new();
        entry.insert("url".into(), url.clone().into());
        entry.insert("via".into(), "link".into());
        let alias = opener.tab.tab_id.clone();
        self.update(&alias, |x| x.opened.push(entry)); // named at once: its load may become a long download
        let result = match (self.tasks.check_url)(&url) {
            Err(why) => Err(Error::refused(why)),
            Ok(()) => {
                let (running, released) = (Arc::clone(&self.tasks.running), self.me.clone());
                let o = owner.clone();
                let abandon = move || {
                    released
                        .upgrade()
                        .is_some_and(|d| d.st().released.contains(&o))
                        || !running(&o)
                };
                self.open_in(
                    br,
                    &url,
                    Some(&owner),
                    window_id,
                    false,
                    opener.attached,
                    Some(&alias),
                    Some(&abandon),
                )
                .await
            }
        };
        self.update(&alias, |x| {
            let pending = x.opened.iter_mut().find(|e| {
                e.get("via").and_then(Value::as_str) == Some("link")
                    && e.get("url").and_then(Value::as_str) == Some(url.as_str())
                    && !e.contains_key("tab")
                    && !e.contains_key("error")
            });
            if let Some(e) = pending {
                match &result {
                    Ok(tab) => {
                        e.insert("tab".into(), tab.tab_id.clone().into());
                    }
                    Err(err) if err.is_driver_error() || err.error.is_refused() => {
                        e.insert(
                            "error".into(),
                            format!("FERB didn't open the link's new tab: {err}").into(),
                        );
                    }
                    Err(_) => {}
                }
            }
        });
    }

    /// `download_tab_done` (HAND-511): a task tab whose own page became a download is closed once the
    /// file is taken (the transfer ends: closing it would fail it; a stalled one ends, HAND-233) and
    /// FERB's open of it has answered.
    pub async fn download_tab_done_inner(&self, br: Arc<Bridge>, chrome_id: i64) {
        let Some(t) = self.live(&br, chrome_id).filter(|t| !t.attached) else {
            return;
        };
        self.update(&t.tab.tab_id, |x| x.download_only = true);
        if let Some(d) = self.seams().downloads {
            while d.busy(&br, chrome_id) {
                d.settle(&br, chrome_id, 60.0).await;
            }
        }
        for _ in 0..200 {
            match self.live(&br, chrome_id) {
                Some(live) if live.loading => tokio::time::sleep(Duration::from_millis(200)).await,
                _ => break,
            }
        }
        let Some(t) = self.live(&br, chrome_id) else {
            return;
        };
        let _ = self.close(&t.tab, None).await;
        if t.opener.is_some() {
            self.closed_news(
                &t,
                "it only downloaded a file (see downloads); FERB closed it",
            );
        }
    }
}

/// `normalize_origin(str(v.get("origin") or ""))` where an origin no URL can be counts as none.
fn normalize(origin: &str) -> String {
    ferb_identity::origin::normalize_origin(origin).unwrap_or_default()
}

/// `_no_secret_into_terminal` (D31 round 2, O3).
fn no_secret_into_terminal(info: &PageElement) -> Result<()> {
    if info.terminal {
        return Err(Error::refused(SECRET_TERMINAL));
    }
    Ok(())
}

/// `MEDIA_ID`: `m<1-9 digits>-<6 hex>`.
fn media_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix('m') else {
        return false;
    };
    let Some((digits, hex)) = rest.split_once('-') else {
        return false;
    };
    (1..=9).contains(&digits.len())
        && digits.bytes().all(|b| b.is_ascii_digit())
        && hex.len() == 6
        && hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn lax_digit(c: char) -> bool {
    crate::lax::is_digit(c)
}

/// `int(head)` after `head.isdigit()`: decimal digits only; another digit raises in Python (here -1).
fn py_int(head: &str) -> i64 {
    let mut n: i64 = 0;
    for c in head.chars() {
        match crate::lax::decimal_value(c) {
            Some(d) => n = n.saturating_mul(10).saturating_add(i64::from(d)),
            None => return -1,
        }
    }
    n
}

/// Python's `int(text)` for a tab reference's id.
fn py_int_strict(text: &str) -> Option<i64> {
    crate::lax::int(&Value::String(text.to_string()))
        .ok()
        .filter(|_| !text.contains('.'))
}

impl TabDriver for Driver {
    fn identity(&self) -> Identity {
        self.identity
    }

    fn task_tab(&self, br: &Arc<Bridge>, chrome_id: i64) -> Option<TaskTab> {
        self.live(br, chrome_id).map(|t| TaskTab {
            task: t.owner.unwrap_or_default(),
            tab: t.tab.tab_id,
            identity: br.binding.identity.as_str().into(),
        })
    }

    fn holds(&self, br: &Arc<Bridge>, chrome_id: i64) -> bool {
        self.st().tabs.values().any(|t| {
            Arc::ptr_eq(&t.bridge, br)
                && t.chrome_id == chrome_id
                && !t.closed
                && t.owner
                    .as_deref()
                    .is_some_and(|o| !o.is_empty() && (self.tasks.running)(o))
        })
    }

    fn tab_closed(&self, br: &Arc<Bridge>, chrome_id: i64, by_page: bool) {
        let matching: Vec<String> = self
            .st()
            .tabs
            .values()
            .filter(|t| Arc::ptr_eq(&t.bridge, br) && t.chrome_id == chrome_id && !t.closed)
            .map(|t| t.tab.tab_id.clone())
            .collect();
        for alias in matching {
            let Some(mut t) = self.tab(&alias) else {
                continue;
            };
            let itself = by_page
                || t.download_only
                || (t.opener.is_some() && (self.tasks.owner_input_near)() != Some(true));
            let mut dropped = t.clone();
            self.drop_tab(
                &mut dropped,
                if itself {
                    "its page closed it"
                } else {
                    "the owner closed this tab"
                },
            );
            self.update(&alias, |x| {
                x.closed = dropped.closed;
                x.left = dropped.left.clone();
                x.held = dropped.held;
            });
            t.closed = true;
            let Some(owner) = t
                .owner
                .clone()
                .filter(|o| !o.is_empty() && !t.closing && (self.tasks.running)(o))
            else {
                continue;
            };
            if itself {
                let why = if by_page {
                    "its page closed it (a sign-in window does once it is done)"
                } else if t.download_only {
                    "it only downloaded a file (see downloads)"
                } else {
                    "a page opened it, and it closed while the owner wasn't using the keyboard or mouse: its page or the browser closed it"
                };
                self.closed_news(&t, why);
                continue;
            }
            let root = (self.tasks.root)(&owner);
            let left = self.st().tabs.values().any(|o| {
                o.tab.tab_id != alias
                    && Arc::ptr_eq(&o.bridge, br)
                    && !o.closed
                    && !o.attached
                    && o.owner
                        .as_deref()
                        .is_some_and(|x| !x.is_empty() && (self.tasks.root)(x) == root)
            });
            let mut data = Map::new();
            data.insert("task_id".into(), owner.into());
            data.insert("tab".into(), alias.into());
            data.insert("profile".into(), br.name().into());
            data.insert("borrowed".into(), t.attached.into());
            data.insert(
                "part".into(),
                (if left || t.attached { "tab" } else { "group" }).into(),
            );
            self.hub.publish("browser.owner_stopped", data);
        }
    }

    fn tab_taken(&self, br: &Arc<Bridge>, chrome_id: i64, how: &str) {
        let matching: Vec<String> = self
            .st()
            .tabs
            .values()
            .filter(|t| Arc::ptr_eq(&t.bridge, br) && t.chrome_id == chrome_id && !t.closed)
            .map(|t| t.tab.tab_id.clone())
            .collect();
        for alias in matching {
            let Some(mut t) = self.tab(&alias) else {
                continue;
            };
            self.drop_tab(
                &mut t,
                "the owner moved it out of FERB's tab group, so it is theirs now",
            );
            let (closed, left, held) = (t.closed, t.left.clone(), t.held);
            self.update(&alias, |x| {
                x.closed = closed;
                x.left = left;
                x.held = held;
            });
            if let Some(owner) = t.owner.filter(|o| !o.is_empty() && (self.tasks.running)(o)) {
                let mut data = Map::new();
                data.insert("task_id".into(), owner.into());
                data.insert("tab".into(), alias.into());
                data.insert("profile".into(), br.name().into());
                data.insert("how".into(), how.into());
                self.hub.publish("browser.owner_took", data);
            }
        }
    }

    fn readopt(
        &self,
        br: &Arc<Bridge>,
        listed: &[Map<String, Value>],
    ) -> Vec<BoxFuture<'static, ()>> {
        let mut seen: Vec<(i64, Map<String, Value>)> = vec![];
        for r in listed {
            if let Some(id) = r
                .get("tab_id")
                .filter(|v| !v.is_boolean())
                .and_then(Value::as_i64)
            {
                match seen.iter_mut().find(|(k, _)| *k == id) {
                    Some(e) => e.1 = r.clone(),
                    None => seen.push((id, r.clone())),
                }
            }
        }
        let mut jobs: Vec<BoxFuture<'static, ()>> = vec![];
        let mine: Vec<Tab> = self
            .st()
            .tabs
            .values()
            .filter(|t| Arc::ptr_eq(&t.bridge, br) && !t.closed)
            .cloned()
            .collect();
        let mut settle = vec![];
        for t in mine {
            let r = seen
                .iter()
                .position(|(k, _)| *k == t.chrome_id)
                .map(|at| seen.remove(at).1);
            let borrowed = r
                .as_ref()
                .is_some_and(|r| r.get("borrowed").is_some_and(ferb_py::value::truthy));
            if r.is_none() || borrowed != t.attached {
                let mut x = t.clone();
                self.drop_tab(
                    &mut x,
                    "it was closed or taken while FERB's connection to the browser was down",
                );
                let (closed, left, held) = (x.closed, x.left.clone(), x.held);
                self.update(&t.tab.tab_id, |y| {
                    y.closed = closed;
                    y.left = left;
                    y.held = held;
                });
                continue;
            }
            let gen = br.gen();
            self.update(&t.tab.tab_id, |y| {
                y.tab.generation = gen;
                y.elements.clear();
                y.snapshot_json.clear();
            });
            let owner = t.owner.clone().unwrap_or_default();
            if !(!owner.is_empty() && (self.tasks.running)(&owner))
                && !(!owner.is_empty() && (self.tasks.paused)(&owner))
            {
                let mut tab = t.tab.clone();
                tab.generation = gen;
                settle.push(tab);
            }
        }
        for (chrome_id, r) in seen {
            let task = r
                .get("task")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let borrowed = r.get("borrowed") == Some(&Value::Bool(true));
            if !task.is_empty() && ((self.tasks.running)(&task) || (self.tasks.paused)(&task)) {
                let tab = self.new_ref(br);
                let mut t = Tab::new(
                    tab.clone(),
                    Arc::clone(br),
                    chrome_id,
                    Some(task),
                    text(&Value::Object(r.clone()), "url"),
                );
                t.attached = borrowed;
                self.insert(t);
                self.hold(&tab.tab_id);
            } else if borrowed {
                let call = br.call("release_tab", json!({"tab_id": chrome_id}), 10.0, None);
                jobs.push(Box::pin(async move {
                    let _ = call.await;
                }));
            } else {
                let call = br.call("close", json!({"tab_id": chrome_id}), 10.0, None);
                jobs.push(Box::pin(async move {
                    let _ = call.await;
                }));
            }
        }
        // Python's order: the settling of re-adopted tabs (in tab order), then the closes and releases
        let mut ordered: Vec<BoxFuture<'static, ()>> = vec![];
        for tab in settle {
            // `_settle`: closed or let go of, as at the task end; the next hello lists it again on failure
            let me = self.me.clone();
            ordered.push(Box::pin(async move {
                if let Some(d) = me.upgrade() {
                    let _ = d.close(&tab, None).await;
                }
            }));
        }
        ordered.extend(jobs);
        ordered
    }

    fn open_from(
        &self,
        br: Arc<Bridge>,
        opener: i64,
        url: String,
        window_id: Option<i64>,
    ) -> BoxFuture<'static, ()> {
        let me = self.me.clone();
        Box::pin(async move {
            if let Some(d) = me.upgrade() {
                d.open_link(br, opener, url, window_id).await;
            }
        })
    }

    fn adopt(&self, br: &Arc<Bridge>, opener: i64, chrome_id: i64, url: &str) -> Option<Adopted> {
        let o = self.live(br, opener)?;
        let owner = o.owner.clone()?;
        if self.st().released.contains(&owner) {
            return None;
        }
        let mut record = Map::new();
        record.insert("tab_id".into(), chrome_id.into());
        record.insert("task".into(), owner.clone().into());
        record.insert("root".into(), (self.tasks.root)(&owner).into());
        record.insert("borrowed".into(), o.attached.into());
        record.insert("title".into(), self.group(&owner)["title"].clone());
        record.insert("muted".into(), (!o.attached).into());
        if self
            .st()
            .tabs
            .values()
            .any(|t| Arc::ptr_eq(&t.bridge, br) && t.chrome_id == chrome_id)
        {
            return Some(Adopted::Record(record));
        }
        if let Some(why) = self.tab_cap(&owner) {
            let entry: Map<String, Value> = [
                ("url".to_string(), Value::from(url)),
                (
                    "error".into(),
                    format!("the page's new tab was closed before it loaded: {why}").into(),
                ),
            ]
            .into_iter()
            .collect();
            self.update(&o.tab.tab_id, |x| x.opened.push(entry));
            return Some(Adopted::Refused(why));
        }
        let tab = self.new_ref(br);
        let mut t = Tab::new(
            tab.clone(),
            Arc::clone(br),
            chrome_id,
            Some(owner),
            url.into(),
        );
        t.attached = o.attached;
        t.opener = Some(o.tab.tab_id.clone());
        t.muted = !o.attached;
        self.insert(t);
        self.hold(&tab.tab_id);
        let entry: Map<String, Value> = [
            ("tab".to_string(), Value::from(tab.tab_id.clone())),
            ("url".into(), url.into()),
            ("via".into(), "the page opened it".into()),
        ]
        .into_iter()
        .collect();
        self.update(&o.tab.tab_id, |x| x.opened.push(entry));
        Some(Adopted::Record(record))
    }

    fn open_count(&self, root: &str) -> i64 {
        Driver::open_count(self, root)
    }

    fn download_tab_done(&self, br: Arc<Bridge>, chrome_id: i64) -> BoxFuture<'static, ()> {
        let me = self.me.clone();
        Box::pin(async move {
            if let Some(d) = me.upgrade() {
                d.download_tab_done_inner(br, chrome_id).await;
            }
        })
    }
}
