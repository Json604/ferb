//! FERB's hidden-input questions (`ferb/daemon/hidden_input.py`): an owner secret (a password, a card detail, a
//! private answer), a login code, and the Mac password for one approved admin command. The value goes straight to
//! the sealed fill and is never returned in a tool result, a span, a log or a model's context: the caller gets a
//! [`Secret`] (wiped when dropped), the question keeps no copy once its wait ends, and nothing here writes a span
//! or the journal. Asking for a hidden value is not a decision about whether a call may run (kb area gateway).
//!
//! decision: GATE-204, GATE-513, GATE-520, GATE-559, GATE-562, GATE-563, GATE-572, GATE-566, GATE-572

use std::sync::Arc;

use ferb_core::types::ToolContext;
use ferb_identity::owner_secrets::{refuse_whole_browser, same_site, site_host, typed_site};
use ferb_identity::Secret;
use ferb_policy::payments;
use serde_json::{json, Map, Value};

use super::{lock, obj, CallError, Cancel, Gateway, Left, Refusal, TaskRecord, ASKS_AFTER_REFUSAL};
use crate::hidden_words::*;
use crate::questions::{card_parts, Approval, Asker, State};
use crate::words::{quoted, REASON_CHARS};
use ferb_policy::confirm;
use ferb_rpc::auth::RunGrant;

/// Why a hidden input gives no value: a safety check's refusal (Python's `Refused`, safe to show a worker), a
/// failure below it (the names' file or the Keychain helper, Python's `OSError`), or the waiting call was stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HiddenError {
    Refused(String),
    Failed(String),
    Cancelled,
}

impl From<ferb_identity::Error> for HiddenError {
    fn from(e: ferb_identity::Error) -> HiddenError {
        match e {
            ferb_identity::Error::Refused(m) => HiddenError::Refused(m),
            ferb_identity::Error::HelperFailed { message, .. } => HiddenError::Refused(message),
            ferb_identity::Error::Io(io) => HiddenError::Failed(format!("OSError: {io}")),
        }
    }
}

impl From<CallError> for HiddenError {
    fn from(e: CallError) -> HiddenError {
        match e {
            CallError::Cancelled => HiddenError::Cancelled,
            CallError::Internal(m) => HiddenError::Failed(m),
        }
    }
}

/// What the owner secret question carries besides its place (`secret(ctx, target, handle=, remember=, shown=,
/// field=, kind=)`).
#[derive(Debug, Clone, Default)]
pub struct SecretAsk {
    /// A saved entry's name: filled from the Keychain, nothing asked.
    pub handle: Option<String>,
    /// The name the typed value is saved under (L36).
    pub remember: Option<String>,
    /// The app's name as macOS reports it, for an `app:` place.
    pub shown: Option<String>,
    /// The page's own label for the field.
    pub field: Option<String>,
    /// The field's kind as the page marks it (GATE-206).
    pub kind: Option<String>,
}

/// The waiting itself, undone whether it ends or is dropped (Python's `except CancelledError` and `finally` in
/// `_wait_answer`): the wait is counted off, and a stopped task's question closes as superseded.
struct Waiting<'a> {
    gw: &'a Gateway,
    id: String,
    task: String,
    worker: String,
    ended: bool,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        if !self.ended
            && self.gw.task_stopped(&self.task)
            && lock(&self.gw.inner).approvals.supersede(&self.id).is_some()
        {
            self.gw.answers.notify_waiters();
            self.gw.publish(
                "approval.resolved",
                obj(json!({"approval_id": self.id, "state": "superseded"})),
            );
        }
        {
            let mut i = lock(&self.gw.inner);
            if let Some(n) = i.waiting.get_mut(&self.id) {
                *n -= 1;
                if *n == 0 {
                    i.waiting.remove(&self.id);
                }
            }
        }
        self.gw.observer().waiting(&self.worker, false);
    }
}

/// The question's end (`finally: ap.answer = None; ap.state = ...`): no copy of the value stays, and an
/// unfilled question reads as declined (or set aside, L3), whether the wait ended or was dropped.
struct Seal<'a> {
    gw: &'a Gateway,
    id: String,
    used: bool,
}

