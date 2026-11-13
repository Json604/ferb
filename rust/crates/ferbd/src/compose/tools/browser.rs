//! The browsers (`_build_browsers`): pairings on the store, the extension hub, the owner's router and FERB's own
//! profile's driver, the native-host channel on the RPC server (`ferb-rpc` `NativeChannel` → `ferb-browser`), the
//! two tool families, and the browser area's seams.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use ferb_browser::bridge::Hub;
use ferb_browser::driver::{Driver, Kind, Tasks};
use ferb_browser::mask::Rect;
use ferb_browser::pairing::Pairings;
use ferb_browser::tools::{Family, Tools, Verb, FERB_BROWSER, OWNER_BROWSER};
use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{ActionSpec, TabRef, ToolContext};
use ferb_gateway::tool::{Args, RealPath, Tool, ToolError, ToolOutput};
use serde_json::{Map, Value};

use super::Parts;
use crate::compose::areas::Shared;
use crate::compose::Options;

/// ferb-policy's URL reading and typed destinations, as the browser tools ask.
pub struct BrowserPolicy;

impl ferb_browser::tools::Policy for BrowserPolicy {
    fn parse(&self, url: &str) -> Option<(String, String)> {
        ferb_policy::urls::parse(url).map(|u| (u.scheme, u.host))
    }
    fn typed_destination(&self, text: &str) -> Option<String> {
        ferb_policy::provenance::typed_destination(text)
    }
}

/// The daemon's part of some browser tools: files through the fs tools' rules; `browser.pair` and `browser.handoff`
/// through the browser area (`_pair_for_task`, `_handoff`) once it is built; recordings through the Mac's recordings
/// manager (browser.record).
pub struct BrowserHost {
    home: String,
    area: Arc<OnceLock<ferb_daemon::areas_b::browsers::Cx>>,
    recordings: Option<Arc<dyn ferb_browser::tools::Recordings>>,
}

fn area_err(e: ferb_rpc::HandlerError) -> ferb_browser::error::Error {
    ferb_browser::error::Error::refused(e.message)
}

impl ferb_browser::tools::Host for BrowserHost {
    fn pair(
        &self,
        browser: &str,
        folder: &str,
    ) -> Option<BoxFuture<'_, ferb_browser::error::Result<Map<String, Value>>>> {
        let cx = self.area.get()?.clone();
        let (browser, folder) = (browser.to_string(), folder.to_string());
        Some(Box::pin(async move {
            super::mac::off_thread(move || {
                Box::pin(async move { cx.pair_for_task(&browser, &folder).await.map_err(area_err) })
            })
            .await
            .unwrap_or_else(|| {
                Err(ferb_browser::error::Error::driver(
                    "the pairing's thread ended",
                ))
            })
        }))
    }
    fn handoff(
        &self,
        tab: TabRef,
        message: String,
        ctx: ToolContext,
    ) -> Option<BoxFuture<'_, ferb_browser::error::Result<Map<String, Value>>>> {
        let cx = self.area.get()?.clone();
        Some(Box::pin(async move {
            let tab = serde_json::to_value(&tab).unwrap_or(Value::Null);
            let out = super::mac::off_thread(move || {
                Box::pin(async move {
                    ferb_daemon::areas_b::browsers::handoff(
                        &cx,
                        tab,
                        &message,
                        &ctx.task_id,
                        &ctx.worker_id,
                    )
                    .await
                })
            })
            .await
            .ok_or_else(|| ferb_browser::error::Error::driver("the handoff's thread ended"))?;
            match out {
                Ok(Ok(v)) => Ok(crate::basics::object(v)),
                Ok(Err(e)) => Err(ferb_browser::error::Error::driver(e)),
                Err(e) => Err(area_err(e)),
            }
        }))
    }
    fn resolve(
        &self,
        path: &str,
        ctx: &ToolContext,
        must_exist: bool,
    ) -> ferb_browser::error::Result<String> {
        ferb_fs::files::resolve(path, &ctx.workspace, &self.home, must_exist).map_err(|e| {
            ferb_browser::error::Error::driver(match e {
                ferb_fs::files::Fail::Refused(m) => m,
                other => format!("{other:?}"),
            })
        })
    }
    fn owner_path(&self, path: &str, workspace: &str) -> String {
        ferb_fs::files::owner_path(path, workspace, &self.home).unwrap_or_else(|_| path.to_string())
    }
    fn read(&self, path: &str) -> ferb_browser::error::Result<Vec<u8>> {
        std::fs::read(path).map_err(|e| ferb_browser::error::Error::driver(e.to_string()))
    }
    /// browser/tools.py:723 `new_file(out, pdf, write_check(ctx))` (fs/files.py:295): a new file inside one opened
    /// folder the call's protection check passes, never over anything; `PathRefused` is the call's refusal.
    fn new_file(
        &self,
        path: &str,
        data: &[u8],
        ctx: &ToolContext,
    ) -> ferb_browser::error::Result<()> {
        use ferb_browser::error::Error;
        use ferb_fs::files::Fail;
        let check = ferb_fs::files::write_check(ctx);
        let closed: ferb_fs::files::Check =
            check.as_ref().map(|f| f as &dyn Fn(&str) -> Option<String>);
        ferb_fs::files::new_file(path, data, closed).map_err(|e| match e {
            Fail::Refused(m) => Error::refused(m),
            Fail::Os(e) => Error::driver(e.to_string()),
            Fail::Raised(m) => Error::driver(m),
            other => Error::driver(format!("{other:?}")),
        })
    }
    /// point.py:49: the display holding a rect's centre (the window server's).
    fn display_of(&self, rect: Rect) -> Option<i64> {
        ferb_mac_hands::native::windows::display_of(rect).map(i64::from)
    }
    fn recordings(&self) -> Option<Arc<dyn ferb_browser::tools::Recordings>> {
        self.recordings.clone()
    }
}

/// The Mac's recordings manager as browser.record starts a tab's recording.
pub struct TabRecordings {
    pub manager: Arc<ferb_mac_hands::recording::RecordingManager>,
    pub hearing: Arc<ferb_mac_hands::listen::Hearing>,
    pub redactor: Arc<ferb_sanitize::redact::Redactor>,
}

impl ferb_browser::tools::Recordings for TabRecordings {
    fn hearing(&self) -> bool {
        self.manager.hearing.is_some()
    }
    fn excluded_host(&self, host: &str) -> bool {
        self.redactor.excluded_host(host)
    }
    fn resolve_audio(&self, target: &str) -> (String, Value) {
        match self.hearing.resolve(target) {
            Ok((bundle, pid)) => (bundle, Value::from(pid)),
            Err(_) => (String::new(), Value::Null),
        }
    }
    fn start(
        &self,
        task: &str,
        target: Map<String, Value>,
        path: String,
        capture: ferb_browser::capture::Recording,
        fps: f64,
        audio: Option<(String, Value)>,
        language: Option<String>,
        sources: Vec<(String, String)>,
        _ctx: &ToolContext,
    ) -> BoxFuture<'_, ferb_browser::error::Result<Map<String, Value>>> {
        let capture = Arc::new(capture);
        let frames: ferb_mac_hands::point::FrameFn = Arc::new(move || {
            let c = capture.clone();
            Box::pin(async move {
                c.capture(None)
                    .await
                    .map(|(jpeg, note)| (jpeg, note.unwrap_or_default()))
                    .map_err(|e| ferb_mac_hands::error::HandError::Other(e.message()))
            })
        });
        let audio = audio.map(|(bundle, pid)| (bundle, pid.as_i64().unwrap_or(0) as i32));
        let (task, manager) = (task.to_string(), self.manager.clone());
        Box::pin(async move {
            manager
                .start(
                    &task,
                    target,
                    std::path::PathBuf::from(path),
                    frames,
                    fps,
                    audio,
                    language,
                    sources,
                )
                .await
                .map_err(|e| match e {
                    ferb_mac_hands::error::HandError::Refused(m) => {
                        ferb_browser::error::Error::refused(m)
                    }
                    other => ferb_browser::error::Error::driver(other.to_string()),
                })
        })
    }
}

/// FERB's own secrets (`KeychainSecretStore` over the signed helper, `[secrets]` read at call time) as FERB's own
/// profile's driver reads them: names and specs for descriptions, the value only inside a sealed fill.
pub struct OwnSecrets(pub Arc<ferb_identity::secrets::KeychainSecretStore>);

