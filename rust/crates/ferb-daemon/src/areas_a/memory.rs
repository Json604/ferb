//! The daemon's memory glue and the memory RPCs. Port of `ferb/daemon/areas/memory.py`: the stores' hooks (a forget's
//! copies, ended items, the core profile following its items, FERB's questions about memory closing), the start's
//! upgrade steps, FERB's own Yes/No questions outside tasks (`_ask_owner`, a chat forget), the weekly profile pass, what
//! FERB saw on this Mac, and the handlers `memory.*`, `timeline.get` and `setup.persona`. Must not let memory
//! authorize an action or save a secret (MEM-12).
//!
//! Chat capture (`_remember`, `_until`, `_save_login_name`) is in [`super::remember`]. Not ported here (each named in
//! the journal entry): the prompt's memory candidates and chat search (`_learning_candidates`, `_chat_search`,
//! `_memory_item_search`), the episode cards' capture, the hourly memory pass and its loops (distill, cards,
//! playbooks, embeddings, the search re-index).

use std::collections::BTreeSet;
use std::rc::Rc;

use ferb_learning::items::{origin_label, Item};
use ferb_learning::journal::Entry;
use ferb_learning::learning::Query;
use ferb_learning::questions::MemoryQuestions;
use ferb_rpc::server::HandlerResult;
use ferb_rpc::wire::Params;
use serde_json::{json, Map, Value};

use super::seams::{Ask, LoginError};
use super::{learning_error, ok, Handler, State};
use crate::shared::{all_hidden, is_true, refusal, refusal_as, text, truthy, EDIT_HIDDEN};

/// GATE-566: the buttons' words of FERB's own questions outside tasks (Yes, No).
pub fn own_labels(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "forget" => ("Forget it", "Keep it"),
        "downloads" => ("Delete them", "Keep them"),
        _ => ("", ""),
    }
}

/// FERB's questions about memory that no longer exist (MEM-12); rows an older build left waiting are dropped at start.
pub const GONE_QUESTIONS: [&str; 3] = ["confirm", "edit", "retire"];
/// A fact FERB's weekly check found out of date, told once in chat.
pub const ENDED_NOTICE: &str =
    "Memory: I stopped using {items}, which look out of date. Tell me if any still holds.";
/// MEM-14: the owner's own words FERB's check found over are set aside.
pub const SET_ASIDE_NOTICE: &str =
    "Memory: {items} look over, so I'll stop using them. They're kept; tell me if \
                                    any still holds and I'll use it again.";
/// The newest rows a `timeline.get` returns unless it asks for more (ruling, step 12a).
pub const TIMELINE_ROWS: i64 = 1000;
/// A lookup ranks at most this many (ruling, step 9b).
pub const MEMORY_LEARNED_MOST: usize = 100;

/// `_named_node`: how FERB names a learned item in a "Forget for good?" question.
fn named_node(node: &Value) -> String {
    let key = ferb_py::value::py_str(&node["key"]);
    format!("“{}”", key.split_whitespace().collect::<Vec<_>>().join(" "))
}

// ---------- the stores' hooks ----------

/// The hooks `_build_stores` and `_build_memory` set on memory and the core profile (compose.py:73, memory.py:54-63).
pub(crate) fn wire(st: &Rc<State>) {
    let weak = Rc::downgrade(st);
    let mut h = st.memory.hooks.borrow_mut();
    {
        let w = weak.clone();
        h.on_change = Box::new(move || {
            if let Some(st) = w.upgrade() {
                // `_export_memory`: the owner's memory as files under FERB's home; a failure never stops a write
                let _ = ferb_learning::export::export(&st.memory, &st.home.join("memory"));
            }
        });
    }
    {
        let w = weak.clone();
        h.rebuild_summary = Box::new(move |day| {
            let Some(st) = w.upgrade() else {
                return Ok(false);
            };
            ferb_learning::summary::rebuild(
                &st.store,
                &st.journal,
                &super::SummarySeams,
                st.memory.zone.as_ref(),
                day,
            )
        });
    }
    {
        let w = weak.clone();
        h.on_forget = Box::new(move |plan| match w.upgrade() {
            Some(st) => forget_item_copies(&st, plan),
            None => Ok(()),
        });
    }
    {
        let w = weak.clone();
        h.on_settled = Box::new(move |ids, deleted| {
            if let (Some(st), true) = (w.upgrade(), deleted) {
                let _ = close_memory_questions(&st, ids, None);
            }
        });
    }
    {
        let w = weak.clone();
        h.on_ended = Box::new(move |ids, new| match w.upgrade() {
            Some(st) => memory_ended(&st, ids, new),
            None => Ok(()),
        });
    }
    {
        let w = weak.clone();
        h.on_saved = Box::new(move |it| match w.upgrade() {
            Some(st) => st.profile.saved(it),
            None => Ok(()),
        });
    }
    {
        let w = weak.clone();
        h.on_set_aside = Box::new(move |ids| match w.upgrade() {
            Some(st) => st.profile.remove(ids, "code"),
            None => Ok(()),
        });
    }
    drop(h);
    let w = weak;
    *st.profile.on_joined.borrow_mut() = Box::new(move |items: &[Item]| {
        if let Some(st) = w.upgrade() {
            now_always_on(&st, items);
        }
    });
}

/// The start's memory steps (`_build_memory`, memory.py:70-85): memory cleaned once, the core profile seeded and
/// settled, the old fact-node copies closed, FERB's waiting questions asked again, an interrupted import resumed.
pub fn start(st: &Rc<State>) -> ferb_learning::Result<()> {
    let record = |kind: &str, data: Value| st.journal.append(kind, Some(data), None).map(|_| ());
    ferb_learning::upgrade::clean_before_tier1(&st.memory, &record)?;
    st.profile.seed()?;
    st.profile.settle("start")?;
    ferb_learning::upgrade::close_fact_nodes(&st.store, &ferb_clock::py::iso(&st.now()), &record)?;
    restore_memory_questions(st)?;
    super::memory_import::resume(st);
    Ok(())
}

