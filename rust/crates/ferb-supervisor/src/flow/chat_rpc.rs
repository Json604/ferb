//! The chat RPCs (ferb/daemon/areas/chat.py): `chat.send` (its stream is [`Supervisor::chat_send`]), `chat.status`,
//! `results.ack`, `chat.approve`, `chat.answer`, `chat.correct`, `chat.attach`, `task.stop`, `tools.list`,
//! `tools.call`, `conversations.list`, `conversations.change` and `ui.click_through`, with the plumbing they share:
//! the owner's answer to a question (`_resolve_approval`), the trees a chat is attached to (`_attached_trees`, which
//! decides whether a result was shown), and a code reply hidden while a question asks for one (`_hide_code_reply`).
//! They interpret none of the owner's words: the supervisor does.
//!
//! [`register`] puts them on ferb-rpc's server as ferb-daemon's areas do; [`ChatRpc::unary`] is the same dispatch
//! without a socket. What the daemon keeps elsewhere comes in through [`ChatDeps`].

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ferb_core::interfaces::BoxFuture;
use ferb_gateway::questions::{card_flags, card_parts, Approval, ResolveError};
use ferb_orchestrator::steering::{MEMORY_ASKS, OPTION_ONLY};
use ferb_rpc::auth::RunGrant;
use ferb_rpc::server::{EventStream, HandlerError, HandlerResult, Server, Session};
use ferb_rpc::wire::Params;
use ferb_sanitize::origin;
use serde_json::{json, Map, Value};

use super::Supervisor;

/// What the chat RPCs read of the gateway (`self.gateway`, `self.state`).
pub trait ChatGateway: Send + Sync {
    /// `state.approvals.pending()`.
    fn pending(&self) -> Vec<Approval>;
    /// `state.approvals.get(id)`.
    fn approval(&self, id: &str) -> Option<Approval>;
    /// `state.approvals.resolve(id, granted, choice, text)`, then `ap.hidden = hidden` (the worker gets FERB's note
    /// beside the answer).
    fn resolve(
        &self,
        id: &str,
        granted: bool,
        choice: Option<usize>,
        text: Option<&str>,
        hidden: &[String],
    ) -> Result<Approval, ResolveError>;
    /// `gateway.tools_for(grant)`.
    fn tools_for(&self, grant: &RunGrant) -> Vec<Value>;
    /// `gateway.call(grant, name, arguments)`: the reply as the RPC carries it, or the internal error's type name.
    /// ferb-gateway's `Gateway::call` future is not `Send` (it holds ferb-policy's `confirm::Facts` across an await):
    /// the daemon runs it on the thread that owns the gateway and hands the reply back.
    fn call<'a>(
        &'a self,
        grant: &'a RunGrant,
        name: &'a str,
        arguments: &'a Map<String, Value>,
    ) -> BoxFuture<'a, Result<Value, String>>;
    /// `state.stopped`: the global stop is engaged.
    fn stopped(&self) -> bool;
}

/// `mac.report_click_through(pid, windows)`: whether the caller is one of FERB's running apps (D31).
pub type ClickThrough = Arc<dyn Fn(i32, &[i64]) -> bool + Send + Sync>;

/// What the chat RPCs use from the rest of the daemon.
pub struct ChatDeps {
    pub supervisor: Supervisor,
    pub gateway: Arc<dyn ChatGateway>,
    /// `persona.status()` (`{"state", "error"?}`); None: no persona scan (`not_started`).
    pub persona: Option<Arc<dyn Fn() -> Map<String, Value> + Send + Sync>>,
    /// `_prepare_persona()` (chat.attach).
    pub prepare_persona: Arc<dyn Fn() + Send + Sync>,
    /// `attention_notices.unseen()`; None: no notices.
    pub notices: Option<Arc<dyn Fn() -> Vec<Value> + Send + Sync>>,
    /// `supervisor.conversations.list()`: no longer read (chat.status and conversations.list read the supervisor's
    /// own store, as Python does); kept so the daemon's wiring compiles unchanged.
    pub conversations: Arc<dyn Fn() -> Vec<Value> + Send + Sync>,
    /// `sense.paused` (L34 D: the menu bar's state).
    pub sensing_paused: Arc<dyn Fn() -> bool + Send + Sync>,
    /// `_shutting_down` (an update's hold).
    pub shutting_down: Arc<dyn Fn() -> bool + Send + Sync>,
    /// `_question_answered(ap)`: what a chat forget answered with the notch's buttons says (I2), "" for nothing.
    pub question_answered: Arc<dyn Fn(&Approval) -> String + Send + Sync>,
    /// `_ask_input_monitoring()`: macOS's own prompt, once, with the owner's first request.
    pub ask_input_monitoring: Arc<dyn Fn() + Send + Sync>,
    /// `mac.report_click_through(pid, windows)`; None: no Mac hands.
    pub click_through: Option<ClickThrough>,
}

