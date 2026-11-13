//! FERB's Mac hands (`_build_mac_and_shell`'s Mac half, `add_mac`, `add_hearing`, `add_recording`): the Mac driver
//! composed over ferb-mac-hands' native layer (Accessibility, the window list and ScreenCaptureKit, CGEvents to one
//! process, the pasteboard, Launch Services), its 12 `mac.*` tools, `recording.*`, `audio.listen`, and the image
//! sanitizer every tool image passes (Vision's text recognition, then painting).
//!
//! Background first (rule 10): the owner-input tap is not composed in this build, so FERB can't tell whether the owner
//! is using the Mac; as Python's daemon with the tap down, foreground input is unavailable (it says why) and only the
//! background routes (Accessibility, events to one process, window capture) run.

use std::collections::BTreeSet;
use std::sync::Arc;

use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{ActionSpec, TerminalInput, ToolContext, ToolResult};
use ferb_gateway::tool::{Args, Point, PreviewTarget, Tool, ToolError, ToolOutput};
use ferb_mac_hands::error::HandError;
use ferb_mac_hands::tools::MacTools;
use serde_json::{json, Map, Value};

use super::Parts;

/// Nothing is painted (the evidence store only lists or purges here).
pub struct NoPaint;
impl ferb_mac_hands::ocr::Paint for NoPaint {
    fn paint(
        &self,
        _image: &[u8],
        _boxes: &[ferb_mac_hands::mask::ImgBox],
    ) -> Result<Vec<u8>, String> {
        Err("nothing is painted here".into())
    }
}

/// ferb-policy's terminal, browser-page and destination facts, as the driver reads them.
pub struct MacPolicy;

impl ferb_mac_hands::driver::Policy for MacPolicy {
    fn surface(
        &self,
        bundle_id: &str,
        chain: &[ferb_mac_hands::driver::Facts],
        owner_apps: &[String],
        shell_app: bool,
        unread: bool,
    ) -> Option<String> {
        ferb_policy::terminal::surface(bundle_id, chain, owner_apps, shell_app, unread)
    }
    fn can_host(&self, bundle_id: &str, owner_apps: &[String], shell_app: bool) -> bool {
        ferb_policy::terminal::can_host(bundle_id, owner_apps, shell_app)
    }
    fn run_menu(&self, path: &[String]) -> bool {
        ferb_policy::terminal::run_menu(path)
    }
    fn terminal_menu(&self, title: &str) -> bool {
        ferb_policy::terminal::terminal_menu(title)
    }
    fn run_control(&self, chain: &[ferb_mac_hands::driver::Facts], editor: bool) -> Option<String> {
        ferb_policy::terminal::run_control(chain, editor)
    }
    fn editor_app(&self, bundle_id: &str) -> bool {
        ferb_policy::terminal::editor_app(bundle_id)
    }
    fn structural(&self, facts: &ferb_mac_hands::driver::Facts) -> bool {
        ferb_policy::terminal::structural(facts)
    }
    fn newline(&self, text: &str) -> bool {
        ferb_policy::terminal::newline(text)
    }
    /// `terminals.CODE_RUNNERS.get(bundle_id.casefold())`.
    fn code_runner(&self, bundle_id: &str) -> Option<String> {
        let b = ferb_sanitize::parity::casefold(bundle_id);
        ferb_policy::terminal::CODE_RUNNERS
            .iter()
            .find(|(k, _)| *k == b)
            .map(|(_, why)| why.to_string())
    }
    /// `bundle_id.casefold() in terminals.OPENS_FILES`.
    fn opens_files(&self, bundle_id: &str) -> bool {
        ferb_policy::terminal::OPENS_FILES
            .contains(&ferb_sanitize::parity::casefold(bundle_id).as_str())
    }
    /// `terminals.UNASKED.format(why=why, app=app)`.
    fn unasked(&self, why: &str, app: &str) -> String {
        ferb_policy::terminal::UNASKED
            .replacen("{why}", why, 1)
            .replacen("{app}", app, 1)
    }
    fn browser_page(&self, address: Option<&str>) -> bool {
        ferb_policy::confirm::browser_page(address)
    }
    fn typed_destination(&self, text: &str) -> Option<String> {
        ferb_policy::provenance::typed_destination(text)
    }
}

/// mac.applescript's sandbox: ferb-shell's profile for osascript (Apple Events to that app only, no launch, no other
/// program), with the shell's clean environment.
pub struct ScriptSandbox(pub Arc<ferb_shell::sandbox::Sandbox>);