fn questions(st: &State) -> MemoryQuestions {
    MemoryQuestions {
        store: st.store.clone(),
    }
}

/// `_forget_item_copies`: a forget's work outside the memory tables, before they change.
fn forget_item_copies(
    st: &Rc<State>,
    plan: &ferb_learning::items::ForgetPlan,
) -> ferb_learning::Result<()> {
    let texts: Vec<String> = if plan.items.is_empty() {
        Vec::new()
    } else {
        st.store
            .query(
                &format!(
                    "SELECT content FROM memory_items WHERE id IN ({})",
                    ferb_learning::store::marks(plan.items.len())
                ),
                plan.items
                    .iter()
                    .map(|i| ferb_learning::store::Sql::Text(i.clone()))
                    .collect(),
            )?
            .iter()
            .map(|r| r.s_at(0))
            .collect()
    };
    for id in &plan.items {
        st.learning
            .drop_fact_index(&format!("memory:{id}"), &st.memory)?;
    }
    if !plan.trail.is_empty() {
        st.tasks
            .forget_trail_copies(&plan.trail.iter().cloned().collect());
    }
    st.tasks.forget_pending(&texts);
    let items = plan.items.clone();
    after_forgets(st, items);
    Ok(())
}

/// `_memory_ended`: the core profile follows at once; the login usernames the owner's ended items gave are cleared.
fn memory_ended(st: &Rc<State>, ids: &[String], new: Option<&str>) -> ferb_learning::Result<()> {
    st.profile.follow(ids, new)?;
    let mut owned = Vec::new();
    for i in ids {
        if let Some(it) = st.memory.get(i)? {
            if it.origin == "owner_stated" {
                owned.push(i.clone());
            }
        }
    }
    if !owned.is_empty() {
        after_forgets(st, owned);
    }
    Ok(())
}

/// `_after_forgets(lambda: self._clear_login_names(items))`: queued behind the work still running, one at a time.
fn after_forgets(st: &Rc<State>, items: Vec<String>) {
    let prior: Vec<tokio::task::JoinHandle<()>> = st.forgetting.borrow_mut().drain(..).collect();
    let me = st.clone();
    let job = tokio::task::spawn_local(async move {
        for p in prior {
            let _ = p.await;
        }
        clear_login_names(&me, &items).await;
    });
    st.forgetting.borrow_mut().push(job);
}

/// `_settle_forgets`: wait until the work every forget left behind has finished.
pub async fn settle_forgets(st: &Rc<State>) {
    loop {
        let pending: Vec<tokio::task::JoinHandle<()>> =
            st.forgetting.borrow_mut().drain(..).collect();
        if pending.is_empty() {
            return;
        }
        for p in pending {
            let _ = p.await;
        }
    }
}

/// `_clear_login_names`: every login record's username that came from one of these items is cleared; whatever stops
/// it is journaled.
pub async fn clear_login_names(st: &Rc<State>, item_ids: &[String]) {
    let lock = st.login_names.clone();
    let _held = lock.lock().await;
    let sources: BTreeSet<String> = item_ids.iter().map(|i| format!("memory:{i}")).collect();
    match st.logins.clear_usernames(&sources) {
        Err(LoginError::Refused(e)) => {
            let _ = st.journal("memory.login_unreadable", json!({"error": e}), None);
        }
        Err(LoginError::Failed(e)) => {
            let mut items = item_ids.to_vec();
            items.sort();
            let _ = st.journal(
                "memory.login_clear_failed",
                json!({"items": items, "error": e}),
                None,
            );
        }
        Ok(cleared) if !cleared.is_empty() => {
            let _ = st.journal("memory.login_cleared", json!({"places": cleared}), None);
        }
        Ok(_) => {}
    }
}

/// `_forget_memory`: the simple forget (MEM-2), now.
pub fn forget_memory(
    st: &State,
    item_id: &str,
    keep_sources: bool,
) -> ferb_learning::Result<ferb_learning::items::ForgetPlan> {
    let plan = st.memory.forget(item_id, keep_sources)?;
    st.journal.append(
        "memory.forgot",
        Some(json!({"items": plan.items.len(), "trail": plan.trail.len()})),
        None,
    )?;
    Ok(plan)
}

/// `_forget_nodes`: learned items the owner asked to forget go, and FERB's questions about them close.
pub fn forget_nodes(st: &State, refs: &[String]) -> ferb_learning::Result<Vec<String>> {
    let deleted = st.learning.forget(refs)?;
    close_memory_questions(st, &deleted, None)?;
    Ok(deleted)
}

/// `_close_memory_questions`: FERB's questions about memory that stopped waiting are closed (superseded) and dropped.
pub fn close_memory_questions(
    st: &State,
    targets: &[String],
    kinds: Option<&[String]>,
) -> ferb_learning::Result<()> {
    let gone: BTreeSet<&String> = targets.iter().collect();
    if gone.is_empty() {
        return Ok(());
    }
    let matching: Vec<String> = st
        .followups
        .borrow()
        .iter()
        .filter(|(_, (kind, target))| {
            gone.contains(target) && kinds.is_none_or(|k| k.contains(kind))
        })
        .map(|(id, _)| id.clone())
        .collect();
    for approval_id in matching {
        st.followups.borrow_mut().remove(&approval_id);
        if st.questions.supersede(&approval_id) {
            st.publish(
                "approval.resolved",
                json!({"approval_id": approval_id, "state": "superseded"}),
            );
        }
    }
    questions(st).drop_about(targets, kinds)
}

