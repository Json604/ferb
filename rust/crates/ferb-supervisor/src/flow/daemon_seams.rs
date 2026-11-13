//! The supervisor plugged into the daemon's areas (feature `daemon`): ferb-daemon's group A calls the supervisor
//! through its seams (`areas_a::seams::Tasks`, `Questions`, `Owner`), as Python's areas call `self.supervisor`, the
//! gateway's `state.approvals` and `_task_result`.
//!
//! - [`SupervisorTasks`] is `Tasks`: a schedule's run is created on the daemon's transaction (`create_scheduled`) and
//!   run by the supervisor on its own runtime (`run_scheduled`), which the daemon's loop thread awaits.
//! - [`GatewayQuestions`] is `Questions`: FERB's own questions outside a task, kept in the gateway's question store
//!   (`Gateway::ask_own`), so the owner's answer reaches them as a task's does.
//! - [`ResultsOwner`] is `Owner`: a task's result is recorded and published as `_task_result` does (chat.rs), and the
//!   banner, the notice in chat and the reminder's notification go to the daemon's notifier (group B) given to it.
//! - [`DaemonSchedules`] is the supervisor's [`super::schedules_chat::SchedulesChat`] on group A's state: a schedule
//!   made, edited or changed from chat, a reminder cancelled, the schedules card.

use std::rc::Rc;
use std::sync::Arc;

use ferb_clock::schedule_store::ClaimedRun;
use ferb_clock::schedules::Schedule;
use ferb_daemon::areas_a::seams::{Ask, RunFailed};
use ferb_daemon::areas_a::{LocalFuture, Owner, Questions, State, Tasks};
use ferb_daemon::shared::Local;
use ferb_orchestrator::models::{ReminderCancel, ScheduleChange, ScheduleRequest};
use ferb_orchestrator::ports::BoxFuture;
use serde_json::{json, Map, Value};

use super::scheduled::Created;
use super::schedules_chat::{Ctx, Entry, SchedulesChat};
use super::Supervisor;

/// `Tasks` for group A: the supervisor and the task store.
pub struct SupervisorTasks {
    pub supervisor: Supervisor,
    /// The runtime the supervisor's flows run on (a scheduled run is started there; group A's loop thread waits).
    pub runtime: tokio::runtime::Handle,
}

impl Tasks for SupervisorTasks {
    fn schedule_running(&self, schedule_id: &str) -> bool {
        self.supervisor.inner().schedule_running(schedule_id)
    }

    fn create_scheduled(
        &self,
        conn: &rusqlite::Connection,
        run: &ClaimedRun,
        late: bool,
    ) -> Result<Value, String> {
        self.supervisor
            .inner()
            .create_scheduled(conn, &run.schedule.id, late, run.retry)
            .map(|c| c.to_json())
    }

    fn run_scheduled(
        &self,
        schedule: Schedule,
        created: Value,
        late: bool,
        overdue_s: f64,
        retry: bool,
    ) -> LocalFuture<Result<Map<String, Value>, RunFailed>> {
        let sup = self.supervisor.clone();
        let job = self.runtime.spawn(async move {
            let created = Created::from_json(&created);
            sup.inner()
                .run_scheduled_rest(&schedule, &created, late, overdue_s, retry)
                .await
        });
        Box::pin(async move {
            match job.await {
                Ok(Ok(out)) => Ok(out),
                Ok(Err(e)) => Err(RunFailed {
                    described: format!("{}: {}", e.class(), e.message()),
                    reason: e.message(),
                }),
                Err(e) => Err(RunFailed {
                    described: format!("JoinError: {e}"),
                    reason: e.to_string(),
                }),
            }
        })
    }

    fn turn_running(&self) -> bool {
        self.supervisor.inner().turn_running()
    }

    fn running(&self) -> bool {
        !self.supervisor.running_trees().is_empty()
    }

    fn mention_paused(&self) -> LocalFuture<()> {
        let sup = self.supervisor.clone();
        let job = self.runtime.spawn(async move {
            let _ = sup.inner().mention_paused().await;
        });
        Box::pin(async move {
            let _ = job.await;
        })
    }

    fn task_column(&self, task_id: &str, column: &str) -> Option<String> {
        self.supervisor
            .inner()
            .task_column(task_id, column)
            .filter(|v| !v.is_null())
            .map(|v| ferb_py::value::py_str(&v))
    }

