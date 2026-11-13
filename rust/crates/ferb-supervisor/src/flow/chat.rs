//! The daemon's `chat.send` (ferb/daemon/areas/chat.py:322 `_chat_send`): an `ack` on receipt, journaled, before
//! anything else; then the supervisor's events for this message, each stamped with the message's request id and the
//! time it was queued (`request_id`, `at`); a task's outcome is recorded as its result (`_task_result`). What the app
//! gets for a failed turn is recorded too.
//!
//! The bus events the daemon merges into the stream (areas/chat.py:360 `bus_events`): `tool.called`,
//! `approval.requested` and `action.*` about this turn's tasks (a task id the turn's events named, or one in its tree),
//! wrapped `{"type": "event", "topic", "data"}` with the data redacted; an `action.*` outside any task goes to every
//! open chat with `request_id: null`. They come through [`super::Daemon::subscribe`] the moment they are published and
//! join the turn's own events in one queue, so each frame keeps the order in which it happened.

use std::collections::BTreeSet;
use std::sync::Arc;

use ferb_sanitize::untrusted::mark;
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;

use super::{Failure, Inner, Supervisor};

/// What `chat.send` was asked.
#[derive(Clone, Debug, Default)]
pub struct SendParams {
    pub session_id: String,
    pub text: String,
    pub input_paths: Vec<String>,
    /// The front window is captured at send (L12).
    pub screen: bool,
}

/// A refusal before the stream starts (the RPC's error).
#[derive(Clone, Debug, PartialEq)]
pub struct Refused(pub String);

impl Supervisor {
    /// `chat.send`: the stream's events, each as the app receives it (`chat.send.event` params), in order; the stream
    /// ends when the receiver closes. A refusal before the ack is an `Err`.
    pub fn chat_send(&self, params: SendParams) -> Result<mpsc::UnboundedReceiver<Value>, Refused> {
        self.inner.chat_send(params)
    }

    /// `chat.send` from its RPC params as the app sends them: `input_paths` must be a list of nonempty paths;
    /// `session_id` and `text` are read as Python's `str()` reads them.
    pub fn chat_send_params(
        &self,
        params: &Map<String, Value>,
    ) -> Result<mpsc::UnboundedReceiver<Value>, Refused> {
        let paths = match params.get("input_paths") {
            None => Vec::new(),
            Some(Value::Array(items)) if items.iter().all(Value::is_string) => items
                .iter()
                .map(|p| p.as_str().unwrap_or("").to_string())
                .collect(),
            Some(_) => {
                return Err(Refused(
                    "input_paths must be a list of nonempty filesystem paths".into(),
                ))
            }
        };
        let text = |k: &str, default: &str| match params.get(k) {
            None => default.to_string(),
            Some(v) => ferb_py::value::py_str(v),
        };
        self.chat_send(SendParams {
            session_id: text("session_id", "default"),
            text: text("text", ""),
            input_paths: paths,
            screen: false,
        })
    }
}