/// The chat RPCs' own state.
pub struct ChatRpc {
    deps: ChatDeps,
    /// `_attached_trees`: trees a chat is open for (their questions are asked there, not by notification).
    attached: Arc<Mutex<BTreeSet<String>>>,
}

fn refusal(message: impl Into<String>) -> HandlerError {
    HandlerError::refusal("FerbError", message)
}

/// Python's `str(params[key])` (a missing key is the KeyError Python raises: an internal error).
/// What `conversations.list {id}` answers for an id no delegated conversation has (the fix against Python's
/// internal error).
pub const NO_CONVERSATION: &str = "There's no delegated conversation with that id.";

/// areas/shared.py:21 `TASK_PAGE, TASK_PAGE_MAX`: the Tasks page's rows per page, and most per call.
pub const TASK_PAGE: i64 = 50;
pub const TASK_PAGE_MAX: i64 = 200;

fn required(params: &Params, key: &str) -> Result<String, HandlerError> {
    params
        .get(key)
        .map(ferb_py::value::py_str)
        .ok_or_else(|| HandlerError::fault("KeyError", format!("'{key}'")))
}

impl ChatRpc {
    pub fn new(deps: ChatDeps) -> Arc<ChatRpc> {
        Arc::new(ChatRpc {
            deps,
            attached: Arc::new(Mutex::new(BTreeSet::new())),
        })
    }

    fn sup(&self) -> &Supervisor {
        &self.deps.supervisor
    }