impl Drop for Seal<'_> {
    fn drop(&mut self) {
        let used = self.used;
        lock(&self.gw.inner).approvals.drop_sealed(&self.id);
        self.gw.with_approval(&self.id, |ap| {
            ap.answer = None;
            ap.state = if used {
                State::Used
            } else if ap.state == State::Superseded {
                State::Superseded
            } else {
                State::Denied
            };
        });
    }
}

impl Gateway {
    /// `ask_secret(ctx, target, ...)` (D12): the owner's hidden input for `target`, bound for the sealed fill. The
    /// model sees handles only. `Ok(None)`: declined, unattended or not given.
    pub async fn ask_secret(
        &self,
        ctx: &ToolContext,
        target: &str,
        ask: SecretAsk,
    ) -> Result<Option<Secret>, HiddenError> {
        refuse_whole_browser(self.d.places.as_ref(), target)?; // before any question, for every caller (D12)
        let Some(store) = self.d.owner_secrets.clone() else {
            return Err(HiddenError::Refused(
                "secret storage protection is unavailable".into(),
            ));
        };
        let nonempty = |v: &Option<String>| v.clone().filter(|s| !s.is_empty());
        let (handle, remember) = (nonempty(&ask.handle), nonempty(&ask.remember));
        let kind = ask.kind.clone();
        if let Some(handle) = handle {
            let target = target.to_string();
            let found = tokio::task::spawn_blocking(move || {
                store.reveal(&handle, &target, kind.as_deref())
            })
            .await
            .map_err(|e| HiddenError::Failed(format!("the store's thread ended: {e}")))??;
            return Ok(Some(found));
        }
        if self.unattended(&ctx.request_id) {
            return Ok(None);
        }
        let sites = self.question_sites(ctx, target);
        // L36 §5: which account, shown to the owner, never a secret; the task's one owner-typed site counts as a
        // link before any entry is saved here (read only)
        let account = {
            let holds = self.d.username_holds.as_deref();
            let holding: ferb_identity::owner_secrets::Holding<'_> =
                holds.map(|f| f as &dyn Fn(&str) -> bool);
            store.account(target, &sites, holding)
        };
        let (username, source) = match account {
            Ok(found) => found,
            Err(ferb_identity::Error::Refused(m)) => {
                if remember.is_some() {
                    return Err(HiddenError::Refused(m));
                }
                (None, None) // m17: a one-time input needs no saved names
            }
            Err(e) => return Err(e.into()),
        };
        let refused = self.refused_now(
            ctx,
            target,
            &super::field_key("secret", ask.kind.as_deref()),
        )?;
        let v = |s: &Option<String>| {
            s.as_deref()
                .filter(|x| !x.is_empty())
                .map(|x| self.value(x))
        };
        let (r_remember, r_username, r_source, r_shown, r_field) = (
            v(&remember),
            v(&username),
            v(&source),
            v(&ask.shown),
            v(&ask.field),
        );
        let r_target = self.value(target);
        let paying = self.payment_line(ctx, target, true);
        let mut question = secret_question(
            &r_target,
            r_remember.as_deref(),
            r_username.as_deref(),
            r_source.as_deref(),
        );
        if !paying.is_empty() {
            question = format!("{question} {}", self.value(&paying));
        }
        let question = self.built(&question, false);
        let summary = self.built(
            &secret_summary(
                &r_target,
                r_remember.as_deref(),
                r_shown.as_deref(),
                r_username.as_deref(),
                r_source.as_deref(),
            ),
            false,
        );
        let detail = self.built(
            &format!(
                "{}{}",
                refused_line(
                    refused
                        .as_ref()
                        .map(|r| (r.what.as_str(), r.words.as_str()))
                ),
                secret_detail(
                    &r_target,
                    r_remember.as_deref(),
                    r_shown.as_deref(),
                    r_field.as_deref()
                )
            ),
            false,
        );
        let exact = if paying.is_empty() {
            String::new()
        } else {
            self.built(&self.value(&paying), false)
        };
        let why = self.why_of(ctx);
        let labels = if remember.is_some() {
            SAVE_LABELS
        } else {
            ONE_TIME_LABELS
        };
        let title = if target.starts_with("app:") {
            APP_TITLE
        } else {
            ""
        };
        let id = self.hidden_question(
            "secret.fill",
            (&ctx.task_id, &ctx.request_id),
            &question,
            |ap| {
                ap.kind = "owner_secret".into();
                ap.summary = summary.clone();
                ap.detail = detail.clone();
                ap.exact = exact.clone();
                ap.why = why.clone();
                ap.secret_labels = labels.iter().map(|s| s.to_string()).collect();
                ap.title = title.into();
            },
            |ap| {
                let mut e = obj(json!({
                    "approval_id": ap.id, "kind": ap.kind, "capability": "secret.fill", "reason": question,
                    "task_id": ctx.task_id, "summary": ap.summary, "why": ap.why
                }));
                e.extend(card_parts(ap));
                e
            },
            &[&question, &summary, &detail, &exact, &why],
        );
        let mut seal = Seal {
            gw: self,
            id: id.clone(),
            used: false,
        };
        let state = self
            .hidden_wait(&ctx.worker_id, &ctx.task_id, &id, None)
            .await?;
        seal.used = state == "granted";
        let answer = self.take_answer(&id);
        let Some(value) = answer.filter(|_| state == "granted") else {
            return Ok(None);
        };
        if let Some(name) = remember {
            // I14: the record serves the one site the owner named
            let (target, kind, kept) = (target.to_string(), ask.kind.clone(), value.duplicate());
            tokio::task::spawn_blocking(move || {
                store.remember(&name, &target, &kept, &sites, kind.as_deref())
            })
            .await
            .map_err(|e| HiddenError::Failed(format!("the store's thread ended: {e}")))??;
        }
        drop(seal);
        Ok(Some(value))
    }