impl ferb_browser::driver::Secrets for OwnSecrets {
    fn specs(&self) -> Vec<ferb_identity::secrets::SecretSpec> {
        self.0.specs()
    }
    fn has(&self, secret: &ferb_identity::secrets::SecretRef) -> bool {
        self.0.has(secret)
    }
    fn spec(
        &self,
        secret: &ferb_identity::secrets::SecretRef,
    ) -> Option<ferb_identity::secrets::SecretSpec> {
        self.0.spec(secret)
    }
    fn reveal(
        &self,
        secret: &ferb_identity::secrets::SecretRef,
    ) -> BoxFuture<'_, std::result::Result<ferb_identity::Secret, String>> {
        let (store, secret) = (self.0.clone(), secret.clone());
        Box::pin(async move {
            tokio::task::spawn_blocking(move || store.reveal(&secret).map_err(|e| e.reason))
                .await
                .unwrap_or_else(|e| Err(e.to_string()))
        })
    }
}

/// `_identity_profile`: FERB's own (browser, folder), from `[identity]`.
pub fn identity_profile(config: &ferb_core::config::Config) -> Option<(String, String)> {
    let folder = match config.get("identity", "profile") {
        Ok(Some(Value::String(s))) if !s.is_empty() => s,
        _ => return None,
    };
    let browser = match config.get("identity", "browser") {
        Ok(Some(Value::String(s))) if !s.is_empty() => s,
        _ => "chrome".into(),
    };
    Some((browser, folder))
}

/// Which apps are browsers, and the browsers FERB pairs with (the Mac-hands part's tables).
pub struct Apps;

impl ferb_browser::rules::BrowserApps for Apps {
    fn known(&self, bundle_id: &str) -> bool {
        ferb_mac_hands::apps::known(bundle_id)
    }
    fn browser(&self, key: &str) -> Option<ferb_browser::rules::BrowserInfo> {
        ferb_browser::profiles::browser(key).map(|b| ferb_browser::rules::BrowserInfo {
            bundle_id: b.bundle_id.to_string(),
            label: b.label.to_string(),
        })
    }
}

/// What both drivers read about FERB's tasks (browsers.py:308-326): a live task (removed when its tree ends), a paused
/// one (it keeps its group, ORCH-308), a loop between its rounds (L31), the task tree (tab cap, group), the tree's goal
/// for its group's label, the tab cap, a registered checker's worker (its tabs), the owner's own input near now
/// (HAND-511), the redactor and a link's address parsed as the browser will (S4). The parts are held weakly: the
/// drivers live inside them.
fn driver_tasks(p: &Parts) -> Tasks {
    let weak = Arc::downgrade(&p.0);
    let parts = move || weak.upgrade().map(Parts);
    let r = p.redactor();
    let (p1, p2, p3, p4, p5, p6, p7, p8) = (
        parts.clone(),
        parts.clone(),
        parts.clone(),
        parts.clone(),
        parts.clone(),
        parts.clone(),
        parts.clone(),
        parts,
    );
    Tasks {
        running: Arc::new(move |task| p1().is_some_and(|p| p.task(task).is_some())),
        paused: Arc::new(move |task| {
            p2().and_then(|p| p.supervisor().map(|s| s.inner().task_paused(task)))
                .unwrap_or(false)
        }),
        resting: Arc::new(move |task| {
            p3().and_then(|p| p.supervisor().map(|s| s.loop_resting(task)))
                .unwrap_or(false)
        }),
        root: Arc::new(move |task| {
            p4().and_then(|p| p.task(task).and_then(|t| t.tree_id))
                .unwrap_or_else(|| task.split('.').next().unwrap_or(task).to_string())
        }),
        title: Arc::new(move |root| {
            // `_task_title`: the live tree's goal
            let Some(p) = p5() else {
                return String::new();
            };
            if p.task(root).is_none() {
                return String::new();
            }
            let shared = p.db();
            let db = crate::basics::lock(&shared);
            db.conn()
                .query_row("SELECT goal FROM tasks WHERE id = ?", [root], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap_or_default()
        }),
        max_tabs: Arc::new(move || {
            p6().map_or(50, |p| match p.config().get("browser", "task_max_tabs") {
                Ok(Some(v)) => v
                    .as_i64()
                    .or_else(|| v.as_f64().map(|f| f as i64))
                    .unwrap_or(50),
                _ => 50,
            })
        }),
        inspection_parent: Arc::new(move |task| {
            p7().and_then(|p| crate::compose::env_seams::inspection_parent(&p, task))
        }),
        owner_input_near: Arc::new(move || {
            let p = p8()?;
            let o = p.owner()?;
            let now = p.clock().now().timestamp_micros() as f64 / 1e6;
            o.input_near(now)
        }),
        redact: Arc::new(move |t: &str| r.text(t)),
        check_url: Arc::new(|url: &str| match ferb_policy::urls::parse(url) {
            Some(u) if u.scheme == "http" || u.scheme == "https" => {
                if u.host.is_empty() {
                    Err("this address names no site, so FERB didn't open it".into())
                } else {
                    Ok(())
                }
            }
            Some(u) => Err(format!("only http(s) URLs can be opened, not {}", u.scheme)),
            None => Err("only http(s) URLs can be opened, not this".into()),
        }),
    }
}

/// `Resolver(self._profiles, self.pairings.all, self._identity_profile, owner_default=self._owner_browsing_profile,
/// unseen=...)` (areas/browsers.py:260, HAND-2, HAND-407): an owner task's profile, the one it names or the owner's
/// pick, only among paired ones; anything else is a refusal for the agent (`NeedsOwner`). Read fresh for each choice.
fn resolver(
    area: Arc<OnceLock<ferb_daemon::areas_b::browsers::Cx>>,
    pairings: Arc<Pairings>,
) -> ferb_browser::driver::Choose {
    Arc::new(move |named: Option<&str>| {
        let refused = |m: String| ferb_browser::error::Error::refused(m);
        let Some(cx) = area.get() else {
            return Err(refused(
                "no browser profile is paired with FERB yet: pick the owner's profile from browser_profiles and pair \
                 it with browser_pair (FERB asks the owner once)"
                    .into(),
            ));
        };
        let bindings = pairings
            .all()
            .map_err(|e| ferb_browser::error::Error::driver(e.0))?
            .into_iter()
            .map(|b| ferb_browser::profiles::Paired {
                owner: b.identity == ferb_core::types::Identity::Owner,
                browser: b.browser,
                folder: b.folder,
            })
            .collect();
        let r = ferb_browser::profiles::Resolver {
            platform: cx.deps.platform.as_ref(),
            profiles: cx.profiles(),
            bindings,
            identity_profile: cx.identity_profile().ok().flatten(),
            aliases: Vec::new(),
            owner_default: cx.owner_browsing_profile().ok().flatten(),
            unseen: (!cx.other_app_data())
                .then(|| ferb_daemon::areas_b::browsers::PROFILES_UNSEEN.to_string()),
        };
        r.choose(named)
            .map(|c| (c.browser, c.folder))
            .map_err(refused)
    })
}

/// The owner's hidden input for a code or a secret in the owner's browser (`router.ask_code` /
/// `router.ask_secret`, areas/browsers.py:298): the gateway's questions (D12), with the calling context. A question
/// the gateway refuses reads as declined here (the driver's seam carries no error).
struct GatewayInput(super::WeakParts);

impl ferb_browser::driver::OwnerInput for GatewayInput {
    fn ask_code(
        &self,
        _task: &str,
        _intent: &str,
        _origin: &str,
    ) -> BoxFuture<'_, Option<ferb_identity::Secret>> {
        // every tool asks with its whole context (`ask_code_for`): the gateway's calls always carry it
        crate::off::off("browser.ask_without_context", Box::pin(async { None }))
    }
    fn ask_secret(
        &self,
        _task: &str,
        _intent: &str,
        _origin: &str,
        _ask: ferb_browser::driver::SecretAsk,
    ) -> BoxFuture<'_, Option<ferb_identity::Secret>> {
        crate::off::off("browser.ask_without_context", Box::pin(async { None }))
    }
    fn ask_code_for<'a>(
        &'a self,
        ctx: &'a ToolContext,
        _intent: &'a str,
        origin: &'a str,
    ) -> BoxFuture<'a, Option<ferb_identity::Secret>> {
        let (p, ctx, origin) = (self.0.upgrade(), ctx.clone(), origin.to_string());
        Box::pin(async move {
            let p = p?;
            let gw = p.gateway()?;
            p.on_calls_thread(move || async move {
                gw.ask_login_code(&ctx, &origin).await.ok().flatten()
            })
            .await
            .flatten()
        })
    }
    fn ask_secret_for<'a>(
        &'a self,
        ctx: &'a ToolContext,
        _intent: &'a str,
        origin: &'a str,
        ask: ferb_browser::driver::SecretAsk,
    ) -> BoxFuture<'a, Option<ferb_identity::Secret>> {
        let (p, ctx, origin) = (self.0.upgrade(), ctx.clone(), origin.to_string());
        let ask = ferb_gateway::gateway::SecretAsk {
            handle: ask.handle,
            remember: ask.remember,
            shown: None,
            field: ask.field,
            kind: ask.kind,
        };
        Box::pin(async move {
            let p = p?;
            let gw = p.gateway()?;
            p.on_calls_thread(move || async move {
                gw.ask_secret(&ctx, &origin, ask).await.ok().flatten()
            })
            .await
            .flatten()
        })
    }
}

