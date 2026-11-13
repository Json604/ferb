//! The daemon's `chat.send` (ferb/daemon/areas/chat.py:322 `_chat_send`): an `ack` on receipt, journaled, before
//! anything else; then the supervisor's events for this message, each stamped with the message's request id and the
//! time it was queued (`request_id`, `at`); a task's outcome is recorded as its result (`_task_result`). What the app
//! gets for a failed turn is recorded too.
//!
//! The bus events the daemon merges into the stream for this turn's tasks (`tool.called`, `approval.requested`,
//! `action.*`) are the daemon's (ferbd) and are not merged here.

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
        let text = self.redact_text(&params.text);
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
        tokio::spawn(async move {
            me.turn_stream(session, text, paths, request_id, hidden, params.screen, tx)
                .await;
        });
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
        let mut my_tasks = false;
        let result = loop {
            tokio::select! {
                biased;
                Some(ev) = rx.recv() => {
                    let ev = stamp(ev);
                    my_tasks = my_tasks || ev.get("task_id").is_some_and(ferb_py::value::truthy);
                    if ev.get("type").and_then(Value::as_str) == Some("outcome") {
                        self.task_result(&ev, true);
                    }
                    let _ = out.send(ev);
                }
                r = &mut run => break r,
            }
        };
        while let Ok(ev) = rx.try_recv() {
            let ev = stamp(ev);
            if ev.get("type").and_then(Value::as_str) == Some("outcome") {
                self.task_result(&ev, true);
            }
            let _ = out.send(ev);
        }
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

    /// notify.py:33 `_task_result`: what FERB told the owner about a task, recorded.
    fn task_result(&self, ev: &Value, shown: bool) {
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