impl ferb_mac_hands::applescript::ScriptSandbox for ScriptSandbox {
    fn wrap(&self, argv: &[String], events_to: &str) -> (Vec<String>, Vec<(String, String)>) {
        let wrapped = self
            .0
            .wrap(
                argv,
                &ferb_shell::sandbox::Options {
                    apps: true,
                    events_to: Some(events_to),
                    ..Default::default()
                },
            )
            // a profile that can't be written: the command can't run contained, so it runs nothing at all
            .unwrap_or_else(|_| vec!["/usr/bin/false".to_string()]);
        (wrapped, ferb_shell::manager::clean_env(false))
    }
    fn cleanup(&self, argv: &[String]) {
        ferb_shell::sandbox::Sandbox::cleanup(
            ferb_shell::sandbox::Sandbox::profile_path(argv).as_deref(),
        );
    }
}

/// The fs tools' path rules and the protection lists for recordings (recording.py's `FileGate`).
pub struct RecordingFiles {
    pub home: String,
    pub protection: Option<Arc<dyn ferb_shell::protection::Protection>>,
}

impl ferb_mac_hands::recording::FileGate for RecordingFiles {
    fn resolve(
        &self,
        path: &str,
        ctx: &ToolContext,
        must_exist: bool,
    ) -> ferb_mac_hands::error::Result<std::path::PathBuf> {
        ferb_fs::files::resolve(path, &ctx.workspace, &self.home, must_exist)
            .map(std::path::PathBuf::from)
            .map_err(|e| match e {
                ferb_fs::files::Fail::Refused(m) => HandError::Refused(m),
                other => HandError::Other(format!("{other:?}")),
            })
    }
    fn closed(&self, path: &std::path::Path) -> Option<String> {
        let p = self.protection.as_ref()?;
        let text = path.to_string_lossy();
        p.writes()
            .iter()
            .find(|e| text == e.path.to_string_lossy() || path.starts_with(&e.path))
            .map(|e| format!("{} is protected ({}): FERB doesn't write there", text, e.id))
    }
    fn make_folders(&self, dir: &std::path::Path) -> ferb_mac_hands::error::Result<()> {
        if let Some(why) = self.closed(dir) {
            return Err(HandError::Refused(why));
        }
        std::fs::create_dir_all(dir).map_err(|e| HandError::Other(e.to_string()))
    }
}

/// FERB.app's on-device speech recognition (`Transcriber`), its transcript through the Redactor.
pub struct Transcribe {
    pub transcriber: Arc<ferb_sense::transcribe::Transcriber>,
    pub redactor: Arc<ferb_sanitize::redact::Redactor>,
}

impl ferb_mac_hands::listen::Transcribe for Transcribe {
    fn transcribe<'a>(
        &'a self,
        path: &'a std::path::Path,
        language: Option<&'a str>,
        audio_s: f64,
        offset: f64,
    ) -> ferb_mac_hands::listen::BoxFut<'a, ferb_mac_hands::error::Result<Map<String, Value>>> {
        Box::pin(async move {
            let t = self
                .transcriber
                .transcribe(path, language, audio_s)
                .await
                .map_err(|e| HandError::Refused(e.message().to_string()))?;
            let r = self.redactor.clone();
            let locale = t.locale.clone();
            let mut out =
                crate::basics::object(ferb_sense::transcribe::redacted(&t, &|s| r.text(s), offset));
            out.insert("language".into(), json!(locale));
            Ok(out)
        })
    }
}

/// No app sound can be captured (a daemon started without the Mac's parts).
pub struct NoCapture;
impl ferb_mac_hands::listen::Capture for NoCapture {
    fn start(
        &self,
        _bundle_id: &str,
        _pid: i32,
        _path: &std::path::Path,
    ) -> ferb_mac_hands::error::Result<Arc<dyn ferb_mac_hands::listen::Running>> {
        Err(HandError::Refused(
            "this FERB was started without the Mac's own parts: it can't capture an app's sound"
                .into(),
        ))
    }
}

/// The image sanitizer (L13): Vision's text recognition, codes, cards and known secrets painted.
pub struct Images {
    pub redactor: Arc<ferb_sanitize::redact::Redactor>,
    pub native: bool,
}

impl Images {
    pub fn redact(&self, image: &[u8]) -> Result<Vec<u8>, String> {
        if !self.native {
            return Err(ferb_mac_hands::ocr::UNAVAILABLE.into());
        }
        ferb_mac_hands::ocr::ImageRedactor {
            redactor: self.redactor.clone(),
            recognize: Arc::new(ferb_mac_hands::native::vision::NativeVision),
            paint: Arc::new(ferb_mac_hands::native::paint::NativePaint),
        }
        .redact(image, &[])
    }
}

