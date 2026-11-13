//! The task records the owner and the developer toolbox read (`areas/rpc.py`): `trace.get`, `verdict.set`,
//! `recovery.list`, `recovery.restore`, `reports.build`, `reports.diagnostic`, `reports.send`; FERB.app's
//! `tasks.preview` and `questions.picture`; the delegated conversations (`areas/chat.py` `conversations.list`,
//! `conversations.change`).

use std::sync::Arc;

use ferb_daemon::areas_b::reports;
use ferb_rpc::{HandlerError, Params, Server, Session};
use serde_json::{json, Map, Value};

use super::{ferb_error, ok, redact_text, Deps};
use crate::basics::lock;

/// `RESTORE_LIST` (rpc.py:30): `ferb restore` lists the newest this many records.
pub const RESTORE_LIST: i64 = 100;

type Handler =
    fn(
        Deps,
        Session,
        Params,
    ) -> futures_util::future::BoxFuture<'static, Result<Map<String, Value>, HandlerError>>;

fn unary(server: &Server, d: &Deps, method: &str, f: Handler) {
    let d = d.clone();
    server.register_unary(method, move |s: Session, p: Params| f(d.clone(), s, p));
}

pub fn register(server: &Server, d: &Deps) {
    unary(server, d, "trace.get", |d, _s, p| Box::pin(trace_get(d, p)));
    unary(server, d, "verdict.set", |d, _s, p| {
        Box::pin(verdict_set(d, p))
    });
    unary(server, d, "recovery.list", |d, _s, _p| {
        Box::pin(recovery_list(d))
    });
    unary(server, d, "recovery.restore", |d, _s, p| {
        Box::pin(recovery_restore(d, p))
    });
    unary(server, d, "reports.build", |d, _s, p| {
        Box::pin(reports_build(d, p))
    });
    unary(server, d, "reports.diagnostic", |d, s, p| {
        Box::pin(reports_diagnostic(d, s, p))
    });
    unary(server, d, "reports.send", |d, _s, p| {
        Box::pin(reports_send(d, p))
    });
    unary(server, d, "tasks.preview", |d, _s, p| {
        Box::pin(tasks_preview(d, p))
    });
    unary(server, d, "questions.picture", |d, _s, p| {
        Box::pin(questions_picture(d, p))
    });
    unary(server, d, "conversations.list", |d, _s, p| {
        Box::pin(conversations_list(d, p))
    });
    unary(server, d, "conversations.change", |d, _s, p| {
        Box::pin(conversations_change(d, p))
    });
}

fn store_fault(e: impl std::fmt::Display) -> HandlerError {
    HandlerError::fault("OperationalError", e.to_string())
}

fn tree<'a>(d: &Deps, db: &'a ferb_store::Db) -> ferb_orchestrator::tree::TaskTree<'a> {
    let c = d.clock.clone();
    ferb_orchestrator::tree::TaskTree::new(
        Some(db),
        Box::new(move || ferb_core::when::isoformat(&c.now())),
    )
}

/// `_resolve_root_task`: an exact id, or a unique root-task prefix of 6+ characters; `last` only when allowed.
pub fn resolve_root_task(
    d: &Deps,
    db: &ferb_store::Db,
    task_id: &str,
    allow_last: bool,
) -> Result<String, HandlerError> {
    let t = tree(d, db);
    if task_id == "last" && allow_last {
        return Ok(t
            .latest_root()
            .map_err(store_fault)?
            .unwrap_or_else(|| task_id.to_string()));
    }
    if task_id.chars().count() >= 6 && !t.recorded(task_id).map_err(store_fault)? {
        let matches = t.roots_with_prefix(task_id).map_err(store_fault)?;
        if matches.len() > 1 {
            return Err(ferb_error(format!(
                "ambiguous task prefix: {}",
                matches.join(", ")
            )));
        }
        if let Some(m) = matches.into_iter().next() {
            return Ok(m);
        }
    }
    Ok(task_id.to_string())
}

fn spans<'a>(d: &Deps, db: &'a ferb_store::Db) -> ferb_store::spans::SpanStore<'a> {
    let r = d.redactor.clone();
    let clock: Arc<dyn ferb_core::interfaces::Clock> = d.clock.clone();
    ferb_store::spans::SpanStore::new(
        db,
        Box::new(ferb_gateway::gateway::StoreTime(clock)),
        Box::new(ferb_gateway::gateway::StoreIds(d.ids.clone())),
        Box::new(move |v: &Value| r.value(v)),
        Box::new(|kind: &str| ferb_policy::rules::REVERSIBLE.contains(&kind)),
    )
}