    /// `ask_login_code(ctx, origin)`: owner-only hidden input for a login code, passed directly to a sealed fill.
    pub async fn ask_login_code(
        &self,
        ctx: &ToolContext,
        origin: &str,
    ) -> Result<Option<Secret>, HiddenError> {
        if self.unattended(&ctx.request_id) {
            return Ok(None);
        }
        let refused = self.refused_now(ctx, origin, &super::field_key("login_code", None))?;
        let paying = self.payment_line(ctx, origin, false);
        let mut question = login_code_question(&self.value(origin));
        if !paying.is_empty() {
            question = format!("{question} {}", self.value(&paying));
        }
        let question = self.built(&question, false);
        let place = site(origin);
        // GATE-566: which code this is for the site in this request (the questions kept for it: open or answered)
        let count = 1 + lock(&self.inner)
            .approvals
            .of_request(&ctx.request_id)
            .iter()
            .filter(|a| a.kind == "owner_login_code" && a.site == place)
            .count();
        let (summary, detail, exact, labels) = match self.payment_of(ctx, origin, false) {
            Some(p) => {
                // GATE-204, GATE-513: the code says in its question which payment it approves
                let (s, d, e) = payments::otp_card(&place, &p);
                (self.value(&s), self.value(&d), self.value(&e), OTP_LABELS)
            }
            None => (
                login_code_summary(&self.value(origin)),
                login_code_detail(Some(&self.value(&self.label_of(ctx))), count),
                String::new(),
                CODE_LABELS,
            ),
        };
        let summary = self.built(&summary, false);
        let detail = self.built(
            &format!(
                "{}{detail}",
                refused_line(
                    refused
                        .as_ref()
                        .map(|r| (r.what.as_str(), r.words.as_str()))
                )
            ),
            false,
        );
        let exact = if exact.is_empty() {
            exact
        } else {
            self.built(&exact, false)
        };
        let why = self.why_of(ctx);
        let id = self.hidden_question(
            "browser.login_code",
            (&ctx.task_id, &ctx.request_id),
            &question,
            |ap| {
                ap.kind = "owner_login_code".into();
                ap.site = place.clone();
                ap.secret_labels = labels.iter().map(|s| s.to_string()).collect();
                ap.summary = summary.clone();
                ap.detail = detail.clone();
                ap.exact = exact.clone();
                ap.why = why.clone();
            },
            |ap| {
                let mut e = obj(json!({
                    "approval_id": ap.id, "capability": "browser.login_code", "kind": ap.kind, "reason": question,
                    "task_id": ctx.task_id, "summary": ap.summary, "why": ap.why
                }));
                e.extend(card_parts(ap));
                e
            },
            &[&question, &summary, &detail, &exact, &why],
        );
        let mut seal = Seal {
            gw: self,
            id: id.clone(),
            used: false,
        };
        let state = self
            .hidden_wait(&ctx.worker_id, &ctx.task_id, &id, None)
            .await?;
        seal.used = state == "granted";
        let answer = self.take_answer(&id);
        drop(seal);
        Ok(answer.filter(|_| state == "granted"))
    }