/// The browser parts, built once.
pub struct Browsers {
    pub pairings: Arc<Pairings>,
    pub hub: Arc<Hub>,
    pub router: Arc<Driver>,
    /// FERB's own profile's driver (None: no FERB browser profile set up).
    pub identity: Option<Arc<Driver>>,
    owner: Arc<Tools>,
    ferb: Option<Arc<Tools>>,
    /// The browser area, once group B builds it (browser.pair, browser.handoff).
    area: Arc<OnceLock<ferb_daemon::areas_b::browsers::Cx>>,
    /// `WakeServer`: the blank local page a profile with no window is loaded with before its pairing page (None: no
    /// runtime to serve it on, a test's build).
    pub wake: Option<Arc<WakePage>>,
    /// D35: a download in a task tab becomes a file in the workspace plus facts, never a dialog
    /// (`downloads.Downloads`, browsers.py:269).
    pub downloads: Arc<ferb_browser::downloads::Downloads>,
    /// Rule 17: a file given to a browser to upload is FERB's own copy (`uploads.Uploads`, browsers.py:284).
    pub uploads: Arc<ferb_browser::staging::Uploads>,
}

/// `downloads.Downloads(home/workspace/downloads, db, clock.now, config.get, redact, publish)` and what a restart cut
/// off cleared, each step on its own (browsers.py:269-279).
fn build_downloads(
    opts: &Options,
    p: &Parts,
    random: Arc<dyn ferb_browser::pairing::Random>,
) -> Arc<ferb_browser::downloads::Downloads> {
    use ferb_browser::downloads::{sqlite::SqliteRecords, Downloads, Fault};
    let conn = rusqlite::Connection::open(opts.home.join("ferb.db"))
        .expect("the store was opened already");
    let (config, events, redactor, journal) = (p.config(), p.events(), p.redactor(), p.journal());
    let dl = Downloads::new(
        opts.home
            .join("workspace")
            .join(ferb_browser::downloads::FOLDER),
        Arc::new(SqliteRecords::new(conn)),
        p.clock(),
        Arc::new(move |section: &str, key: &str| {
            config
                .get(section, key)
                .map_err(|e| Fault::new("ValueError", &e.to_string()))
        }),
        Arc::new(move |t: &str| redactor.text(t)),
        Arc::new(move |topic: &str, data: Map<String, Value>| events.publish(topic, data)),
        random,
    );
    if let Err(e) = dl.sweep() {
        journal.note(
            "browser.downloads_startup_failed",
            serde_json::json!({"step": "sweep", "error": format!("OSError: {e}")}),
        );
    }
    if let Err(e) = dl.finalize_stale() {
        journal.note(
            "browser.downloads_startup_failed",
            serde_json::json!({"step": "finalize_stale", "error": e.typed()}),
        );
    }
    dl
}

/// `uploads.Uploads(home/workspace/uploads, untrusted roots)` and the copies a restart left behind swept (they belong
/// to no task now; browsers.py:284-290).
fn build_uploads(
    opts: &Options,
    p: &Parts,
    random: Arc<dyn ferb_browser::pairing::Random>,
) -> Arc<ferb_browser::staging::Uploads> {
    let untrusted = crate::compose::gateway::fs_roots(&p.config(), &opts.home).1;
    let up = Arc::new(ferb_browser::staging::Uploads::new(
        &opts
            .home
            .join("workspace")
            .join(ferb_browser::staging::FOLDER)
            .to_string_lossy(),
        &opts.user_home.to_string_lossy(),
        Arc::new(move || untrusted.clone()),
        random,
    ));
    if let Err(e) = up.sweep() {
        p.journal().note(
            "browser.uploads_startup_failed",
            serde_json::json!({"error": format!("{}: {}", e.class, e.message), "left": e.left}),
        );
    }
    up
}

/// The image library a tab's picture is masked with (`mask.size`, `mask.mask_secret_fields`): Quartz.
struct QuartzImages;

impl ferb_browser::mask::Images for QuartzImages {
    fn size(&self, image: &[u8]) -> std::result::Result<(i64, i64), String> {
        ferb_mac_hands::native::paint::size(image).map_err(|e| e.to_string())
    }
    fn mask(
        &self,
        image: &[u8],
        boxes: &[ferb_browser::mask::ImageBox],
    ) -> std::result::Result<Vec<u8>, String> {
        ferb_mac_hands::native::paint::mask_secret_fields(image, boxes).map_err(|e| e.to_string())
    }
}

/// The profiles the drivers know (`self._profiles`, `known`, `unseen`): the browser area's, read fresh.
struct AreaProfiles {
    area: Arc<OnceLock<ferb_daemon::areas_b::browsers::Cx>>,
    pairings: Arc<Pairings>,
}

impl ferb_browser::owner::ProfilesView for AreaProfiles {
    fn discovered(&self) -> Vec<ferb_browser::profiles::BrowserProfile> {
        self.area.get().map(|cx| cx.profiles()).unwrap_or_default()
    }
    fn known(&self) -> Vec<ferb_browser::profiles::BrowserProfile> {
        let Some(cx) = self.area.get() else {
            return Vec::new();
        };
        let paired: Vec<(String, String)> = self
            .pairings
            .all()
            .map(|all| all.into_iter().map(|b| (b.browser, b.folder)).collect())
            .unwrap_or_default();
        ferb_browser::profiles::with_paired(cx.deps.platform.as_ref(), &cx.profiles(), &paired)
    }
    fn unseen(&self) -> Option<String> {
        let cx = self.area.get()?;
        (!cx.other_app_data()).then(|| ferb_daemon::areas_b::browsers::PROFILES_UNSEEN.to_string())
    }
}

/// `_tab_task(br, tab_id)` (sensing.py:97): the task that owns this tab, through either driver.
fn tab_task(p: &Parts, br: &Arc<ferb_browser::bridge::Bridge>, tab: i64) -> Option<String> {
    [p.owner_driver(), p.ferb_driver()]
        .into_iter()
        .flatten()
        .find_map(|d| d.owner_of(br, tab).flatten())
}

