//! FERB's Mac hands (`_build_mac_and_shell`'s Mac half, `add_mac`, `add_hearing`, `add_recording`): the Mac driver
//! composed over ferb-mac-hands' native layer (Accessibility, the window list and ScreenCaptureKit, CGEvents to one
//! process, the pasteboard, Launch Services), its 12 `mac.*` tools, `recording.*`, `audio.listen`, and the image
//! sanitizer every tool image passes (Vision's text recognition, then painting).
//!
//! Background first (rule 10): foreground input is available only while the owner-input tap runs and sensing isn't
//! paused, and yields whenever the owner is using keyboard or mouse; with the tap down FERB counts the owner as using
//! the Mac (SENSE-406) and only the background routes (Accessibility, events to one process, window capture) run.

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
                    // desktop.py:317 gives AppleScript only the apps and the events' socket
                    ..crate::off::off("applescript.sandbox_rest", Default::default())
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

tokio::task_local! {
    /// Set around a live preview's frame (`compose::previews`): the frame's faults are put here for preview.failed,
    /// and its image doesn't queue behind another image's child process (HAND-531).
    pub static PREVIEW_FAULTS: Arc<std::sync::Mutex<Vec<ferb_mac_hands::ocr::Fault>>>;
}

/// One text-reading child process at a time in this process (HAND-531).
fn child_gate() -> Arc<std::sync::Mutex<()>> {
    static GATE: std::sync::OnceLock<Arc<std::sync::Mutex<()>>> = std::sync::OnceLock::new();
    GATE.get_or_init(Default::default).clone()
}

/// The program that reads an image's text in a fresh process (HAND-531): inside FERB.app its
/// `Contents/Resources/bin/ferb`, ENG-7's program for internal commands (it holds no permission, and Vision needs
/// none); elsewhere the `ferb` built next to this executable. None when there is no such file.
pub fn child_program() -> Option<std::path::PathBuf> {
    let path = match crate::home::app_bundle() {
        Some(app) => app.join(ferb_device::install::BUNDLE_CLI),
        None => crate::host::real_exe()?.with_file_name("ferb"),
    };
    path.is_file().then_some(path)
}

impl Images {
    /// The recognizers in the order they're tried (HAND-531): Vision in this process as it chooses its devices, again
    /// with every stage on the CPU, then FERB's own program in a fresh process (one at a time; `wait` says whether
    /// this image waits for a child already running; None: no child).
    pub fn recognizers(child: Option<bool>) -> Vec<ferb_mac_hands::ocr::Attempt> {
        use ferb_mac_hands::native::vision::{CpuVision, NativeVision};
        use ferb_mac_hands::ocr::Attempt;
        let mut out = vec![
            Attempt {
                step: "vision",
                recognize: Arc::new(NativeVision),
            },
            Attempt {
                step: "vision_cpu",
                recognize: Arc::new(CpuVision),
            },
        ];
        if let (Some(wait), Some(program)) = (child, child_program()) {
            out.push(Attempt {
                step: "child",
                recognize: Arc::new(ferb_mac_hands::vision_child::ChildVision {
                    program,
                    args: vec![ferb_mac_hands::vision_child::COMMAND.to_string()],
                    timeout: ferb_mac_hands::vision_child::CHILD_TIMEOUT,
                    gate: child_gate(),
                    wait,
                }),
            });
        }
        out
    }

    pub fn redact(&self, image: &[u8]) -> Result<Vec<u8>, String> {
        self.redact_noted(image, Some(true))
            .map(|r| r.image)
            .map_err(|w| w.to_string())
    }

    /// The masked image with the attempts that failed before one read it, or every fault (HAND-531). A daemon
    /// without the Mac's parts reads nothing: withheld with no fault.
    pub fn redact_noted(
        &self,
        image: &[u8],
        child: Option<bool>,
    ) -> Result<ferb_mac_hands::ocr::Redacted, ferb_mac_hands::ocr::Withheld> {
        if !self.native {
            return Err(ferb_mac_hands::ocr::Withheld::default());
        }
        ferb_mac_hands::ocr::ImageRedactor {
            redactor: self.redactor.clone(),
            recognize: Self::recognizers(child),
            paint: Arc::new(ferb_mac_hands::native::paint::NativePaint),
        }
        .redact_noted(image, &[])
    }
}