impl ferb_gateway::call_hooks::ImageSanitizer for Images {
    fn sanitize<'a>(&'a self, image: &'a [u8]) -> BoxFuture<'a, Result<Vec<u8>, String>> {
        let image = image.to_vec();
        let me = Images {
            redactor: self.redactor.clone(),
            native: self.native,
        };
        // a plain thread: the first recognition can take minutes, and a shutdown never waits for it
        let (tx, rx) = tokio::sync::oneshot::channel();
        let spawned = std::thread::Builder::new()
            .name("ferb-image-redact".into())
            .spawn(move || {
                let _ = tx.send(me.redact(&image));
            });
        Box::pin(async move {
            if spawned.is_err() {
                return Err(ferb_mac_hands::ocr::UNAVAILABLE.to_string());
            }
            rx.await
                .unwrap_or_else(|_| Err(ferb_mac_hands::ocr::UNAVAILABLE.to_string()))
        })
    }
}

/// The working app's first picture (`app_window`) and its secret fields painted (`mask_secret_fields`).
pub struct AppWindows(pub Arc<MacTools>);

impl ferb_gateway::call_hooks::AppWindows for AppWindows {
    fn shot<'a>(
        &'a self,
        app: &'a str,
        scopes: &'a BTreeSet<String>,
    ) -> BoxFuture<'a, Result<Option<ferb_gateway::call_hooks::AppShot>, ferb_gateway::tool::Shot>>
    {
        let (tools, app, scopes) = (self.0.clone(), app.to_string(), scopes.clone());
        Box::pin(async move {
            let shot = off_thread(move || {
                Box::pin(
                    async move { ferb_mac_hands::tools::app_window(&tools, &app, &scopes).await },
                )
            })
            .await
            .unwrap_or_else(|| Err(HandError::Other("the capture's thread ended".into())));
            match shot {
                Ok(None) => Ok(None),
                Ok(Some(w)) => Ok(Some(ferb_gateway::call_hooks::AppShot {
                    image: w.image,
                    rects: w
                        .rects
                        .iter()
                        .map(|&(x, y, w, h)| [x as f64, y as f64, w as f64, h as f64])
                        .collect(),
                    facts: w.facts,
                    read_sources: w.read_sources,
                })),
                Err(HandError::NoTarget(_)) => Err(ferb_gateway::tool::Shot::NoTarget),
                Err(HandError::Withheld(m)) => Err(ferb_gateway::tool::Shot::Withheld(m)),
                Err(_) => Err(ferb_gateway::tool::Shot::Failed),
            }
        })
    }
    fn mask<'a>(
        &'a self,
        image: Vec<u8>,
        rects: &'a [[f64; 4]],
    ) -> BoxFuture<'a, Result<Vec<u8>, ferb_gateway::tool::Shot>> {
        let boxes: Vec<ferb_mac_hands::mask::ImgBox> = rects
            .iter()
            .map(|r| (r[0] as i64, r[1] as i64, r[2] as i64, r[3] as i64))
            .collect();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                ferb_mac_hands::native::paint::mask_secret_fields(&image, &boxes)
            })
            .await
            .ok()
            .and_then(Result::ok)
            .ok_or_else(|| {
                ferb_gateway::tool::Shot::Withheld(ferb_mac_hands::mask::UNPAINTABLE.into())
            })
        })
    }
}

/// The Mac's parts, built once.
pub struct Mac {
    pub tools: Arc<MacTools>,
    pub hearing: Arc<ferb_mac_hands::listen::Hearing>,
    pub recordings: Arc<ferb_mac_hands::recording::RecordingManager>,
    pub images: Arc<Images>,
}

fn config_bool(
    config: &ferb_core::config::Config,
    section: &str,
    key: &str,
    default: bool,
) -> bool {
    match config.get(section, key) {
        Ok(Some(Value::Bool(b))) => b,
        _ => default,
    }
}

