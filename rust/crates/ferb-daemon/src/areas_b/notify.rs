//! Telling the owner outside a chat turn: banners (FERB.app or osascript), held banners, notices in chat, and the
//! notification RPCs. Port of `ferb/daemon/areas/notify.py`. Must not decide what is worth telling: callers do.
//!
//! RPCs: `notifications.subscribe` (FERB.app takes FERB's banners while its stream is open), `notifications.categories`,
//! `attention.ack`, `notices.list`. The rest ([`Notifier`]) is what the supervisor, the gateway and the browser area
//! call: `notify` (category switches, held while the Mac is out of use), `deliver` (FERB.app or osascript),
//! `post_notice` / `tell_owner` / `loop_notice` / `tell_refused` (a notice line in the owner's chat).

use std::sync::{Arc, Mutex};

use ferb_orchestrator::attention::AttentionNotices;
use ferb_rpc::{BoxFuture, EventStream, HandlerError, HandlerResult, Server};
use ferb_store::Db;
use serde_json::{json, Map, Value};

use super::banners::{self, Banner, Posters};
use super::core::{ok, refusal, Host, P};

/// `osascript -e 'display notification …'` posts under Script Editor's bundle id.
pub const OSASCRIPT_POSTER: &str = "com.apple.ScriptEditor2";

/// The daemon's store connection the notices live in (`attention_notices`).
pub type SharedDb = Arc<Mutex<Db>>;

/// What posts a macOS notification without FERB.app (`osascript`), and what knows the Mac is out of use.
pub trait Desk: Send + Sync {
    /// `_osascript_notify`'s process: `osascript -e <script>`; its own 10 s limit.
    fn osascript(&self, script: &str) -> BoxFuture<'static, Result<(), String>>;
    /// `MacPresence.away()` (asleep, displays asleep, locked or switched away; unknown counts as present).
    fn away(&self) -> bool;
}

/// What the notices path tells others (seams owned by perception and the clients' subscriptions).
pub trait Listeners: Send + Sync {
    /// `_client_shows(topic)`: a connected client's `logs.subscribe` receives this topic.
    fn client_shows(&self, topic: &str) -> bool;
    /// `perception.banner_posted` (FERB knows its own banners); returns a handle to un-post on failure.
    fn banner_posted(&self, _shown: &Value, _poster: &str) -> Option<Value> {
        None
    }
    fn banner_failed(&self, _banner: &Value) {}
    /// `perception.policy.told(bundle_id, conversation, row id, text)` for a notification row a notice cites.
    fn told(&self, _bundle_id: &str, _conversation: &str, _row_id: &str, _text: &str) {}
    /// `trail.row(id)`: (source, data_json) of a trail row.
    fn trail_row(&self, _id: &str) -> Option<(String, String)> {
        None
    }
    /// `self._attached_trees`: task trees a chat attached to (single-client state, stage 1).
    fn attached(&self, _tree: &str) -> bool {
        false
    }
    /// `self.tasks.column(task_id, "request_id")`.
    fn request_of(&self, _task_id: &str) -> Value {
        Value::Null
    }
}

pub struct Deps {
    pub posters: Posters,
    /// The store holding `attention_notices`; None: owner notices are unavailable (`attention_notices is None`).
    pub notices: Option<SharedDb>,
}

pub fn register(server: &Server, host: &Host, deps: &Deps) {
    let posters = deps.posters.clone();
    server.register_stream(
        "notifications.subscribe",
        move |_s, params: Map<String, Value>| -> Box<dyn EventStream> {
            if !P(&params).is_true("authorized") {
                return Box::new(Failed(Some(refusal(
                    "FERB.app may not post notifications yet; FERB keeps posting them through osascript",
                ))));
            }
            let poster = posters.open();
            Box::new(Subscribed {
                posters: posters.clone(),
                poster,
            })
        },
    );
    let h = host.clone();
    server.register_unary(
        "notifications.categories",
        move |_s, params: Map<String, Value>| {
            let h = h.clone();
            async move { categories(&h, &params) }
        },
    );
    let (h, db) = (host.clone(), deps.notices.clone());
    server.register_unary("attention.ack", move |_s, params: Map<String, Value>| {
        let (h, db) = (h.clone(), db.clone());
        async move { attention_ack(&h, db.as_ref(), &params) }
    });
    let (h, db) = (host.clone(), deps.notices.clone());
    server.register_unary("notices.list", move |_s, _params: Map<String, Value>| {
        let (h, db) = (h.clone(), db.clone());
        async move { notices_list(&h, db.as_ref()) }
    });
}