    fn set_task_status(&self, task_id: &str, status: &str, outcome: &Value) -> Result<(), String> {
        self.supervisor
            .inner()
            .set_status(task_id, status, Some(outcome));
        Ok(())
    }

    fn forget_pending(&self, texts: &[String]) {
        let _ = self.supervisor.inner().forget_pending(texts);
    }

    fn forget_trail_copies(&self, trail: &std::collections::BTreeSet<String>) {
        // copied conversation evidence goes and its delegations pause (conversations.py `forget_sources`)
        let _ = self.supervisor.inner().forget_trail_copies(trail);
    }
}

/// `Questions` for group A: the gateway's question store (`Gateway::ask_own`).
pub struct GatewayQuestions(pub Arc<ferb_gateway::gateway::Gateway>);

impl Questions for GatewayQuestions {
    fn ask(&self, ask: Ask<'_>) -> Result<String, String> {
        let head = ask.kind.split(':').next().unwrap_or(ask.kind);
        let Some((capability, fixed)) = ferb_orchestrator::steering::memory_questions(head) else {
            return Err(format!("KeyError: '{head}'"));
        };
        let options: Vec<String> = match ask.options {
            Some(o) => o.to_vec(),
            None => fixed.iter().map(|s| s.to_string()).collect(),
        };
        let labels: Vec<String> = [ask.labels.0, ask.labels.1]
            .iter()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect();
        let ap = self.0.ask_own(&ferb_gateway::gateway::OwnQuestion {
            capability: capability.into(),
            reason: ask.reason.into(),
            options,
            request_id: ask.request_id.into(),
            approval_id: ask.approval_id.map(str::to_string),
            summary: ask.summary,
            detail: ask.detail,
            labels,
            ..Default::default()
        });
        Ok(ap.id)
    }

    fn supersede(&self, approval_id: &str) -> bool {
        self.0.supersede(approval_id).is_some()
    }

    fn waiting(&self, approval_id: &str) -> bool {
        self.0.waiting(approval_id)
    }
}

/// The supervisor's own task questions (a plan's ask step, a checker's ask, the check-in, a runtime offer, a paused
/// loop) in the gateway's one question store, as Python keeps them in `state.approvals`: one store holds every
/// question and one id counter numbers them. The gateway publishes nothing for them (`OwnEvent::Quiet`): the
/// supervisor publishes its own `approval.requested`, with `question_kind` and the offer.
pub struct GatewayTaskQuestions(pub Arc<ferb_gateway::gateway::Gateway>);

impl super::owner_wait::Questions for GatewayTaskQuestions {
    fn ask(&self, ask: &super::owner_wait::Ask) -> String {
        self.0
            .ask_own(&ferb_gateway::gateway::OwnQuestion {
                capability: ask.capability.clone(),
                reason: ask.question.clone(),
                options: ask.options.clone(),
                request_id: ask.request_id.clone(),
                approval_id: None,
                summary: Some(ask.summary.clone()),
                detail: Some(ask.detail.clone()),
                labels: ask.labels.clone(),
                task_id: ask.task_id.clone(),
                part: ask.part.clone(),
                runtime_switch: ask
                    .runtime_switch
                    .as_ref()
                    .and_then(|v| v.as_object().cloned()),
                options_only: ask.options_only,
                from_task: ask.from_task,
                corrects: ask.corrects,
                exact: ask.exact.clone(),
                event: ferb_gateway::gateway::OwnEvent::Quiet,
            })
            .id
    }
    fn get(&self, id: &str) -> Option<super::owner_wait::Asked> {
        self.0
            .approval(id)
            .as_ref()
            .map(super::owner_wait::asked_of)
    }
    fn supersede(&self, id: &str) -> bool {
        self.0.supersede(id).is_some()
    }
    fn pending(&self) -> Vec<super::owner_wait::Asked> {
        self.0
            .pending()
            .iter()
            .map(super::owner_wait::asked_of)
            .collect()
    }
    fn answer(&self, id: &str, words: &str) {
        let _ = self.0.resolve(id, true, None, Some(words));
    }
    fn card(&self, id: &str) -> Map<String, Value> {
        self.0.question_card(id)
    }
}

/// What the daemon's notifier does with a banner (group B's `_notify`): (title, text, category, target, when_hidden,
/// ident, card).
pub type Notify = Arc<dyn Fn(Banner) -> BoxFuture<'static, ()> + Send + Sync>;