    /// `admin_password(grant, rec, command)`: the Mac password for the one approved admin command (D31), asked
    /// only after the owner's Yes to the command. Its question shows the command (GATE-566).
    pub(super) async fn admin_password(
        &self,
        grant: &RunGrant,
        rec: &TaskRecord,
        command: &str,
        cancel: &Cancel,
        left: &Left,
    ) -> Result<Option<Secret>, CallError> {
        if self.unattended(&rec.request_id) {
            return Ok(None);
        }
        let exact = if ferb_py::value::strip(command).is_empty() {
            String::new()
        } else {
            self.built(&self.value(command), false)
        };
        let id = self.hidden_question(
            "shell.admin",
            (&grant.task_id, &rec.request_id),
            ADMIN_QUESTION,
            |ap| {
                ap.kind = "owner_admin_password".into();
                ap.summary = ADMIN_SUMMARY.into();
                ap.detail = ADMIN_DETAIL.into();
                ap.exact = exact.clone();
                ap.title = ADMIN_TITLE.into();
                ap.secret_labels = ADMIN_LABELS.iter().map(|s| s.to_string()).collect();
            },
            |ap| {
                let mut e = obj(json!({
                    "approval_id": ap.id, "capability": "shell.admin", "kind": ap.kind, "reason": ADMIN_QUESTION,
                    "task_id": grant.task_id, "summary": ap.summary
                }));
                e.extend(card_parts(ap));
                e
            },
            &[&exact],
        );
        let mut seal = Seal {
            gw: self,
            id: id.clone(),
            used: false,
        };
        let state = self
            .hidden_wait(&grant.worker_id, &grant.task_id, &id, Some((cancel, left)))
            .await?;
        let answer = self.take_answer(&id).filter(|a| !a.expose().is_empty());
        let delivered = state == "granted" && answer.is_some();
        seal.used = delivered;
        drop(seal);
        Ok(answer.filter(|_| delivered))
    }

    fn unattended(&self, request_id: &str) -> bool {
        lock(&self.inner).approvals.unattended.contains(request_id)
    }

    /// The answer out of its question, once: no copy stays there.
    fn take_answer(&self, id: &str) -> Option<Secret> {
        let mut i = lock(&self.inner);
        if let Some(sealed) = i.approvals.take_sealed(id) {
            return Some(sealed);
        }
        let ap = i.approvals.get_mut(id)?;
        ap.answer.take().map(Secret::new)
    }

    /// `approvals.question(ask, question, [])` with the card set by `card`, published as `approval.requested`
    /// (`event`, FERB's own `marked` words kept as built): the question's id.
    #[allow(clippy::too_many_arguments)]
    fn hidden_question(
        &self,
        capability: &str,
        (task_id, request_id): (&str, &str),
        question: &str,
        card: impl FnOnce(&mut Approval),
        event: impl FnOnce(&Approval) -> Map<String, Value>,
        marked: &[&str],
    ) -> String {
        let ap = {
            let mut i = lock(&self.inner);
            let ap = i.approvals.question(
                &Asker {
                    capability,
                    task_id,
                    request_id,
                },
                question,
                &[],
                None,
            );
            card(ap);
            ap.clone()
        };
        self.publish_marked("approval.requested", event(&ap), &super::Marks::of(marked));
        ap.id
    }