/// `_trace` (rpc.py:248): the task's spans, its outcome (its recorded result when the spans hold none) and its story
/// (composed live while it runs or for one written before stories were kept; a learning call has none).
async fn trace_get(d: Deps, p: Params) -> Result<Map<String, Value>, HandlerError> {
    let given = match p.get("task_id") {
        Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
        _ => "last".into(),
    };
    let db = lock(&d.db);
    let task_id = resolve_root_task(&d, &db, &given, true)?;
    let s = spans(&d, &db);
    let spans_json = s.tree_json(&task_id).map_err(store_fault)?;
    let mut outcome = s.outcome(&task_id).map_err(store_fault)?;
    if !outcome.get("result").is_some_and(ferb_py::value::truthy) {
        if let Some(Value::String(recorded)) = tree(&d, &db)
            .column(&task_id, "outcome_json")
            .map_err(store_fault)?
        {
            if recorded != "{}" {
                outcome["result"] = ferb_py::json::loads(&recorded)
                    .map_err(|_| HandlerError::fault("JSONDecodeError", ""))?;
            }
        }
    }
    let mut episode = ferb_orchestrator::episodes::get(&db, &task_id).map_err(store_fault)?;
    if episode.is_none()
        && !spans_json.is_empty()
        && !spans_json.iter().all(|s| s["kind"] == "model.learning")
    {
        episode = Some(ferb_trace::compose::compose(
            &json!({"task_id": task_id, "spans": spans_json, "outcome": outcome}),
        ));
    }
    ok(
        json!({"task_id": task_id, "spans": spans_json, "outcome": outcome,
              "episode": episode.unwrap_or(Value::Null)}),
    )
}

/// `_verdict_set` (rpc.py:293): the owner's pass or fail on a root task, journaled; its episode card is captured
/// again (the verdict is one of the card's inputs). Acceptance is the owner's word; code never infers it.
async fn verdict_set(d: Deps, p: Params) -> Result<Map<String, Value>, HandlerError> {
    let verdict = ferb_daemon::shared::text(&p, "verdict");
    if verdict != "pass" && verdict != "fail" {
        return Err(ferb_error("verdict must be 'pass' or 'fail'"));
    }
    let given = ferb_daemon::shared::text(&p, "task_id");
    let note = ferb_daemon::shared::text(&p, "note");
    let task_id = {
        let db = lock(&d.db);
        let task_id = resolve_root_task(&d, &db, &given, false)?;
        let t = tree(&d, &db);
        if !t.recorded(&task_id).map_err(store_fault)? {
            return Err(ferb_error(format!(
                "unknown task {}",
                ferb_daemon::shared::py_repr(&Value::String(given))
            )));
        }
        if !t
            .column(&task_id, "parent_id")
            .map_err(store_fault)?
            .unwrap_or(Value::Null)
            .is_null()
        {
            let tree_id = t
                .column(&task_id, "tree_id")
                .map_err(store_fault)?
                .map(|v| ferb_py::value::py_str(&v))
                .unwrap_or_else(|| "None".into());
            return Err(ferb_error(format!(
                "task {task_id} is part of {tree_id}; give the verdict for {tree_id}"
            )));
        }
        let note = d.redactor.value(&json!({"note": note}))["note"].clone();
        ferb_store::task_results::TaskResults::new(&db)
            .record_verdict(
                &d.ids.new_id(),
                &task_id,
                &verdict,
                note.as_str().unwrap_or(""),
                &ferb_core::when::isoformat(&d.clock.now()),
            )
            .map_err(store_fault)?;
        task_id
    };
    d.journal.note(
        "owner.verdict",
        json!({"task_id": task_id, "verdict": verdict}),
    );
    // the card is read again with the verdict (L28): on group A's thread, where the learning store lives
    let tid = task_id.clone();
    let captured =
        d.a.call(move |st| async move { capture_card(&st, &tid) })
            .await
            .unwrap_or_else(|_| Err(("memory.episode_deferred", "FERB is stopping".to_string())));
    if let Err((kind, why)) = captured {
        d.journal.note(
            kind,
            json!({"task_id": task_id, "error": redact_text(&d, &why)}),
        );
    }
    ok(json!({"task_id": task_id, "verdict": verdict}))
}