/// One banner as `_notify` takes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Banner {
    pub title: String,
    pub text: String,
    pub category: String,
    pub target: Value,
    pub when_hidden: bool,
    pub ident: String,
    pub card: Value,
}

/// `_tell_owner(ident, refs, text, origin, cards)`: a quiet notice in chat (the notifier's).
pub type Tell = Arc<dyn Fn(&str, Vec<String>, &str, &str, Option<Value>) + Send + Sync>;

/// `Owner` for group A: `_task_result` (recorded and published here, its banner through `notify`), `_notify` and
/// `_tell_owner` (the notifier's).
pub struct ResultsOwner {
    pub supervisor: Supervisor,
    pub notify: Notify,
    /// `self.posters.connected`: a FERB.app poster is connected (it decides the banner by the notch).
    pub posters_connected: Arc<dyn Fn() -> bool + Send + Sync>,
    /// `_client_shows("task.result")`: a connected client shows results.
    pub client_shows: Arc<dyn Fn() -> bool + Send + Sync>,
    /// `_tell_owner`.
    pub tell: Tell,
}

impl Owner for ResultsOwner {
    fn task_result(&self, event: Map<String, Value>, shown: bool, alert: bool) -> LocalFuture<()> {
        // notify.py:33 `_task_result`
        let inner = self.supervisor.inner().clone();
        let ev = inner.redact(&Value::Object(event));
        let answer = match ev.get("answer").filter(|a| ferb_py::value::truthy(a)) {
            Some(a) => ferb_py::value::py_str(a),
            None => ev
                .get("summary")
                .filter(|a| ferb_py::value::truthy(a))
                .map(ferb_py::value::py_str)
                .unwrap_or_default(),
        };
        let status = ev.get("status").and_then(Value::as_str).unwrap_or("");
        let shown_now = shown || status == "cancelled";
        let task_id = ev
            .get("task_id")
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let name = ferb_orchestrator::py::strip(
            &ev.get("name")
                .filter(|n| ferb_py::value::truthy(n))
                .map(ferb_py::value::py_str)
                .unwrap_or_default(),
        )
        .to_string();
        let target = json!({"kind": "task", "task_id": task_id});
        let connected = (self.posters_connected)();
        let banner = if alert {
            Some(Banner {
                title: if name.is_empty() {
                    "FERB alert".into()
                } else {
                    name.clone()
                },
                text: answer.clone(),
                category: "alert".into(),
                target: target.clone(),
                when_hidden: false,
                ident: String::new(),
                card: Value::Null,
            })
        } else if status != "cancelled" && (connected || (!shown_now && !(self.client_shows)())) {
            Some(Banner {
                title: if !name.is_empty() {
                    name.clone()
                } else if status == "done" {
                    "FERB finished".into()
                } else {
                    "FERB".into()
                },
                text: answer.clone(),
                category: "task_finished".into(),
                target,
                when_hidden: connected,
                ident: format!("task:{task_id}"),
                card: Value::Null,
            })
        } else {
            None
        };
        let notify = self.notify.clone();
        Box::pin(async move {
            // recorded (and `task.result` published) first, as `_task_result` records before it notifies
            let mut recorded = ev.clone();
            if recorded.get("request_id").is_none() {
                // a resumed plan or scheduled run: the request its task ran for
                recorded["request_id"] = inner
                    .task_column(&task_id, "request_id")
                    .unwrap_or(Value::Null);
            }
            if let Some(b) = banner {
                notify(b).await;
            }
            inner.task_result(&recorded, shown);
        })
    }

    fn notify(
        &self,
        title: &str,
        text: &str,
        ident: &str,
        target: Value,
        card: Value,
    ) -> LocalFuture<Result<(), String>> {
        let banner = Banner {
            title: title.into(),
            text: text.into(),
            category: if ident.starts_with("reminder:") {
                "reminder".into()
            } else {
                String::new()
            },
            target,
            when_hidden: false,
            ident: ident.into(),
            card,
        };
        let notify = self.notify.clone();
        Box::pin(async move {
            notify(banner).await;
            Ok(())
        })
    }

    fn tell_owner(
        &self,
        ident: &str,
        refs: Vec<String>,
        text: &str,
        origin: &str,
        cards: Option<Value>,
    ) -> LocalFuture<()> {
        (self.tell)(ident, refs, text, origin, cards);
        Box::pin(async {})
    }
}