/// A stream that fails at once.
struct Failed(Option<HandlerError>);

impl EventStream for Failed {
    fn next(&mut self) -> BoxFuture<'_, Option<HandlerResult>> {
        let e = self.0.take();
        Box::pin(async move { e.map(Err) })
    }
}

/// `_notifications_subscribe`: each banner as it comes; the poster closes when the stream ends or its caller leaves.
struct Subscribed {
    posters: Posters,
    poster: banners::Poster,
}

impl EventStream for Subscribed {
    fn next(&mut self) -> BoxFuture<'_, Option<HandlerResult>> {
        Box::pin(async move { self.poster.queue.recv().await.map(Ok) })
    }
}

impl Drop for Subscribed {
    fn drop(&mut self) {
        self.posters.close(self.poster.id);
    }
}

/// `_notifications_categories`.
fn categories(h: &Host, params: &Map<String, Value>) -> HandlerResult {
    if let Some(set) = params.get("set") {
        let switched = banners::set_categories(&h.config, set)?;
        h.journal("banners.switched", json!({"set": set}), None)?;
        return ok(json!({"categories": switched}));
    }
    h.get("banners", "question")?; // an unreadable config refuses, as Python's read does
    ok(json!({"categories": banners::categories(&banners::getter(&h.config))}))
}

/// The notice ids `attention.ack` names, or why they don't do (ENG-6: Python answered these with an internal error,
/// `KeyError` or `ValueError`; here they are refusals in the same words).
fn ack_ids(params: &Map<String, Value>) -> Result<Vec<String>, HandlerError> {
    const BAD: &str = "ids must be a nonempty list of notice ids";
    let ids = match (params.get("ids"), params.get("id")) {
        (Some(ids), _) => ids.clone(),
        (None, Some(id)) => json!([id]),
        (None, None) => return Err(refusal(BAD)),
    };
    match ids {
        Value::Array(items) if !items.is_empty() => items
            .iter()
            .map(|i| match i {
                Value::String(s) if !s.is_empty() => Ok(s.clone()),
                _ => Err(refusal(BAD)),
            })
            .collect(),
        _ => Err(refusal(BAD)),
    }
}

fn notices<'a>(h: &Host, db: &'a Db) -> AttentionNotices<'a> {
    let clock = h.clock.clone();
    let redactor = h.redactor.clone();
    AttentionNotices {
        db,
        now: Box::new(move || ferb_core::when::isoformat(&clock.now())),
        redact: Arc::new(move |v| redactor.value(v)),
    }
}

/// `_attention_ack`: a client showed these notices (they leave `chat.status`).
fn attention_ack(h: &Host, db: Option<&SharedDb>, params: &Map<String, Value>) -> HandlerResult {
    let Some(db) = db else {
        return Err(refusal("Owner notices are unavailable"));
    };
    let ids = ack_ids(params)?;
    let db = db.lock().unwrap();
    notices(h, &db).acknowledge(&ids).map_err(|e| match e {
        // an unknown id (ENG-6: Python's ValueError, an internal error)
        ferb_orchestrator::sql::Error::Value(text) => refusal(text),
        other => HandlerError::fault("DatabaseError", other.to_string()),
    })?;
    ok(json!({"ok": true}))
}