/// `_restore_memory_questions`: at start, a forget's Yes / No still waiting is asked again with its id; one whose
/// memory has gone is dropped; MEM-12's retired kinds are dropped.
pub fn restore_memory_questions(st: &Rc<State>) -> ferb_learning::Result<()> {
    let q = questions(st);
    q.drop_kinds(&GONE_QUESTIONS.map(String::from))?;
    for row in q.waiting()? {
        let s = |n: usize| ferb_py::value::py_str(&row[n]);
        let (approval_id, kind, target, reason, request_id) = (s(0), s(1), s(2), s(3), s(4));
        let gone = if kind == "browsing" {
            true
        } else if kind.starts_with("downloads:") {
            false
        } else if kind == "forget:learning" {
            st.learning.get(&target, true)?.is_none()
        } else {
            st.memory.get(&target)?.is_none()
        };
        if gone {
            q.answered(&approval_id)?;
        } else if !st.questions.waiting(&approval_id) {
            let _ = ask_owner(
                st,
                &kind,
                &target,
                &reason,
                &request_id,
                Some(&approval_id),
                None,
            );
        }
    }
    Ok(())
}

/// `_ask_owner`: FERB's own question outside any task (a forget's Yes / No, the downloads cap, the browsing profile).
/// Kept in `memory_questions` until answered, so a restart asks it again. Its id.
pub fn ask_owner(
    st: &State,
    kind: &str,
    ident: &str,
    reason: &str,
    request_id: &str,
    approval_id: Option<&str>,
    options: Option<&[String]>,
) -> Result<String, String> {
    let head = kind.split(':').next().unwrap_or(kind);
    let (title, rest) = reason.split_once('\n').unwrap_or((reason, ""));
    let (summary, detail) = if ferb_py::value::strip(rest).is_empty() {
        (None, None)
    } else {
        (
            Some(ferb_py::value::strip(title).to_owned()),
            Some(rest.split_whitespace().collect::<Vec<_>>().join(" ")),
        )
    };
    let id = st.questions.ask(Ask {
        kind,
        reason,
        options,
        request_id,
        approval_id,
        summary,
        detail,
        labels: own_labels(head),
    })?;
    questions(st)
        .keep(
            &id,
            kind,
            ident,
            reason,
            request_id,
            &ferb_clock::py::iso(&st.now()),
        )
        .map_err(|e| e.to_string())?;
    st.followups
        .borrow_mut()
        .insert(id.clone(), (kind.to_owned(), ident.to_owned()));
    Ok(id)
}

/// The open question of FERB's own of this kind (`_owner_followups` with a pending approval), whichever task asked
/// it (areas/browsers.py `_browsing`'s one-question-at-a-time).
pub fn pending_own(st: &State, kind: &str) -> Option<String> {
    st.followups
        .borrow()
        .iter()
        .find(|(id, (k, _))| k == kind && st.questions.waiting(id))
        .map(|(id, _)| id.clone())
}

/// `_chat_forget`: a forget the owner asked for in chat. Nothing is deleted now: FERB asks a plain Yes/No question
/// naming exactly what goes and what stays; [`question_answered`] forgets on the Yes.
pub fn chat_forget(st: &State, item_id: &str, request_id: &str) -> ferb_learning::Result<String> {
    let node = st.learning.get(item_id, true)?;
    let ident = match &node {
        Some(n) if n["kind"] == "fact" => n["body"]
            .get("id")
            .map(ferb_py::value::py_str)
            .unwrap_or_default(),
        _ => item_id.to_owned(),
    };
    let (target, lines, keeps): ((&str, String), Vec<String>, String) =
        if st.memory.get(&ident)?.is_some() {
            let plan = st.memory.plan_forget(&ident, false)?;
            (
                ("memory", ident.clone()),
                plan.describe(&st.store, true)?,
                plan.keeps().to_owned(),
            )
        } else if let Some(n) = node
            .as_ref()
            .filter(|n| n["kind"] != "fact" && n["kind"] != "identity")
        {
            let doomed = st
                .learning
                .descendants(&BTreeSet::from([item_id.to_owned()]))?;
            let built = doomed.len().saturating_sub(1);
            let tail = if built > 0 {
                format!(
                    " and {built} thing{} it learned from it",
                    if built != 1 { "s" } else { "" }
                )
            } else {
                String::new()
            };
            (
                ("learning", item_id.to_owned()),
                vec![named_node(n) + &tail],
                String::new(),
            )
        } else {
            return Ok("I couldn't find that memory, so nothing was deleted.".into());
        };
    let reason = format!(
        "Forget this for good?\nFERB deletes: {}.{} It can't be brought back.",
        lines.join("; "),
        if keeps.is_empty() {
            String::new()
        } else {
            format!(" {keeps}")
        }
    );
    let _ = ask_owner(
        st,
        &format!("forget:{}", target.0),
        &target.1,
        &reason,
        &format!("memory-forget-{request_id}"),
        None,
        None,
    );
    Ok(reason + "\nAnswer Yes or No.")
}

/// An answered question as `_question_answered` reads it.
pub struct Answered<'a> {
    pub approval_id: &'a str,
    pub state: &'a str,
    pub choice: Option<&'a str>,
}

/// `_question_answered`: FERB's own follow-up to an answered question (a chat forget on its Yes). A line, or "".
pub fn question_answered(st: &State, ap: &Answered<'_>) -> ferb_learning::Result<String> {
    let Some((what, ident)) = st.followups.borrow_mut().remove(ap.approval_id) else {
        return Ok(String::new());
    };
    questions(st).answered(ap.approval_id)?;
    if what.starts_with("downloads:") {
        return Ok(super::retention::downloads_answered(st, ap, &ident));
    }
    if what == "browsing" {
        // APP-777's browsing choice is the browser area's (group B), once it is built
        let hook = st.browsing_answered.borrow().clone();
        return Ok(match hook {
            Some(f) => f(ap.state, ap.choice.unwrap_or(""), &ident),
            None => String::new(),
        });
    }
    if !(ap.state == "granted" && ap.choice == Some("Yes")) {
        return Ok("Kept: nothing was forgotten.".into());
    }
    let kind = what.strip_prefix("forget:").unwrap_or(&what);
    let gone = if kind == "memory" {
        match forget_memory(st, &ident, false) {
            Ok(plan) => plan.items.len(),
            Err(ferb_learning::Error::MemoryRefused(_)) => {
                return Ok("It was already gone: nothing else was deleted.".into())
            }
            Err(e) => return Err(e),
        }
    } else if st.learning.get(&ident, true)?.is_none() {
        return Ok("It was already gone: nothing else was deleted.".into());
    } else {
        forget_nodes(st, &[ident])?.len()
    };
    st.journal.append(
        "memory.forgot",
        Some(json!({"via": "chat", "items": gone})),
        None,
    )?;
    Ok(if kind == "memory" {
        format!("Forgot it: {gone} item(s) deleted.")
    } else {
        format!("Forgot it: {gone} item(s) deleted, with the records built on them.")
    })
}

