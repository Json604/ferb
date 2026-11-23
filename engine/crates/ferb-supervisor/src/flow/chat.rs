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
    /// Stage 2 (ENG-10): the client that sends, and its own id for this submission (a repeat returns the same
    /// acceptance). Absent: the stream's bytes are exactly as before.
    pub client_id: Option<String>,
    pub submission_id: Option<String>,
    /// ORCH-699: the message goes to the temporary chat (started if off): no prior context, nothing kept.
    pub fresh: bool,
    /// ORCH-699: what the owner gives the temporary chat to start from (with `fresh` only; none for now).
    pub extra_context: Option<String>,
}

/// The longest `client_id` or `submission_id` (characters).
pub const CLIENT_ID_MAX: usize = 200;

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
        let id = |k: &str| -> Result<Option<String>, Refused> {
            match params.get(k) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(s))
                    if !s.trim().is_empty() && s.chars().count() <= CLIENT_ID_MAX =>
                {
                    Ok(Some(s.clone()))
                }
                Some(_) => Err(Refused(format!(
                    "{k} must be a short text of at most {CLIENT_ID_MAX} characters"
                ))),
            }
        };
        let client_id = id("client_id")?;
        let submission_id = id("submission_id")?;
        if submission_id.is_some() && client_id.is_none() {
            return Err(Refused(
                "submission_id needs the client_id it belongs to".into(),
            ));
        }
        // decision: ORCH-699 — `fresh` (a bool when given, never a default from another type) and its optional
        // `extra_context`
        let fresh = match params.get("fresh") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(Refused("fresh must be true or false".into())),
        };
        let extra_context = match params.get("extra_context") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return Err(Refused("extra_context must be text".into())),
        };
        // ENG-10: a client naming itself and no conversation writes in the current one (the phones send no session);
        // a caller without a client id keeps today's default
        let session_id = match (params.get("session_id"), &client_id) {
            (None | Some(Value::Null), Some(_)) => self
                .inner
                .continuity(|c| c.current())
                .unwrap_or_else(|_| "owner".into()),
            _ => text("session_id", "default"),
        };
        self.chat_send(SendParams {
            session_id,
            text: text("text", ""),
            input_paths: paths,
            // L12: captured as it arrives, when the daemon captures at send
            screen: self.inner.deps.daemon.captures_at_send(),
            client_id,
            submission_id,
            fresh,
            extra_context,
        })
    }
}

impl Supervisor {
    /// Stage 2: once at start, after `recover_after_restart` and before the daemon serves: the chat entries' journal
    /// backstop and start-up pass (`resuming`: the trees that will run again; chat_log.rs).
    pub fn chat_entries_after_restart(&self, resuming: &BTreeSet<String>) {
        self.inner.chat_log.after_restart(&self.inner, resuming);
        self.inner.chat_log.start(&self.inner);
    }

    /// Stage 2: a bus event the chat's entries read (chat_log.rs `TOPICS`), as the daemon's events hand it over (redacted),
    /// queued in the order it was published. The daemon registers it after its other consumers.
    pub fn chat_bus(&self, topic: &str, data: Value) {
        // decision: ORCH-699 — an event about a temporary chat's message or task goes to its memory, never the kept chat
        if super::chat_log::TOPICS.contains(&topic) && self.inner.temporary_event(&data) {
            // an answer carries what its record keeps (how, the choice), as the kept chat's question gets it
            let data = if topic == "approval.resolved" {
                let db = self.inner.db();
                super::chat_log::answered(db.conn(), &data).unwrap_or(data)
            } else {
                data
            };
            self.inner
                .temporary_feed(super::entries::Input::Bus { topic, data: &data });
            return;
        }
        if super::chat_log::TOPICS.contains(&topic) {
            self.inner.chat_log.bus(&self.inner, topic, data);
        }
    }

    /// Stage 2: a reminder's banner (its notifications item), posted in the current conversation once.
    pub fn chat_post(&self, item: Value) {
        self.inner.chat_log.post(&self.inner, item);
    }

    /// MEM-461: notices already in the chat folded into another place (the review of what an import brought in).
    pub fn fold_notices(&self, refs: &[String], into: &str) -> usize {
        self.inner.chat_log.fold_notices(&self.inner, refs, into)
    }

    /// Stage 2: the chat's entries as kept (chat_log.rs).
    pub fn chat_log(&self) -> &super::chat_log::ChatLog {
        &self.inner.chat_log
    }

