//! What happens around a tool call without deciding whether it may run (`ferb/daemon/call_hooks.py`; kb area
//! gateway: "non-authorization features out of the authorization path"): the task's live preview and FERB's cursor
//! point before it, the desktop samples around a call that can change the screen (DMN-5), the change record's copies
//! of the files it names (L39), the working app's first picture, image sanitizing and the evidence store (V1, D26,
//! L13), the advisory shadow check (D23, shadow only). Each hook only records or shows; a failing hook never stops or
//! changes the call's authorization, and none runs before the owner's questions are answered.
//!
//! Every part it drives belongs to another part of FERB and comes in as a seam: the disturbance sentinel, the live
//! previews, the evidence store, the change record, the image sanitizer, the app-window picture and its masking,
//! the shadow advisor, and where events and journaled problems go.
//!
//! decision: DMN-5, DMN-403, HAND-212

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_core::interfaces::BoxFuture;
use ferb_core::types::ActionSpec;
use ferb_policy::rules::Kind;
use ferb_rpc::auth::RunGrant;
use ferb_sanitize::origin;
use serde_json::{json, Map, Value};

use crate::seams::CallHooks;
use crate::tool::{Args, PreviewTarget, Shot, Tool, ToolOutput};

/// L39: the kinds of call that can change the owner's files, whose named files the change record copies first.
pub const CHANGE_KINDS: [&str; 5] = [
    Kind::WRITE_FILE,
    Kind::TRASH,
    Kind::DELETE_PERMANENT,
    Kind::RUN_SHELL,
    Kind::APPLESCRIPT,
];

/// `ferb/act/point.py` `POINT_TIMEOUT_S`: a point that takes longer is skipped.
pub const POINT_TIMEOUT_S: f64 = 0.5;

/// The small facts kept beside a tool's image.
pub const IMAGE_FACTS: [&str; 13] = [
    "app",
    "bundle_id",
    "window",
    "title",
    "url",
    "tab",
    "shot",
    "width",
    "height",
    "window_id",
    "path",
    "duration_s",
    "sample_times_s",
];

/// `evidence_kind(tool)`: a snapshot's image, a recording's frames, else a screenshot.
pub fn evidence_kind(tool: &str) -> &'static str {
    if tool.ends_with(".snapshot") {
        "snapshot"
    } else if tool.starts_with("recording.") {
        "recording"
    } else {
        "screenshot"
    }
}

fn present(v: &Value) -> bool {
    !(v.is_null() || v.as_str() == Some(""))
}

/// `image_facts(tool, content)`: where an image came from, its size, how many controls the result listed.
pub fn image_facts(tool: &str, content: &Map<String, Value>) -> Map<String, Value> {
    let mut facts = Map::new();
    facts.insert("tool".into(), Value::String(tool.to_string()));
    for k in IMAGE_FACTS {
        if let Some(v) = content.get(k).filter(|v| present(v)) {
            facts.insert(k.into(), v.clone());
        }
    }
    if let Some(Value::Array(e)) = content.get("elements") {
        facts.insert("elements".into(), json!(e.len()));
    }
    facts
}

/// `disturbance.told(changed)` (HAND-212): what changed on the owner's screen during one call, briefly, for the
/// worker's own result; {} when nothing did.
pub fn told(changed: &Map<String, Value>) -> Map<String, Value> {
    let name = |app: &Value| -> Value {
        match app {
            Value::Object(a) => match a.get("name").filter(|v| ferb_py::value::truthy(v)) {
                Some(n) => n.clone(),
                None => a.get("bundle_id").cloned().unwrap_or(Value::Null),
            },
            other => other.clone(),
        }
    };
    let mut out = Map::new();
    match changed
        .get("frontmost_app")
        .filter(|v| ferb_py::value::truthy(v))
    {
        Some(front) => {
            let side = |k: &str| name(front.get(k).unwrap_or(&Value::Null));
            out.insert(
                "front_app".into(),
                json!({"before": side("before"), "after": side("after")}),
            );
        }
        None if changed.contains_key("key_window") => {
            out.insert("focused_window_changed".into(), Value::Bool(true));
        }
        None => {}
    }
    if let Some(f) = changed.get("fullscreen") {
        out.insert(
            "fullscreen".into(),
            f.get("after").cloned().unwrap_or(Value::Null),
        );
    }
    if changed.contains_key("active_space_epoch") {
        out.insert("space_changed".into(), Value::Bool(true));
    }
    if !out.is_empty() {
        out.insert("put_back".into(), Value::Bool(false));
    }
    out
}