/// `f` on a plain thread of its own, its answer awaited: the first recognition can take minutes, and a shutdown
/// never waits for it. Without a thread, `none` is the answer.
fn on_image_thread<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
    none: T,
) -> BoxFuture<'static, T> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("ferb-image-redact".into())
        .spawn(move || {
            let _ = tx.send(f());
        });
    Box::pin(async move {
        if spawned.is_err() {
            return none;
        }
        rx.await.unwrap_or(none)
    })
}

impl ferb_gateway::call_hooks::ImageSanitizer for Images {
    fn sanitize<'a>(&'a self, image: &'a [u8]) -> BoxFuture<'a, Result<Vec<u8>, String>> {
        let image = image.to_vec();
        let me = Images {
            redactor: self.redactor.clone(),
            native: self.native,
        };
        on_image_thread(
            move || me.redact(&image),
            Err(ferb_mac_hands::ocr::UNAVAILABLE.to_string()),
        )
    }
}

/// The Mac's image sanitizer: [`Images`], each image that needed more than Vision's first attempt journaled
/// (HAND-531): `image.redaction_failed` when it was withheld, `image.redaction_recovered` when a later attempt read
/// it; each with what the image was for (`use`) and the faults (step, error domain and code), never the image or its
/// text. Bounded like the previews' notes: the first row at once, then one per [`NOTE_EVERY_S`] per kind, use and
/// faults, with the repeats between.
#[derive(Clone)]
pub struct MacImages {
    pub native: bool,
    images: Arc<Images>,
    journal: Arc<crate::basics::Journal>,
    monotonic: Arc<dyn Fn() -> f64 + Send + Sync>,
    noted: Arc<std::sync::Mutex<std::collections::HashMap<String, (f64, u64)>>>,
}

/// One row per kind, use and faults within this many seconds (the previews' bound, task_preview `NOTE_EVERY_S`).
pub const NOTE_EVERY_S: f64 = ferb_daemon::areas_b::task_preview::NOTE_EVERY_S;

impl MacImages {
    pub fn new(
        images: Images,
        journal: Arc<crate::basics::Journal>,
        monotonic: Arc<dyn Fn() -> f64 + Send + Sync>,
    ) -> MacImages {
        MacImages {
            native: images.native,
            images: Arc::new(images),
            journal,
            monotonic,
            noted: Default::default(),
        }
    }

    /// An image for `use_` (the front window, a recording's frame), waiting for a child process when it needs one.
    pub fn redact(&self, image: &[u8], use_: &str) -> Result<Vec<u8>, String> {
        let got = self.images.redact_noted(image, Some(true));
        self.note(use_, &got);
        got.map(|r| r.image).map_err(|w| w.to_string())
    }

    /// Reads a blank picture with this process's own attempts (no child: its work would be thrown away), journaled
    /// like an image (`use` warm). Meant for when the Mac comes back into use: a recognizer that stopped working is
    /// found then, and the CPU one is loaded, before the next picture needs it.
    pub fn warm(&self) {
        let blank = ferb_mac_hands::native::vision::blank_png(160, 48);
        let got = self.images.redact_noted(&blank, None);
        self.note("warm", &got);
    }

    /// The journal rows for one image (HAND-531).
    pub fn note(
        &self,
        use_: &str,
        got: &Result<ferb_mac_hands::ocr::Redacted, ferb_mac_hands::ocr::Withheld>,
    ) {
        let (kind, faults, read_by) = match got {
            Ok(r) if r.faults.is_empty() => return,
            Ok(r) => ("image.redaction_recovered", &r.faults, Some(r.read_by)),
            // nothing was tried (a daemon without the Mac's parts): nothing to record
            Err(w) if w.faults.is_empty() => return,
            Err(w) => ("image.redaction_failed", &w.faults, None),
        };
        let facts: Vec<Value> = faults.iter().map(|f| f.facts()).collect();
        let key = format!("{kind}:{use_}:{}", Value::Array(facts.clone()));
        let now = (self.monotonic)();
        let since = {
            let mut noted = self.noted.lock().unwrap_or_else(|e| e.into_inner());
            let (due, since) = noted.get(&key).copied().unwrap_or((f64::MIN, 0));
            if now < due {
                noted.insert(key, (due, since + 1));
                return;
            }
            noted.insert(key, (now + NOTE_EVERY_S, 0));
            since
        };
        let mut data = Map::new();
        data.insert("use".into(), json!(use_));
        data.insert("faults".into(), Value::Array(facts));
        if let Some(step) = read_by {
            data.insert("read_by".into(), json!(step));
        }
        if since != 0 {
            data.insert("repeats".into(), json!(since));
        }
        self.journal.note(kind, Value::Object(data));
    }
}