    /// Everything the chat's entries were told, written now (tests, shutdown).
    pub fn chat_flush(&self) {
        self.inner.chat_log.flush(&self.inner);
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
        if session.starts_with("schedule:")
            || session.starts_with("preview:")
            || ferb_core::temporary::is_session(&session)
        {
            return Err(Refused(
                "session_id names one of FERB's own sessions, not a chat".into(),
            ));
        }
        // decision: ORCH-699 — a fresh message is the temporary chat's: its conversation and a request id that carries
        // the mark (ferb-core temporary), so every route reads the switch from the id
        let (session, request_id) = if params.fresh {
            self.temporary_accept(params.extra_context.as_deref())
        } else {
            (session, self.new_id())
        };
        let hidden: Vec<String> = self
            .deps
            .redactor
            .kinds(&params.text)
            .unwrap_or_default()
            .into_iter()
            .map(str::to_string)
            .collect();
        let redacted = self.redact_text(&params.text);
        let text = self.hide_code_reply(&redacted, &session);
        // GATE-604 (owner GATE-601): each identifier of the owner's own words is kept sealed for any site, so FERB
        // never asks for it again; the text stays masked, and the model learns only the names
        let sealed = self.seal_identifiers(&params.text);
        let (tx, rx) = mpsc::unbounded_channel();
        // journal.py `append` as `self.journal` writes it (its id, then its time), on the ack's own transaction
        let data = self.redact(
            &json!({"session_id": session, "request_id": request_id, "received_at": received}),
        );
        let (row_id, at) = (self.new_id(), self.now_iso());
        let message = ferb_orchestrator::py::strip(&mark(&text).for_model).to_string();
        let mut ack = Map::new();
        ack.insert("type".into(), json!("ack"));
        ack.insert("request_id".into(), json!(request_id));
        ack.insert("message".into(), json!(message));
        ack.insert("at".into(), json!(at));
        if !hidden.is_empty() {
            ack.insert("hidden".into(), json!(hidden));
        }
        if params.fresh {
            ack.insert("fresh".into(), json!(true));
            ack.insert("conversation".into(), json!(session));
        }
        if !sealed.is_empty() {
            let kept: Vec<Value> = sealed
                .iter()
                .map(|(kind, name)| json!({"kind": kind, "name": name}))
                .collect();
            ack.insert("sealed".into(), Value::Array(kept));
        }
        // stage 2: the message entry, in ONE transaction with the chat.ack row and the client's submission (ENG-10)
        let sha = ferb_store::chat_entries::text_sha256(&redacted);
        let submission = match (&params.client_id, &params.submission_id) {
            (Some(c), Some(s)) => Some((c.as_str(), s.as_str(), sha.as_str())),
            _ => None,
        };
        if params.fresh {
            // the temporary chat: its message entry in memory only (never chat_entries, never a submission row); the
            // chat.ack row (ids and time, no words) is kept like every message's: an audit row (rule 6)
            let _ =
                super::store::journal_row(self.db().conn(), &row_id, &at, "chat.ack", &data, None);
            let mut frame = Value::Object(ack.clone());
            frame["input_paths"] = json!(params.input_paths);
            if let Some(client) = &params.client_id {
                frame["client_id"] = json!(client);
            }
            self.temporary_feed(super::entries::Input::Frame {
                request_id: &request_id,
                frame: &frame,
            });
        }
        let accepted = if params.fresh {
            Ok(Ok(super::chat_log::Accepted::default()))
        } else {
            self.chat_log.accept(
                self,
                &session,
                &request_id,
                &Value::Object(ack.clone()),
                // the files as the client sent them (what FERB.app's turn keeps)
                &params.input_paths,
                params.client_id.as_deref(),
                submission,
                |conn| {
                    super::store::journal_row(conn, &row_id, &at, "chat.ack", &data, None)
                        .map(|_| ())
                },
            )
        };
        match accepted {
            Ok(Ok(a)) => {
                if params.client_id.is_some() && !params.fresh {
                    ack.insert("entry_id".into(), json!(a.entry_id));
                    ack.insert("seq".into(), json!(a.seq));
                    ack.insert("epoch".into(), json!(a.epoch));
                    if let Some(s) = &params.submission_id {
                        ack.insert("submission_id".into(), json!(s));
                    }
                }
            }
            Ok(Err(super::chat_log::Repeat::Same(done))) => {
                // the same submission again: the same acceptance, and no second turn
                let _ = tx.send(self.repeated_ack(&done));
                return Ok(rx);
            }
            Ok(Err(super::chat_log::Repeat::Conflict)) => {
                return Err(Refused(
                    "that submission_id was already used for a different message".into(),
                ));
            }
            Err(_) => {
                // the entries could not be written: the journal row as before (the start-up backstop makes the
                // message's entry from it)
                let _ = super::store::journal_row(
                    self.db().conn(),
                    &row_id,
                    &at,
                    "chat.ack",
                    &data,
                    None,
                );
            }
        }
        if !hidden.is_empty() {
            self.journal(
                "chat.secret_hidden",
                json!({"session_id": session, "request_id": request_id, "kinds": hidden}),
                None,
            );
        }
        if !sealed.is_empty() {
            let names: Vec<&str> = sealed.iter().map(|(_, name)| name.as_str()).collect();
            self.journal(
                "identifier.sealed",
                json!({"session_id": session, "request_id": request_id, "names": names}),
                None,
            );
        }
        let _ = tx.send(Value::Object(ack));
        // SENSE-424: where the message was sent from, by its own client (a phone by the name the Mac knows it by)
        let sent_from = crate::words::sent_from(params.client_id.as_deref(), |endpoint| {
            ferb_store::phones::by_endpoint(self.db().conn(), endpoint)
                .ok()
                .flatten()
                .map(|p| p.name)
        });
        // decision: ORCH-704 — a temporary chat's message is captured at send as any message is (the screen is the
        // present, not memory): its front window and screenshot reach its planner and workers, and are kept only where
        // any message's capture is (the evidence store, the traces), never in a chat entry
        let screen = params.screen;
        let me = self.me();
        let job = tokio::spawn(async move {
            me.turn_stream(
                session,
                text,
                paths,
                request_id,
                (hidden, sealed),
                (screen, sent_from),
                tx,
            )
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
        (hidden, sealed): (Vec<String>, Vec<(String, String)>),
        (screen, sent_from): (bool, Option<String>),
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
                (hidden, sealed),
                (screen, sent_from),
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
                    self.log_frame(&session, &request_id, &wrapper);
                    let _ = out.send(wrapper);
                }
                return;
            }
            let ev = stamp(ev);
            if let Some(t) = ev.get("task_id").filter(|t| ferb_py::value::truthy(t)) {
                my_tasks.insert(ferb_py::value::py_str(t));
            }
            // stage 2: the entries are fed the frame on the stream's own path, just before the stream gets it
            self.log_frame(&session, &request_id, &ev);
            if ev.get("type").and_then(Value::as_str) == Some("outcome") {
                let published = self.task_result(&ev, true);
                if !ferb_core::temporary::is_session(&session) {
                    self.chat_log.stream_result(&self, &published);
                }
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
            self.log_ended(&session, &request_id);
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
                let frame = stamp(json!({"type": "reply", "text": said, "status": "failed"}));
                self.log_frame(&session, &request_id, &frame);
                let _ = out.send(frame);
            }
        }
        self.log_ended(&session, &request_id);
    }

    /// A stream's frame to the conversation's entries: the kept chat's log, or the temporary chat's memory (ORCH-699).
    fn log_frame(self: &Arc<Self>, session: &str, request_id: &str, frame: &Value) {
        if ferb_core::temporary::is_session(session) {
            self.temporary_feed(super::entries::Input::Frame { request_id, frame });
        } else {
            self.chat_log.frame(self, session, request_id, frame);
        }
    }

    /// A stream's end, as [`Inner::log_frame`] routes it.
    fn log_ended(self: &Arc<Self>, session: &str, request_id: &str) {
        if ferb_core::temporary::is_session(session) {
            self.temporary_feed(super::entries::Input::StreamEnded { request_id });
        } else {
            self.chat_log.ended(self, session, request_id);
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

    /// GATE-604: the identifiers of the owner's own message (`text` as typed or pasted, never what FERB read), each
    /// kept sealed for any site through the daemon: (kind, name), once per name, in the message's order. An
    /// identifier the owner let FERB show is no longer masked, so it is not among them.
    fn seal_identifiers(&self, text: &str) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for (a, b, kind) in self.deps.redactor.identifiers(text).unwrap_or_default() {
            if let Some(name) = self.deps.daemon.seal_identifier(kind, &text[a..b]) {
                if !out.iter().any(|(_, n)| *n == name) {
                    out.push((kind.to_string(), name));
                }
            }
        }
        out
    }

    /// areas/chat.py:24 `_hide_code_reply` (rule 14): a chat message while a question is open, before any model sees
    /// it: its codes are masked when a worker's question asks for a code, and a one-word reply to a password or PIN
    /// question is hidden whole; the planner's own clarifying question in this chat counts too. FERB's memory
    /// questions never ask for a secret, and a reply that is exactly an option of a waiting question is a choice.
    /// FERB's own "which is it?" about memory questions only asks for nothing (`asked_only_about`, L36); the planner's
    /// question it displaced still counts (`displaced_question`).
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
                if !self.asked_only_about(session_id, ferb_orchestrator::steering::MEMORY_ASKS) {
                    text = redactor.answer(&text, &asked, true);
                }
            }
            let displaced = self.displaced_question(session_id);
            if !displaced.is_empty() {
                text = redactor.answer(&text, &displaced, true);
            }
        }
        text
    }

    /// notify.py:33 `_task_result`: what FERB told the owner about a task, recorded.
    pub fn task_result(&self, ev: &Value, shown: bool) -> Value {
        let ev = self.redact(ev);
        // decision: ORCH-699 — a temporary chat's task: its end reaches only that chat's memory; no task_results row
        // (the Tasks page, "since you last talked"), no `task.result` on the bus
        if self.temporary_event(&ev) {
            let mut published = match ev {
                Value::Object(m) => m,
                _ => Map::new(),
            };
            published.insert("shown".into(), json!(true));
            let published = Value::Object(published);
            let request = published
                .get("request_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if !self.temporary_holds(&request) {
                // a task of a chat already ended (stopped with it): its last rows go too
                self.temporary_forget();
            }
            return published;
        }
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
        let published = Value::Object(published);
        self.deps.daemon.publish("task.result", published.clone());
        published
    }

    /// A submission accepted before, said again as its ack was (stage 2, ENG-10): the same request, message entry,
    /// counter and epoch, marked a repeat; no second turn starts.
    fn repeated_ack(&self, done: &ferb_store::chat_entries::Submission) -> Value {
        let (entry, epoch) = {
            let db = self.db();
            let entry = ferb_store::chat_entries::get(db.conn(), &done.entry_id)
                .ok()
                .flatten();
            let epoch = ferb_store::chat_entries::stream(db.conn(), &done.conversation_id)
                .ok()
                .flatten()
                .map(|s| s.epoch);
            (entry, epoch)
        };
        let said: ferb_core::chat_entry::MessageBody =
            entry.as_ref().and_then(|e| e.body_as()).unwrap_or_default();
        let mut ack = Map::new();
        ack.insert("type".into(), json!("ack"));
        ack.insert("request_id".into(), json!(done.request_id));
        ack.insert("message".into(), json!(said.text));
        ack.insert("at".into(), json!(done.accepted_at));
        if !said.hidden.is_empty() {
            ack.insert("hidden".into(), json!(said.hidden));
        }
        ack.insert("entry_id".into(), json!(done.entry_id));
        ack.insert("seq".into(), json!(done.seq));
        ack.insert("epoch".into(), json!(epoch));
        ack.insert("submission_id".into(), json!(done.submission_id));
        ack.insert("repeat".into(), json!(true));
        Value::Object(ack)
    }
}