/// The rest of `_build_browsers` (browsers.py:269-326) and `_build_sense`'s hub hooks (sensing.py:61-64), once every
/// part exists: the hub's downloads, closed tabs, tab trail, pause, ownership and sound; each driver's downloads,
/// uploads, clipboard, image library, native reads, upload files, profiles and (FERB's own) inbox.
pub fn wire_parts(
    p: &Parts,
    desktop: Arc<crate::compose::desktop_seams::DesktopSeams>,
    helper: Option<Arc<ferb_identity::Helper>>,
) {
    let Some(b) = p.browsers() else { return };
    // browsers.py:297: the owner's browser seals a fill through the signed secrets helper (`helper_seal`); a daemon
    // without the Keychain (a test's) keeps the driver's own seal of the same format
    match helper {
        Some(h) => b.router.set_sealer(Arc::new(
            move |key: &[u8], value: &ferb_identity::Secret, command: &str, origin: &str| {
                let derived = ferb_identity::seal::seal_key(key);
                h.seal(&derived[..], value, command, origin)
                    .map_err(|_| ferb_identity::seal::SealError::Seal)
            },
        )),
        None => crate::off::off("browser.sealer_in_process", ()),
    }
    let weak = p.downgrade();
    // the downloads keeper's hooks: only a live task's tab holds downloads (D35); budgets per task tree; HAND-211
    {
        let mut hooks = b.downloads.hooks();
        let hub = Arc::downgrade(&b.hub);
        hooks.task_tab = Arc::new(move |peer: &dyn ferb_browser::downloads::Peer, tab: i64| {
            hub.upgrade()?.task_tab(&peer.bridge()?, tab)
        });
        let w = weak.clone();
        hooks.task_root = Arc::new(move |task: &str| {
            w.upgrade()
                .and_then(|p| p.task(task).and_then(|t| t.tree_id))
                .unwrap_or_else(|| task.split('.').next().unwrap_or(task).to_string())
        });
        let w = weak.clone();
        hooks.on_over_cap = Arc::new(move || {
            let Some(a) = w.upgrade().and_then(|p| p.group_a()) else {
                return;
            };
            let _ = a.spawn(|st| async move {
                if let Some(purges) = st.purges.clone() {
                    let _ = ferb_daemon::areas_a::retention::downloads_cap(&st, purges.as_ref());
                }
            });
        });
        b.downloads.set_hooks(hooks);
    }
    let mut hooks = b.hub.hooks();
    hooks.on_download = Some(b.downloads.on_download());
    // browsers.py:77: a closed tab's upload copies need not stay; a tree that already ended has them collected
    {
        let (uploads, journal) = (b.uploads.clone(), p.journal());
        hooks.on_tab_closed = Arc::new(move |br: &Arc<ferb_browser::bridge::Bridge>, tab: i64| {
            if !uploads.closed(&ferb_browser::upload::tab_key(&br.name(), tab)) {
                return None;
            }
            let (u, j) = (uploads.clone(), journal.clone());
            Some(Box::pin(async move {
                if let Ok(Err(e)) = tokio::task::spawn_blocking(move || u.collect()).await {
                    j.note(
                        "browser.uploads_cleanup_failed",
                        serde_json::json!({"error": format!("{}: {}", e.class, e.message), "left": e.left}),
                    );
                }
            }) as BoxFuture<'static, ()>)
        });
    }
    // sensing.py:118 `_on_tab`: each tab event into the trail (while paused, only FERB's own tabs reach here)
    {
        let w = weak.clone();
        hooks.on_tab = Arc::new(
            move |br: &Arc<ferb_browser::bridge::Bridge>, e: &ferb_browser::protocol::TabEvent| {
                let Some(p) = w.upgrade() else { return };
                let Some(trail) = p.trail() else { return };
                let task = tab_task(&p, br, e.tab_id);
                if let Some(o) = ferb_sense::watchers::browser_tabs::tab_observation(
                    &br.binding.browser,
                    &br.binding.folder,
                    br.binding.identity == ferb_core::types::Identity::Ferb,
                    &e.event,
                    e.url.as_deref(),
                    e.title.as_deref(),
                    e.incognito,
                    task.as_deref(),
                ) {
                    (trail.emit)(o);
                }
            },
        );
    }
    // sensing.py:62-63: `ferb pause` stops the owner's tabs from reaching the trail; FERB's own tabs still do
    {
        let w = weak.clone();
        hooks.paused = Arc::new(move || w.upgrade().is_some_and(|p| p.sensing_paused()));
        let w = weak.clone();
        hooks.owned = Arc::new(move |br: &Arc<ferb_browser::bridge::Bridge>, tab: i64| {
            w.upgrade().is_some_and(|p| tab_task(&p, br, tab).is_some())
        });
    }
    // sensing.py:104 `_tab_sound` (HAND-207, HAND-213): a tab that isn't FERB's changed sound or mute, a disturbance
    // fact when one of FERB's calls was near; a mute the owner changed themselves is left out
    {
        let w = weak.clone();
        hooks.on_sound = Arc::new(
            move |br: &Arc<ferb_browser::bridge::Bridge>, e: &ferb_browser::protocol::TabEvent| {
                if e.ferb == Some(true) {
                    return;
                }
                let Some(p) = w.upgrade() else { return };
                let Some(sentinel) = p.sentinel() else { return };
                sentinel.sound_changed(&sound_tab(&p, br, e.tab_id), &sound_change(e));
            },
        );
    }
    b.hub.set_hooks(hooks);

    // each driver's seams (browsers.py:300-321)
    let native = p.native();
    let seams = |inbox: Option<ferb_browser::owner::Inbox>| ferb_browser::ops::Seams {
        downloads: Some(b.downloads.clone() as Arc<dyn ferb_browser::ops::DownloadsView>),
        uploads: Some(b.uploads.clone() as Arc<dyn ferb_browser::ops::Staging>),
        clipboard: native.then(|| {
            Arc::new(|| {
                ferb_mac_hands::tools::Pasteboard::read(
                    &ferb_mac_hands::native::pasteboard::NativePasteboard,
                )
            }) as ferb_browser::ops::Clipboard
        }),
        images: native.then(|| Arc::new(QuartzImages) as Arc<dyn ferb_browser::mask::Images>),
        native: Some(Arc::new(crate::compose::desktop_seams::BrowserNative(
            desktop.clone(),
        )) as Arc<dyn ferb_browser::ops::Native>),
        files: Some(Arc::new(ferb_browser::upload::unix::UnixFiles {
            home: p.user_home().to_string_lossy().into_owned(),
        }) as Arc<dyn ferb_browser::upload::Files>),
        profiles: Some(Arc::new(AreaProfiles {
            area: b.area.clone(),
            pairings: b.pairings.clone(),
        }) as Arc<dyn ferb_browser::owner::ProfilesView>),
        inbox,
    };
    b.router.set_seams(seams(None));
    if let Some(identity) = &b.identity {
        // FerbIdentity's mailbox (browsers.py:304): FERB's own address, read when asked
        let config = p.config();
        identity.set_seams(seams(Some(ferb_browser::owner::Inbox {
            address: Arc::new(move || match config.get("mailbox", "address") {
                Ok(Some(Value::String(s))) => s,
                _ => String::new(),
            }),
            feed_url: ferb_identity::mailbox::FEED_URL.to_string(),
        })));
    }
}

/// `{"browser": br.name, "tab_id": id}` plus the task that owns it (sensing.py:114).
fn sound_tab(p: &Parts, br: &Arc<ferb_browser::bridge::Bridge>, tab: i64) -> Value {
    let mut m = Map::new();
    m.insert("browser".into(), Value::from(br.name()));
    m.insert("tab_id".into(), Value::from(tab));
    if let Some(task) = tab_task(p, br, tab) {
        m.insert("task".into(), Value::from(task));
    }
    Value::Object(m)
}

/// sensing.py:110-113: each of audible and muted that changed and is known, as {before, after}; a mute the owner
/// changed themselves (`muted_reason` "user") is not FERB's to record.
pub fn sound_change(e: &ferb_browser::protocol::TabEvent) -> Map<String, Value> {
    let owners_mute = e.muted_reason.as_deref() == Some("user");
    let mut out = Map::new();
    for (k, v) in [("audible", e.audible), ("muted", e.muted)] {
        let Some(v) = v else { continue };
        if !e.changed.iter().any(|c| c == k) || (k == "muted" && owners_mute) {
            continue;
        }
        out.insert(k.into(), serde_json::json!({"before": !v, "after": v}));
    }
    out
}

/// `WakeServer` (ext_bridge.py:809): a unique blank local page per wake (not about:blank: its fragment is dropped, so
/// it couldn't be told apart from the owner's tabs), served on 127.0.0.1 for the daemon's life.
pub struct WakePage {
    port: u16,
}

impl WakePage {
    /// Listens on 127.0.0.1 (any free port) and answers each connection with the page, on this runtime.
    pub fn start() -> std::io::Result<Arc<WakePage>> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let handle = tokio::runtime::Handle::try_current().map_err(std::io::Error::other)?;
        let _entered = handle.enter();
        let listener = tokio::net::TcpListener::from_std(listener)?;
        handle.spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        tokio::spawn(wake_answer(stream));
                    }
                    Err(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
                }
            }
        });
        Ok(Arc::new(WakePage { port }))
    }

    /// `url()`: `http://127.0.0.1:<port>/wake/<token_hex(8)>`.
    pub fn url(&self) -> String {
        format!(
            "http://127.0.0.1:{}/wake/{}",
            self.port,
            ferb_runtimes::codex::token_hex(8)
        )
    }
}

/// One connection: its request line read (a browser may open a connection and send nothing: 5 s), then the page.
async fn wake_answer(stream: tokio::net::TcpStream) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut reader = BufReader::new(stream);
    let mut first = String::new();
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        reader.read_line(&mut first),
    )
    .await;
    if matches!(read, Ok(Ok(_))) {
        let body = b"<!doctype html><title>FERB</title>";
        let mut out = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        let mut stream = reader.into_inner();
        let _ = stream.write_all(&out).await;
        let _ = stream.shutdown().await;
    }
}

/// The drivers' wake (`Router._connected`, HAND-403): a profile that isn't connected is loaded with no window
/// (`open_known_profile(browser, folder, None, known, is_running=…)`), or with its own window the second time, through
/// the browser area's launcher. Nothing before the area is built.
struct ProfileWake {
    area: Arc<OnceLock<ferb_daemon::areas_b::browsers::Cx>>,
    pairings: Arc<Pairings>,
}