/// `_capture_card(task_id, None, report=True)` (memory.py:101) on group A's learning store: a test run's task
/// learns nothing (GATE-550); the refs skipped as damaged or expired and a card built on forgotten content are
/// journaled. The card is read on this connection (the separate reader process is not part of the Rust build).
fn capture_card(
    st: &std::rc::Rc<ferb_daemon::areas_a::State>,
    task_id: &str,
) -> Result<(), (&'static str, String)> {
    let failed = |e: ferb_learning::Error| ("memory.episode_failed", e.to_string());
    if ferb_orchestrator::test_runs::TestRuns::new(st.db())
        .task(Some(task_id))
        .unwrap_or(false)
    {
        return Ok(());
    }
    st.learning.inputs_changed();
    let prepared = st.learning.prepare_episode(task_id, None).map_err(failed)?;
    if let Some(p) = &prepared {
        for (kind, key) in [
            ("memory.episode_refs_damaged", "damaged"),
            ("memory.episode_refs_expired", "expired"),
        ] {
            if p.get(key).is_some_and(ferb_py::value::truthy) {
                let _ = st.journal(kind, json!({"task_id": task_id, "refs": p[key]}), None);
            }
        }
        if p.get("forgotten").is_some_and(ferb_py::value::truthy) {
            let _ = st.journal(
                "memory.episode_skipped",
                json!({"task_id": task_id, "why": "built on forgotten content"}),
                None,
            );
        }
    }
    st.learning
        .write_episode(prepared.as_ref())
        .map_err(failed)?;
    Ok(())
}

fn recovery_root(d: &Deps) -> String {
    d.home.join("recovery").to_string_lossy().into_owned()
}

fn config_days(d: &Deps, key: &str, default: i64) -> i64 {
    let v = d
        .config
        .get("retention", key)
        .ok()
        .flatten()
        .and_then(|v| v.as_i64())
        .unwrap_or(default);
    ferb_store::retention::days(v)
}

/// `_recovery_list` (rpc.py:265): what FERB can put back, newest first, at most RESTORE_LIST of `total`.
async fn recovery_list(d: Deps) -> Result<Map<String, Value>, HandlerError> {
    let db = lock(&d.db);
    let (rows, total) = ferb_fs::recovery::recent(db.conn(), &recovery_root(&d), RESTORE_LIST)
        .map_err(store_fault)?;
    let home = d.user_home.to_string_lossy();
    let records: Vec<Value> = rows
        .into_iter()
        .map(|mut r| {
            let original = r
                .get("original")
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            r.insert(
                "original".into(),
                json!(ferb_fs::recovery::short(&original, &home)),
            );
            Value::Object(r)
        })
        .collect();
    ok(
        json!({"days": config_days(&d, "recovery_days", 7), "total": total,
              "trash_days": ferb_fs::recovery::TRASH_RECORD_MAX_DAYS, "records": records}),
    )
}

/// `_recovery_restore` (rpc.py:273): put a file back (never over a change made since), journaled; the file work
/// in a worker thread. The protection lists close what the owner's restore may write as they close FERB's tools.
async fn recovery_restore(d: Deps, p: Params) -> Result<Map<String, Value>, HandlerError> {
    let reference = ferb_daemon::shared::text(&p, "id");
    let path = lock(&d.db).path().to_path_buf();
    let (root, home, closed) = (
        recovery_root(&d),
        d.user_home.to_string_lossy().into_owned(),
        d.closed.clone(),
    );
    let done = tokio::task::spawn_blocking(move || {
        let db = ferb_store::Db::open(&path)
            .map_err(|e| ferb_fs::recovery::RestoreError::Other(e.to_string()))?;
        let row = ferb_fs::recovery::find(db.conn(), &reference)?;
        let check = |path: &str, access: &str| closed(path, access);
        let back = ferb_fs::recovery::restore(&row.id, db.conn(), &root, Some(&check), &home)?;
        Ok::<_, ferb_fs::recovery::RestoreError>((row, back))
    })
    .await
    .map_err(|e| HandlerError::fault("RuntimeError", e.to_string()))?;
    let (row, back) = match done {
        Ok(x) => x,
        Err(ferb_fs::recovery::RestoreError::NotRestorable(m)) => return Err(ferb_error(m)),
        Err(ferb_fs::recovery::RestoreError::Os(e)) => {
            return Err(HandlerError::fault("OSError", format!("{e:?}")))
        }
        Err(ferb_fs::recovery::RestoreError::Other(m)) => {
            return Err(HandlerError::fault("RuntimeError", m))
        }
    };
    let resolved = ferb_fs::pypath::resolve(&row.original).unwrap_or_else(|_| row.original.clone());
    let in_place = back == resolved;
    d.journal.note(
        "recovery.restored",
        json!({"id": row.id, "kind": row.kind, "original": row.original, "restored_to": back,
               "in_place": in_place}),
    );
    let home = d.user_home.to_string_lossy();
    ok(
        json!({"id": row.id, "kind": row.kind, "original": ferb_fs::recovery::short(&row.original, &home),
              "restored_to": ferb_fs::recovery::short(&back, &home), "in_place": in_place}),
    )
}