/// HAND-548: what a tool call's span keeps of one image's redaction: each attempt that failed (step, error domain and
/// code) and `read_by` (a later attempt read it) or `withheld`; empty when the first attempt read it, or nothing was
/// tried. Never the image or its text.
pub fn span_notes(
    got: &Result<ferb_mac_hands::ocr::Redacted, ferb_mac_hands::ocr::Withheld>,
) -> Map<String, Value> {
    let (faults, last) = match got {
        Ok(r) => (&r.faults, ("read_by", json!(r.read_by))),
        Err(w) => (&w.faults, ("withheld", json!(true))),
    };
    let mut out = Map::new();
    if faults.is_empty() {
        return out;
    }
    out.insert(
        "faults".into(),
        Value::Array(faults.iter().map(|f| f.facts()).collect()),
    );
    out.insert(last.0.into(), last.1);
    out
}

impl ferb_gateway::call_hooks::ImageSanitizer for MacImages {
    fn sanitize<'a>(&'a self, image: &'a [u8]) -> BoxFuture<'a, Result<Vec<u8>, String>> {
        Box::pin(async move { self.sanitize_noted(image).await.0 })
    }

    fn sanitize_noted<'a>(
        &'a self,
        image: &'a [u8],
    ) -> BoxFuture<'a, ferb_gateway::call_hooks::SanitizedNoted> {
        let image = image.to_vec();
        let me = self.clone();
        // a live preview's frame: its faults go to preview.failed, and it never queues behind a running child
        let preview = PREVIEW_FAULTS.try_with(|f| f.clone()).ok();
        on_image_thread(
            move || {
                let got = me.images.redact_noted(&image, Some(preview.is_none()));
                me.note(if preview.is_some() { "preview" } else { "tool" }, &got);
                let notes = span_notes(&got);
                let answer = match got {
                    Ok(r) => Ok(r.image),
                    Err(w) => {
                        if let Some(sink) = preview {
                            sink.lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .extend(w.faults.iter().cloned());
                        }
                        Err(w.to_string())
                    }
                };
                (answer, notes)
            },
            (
                Err(ferb_mac_hands::ocr::UNAVAILABLE.to_string()),
                Map::new(),
            ),
        )
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
    /// FERB.app's speech recognition (`hearing.transcriber`: doctor's and setup's status probe).
    pub transcriber: Arc<ferb_sense::transcribe::Transcriber>,
    pub recordings: Arc<ferb_mac_hands::recording::RecordingManager>,
    pub images: Arc<MacImages>,
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
        // SENSE-406 (desktop.py `_build_mac_and_shell`): with the owner-input tap down (paused, Input Monitoring
        // missing, macOS refused the tap) FERB counts the owner as using the Mac: no foreground input
        let tap = p.owner().map(|o| o.input.clone());
        let paused = {
            let marker = p.home().join("PAUSED");
            move || marker.exists()
        };
        let owner_active: Arc<dyn Fn() -> bool + Send + Sync> = match tap.clone() {
            Some(t) => Arc::new(move || t.active() || !t.is_running()),
            None => Arc::new(|| true),
        };
        let mut input = ferb_mac_hands::input::RealInput::new(
            owner_active,
            ferb_mac_hands::locks::GuiLock::new(),
        );
        input.available = match tap {
            Some(t) => {
                let paused = paused.clone();
                Arc::new(move || !paused() && t.is_running())
            }
            // the Mac's parts are off: no owner-input tap, so no foreground input
            None => crate::off::off("input.no_tap", Arc::new(|| false)),
        };
        input.why_unavailable = Arc::new(move || {
            if paused() {
                ferb_mac_hands::input::PAUSED.to_string()
            } else {
                ferb_mac_hands::input::UNMONITORED.to_string()
            }
        });
        // desktop.py:278 `owner_wait`: the owner's time never counts as a stall (the supervisor, once built)
        input.on_wait = owner_wait(p.downgrade());
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
            // one monotonic origin with the previews' clock (a capture's deadline means the same instant to both)
            time: Arc::new(ferb_mac_hands::driver::RealTime(
                crate::compose::previews::epoch(),
            )),
        });
        let c = config.clone();
        d.excluded = Arc::new(move || {
            c.exclude_apps()
                .map(|s| s.into_iter().collect())
                .unwrap_or_default()
        });
        // one instance per folder: the gateway's raw-input question reads the records the driver writes (HAND-554)
        d.profiles =
            ferb_mac_hands::profile::Profiles::shared(&p.home().join("device").join("apps"));
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
        // desktop.py:290 `web_secrets=self._web_secrets` (L13): a browser window's capture asks its paired profiles
        d.web_secrets = Some(web_secrets(p.downgrade()));
        // sensing.py:58: FERB's app switches and its actions in an app go to the daemon's one attribution
        let a = p.attribution();
        d.on_activate =
            Arc::new(move |bundle: &str, task: Option<&str>| a.ferb_activated(bundle, task));
        // HAND-549, HAND-554: an app that came to the front during a Mac call is told to the owner on the same path
        // as a browser window that came up (HAND-546's notice, `browser.window_shown`, no site)
        let events = p.events();
        d.came_forward = Some(Arc::new(move |task: &str, shown: Value| {
            let mut data = Map::new();
            data.insert("task_id".into(), Value::String(task.to_string()));
            data.insert("site".into(), Value::String(String::new()));
            data.insert("shown".into(), Value::Array(vec![shown]));
            events.publish("browser.window_shown", data);
        }));
        let a = p.attribution();
        d.acting = Some(Arc::new(move |bundle: &str, task: Option<&str>| {
            Box::new(a.acting_owned(bundle, task)) as Box<dyn std::any::Any + Send>
        }));
        let driver = Arc::new(d);

        let paint: ferb_mac_hands::driver::PaintFn = Arc::new(|jpeg, masks, bounds, size| {
            ferb_mac_hands::driver::painted(&native::paint::NativePaint, jpeg, masks, bounds, size)
        });
        let mut tools = MacTools::new(driver.clone(), paint);
        tools.home = p.user_home().to_string_lossy().into_owned();
        tools.cwd = p.home().join("workspace").to_string_lossy().into_owned();
        // desktop.py:317,322: AppleScript runs under the shell's own sandbox (its profiles folder swept at start, the
        // daemon's socket named), and only when that sandbox can run (else mac.applescript says it isn't available)
        let contained = opts.egress.containment.get().is_some_and(|c| c.0);
        if let (true, Some(shells)) = (contained, p.shells()) {
            tools.sandbox = Some(Arc::new(ScriptSandbox(shells.sandbox().clone())));
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
            crate::off::off("hearing.no_capture", Arc::new(NoCapture))
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
                transcriber: transcriber.clone(),
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
        let clock = p.clock();
        let images = Arc::new(MacImages::new(
            Images {
                redactor: redactor.clone(),
                native: native_on,
            },
            p.journal(),
            Arc::new(move || clock.monotonic()),
        ));
        let img = images.clone();
        let recordings = ferb_mac_hands::recording::RecordingManager::new(
            Arc::new(move |jpeg: &[u8]| img.redact(jpeg, "recording").map_err(HandError::Withheld)),
            Arc::new(RecordingFiles {
                home: p.user_home().to_string_lossy().into_owned(),
                protection: p.shell_protection(),
            }),
            Some(hearing.clone()),
            Some(p.home().join("run").join("recordings")),
        );
        tools.recordings = Some(recordings.clone());
        sweep(&hearing, &recordings, &p.journal());
        Mac {
            tools: Arc::new(tools),
            hearing,
            transcriber,
            recordings,
            images,
        }
    }
}