    /// `_attached_trees`, as they stand.
    pub fn attached_trees(&self) -> BTreeSet<String> {
        self.attached
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// One unary method, as the server dispatches it (the session's grant for `tools.*`).
    pub fn unary<'a>(
        self: &'a Arc<Self>,
        method: &'a str,
        session: &'a Session,
        params: Params,
    ) -> BoxFuture<'a, HandlerResult> {
        Box::pin(async move {
            match method {
                "chat.status" => Ok(self.status()),
                "results.ack" => self.results_ack(&params),
                "chat.approve" => self.approve(&params),
                "chat.answer" => self.answer(&params),
                "chat.correct" => self.correct(&params).await,
                "chat.attach" => self.attach(&params),
                "task.stop" => self.task_stop(&params),
                "tasks.list" => self.tasks_list(&params),
                "tools.list" => self.tools_list(session),
                "tools.call" => self.tools_call(session, &params).await,
                "conversations.list" => self.conversations_list(&params),
                "conversations.change" => self.conversations_change(&params),
                "ui.click_through" => self.click_through(session, &params),
                other => Err(HandlerError::fault("KeyError", format!("'{other}'"))),
            }
        })
    }

    /// areas/chat.py:59 `_chat_status`: answers no chat has shown yet, tasks still running, questions waiting for the
    /// owner. Reading never marks anything.
    pub fn status(&self) -> Params {
        let inner = self.sup().inner();
        let rows = {
            let db = inner.db();
            ferb_store::task_results::TaskResults::new(&db)
                .unshown()
                .unwrap_or_default()
        };
        let persona = match &self.deps.persona {
            Some(f) => f(),
            None => json!({"state": "not_started"})
                .as_object()
                .cloned()
                .unwrap_or_default(),
        };
        let approvals = self.deps.gateway.pending();
        let trees_of = |request_id: &str| -> BTreeSet<String> {
            let db = inner.db();
            let now = inner.now_iso();
            let trees =
                ferb_orchestrator::tree::TaskTree::new(Some(&db), Box::new(move || now.clone()))
                    .trees_of_request(request_id)
                    .unwrap_or_default();
            trees
        };
        let pending: Vec<Value> = approvals
            .iter()
            .map(|ap| {
                let trees = trees_of(&ap.request_id);
                let mut m = Map::new();
                m.insert("approval_id".into(), json!(ap.id));
                m.insert("capability".into(), json!(ap.capability));
                m.insert("kind".into(), json!(ap.kind));
                m.insert("reason".into(), json!(ap.reason));
                m.insert(
                    "task_id".into(),
                    if trees.len() == 1 {
                        json!(trees.iter().next())
                    } else {
                        Value::Null
                    },
                );
                m.insert("options".into(), json!(ap.options));
                if !ap.summary.is_empty() {
                    m.insert("summary".into(), json!(ap.summary));
                }
                if ap.from_task {
                    m.insert("from_task".into(), json!(true));
                }
                if !ap.why.is_empty() {
                    m.insert("why".into(), json!(ap.why));
                }
                m.extend(card_parts(ap));
                if !ap.task_id.is_empty() {
                    m.insert("source_task_id".into(), json!(ap.task_id));
                }
                if !ap.part.is_empty() {
                    m.insert("part".into(), json!(ap.part));
                }
                if let Some(sw) = ap.runtime_switch.as_ref().filter(|s| !s.is_empty()) {
                    m.insert("runtime_switch".into(), Value::Object(sw.clone()));
                }
                m.extend(card_flags(ap));
                Value::Object(m)
            })
            .collect();
        let running: Vec<Value> = inner
            .running_trees()
            .into_iter()
            .filter(|(t, _)| !inner.is_watch(t)) // a watch check isn't listed: no client adopts it (I3)
            .map(|(t, goal)| {
                let mut r = Map::new();
                r.insert("task_id".into(), json!(t));
                r.insert("goal".into(), json!(goal));
                r.insert("name".into(), json!(inner.tree_name(&t)));
                if let Some(started) = inner.task_column(&t, "created_at").filter(|v| !v.is_null())
                {
                    r.insert("started_at".into(), started);
                }
                if let Some(checklist) = inner.checklist_state(&t) {
                    r.insert("checklist".into(), inner.redact(&checklist));
                }
                Value::Object(r)
            })
            .collect();
        let results: Vec<Value> = rows.iter().map(result_of).collect();
        let mut persona_out = Map::new();
        persona_out.insert(
            "state".into(),
            persona.get("state").cloned().unwrap_or(Value::Null),
        );
        if let Some(e) = persona.get("error").filter(|e| ferb_py::value::truthy(e)) {
            persona_out.insert("error".into(), e.clone());
        }
        let mut out = Map::new();
        out.insert("results".into(), Value::Array(results));
        out.insert("running".into(), Value::Array(running));
        out.insert("approvals".into(), inner.redact(&Value::Array(pending)));
        out.insert("persona".into(), Value::Object(persona_out));
        out.insert(
            "notices".into(),
            Value::Array(self.deps.notices.as_ref().map(|f| f()).unwrap_or_default()),
        );
        let conversations = inner
            .conversations()
            .list(None)
            .unwrap_or_default()
            .into_iter()
            .map(Value::Object)
            .collect();
        out.insert(
            "delegated_conversations".into(),
            Value::Array(conversations),
        );
        out.insert(
            "loops".into(),
            Value::Array(self.deps.supervisor.loop_statuses()),
        );
        out.insert("paused".into(), json!((self.deps.sensing_paused)()));
        out.insert("stopped".into(), json!(self.deps.gateway.stopped()));
        out
    }

    /// areas/chat.py:119 `_results_ack`.
    pub fn results_ack(&self, params: &Params) -> HandlerResult {
        let ids: Option<Vec<&str>> = match params.get("task_ids") {
            Some(Value::Array(a)) => a
                .iter()
                .map(|t| t.as_str().filter(|s| !s.is_empty()))
                .collect(),
            _ => None,
        };
        let Some(ids) = ids else {
            // Python raises ValueError: an internal error (named in the journal by the daemon)
            return Err(HandlerError::fault(
                "ValueError",
                "task_ids must be a list of task ids",
            ));
        };
        let db = self.sup().inner().db();
        ferb_store::task_results::TaskResults::new(&db)
            .mark_shown(&ids)
            .map_err(|e| HandlerError::fault("OperationalError", e.to_string()))?;
        Ok(obj(json!({"ok": true})))
    }

    /// areas/rpc.py:53 `_tasks_list`: root tasks newest first, `limit` at a time before `before` (the `created_at` the
    /// last page ended at), each with its status, what FERB answered (its result, else its outcome) and the owner's
    /// latest verdict; a single ask's checklist count, live while it runs, else as its end recorded it (APP-894).
    pub fn tasks_list(&self, params: &Params) -> HandlerResult {
        let limit = match params.get("limit") {
            None => Some(TASK_PAGE),
            Some(Value::Number(n)) => n.as_i64().filter(|l| (1..=TASK_PAGE_MAX).contains(l)),
            Some(_) => None,
        };
        let Some(limit) = limit else {
            return Err(refusal(format!(
                "limit must be a whole number from 1 to {TASK_PAGE_MAX}"
            )));
        };
        let before = match params.get("before") {
            Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
            _ => "9999".to_string(),
        };
        let inner = self.sup().inner();
        let rows = {
            let db = inner.db();
            ferb_store::task_results::TaskResults::new(&db)
                .page(&before, limit + 1)
                .map_err(|e| HandlerError::fault("OperationalError", e.to_string()))?
        };
        let text = |r: &Map<String, Value>, k: &str| match r.get(k) {
            None | Some(Value::Null) => String::new(),
            Some(v) => ferb_py::value::py_str(v),
        };
        let mut out = Vec::new();
        for r in rows.iter().take(limit as usize) {
            let outcome: Map<String, Value> = r
                .get("outcome_json")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or_default();
            let pick = |v: Option<&Value>| v.filter(|x| ferb_py::value::truthy(x)).cloned();
            let answer = pick(r.get("answer"))
                .or_else(|| pick(outcome.get("answer")))
                .or_else(|| pick(outcome.get("summary")))
                .map(|v| ferb_py::value::py_str(&v))
                .unwrap_or_default();
            let id = text(r, "id");
            let goal = text(r, "goal");
            let name = r.get("name").and_then(Value::as_str).map(str::to_string);
            let checklist = inner
                .checklist_state(&id)
                .filter(ferb_py::value::truthy)
                .or_else(|| pick(outcome.get("checklist")));
            let mut row = obj(json!({"task_id": id, "goal": inner.redact_text(&goal),
                "name": inner.redact_text(&ferb_orchestrator::names::shown_name(name.as_deref(), &goal)),
                "status": r.get("status").cloned().unwrap_or(Value::Null),
                "created_at": r.get("created_at").cloned().unwrap_or(Value::Null),
                "updated_at": r.get("updated_at").cloned().unwrap_or(Value::Null),
                "answer": inner.redact_text(&answer),
                "verdict": r.get("verdict").cloned().unwrap_or(Value::Null)}));
            if let Some(c) = checklist {
                let (done, total) = ferb_orchestrator::plans::steps_done(&c);
                row.insert("steps".into(), json!({"done": done, "total": total}));
            }
            out.push(Value::Object(row));
        }
        Ok(obj(
            json!({"tasks": out, "more": rows.len() as i64 > limit}),
        ))
    }

    /// areas/chat.py:191 `_chat_approve`.
    pub fn approve(&self, params: &Params) -> HandlerResult {
        let id = required(params, "approval_id")?;
        let granted = params
            .get("granted")
            .map(ferb_py::value::truthy)
            .ok_or_else(|| HandlerError::fault("KeyError", "'granted'"))?;
        let choice = match params.get("choice") {
            None | Some(Value::Null) => None,
            Some(v) => Some(py_int(v)?),
        };
        self.resolve_approval(&id, granted, choice, None)
    }

    /// areas/chat.py:196 `_chat_answer`: the owner's free-text answer; empty text declines.
    pub fn answer(&self, params: &Params) -> HandlerResult {
        let text = params
            .get("text")
            .filter(|v| ferb_py::value::truthy(v))
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let id = required(params, "approval_id")?;
        let given = (!text.is_empty()).then_some(text.as_str());
        self.resolve_approval(&id, !text.is_empty(), None, given)
    }

    /// areas/chat.py:243 `_resolve_approval`: the owner's answer to a question.
    pub fn resolve_approval(
        &self,
        id: &str,
        granted: bool,
        choice: Option<i64>,
        text: Option<&str>,
    ) -> HandlerResult {
        let inner = self.sup().inner();
        let redactor = &inner.deps.redactor;
        let ap = self.deps.gateway.approval(id);
        let mut granted = granted;
        let mut hidden: Vec<String> = Vec::new();
        let sealed = ap
            .as_ref()
            .is_some_and(|a| a.kind == "owner_admin_password" || a.kind == "owner_secret")
            && text.is_some()
            && granted;
        let mut text = text.map(str::to_string);
        if let (Some(t), true) = (text.clone(), granted) {
            if sealed {
                if t.contains(['\r', '\n', '\0']) {
                    return Err(refusal("the hidden password must be one line"));
                }
            } else {
                let t = ferb_orchestrator::py::strip(&t).to_string();
                granted = !t.is_empty();
                let mut t2 = t.clone();
                if let Some(ap) = &ap {
                    let options_only =
                        OPTION_ONLY.contains(&ap.capability.as_str()) || ap.options_only;
                    if options_only && !t.is_empty() && ap.option_for(&t).is_none() {
                        return Err(refusal(format!(
                            "Pick {}.",
                            ferb_orchestrator::lines::choices(&ap.options)
                        )));
                    }
                    if ap.kind == "owner_question" && ap.option_for(&t).is_none() {
                        hidden = redactor
                            .kinds(&t)
                            .unwrap_or_default()
                            .into_iter()
                            .map(str::to_string)
                            .collect();
                        t2 = if MEMORY_ASKS.contains(&ap.capability.as_str()) {
                            redactor.text(&t)
                        } else {
                            redactor.answer(&t, &ap.reason, false)
                        };
                    }
                }
                text = Some(t2);
            }
        }
        let choice = match choice {
            None => None,
            Some(c) if c >= 1 => Some(c as usize),
            Some(c) => {
                return Err(HandlerError::fault(
                    "ValueError",
                    format!("choice {c} is not one of the options"),
                ))
            }
        };
        let resolved = self
            .deps
            .gateway
            .resolve(id, granted, choice, text.as_deref(), &hidden);
        let ap = match resolved {
            Ok(ap) => ap,
            Err(e) => {
                let (class, message) = match e {
                    ResolveError::Unknown(m) => ("KeyError", m),
                    ResolveError::Invalid(m) => ("ValueError", m),
                };
                if sealed {
                    // the answer isn't known by value (GATE-572): its failure is recorded without its words
                    inner.journal(
                        "daemon.internal_error",
                        json!({"method": "chat.answer", "error": class, "withheld": true}),
                        None,
                    );
                }
                return Err(HandlerError::fault(class, message));
            }
        };
        let followed = (self.deps.question_answered)(&ap);
        if !followed.is_empty() && ap.capability == "memory.forget" {
            inner.deps.daemon.publish(
                "memory.forgot",
                json!({"approval_id": ap.id, "text": followed}),
            );
        }
        inner.journal(
            "approval.resolved",
            json!({"approval_id": ap.id, "state": ap.state.as_str(), "capability": ap.capability}),
            None,
        );
        if !hidden.is_empty() {
            inner.journal(
                "chat.secret_hidden",
                json!({"approval_id": ap.id, "request_id": ap.request_id, "kinds": hidden}),
                None,
            );
        }
        inner.deps.daemon.publish(
            "approval.resolved",
            json!({"approval_id": ap.id, "state": ap.state.as_str()}),
        );
        let mut out = obj(json!({"approval_id": ap.id, "state": ap.state.as_str()}));
        if !hidden.is_empty() {
            out.insert("hidden".into(), json!(hidden));
        }
        Ok(out)
    }

    /// areas/chat.py:201 `_chat_correct` (GATE-510): a task's safety-check card's box. The owner's exact option words
    /// are that option; any other words are the owner's correction of that check's task.
    pub async fn correct(&self, params: &Params) -> HandlerResult {
        let id = params
            .get("approval_id")
            .filter(|v| ferb_py::value::truthy(v))
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let raw = params
            .get("text")
            .filter(|v| ferb_py::value::truthy(v))
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let raw = ferb_orchestrator::py::strip(&raw).to_string();
        let ap = self.deps.gateway.approval(&id);
        let Some(ap) = ap.filter(|a| a.state == ferb_gateway::questions::State::Pending) else {
            return Err(refusal(
                "That question isn't waiting any more, so nothing was sent to the task.",
            ));
        };
        if !ap.corrects || ap.task_id.is_empty() {
            return Err(refusal(
                "This question isn't part of a task, so it takes only Yes or No.",
            ));
        }
        if raw.is_empty() {
            return Err(refusal(
                "Type what the task should do instead, or pick one of the answers.",
            ));
        }
        if ap.option_for(&raw).is_some() {
            return self.resolve_approval(&id, true, None, Some(&raw));
        }
        let inner = self.sup().inner();
        let redactor = &inner.deps.redactor;
        // the kinds masked below, by name: the notice and the fact line
        let hidden: Vec<String> = redactor
            .kinds(&raw)
            .unwrap_or_default()
            .into_iter()
            .map(str::to_string)
            .collect();
        let tree = inner
            .tree_of(&ap.task_id)
            .unwrap_or_else(|| ap.task_id.clone());
        let session_id = inner
            .continuity(|c| c.conversation_of(&tree))
            .ok()
            .flatten()
            .unwrap_or_default();
        let text = inner.hide_code_reply(&inner.redact_text(&raw), &session_id);
        let told = self
            .sup()
            .correct_from_card(&id, &text, &hidden)
            .await
            .map_err(refusal)?;
        let field = |k: &str| told.get(k).cloned().unwrap_or(Value::Null);
        if !hidden.is_empty() {
            inner.journal(
                "chat.secret_hidden",
                json!({"session_id": session_id, "request_id": field("request_id"), "kinds": hidden}),
                None,
            );
        }
        inner.journal(
            "approval.corrected",
            json!({"approval_id": id, "task_id": field("task_id"), "state": field("state")}),
            None,
        );
        let mut out = obj(
            json!({"approval_id": id, "state": field("state"), "task_id": field("task_id"),
            "message": ferb_sanitize::untrusted::mark(&text).for_model, "reply": field("text")}),
        );
        if !hidden.is_empty() {
            out.insert("hidden".into(), json!(hidden));
        }
        Ok(out)
    }

    /// areas/chat.py:159 `_chat_attach`: chat is open again.
    pub fn attach(&self, params: &Params) -> HandlerResult {
        let inner = self.sup().inner();
        {
            let mut attached = self.attached.lock().unwrap_or_else(|e| e.into_inner());
            for (t, _) in inner.running_trees() {
                attached.insert(t);
            }
        }
        let new = params.get("new").is_some_and(ferb_py::value::truthy);
        let resume = params
            .get("resume")
            .filter(|v| !v.is_null())
            .map(ferb_py::value::py_str);
        let conversation = inner
            .continuity(|c| c.attach(new, resume.as_deref()))
            .map_err(|e| HandlerError::fault("ValueError", e.to_string()))?;
        (self.deps.prepare_persona)();
        Ok(obj(
            json!({"ok": true, "session_id": conversation, "capabilities": ["input_paths"]}),
        ))
    }

    /// areas/chat.py:168 `_task_stop` (the stop itself is control and recovery's `stop_tree`).
    pub fn task_stop(&self, params: &Params) -> HandlerResult {
        match self.sup().inner().task_stop(params) {
            Ok(v) => Ok(obj(v)),
            Err(m) => Err(refusal(m)),
        }
    }

    /// areas/chat.py:181 `_tools_list`.
    pub fn tools_list(&self, session: &Session) -> HandlerResult {
        let Some(grant) = &session.grant else {
            return Err(refusal("no run token"));
        };
        Ok(obj(json!({"tools": self.deps.gateway.tools_for(grant)})))
    }

    /// areas/chat.py:186 `_tools_call`: the tool bridge's call, with the run token's grant.
    pub async fn tools_call(&self, session: &Session, params: &Params) -> HandlerResult {
        let Some(grant) = session.grant.clone() else {
            return Err(refusal("no run token"));
        };
        let name = match params.get("name") {
            None => "None".to_string(),
            Some(v) => ferb_py::value::py_str(v),
        };
        let arguments = match params.get("arguments") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        match self.deps.gateway.call(&grant, &name, &arguments).await {
            Ok(reply) => Ok(obj(reply)),
            Err(class) => Err(HandlerError::fault(&class, "")),
        }
    }

    /// areas/chat.py:135 `_conversations_list`: the delegated conversations (the supervisor's store), or one's digest
    /// by `id`.
    ///
    /// Fixed against Python (ENG-6): an id no conversation has raised ValueError inside `digest`, an internal error
    /// on the wire (-32603 "ValueError"); here it is refused in plain words.
    pub fn conversations_list(&self, params: &Params) -> HandlerResult {
        let store = self.sup().inner().conversations();
        match params.get("id").filter(|v| ferb_py::value::truthy(v)) {
            None => {
                let rows = store.list(None).map_err(|e| refusal(e.to_string()))?;
                Ok(obj(
                    json!({"conversations": rows.into_iter().map(Value::Object).collect::<Vec<_>>()}),
                ))
            }
            Some(id) => match store.digest(&ferb_py::value::py_str(id)) {
                Ok(digest) => Ok(obj(digest)),
                Err(ferb_orchestrator::sql::Error::Value(_)) => Err(refusal(NO_CONVERSATION)),
                Err(e) => Err(refusal(e.to_string())),
            },
        }
    }

    /// areas/chat.py:140 `_conversations_change`: the owner pauses, resumes, takes over or closes a delegated
    /// conversation (`change_conversation`), journaled with the owner as its origin; the conversation as it is now.
    ///
    /// Fixed against Python (ENG-6), as `conversations.list`: a ValueError (an unknown id, an unsupported action, a
    /// resume while FERB is stopped) left the handler as an internal error; here it is refused in its own words.
    pub fn conversations_change(&self, params: &Params) -> HandlerResult {
        let id = required(params, "id")?;
        let action = required(params, "action")?;
        let words = required(params, "owner_words")?;
        let inner = self.sup().inner();
        inner
            .change_conversation(&id, &action, &words)
            .map_err(refusal)?;
        let redacted = inner.deps.redactor.value(&Value::Object(params.clone()));
        inner.journal("conversation.owner_change", redacted, Some(origin::OWNER));
        let row = inner
            .conversations()
            .get(&id)
            .map_err(|e| refusal(e.to_string()))?;
        Ok(obj(json!({"conversation": row})))
    }

    /// areas/chat.py:146 `_ui_click_through` (D31): the notch's click-through windows, from FERB's own app only.
    pub fn click_through(&self, session: &Session, params: &Params) -> HandlerResult {
        let windows: Option<Vec<i64>> = match params.get("windows") {
            Some(Value::Array(a)) => a
                .iter()
                .map(|w| w.as_i64().filter(|n| *n > 0 && !w.is_boolean()))
                .collect(),
            _ => None,
        };
        let Some(windows) = windows else {
            return Err(HandlerError::refusal(
                "Refused",
                "windows must be a list of window numbers",
            ));
        };
        let reported = match (&self.deps.click_through, session.pid) {
            (Some(report), Some(pid)) => report(pid, &windows),
            _ => false,
        };
        if !reported {
            return Err(HandlerError::refusal(
                "Refused",
                "only FERB's own app reports its click-through windows",
            ));
        }
        Ok(obj(json!({"windows": windows.len()})))
    }

    /// areas/chat.py:322 `_chat_send`: the stream. A refusal before the ack is its error.
    pub fn chat_send(
        &self,
        params: &Params,
    ) -> Result<tokio::sync::mpsc::UnboundedReceiver<Value>, HandlerError> {
        if (self.deps.shutting_down)() {
            return Err(refusal(
                "daemon is shutting down; no new chat task was started",
            ));
        }
        (self.deps.ask_input_monitoring)();
        self.sup()
            .chat_send_params(params)
            .map_err(|r| refusal(r.0))
    }
}