/// `_report_facts` (rpc.py:321): the runtimes per role and the extensions' versions a report names.
fn report_facts(d: &Deps) -> Value {
    let cfg = ferb_daemon::areas_b::runtimes::cli::View(&d.config);
    let runtimes: Vec<String> = ferb_runtimes::catalog::ROLES
        .iter()
        .map(|role| {
            let name = d.runtimes.runtime_for(role);
            let model = ferb_runtimes::catalog::model_for(&cfg, &name);
            format!("{role}: {name} {model}").trim_end().to_string()
        })
        .collect();
    json!({"runtimes": runtimes, "extensions": (d.extensions)()})
}

fn versions(d: &Deps) -> reports::Versions {
    reports::Versions {
        ferb: d.facts.version.clone(),
        macos: d.facts.macos.clone(),
    }
}

fn report_error(e: reports::ReportError) -> HandlerError {
    match e {
        reports::ReportError::Refused(m) | reports::ReportError::Unsent(m) => ferb_error(m),
        reports::ReportError::Fault(m) => HandlerError::fault("RuntimeError", m),
    }
}

fn local_now(d: &Deps) -> ferb_core::when::DateTime {
    d.clock.now()
}

/// `_reports_build` (rpc.py:333): the report file of a turn FERB recorded as ending badly; nothing is sent.
async fn reports_build(d: Deps, p: Params) -> Result<Map<String, Value>, HandlerError> {
    let given = ferb_daemon::shared::text(&p, "task_id");
    let (db_path, task_id) = {
        let db = lock(&d.db);
        (
            db.path().to_path_buf(),
            resolve_root_task(&d, &db, &given, false)?,
        )
    };
    let request_id = ferb_daemon::shared::text(&p, "request_id");
    let app_version = ferb_daemon::shared::text(&p, "app_version");
    let facts = report_facts(&d);
    let dd = d.clone();
    let (found, built) = tokio::task::spawn_blocking(move || {
        let r = dd.redactor.clone();
        let r2 = dd.redactor.clone();
        let walk = move |v: &Value| r.value(v);
        let text = move |t: &str| r2.text(t);
        let v = versions(&dd);
        let env = reports::Env {
            home: &dd.home,
            now: local_now(&dd),
            walk: &walk,
            text: &text,
            app_version: &app_version,
            versions: &v,
        };
        reports::report(&db_path, &task_id, &request_id, &facts, &env)
    })
    .await
    .map_err(|e| HandlerError::fault("RuntimeError", e.to_string()))?
    .map_err(report_error)?;
    d.journal.note(
        "report.built",
        json!({"task_id": found.task_id, "file": built.path}),
    );
    ok(built.to_value())
}