/// The topics a chat turn takes from the bus (areas/chat.py:360).
pub const BUS_TOPICS: [&str; 3] = ["tool.called", "approval.requested", "action.*"];

/// The key a bus event carries in the turn's queue (never a key of the supervisor's own events).
const BUS: &str = "\u{0}bus";
/// The turn (its request id) whose subscription queued a bus event.
const TURN: &str = "\u{0}turn";
/// The key of a wake mark on a tree's line ([`Inner::python_wakes_here`]); never sent on.
pub(crate) const WAKE: &str = "\u{0}wake";

/// The events that became ready together (between two suspensions of the work that made them), in the order Python's
/// loop hands them over: `_chat_send` reads two queues, the turn's own events (one task) and the bus's (another), and
/// each task, once woken, takes everything queued for it. So the events of one burst come grouped by their queue,
/// the queue that got the first of them first, each group in its own order.
///
/// A burst ends where the work that made it suspends in Python. The stream's own wake-ups don't mark that (the work
/// and the stream share a task, and a tree's line reaches the turn's queue one hop later), so the work marks it on its
/// line: the events before a wake mark are one burst whenever the stream takes them, and the mark goes no further.
/// What one poll of a tree's work or of a worker's tool call told comes between two marks (ferb-worker's bursts.rs,
/// `run_tree`'s forward), so a batch cut in the middle of a producer's run can't split or merge Python's bursts.
fn as_python_wakes(batch: Vec<Value>) -> Vec<Value> {
    let mut out = Vec::with_capacity(batch.len());
    let mut burst = Vec::new();
    for ev in batch {
        if ev.get(WAKE).is_some() {
            out.extend(one_burst(std::mem::take(&mut burst)));
        } else {
            burst.push(ev);
        }
    }
    out.extend(one_burst(burst));
    out
}

/// One burst's events, grouped by the queue Python took them from (the queue that got the first of them first).
fn one_burst(batch: Vec<Value>) -> Vec<Value> {
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
pub(crate) fn py_json(p: &ferb_sanitize::Py) -> Value {
    p.to_json(false)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}