/// The disturbance sentinel (`ferb/daemon/disturbance.py`, DMN-5): samples right around a call that can change the
/// screen. Errors are the error's name for the trace (`error_name`).
pub trait Sentinel: Send + Sync {
    /// HAND-207: a sound change near this call is noted.
    fn call_started(&self, task: &str, span: &str, name: &str);
    fn before_action<'a>(
        &'a self,
        task: &'a str,
        span: &'a str,
        name: &'a str,
        arguments: &'a Map<String, Value>,
        on_screen: bool,
    ) -> BoxFuture<'a, Result<(), String>>;
    fn call_ended(&self, span: &str);
    fn after_action<'a>(
        &'a self,
        span: &'a str,
    ) -> BoxFuture<'a, Result<Map<String, Value>, String>>;
}

/// The live previews (`ferb/daemon/task_preview.py` `TaskPreviews`): what each step works on.
pub trait Previews: Send + Sync {
    fn declare(&self, root: &str, task: &str, target: Option<PreviewTarget>, locks: &[String]);
    fn remove(&self, task_ids: &BTreeSet<String>);
}

/// The evidence store (V1): an image (or facts only) kept for the checker; the new row's id, or the error's name.
pub trait Evidence: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    fn put<'a>(
        &'a self,
        task: &'a str,
        span: &'a str,
        kind: &'a str,
        image: Option<&'a [u8]>,
        data: Map<String, Value>,
        origin: String,
    ) -> BoxFuture<'a, Result<String, String>>;
}

/// The change record (L39, `ferb/act/fs/changes.py` `Changes.before`): the counts for the call's record, or the
/// error's name.
pub trait ChangeRecord: Send + Sync {
    fn before<'a>(
        &'a self,
        task: &'a str,
        tree: &'a str,
        paths: Vec<String>,
    ) -> BoxFuture<'a, Result<Map<String, Value>, String>>;
}

/// The image sanitizer (L13, `ImageRedactor`): the image with its codes, card numbers and known secrets painted.
pub trait ImageSanitizer: Send + Sync {
    fn sanitize<'a>(&'a self, image: &'a [u8]) -> BoxFuture<'a, Result<Vec<u8>, String>>;
}

/// The working app's window as one picture (`ferb/act/mac/tools.py` `app_window`).
#[derive(Debug, Clone, Default)]
pub struct AppShot {
    pub image: Option<Vec<u8>>,
    /// Its secret fields, in image points (x, y, w, h), to paint over before anything else sees it.
    pub rects: Vec<[f64; 4]>,
    pub facts: Map<String, Value>,
    pub read_sources: Vec<(String, String)>,
}

/// The app-window picture and its secret-field masking (`app_window`, `mask_secret_fields`).
pub trait AppWindows: Send + Sync {
    fn shot<'a>(
        &'a self,
        app: &'a str,
        scopes: &'a BTreeSet<String>,
    ) -> BoxFuture<'a, Result<Option<AppShot>, Shot>>;
    fn mask<'a>(
        &'a self,
        image: Vec<u8>,
        rects: &'a [[f64; 4]],
    ) -> BoxFuture<'a, Result<Vec<u8>, Shot>>;
}

/// The shadow advisor (D23, advice only): `(call, "span:<id>")`; its failure is journaled, never the call's.
pub type Shadow =
    Arc<dyn Fn(Value, String) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;
/// Where an event goes (`cursor.point`).
pub type Publish = Arc<dyn Fn(&str, Map<String, Value>) + Send + Sync>;
/// Where a fault the hooks survive is journaled.
pub type Journal = Arc<dyn Fn(&str, Map<String, Value>) + Send + Sync>;

/// What the real hooks drive; every part is optional, as in the daemon when it isn't wired.
#[derive(Clone)]
pub struct HookParts {
    pub sentinel: Option<Arc<dyn Sentinel>>,
    pub previews: Option<Arc<dyn Previews>>,
    pub evidence: Option<Arc<dyn Evidence>>,
    pub changes: Option<Arc<dyn ChangeRecord>>,
    pub images: Option<Arc<dyn ImageSanitizer>>,
    pub app_windows: Option<Arc<dyn AppWindows>>,
    pub shadow: Option<Shadow>,
    pub publish: Publish,
    pub journal: Journal,
    /// FERB's workspace and the owner's home, for the paths a call names.
    pub workspace: String,
    pub home: String,
}