/// `_now_always_on` (MEM-423): facts that just joined the core profile, each told once by a quiet notice
/// with its note card.
fn now_always_on(st: &Rc<State>, items: &[Item]) {
    for it in items {
        let text = it.content.split_whitespace().collect::<Vec<_>>().join(" ");
        let line = format!("Now always on: “{text}” ({})", origin_label(&it.origin));
        let mut card = ferb_orchestrator::cards::note(&it.id, &text, "saved", &line);
        card["label"] = json!(origin_label(&it.origin));
        card["origin"] = json!(it.origin);
        card["always_on"] = json!(true);
        let fut = st.owner.tell_owner(
            &format!("memory-always-on-{}", it.id),
            vec![format!("memory:{}", it.id)],
            &line,
            &ferb_sanitize::origin::join(["ferb", "inferred"]),
            Some(json!([card])),
        );
        tokio::task::spawn_local(fut);
    }
}

/// `_profile_pass`: the weekly core-profile pass (MEM-7) when due or forced; facts it found out of date are told once.
pub async fn profile_pass(st: &Rc<State>, force: bool) -> ferb_learning::Result<Option<Value>> {
    if !(force || st.profile.due()?) {
        return Ok(None);
    }
    let Some(make) = st.make_runtime.clone() else {
        return Err(ferb_learning::Error::Runtime(
            "no model runtime is set up for the profile pass".into(),
        ));
    };
    // memory.py:832: the pass's model calls as spans, its usage as `model.usage` spans with the role "profile"
    let spans = LearningSpans::new(st);
    let out = st
        .profile
        .run_pass(
            &move |a: &str, b: &str, c: &[String], d: &Value| make(a, b, c, d),
            &st.prompts,
            &st.device_id(),
            Some(&spans),
            &|runtime: &str, data: &Value| spans.usage("profile", runtime, data),
        )
        .await?;
    let ended_items: Vec<Value> = out
        .get("ended_items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let (aside, ended): (Vec<&Value>, Vec<&Value>) = ended_items
        .iter()
        .partition(|e| e.get("outcome") == Some(&json!("set_aside")));
    let stamp = ferb_clock::py::iso(&st.now());
    let said = |list: &[&Value]| {
        list.iter()
            .map(|e| format!("“{}”", ferb_py::value::py_str(&e["text"])))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let refs = |list: &[&Value]| {
        list.iter()
            .map(|e| format!("memory:{}", ferb_py::value::py_str(&e["id"])))
            .collect::<Vec<_>>()
    };
    if !ended.is_empty() {
        st.owner
            .tell_owner(
                &format!("memory-profile-ended-{stamp}"),
                refs(&ended),
                &ENDED_NOTICE.replace("{items}", &said(&ended)),
                &ferb_sanitize::origin::join(["ferb", "inferred"]),
                None,
            )
            .await;
    }
    if !aside.is_empty() {
        st.owner
            .tell_owner(
                &format!("memory-profile-aside-{stamp}"),
                refs(&aside),
                &SET_ASIDE_NOTICE.replace("{items}", &said(&aside)),
                &ferb_sanitize::origin::join(["ferb", "owner"]),
                None,
            )
            .await;
    }
    Ok(Some(out))
}

/// `_core_profile_text` / `_core_profile_pending` / `_settle_core_profile`.
pub fn core_profile_text(st: &State) -> String {
    st.profile.prompt_text().unwrap_or_default()
}

pub fn core_profile_pending(st: &State) -> String {
    st.profile.pending_text().unwrap_or_default()
}

pub fn settle_core_profile(st: &State, why: &str) {
    let _ = st.profile.settle(why);
}

/// `_time_meanings`: the owner's own meanings of fuzzy time words, stored when they corrected FERB.
pub fn time_meanings(st: &State) -> Vec<(String, String)> {
    st.memory
        .items(Some("active"), Some("owner"), true)
        .unwrap_or_default()
        .into_iter()
        .filter(|it| it.kind == "preference" && it.key.starts_with("time_"))
        .map(|it| {
            (
                it.key.trim_start_matches("time_").replace('_', " "),
                it.content,
            )
        })
        .collect()
}

/// `_memory_id`: a memory id the planner named: a memory item's id, or the fact node it was shown as.
pub fn memory_id(st: &State, ident: &str) -> String {
    let ident = ferb_py::value::strip(ident).to_owned();
    if matches!(st.memory.get(&ident), Ok(Some(_))) {
        return ident;
    }
    if ident.is_empty() {
        return ident;
    }
    match st.learning.get(&ident, true) {
        Ok(Some(node)) if node["kind"] == "fact" => node["body"]
            .get("id")
            .map(ferb_py::value::py_str)
            .unwrap_or(ident),
        _ => ident,
    }
}

/// `_item_dict`: a memory item as the memory page and the CLI read it.
pub fn item_dict(
    st: &State,
    it: &Item,
    conflict: bool,
    imported: Option<Vec<String>>,
) -> ferb_learning::Result<Value> {
    let imported = match imported {
        Some(i) => i,
        None => ferb_learning::import_job::imported_from(&st.store, std::slice::from_ref(&it.id))?
            .into_iter()
            .find(|(id, _)| *id == it.id)
            .map(|(_, from)| from)
            .unwrap_or_default(),
    };
    let in_profile = st.profile.ids()?.contains(&it.id);
    let iso = |t: &ferb_learning::DateTime| t.isoformat(ferb_py::time::Timespec::Auto);
    Ok(json!({
        "id": it.id, "kind": it.kind, "subject": it.subject, "key": it.key, "content": it.content,
        "origin": it.origin, "label": origin_label(&it.origin), "about": it.about, "status": it.status,
        "valid_from": iso(&it.valid_from),
        "valid_to": it.valid_to.as_ref().map(iso),
        "supersedes": it.supersedes,
        "review_at": it.review_at.as_ref().map(iso),
        "in_profile": in_profile,
        "set_aside": it.set_aside,
        "sources": it.source_ids, "evidence_days": st.memory.evidence_days(&it.id)?, "conflict": conflict,
        "imported_from": imported,
    }))
}

// ---------- the handlers ----------

pub(crate) fn handlers() -> Vec<(&'static str, Handler)> {
    vec![
        ("memory.learning", |s, p| Box::pin(memory_learning(s, p))),
        ("memory.list", |s, p| Box::pin(memory_list(s, p))),
        ("memory.show", |s, p| Box::pin(memory_show(s, p))),
        ("memory.add", |s, p| Box::pin(memory_add(s, p))),
        ("memory.profile", |s, p| Box::pin(memory_profile(s, p))),
        ("memory.edit", |s, p| Box::pin(memory_edit(s, p))),
        ("memory.retract", |s, p| Box::pin(memory_retract(s, p))),
        ("memory.forget", |s, p| Box::pin(memory_forget(s, p))),
        ("memory.restore", |s, p| Box::pin(memory_restore(s, p))),
        ("memory.repeats", |s, p| Box::pin(memory_repeats(s, p))),
        ("timeline.get", |s, p| Box::pin(timeline(s, p))),
        ("setup.persona", |s, p| Box::pin(setup_persona(s, p))),
    ]
}

fn le<T>(r: ferb_learning::Result<T>) -> Result<T, ferb_rpc::server::HandlerError> {
    r.map_err(learning_error)
}

/// `str(params["key"])` of a key the handler can't do without. Fixed against Python (named in the tests): a missing
/// key answered with an internal error (`KeyError`); here it is a refusal naming what is missing.
fn required(
    params: &Params,
    key: &str,
    said: &str,
) -> Result<String, ferb_rpc::server::HandlerError> {
    match params.get(key) {
        Some(v) => Ok(ferb_py::value::py_str(v)),
        None => Err(refusal(said)),
    }
}

/// `memory.learning {op, id?, ...}`: learned nodes for the memory page and the CLI.
async fn memory_learning(st: Rc<State>, params: Params) -> HandlerResult {
    let l = &st.learning;
    let refused = |m: &str| refusal_as("LearningRefused", m);
    let op = match params.get("op") {
        Some(v) => ferb_py::value::py_str(v),
        None => "inventory".into(),
    };
    let ident = crate::shared::text_or(&params, "id", "");
    match op.as_str() {
        "backlog" => ok(json!({"backlog": null})),
        "guess" | "guess_state" => Err(refused(
            "FERB no longer asks about its guesses: it saves what it understands, labelled as its understanding; \
             correct it by telling FERB or on the memory page",
        )),
        "inventory" => {
            let items = le(l.items(None, truthy(&params, "history"), false, &[]))?;
            let out: Vec<Value> = items
                .into_iter()
                .map(|mut n| {
                    let confirmable = !ferb_py::value::truthy(&n["confirmed"])
                        && n["kind"] == "identity_proposal";
                    n["confirmable"] = json!(confirmable);
                    n
                })
                .collect();
            ok(json!({"items": out}))
        }
        "search" => {
            let query = crate::shared::text_or(&params, "query", "");
            let f = |k: &str| crate::shared::text(&params, k);
            let (kind, app, person, since, until) = (f("kind"), f("app"), f("person"), f("since"), f("until"));
            fn nz(s: &str) -> Option<&str> {
                (!s.is_empty()).then_some(s)
            }
            let found = le(l.search(
                &query,
                &Query {
                    kind: nz(&kind),
                    app: nz(&app),
                    person: nz(&person),
                    since: nz(&since),
                    until: nz(&until),
                    limit: Some(MEMORY_LEARNED_MOST),
                    leave_out: ferb_learning::notes::PROMPT_LEAVES_OUT.to_vec(),
                    no_sources: true,
                    ..Query::default()
                },
            ))?;
            ok(json!({"items": found}))
        }
        "get" => {
            let mut node = le(l.get(&ident, false))?;
            let history = match node.as_mut() {
                Some(n) => {
                    let count = le(st.store.count(
                        "SELECT COUNT(*) FROM learning_sources WHERE node_id = ?",
                        vec![ferb_learning::store::Sql::Text(ident.clone())],
                    ))?;
                    n["source_count"] = json!(count);
                    le(l.versions(
                        &ferb_py::value::py_str(&n["kind"]),
                        &ferb_py::value::py_str(&n["key"]),
                    ))?
                }
                None => Vec::new(),
            };
            ok(json!({"item": node, "history": history}))
        }
        "confirm" => {
            if is_fact(&le(l.get(&ident, true))?) {
                return Err(refused("memories need no confirming; correct one with edit or forget"));
            }
            ok(json!({"item": le(l.confirm(&ident))?}))
        }
        "edit" => {
            let new_text = crate::shared::text_or(&params, "text", "");
            if let Some(item_id) = fact_item(&le(l.get(&ident, true))?) {
                let mut p = Map::new();
                p.insert("id".into(), json!(item_id));
                p.insert("content".into(), json!(new_text));
                let item = memory_edit(st.clone(), p).await?;
                return ok(json!({"item": Value::Object(item)}));
            }
            ok(json!({"item": le(l.edit(&ident, &new_text))?}))
        }
        "retire" => {
            if let Some(item_id) = fact_item(&le(l.get(&ident, true))?) {
                match st.memory.retire(&item_id, "the owner, on the memory page") {
                    Ok(()) => {}
                    Err(ferb_learning::Error::MemoryRefused(_)) => {
                        return Err(refused("that memory no longer holds"))
                    }
                    Err(e) => return Err(learning_error(e)),
                }
                let it = le(st.memory.get(&item_id))?;
                let item = match it {
                    Some(it) => le(item_dict(&st, &it, false, None))?,
                    None => return Err(crate::shared::fault("AttributeError", "the retired memory is gone")),
                };
                return ok(json!({"item": item}));
            }
            ok(json!({"item": le(l.retire(&ident))?}))
        }
        "forget" => {
            if let Some(item_id) = fact_item(&le(l.get(&ident, true))?) {
                if truthy(&params, "confirm") {
                    le(forget_memory(&st, &item_id, false))?;
                    settle_forgets(&st).await;
                    return ok(json!({"deleted": [ident]}));
                }
                let plan = le(st.memory.plan_forget(&item_id, false))?;
                return ok(json!({"descendants": [], "will_delete": le(plan.describe(&st.store, false))?,
                                 "keeps": plan.keeps()}));
            }
            let doomed = le(l.descendants(&BTreeSet::from([ident.clone()])))?;
            if truthy(&params, "confirm") {
                return ok(json!({"deleted": le(forget_nodes(&st, &[ident]))?}));
            }
            ok(json!({"descendants": doomed}))
        }
        "rollback" => ok(json!({"invalidated": le(l.rollback(&crate::shared::text_or(&params, "batch", "")))?})),
        "digest" => ok(json!({"digest": le(l.digest())?})),
        "eval_export" => {
            // ferb/eval/memory.py `history_queries`
            let rows = le(st.store.query(
                "SELECT task_id,request_text FROM episodes ORDER BY started_at,task_id",
                Vec::new(),
            ))?;
            let cases: Vec<Value> = rows
                .iter()
                .map(|r| {
                    json!({"task_id": ferb_learning::store::to_json(r.at(0)),
                           "query": ferb_learning::store::to_json(r.at(1)),
                           "relevant": [], "expected": "", "origin": "owner_history"})
                })
                .collect();
            ok(json!({"cases": cases, "label_status": "needs owner relevance and expected-result labels",
                      "recommended_queries": "approximately 50–100, including Hinglish, people, temporal and app queries"}))
        }
        "embedding" => {
            let (root, fetch) = embedding_root(&st);
            let vectors = le(st.store.count("SELECT COUNT(*) FROM recall_vectors", Vec::new()))?;
            let active = l.embedder().map(|e| e.name().to_owned());
            ok(json!({"active": active, "path": root, "fetched_by_ferb": fetch, "vectors": vectors}))
        }
        _ => Err(refused("unknown learning operation")),
    }
}

fn is_fact(node: &Option<Value>) -> bool {
    node.as_ref().is_some_and(|n| n["kind"] == "fact")
}

fn fact_item(node: &Option<Value>) -> Option<String> {
    node.as_ref()
        .filter(|n| n["kind"] == "fact")
        .map(|n| ferb_py::value::py_str(&n["body"]["id"]))
}

/// `_embedding_root`: where the local model's files are (`[memory] embedding_model`, else the copy FERB.app ships,
/// else a checkout's under FERB's home), and whether FERB may fetch them there itself.
fn embedding_root(st: &State) -> (String, bool) {
    let configured = st
        .config_get("memory", "embedding_model")
        .filter(ferb_py::value::truthy)
        .map(|v| ferb_py::value::strip(&ferb_py::value::py_str(&v)).to_owned())
        .unwrap_or_default();
    if !configured.is_empty() {
        let expanded = match configured.strip_prefix('~') {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => std::env::var("HOME")
                .map(|h| format!("{h}{rest}"))
                .unwrap_or(configured),
            _ => configured,
        };
        return (expanded, false);
    }
    if let Some(bundle) = &st.app_bundle {
        let resources = bundle.join(ferb_device::install::BUNDLE_MODELS);
        let parent = resources.parent().unwrap_or(&resources).to_path_buf();
        return (
            ferb_memory::embeddings::default_root(&parent)
                .to_string_lossy()
                .into_owned(),
            false,
        );
    }
    (
        ferb_memory::embeddings::default_root(&st.home)
            .to_string_lossy()
            .into_owned(),
        true,
    )
}

/// `memory.list {status?, subject?, history?, imported_from?}`.
async fn memory_list(st: Rc<State>, params: Params) -> HandlerResult {
    let status = crate::shared::text_or(&params, "status", "active");
    let subject = params
        .get("subject")
        .filter(|v| !v.is_null())
        .map(ferb_py::value::py_str);
    let items = le(st.memory.items(
        (status != "all").then_some(status.as_str()),
        subject.as_deref(),
        !truthy(&params, "history"),
    ))?;
    let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    let where_: std::collections::HashMap<String, Vec<String>> =
        le(ferb_learning::import_job::imported_from(&st.store, &ids))?
            .into_iter()
            .collect();
    let wanted = text(&params, "imported_from");
    let items: Vec<&Item> = items
        .iter()
        .filter(|i| {
            wanted.is_empty()
                || match where_.get(&i.id) {
                    Some(from) => wanted == "any" && !from.is_empty() || from.contains(&wanted),
                    None => false,
                }
        })
        .collect();
    let mut out = Vec::new();
    for it in items {
        out.push(le(item_dict(
            &st,
            it,
            false,
            Some(where_.get(&it.id).cloned().unwrap_or_default()),
        ))?);
    }
    ok(json!({"items": out}))
}

/// `memory.show {subject}`.
async fn memory_show(st: Rc<State>, params: Params) -> HandlerResult {
    let subject = required(&params, "subject", "memory.show needs a subject")?;
    let items = le(st.memory.items(None, Some(&subject), false))?;
    let mut out = Vec::new();
    for it in &items {
        out.push(le(item_dict(&st, it, false, None))?);
    }
    ok(json!({"subject": subject, "items": out}))
}

/// `memory.add {content, kind?, subject?, key?}`: the owner's own words, saved as theirs.
async fn memory_add(st: Rc<State>, params: Params) -> HandlerResult {
    let content = required(
        &params,
        "content",
        "memory.add needs content: the words to remember",
    )?;
    let content = ferb_py::value::strip(&content).to_owned();
    let entry: Entry = st.journal("memory.added", json!({"content": content}), None)?;
    let or = |k: &str, d: &str| {
        let t = text(&params, k);
        if t.is_empty() {
            d.to_owned()
        } else {
            t
        }
    };
    let key = {
        let k = text(&params, "key");
        if k.is_empty() {
            ferb_memory::terms::slug(&content, Some(30))
        } else {
            k
        }
    };
    let it = le(st.memory.propose(
        &or("kind", "fact"),
        &or("subject", "owner"),
        &key,
        &content,
        "owner_stated",
        &[("journal".to_owned(), entry.id)],
        1.0,
    ))?;
    ok(le(item_dict(&st, &it, false, None))?)
}

/// `memory.profile {op?, id?, section?}`: the core profile as the memory page shows and edits it.
async fn memory_profile(st: Rc<State>, params: Params) -> HandlerResult {
    let op = {
        let t = text(&params, "op");
        if t.is_empty() {
            "get".to_owned()
        } else {
            t
        }
    };
    let ident = text(&params, "id");
    match op.as_str() {
        "add" => match st.profile.add(&ident, &text(&params, "section"), "owner") {
            Ok(()) => {}
            Err(e) if e.is_value_error() => return Err(refusal(e.to_string())),
            Err(e) => return Err(learning_error(e)),
        },
        "remove" => le(st.profile.remove(&[ident], "owner"))?,
        "pass" => {
            le(profile_pass(&st, true).await)?;
        }
        "get" => {}
        _ => {
            return Err(refusal(
                "unknown memory.profile op: get, add, remove or pass",
            ))
        }
    }
    ok(le(st.profile.view())?)
}

/// `memory.edit {id, content}`: the owner's correction becomes a new version, saved as theirs; words redaction would
/// hide whole are refused.
async fn memory_edit(st: Rc<State>, params: Params) -> HandlerResult {
    if !params.contains_key("id") || !params.contains_key("content") {
        return Err(refusal(
            "memory.edit needs the memory's id and its new content",
        ));
    }
    let content = ferb_py::value::py_str(&params["content"]);
    if all_hidden(&st.redact_text(&content)) {
        return Err(refusal(EDIT_HIDDEN));
    }
    let entry = st.journal(
        "memory.edited",
        json!({"id": params["id"], "content": params["content"]}),
        None,
    )?;
    let id = ferb_py::value::py_str(&params["id"]);
    let it = le(st
        .memory
        .edit(&id, &content, &("journal".to_owned(), entry.id)))?;
    ok(le(item_dict(&st, &it, false, None))?)
}

/// `memory.retract {id, reason?}`.
async fn memory_retract(st: Rc<State>, params: Params) -> HandlerResult {
    let id = required(&params, "id", "memory.retract needs the memory's id")?;
    let reason = crate::shared::text_or(&params, "reason", "the owner");
    le(st.memory.retract(&id, &reason, false))?;
    clear_login_names(&st, std::slice::from_ref(&id)).await;
    ok(json!({"retracted": params["id"]}))
}

/// `memory.forget {id, confirm?, keep_sources?}`: what would go, then (confirmed) gone.
async fn memory_forget(st: Rc<State>, params: Params) -> HandlerResult {
    let id = required(&params, "id", "memory.forget needs the memory's id")?;
    let keep = truthy(&params, "keep_sources");
    let plan = le(st.memory.plan_forget(&id, keep))?;
    if !truthy(&params, "confirm") {
        return ok(
            json!({"confirmed": false, "will_delete": le(plan.describe(&st.store, false))?,
                         "keeps": plan.keeps()}),
        );
    }
    let described = le(plan.describe(&st.store, false))?;
    le(forget_memory(&st, &id, keep))?;
    settle_forgets(&st).await;
    ok(json!({"confirmed": true, "deleted": described, "keeps": plan.keeps()}))
}

/// `memory.restore {id}` (MEM-14): a fact FERB set aside still holds.
async fn memory_restore(st: Rc<State>, params: Params) -> HandlerResult {
    let id = text(&params, "id");
    if le(st.memory.get(&id))?.is_none() {
        return Err(refusal("no memory with that id"));
    }
    let restored = le(st.memory.restore(&id))?;
    let it = le(st.memory.get(&id))?.ok_or_else(|| refusal("no memory with that id"))?;
    ok(json!({"restored": restored, "item": le(item_dict(&st, &it, false, None))?}))
}

/// `memory.repeats {}`: each playbook's runs.
async fn memory_repeats(st: Rc<State>, _params: Params) -> HandlerResult {
    ok(json!({"playbooks": le(ferb_learning::playbooks::repeat_stats(&st.store))?}))
}

/// `int(value)` as Python reads a JSON value: whole numbers, floats cut toward zero, text of digits.
fn py_int(v: &Value) -> Option<i64> {
    match v {
        Value::Bool(b) => Some(i64::from(*b)),
        Value::Number(n) => n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|f| f.is_finite())
                .map(|f| f.trunc() as i64)
        }),
        Value::String(s) => {
            let t = ferb_py::value::strip(s).replace('_', "");
            t.parse::<i64>().ok()
        }
        _ => None,
    }
}