impl Inner {
    fn chat_send(&self, params: SendParams) -> Result<mpsc::UnboundedReceiver<Value>, Refused> {
        let received = self.now_iso();
        let ok = params
            .input_paths
            .iter()
            .all(|p| !ferb_orchestrator::py::strip(p).is_empty() && !p.contains('\0'));
        if !ok {
            return Err(Refused(
                "input_paths must be a list of nonempty filesystem paths".into(),
            ));
        }
        let mut paths: Vec<String> = Vec::new();
        for p in &params.input_paths {
            let n = ferb_fs::files::normalize_path(p, &self.deps.workspace, &self.deps.home)
                .unwrap_or_else(|_| p.clone());
            if !paths.contains(&n) {
                paths.push(n);
            }
        }
        if paths
            .iter()
            .any(|p| self.deps.redactor.hit(p).ok().flatten().is_some())
        {
            return Err(Refused(
                "input_paths contains protected secret text; rename the attachment before sharing its path".into(),
            ));
        }
        let session = params.session_id.clone();
        if session.starts_with("schedule:") || session.starts_with("preview:") {
            return Err(Refused(
                "session_id names one of FERB's own sessions, not a chat".into(),
            ));
        }
        let hidden: Vec<String> = self
            .deps
            .redactor
            .kinds(&params.text)
            .unwrap_or_default()
            .into_iter()
            .map(str::to_string)
            .collect();
        let text = self.hide_code_reply(&self.redact_text(&params.text), &session);
        let request_id = self.new_id();
        let (tx, rx) = mpsc::unbounded_channel();
        let entry = self.journal(
            "chat.ack",
            json!({"session_id": session, "request_id": request_id, "received_at": received}),
            None,
        );
        if !hidden.is_empty() {
            self.journal(
                "chat.secret_hidden",
                json!({"session_id": session, "request_id": request_id, "kinds": hidden}),
                None,
            );
        }
        let message = ferb_orchestrator::py::strip(&mark(&text).for_model).to_string();
        let mut ack = Map::new();
        ack.insert("type".into(), json!("ack"));
        ack.insert("request_id".into(), json!(request_id));
        ack.insert("message".into(), json!(message));
        ack.insert("at".into(), json!(entry.at));
        if !hidden.is_empty() {
            ack.insert("hidden".into(), json!(hidden));
        }
        let _ = tx.send(Value::Object(ack));
        let me = self.me();
        let job = tokio::spawn(async move {
            me.turn_stream(session, text, paths, request_id, hidden, params.screen, tx)
                .await;
        });
        self.track(job);
        Ok(rx)
    }