/// `_notices_list`: every notice FERB posted, newest first, as clients render them, and whether one was shown.
fn notices_list(h: &Host, db: Option<&SharedDb>) -> HandlerResult {
    let Some(db) = db else {
        return ok(json!({"notices": []}));
    };
    let db = db.lock().unwrap();
    let n = notices(h, &db);
    let fault =
        |e: ferb_orchestrator::sql::Error| HandlerError::fault("DatabaseError", e.to_string());
    let mut rows = n.list(true).map_err(fault)?;
    rows.extend(n.list(false).map_err(fault)?);
    // a stable sort on created_at, newest first (Python's sort(reverse=True) keeps equal keys in order)
    rows.sort_by_key(|r| std::cmp::Reverse(key(r)));
    let mut out = Vec::new();
    for r in &rows {
        let mut v = AttentionNotices::view(r).map_err(fault)?;
        v["shown"] = Value::Bool(!r.get("delivered_at").is_none_or(Value::is_null));
        out.push(v);
    }
    ok(json!({"notices": out}))
}

fn key(row: &Map<String, Value>) -> String {
    row.get("created_at")
        .map(ferb_py::value::py_str)
        .unwrap_or_default()
}

/// The non-RPC half: what other parts call to tell the owner.
pub struct Notifier {
    pub host: Host,
    pub posters: Posters,
    pub desk: Arc<dyn Desk>,
    pub listeners: Arc<dyn Listeners>,
    pub notices: Option<SharedDb>,
    /// Banners waiting for the Mac to be in use again (`_held`).
    pub held: Mutex<Vec<Banner>>,
    /// `uuid.uuid4().hex[:12]` (an injected source: tests count).
    pub uid: Arc<dyn Fn() -> String + Send + Sync>,
}

/// AppleScript's string quoting of an already redacted text (`_osascript_notify`'s `q`).
pub fn applescript_quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

impl Notifier {
    /// `_osascript_notify`: the OS's own notification; title and text redacted and quoted for AppleScript.
    pub async fn osascript_notify(&self, title: &str, text: &str) -> Result<(), String> {
        let script = format!(
            "display notification {} with title {}",
            applescript_quote(&self.host.redact_text(text)),
            applescript_quote(&self.host.redact_text(title))
        );
        self.desk.osascript(&script).await
    }

    /// `_notify_safely`: FERB knows its own banners (L10); a banner never posted un-posts its record.
    pub async fn notify_safely(&self, title: &str, text: &str) {
        let shown =
            json!({"title": self.host.redact_text(title), "body": self.host.redact_text(text)});
        let banner = self
            .listeners
            .banner_posted(&self.host.redact(&shown), OSASCRIPT_POSTER);
        if self.osascript_notify(title, text).await.is_err() {
            if let Some(b) = banner {
                self.listeners.banner_failed(&b);
            }
        }
    }

    /// `_notify`: a category the owner switched off posts nothing; while the Mac is out of use, non-urgent banners
    /// wait until it's back.
    pub async fn notify(&self, banner: Banner, urgent: bool) {
        if !banners::enabled(&banners::getter(&self.host.config), &banner.category) {
            return;
        }
        if !urgent && self.desk.away() {
            self.held.lock().unwrap().push(banner);
            return;
        }
        self.deliver(&banner).await;
    }

    /// One pass of `_flush_held` (Python's loop runs it every 10 s).
    pub async fn flush_held(&self) {
        let held = {
            let mut held = self.held.lock().unwrap();
            if held.is_empty() || self.desk.away() {
                return;
            }
            std::mem::take(&mut *held)
        };
        for banner in &held {
            self.deliver(banner).await;
        }
    }