/// `timeline.get {since?, limit?}`: the journal's newest rows (bounded, step 12a).
async fn timeline(st: Rc<State>, params: Params) -> HandlerResult {
    let since = if truthy(&params, "since") {
        let raw = ferb_py::value::py_str(&params["since"]);
        // `datetime.fromisoformat(since).astimezone()`: a naive time is the Mac's local time. Fixed against Python
        // (named in the tests): a time it refuses was an internal error (ValueError); here it is a refusal.
        let parsed = match ferb_clock::py::parse_maybe_naive(&raw) {
            Ok(Some(t)) => Some(t),
            Ok(None) => naive_local(&st, &raw),
            Err(_) => None,
        };
        match parsed {
            Some(t) => Some(ferb_clock::py::iso(&st.clock.zone().local(&t))),
            None => {
                return Err(refusal(
                    "since must be a date and time, like 2026-01-01T09:00:00+00:00",
                ))
            }
        }
    } else {
        None
    };
    let limit = if truthy(&params, "limit") {
        match py_int(&params["limit"]) {
            Some(n) => n,
            None => return Err(refusal("limit must be a whole number")),
        }
    } else {
        TIMELINE_ROWS
    }
    .max(1);
    let rows = le(st.journal.entries(since.as_deref(), Some(limit + 1)))?;
    let start = rows.len().saturating_sub(limit as usize);
    let entries: Vec<Value> = rows[start..]
        .iter()
        .map(|e| json!({"at": e.at, "kind": e.kind, "data": e.data}))
        .collect();
    ok(json!({"entries": entries, "more": rows.len() as i64 > limit}))
}