/// `CallHooks` (`call_hooks.py`).
pub struct RealHooks {
    parts: HookParts,
    /// (task, worker) -> apps whose window it has as evidence; (task, "initial") -> the first picture's state.
    looked: Mutex<HashMap<(String, String), BTreeSet<String>>>,
    /// Calls sampled before they ran (their span ids).
    sampled: Mutex<BTreeSet<String>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl RealHooks {
    pub fn new(parts: HookParts) -> RealHooks {
        RealHooks {
            parts,
            looked: Mutex::new(HashMap::new()),
            sampled: Mutex::new(BTreeSet::new()),
        }
    }

    /// The apps whose window a worker run has as evidence (`_looked_at`).
    pub fn looked_at(&self, task: &str, worker: &str) -> BTreeSet<String> {
        lock(&self.looked)
            .get(&(task.to_string(), worker.to_string()))
            .cloned()
            .unwrap_or_default()
    }

    /// This task's first-picture state (`looked[(task, "initial")]`).
    pub fn initial_state(&self, task: &str) -> BTreeSet<String> {
        self.looked_at(task, "initial")
    }

    fn initial_add(&self, task: &str, what: &str) {
        lock(&self.looked)
            .entry((task.to_string(), "initial".to_string()))
            .or_default()
            .insert(what.to_string());
    }
}

/// `{**content, key: value}`.
fn with(out: &mut ToolOutput, key: &str, value: Value) {
    out.result.content.insert(key.to_string(), value);
}

impl CallHooks for RealHooks {
    fn forget(&self, task_ids: &BTreeSet<String>) {
        if let Some(p) = &self.parts.previews {
            p.remove(task_ids);
        }
        lock(&self.looked).retain(|k, _| !task_ids.contains(&k.0));
    }

    fn stopped(&self, task_ids: &BTreeSet<String>) {
        if let Some(p) = &self.parts.previews {
            p.remove(task_ids);
        }
    }

    fn initial_image_given(&self, task_id: &str) {
        self.initial_add(task_id, "front window at send");
    }

    fn image_provided(&self, task_id: &str) {
        self.initial_add(task_id, "provided");
    }

    fn preview(
        &self,
        tool: &dyn Tool,
        args: &Args,
        grant: &RunGrant,
        root: &str,
        locks: &[String],
        observed: Option<&Map<String, Value>>,
    ) {
        // a declaration that fails is no surface, and never touches the call or its result
        let target = tool
            .preview_target(args, &grant.task_id, observed)
            .ok()
            .flatten();
        if observed.is_some() && target.is_none() {
            return; // the result adds nothing: the call's own surface stands
        }
        if let Some(p) = &self.parts.previews {
            p.declare(root, &grant.task_id, target, locks);
        }
    }