impl ferb_browser::driver::Wake for ProfileWake {
    fn start(&self, browser: &str, folder: &str, window: bool) {
        use ferb_browser::profiles;
        let Some(cx) = self.area.get() else { return };
        let platform = cx.deps.platform.as_ref();
        let paired: Vec<(String, String)> = self
            .pairings
            .all()
            .map(|all| all.into_iter().map(|b| (b.browser, b.folder)).collect())
            .unwrap_or_default();
        let known = profiles::with_paired(platform, &cx.profiles(), &paired);
        let binary = profiles::browser(browser)
            .map(|b| cx.deps.binaries.binary(platform, b))
            .unwrap_or_default();
        let running = profiles::running(platform, &cx.deps.binaries, browser);
        match profiles::open_command(
            browser, folder, None, &known, &binary, true, running, window,
        ) {
            Ok(args) => {
                if let Err(e) = (cx.deps.launch)(&args) {
                    crate::log::line(&format!("couldn't start {browser}/{folder}: {e}"));
                }
            }
            Err(why) => crate::log::line(&format!("didn't start {browser}/{folder}: {why}")),
        }
    }
}

/// The native host's two kernel facts on the Mac (`_browser_pid`, `_app_bundle`): the relay's parent (the browser)
/// from its socket, and that process's app from its executable's bundle.
#[cfg(target_os = "macos")]
pub fn peer_seams() -> (
    ferb_browser::bridge::native::BrowserPid,
    ferb_browser::bridge::AppOf,
) {
    (
        Arc::new(ferb_browser::macos::native_host_parent),
        Arc::new(ferb_browser::macos::app_bundle),
    )
}

/// Elsewhere nothing can be told, so every native-host connection is refused (fails closed).
#[cfg(not(target_os = "macos"))]
pub fn peer_seams() -> (
    ferb_browser::bridge::native::BrowserPid,
    ferb_browser::bridge::AppOf,
) {
    crate::off::off(
        "browser.peer_elsewhere",
        (Arc::new(|_fd| None), Arc::new(|_pid| None)),
    )
}

/// HAND-406 (areas/browsers.py:255-259): each refused connection or message journaled
/// `browser.connection_refused`, the first at once and then counted per reason and profile or app (`_Repeats`); and
/// what the socket refused before the hub saw it (`server.on_native_refused`) noted as a "host" refusal.
pub fn wire_refusals(p: &Parts, server: &ferb_rpc::Server, repeats: &ferb_daemon::shared::Repeats) {
    let Some(b) = p.browsers() else { return };
    let hub = b.hub.clone();
    let mut hooks = hub.hooks();
    let r = repeats.clone();
    hooks.record_refusal = Arc::new(move |facts: Map<String, Value>| {
        let text = |k: &str| match facts.get(k) {
            Some(Value::String(s)) => s.clone(),
            Some(v) if !v.is_null() => ferb_py::value::py_str(v),
            _ => String::new(),
        };
        let reason = match facts.get("reason") {
            None | Some(Value::Null) => "None".to_string(),
            _ => text("reason"),
        };
        let who = Some(text("profile"))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| text("app"));
        let kind = ferb_daemon::areas_b::browsers::REFUSED;
        r.record(kind, &[kind, &reason, &who], facts);
    });
    hub.set_hooks(hooks);
    let h = hub.clone();
    server.on_native_refused(move |why, origin| h.note_host_refusal(why, origin));
}

impl Browsers {
    pub fn build(opts: &Options, p: &Parts) -> Browsers {
        let conn = rusqlite::Connection::open(opts.home.join("ferb.db"))
            .expect("the store was opened already");
        let random: Arc<dyn ferb_browser::pairing::Random> =
            Arc::new(ferb_browser::pairing::SystemRandom);
        let pairings = Arc::new(Pairings::new(
            Arc::new(ferb_browser::pairing::sqlite::SqliteInstalls::new(conn)),
            p.clock(),
            random.clone(),
        ));
        let ledger: Arc<dyn ferb_browser::bridge::Ledger> = Arc::new(
            crate::compose::gateway::BridgeLedger(Arc::new(crate::compose::gateway::RealLedger {
                db: p.db(),
                clock: p.clock(),
                ids: p.ids(),
            })),
        );
        let events = p.events();
        let config = p.config();
        let hub = Hub::new(
            pairings.clone(),
            ledger,
            Arc::new(move |topic, data| events.publish(topic, data)),
            Arc::new(move || match config.get("device", "id") {
                Ok(Some(Value::String(s))) => s,
                _ => "unknown-device".into(),
            }),
            random.clone(),
            Arc::new(Apps),
            peer_seams().1,
        );
        let tasks = driver_tasks(p);
        let area: Arc<OnceLock<ferb_daemon::areas_b::browsers::Cx>> = Arc::new(OnceLock::new());
        let wake: Arc<dyn ferb_browser::driver::Wake> = Arc::new(ProfileWake {
            area: area.clone(),
            pairings: pairings.clone(),
        });
        let router = Driver::new(
            hub.clone(),
            Kind::Owner {
                choose: resolver(area.clone(), pairings.clone()),
                input: Some(Arc::new(GatewayInput(p.downgrade()))),
            },
            tasks,
            Some(wake.clone()),
            random.clone(),
        );
        let host: Arc<dyn ferb_browser::tools::Host> = Arc::new(BrowserHost {
            home: opts.user_home.to_string_lossy().into_owned(),
            area: area.clone(),
            recordings: p.mac().map(|m| {
                Arc::new(TabRecordings {
                    manager: m.recordings.clone(),
                    hearing: m.hearing.clone(),
                    redactor: p.redactor(),
                }) as Arc<dyn ferb_browser::tools::Recordings>
            }),
        });
        let owner = Arc::new(Tools {
            family: OWNER_BROWSER,
            driver: router.clone(),
            policy: Arc::new(BrowserPolicy),
            host: host.clone(),
        });
        // `FerbIdentity`: FERB's own profile, its secrets through the signed helper (its tools join the registry
        // once that profile is paired as FERB)
        let cfg = p.config();
        let specs_cfg = p.config();
        let secrets = Arc::new(ferb_identity::secrets::KeychainSecretStore::new(
            move || {
                ferb_identity::secrets::specs_from_config(
                    specs_cfg.section("secrets").ok().as_ref(),
                )
            },
            crate::compose::areas::helper(opts, &opts.home),
        ));
        let identity = Driver::new(
            hub.clone(),
            Kind::Ferb {
                profile: Arc::new(move || identity_profile(&cfg)),
                secrets: Arc::new(OwnSecrets(secrets)),
            },
            driver_tasks(p),
            Some(wake),
            random,
        );
        let ferb = Arc::new(Tools {
            family: FERB_BROWSER,
            driver: identity.clone(),
            policy: Arc::new(BrowserPolicy),
            host,
        });
        let downloads = build_downloads(opts, p, Arc::new(ferb_browser::pairing::SystemRandom));
        let uploads = build_uploads(opts, p, Arc::new(ferb_browser::pairing::SystemRandom));
        Browsers {
            pairings,
            hub,
            router,
            identity: Some(identity),
            owner,
            ferb: Some(ferb),
            area,
            downloads,
            uploads,
            wake: match WakePage::start() {
                Ok(w) => Some(w),
                Err(e) => {
                    crate::log::line(&format!("the browsers' wake page didn't start: {e}"));
                    None
                }
            },
        }
    }

    pub fn owner_tools(&self) -> Vec<Arc<dyn Tool>> {
        family_tools(&self.owner)
    }

    /// FERB's own profile's tools: the catalog's `ferb_browser.*` (21).
    pub fn ferb_tools(&self) -> Vec<Arc<dyn Tool>> {
        self.ferb.as_ref().map(family_tools).unwrap_or_default()
    }

    /// The browser area, for browser.pair and browser.handoff.
    pub fn attach_area(&self, cx: ferb_daemon::areas_b::browsers::Cx) {
        let _ = self.area.set(cx);
    }

    /// The browser area, once group B has built it.
    pub fn area(&self) -> Option<ferb_daemon::areas_b::browsers::Cx> {
        self.area.get().cloned()
    }

    /// `_attach_identity_if_paired` at start: FERB's tools exist only once its own profile is paired as FERB.
    pub fn identity_paired(&self, config: &ferb_core::config::Config) -> bool {
        let Some((b, f)) = identity_profile(config) else {
            return false;
        };
        self.pairings
            .for_profile(&b, &f)
            .ok()
            .flatten()
            .is_some_and(|p| p.identity == ferb_core::types::Identity::Ferb)
    }
}

fn family_tools(t: &Arc<Tools>) -> Vec<Arc<dyn Tool>> {
    t.verbs()
        .into_iter()
        .map(|v| {
            let name: &'static str = Box::leak(t.name(v).into_boxed_str());
            Arc::new(BrowserTool {
                name,
                verb: v,
                tools: t.clone(),
            }) as Arc<dyn Tool>
        })
        .collect()
}

/// One browser verb of one family through the gateway's `Tool` trait.
pub struct BrowserTool {
    name: &'static str,
    verb: &'static Verb,
    tools: Arc<Tools>,
}

fn v(args: &Args) -> Value {
    Value::Object(args.clone())
}

impl Tool for BrowserTool {
    fn name(&self) -> &str {
        self.name
    }