/// One unshown result as `chat.status` lists it.
fn result_of(row: &Map<String, Value>) -> Value {
    let recorded: Map<String, Value> = row
        .get("outcome_json")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or_default();
    let mut out = Map::new();
    for key in ["task_id", "request_id", "at", "status", "goal", "answer"] {
        out.insert(key.into(), row.get(key).cloned().unwrap_or(Value::Null));
    }
    let name = row.get("name").and_then(Value::as_str);
    let goal = row
        .get("goal")
        .filter(|g| ferb_py::value::truthy(g))
        .map(ferb_py::value::py_str)
        .unwrap_or_default();
    out.insert(
        "name".into(),
        json!(ferb_orchestrator::names::shown_name(name, &goal)),
    );
    out.insert(
        "artifacts".into(),
        recorded.get("artifacts").cloned().unwrap_or(json!([])),
    );
    out.insert(
        "verified".into(),
        recorded.get("verified").cloned().unwrap_or(Value::Null),
    );
    out.insert(
        "verification".into(),
        recorded.get("verification").cloned().unwrap_or(json!([])),
    );
    for key in ["card", "note", "lines", "runtime_switch", "checklist"] {
        if let Some(v) = recorded.get(key).filter(|v| ferb_py::value::truthy(v)) {
            out.insert(key.into(), v.clone());
        }
    }
    if let Some(cards) = row
        .get("cards_json")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
    {
        out.insert(
            "cards".into(),
            serde_json::from_str(cards).unwrap_or(Value::Null),
        );
    }
    Value::Object(out)
}