impl Mac {
    pub fn build(opts: &crate::compose::Options, p: &Parts) -> Mac {
        use ferb_mac_hands::native;
        let config = p.config();
        let device_id = match config.get("device", "id") {
            Ok(Some(Value::String(s))) => s,
            _ => "unknown-device".into(),
        };
        let handlers = Arc::new(ferb_mac_hands::apps::WebHandlers::default());
        let mut input = ferb_mac_hands::input::RealInput::new(
            // SENSE-406: with the owner-input tap down FERB counts the owner as using the Mac
            Arc::new(|| true),
            ferb_mac_hands::locks::GuiLock::new(),
        );
        input.available = Arc::new(|| false);
        input.why_unavailable = Arc::new(|| ferb_mac_hands::input::UNMONITORED.to_string());
        let mut d = ferb_mac_hands::driver::MacDriver::new(ferb_mac_hands::driver::Parts {
            device_id,
            input,
            gestures: Arc::new(ferb_mac_hands::input::RealGestures {
                events: Arc::new(native::events::NativeEvents::new()),
            }),
            ax: Arc::new(native::ax::AxDriver::new(
                ferb_policy::confirm::browser_page,
            )),
            apps: Arc::new(native::apps::NativeApps::new(handlers.clone())),
            shots: Arc::new(native::capture::NativeShots),
            policy: Arc::new(MacPolicy),
            time: Arc::new(ferb_mac_hands::driver::RealTime::default()),
        });
        let c = config.clone();
        d.excluded = Arc::new(move || {
            c.exclude_apps()
                .map(|s| s.into_iter().collect())
                .unwrap_or_default()
        });
        d.profiles = Arc::new(ferb_mac_hands::profile::Profiles::new(
            Some(&p.home().join("device").join("apps")),
            Arc::new(ferb_mac_hands::profile::now_utc),
        ));
        let c = config.clone();
        d.snapshot_images = Arc::new(move || config_bool(&c, "mac", "snapshot_image", true));
        let c = config.clone();
        d.terminal_apps = Arc::new(move || match c.get("protection", "terminal_apps") {
            Ok(Some(Value::Array(a))) => a
                .iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect(),
            _ => ferb_policy::protection::DEFAULT_TERMINAL_APPS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        });
        let native_on = opts.mac;
        d.clipboard = Arc::new(move || {
            if native_on {
                ferb_mac_hands::tools::Pasteboard::read(&native::pasteboard::NativePasteboard)
            } else {
                (0, None, false)
            }
        });
        // L39: apps a task's calls launch are its resources, quit at its release unless the owner used them
        let db = p.db();
        let tasks = p.clone();
        let clock = p.clock();
        d.launched = Some(Arc::new(ferb_mac_hands::launched::LaunchedApps::new(
            Arc::new(native::apps::NativeControl),
            Arc::new(
                move |task: &str, app: &ferb_mac_hands::launched::Launched| {
                    let tree = tasks
                        .task(task)
                        .and_then(|t| t.tree_id)
                        .unwrap_or_else(|| task.split('.').next().unwrap_or(task).to_string());
                    let d = crate::basics::lock(&db);
                    ferb_daemon::areas_b::desktop::brought_forward(
                        d.conn(),
                        &tree,
                        app.pid as i64,
                        &app.at,
                    )
                    .unwrap_or(false)
                },
            ),
            Arc::new(move || ferb_core::when::isoformat(&clock.now())),
        )));
        d.route = native::events::load_route().is_some();
        d.handlers = handlers;
        let driver = Arc::new(d);

        let paint: ferb_mac_hands::driver::PaintFn = Arc::new(|jpeg, masks, bounds, size| {
            ferb_mac_hands::driver::painted(&native::paint::NativePaint, jpeg, masks, bounds, size)
        });
        let mut tools = MacTools::new(driver.clone(), paint);
        tools.home = p.user_home().to_string_lossy().into_owned();
        tools.cwd = p.home().join("workspace").to_string_lossy().into_owned();
        if let Some(protection) = p.shell_protection() {
            let sandbox = Arc::new(ferb_shell::sandbox::Sandbox::new(
                protection,
                p.home().join("profiles"),
                Arc::new(|| None),
                None,
            ));
            tools.sandbox = Some(Arc::new(ScriptSandbox(sandbox)));
        }
        tools.automation = Some(ferb_mac_hands::permissions::AutomationChecks::new(
            Arc::new(move |bundle: &str, ask: bool| {
                if native_on {
                    native::permissions::automation(bundle, ask)
                } else {
                    ferb_mac_hands::permissions::Status::Unknown
                }
            }),
        ));
        // mac.secret's hidden input: the gateway's question (D12), run on the calls' thread
        let gw_parts = p.clone();
        tools.ask_secret = Some(Arc::new(
            move |ctx, target, handle, remember, shown, field| {
                let p = gw_parts.clone();
                Box::pin(async move {
                    let gw = p.gateway()?;
                    let ask = ferb_gateway::gateway::SecretAsk {
                        handle,
                        remember,
                        shown,
                        field,
                        kind: None,
                    };
                    let found = p
                        .on_calls_thread(move || async move {
                            gw.ask_secret(&ctx, &target, ask).await.ok().flatten()
                        })
                        .await
                        .flatten()?;
                    Some(zeroize::Zeroizing::new(found.expose().to_string()))
                })
            },
        ));

        // hearing (L26) and recordings
        let redactor = p.redactor();
        let transcriber = Arc::new(ferb_sense::transcribe::Transcriber::new(
            ferb_daemon::areas_b::voice::voice_bundle(p.home(), opts.app_bundle.as_deref()),
            {
                let c = config.clone();
                move || match (
                    c.get("hearing", "requirement"),
                    c.get("voice", "requirement"),
                ) {
                    (Ok(Some(Value::String(s))), _) | (_, Ok(Some(Value::String(s))))
                        if !s.is_empty() =>
                    {
                        Some(s)
                    }
                    _ => None,
                }
            },
            move |argv| {
                if native_on {
                    ferb_sense::transcribe::launch(argv)
                } else {
                    Box::pin(async {
                        Err(std::io::Error::other(
                            "FERB.app isn't started by this daemon",
                        ))
                    })
                }
            },
            ferb_sense::transcribe::signed,
            Some(p.home().clone()),
        ));
        let capture: Arc<dyn ferb_mac_hands::listen::Capture> = if native_on {
            let mono = p.clock();
            Arc::new(native::audio::AppAudio {
                clock: Arc::new(move || mono.monotonic()),
            })
        } else {
            Arc::new(NoCapture)
        };
        let lookup = driver.clone();
        let mono = p.clock();
        let ids = p.ids();
        let hearing = ferb_mac_hands::listen::Hearing::new(
            p.home()
                .join("workspace")
                .join(ferb_mac_hands::listen::FOLDER),
            capture,
            Arc::new(Transcribe {
                transcriber,
                redactor: redactor.clone(),
            }),
            {
                let r = redactor.clone();
                Arc::new(move |t: &str| r.text(t))
            },
            Some(Arc::new(move |q: &str| {
                lookup.app(q).map(|a| (a.bundle_id, a.pid))
            })),
            Arc::new(move || mono.monotonic()),
            Arc::new(move || ids.new_id()),
        );
        let images = Arc::new(Images {
            redactor: redactor.clone(),
            native: native_on,
        });
        let img = images.clone();
        let recordings = ferb_mac_hands::recording::RecordingManager::new(
            Arc::new(move |jpeg: &[u8]| img.redact(jpeg).map_err(HandError::Withheld)),
            Arc::new(RecordingFiles {
                home: p.user_home().to_string_lossy().into_owned(),
                protection: p.shell_protection(),
            }),
            Some(hearing.clone()),
            Some(p.home().join("run").join("recordings")),
        );
        tools.recordings = Some(recordings.clone());
        Mac {
            tools: Arc::new(tools),
            hearing,
            recordings,
            images,
        }
    }
}