/// `_reports_diagnostic` (rpc.py:78): Settings → Advanced → Export a diagnostic file, with the health check and what
/// FERB records; the app previews it and `reports.send` posts it.
async fn reports_diagnostic(
    d: Deps,
    s: Session,
    p: Params,
) -> Result<Map<String, Value>, HandlerError> {
    let send = ferb_daemon::shared::is_true(&p, "send");
    let mut q = Map::new();
    q.insert("selftest".into(), json!(false));
    q.insert("hearing".into(), json!(false));
    let health = super::setup::doctor_check(d.clone(), s, q).await?;
    let recording = (d.sense_status)()?;
    let app_version = ferb_daemon::shared::text(&p, "app_version");
    let facts = report_facts(&d);
    let db_path = lock(&d.db).path().to_path_buf();
    let dd = d.clone();
    let built = tokio::task::spawn_blocking(move || {
        let r = dd.redactor.clone();
        let r2 = dd.redactor.clone();
        let walk = move |v: &Value| r.value(v);
        let text = move |t: &str| r2.text(t);
        let v = versions(&dd);
        let env = reports::Env {
            home: &dd.home,
            now: local_now(&dd),
            walk: &walk,
            text: &text,
            app_version: &app_version,
            versions: &v,
        };
        reports::diagnostic(&db_path, &Value::Object(health), &recording, &facts, &env)
    })
    .await
    .map_err(|e| HandlerError::fault("RuntimeError", e.to_string()))?
    .map_err(report_error)?;
    d.journal.note(
        "diagnostic.built",
        json!({"file": built.path, "send": send}),
    );
    ok(built.to_value())
}

/// `_reports_send` (rpc.py:352): the owner's Send under a report or diagnostic file they previewed: through the
/// redaction once more, to the developer's relay (one issue, the file linked). Never retried by itself; on any
/// failure the file stays and the reply says why.
async fn reports_send(d: Deps, p: Params) -> Result<Map<String, Value>, HandlerError> {
    let (path, kind) =
        reports::saved(&d.home, &ferb_daemon::shared::text(&p, "path")).map_err(report_error)?;
    let shown = path.to_string_lossy().into_owned();
    let configured = d
        .config
        .get("feedback", "relay_url")
        .ok()
        .flatten()
        .unwrap_or(Value::String(String::new()));
    let title = ferb_daemon::shared::text(&p, "title");
    let app_version = ferb_daemon::shared::text(&p, "app_version");
    let attempt = async {
        // RELAY_URL replaced by the daemon's own (a test daemon's is empty: never the owner's deployed relay)
        let configured = if ferb_py::value::truthy(&configured) {
            configured
        } else {
            Value::String(d.relay_default.clone())
        };
        if !ferb_py::value::truthy(&configured) {
            return Err(reports::ReportError::Unsent(reports::NOT_SET_UP.into()));
        }
        let url = reports::relay_url(&configured)?;
        let (path2, dd) = (path.clone(), d.clone());
        let (title, app_version) = (title.clone(), app_version.clone());
        let out = tokio::task::spawn_blocking(move || {
            let r = dd.redactor.clone();
            let text = move |t: &str| r.text(t);
            reports::prepare(
                &path2,
                kind,
                &title,
                &app_version,
                &text,
                &versions(&dd),
                reports::MAX_REPORT_BYTES,
            )
        })
        .await
        .map_err(|e| reports::ReportError::Fault(e.to_string()))??;
        let http = reports::ReqwestRelay::new(reports::SEND_TIMEOUT_S)
            .map_err(reports::ReportError::Unsent)?;
        let id = reports::send(
            &url,
            &out,
            &http,
            &d.facts.version,
            reports::SEND_DEADLINE_S,
            1.0,
        )
        .await?;
        Ok((id, out.shortened))
    };
    match attempt.await {
        Ok((report_id, shortened)) => {
            d.journal.note(
                "report.sent",
                json!({"file": shown, "kind": kind, "report_id": report_id, "shortened": shortened}),
            );
            ok(json!({"sent": true, "path": shown, "report_id": report_id, "shortened": shortened}))
        }
        Err(reports::ReportError::Unsent(why)) => {
            d.journal.note(
                "report.send_failed",
                json!({"file": shown, "kind": kind, "why": why}),
            );
            ok(json!({"sent": false, "path": shown, "error": why}))
        }
        Err(e) => Err(report_error(e)),
    }
}