    fn describe(&self, args: &Args) -> ActionSpec {
        self.tools.describe(self.verb, &v(args))
    }

    fn locks(&self, args: &Args) -> Vec<String> {
        self.tools.locks(self.verb, &v(args))
    }

    fn file_paths(&self, args: &Args) -> Option<Vec<(String, String)>> {
        let out = self.tools.file_paths(self.verb, &v(args));
        if out.is_empty() {
            return None;
        }
        Some(
            out.into_iter()
                .map(|(p, a)| (p, a.as_str().to_string()))
                .collect(),
        )
    }

    fn real_path(&self, _args: &Args, _workspace: &str) -> Option<RealPath> {
        None
    }

    fn ledger_args(&self, args: &Args, hashed: &Args) -> Option<Args> {
        Some(self.tools.ledger_args(&v(args), hashed))
    }

    /// `preview_target` (browser/tools.py:180): the tab the call names; nothing is looked up until a frame is asked.
    fn preview_target(
        &self,
        args: &Args,
        owner: &str,
        observed: Option<&Map<String, Value>>,
    ) -> Result<Option<ferb_gateway::tool::PreviewTarget>, String> {
        let Some(t) = self.tools.preview_target(&v(args), observed.is_some()) else {
            return Ok(None);
        };
        let resolve = crate::compose::previews::browser_resolve(
            self.tools.driver.clone(),
            t.tab,
            owner.to_string(),
        );
        Ok(Some(ferb_gateway::tool::PreviewTarget {
            scope: t.scope,
            label: t.label,
            capture: Some(crate::compose::previews::capture_of(resolve)),
        }))
    }

    /// D31 (browser/tools.py:988-997, `BrowserAct.terminal_input`): what this act would put into a terminal in the
    /// page, from the driver's snapshot facts now; only browser.act has the hook.
    fn terminal_input<'a>(
        &'a self,
        args: &'a Args,
        _task_id: &'a str,
    ) -> BoxFuture<'a, Option<ferb_core::types::TerminalInput>> {
        Box::pin(async move {
            if self.verb.verb != "act" {
                return None;
            }
            self.tools.terminal_input(&v(args))
        })
    }

    /// L33 (browser/tools.py:980-986, 1049-1051): the element the call names, for FERB's cursor; act, login_code and
    /// secret have the hook, and anything unsure is no point.
    fn point<'a>(
        &'a self,
        args: &'a Args,
        grant: &'a ferb_rpc::auth::RunGrant,
    ) -> BoxFuture<'a, Result<Option<ferb_gateway::tool::Point>, String>> {
        Box::pin(async move {
            Ok(self
                .tools
                .point(self.verb, &v(args), &grant.task_id)
                .await
                .map(|p| ferb_gateway::tool::Point {
                    kind: p.kind.to_string(),
                    rect: [p.rect.0, p.rect.1, p.rect.2, p.rect.3],
                    display: p.display,
                }))
        })
    }

    /// GATE-579 (pay_picture.py:55-90 `capture`; the gateway holds the call's locks and the time limit): a task tab's
    /// owner-only view when its extension has one (HAND-497), else the live preview's capture of the call's tab; a
    /// failure anywhere is no picture.
    fn pay_picture<'a>(&'a self, args: &'a Args, owner: &'a str) -> BoxFuture<'a, Option<Vec<u8>>> {
        Box::pin(async move {
            let a = v(args);
            let tab = args
                .get("tab")
                .filter(|t| ferb_py::value::truthy(t))
                .map(ferb_py::value::py_str);
            let target = self.tools.preview_target(&a, false);
            if tab.is_none() && target.is_none() {
                return None;
            }
            let driver = self.tools.driver.clone();
            let take = async move {
                if let Some(tab) = &tab {
                    let held = driver.owned_by(tab, owner, true).ok()?;
                    if driver.owner_view_supported(&held).ok()? {
                        // GATE-579: unmasked, for the owner's eyes only
                        return driver
                            .owner_view(&held)
                            .await
                            .ok()
                            .map(|i| i.jpeg().to_vec());
                    }
                }
                // an older extension: the tab's masked preview frame, used as it is
                let t = target?;
                let resolve = crate::compose::previews::browser_resolve(
                    driver.clone(),
                    t.tab,
                    owner.to_string(),
                );
                let (_, shoot) = resolve().await.ok()?;
                shoot().await.ok().map(|(jpeg, _)| jpeg)
            };
            // `PREVIEW_DEADLINE`: the capture's own waits fit the picture's limit
            let ctx = Arc::new(ferb_mac_hands::point::PreviewCtx {
                deadline: crate::compose::previews::monotonic()
                    + ferb_gateway::pay_picture::PICTURE_LIMIT_S,
                phases: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            });
            ferb_mac_hands::point::PREVIEW
                .scope(ctx, take)
                .await
                .filter(|b| !b.is_empty())
        })
    }

    /// GATE-580 (pay_press.py:26-60 `read`; the gateway holds the call's locks and the time limit): the tab's address
    /// now (`shows`) and the money amounts on the page's text lines holding the approved amount's last digits
    /// (`find`, past the latest snapshot's matching elements); a read that fails reads as nothing (both None).
    fn pay_press<'a>(
        &'a self,
        args: &'a Args,
        owner: &'a str,
        declared: Option<&'a str>,
    ) -> BoxFuture<'a, Option<ferb_policy::payments::Pressed>> {
        use ferb_policy::payments::Pressed;
        use ferb_py::value::{py_str, truthy};
        Box::pin(async move {
            let tab = args.get("tab").filter(|t| truthy(t)).map(py_str)?;
            let needle = ferb_policy::payments::digits_of(declared);
            let d = &self.tools.driver;
            let look = async {
                let held = d.owned_by(&tab, owner, true).ok()?;
                let shows = if needle.is_empty() { " " } else { &needle };
                let now = d.shows(&held, shows).await.ok()?;
                if !now.get("open").is_some_and(truthy) {
                    return Some(Pressed::unread());
                }
                let mut amounts = Some(Vec::new());
                if !needle.is_empty() {
                    let found = d.find(&held, &needle, 0, 1_000_000).await.ok()?;
                    let hits: Option<Vec<Value>> =
                        if found.get("next_offset").is_none_or(Value::is_null) {
                            let text = found.get("text")?.as_str()?;
                            Some(serde_json::from_str(text).ok()?)
                        } else {
                            None
                        };
                    // HAND-484: the latest snapshot's matching elements come first (as old as that snapshot): only
                    // the page's text lines, read now, count
                    let skip = found
                        .get("element_matches")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as usize;
                    amounts = hits.map(|hits| {
                        let lines: Vec<String> = hits
                            .iter()
                            .skip(skip)
                            .map(|h| match h.get("text") {
                                Some(t) if truthy(t) => py_str(t),
                                _ => String::new(),
                            })
                            .collect();
                        ferb_browser::money::page_amounts(lines.iter().map(|l| Some(l.as_str())))
                    });
                }
                let url = now.get("url").filter(|u| truthy(u)).map(py_str);
                Some(Pressed { url, amounts })
            };
            Some(look.await.unwrap_or_else(Pressed::unread))
        })
    }

    /// D6/D26 (call_hooks.py:194-216): the task tab's picture now, with the page's masking note, for a browser
    /// snapshot's first picture; its secret fields that couldn't be painted over withhold it (FERB's words).
    fn tab_picture<'a>(
        &'a self,
        args: &'a Args,
        task: &'a str,
    ) -> BoxFuture<'a, Result<(Vec<u8>, String), ferb_gateway::tool::Shot>> {
        use ferb_gateway::tool::Shot;
        Box::pin(async move {
            let shot = |e: ferb_browser::error::Error| match e.class() {
                "NoTarget" => Shot::NoTarget,
                "Withheld" => Shot::Withheld(e.message()),
                _ => Shot::Failed,
            };
            let tab = args
                .get("tab")
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            let d = &self.tools.driver;
            let held = d.owned_by(&tab, task, true).map_err(shot)?;
            // the snapshot's own generation: the element ids this call returned stay valid
            let image = d.screenshot(&held, true).await.map_err(shot)?;
            let masking = d.masking(&held).map_err(shot)?;
            Ok((image, masking))
        })
    }

    fn run<'a>(
        &'a self,
        args: &'a Args,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            match self.tools.run(self.verb, &v(args), ctx).await {
                Ok(r) => Ok(r.into()),
                Err(e) => Err(ToolError {
                    class: e.error.class().to_string(),
                    message: e.error.to_string(),
                    before_acting: !e.unknown,
                }),
            }
        })
    }
}