/// The supervisor's schedules from chat on group A's state (its stores, its journal, its timer loop's wake).
pub struct DaemonSchedules {
    pub local: Local<State>,
    /// The owner's home folder (`scope_said`'s folders) and FERB's workspace (paths normalized as a task's).
    pub home: String,
    pub workspace: String,
    /// Wait on the loop thread without yielding (a caller whose runtime must not run anything meanwhile: a replay on
    /// a paused clock); the daemon program awaits.
    pub blocking: bool,
}

impl DaemonSchedules {
    fn on<R, F>(&self, f: F) -> BoxFuture<'_, Option<R>>
    where
        R: Send + 'static,
        F: FnOnce(Rc<State>) -> R + Send + 'static,
    {
        if self.blocking {
            let out = self
                .local
                .call_blocking(move |st| async move { f(st) })
                .ok();
            return Box::pin(async move { out });
        }
        let local = self.local.clone();
        Box::pin(async move { local.call(move |st| async move { f(st) }).await.ok() })
    }
}

/// The worker type's name as the owner reads it (`scope_said`): the runtime catalog's label, else the type itself.
fn worker_label(kind: &str) -> String {
    kind.to_string()
}

/// A [`Ctx`] on group A's state: its timer and schedule stores, journal, `_book` and `_typed_behind_memory`.
pub fn with_ctx<R>(st: &State, home: &str, workspace: &str, f: impl FnOnce(&Ctx<'_>) -> R) -> R {
    let timers = st.timers();
    let schedules = st.schedules();
    let ids = st.ids.clone();
    let new_id = move || ids.new_id();
    let journal = |kind: &str, data: Value, origin: Option<&str>| {
        let _ = st.journal(kind, data, origin);
    };
    let book = |s: &Schedule, due: &ferb_clock::py::Time, after: Option<&ferb_clock::py::Time>| {
        ferb_daemon::areas_a::schedules::book(st, s, due, "", after).map_err(|e| e.message)
    };
    let typed = |request_id: &str| typed_behind_memory(st, request_id);
    let ctx = Ctx {
        timers: &timers,
        schedules: &schedules,
        clock: &*st.clock,
        new_id: &new_id,
        redactor: &st.redactor,
        journal: &journal,
        book: &book,
        typed_behind_memory: &typed,
        home,
        workspace,
        worker_label: &worker_label,
        browsing: None,
    };
    f(&ctx)
}

/// areas/schedules.py:397 `_typed_behind_memory`: what the owner typed behind each active owner-stated memory item
/// (the `memory.added` / `edited` entry it rests on, or their own words of the chat turn it was captured in), never
/// the item's content; the current request's turn and the setup persona left out.
pub fn typed_behind_memory(st: &State, request_id: &str) -> Vec<String> {
    let persona = ferb_memory::terms::slug(ferb_device_persona_key(), None);
    let Ok(items) = st.memory.items(Some("active"), None, true) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = items
        .iter()
        .filter(|i| i.origin == "owner_stated" && i.key != persona)
        .flat_map(|i| i.source_ids.iter())
        .filter_map(|s| s.strip_prefix("journal:").map(str::to_string))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    ids.sort();
    if ids.is_empty() {
        return Vec::new();
    }
    let Ok(entries) = st.journal.by_ids(&ids) else {
        return Vec::new();
    };
    let mut typed = Vec::new();
    for e in entries {
        match e.kind.as_str() {
            "memory.added" | "memory.edited" => typed.push(
                e.data
                    .get("content")
                    .map(ferb_py::value::py_str)
                    .unwrap_or_default(),
            ),
            "chat.message"
                if e.data.get("request_id").and_then(Value::as_str) != Some(request_id) =>
            {
                let parts = e.data.get("untrusted_parts").and_then(Value::as_i64);
                let attached = e
                    .data
                    .get("input_paths")
                    .is_some_and(ferb_py::value::truthy);
                let text = e
                    .data
                    .get("text")
                    .map(ferb_py::value::py_str)
                    .unwrap_or_default();
                let pasted = parts.map(|p| (p - i64::from(attached)).max(0) as usize);
                if let Some(own) = ferb_sanitize::untrusted::recorded_own_words(&text, pasted)
                    .filter(|o| !o.is_empty())
                {
                    typed.push(own);
                }
            }
            _ => {}
        }
    }
    typed
}

/// ferb/device/persona.py `PERSONA_KEY`.
fn ferb_device_persona_key() -> &'static str {
    "setup-persona"
}