/// At the daemon's start, before any listen or recording: main.py:432 `recordings.sweep` (frames a restart cut off
/// belong to no recording now; a failure is journaled, never stops the start) and desktop.py:324 `hearing.sweep`
/// (sound and answers a restart left behind, L26).
pub fn sweep(
    hearing: &ferb_mac_hands::listen::Hearing,
    recordings: &ferb_mac_hands::recording::RecordingManager,
    journal: &crate::basics::Journal,
) {
    if let Err(e) = recordings.sweep() {
        journal.note(
            "recordings.startup_failed",
            json!({"error": ferb_shell::py_os_error(&e)}),
        );
    }
    hearing.sweep();
}

/// desktop.py:278 `owner_wait`: while a call waits for the owner (foreground input yielding to them), the worker
/// counts as waiting, never stalled (`supervisor.activity.waiting`); nothing before the supervisor is built.
pub fn owner_wait(parts: super::WeakParts) -> OnWait {
    Arc::new(move |holder: &str, on: bool| {
        if let Some(p) = parts.upgrade() {
            if let Some(s) = p.supervisor() {
                s.worker_waiting(holder, on);
            }
        }
    })
}

/// RealInput's `on_wait`: (holder, waiting).
pub type OnWait = Arc<dyn Fn(&str, bool) + Send + Sync>;