/// A naive ISO time read in the Mac's zone (Python's `astimezone()` of a naive datetime).
fn naive_local(st: &State, raw: &str) -> Option<ferb_clock::py::Time> {
    let d = ferb_py::time::DateTime::from_isoformat(raw).ok()?;
    let naive = chrono::NaiveDate::from_ymd_opt(d.year, d.month, d.day)?.and_hms_micro_opt(
        d.hour,
        d.minute,
        d.second,
        d.microsecond,
    )?;
    st.clock.zone().wall_dt(naive).ok()
}

/// `setup.persona {refresh?, status?}` (MEM-13): {} starts what FERB saw on this Mac, once; `refresh` again; `status`
/// only reads it.
async fn setup_persona(st: Rc<State>, params: Params) -> HandlerResult {
    if truthy(&params, "status") && st.persona.borrow().built() {
        return ok(st.persona.borrow().status());
    }
    if st.shutting_down.get() {
        return ok(json!({"state": "closed"}));
    }
    let out = super::persona::start(&st, is_true(&params, "refresh"), "setup");
    ok(out)
}

/// `self.spans` as the learners' model calls use it (memory.py:66-68, 806, 832, 1167, intro.py:117): each call a
/// `model.learning` span and each usage event a `model.usage` span naming that learner's role
/// (`_record_learning_usage`, memory.py:763-769), in the store, redacted as every span is.
pub struct LearningSpans {
    db: Rc<ferb_store::Db>,
    clock: std::sync::Arc<dyn ferb_clock::clock::ZonedClock>,
    ids: std::sync::Arc<dyn ferb_core::interfaces::IdSource>,
    redactor: std::sync::Arc<ferb_sanitize::redact::Redactor>,
    open: std::cell::RefCell<std::collections::HashMap<String, ferb_store::spans::Span>>,
}