/// The browser area's seams (`areas/browsers.py`).
pub fn browsers_deps(
    opts: &Options,
    s: &Shared,
    notifier: &Arc<ferb_daemon::areas_b::notify::Notifier>,
) -> ferb_daemon::areas_b::browsers::Deps {
    use ferb_daemon::areas_b::browsers as b;
    let br = s.tools.browsers().expect("built with the tools");
    let native = opts.native;
    let launch: b::Launch = if opts.launch_browsers {
        Arc::new(|args: &[String]| ferb_browser::macos::launch_detached(args))
    } else {
        Arc::new(|_args: &[String]| {
            Err(std::io::Error::other(
                "this daemon was started without starting browsers",
            ))
        })
    };
    // the native host the manifests name: FERB's launcher in its home, running the Rust relay: FERB.app's
    // Contents/Helpers/ferb-native-host (ferb-device BUNDLE_RUST_NATIVE_HOST, what `setup --service` writes), else
    // the one built beside this program
    let relay = match &opts.app_bundle {
        Some(app) => app.join(ferb_device::install::BUNDLE_RUST_NATIVE_HOST),
        None => crate::host::real_exe()
            .and_then(|e| e.parent().map(|d| d.join("ferb-native-host")))
            .unwrap_or_default(),
    };
    b::Deps {
        pairings: br.pairings.clone(),
        hub: br.hub.clone(),
        data_access: Arc::new(move || {
            native
                && ferb_mac_hands::native::permissions::full_disk_access()
                    == ferb_mac_hands::permissions::Status::Granted
        }),
        platform: Arc::new(ferb_browser::macos::MacPlatform),
        binaries: Arc::new(ferb_browser::profiles::Binaries::default()),
        browser_support: opts.browser_support.clone(),
        browser_data_dirs: crate::off::off("browser.data_dirs", None),
        launch,
        wake_url: match br.wake.clone() {
            Some(w) => Arc::new(move || w.url()),
            // no runtime to serve it on (a failed bind is logged at start): no wake page to open
            None => crate::off::off("browser.no_wake_page", Arc::new(String::new)),
        },
        // the router's tabs (`self.router.shows`, `show_tab`, `tab_profile`): a human check handed to the owner
        tabs: Some(Arc::new(crate::compose::areas::RouterTabs(
            br.router.clone(),
        ))),
        // `self._notify` (areas/notify.py)
        notifier: Arc::new(crate::compose::areas::BrowserBanners(notifier.clone())),
        // `supervisor.activity.waiting(worker_id, on)` (browsers.py:680): the owner's time never counts as a stall
        activity: Some({
            let t = s.tools.clone();
            Arc::new(move |worker: &str, on: bool| {
                if let Some(sup) = t.supervisor() {
                    sup.worker_waiting(worker, on);
                }
            })
        }),
        identity_browser: Arc::new(IdentityAttach {
            registry: {
                let gw = s.gateway.clone();
                Arc::new(
                    move |f: &mut dyn FnMut(&mut ferb_gateway::tool::Registry)| {
                        f(&mut gw.registry())
                    },
                )
            },
            tools: br.ferb_tools(),
            driver: br.identity.clone(),
        }),
        native_host: b::NativeHost {
            launcher: s.home.join("bin/ferb-native-host"),
            exec_part: ferb_device::py::shlex_quote(&relay.to_string_lossy()),
        },
        chrome_data: opts
            .user_home
            .join("Library/Application Support/Google/Chrome"),
        new_handoff_id: Arc::new(|| ferb_runtimes::codex::token_hex(6)),
        pair_load_wait_s: 2.0,
        pair_timeout_s: 60.0,
        handoff_poll_s: 2.0,
        state: Arc::new(b::State::default()),
    }
}

/// `use_identity_browser` / `_detach_identity_browser` (browsers.py:44-65): FERB's own browser's `ferb_browser.*`
/// tools in the gateway's registry, and its tabs closed when its account is cleared.
pub struct IdentityAttach {
    /// The gateway's registry, held for one change or read.
    pub registry: WithRegistry,
    /// FERB's own profile's tools (`Browsers::ferb_tools`).
    pub tools: Vec<Arc<dyn Tool>>,
    /// FERB's own profile's driver (`self.ferb_identity`); None: not built.
    pub driver: Option<Arc<Driver>>,
}

/// One visit to the gateway's registry.
pub type WithRegistry =
    Arc<dyn Fn(&mut dyn FnMut(&mut ferb_gateway::tool::Registry)) + Send + Sync>;

impl IdentityAttach {
    fn prefix() -> String {
        format!("{}.", FERB_BROWSER.prefix)
    }
}

impl ferb_daemon::areas_b::browsers::IdentityBrowser for IdentityAttach {
    fn ready(&self) -> bool {
        self.driver.is_some() && !self.tools.is_empty()
    }
    fn attached(&self) -> bool {
        let prefix = Self::prefix();
        let mut found = false;
        (self.registry)(&mut |reg| found = reg.names().iter().any(|n| n.starts_with(&prefix)));
        found
    }
    fn attach(&self) {
        (self.registry)(&mut |reg| {
            for t in &self.tools {
                reg.set(t.clone());
            }
        });
    }
    fn detach(&self) {
        let prefix = Self::prefix();
        let mut names: Vec<String> = Vec::new();
        (self.registry)(&mut |reg| {
            names = reg
                .names()
                .into_iter()
                .filter(|n| n.starts_with(&prefix))
                .collect();
            for n in &names {
                reg.remove(n);
            }
        });
        // the tabs it opened close (no task can release them through it any more)
        if let (false, Some(d)) = (names.is_empty(), self.driver.clone()) {
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                rt.spawn(async move { d.shutdown().await });
            }
        }
    }
}

/// The native host's channel on the RPC server: each connection after its hello is served by the hub.
pub fn native_channel(p: &Parts) -> Option<ferb_browser::bridge::native::Channel> {
    let hub = p.browsers()?.hub.clone();
    Some(ferb_browser::bridge::native::Channel {
        hub,
        browser_pid: peer_seams().0,
    })
}

pub fn _unused(_: PathBuf, _: Family) {}

/// `accepted_extension_ids(config)`: the FERB extension ids a native host's hello may name.
pub fn extension_ids(config: &ferb_core::config::Config) -> std::collections::HashSet<String> {
    let configured = config.get("browser", "extension_ids").ok().flatten();
    let store = ferb_browser::autoinstall::store_extension_id(
        config
            .get("browser", "store_extension_id")
            .ok()
            .flatten()
            .as_ref(),
    );
    ferb_browser::autoinstall::accepted_extension_ids(configured.as_ref(), &store)
        .into_iter()
        .collect()
}

#[cfg(test)]
mod wake_tests {
    use super::WakePage;

    /// `WakeServer`: each wake is its own address on 127.0.0.1 (a pairing's blank page must never be about:blank or
    /// empty: an empty address opened the browsers with nothing the pairing could close), and it answers a page.
    #[tokio::test]
    async fn each_wake_is_a_unique_local_page_that_answers() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let wake = WakePage::start().expect("the wake page starts");
        let (a, b) = (wake.url(), wake.url());
        assert_ne!(a, b);
        let rest = a
            .strip_prefix("http://127.0.0.1:")
            .expect("a local address");
        let (port, path) = rest.split_once('/').unwrap();
        assert!(
            path.starts_with("wake/") && path.len() == "wake/".len() + 16,
            "{a}"
        );
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap()))
            .await
            .unwrap();
        s.write_all(format!("GET /{path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut got = String::new();
        s.read_to_string(&mut got).await.unwrap();
        assert!(got.starts_with("HTTP/1.1 200 OK\r\n"), "{got}");
        assert!(got.ends_with("<!doctype html><title>FERB</title>"), "{got}");
    }
}

#[cfg(test)]
mod identity_tests {
    use std::sync::{Arc, Mutex};

    use ferb_core::interfaces::BoxFuture;
    use ferb_core::types::{ActionSpec, ToolContext};
    use ferb_daemon::areas_b::browsers::IdentityBrowser;
    use ferb_gateway::tool::{Args, Registry, Tool, ToolError, ToolOutput};

    use super::{IdentityAttach, WithRegistry};

    struct Named(&'static str);
    impl Tool for Named {
        fn name(&self) -> &str {
            self.0
        }
        fn describe(&self, _args: &Args) -> ActionSpec {
            ActionSpec::default()
        }
        fn run<'a>(
            &'a self,
            _args: &'a Args,
            _ctx: &'a ToolContext,
        ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
            Box::pin(async {
                Err(ToolError {
                    class: "Unused".into(),
                    message: String::new(),
                    before_acting: true,
                })
            })
        }
    }