/// A future the Mac hands make that isn't `Send` (their capture paths borrow a paint callback that isn't `Sync`:
/// a seam mismatch, listed), run on a thread of its own driven by this runtime. Dropping the returned future (the
/// call cancelled, its task stopped) drops the inner one too. A plain thread, not the blocking pool: a runtime's
/// shutdown waits for its blocking pool, and these can be long (a first text recognition, a recording).
pub async fn off_thread<T, F>(make: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> std::pin::Pin<Box<dyn std::future::Future<Output = T>>> + Send + 'static,
{
    let rt = tokio::runtime::Handle::current();
    let (_held, mut gone) = tokio::sync::oneshot::channel::<()>();
    let (tx, rx) = tokio::sync::oneshot::channel::<T>();
    std::thread::Builder::new()
        .name("ferb-mac-call".into())
        .spawn(move || {
            rt.block_on(async move {
                tokio::select! {
                    r = make() => { let _ = tx.send(r); }
                    _ = &mut gone => {}
                }
            })
        })
        .ok()?;
    rx.await.ok()
}

fn hand(e: HandError) -> ToolError {
    ToolError {
        class: e.class().to_string(),
        message: e.to_string(),
        before_acting: !e.outcome_unknown(),
    }
}

/// The Mac tools in `add_mac`'s order.
pub const MAC_TOOLS: [&str; 12] = [
    "mac.apps",
    "mac.open",
    "mac.snapshot",
    "mac.menu",
    "mac.act",
    "mac.secret",
    "mac.applescript",
    "mac.screenshot",
    "mac.click",
    "mac.pointer",
    "mac.record",
    "mac.clipboard",
];

/// One Mac tool through the gateway's `Tool` trait.
pub struct MacTool {
    pub name: &'static str,
    pub tools: Arc<MacTools>,
    pub native: bool,
}

fn v(a: &Args) -> Value {
    Value::Object(a.clone())
}

impl Tool for MacTool {
    fn name(&self) -> &str {
        self.name
    }
    fn describe(&self, args: &Args) -> ActionSpec {
        self.tools.describe(self.name, &v(args))
    }
    fn precheck(&self, args: &Args) -> Option<String> {
        self.tools.precheck(self.name, &v(args))
    }
    fn unavailable<'a>(&'a self, args: &'a Args) -> BoxFuture<'a, Option<Map<String, Value>>> {
        Box::pin(async move {
            if self.name != "mac.applescript" {
                return None;
            }
            self.tools
                .unavailable(&v(args))
                .await
                .map(crate::basics::object)
        })
    }
    fn locks(&self, args: &Args) -> Vec<String> {
        self.tools.locks(self.name, &v(args))
    }
    fn file_paths(&self, args: &Args) -> Option<Vec<(String, String)>> {
        let out = self.tools.file_paths(self.name, &v(args));
        (!out.is_empty()).then(|| out.into_iter().map(|(p, a)| (p, a.to_string())).collect())
    }
    fn ledger_args(&self, args: &Args, hashed: &Args) -> Option<Args> {
        Some(self.tools.ledger_args(self.name, &v(args), hashed))
    }
    fn terminal_input<'a>(
        &'a self,
        args: &'a Args,
        task_id: &'a str,
    ) -> BoxFuture<'a, Option<TerminalInput>> {
        Box::pin(async move {
            self.tools
                .terminal_input(self.name, &v(args), task_id)
                .await
        })
    }
    fn browser_page<'a>(&'a self, args: &'a Args) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move { self.tools.browser_page(self.name, &v(args)).await })
    }
    fn preview_target(
        &self,
        args: &Args,
        _owner: &str,
        observed: Option<&Map<String, Value>>,
    ) -> Result<Option<PreviewTarget>, String> {
        Ok(self
            .tools
            .preview_target(self.name, &v(args), observed)
            .map(|t| PreviewTarget {
                scope: t.scope,
                label: t.label,
                capture: t
                    .resolve
                    .map(|r| Arc::new(r) as Arc<dyn std::any::Any + Send + Sync>),
            }))
    }
    fn point<'a>(
        &'a self,
        args: &'a Args,
        _grant: &'a ferb_rpc::auth::RunGrant,
    ) -> BoxFuture<'a, Result<Option<Point>, String>> {
        Box::pin(async move {
            Ok(self.tools.point(self.name, &v(args)).await.map(|p| Point {
                kind: p.kind.to_string(),
                rect: [p.rect.0, p.rect.1, p.rect.2, p.rect.3],
                display: i64::from(p.display),
            }))
        })
    }
    fn run<'a>(
        &'a self,
        args: &'a Args,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let a = v(args);
            if self.name == "mac.clipboard" {
                let board: Arc<dyn ferb_mac_hands::tools::Pasteboard> = if self.native {
                    Arc::new(ferb_mac_hands::native::pasteboard::NativePasteboard)
                } else {
                    return Err(ToolError::refused(
                        "this FERB was started without the Mac's own parts: it can't write the clipboard",
                    ));
                };
                let text = a.get("text").and_then(Value::as_str).unwrap_or("");
                return ferb_mac_hands::tools::clipboard_run(board, text, None, ctx)
                    .await
                    .map(Into::into)
                    .map_err(|e| ToolError::failed("RuntimeError", e));
            }
            let (tools, name, ctx) = (self.tools.clone(), self.name, ctx.clone());
            off_thread(move || Box::pin(async move { tools.run(name, &a, &ctx).await }))
                .await
                .unwrap_or_else(|| Err(HandError::Other("the Mac tool's thread ended".into())))
                .map(Into::into)
                .map_err(hand)
        })
    }
}