    fn point<'a>(
        &'a self,
        tool: &'a dyn Tool,
        args: &'a Args,
        grant: &'a RunGrant,
        root: &'a str,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let limit = Duration::from_secs_f64(POINT_TIMEOUT_S);
            let Ok(Ok(Some(p))) = tokio::time::timeout(limit, tool.point(args, grant)).await else {
                return; // never delays or fails the action
            };
            let rect: Vec<Value> = p.rect.iter().map(|v| json!(*v)).collect();
            let data = json!({"task_id": root, "worker_id": grant.worker_id, "kind": p.kind, "rect": rect,
                              "display": p.display});
            (self.parts.publish)(
                "cursor.point",
                data.as_object().cloned().unwrap_or_default(),
            );
        })
    }

    fn screen_before<'a>(
        &'a self,
        grant: &'a RunGrant,
        span_id: &'a str,
        name: &'a str,
        arguments: &'a Map<String, Value>,
        tool: &'a dyn Tool,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let Some(sentinel) = &self.parts.sentinel else {
                return;
            };
            let facts = tool.facts();
            if !facts.observation {
                sentinel.call_started(&grant.task_id, span_id, name);
            }
            if facts.on_screen {
                lock(&self.sampled).insert(span_id.to_string());
                // a failed sample never stops the call
                let _ = sentinel
                    .before_action(&grant.task_id, span_id, name, arguments, true)
                    .await;
            }
        })
    }

    fn screen_after<'a>(&'a self, span_id: &'a str) -> BoxFuture<'a, Map<String, Value>> {
        Box::pin(async move {
            let Some(sentinel) = &self.parts.sentinel else {
                return Map::new();
            };
            sentinel.call_ended(span_id);
            if !lock(&self.sampled).remove(span_id) {
                return Map::new();
            }
            match sentinel.after_action(span_id).await {
                Ok(m) => m,
                Err(name) => {
                    let mut m = Map::new();
                    m.insert("error".into(), Value::String(name));
                    m
                }
            }
        })
    }

    fn told(&self, screen: &Map<String, Value>) -> Map<String, Value> {
        let fact = if screen.is_empty() {
            Map::new()
        } else {
            told(screen)
        };
        let mut out = Map::new();
        if !fact.is_empty() {
            out.insert("screen_changed_during_call".into(), Value::Object(fact));
        }
        out
    }

    fn shadow(
        &self,
        tool: &str,
        spec: &ActionSpec,
        target: &Map<String, Value>,
        ids: &[String],
        task: &str,
        words: &str,
        span: &str,
    ) {
        let Some(shadow) = self.parts.shadow.clone() else {
            return;
        };
        let call = json!({"tool": tool, "kind": spec.kind, "target": target, "content": spec.content,
                          "code_questions": ids, "task_id": task, "owner_words": words});
        let work = shadow(call, format!("span:{span}"));
        let journal = self.parts.journal.clone();
        // fire-and-forget, kept until it ends; a failure is journaled, never the call's
        tokio::spawn(async move {
            if let Err(name) = work.await {
                journal(
                    "gateway.background_failed",
                    json!({"what": "shadow", "error": name})
                        .as_object()
                        .cloned()
                        .unwrap_or_default(),
                );
            }
        });
    }

    fn watch_changes<'a>(
        &'a self,
        tool: &'a dyn Tool,
        args: &'a Args,
        spec: &'a ActionSpec,
        grant: &'a RunGrant,
        tree: &'a str,
    ) -> BoxFuture<'a, Map<String, Value>> {
        Box::pin(async move {
            let Some(changes) = &self.parts.changes else {
                return Map::new();
            };
            if !CHANGE_KINDS.contains(&spec.kind.as_str()) {
                return Map::new();
            }
            let (ws, home) = (&self.parts.workspace, &self.parts.home);
            let owner = |p: &str| ferb_fs::files::owner_path(p, ws, home).map_err(|_| ());
            let mut paths: Vec<String> = Vec::new();
            let collected: Result<(), ()> = (|| {
                if let Some(declared) = tool.file_paths(args) {
                    for (p, access) in declared {
                        if access != "read" {
                            paths.push(owner(&p)?);
                        }
                    }
                }
                let content = spec.content.as_deref().filter(|c| !c.is_empty());
                if let (true, Some(text)) = (
                    spec.kind == Kind::RUN_SHELL || spec.kind == Kind::APPLESCRIPT,
                    content,
                ) {
                    let here = match (&spec.target.path, spec.kind == Kind::RUN_SHELL) {
                        (Some(p), true) if !p.is_empty() => p.clone(),
                        _ => ws.clone(),
                    };
                    paths.extend(ferb_fs::changes::named(text, &owner(&here)?, home));
                }
                Ok(())
            })();
            if collected.is_err() {
                // never stops the call: its check gets no diff for it
                return json!({"error": "RuntimeError"})
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
            }
            if paths.is_empty() {
                return Map::new();
            }
            match changes.before(&grant.task_id, tree, paths).await {
                Ok(counts) => counts,
                Err(name) => json!({"error": name})
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            }
        })
    }

    fn initial_image<'a>(
        &'a self,
        name: &'a str,
        tool: &'a dyn Tool,
        args: &'a Args,
        spec: &'a ActionSpec,
        grant: &'a RunGrant,
        out: ToolOutput,
    ) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let mac = name.starts_with("mac.");
            if grant.observe_only
                || !(mac || name == "browser.snapshot" || name == "ferb_browser.snapshot")
            {
                return out;
            }
            let key = (grant.task_id.clone(), "initial".to_string());
            let site = spec.target.site.clone().filter(|s| !s.is_empty());
            {
                let mut looked = lock(&self.looked);
                let state = looked.entry(key.clone()).or_default();
                if !state.is_empty()
                    || (mac && (site.is_none() || self.parts.app_windows.is_none()))
                {
                    return out;
                }
                // reserved before awaiting, so concurrent calls cannot emit two initial pictures
                state.insert("attempted".into());
            }
            let mut out = out;
            let taken: Result<Option<ToolOutput>, Shot> = async {
                if mac {
                    let windows = self.parts.app_windows.as_ref().expect("checked");
                    let site = site.as_deref().expect("checked");
                    let Some(shot) = windows.shot(site, &grant.scopes).await? else {
                        return Ok(None);
                    };
                    let mut image = shot.image.clone();
                    if let (Some(img), false) = (image.clone(), shot.rects.is_empty()) {
                        image = Some(windows.mask(img, &shot.rects).await?);
                    }
                    let mut o = out.clone();
                    o.result.image = image;
                    with(&mut o, "initial_image", Value::Object(shot.facts.clone()));
                    let mut sources: Vec<(String, String)> = Vec::new();
                    for rs in out
                        .result
                        .read_sources
                        .iter()
                        .chain(shot.read_sources.iter())
                    {
                        if !sources.contains(rs) {
                            sources.push(rs.clone());
                        }
                    }
                    o.result.read_sources = sources;
                    return Ok(Some(o));
                }
                let (image, masking) = tool.tab_picture(args, &grant.task_id).await?;
                let mut o = out.clone();
                o.result.image = Some(image);
                let mut facts = json!({"shot": null, "width": null, "height": null})
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                if !masking.is_empty() {
                    facts.insert("masking".into(), Value::String(masking));
                }
                with(&mut o, "initial_image", Value::Object(facts));
                Ok(Some(o))
            }
            .await;
            match taken {
                Ok(Some(o)) => o,
                Ok(None) => out,
                Err(Shot::NoTarget) => {
                    // no window yet; the first actual working window can still supply the initial image
                    lock(&self.looked).remove(&key);
                    out
                }
                Err(Shot::Withheld(why)) => {
                    with(&mut out, "image_withheld", Value::String(why));
                    out
                }
                Err(Shot::Failed) => {
                    with(
                        &mut out,
                        "image_withheld",
                        Value::String("initial window capture unavailable".into()),
                    );
                    out
                }
            }
        })
    }

    fn redact_image<'a>(
        &'a self,
        out: ToolOutput,
    ) -> BoxFuture<'a, (ToolOutput, Map<String, Value>)> {
        Box::pin(async move {
            let mut out = out;
            let Some(image) = out.result.image.clone() else {
                return (out, Map::new());
            };
            let clean = match &self.parts.images {
                Some(s) => s.sanitize(&image).await,
                None => Err("no image sanitizer".into()),
            };
            match clean {
                Ok(img) => {
                    out.result.image = Some(img);
                    (out, Map::new())
                }
                Err(_) => {
                    out.result.image = None;
                    let mut facts = Map::new();
                    facts.insert(
                        "image_withheld".into(),
                        Value::String("image redaction unavailable".into()),
                    );
                    (out, facts)
                }
            }
        })
    }

    fn keep_image<'a>(
        &'a self,
        name: &'a str,
        site: Option<&'a str>,
        grant: &'a RunGrant,
        span_id: &'a str,
        content: &'a Map<String, Value>,
        image: Option<&'a [u8]>,
        origins: &'a BTreeSet<String>,
    ) -> BoxFuture<'a, Map<String, Value>> {
        Box::pin(async move {
            let Some(evidence) = &self.parts.evidence else {
                return Map::new();
            };
            let initial = content.get("initial_image").and_then(Value::as_object);
            let mut facts = match initial {
                Some(i) => i.clone(),
                None => image_facts(name, content),
            };
            if let Some(w) = content
                .get("image_withheld")
                .filter(|v| ferb_py::value::truthy(v))
            {
                facts.insert("image_withheld".into(), w.clone());
            }
            let kind = if initial.is_some() && name.starts_with("mac.") {
                "app_window"
            } else {
                evidence_kind(name)
            };
            let joined = origin::join(origins.iter().map(String::as_str));
            let mut out = Map::new();
            match evidence
                .put(&grant.task_id, span_id, kind, image, facts, joined)
                .await
            {
                Ok(id) => {
                    if let (true, Some(site)) =
                        (name.starts_with("mac."), site.filter(|s| !s.is_empty()))
                    {
                        // this image (kept or facts only) was the worker's first look at that app
                        lock(&self.looked)
                            .entry((grant.task_id.clone(), grant.worker_id.clone()))
                            .or_default()
                            .insert(ferb_policy::py::lower(site));
                    }
                    out.insert("evidence".into(), Value::String(id));
                }
                Err(name) => {
                    out.insert("evidence_error".into(), Value::String(name));
                }
            }
            out
        })
    }
}