    /// `_wait_answer` for a hidden input: its state once answered; "gone" when it closed with its task;
    /// CALLER_LEFT when the call that asks lost its caller first. A tool's hidden input is ended by dropping it
    /// (the call's run is cancelled around it); the admin password's by the call's own `cancel`.
    async fn hidden_wait(
        &self,
        worker: &str,
        task: &str,
        id: &str,
        call: Option<(&Cancel, &Left)>,
    ) -> Result<String, CallError> {
        self.observer().waiting(worker, true);
        *lock(&self.inner).waiting.entry(id.to_string()).or_insert(0) += 1;
        let mut w = Waiting {
            gw: self,
            id: id.to_string(),
            task: task.to_string(),
            worker: worker.to_string(),
            ended: false,
        };
        let never = Cancel::new();
        let (cancel, left): (&Cancel, Left) = match call {
            Some((c, l)) => (c, l.clone()),
            None => (&never, self.caller_left_of(task)),
        };
        let out = loop {
            let n = self.answers.notified();
            tokio::pin!(n);
            n.as_mut().enable();
            {
                let mut i = lock(&self.inner);
                match i.approvals.get_mut(id) {
                    None => break Ok("gone".to_string()),
                    Some(_) if left() => break Ok(super::CALLER_LEFT.to_string()),
                    Some(ap) if ap.state != State::Pending => {
                        ap.taken = true;
                        break Ok(ap.state.as_str().to_string());
                    }
                    Some(_) => {}
                }
            }
            let waited = cancel
                .guard(async {
                    tokio::select! {
                        () = &mut n => {},
                        () = tokio::time::sleep(std::time::Duration::from_millis(200)) => {},
                    }
                })
                .await;
            if let Err(e) = waited {
                break Err(e);
            }
        };
        w.ended = out.is_ok();
        drop(w);
        out
    }

    /// Whether the newest call in flight for this task lost its caller (`server.caller_left()`, which Python reads
    /// from the call that runs the tool).
    fn caller_left_of(&self, task: &str) -> Left {
        let found = lock(&self.inner)
            .in_flight
            .iter()
            .rev()
            .find(|(_, t, _, _)| t == task)
            .map(|(_, _, _, l)| l.clone());
        found.unwrap_or_else(|| Arc::new(|| false))
    }

    /// The task's tree root (`(tasks.tree_of(task) or task).split(".")[0]`).
    fn root_of(&self, task: &str) -> String {
        let tree = self.tree_of(task).unwrap_or_else(|| task.to_string());
        tree.split('.').next().unwrap_or(&tree).to_string()
    }

    /// `_refused` (GATE-562, owner GATE-563): the page refused this field in this task. Past ASKS_AFTER_REFUSAL
    /// fresh asks FERB asks no more and the step stops, saying what the page said; else the ask is counted.
    fn refused_now(
        &self,
        ctx: &ToolContext,
        target: &str,
        key: &str,
    ) -> Result<Option<Refusal>, HiddenError> {
        let root = self.root_of(&ctx.task_id);
        let host = super::page_of(target).0;
        let place = if host.is_empty() { target } else { &host };
        let place = place.to_lowercase();
        let bare = place.strip_prefix("www.").unwrap_or(&place).to_string();
        let mut i = lock(&self.inner);
        let Some(refusal) = i.refusals.get_mut(&root).and_then(|t| {
            t.iter_mut()
                .find(|(k, _)| *k == (bare.clone(), key.to_string()))
                .map(|(_, r)| r)
        }) else {
            return Ok(None);
        };
        if refusal.count == 0 {
            return Ok(None);
        }
        if refusal.asked_after >= ASKS_AFTER_REFUSAL {
            let said = if refusal.words.is_empty() {
                String::new()
            } else {
                format!(" It says: “{}”", refusal.words)
            };
            return Err(HiddenError::Refused(format!(
                "{} refused the {} the owner gave {} times in this task, and FERB has asked the owner {} more times \
                 since the first; it asks no more for it. Stop this step and report what the page said.{said}",
                refusal.place, refusal.what, refusal.count, refusal.asked_after
            )));
        }
        refusal.asked_after += 1;
        Ok(Some(refusal.clone()))
    }