    /// `_deliver`: FERB.app posts it while it is the poster, else osascript does. Title and body redacted here.
    pub async fn deliver(&self, banner: &Banner) {
        if self.posters.connected() {
            let id = if banner.ident.is_empty() {
                format!("banner:{}", (self.uid)())
            } else {
                banner.ident.clone()
            };
            let mut item = Map::new();
            item.insert("id".into(), json!(id));
            item.insert("category".into(), json!(banner.category));
            item.insert("title".into(), json!(self.host.redact_text(&banner.title)));
            item.insert("body".into(), json!(self.host.redact_text(&banner.text)));
            item.insert("target".into(), Value::Object(banner.target.clone()));
            item.insert("when_hidden".into(), json!(banner.when_hidden));
            if !banner.card.is_empty() {
                item.insert(
                    "card".into(),
                    self.host.redact(&Value::Object(banner.card.clone())),
                );
            }
            if self.posters.deliver(&item) {
                return;
            }
        }
        self.notify_safely(&banner.title, &banner.text).await;
    }

    /// `_post_notice`: a notice is a FERB message in the owner's chat: journaled `chat.notice`, published
    /// `attention.notice`; a notification it cites gets its told receipt; a banner only for a loop's notice while
    /// FERB.app is the poster, or when no connected client receives `attention.notice`.
    pub async fn post_notice(
        &self,
        notice: &Map<String, Value>,
        category: &str,
        task_id: &str,
    ) -> Result<(), String> {
        let view = AttentionNotices::view(notice).map_err(|e| e.to_string())?;
        let mut logged = Map::new();
        for k in ["id", "text", "quotes", "cards"] {
            if let Some(v) = view.get(k) {
                logged.insert(k.into(), v.clone());
            }
        }
        let origin = notice.get("origin").map(ferb_py::value::py_str);
        self.host
            .journal("chat.notice", Value::Object(logged), origin.as_deref())
            .map_err(|e| e.message)?;
        let refs: Vec<String> = notice
            .get("source_refs_json")
            .and_then(Value::as_str)
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        for r in refs {
            let id = r.strip_prefix("trail:").unwrap_or(&r);
            if let Some((source, data)) = self.listeners.trail_row(id) {
                if source == "notification" {
                    let data: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
                    let s = |k: &str| data.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
                    let text = view
                        .get("text")
                        .map(ferb_py::value::py_str)
                        .unwrap_or_default();
                    self.listeners
                        .told(&s("bundle_id"), &s("conversation"), id, &text);
                }
            }
        }
        let connected = self.posters.connected();
        let bannered = connected
            && category == "loop"
            && banners::enabled(&banners::getter(&self.host.config), category);
        let mut published = view.clone();
        if bannered {
            published["banner"] = Value::Bool(true);
        }
        self.host.publish("attention.notice", published);
        let text = view
            .get("text")
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        if connected {
            if category == "loop" {
                let mut target = Map::new();
                target.insert("kind".into(), json!("task"));
                target.insert("task_id".into(), json!(task_id));
                let id = view
                    .get("id")
                    .map(ferb_py::value::py_str)
                    .unwrap_or_default();
                let banner = Banner {
                    title: "FERB loop".into(),
                    text,
                    category: "loop".into(),
                    target,
                    when_hidden: true,
                    ident: format!("loop:{id}"),
                    card: Map::new(),
                };
                self.notify(banner, false).await;
            }
        } else if !self.listeners.client_shows("attention.notice") {
            let banner = Banner {
                title: "FERB noticed".into(),
                text,
                category: category.into(),
                ..Banner::default()
            };
            self.notify(banner, false).await;
        }
        Ok(())
    }

    /// `_tell_owner`: one notice line in the owner's chat, posted once per `ident`.
    #[allow(clippy::too_many_arguments)]
    pub async fn tell_owner(
        &self,
        ident: &str,
        refs: &[String],
        text: &str,
        notice_origin: &str,
        category: &str,
        task_id: &str,
        cards: &[Value],
        quotes: &Value,
        quote_origin: Option<&str>,
    ) -> Result<(), String> {
        let Some(db) = &self.notices else {
            return Ok(());
        };
        let notice = {
            let guard = db.lock().unwrap();
            let n = notices(&self.host, &guard);
            n.add(
                ident,
                refs,
                text,
                quotes,
                &[],
                quote_origin,
                Some(notice_origin),
                cards,
            )
            .map_err(|e| e.to_string())?
        };
        if let Some(notice) = notice {
            self.post_notice(&notice, category, task_id).await?;
        }
        Ok(())
    }