/// `_task_preview` (rpc.py:208, FERB.app only): the live preview now (`hooks.previews.frame`): the task's state and
/// line, the masked frame (base64, after the redacted rest) unless the app's `have` names it. Never waits for the
/// worker; the request (and its capture, when nobody else waits for it) ends when the app's connection ends.
async fn tasks_preview(d: Deps, p: Params) -> Result<Map<String, Value>, HandlerError> {
    let task_id = p
        .get("task_id")
        .filter(|v| ferb_py::value::truthy(v))
        .map(ferb_py::value::py_str)
        .unwrap_or_default();
    let worker = p
        .get("worker")
        .filter(|v| ferb_py::value::truthy(v))
        .map(ferb_py::value::py_str);
    // `have if isinstance(have, int) and not isinstance(have, bool)`: a JSON integer only
    let have = p.get("have").and_then(Value::as_i64);
    let answer = d
        .tools
        .previews()
        .frame(&task_id, worker.as_deref(), have)
        .await;
    let mut out = match d.redactor.value(&Value::Object(answer.fields)) {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    if let Some(image) = answer.image {
        use base64::Engine;
        out.insert(
            "image".into(),
            base64::engine::general_purpose::STANDARD
                .encode(image)
                .into(),
        );
    }
    ok(Value::Object(out))
}

/// `_question_picture` (rpc.py:238, FERB.app only; GATE-579): the payment question's one-time picture, removed by
/// this answer; `{}` when there is none.
async fn questions_picture(d: Deps, p: Params) -> Result<Map<String, Value>, HandlerError> {
    let id = ferb_daemon::shared::text(&p, "approval_id");
    match d.gateway.take_picture(&id) {
        Some(image) if !image.is_empty() => {
            use base64::Engine;
            ok(json!({"image": base64::engine::general_purpose::STANDARD.encode(image)}))
        }
        _ => ok(json!({})),
    }
}

fn conversations<'a>(
    d: &Deps,
    db: &'a ferb_store::Db,
) -> ferb_orchestrator::conversations::ConversationStore<'a> {
    let c = d.clock.clone();
    let r = d.redactor.clone();
    ferb_orchestrator::conversations::ConversationStore::new(
        db,
        Arc::new(move || ferb_core::when::isoformat(&c.now())),
        Arc::new(move |v: &Value| r.value(v)),
        Arc::new(|_t: &str, _d: Value| {}),
    )
}

fn conversation_error(e: ferb_orchestrator::sql::Error) -> HandlerError {
    match e {
        // Python's ValueError left the handler as an internal error; Rust refuses in its words (named fix)
        ferb_orchestrator::sql::Error::Value(m) => ferb_error(m),
        other => store_fault(other),
    }
}

/// `_conversations_list` (chat.py:135): one conversation's digest by id, else every conversation.
async fn conversations_list(d: Deps, p: Params) -> Result<Map<String, Value>, HandlerError> {
    let db = lock(&d.db);
    let store = conversations(&d, &db);
    match p.get("id").filter(|v| ferb_py::value::truthy(v)) {
        Some(id) => ok(store
            .digest(&ferb_py::value::py_str(id))
            .map_err(conversation_error)?),
        None => {
            let rows = store.list(None).map_err(conversation_error)?;
            ok(json!({"conversations": rows.into_iter().map(Value::Object).collect::<Vec<_>>()}))
        }
    }
}

/// `_conversations_change` (chat.py:140): the owner pauses, takes over or closes a delegated conversation, journaled
/// with the owner as its origin. Resuming one restarts its plan through the supervisor's delegated conversations,
/// not ported yet.
async fn conversations_change(d: Deps, p: Params) -> Result<Map<String, Value>, HandlerError> {
    let field = |k: &str| match p.get(k) {
        Some(v) => Ok(ferb_py::value::py_str(v)),
        // Python's KeyError left the handler as an internal error; Rust refuses in plain words (named fix)
        None => Err(ferb_error(
            "conversations.change needs the conversation's id, an action and the owner's words",
        )),
    };
    let (id, action, words) = (field("id")?, field("action")?, field("owner_words")?);
    // the supervisor's own store: a resumed conversation's saved plan runs again (`_resume_conversation_plans`)
    match d.tools.supervisor() {
        Some(sup) => sup
            .inner()
            .change_conversation(&id, &action, &words)
            .map_err(ferb_error)?,
        None => {
            let db = lock(&d.db);
            let store = conversations(&d, &db);
            store.get(&id).map_err(conversation_error)?;
            if action == "resume" {
                return Err(ferb_error("daemon not started"));
            }
            store
                .owner_change(&id, &action, &words)
                .map_err(conversation_error)?;
        }
    }
    let row = {
        let db = lock(&d.db);
        conversations(&d, &db)
            .get(&id)
            .map_err(conversation_error)?
    };
    if let Err(e) = d.journal.append(
        "conversation.owner_change",
        d.redactor.value(&Value::Object(p.clone())),
        Some("owner"),
    ) {
        eprintln!("FERB: couldn't journal conversation.owner_change: {e}");
    }
    ok(json!({"conversation": row}))
}