/// recording.stop / recording.inspect (recording.py `RecordingStop`, `RecordingInspect`).
pub struct RecordingTool {
    pub name: &'static str,
    pub recordings: Arc<ferb_mac_hands::recording::RecordingManager>,
    pub home: String,
}

impl Tool for RecordingTool {
    fn name(&self) -> &str {
        self.name
    }
    fn describe(&self, args: &Args) -> ActionSpec {
        if self.name == "recording.stop" {
            return ActionSpec::new("write_file");
        }
        let mut s = ActionSpec::new("read");
        s.target.path = args.get("path").and_then(Value::as_str).map(String::from);
        s
    }
    fn locks(&self, args: &Args) -> Vec<String> {
        match (self.name, args.get("id").and_then(Value::as_str)) {
            ("recording.stop", Some(id)) => vec![format!("recording:{id}")],
            _ => vec![],
        }
    }
    fn file_paths(&self, args: &Args) -> Option<Vec<(String, String)>> {
        if self.name == "recording.stop" {
            let id = args.get("id").and_then(Value::as_str).unwrap_or("");
            return Some(
                self.recordings
                    .path_of(id)
                    .map(|p| vec![(p.to_string_lossy().into_owned(), "write".to_string())])
                    .unwrap_or_default(),
            );
        }
        let path = args.get("path").and_then(Value::as_str).unwrap_or("");
        Some(vec![(path.to_string(), "read".to_string())])
    }
    fn run<'a>(
        &'a self,
        args: &'a Args,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            if self.name == "recording.stop" {
                let id = args
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let out = self
                    .recordings
                    .stop(&id, &ctx.task_id, None, true)
                    .await
                    .map_err(hand)?;
                let sources = self.recordings.sources_of(&id).await;
                let complete = out
                    .get("capture_complete")
                    .is_some_and(ferb_py::value::truthy);
                return Ok(ToolResult {
                    content: out,
                    is_error: !complete,
                    read_sources: sources,
                    ..ToolResult::default()
                }
                .into());
            }
            let raw = args.get("path").and_then(Value::as_str).unwrap_or("");
            let path = ferb_fs::files::resolve(raw, &ctx.workspace, &self.home, true).map_err(
                |e| match e {
                    ferb_fs::files::Fail::Refused(m) => ToolError::refused(m),
                    other => ToolError::failed("OSError", format!("{other:?}")),
                },
            )?;
            let times: Option<Vec<f64>> = args
                .get("times_s")
                .and_then(Value::as_array)
                .map(|t| t.iter().filter_map(Value::as_f64).collect());
            let path = std::path::PathBuf::from(path);
            let (info, image) = ferb_mac_hands::recording::inspect_video(
                &path,
                times.as_deref(),
                &ferb_mac_hands::native::paint::NativeSheet,
            )
            .await
            .map_err(hand)?;
            Ok(ToolResult {
                content: info,
                image: Some(image),
                read_sources: vec![(
                    "recording".into(),
                    ferb_mac_hands::recording::recording_id(&path),
                )],
                ..ToolResult::default()
            }
            .into())
        })
    }
}