    /// `_loop_notice` (L31): a loop's back-off or pause notice, posted like a notice.
    pub async fn loop_notice(
        &self,
        tree_id: &str,
        step_id: &str,
        text: &str,
        notice_origin: &str,
    ) -> Result<(), String> {
        let ident = format!("loop-{step_id}-{}", (self.uid)());
        self.tell_owner(
            &ident,
            &[format!("task:{tree_id}")],
            text,
            notice_origin,
            "loop",
            tree_id,
            &[],
            &Value::Null,
            None,
        )
        .await
    }

    /// `_tell_refused` (GATE-562): a page asked again for a value FERB filled sealed.
    pub async fn tell_refused(&self, data: &Map<String, Value>) -> Result<(), String> {
        let s = |k: &str, d: &str| match data.get(k) {
            Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
            _ => d.to_owned(),
        };
        let what = s("what", "value");
        let place = s("place", "the page");
        let words = ferb_py::value::strip(&s("words", "")).to_owned();
        let text = format!(
            "{place} didn't accept the {what} you gave: the page asked for it again{}",
            if words.is_empty() { "." } else { ". It says:" }
        );
        let span = data
            .get("span_id")
            .map(ferb_py::value::py_str)
            .unwrap_or("None".into());
        let task = data
            .get("task_id")
            .map(ferb_py::value::py_str)
            .unwrap_or("None".into());
        let quotes = if words.is_empty() {
            Value::Null
        } else {
            json!([{"text": words}])
        };
        let quote_origin = ferb_sanitize::origin::external("web", &place);
        self.tell_owner(
            &format!("refused-{span}"),
            &[format!("task:{task}")],
            &text,
            ferb_sanitize::origin::FERB,
            "alert",
            &s("task_id", ""),
            &[],
            &quotes,
            Some(&quote_origin),
        )
        .await
    }

    /// `_task_result`: what FERB told (or will tell) the owner about a task; a macOS notification only when chat
    /// was closed and no connected FERB client shows `task.result` (L9). An `alert` (a watch's condition met) is told
    /// as a reminder is; without `publish` it is only recorded (fix wave 6c, I3).
    pub async fn task_result(
        &self,
        ev: &Value,
        shown: bool,
        alert: bool,
        publish: bool,
    ) -> Result<(), String> {
        let ev = self.host.redact(ev);
        let s = |k: &str| match ev.get(k) {
            Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
            _ => String::new(),
        };
        let answer = if s("answer").is_empty() {
            s("summary")
        } else {
            s("answer")
        };
        let status = ev
            .get("status")
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let shown = shown || status == "cancelled"; // stopped by the owner: they know
        let at = if s("at").is_empty() {
            self.host.now_iso()
        } else {
            s("at")
        };
        let task_id = ev
            .get("task_id")
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let cards: Vec<Value> = match ev.get("cards") {
            Some(Value::Array(a)) => a.iter().filter(|c| c.is_object()).cloned().collect(),
            _ => vec![],
        };
        let goal = ev
            .get("goal")
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let Some(db) = &self.notices else {
            return Err("daemon not started".into());
        };
        {
            let db = db.lock().unwrap();
            ferb_store::task_results::TaskResults::new(&db)
                .record(
                    &task_id,
                    &at,
                    &status,
                    &goal,
                    &answer,
                    shown,
                    Some(&json!(cards)),
                    &s("name"),
                )
                .map_err(|e| e.to_string())?;
        }
        if !publish {
            return Ok(());
        }
        let mut task = Map::new();
        task.insert("kind".into(), json!("task"));
        task.insert("task_id".into(), json!(task_id));
        let name = ferb_py::value::strip(&s("name")).to_owned();
        if alert {
            let banner = Banner {
                title: if name.is_empty() {
                    "FERB alert".into()
                } else {
                    name
                },
                text: answer.clone(),
                category: "alert".into(),
                target: task,
                ..Banner::default()
            };
            self.notify(banner, true).await;
        } else if status != "cancelled"
            && (self.posters.connected() || (!shown && !self.listeners.client_shows("task.result")))
        {
            let title = if !name.is_empty() {
                name
            } else if status == "done" {
                "FERB finished".into()
            } else {
                "FERB".into()
            };
            let banner = Banner {
                title,
                text: answer.clone(),
                category: "task_finished".into(),
                target: task,
                when_hidden: self.posters.connected(),
                ident: format!("task:{task_id}"),
                card: Map::new(),
            };
            self.notify(banner, false).await;
        }
        let mut out = ev.as_object().cloned().unwrap_or_default();
        if !out.contains_key("request_id") {
            // a resumed plan or scheduled run: the request its task ran for
            out.insert("request_id".into(), self.listeners.request_of(&task_id));
        }
        out.insert("at".into(), json!(at));
        out.insert("shown".into(), json!(shown));
        self.host.publish("task.result", Value::Object(out));
        Ok(())
    }