fn obj(v: Value) -> Params {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// Python's `int(v)` on a decoded JSON value (a bool is 0 or 1; text is parsed).
fn py_int(v: &Value) -> Result<i64, HandlerError> {
    match v {
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f.trunc() as i64))
            .ok_or_else(|| HandlerError::fault("ValueError", "invalid int")),
        Value::String(s) => ferb_orchestrator::py::strip(s).parse().map_err(|_| {
            HandlerError::fault(
                "ValueError",
                format!("invalid literal for int() with base 10: '{s}'"),
            )
        }),
        _ => Err(HandlerError::fault("TypeError", "int() argument")),
    }
}

/// `chat.send`'s events as ferb-rpc's stream reads them. A task the turn names is attached to this chat while the
/// chat reads it, until its outcome (`_attached_trees`); when the chat leaves, its tasks are no longer attached.
struct ChatEvents {
    rx: tokio::sync::mpsc::UnboundedReceiver<Value>,
    attached: Arc<Mutex<BTreeSet<String>>>,
    mine: BTreeSet<String>,
}

impl EventStream for ChatEvents {
    fn next(&mut self) -> BoxFuture<'_, Option<HandlerResult>> {
        Box::pin(async move {
            let ev = self.rx.recv().await?;
            if let Some(t) = ev.get("task_id").filter(|t| ferb_py::value::truthy(t)) {
                let t = ferb_py::value::py_str(t);
                let mut attached = self.attached.lock().unwrap_or_else(|e| e.into_inner());
                if ev.get("type").and_then(Value::as_str) == Some("outcome") {
                    attached.remove(&t);
                } else if ev.get("type").and_then(Value::as_str) != Some("event") {
                    attached.insert(t.clone());
                    self.mine.insert(t);
                }
            }
            Some(Ok(obj(ev)))
        })
    }
}