struct SpanTime(std::sync::Arc<dyn ferb_clock::clock::ZonedClock>);
impl ferb_store::spans::Clock for SpanTime {
    fn now(&self) -> ferb_py::time::DateTime {
        super::to_py(&self.0.now())
    }
}

struct SpanIds(std::sync::Arc<dyn ferb_core::interfaces::IdSource>);
impl ferb_store::spans::IdSource for SpanIds {
    fn new_id(&self) -> String {
        self.0.new_id()
    }
}

impl LearningSpans {
    pub fn new(st: &State) -> LearningSpans {
        LearningSpans::over(
            st.store.db.clone(),
            st.clock.clone(),
            st.ids.clone(),
            st.redactor.clone(),
        )
    }

    /// Over a store and the daemon's clock, ids and redactor (group A's are [`LearningSpans::new`]'s).
    pub fn over(
        db: Rc<ferb_store::Db>,
        clock: std::sync::Arc<dyn ferb_clock::clock::ZonedClock>,
        ids: std::sync::Arc<dyn ferb_core::interfaces::IdSource>,
        redactor: std::sync::Arc<ferb_sanitize::redact::Redactor>,
    ) -> LearningSpans {
        LearningSpans {
            db,
            clock,
            ids,
            redactor,
            open: Default::default(),
        }
    }