impl SchedulesChat for DaemonSchedules {
    fn create<'a>(
        &'a self,
        spec: &'a ScheduleRequest,
        own_words: &'a str,
        request_id: &'a str,
        origins: &'a std::collections::BTreeSet<String>,
        request_origins: &'a std::collections::BTreeSet<String>,
    ) -> BoxFuture<'a, (String, Option<Value>)> {
        let (spec, own, rid, o, ro) = (
            spec.clone(),
            own_words.to_string(),
            request_id.to_string(),
            origins.clone(),
            request_origins.clone(),
        );
        let (home, ws) = (self.home.clone(), self.workspace.clone());
        let run = self.on(move |st| {
            with_ctx(&st, &home, &ws, |ctx| {
                super::schedules_chat::create(ctx, &spec, &own, &rid, &o, &ro)
            })
        });
        Box::pin(async move { run.await.unwrap_or_else(|| (GONE.into(), None)) })
    }

    fn edit<'a>(
        &'a self,
        ch: &'a ScheduleChange,
        own_words: &'a str,
        request_id: &'a str,
        origins: &'a std::collections::BTreeSet<String>,
        request_origins: &'a std::collections::BTreeSet<String>,
    ) -> BoxFuture<'a, (String, Option<Value>)> {
        let (ch, own, rid, o, ro) = (
            ch.clone(),
            own_words.to_string(),
            request_id.to_string(),
            origins.clone(),
            request_origins.clone(),
        );
        let (home, ws) = (self.home.clone(), self.workspace.clone());
        let run = self.on(move |st| {
            with_ctx(&st, &home, &ws, |ctx| {
                super::schedules_chat::edit(ctx, &ch, &own, &rid, &o, &ro)
            })
        });
        Box::pin(async move { run.await.unwrap_or_else(|| (GONE.into(), None)) })
    }

    fn change<'a>(&'a self, ch: &'a ScheduleChange, own_words: &'a str) -> BoxFuture<'a, String> {
        let (ch, own) = (ch.clone(), own_words.to_string());
        let run = self.on(move |st| {
            let until = ch.until.as_ref().map(|u| {
                (
                    u.quote.clone(),
                    super::schedules_chat::clock_when(&u.when()),
                )
            });
            ferb_daemon::areas_a::schedules::schedule_change_from_chat(
                &st,
                &ferb_daemon::areas_a::schedules::ChatChange {
                    id: ch.id.clone(),
                    action: ch.action.clone(),
                    quote: ch.quote.clone(),
                    name: ch.name.clone(),
                    until,
                },
                &own,
            )
            .unwrap_or_else(|e| e.message)
        });
        Box::pin(async move { run.await.unwrap_or_else(|| GONE.into()) })
    }

    fn reminder_cancel<'a>(
        &'a self,
        rc: &'a ReminderCancel,
        own_words: &'a str,
    ) -> BoxFuture<'a, String> {
        let (id, quote, own) = (rc.id.clone(), rc.quote.clone(), own_words.to_string());
        let run = self.on(move |st| {
            let rc = ferb_daemon::areas_a::schedules::ChatChange {
                id,
                quote,
                ..Default::default()
            };
            ferb_daemon::areas_a::schedules::reminder_cancel_from_chat(&st, &rc, &own)
                .unwrap_or_else(|e| e.message)
        });
        Box::pin(async move { run.await.unwrap_or_else(|| GONE.into()) })
    }

    fn problems<'a>(&'a self, entry: Entry) -> BoxFuture<'a, Vec<String>> {
        let (home, ws) = (self.home.clone(), self.workspace.clone());
        let run = self.on(move |st| {
            with_ctx(&st, &home, &ws, |ctx| {
                super::schedules_chat::problems(ctx, &entry)
            })
        });
        Box::pin(async move { run.await.unwrap_or_default() })
    }

    fn card<'a>(&'a self) -> BoxFuture<'a, Option<(String, Value)>> {
        let run = self.on(move |st| ferb_daemon::areas_a::schedules::schedules_card(&st).ok());
        Box::pin(async move { run.await.flatten() })
    }
}

/// What a schedule entry says when group A's loop has stopped.
const GONE: &str = "FERB couldn't reach its schedules just now; nothing was changed.";