    /// browsers.py:44-65, 342-350: FERB's own browser, once its profile is paired as FERB, puts its `ferb_browser.*`
    /// tools in the gateway's registry (so `identity.check` says paired and tasks can use FERB's own account); clearing
    /// the account removes exactly those, never the owner's `browser.*`. Not built (no tools): never ready.
    #[test]
    fn ferbs_own_browser_attaches_and_detaches_its_tools_only() {
        let shared = Arc::new(Mutex::new(Registry::new()));
        shared.lock().unwrap().set(Arc::new(Named("browser.open")));
        let s = shared.clone();
        let registry: WithRegistry = Arc::new(move |f| f(&mut s.lock().unwrap()));
        let none = IdentityAttach {
            registry: registry.clone(),
            tools: vec![],
            driver: None,
        };
        assert!(!none.ready());
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(Named("ferb_browser.open")),
            Arc::new(Named("ferb_browser.fill_secret")),
        ];
        let ferb = IdentityAttach {
            registry,
            tools,
            driver: None,
        };
        assert!(!ferb.attached());
        ferb.attach();
        assert!(ferb.attached());
        let mut names = shared.lock().unwrap().names();
        names.sort();
        assert_eq!(
            names,
            [
                "browser.open",
                "ferb_browser.fill_secret",
                "ferb_browser.open"
            ]
        );
        ferb.detach();
        assert!(!ferb.attached());
        assert_eq!(shared.lock().unwrap().names(), ["browser.open"]);
    }
}

#[cfg(test)]
mod wiring_tests {
    use std::sync::{Arc, OnceLock};

    use ferb_browser::bridge::Hub;
    use ferb_browser::driver::{Driver, Kind, Tasks};
    use ferb_browser::pairing::Pairings;
    use ferb_browser::tools::{Tools, OWNER_BROWSER};
    use ferb_core::types::{Identity, PathCheck, ToolContext};
    use ferb_gateway::tool::{Args, Shot, Tool};
    use serde_json::{json, Value};

    use super::{BrowserHost, BrowserPolicy, BrowserTool};

    struct NoLedger;
    impl ferb_browser::bridge::Ledger for NoLedger {
        fn state(&self, _intent_id: &str) -> Option<String> {
            None
        }
        fn reconcile(&self, _intent_id: &str, _observed: &str, _evidence: Vec<String>) {}
    }

    fn host() -> Arc<BrowserHost> {
        Arc::new(BrowserHost {
            home: "/invented-home".into(),
            area: Arc::new(OnceLock::new()),
            recordings: None,
        })
    }

    /// One owner-family verb over a driver with no browser connected (no tab is open).
    fn tool(verb: &str) -> BrowserTool {
        let random: Arc<dyn ferb_browser::pairing::Random> =
            Arc::new(ferb_browser::pairing::SystemRandom);
        let pairings = Arc::new(Pairings::new(
            Arc::new(ferb_browser::pairing::sqlite::SqliteInstalls::new(
                rusqlite::Connection::open_in_memory().unwrap(),
            )),
            Arc::new(ferb_clock::clock::FakeClock::new(None)),
            random.clone(),
        ));
        let hub = Hub::new(
            pairings,
            Arc::new(NoLedger),
            Arc::new(|_topic, _data| {}),
            Arc::new(|| "dev-invented".to_string()),
            random.clone(),
            Arc::new(super::Apps),
            super::peer_seams().1,
        );
        let driver = Driver::new(
            hub,
            Kind::Owner {
                choose: Arc::new(|_profile: Option<&str>| {
                    Err(ferb_browser::error::Error::refused("no profile"))
                }),
                input: None,
            },
            Tasks::default(),
            None,
            random,
        );
        BrowserTool {
            name: Box::leak(format!("browser.{verb}").into_boxed_str()),
            verb: ferb_browser::tools::verb(verb).expect("a browser verb"),
            tools: Arc::new(Tools {
                family: OWNER_BROWSER,
                driver,
                policy: Arc::new(BrowserPolicy),
                host: host(),
            }),
        }
    }

    fn args(v: Value) -> Args {
        v.as_object().cloned().unwrap()
    }

    fn ctx(workspace: &str, closed: Option<PathCheck>) -> ToolContext {
        ToolContext {
            task_id: "t-1".into(),
            worker_id: "w-1".into(),
            request_id: "r-1".into(),
            identity: Identity::Owner,
            scopes: Default::default(),
            device_id: "dev-invented".into(),
            span_id: None,
            workspace: workspace.into(),
            owner_roots: vec![],
            untrusted_roots: vec![],
            intent_id: None,
            observe_only: false,
            probe: false,
            path_closed: closed,
            escape: None,
            terminal: None,
            folders: vec![],
        }
    }

    // decision: GATE-580
    /// pay_press.py:26-60: a call with no tab has nothing to read (None); a tab that isn't the task's (or a read that
    /// fails) reads as a page with no address and no amounts, so the press is compared against nothing fresher.
    #[tokio::test(flavor = "current_thread")]
    async fn a_payment_press_reads_the_named_tab_or_nothing() {
        let t = tool("act");
        assert!(t
            .pay_press(&args(json!({"element": "e1"})), "t-1", Some("$12.50"))
            .await
            .is_none());
        let read = t
            .pay_press(
                &args(json!({"tab": "tab-9", "element": "e1", "action": "click"})),
                "t-1",
                Some("$12.50"),
            )
            .await
            .expect("a browser tab is read");
        assert_eq!(read.url, None);
        assert_eq!(read.amounts, None);
    }

    // decision: GATE-579
    /// pay_picture.py:55-90: no tab and no surface is no picture; a tab that isn't the task's is no picture (never
    /// another tab's).
    #[tokio::test(flavor = "current_thread")]
    async fn a_payment_picture_is_only_of_the_tasks_own_tab() {
        let t = tool("act");
        assert!(t.pay_picture(&args(json!({})), "t-1").await.is_none());
        assert!(t
            .pay_picture(&args(json!({"tab": "tab-9", "element": "e1"})), "t-1")
            .await
            .is_none());
    }

    /// call_hooks.py:194-228 (D6/D26): a snapshot's first picture of a tab that isn't the task's is "unavailable",
    /// never a picture of something else; L33 (tools.py:980-986): no tab, no point; D31 (tools.py:988-997): only
    /// browser.act has terminal facts, and a tab the driver doesn't hold has none.
    #[tokio::test(flavor = "current_thread")]
    async fn a_tab_the_task_doesnt_hold_gives_no_picture_point_or_terminal_input() {
        let snap = tool("snapshot");
        assert_eq!(
            snap.tab_picture(&args(json!({"tab": "tab-9"})), "t-1")
                .await,
            Err(Shot::Failed)
        );
        let act = tool("act");
        let a = args(json!({"tab": "tab-9", "element": "e1", "action": "type", "text": "ls\n"}));
        let grant = ferb_rpc::auth::RunGrant {
            task_id: "t-1".into(),
            worker_id: "w-1".into(),
            scopes: Default::default(),
            identity: ferb_rpc::auth::Identity::Owner,
            expires_at: 0.0,
            observe_only: false,
            tools: Default::default(),
            max_calls: 0,
        };
        assert_eq!(act.point(&a, &grant).await, Ok(None));
        assert!(act.terminal_input(&a, "t-1").await.is_none());
        assert!(snap.terminal_input(&a, "t-1").await.is_none());
    }

    // decision: HAND-428
    /// browser/tools.py:723 (fs/files.py:295-307): a PDF FERB makes is a new file the call's protection check
    /// passes: a closed folder refuses and writes nothing, an existing file is never replaced.
    #[test]
    fn a_browser_save_passes_the_protection_lists_and_never_replaces() {
        use ferb_browser::tools::Host;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let open_dir = root.join("open");
        let closed_dir = root.join("closed");
        std::fs::create_dir_all(&open_dir).unwrap();
        std::fs::create_dir_all(&closed_dir).unwrap();
        let shut = closed_dir.to_string_lossy().into_owned();
        let check = PathCheck(Arc::new(move |p: &std::path::Path, access: &str| {
            (access == "write" && p.starts_with(&shut)).then(|| "a protected folder".to_string())
        }));
        let c = ctx(&root.to_string_lossy(), Some(check));
        let h = host();

        let refused = closed_dir.join("page.pdf");
        let e = h
            .new_file(&refused.to_string_lossy(), b"%PDF-1.4", &c)
            .expect_err("a protected folder refuses");
        assert_eq!(e.class(), "Refused");
        assert!(!refused.exists());

        let made = open_dir.join("page.pdf");
        h.new_file(&made.to_string_lossy(), b"%PDF-1.4", &c)
            .unwrap();
        assert_eq!(std::fs::read(&made).unwrap(), b"%PDF-1.4");
        let again = h
            .new_file(&made.to_string_lossy(), b"other", &c)
            .expect_err("never over an existing file");
        assert_eq!(again.class(), "Refused");
        assert_eq!(std::fs::read(&made).unwrap(), b"%PDF-1.4");
    }
}