/// A browser window's mask state, asked of its paired profiles.
type WebSecretsFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = Option<ferb_mac_hands::mask::WebSecrets>> + Send>,
>;

/// browsers.py:115 `_web_secrets` (L13) through the browser area, once group B builds it; before that no paired
/// profile can answer (Python's `hub` is None then: the same words).
pub fn web_secrets(parts: super::WeakParts) -> ferb_mac_hands::driver::WebSecretsFn {
    Arc::new(
        move |browser: String,
              bounds: ferb_mac_hands::ax::Rect,
              title: String|
              -> WebSecretsFuture {
            let area = parts
                .upgrade()
                .and_then(|p| p.browsers().and_then(|b| b.area()));
            Box::pin(async move {
                Some(match area {
                    Some(cx) => cx.web_secrets(&browser, bounds, &title).await,
                    None => ferb_mac_hands::mask::WebSecrets {
                        state: None,
                        why: format!("no paired profile of {browser} is connected"),
                        late: false,
                        tab: None,
                    },
                })
            })
        },
    )
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
    /// HAND-582, HAND-583: the browser area, for a web page a paired profile's browser tools can take.
    pub parts: Option<super::WeakParts>,
}

/// HAND-582, HAND-583: the fact that refuses a Mac call on a web page a paired, connected profile's browser tools can
/// take (before any question, nothing done): what the page is and the route the worker should use instead.
pub fn web_route_fact(
    target: &ferb_mac_hands::tools::WebTarget,
    route: &ferb_daemon::areas_b::browsers::PageRoute,
) -> Map<String, Value> {
    let named = |(b, f, label): &(String, String, String)| format!("{label} ({b}/{f})");
    let profiles: Vec<String> = route.profiles.iter().map(named).collect();
    let ask = "If this task isn't allowed that profile, the refusal of browser.open says so: then ask the owner with \
               owner.ask (naming the profile, options Yes and No) whether the task should use it. Nothing was done \
               and the owner wasn't asked";
    let detail = if target.opens {
        format!(
            "Open web pages with browser.open, not mac.open: it opens the page in a paired profile ({}) in the \
             background, while mac.open would bring the browser to the front. {ask}.",
            profiles.join(", ")
        )
    } else {
        let tab = route
            .tab_ref
            .as_deref()
            .map(|t| format!(": browser.use_tab tab_ref={t} takes this tab, then browser.snapshot and browser.act"))
            .unwrap_or_default();
        format!(
            "This is a web page in {}, where FERB's extension is connected, so it belongs to the browser tools, \
             which work in the background with the window minimized{tab}. The Mac tools would need the browser's \
             window in front. {ask}.",
            profiles.join(", ")
        )
    };
    let mut out = Map::new();
    out.insert("code".into(), json!("use_browser_tools"));
    out.insert(
        "profiles".into(),
        json!(route
            .profiles
            .iter()
            .map(|(b, f, _)| format!("{b}/{f}"))
            .collect::<Vec<_>>()),
    );
    if let Some(t) = &route.tab_ref {
        out.insert("tab_ref".into(), json!(t));
    }
    out.insert("performed".into(), json!(false));
    out.insert("asked_owner".into(), json!(false));
    out.insert("detail".into(), json!(detail));
    out
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
            if matches!(
                self.name,
                "mac.act" | "mac.click" | "mac.pointer" | "mac.open"
            ) {
                // HAND-582, HAND-583: a web page a paired, connected profile's browser tools can take
                let target = self.tools.web_target(self.name, &v(args)).await?;
                let area = self
                    .parts
                    .as_ref()?
                    .upgrade()
                    .and_then(|p| p.browsers().and_then(|b| b.area()))?;
                let route = area
                    .page_route(&target.browser, &target.url, target.opens)
                    .await?;
                return Some(web_route_fact(&target, &route));
            }
            if self.name != "mac.applescript" {
                return None;
            }
            self.tools
                .unavailable(&v(args))
                .await
                .map(crate::basics::object)
        })
    }
    fn window_reason<'a>(&'a self, args: &'a Args) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move { self.tools.window_reason(self.name, &v(args)).await })
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
    /// GATE-579 (pay_picture.py:79-84; the gateway holds the call's locks and the time limit): a Mac app window's
    /// payment picture is the live preview's capture of the call's window, used as it is; none without a surface.
    fn pay_picture<'a>(
        &'a self,
        args: &'a Args,
        _owner: &'a str,
    ) -> BoxFuture<'a, Option<Vec<u8>>> {
        Box::pin(async move {
            let resolve = self
                .tools
                .preview_target(self.name, &v(args), None)?
                .resolve?;
            let take = async move {
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
                // perception's `bind_clipboard` (sensing.py:66): FERB's own write isn't read back as the owner's copy
                let mark = |count: i64, task: &str| {
                    ferb_sense::capture::mark_clipboard_write(count, Some(task))
                };
                return ferb_mac_hands::tools::clipboard_run(board, text, Some(&mark), ctx)
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
            // listen.py:246: that browser's own process, when the driver knows it
            let exact = self
                .driver
                .browser_pid(&t)
                .map_err(|e| HandError::Refused(e.message()))?;
            Ok((url, browser, bundle, exact))
        })
    }
    /// listen.py:253 (HAND-207): a tab FERB opened is muted; the task needs to hear this one, so it is unmuted for
    /// the rest of the task (the owner's own tab keeps its setting). The tab was just checked: one gone since is
    /// said as the sound's error (this seam has no failure of its own).
    fn tab_sound<'a>(
        &'a self,
        tab: &'a str,
        task: &'a str,
    ) -> ferb_mac_hands::listen::BoxFut<'a, Map<String, Value>> {
        Box::pin(async move {
            let sound = match self.driver.owned_by(tab, task, true) {
                Ok(t) => self.driver.tab_sound(&t, true).await,
                Err(e) => Err(e),
            };
            sound.unwrap_or_else(|e| {
                let mut out = Map::new();
                out.insert("tab_sound_error".into(), json!(e.message()));
                out
            })
        })
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
                parts: Some(p.downgrade()),
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