    fn store(&self) -> ferb_store::spans::SpanStore<'_> {
        let r = self.redactor.clone();
        ferb_store::spans::SpanStore::new(
            &self.db,
            Box::new(SpanTime(self.clock.clone())),
            Box::new(SpanIds(self.ids.clone())),
            Box::new(move |v: &Value| r.value(v)),
            Box::new(|kind: &str| ferb_policy::rules::REVERSIBLE.contains(&kind)),
        )
    }

    /// `_record_learning_usage(role)(runtime, data)`: one `model.usage` span with the learner's role and the
    /// event's cost.
    pub fn usage(&self, role: &str, runtime: &str, data: &Value) {
        let store = self.store();
        if let Ok(mut span) = store.start(
            "model.usage",
            &json!({"runtime": runtime, "role": role}),
            None,
            None,
            None,
        ) {
            let _ = store.finish(
                &mut span,
                ferb_store::spans::Finish {
                    output: Some(data),
                    error: None,
                    cost_usd: data.get("cost_usd").and_then(Value::as_f64),
                    origin: None,
                    summary: None,
                },
            );
        }
    }
}

impl ferb_learning::calls::Spans for LearningSpans {
    fn start(
        &self,
        kind: &str,
        task_id: Option<&str>,
        input: Value,
        parent_id: Option<&str>,
    ) -> String {
        match self.store().start(kind, &input, task_id, parent_id, None) {
            Ok(span) => {
                let id = span.id.clone();
                self.open.borrow_mut().insert(id.clone(), span);
                id
            }
            Err(_) => String::new(),
        }
    }

    fn finish(&self, id: &str, output: Option<Value>, error: Option<String>) {
        let Some(mut span) = self.open.borrow_mut().remove(id) else {
            return;
        };
        let _ = self.store().finish(
            &mut span,
            ferb_store::spans::Finish {
                output: output.as_ref(),
                error: error.as_deref(),
                cost_usd: None,
                origin: None,
                summary: None,
            },
        );
    }
}