impl Drop for ChatEvents {
    fn drop(&mut self) {
        // chat closed mid-task: never stops it
        let mut attached = self.attached.lock().unwrap_or_else(|e| e.into_inner());
        for t in &self.mine {
            attached.remove(t);
        }
    }
}

/// A stream that ends with its one error.
struct Failed(Option<HandlerError>);

impl EventStream for Failed {
    fn next(&mut self) -> BoxFuture<'_, Option<HandlerResult>> {
        Box::pin(async move { self.0.take().map(Err) })
    }
}

/// The chat RPCs on the daemon's server (rpc.py's table for areas/chat.py), as ferb-daemon's areas register theirs.
pub fn register(server: &Server, rpc: Arc<ChatRpc>) {
    for method in [
        "chat.status",
        "results.ack",
        "chat.approve",
        "chat.answer",
        "chat.correct",
        "chat.attach",
        "task.stop",
        "tasks.list",
        "tools.list",
        "tools.call",
        "conversations.list",
        "conversations.change",
        "ui.click_through",
    ] {
        let rpc = rpc.clone();
        server.register_unary(method, move |session: Session, params: Params| {
            let rpc = rpc.clone();
            async move { rpc.unary(method, &session, params).await }
        });
    }
    let send = rpc.clone();
    server.register_stream(
        "chat.send",
        move |_session: Session, params: Params| -> Box<dyn EventStream> {
            match send.chat_send(&params) {
                Ok(rx) => Box::new(ChatEvents {
                    rx,
                    attached: send.attached.clone(),
                    mine: BTreeSet::new(),
                }),
                Err(e) => Box::new(Failed(Some(e))),
            }
        },
    );
}