/// The browsers' tabs as audio.listen hears them (HAND-207, HAND-229).
pub struct ListenTabs {
    pub driver: Arc<ferb_browser::driver::Driver>,
}

impl ferb_mac_hands::listen::TabSource for ListenTabs {
    fn tab<'a>(
        &'a self,
        tab: &'a str,
        task: &'a str,
    ) -> ferb_mac_hands::listen::BoxFut<
        'a,
        ferb_mac_hands::error::Result<(String, String, String, Option<i32>)>,
    > {
        Box::pin(async move {
            let t = self
                .driver
                .owned_by(tab, task, true)
                .map_err(|e| HandError::Refused(e.message()))?;
            let url = self.driver.tab_url(tab).unwrap_or_default();
            let browser = t.browser.clone();
            let bundle = ferb_browser::profiles::browser(&browser)
                .map(|b| b.bundle_id.to_string())
                .unwrap_or_default();
            Ok((url, browser, bundle, None))
        })
    }
    fn tab_sound<'a>(
        &'a self,
        _tab: &'a str,
        _task: &'a str,
    ) -> ferb_mac_hands::listen::BoxFut<'a, Map<String, Value>> {
        Box::pin(async { Map::new() })
    }
}

/// audio.listen.
pub struct ListenTool {
    pub hearing: Arc<ferb_mac_hands::listen::Hearing>,
    pub parts: Parts,
}

impl Tool for ListenTool {
    fn name(&self) -> &str {
        "audio.listen"
    }
    fn describe(&self, args: &Args) -> ActionSpec {
        ferb_mac_hands::listen::listen_describe(args.get("app").and_then(Value::as_str))
    }
    fn run<'a>(
        &'a self,
        args: &'a Args,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let driver = match ctx.identity {
                ferb_core::types::Identity::Ferb => self.parts.ferb_driver(),
                ferb_core::types::Identity::Owner => self.parts.owner_driver(),
            };
            let tabs = driver.map(|driver| ListenTabs { driver });
            ferb_mac_hands::listen::audio_listen(
                &self.hearing,
                tabs.as_ref()
                    .map(|t| t as &dyn ferb_mac_hands::listen::TabSource),
                &v(args),
                ctx,
            )
            .await
            .map(Into::into)
            .map_err(hand)
        })
    }
}