    /// `_chat_send`'s `turn()`: the supervisor's events, stamped, until the turn ends.
    #[allow(clippy::too_many_arguments)] // the turn's own arguments
    async fn turn_stream(
        self: Arc<Self>,
        session: String,
        text: String,
        paths: Vec<String>,
        request_id: String,
        hidden: Vec<String>,
        screen: bool,
        out: mpsc::UnboundedSender<Value>,
    ) {
        let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
        // `self.bus.subscribe({"tool.called", "approval.requested", "action.*"})`: each event joins the turn's queue
        // as it is published, redacted as `bus_events` redacts it
        let unsubscribe = {
            let bus = tx.clone();
            let redactor = self.deps.redactor.clone();
            let me = self.me.clone();
            let turn = request_id.clone();
            self.deps.daemon.subscribe(
                &BUS_TOPICS,
                Arc::new(move |topic: &str, data: &ferb_sanitize::Py| {
                    let data = py_json(&redactor.py(data));
                    // Tagged with this turn: a running tree's line is shared by every chat's subscription, and each
                    // turn takes only its own copy. An event about a running tree's task rides that tree's line, so it
                    // reaches the stream as the tree's own events do (one hop, as in Python both the tree's queue and
                    // the bus's are); anything else joins the turn's queue now.
                    let event = json!({BUS: topic, "data": data, TURN: turn});
                    let tid = data.get("task_id").filter(|t| ferb_py::value::truthy(t));
                    if let (Some(tid), Some(sup)) = (tid, me.upgrade()) {
                        let tree = sup.tree_of(&ferb_py::value::py_str(tid));
                        let open = tree.filter(|t| sup.live().live_lines.contains(t));
                        if let (Some(tree), Some(worker), Value::Object(ev)) =
                            (open, sup.worker.get(), &event)
                        {
                            worker.tell_tree(&tree, ev.clone());
                            return;
                        }
                    }
                    let _ = bus.send(event);
                }),
            )
        };
        let stamp = |ev: Value| -> Value {
            let mut ev = match ev {
                Value::Object(m) => m,
                _ => Map::new(),
            };
            ev.insert("request_id".into(), json!(request_id));
            ev.insert("at".into(), json!(self.now_iso()));
            Value::Object(ev)
        };
        let run = async {
            let tx = tx;
            self.send(
                &session,
                &text,
                paths,
                request_id.clone(),
                hidden,
                screen,
                &tx,
            )
            .await
        };
        tokio::pin!(run);
        let mut my_tasks: BTreeSet<String> = BTreeSet::new();
        let take = |ev: Value, my_tasks: &mut BTreeSet<String>| {
            if let Some(topic) = ev.get(BUS).and_then(Value::as_str) {
                if ev.get(TURN).and_then(Value::as_str) != Some(request_id.as_str()) {
                    return; // another chat's copy, on a tree's line this turn reads
                }
                let data = &ev["data"];
                let ours = self.ours(data, my_tasks);
                let outside = data
                    .get("task_id")
                    .is_none_or(|t| !ferb_py::value::truthy(t));
                if ours || (topic.starts_with("action.") && outside) {
                    let mut wrapper = stamp(json!({"type": "event", "topic": topic, "data": data}));
                    if !ours {
                        // an observation outside any task goes to every open chat: it is about no message
                        wrapper["request_id"] = Value::Null;
                    }
                    let _ = out.send(wrapper);
                }
                return;
            }
            let ev = stamp(ev);
            if let Some(t) = ev.get("task_id").filter(|t| ferb_py::value::truthy(t)) {
                my_tasks.insert(ferb_py::value::py_str(t));
            }
            if ev.get("type").and_then(Value::as_str) == Some("outcome") {
                self.task_result(&ev, true);
            }
            let _ = out.send(ev);
        };
        // the daemon's shutdown cancels the turn where it is (main.py's drain): nothing more is sent, a running tree
        // finishes its own clean-up (run_tree), and the stream ends
        let result = loop {
            if self.shutting_down_now() {
                break None;
            }
            tokio::select! {
                biased;
                _ = self.shutting_down() => break None,
                Some(ev) = rx.recv() => {
                    let mut batch = vec![ev];
                    while let Ok(ev) = rx.try_recv() {
                        batch.push(ev);
                    }
                    for ev in as_python_wakes(batch) {
                        take(ev, &mut my_tasks);
                    }
                }
                r = &mut run => break Some(r),
            }
        };
        let Some(result) = result else {
            if self.runs_tree_for(&request_id) {
                let _ = run.await;
            }
            if let Some(leave) = unsubscribe {
                leave();
            }
            return;
        };
        let mut rest = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            rest.push(ev);
        }
        for ev in as_python_wakes(rest) {
            take(ev, &mut my_tasks);
        }
        if let Some(leave) = unsubscribe {
            leave();
        }
        let my_tasks = !my_tasks.is_empty();
        match result {
            Ok(()) | Err(Failure::Cancelled) => {}
            Err(failure) => {
                let why = format!(
                    "{}: {}",
                    failure.class(),
                    self.redact_text(&failure.message())
                );
                self.journal(
                    "chat.failed",
                    json!({"request_id": request_id, "why": why}),
                    None,
                );
                let err = ferb_worker::Error::other(&failure.class(), failure.message());
                let route = || self.deps.daemon.has_route();
                let said = self.redact_text(&ferb_worker::errors::couldnt(
                    if my_tasks {
                        "finish that"
                    } else {
                        "start that"
                    },
                    &err,
                    Some(&|t| self.redact_text(t)),
                    &route,
                ));
                self.journal(
                    "chat.reply",
                    json!({"session_id": session, "request_id": request_id, "text": said}),
                    Some(ferb_sanitize::origin::FERB),
                );
                let _ = out.send(stamp(
                    json!({"type": "reply", "text": said, "status": "failed"}),
                ));
            }
        }
    }

    /// A tree of this request is running (its run takes the shutdown's path itself).
    fn runs_tree_for(&self, request_id: &str) -> bool {
        self.live()
            .tree_of
            .values()
            .any(|t| t.worker.request_id == request_id)
    }

    /// `tid in my_tasks or self.tasks.tree_of(str(tid)) in my_tasks`.
    fn ours(&self, data: &Value, my_tasks: &BTreeSet<String>) -> bool {
        let tid = match data.get("task_id") {
            None | Some(Value::Null) => "None".to_string(),
            Some(v) => ferb_py::value::py_str(v),
        };
        if my_tasks.contains(&tid) {
            return true;
        }
        self.tree_of(&tid).is_some_and(|t| my_tasks.contains(&t))
    }

    /// areas/chat.py:24 `_hide_code_reply` (rule 14): a chat message while a question is open, before any model sees
    /// it: its codes are masked when a worker's question asks for a code, and a one-word reply to a password or PIN
    /// question is hidden whole; the planner's own clarifying question in this chat counts too. FERB's memory
    /// questions never ask for a secret, and a reply that is exactly an option of a waiting question is a choice.
    /// (FERB's own "which is it?" about an answer, `_answer_focus`, isn't ported: no question is displaced by it.)
    pub fn hide_code_reply(&self, text: &str, session_id: &str) -> String {
        let redactor = &self.deps.redactor;
        let open_now = self.deps.daemon.approvals();
        let choice = {
            let t = ferb_orchestrator::py::strip(text);
            ferb_orchestrator::py::strip(t.trim_end_matches(['.', '!'])).to_string()
        };
        let mut text = text.to_string();
        for ap in &open_now {
            if ferb_orchestrator::steering::MEMORY_ASKS.contains(&ap.capability.as_str()) {
                continue;
            }
            let reason = if ap.kind == "owner_login_code" {
                "login code"
            } else {
                ap.reason.as_str()
            };
            if (ap.kind == "owner_question" || ap.kind == "owner_login_code")
                && ap.option_for(&choice).is_none()
            {
                text = redactor.answer(&text, reason, true);
            }
        }
        let asked = self
            .continuity(|c| c.pending(session_id))
            .ok()
            .map(|(_, asked)| asked)
            .filter(ferb_py::value::truthy)
            .map(|a| ferb_py::value::py_str(&a));
        if !open_now.iter().any(|ap| ap.option_for(&choice).is_some()) {
            if let Some(asked) = asked {
                text = redactor.answer(&text, &asked, true);
            }
        }
        text
    }

    /// notify.py:33 `_task_result`: what FERB told the owner about a task, recorded.
    pub fn task_result(&self, ev: &Value, shown: bool) {
        let ev = self.redact(ev);
        let s = |k: &str| match ev.get(k) {
            None | Some(Value::Null) => String::new(),
            Some(v) => ferb_py::value::py_str(v),
        };
        let answer = match ev.get("answer").filter(|a| ferb_py::value::truthy(a)) {
            Some(a) => ferb_py::value::py_str(a),
            None => s("summary"),
        };
        let shown = shown || s("status") == "cancelled";
        let at = match ev.get("at").filter(|a| ferb_py::value::truthy(a)) {
            Some(a) => ferb_py::value::py_str(a),
            None => self.now_iso(),
        };
        let cards: Vec<Value> = ev
            .get("cards")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter(|c| c.is_object()).cloned().collect())
            .unwrap_or_default();
        let cards = Value::Array(cards);
        let db = self.db();
        let _ = ferb_store::task_results::TaskResults::new(&db).record(
            &s("task_id"),
            &at,
            &s("status"),
            &s("goal"),
            &answer,
            shown,
            Some(&cards),
            &s("name"),
        );
        drop(db);
        let mut published = match ev {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        published.insert("at".into(), json!(at));
        published.insert("shown".into(), json!(shown));
        self.deps
            .daemon
            .publish("task.result", Value::Object(published));
    }
}