    /// `_approval_notice`: an approval is waiting and no chat shows it: a banner (L9, spec 8.4). The setup profile
    /// question is never a banner (L3). Returns the banner to post (`_spawn`ed by the caller), or None.
    pub fn approval_notice(&self, data: &Value) -> Option<(Banner, bool)> {
        let s = |k: &str| match data.get(k) {
            Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
            _ => String::new(),
        };
        let tid = s("task_id");
        if data.get("capability") == Some(&json!("setup.persona")) {
            return None;
        }
        let line = if s("summary").is_empty() {
            s("reason")
        } else {
            s("summary")
        };
        let text = format!("FERB needs your OK: {line}");
        let root = tid.split('.').next().unwrap_or("").to_owned();
        if self.posters.connected() {
            let approval = s("approval_id");
            let mut target = Map::new();
            target.insert("kind".into(), json!("question"));
            target.insert("approval_id".into(), json!(approval));
            target.insert("task_id".into(), json!(root));
            return Some((
                Banner {
                    title: "FERB needs you".into(),
                    text,
                    category: "question".into(),
                    target,
                    when_hidden: true,
                    ident: format!("question:{approval}"),
                    card: Map::new(),
                },
                true,
            ));
        }
        if self.listeners.attached(&root) || self.listeners.client_shows("approval.requested") {
            return None;
        }
        Some((
            Banner {
                title: "FERB needs you".into(),
                text,
                category: "question".into(),
                ..Banner::default()
            },
            true,
        ))
    }

    /// `_tell_set_aside`: each file FERB set aside because it couldn't be read, journaled `home.set_aside` and told
    /// once as a notice; a failure is journaled and the next start tells it.
    pub async fn tell_set_aside(&self) {
        let entries = ferb_store::salvage::pending(&self.host.home);
        if entries.is_empty() {
            return;
        }
        let run = async {
            for entry in &entries {
                let v = serde_json::to_value(entry).map_err(|e| e.to_string())?;
                let mut row = Map::new();
                for k in ["file", "moved_to", "why", "at"] {
                    row.insert(k.into(), v.get(k).cloned().unwrap_or(Value::Null));
                }
                self.host
                    .journal("home.set_aside", Value::Object(row), None)
                    .map_err(|e| e.message)?;
                let moved = v
                    .get("moved_to")
                    .map(ferb_py::value::py_str)
                    .unwrap_or_default();
                let name = std::path::Path::new(&moved)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.tell_owner(
                    &format!("set-aside-{name}"),
                    &[],
                    &ferb_store::salvage::notice(entry),
                    ferb_sanitize::origin::FERB,
                    "alert",
                    "",
                    &[],
                    &Value::Null,
                    None,
                )
                .await?;
            }
            ferb_store::salvage::told(&self.host.home, &entries).map_err(|e| e.to_string())
        };
        if let Err(e) = run.await {
            let text: String = e.chars().take(300).collect();
            let _ = self
                .host
                .journal("home.set_aside_failed", json!({"error": text}), None);
        }
    }
}