/// `add_mac`'s tools, recording.stop / recording.inspect, audio.listen.
pub fn mac_tools(p: &Parts) -> Vec<Arc<dyn Tool>> {
    let Some(m) = p.mac() else {
        return Vec::new();
    };
    MAC_TOOLS
        .iter()
        .map(|&name| {
            Arc::new(MacTool {
                name,
                tools: m.tools.clone(),
                native: m.images.native,
            }) as Arc<dyn Tool>
        })
        .collect()
}

pub fn recording_tool(p: &Parts, name: &'static str) -> Arc<dyn Tool> {
    let m = p.mac().expect("the Mac parts are built with the tools");
    Arc::new(RecordingTool {
        name,
        recordings: m.recordings.clone(),
        home: p.user_home().to_string_lossy().into_owned(),
    })
}

pub fn listen_tool(p: &Parts) -> Arc<dyn Tool> {
    let m = p.mac().expect("the Mac parts are built with the tools");
    Arc::new(ListenTool {
        hearing: m.hearing.clone(),
        parts: p.clone(),
    })
}

/// The evidence store (V1) as the gateway's hooks keep a call's image: `EvidenceStore(home / "evidence", ...)`.
pub struct EvidenceKeeper {
    pub parts: Parts,
}

impl EvidenceKeeper {
    fn store(&self) -> std::io::Result<ferb_mac_hands::evidence::EvidenceStore> {
        let p = &self.parts;
        let (clock, zoned, ids, r) = (p.clock(), p.zoned(), p.ids(), p.redactor());
        ferb_mac_hands::evidence::EvidenceStore::new(
            &p.home().join("evidence"),
            Arc::new(move || {
                let now = clock.now();
                let day = zoned
                    .zone()
                    .local(&now)
                    .date_naive()
                    .format("%Y-%m-%d")
                    .to_string();
                (ferb_core::when::isoformat(&now), day)
            }),
            Arc::new(move || ids.new_id()),
            Arc::new(move |v: &Value| r.value(v)),
            Arc::new(ferb_mac_hands::native::paint::NativePaint),
        )
    }
}

impl ferb_gateway::call_hooks::Evidence for EvidenceKeeper {
    fn put<'a>(
        &'a self,
        task: &'a str,
        span: &'a str,
        kind: &'a str,
        image: Option<&'a [u8]>,
        data: Map<String, Value>,
        origin: String,
    ) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let store = self.store().map_err(|_| "OSError".to_string())?;
            let db = self.parts.db();
            let d = crate::basics::lock(&db);
            store
                .put(
                    d.conn(),
                    task,
                    (!span.is_empty()).then_some(span),
                    kind,
                    image,
                    Some(data),
                    &[],
                    "image/jpeg",
                    &origin,
                )
                .map_err(|_| "OperationalError".to_string())
        })
    }
}

/// The real call hooks (`call_hooks.py`) over this daemon's parts: the image sanitizer, the evidence store and the
/// working app's first picture. Not composed in this build: the disturbance sentinel, the live previews and the
/// change record (their parts aren't wired), the shadow advisor.
pub fn hooks(p: &Parts) -> Arc<dyn ferb_gateway::seams::CallHooks> {
    let events = p.events();
    let journal = p.journal();
    let mac = p.mac();
    Arc::new(ferb_gateway::call_hooks::RealHooks::new(
        ferb_gateway::call_hooks::HookParts {
            sentinel: None,
            previews: None,
            evidence: Some(Arc::new(EvidenceKeeper { parts: p.clone() })),
            changes: None,
            images: mac
                .map(|m| m.images.clone() as Arc<dyn ferb_gateway::call_hooks::ImageSanitizer>),
            app_windows: mac.map(|m| {
                Arc::new(AppWindows(m.tools.clone()))
                    as Arc<dyn ferb_gateway::call_hooks::AppWindows>
            }),
            shadow: None,
            publish: Arc::new(move |topic: &str, data: Map<String, Value>| {
                events.publish(topic, data)
            }),
            journal: Arc::new(move |kind: &str, data: Map<String, Value>| {
                journal.note(kind, Value::Object(data))
            }),
            workspace: p.home().join("workspace").to_string_lossy().into_owned(),
            home: p.user_home().to_string_lossy().into_owned(),
        },
    ))
}