/// The topics a chat turn takes from the bus (areas/chat.py:360).
pub const BUS_TOPICS: [&str; 3] = ["tool.called", "approval.requested", "action.*"];

/// The key a bus event carries in the turn's queue (never a key of the supervisor's own events).
const BUS: &str = "\u{0}bus";
/// The turn (its request id) whose subscription queued a bus event.
const TURN: &str = "\u{0}turn";

/// The events that became ready together (between two suspensions of the work that made them), in the order Python's
/// loop hands them over: `_chat_send` reads two queues, the turn's own events (one task) and the bus's (another), and
/// each task, once woken, takes everything queued for it. So the events of one burst come grouped by their queue,
/// the queue that got the first of them first, each group in its own order.
fn as_python_wakes(batch: Vec<Value>) -> Vec<Value> {
    let is_bus = |ev: &Value| ev.get(BUS).is_some();
    let Some(first) = batch.first().map(is_bus) else {
        return batch;
    };
    let (mut lead, rest): (Vec<Value>, Vec<Value>) =
        batch.into_iter().partition(|ev| is_bus(ev) == first);
    lead.extend(rest);
    lead
}

/// A Python value as `json.loads(json.dumps(v))` reads it.
fn py_json(p: &ferb_sanitize::Py) -> Value {
    p.to_json(false)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}