    /// `_login_sites` then `_question_sites` (I14): the task's owner-typed sites a record at `target` lists, only
    /// when they are exactly one site and `target` is neither an app nor one of them. `secrets.list` reads them too
    /// (secret_tools.py `sites`), as links before any entry is saved there.
    pub fn question_sites(&self, ctx: &ToolContext, target: &str) -> Vec<String> {
        let places = self.d.places.as_ref();
        let request = self.request(&ctx.request_id);
        let own = site_host(places, target);
        let words = ferb_policy::provenance::prepare(&request.words);
        let typed_in = |t: &str| words.tokens.contains(t) || words.hosts.contains(t);
        let mut typed: Vec<String> = request
            .targets
            .sites
            .iter()
            .map(|s| site_host(places, s))
            .filter(|h| !h.is_empty() && typed_site(h, &typed_in))
            .collect();
        typed.sort();
        typed.dedup();
        if own.is_empty() || typed.iter().any(|h| same_site(h, &own)) {
            return Vec::new();
        }
        let bare: std::collections::BTreeSet<&str> = typed
            .iter()
            .map(|h| h.strip_prefix("www.").unwrap_or(h))
            .collect();
        if bare.len() == 1 {
            typed
        } else {
            Vec::new()
        }
    }

    /// `_payment(ctx, target, exact)` (GATE-204): the payment the owner approved in this task whose bank page this is.
    fn payment_of(
        &self,
        ctx: &ToolContext,
        target: &str,
        exact: bool,
    ) -> Option<payments::Approval> {
        let host = ferb_policy::urls::parse(target)
            .map(|p| p.host)
            .unwrap_or_default();
        if host.is_empty() {
            return None;
        }
        let tree = self.root_of(&ctx.task_id);
        let found = lock(&self.payments).for_otp(&tree, &host)?;
        if exact && found.bank_host.as_deref() != Some(host.as_str()) {
            return None;
        }
        Some(found)
    }

    /// `_payment_line`: what a code or secret asked on that bank page repeats; "" when no approved payment is its.
    fn payment_line(&self, ctx: &ToolContext, target: &str, exact: bool) -> String {
        match self.payment_of(ctx, target, exact) {
            Some(p) => payments::otp_line(Some(&p)),
            None => String::new(),
        }
    }

    /// `_call(ctx)`: the fill call this question serves, as its record has it, or {}.
    fn call_of(&self, ctx: &ToolContext) -> Map<String, Value> {
        let Some(span) = ctx.span_id.as_deref().filter(|s| !s.is_empty()) else {
            return Map::new();
        };
        let row: Option<Option<String>> = {
            let db = lock(&self.d.db);
            db.conn()
                .query_row("SELECT input_json FROM spans WHERE id = ?", [span], |r| {
                    r.get(0)
                })
                .ok()
        };
        let text = row.flatten().unwrap_or_else(|| "{}".into());
        match serde_json::from_str::<Value>(if text.is_empty() { "{}" } else { &text }) {
            Ok(Value::Object(m)) => m,
            _ => Map::new(),
        }
    }

    /// `_why` (GATE-559): the worker's reason for the fill, as confirmations show it.
    fn why_of(&self, ctx: &ToolContext) -> String {
        let why = self
            .call_of(ctx)
            .get("why")
            .filter(|w| ferb_py::value::truthy(w))
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        if ferb_py::value::strip(&why).is_empty() {
            return String::new();
        }
        self.built(
            &confirm::reason_line(&quoted(&self.value(&why), REASON_CHARS)),
            false,
        )
    }

    /// `_label`: the page's own label for the field the fill fills (the call's `control_label` fact), or "".
    fn label_of(&self, ctx: &ToolContext) -> String {
        match self.call_of(ctx).get("facts") {
            Some(Value::Object(f)) => f
                .get("control_label")
                .filter(|v| ferb_py::value::truthy(v))
                .map(ferb_py::value::py_str)
                .unwrap_or_default(),
            _ => String::new(),
        }
    }
}