/// The real call hooks (`call_hooks.py`) over this daemon's parts: the image sanitizer, the evidence store, the
/// working app's first picture, the disturbance sentinel, the change record and the live previews. Not composed: the
/// shadow advisor (D23, off).
pub fn hooks(p: &Parts) -> Arc<dyn ferb_gateway::seams::CallHooks> {
    let events = p.events();
    let journal = p.journal();
    let mac = p.mac();
    Arc::new(ferb_gateway::call_hooks::RealHooks::new(
        ferb_gateway::call_hooks::HookParts {
            sentinel: p.sentinel().map(|s| {
                Arc::new(crate::compose::desktop::GatewaySentinel(s))
                    as Arc<dyn ferb_gateway::call_hooks::Sentinel>
            }),
            previews: Some(Arc::new(crate::compose::previews::GatewayPreviews(
                p.previews().clone(),
            ))
                as Arc<dyn ferb_gateway::call_hooks::Previews>),
            evidence: Some(Arc::new(EvidenceKeeper { parts: p.clone() })),
            changes: Some(Arc::new(super::ChangeHook(p.changes()))),
            images: mac
                .map(|m| m.images.clone() as Arc<dyn ferb_gateway::call_hooks::ImageSanitizer>),
            app_windows: mac.map(|m| {
                Arc::new(AppWindows(m.tools.clone()))
                    as Arc<dyn ferb_gateway::call_hooks::AppWindows>
            }),
            // Jev's shadow advice: Jev is off
            shadow: crate::off::off("hooks.shadow", None),
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

#[cfg(test)]
mod tests {
    use super::*;

    struct NoWords;
    impl ferb_mac_hands::listen::Transcribe for NoWords {
        fn transcribe<'a>(
            &'a self,
            _path: &'a std::path::Path,
            _language: Option<&'a str>,
            _audio_s: f64,
            _offset: f64,
        ) -> ferb_mac_hands::listen::BoxFut<'a, ferb_mac_hands::error::Result<Map<String, Value>>>
        {
            Box::pin(async { Ok(Map::new()) })
        }
    }

    fn journal(dir: &std::path::Path) -> crate::basics::Journal {
        let db = crate::basics::open_store(&dir.join("ferb.db")).unwrap();
        crate::basics::Journal {
            db: Arc::new(std::sync::Mutex::new(db)),
            clock: Arc::new(ferb_clock::clock::SystemClock::new(
                ferb_clock::zone::Zone::utc(),
            )),
            ids: Arc::new(ferb_core::ids::SequentialIds::new("j")),
            redactor: Arc::new(ferb_sanitize::redact::Redactor::new(&[], &[])),
        }
    }

    /// main.py:432 and desktop.py:324 (L26): at start a restart's leftover sound files and recording frames go; a
    /// recording folder that can't be removed is journaled (`recordings.startup_failed`) and the start goes on.
    // decision: HAND-207
    #[test]
    fn the_start_sweeps_what_a_restart_left() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let listen = tmp.path().join("workspace").join("listen");
        let staging = tmp.path().join("run").join("recordings");
        std::fs::create_dir_all(&listen).unwrap();
        std::fs::create_dir_all(staging.join("rec-old")).unwrap();
        std::fs::write(listen.join("listen-1.caf"), b"x").unwrap();
        std::fs::write(staging.join("rec-old").join("f1.jpg"), b"x").unwrap();
        std::fs::write(staging.join("other.txt"), b"x").unwrap();
        let hearing = ferb_mac_hands::listen::Hearing::new(
            listen.clone(),
            Arc::new(NoCapture),
            Arc::new(NoWords),
            Arc::new(|t: &str| t.to_string()),
            None,
            Arc::new(|| 0.0),
            Arc::new(|| "id".to_string()),
        );
        let recordings = ferb_mac_hands::recording::RecordingManager::new(
            Arc::new(|b: &[u8]| Ok(b.to_vec())),
            Arc::new(RecordingFiles {
                home: tmp.path().to_string_lossy().into_owned(),
                protection: None,
            }),
            Some(hearing.clone()),
            Some(staging.clone()),
        );
        let j = journal(tmp.path());
        sweep(&hearing, &recordings, &j);
        assert_eq!(std::fs::read_dir(&listen).unwrap().count(), 0);
        assert!(!staging.join("rec-old").exists());
        assert!(
            staging.join("other.txt").exists(),
            "only recordings' frames"
        );
        assert!(j.of_kind("recordings.startup_failed", "", "").is_empty());
        // a frame folder that can't be removed: journaled, never a failed start
        std::fs::create_dir_all(staging.join("rec-stuck")).unwrap();
        std::fs::write(staging.join("rec-stuck").join("f1.jpg"), b"x").unwrap();
        std::fs::set_permissions(
            staging.join("rec-stuck"),
            std::fs::Permissions::from_mode(0o500),
        )
        .unwrap();
        sweep(&hearing, &recordings, &j);
        std::fs::set_permissions(
            staging.join("rec-stuck"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let rows = j.of_kind("recordings.startup_failed", "", "");
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(
            rows[0].1["error"]
                .as_str()
                .unwrap()
                .starts_with("PermissionError: [Errno 13]"),
            "{rows:?}"
        );
    }

    /// sensing.py:58 `mac.acting = a.acting`: while a FERB action runs in an app, the attribution knows it; the
    /// driver's owned value ends it when dropped.
    // decision: DMN-501
    #[test]
    fn an_action_in_an_app_is_attributed_while_it_runs() {
        let a = Arc::new(ferb_sense::attribution::Attribution::new(|| 0.0));
        let guard = a.acting_owned("com.example.app", Some("t1"));
        assert_eq!(
            a.acting_now().get("com.example.app"),
            Some(&vec![Some("t1".to_string())])
        );
        drop(guard);
        assert!(a.acting_now().is_empty());
    }

    /// browsers.py:126: before any paired profile can answer (the browser area not built), a browser window's
    /// capture is told so in Python's words; the owner's waits are nothing before the supervisor exists.
    // decision: HAND-518
    #[tokio::test]
    async fn web_secrets_before_the_browsers_say_no_profile_is_connected() {
        let gone = super::super::WeakParts(std::sync::Weak::new());
        let ask = web_secrets(gone.clone());
        let got = ask("chrome".into(), (0.0, 0.0, 10.0, 10.0), "t".into())
            .await
            .unwrap();
        assert!(got.state.is_none());
        assert_eq!(got.why, "no paired profile of chrome is connected");
        assert!(!got.late);
        owner_wait(gone)("w1", true);
    }

    fn fault(step: &str, domain: &str, code: i64) -> ferb_mac_hands::ocr::Fault {
        let mut f = ferb_mac_hands::ocr::Fault::new(domain, code);
        f.step = step.into();
        f
    }

    /// HAND-531: an image withheld after every attempt, or read only by a later one, is journaled with what it was
    /// for and each fault's step, domain and code (never the image or its text), at most once per kind, use and
    /// faults in NOTE_EVERY_S, the rows between counted.
    // decision: HAND-531
    #[test]
    fn an_image_that_needed_more_than_the_first_attempt_is_journaled_bounded_and_without_content() {
        use ferb_mac_hands::ocr::{Redacted, Withheld};
        let tmp = tempfile::tempdir().unwrap();
        let j = Arc::new(journal(tmp.path()));
        let now = Arc::new(std::sync::Mutex::new(100.0));
        let t = now.clone();
        let images = MacImages::new(
            Images {
                redactor: Arc::new(ferb_sanitize::redact::Redactor::new(&[], &[])),
                native: false,
            },
            j.clone(),
            Arc::new(move || *t.lock().unwrap()),
        );
        let withheld = || -> Result<Redacted, Withheld> {
            Err(Withheld {
                faults: vec![
                    fault("vision", "com.apple.Vision", 9),
                    fault("vision_cpu", "com.apple.Vision", 9),
                    fault("child", ferb_mac_hands::ocr::domain::TIMEOUT, 150),
                ],
            })
        };
        images.note("tool", &withheld());
        images.note("tool", &withheld());
        images.note("tool", &withheld());
        // the first attempt read it: nothing to record; a daemon that tried nothing: nothing either
        let read = |faults| -> Result<Redacted, Withheld> {
            Ok(Redacted {
                image: b"Your verification code is 482913".to_vec(),
                read_by: "vision",
                faults,
            })
        };
        images.note("tool", &read(vec![]));
        images.note("tool", &Err(Withheld::default()));
        let rows = j.of_kind("image.redaction_failed", "", "");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].1,
            json!({"use": "tool", "faults": [
                {"step": "vision", "domain": "com.apple.Vision", "code": 9},
                {"step": "vision_cpu", "domain": "com.apple.Vision", "code": 9},
                {"step": "child", "domain": "ferb.timeout", "code": 150}]})
        );
        // a later window: one row, with the two it held back
        *now.lock().unwrap() += NOTE_EVERY_S;
        images.note("tool", &withheld());
        let rows = j.of_kind("image.redaction_failed", "", "");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].1["repeats"], json!(2));
        // read by the CPU after Vision failed: recovered, with the attempt that read it; never the image or its text
        let mut cpu = read(vec![fault("vision", "com.apple.Vision", 9)]);
        if let Ok(r) = cpu.as_mut() {
            r.read_by = "vision_cpu";
        }
        images.note("preview", &cpu);
        let rec = j.of_kind("image.redaction_recovered", "", "");
        assert_eq!(
            rec.iter().map(|r| r.1.clone()).collect::<Vec<_>>(),
            [json!({"use": "preview", "read_by": "vision_cpu",
                    "faults": [{"step": "vision", "domain": "com.apple.Vision", "code": 9}]})]
        );
        let all = crate::basics::lock(&j.db)
            .conn()
            .query_row("SELECT group_concat(data_json) FROM journal", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap();
        assert!(
            !all.contains("482913") && !all.contains("verification"),
            "{all}"
        );
    }

    /// HAND-548: the span's notes of one image: the faults and how it ended; nothing when the first attempt read it.
    #[test]
    fn an_images_span_notes_are_its_faults_and_how_it_ended() {
        use ferb_mac_hands::ocr::{Redacted, Withheld};
        let first: Result<Redacted, Withheld> = Ok(Redacted {
            image: b"Your verification code is 482913".to_vec(),
            read_by: "vision",
            faults: vec![],
        });
        assert!(span_notes(&first).is_empty());
        assert!(span_notes(&Err(Withheld::default())).is_empty());
        let cpu: Result<Redacted, Withheld> = Ok(Redacted {
            image: b"Your verification code is 482913".to_vec(),
            read_by: "vision_cpu",
            faults: vec![fault("vision", "com.apple.Vision", 9)],
        });
        let notes = Value::Object(span_notes(&cpu));
        assert_eq!(
            notes,
            json!({"faults": [{"step": "vision", "domain": "com.apple.Vision", "code": 9}],
                   "read_by": "vision_cpu"})
        );
        assert!(!notes.to_string().contains("482913"));
        let withheld: Result<Redacted, Withheld> = Err(Withheld {
            faults: vec![fault("vision", "com.apple.Vision", 9)],
        });
        assert_eq!(
            Value::Object(span_notes(&withheld)),
            json!({"faults": [{"step": "vision", "domain": "com.apple.Vision", "code": 9}],
                   "withheld": true})
        );
    }
}
