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
    fn new_file(
        &self,
        path: &str,
        data: &[u8],
        _ctx: &ToolContext,
    ) -> ferb_browser::error::Result<()> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| ferb_browser::error::Error::driver(e.to_string()))?;
        f.write_all(data)
            .map_err(|e| ferb_browser::error::Error::driver(e.to_string()))
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
        // every tool asks with its whole context (`ask_code_for`)
        Box::pin(async { None })
    }
    fn ask_secret(
        &self,
        _task: &str,
        _intent: &str,
        _origin: &str,
        _ask: ferb_browser::driver::SecretAsk,
    ) -> BoxFuture<'_, Option<ferb_identity::Secret>> {
        Box::pin(async { None })
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
            Arc::new(|_pid| None),
        );
        let tasks = driver_tasks(p);
        let area: Arc<OnceLock<ferb_daemon::areas_b::browsers::Cx>> = Arc::new(OnceLock::new());
        let router = Driver::new(
            hub.clone(),
            Kind::Owner {
                choose: resolver(area.clone(), pairings.clone()),
                input: Some(Arc::new(GatewayInput(p.downgrade()))),
            },
            tasks,
            None,
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
            None,
            random,
        );
        let ferb = Arc::new(Tools {
            family: FERB_BROWSER,
            driver: identity.clone(),
            policy: Arc::new(BrowserPolicy),
            host,
        });
        Browsers {
            pairings,
            hub,
            router,
            identity: Some(identity),
            owner,
            ferb: Some(ferb),
            area,
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
    _notifier: &Arc<ferb_daemon::areas_b::notify::Notifier>,
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
        browser_data_dirs: None,
        launch,
        wake_url: Arc::new(String::new),
        tabs: None,
        notifier: Arc::new(NoBanner),
        activity: None,
        identity_browser: Arc::new(NoIdentity),
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

struct NoBanner;
impl ferb_daemon::areas_b::browsers::Notifier for NoBanner {
    fn notify(
        &self,
        _t: &str,
        _x: &str,
        _u: bool,
        _c: &str,
        _g: Value,
        _i: &str,
    ) -> ferb_daemon::areas_b::browsers::BoxFut<()> {
        Box::pin(async {})
    }
}

struct NoIdentity;
impl ferb_daemon::areas_b::browsers::IdentityBrowser for NoIdentity {
    fn ready(&self) -> bool {
        false
    }
    fn attached(&self) -> bool {
        false
    }
    fn attach(&self) {}
    fn detach(&self) {}
}

/// The native host's channel on the RPC server: each connection after its hello is served by the hub.
pub fn native_channel(p: &Parts) -> Option<ferb_browser::bridge::native::Channel> {
    let hub = p.browsers()?.hub.clone();
    Some(ferb_browser::bridge::native::Channel {
        hub,
        browser_pid: Arc::new(|_fd| None),
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
