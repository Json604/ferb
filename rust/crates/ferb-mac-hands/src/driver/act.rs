//! mac.act's element and key actions (driver.py:1918-2470): the route the worker chose, refused when the action
//! doesn't take it; typing in chunks with the keyboard focus checked between them; edits through the selected-text
//! range; scrolling by scroll bar, Page keys or the pointer's wheel; pop-up items by exact title. No retry, no
//! ancestor press, no source substitution, no replay.

use std::sync::{Arc, Mutex};

use ferb_core::types::{
    ActionFacts, ActionResult, ComputerAction, ComputerActionKind as K, TerminalGrant,
};
use serde_json::{json, Value};

use super::{
    default_mechanism, element_routes, focused_typing, kind_str, ok_result, py_repr, with,
    MacDriver, Shot, A, FOCUS_ELSEWHERE, NEEDS_ELEMENT, SECURE_KEYS, SECURE_TYPED, SETTLE_S,
    TYPE_CHUNK,
};
use crate::ax::{AxNode, El, ScrollPosition};
use crate::error::{refused, stale, HandError, Result};
use crate::input::Step;
use crate::keys;

impl MacDriver {
    async fn terminal_check(
        self: &Arc<Self>,
        a: &A,
        el: Option<&El>,
        terminal: Option<&TerminalGrant>,
        task: Option<&str>,
        text: Option<&str>,
        keys_: Option<&str>,
        menu: Option<&str>,
    ) -> Result<bool> {
        let (a2, e, g, t, tx, k, m) = (
            a.clone(),
            el.cloned(),
            terminal.cloned(),
            task.map(String::from),
            text.map(String::from),
            keys_.map(String::from),
            menu.map(String::from),
        );
        if let Some(r) = self
            .blk(move |d| {
                d.terminal_refusal(
                    &a2,
                    e.as_ref(),
                    g.as_ref(),
                    t.as_deref(),
                    tx.as_deref(),
                    k.as_deref(),
                    m.as_deref(),
                )
            })
            .await
        {
            return refused(r);
        }
        if terminal.is_none() {
            return Ok(false);
        }
        let (a2, e) = (a.clone(), el.cloned());
        Ok(self
            .blk(move |d| d.terminal_why(&a2, e.as_ref()))
            .await
            .is_some())
    }

    async fn facts(
        self: &Arc<Self>,
        a: &A,
        target: Option<&El>,
        received: Option<&El>,
        terminal: Option<bool>,
        after: bool,
    ) -> ActionFacts {
        let (a2, t, r) = (a.clone(), target.cloned(), received.cloned());
        self.blk(move |d| d.facts_now(&a2, t.as_ref(), r.as_ref(), terminal, after))
            .await
    }

    /// driver.py:1920: mac.act.
    pub async fn act(
        self: &Arc<Self>,
        app: &str,
        action: &ComputerAction,
        holder: &str,
        task: Option<&str>,
        terminal: Option<&TerminalGrant>,
    ) -> Result<ActionResult> {
        if matches!(action.kind, K::Key | K::Type | K::SetValue | K::Select)
            && action.text.is_none()
        {
            return refused(format!(
                "{} requires explicit text; omitted text never means clear or Return",
                kind_str(action.kind)
            ));
        }
        let a = self.resolve(app).await?;
        let element = action.element.clone().filter(|e| !e.is_empty());
        {
            let st = self.state.lock().expect("lock");
            if let Some(s) = st.get(&a.bundle_id) {
                if element.is_some() && s.pid != 0 && s.pid != a.pid {
                    return stale("element process changed; take a new snapshot");
                }
            }
        }
        let mechanism = default_mechanism(action);
        if mechanism == "foreground" && !action.allow_foreground {
            return Ok(ok_result(
                false,
                "This explicit route needs task-wide foreground permission.",
                json!({"code": "needs_foreground", "method": mechanism, "performed": false}),
            ));
        }
        let routes = element_routes(action.kind);
        if !routes.contains(&mechanism) {
            return refused(format!(
                "{} takes mechanism {}, not {mechanism}; nothing was sent",
                kind_str(action.kind),
                routes.join(" or ")
            ));
        }
        if action.kind == K::Key {
            return self
                .act_key(
                    &a,
                    action.text.as_deref().unwrap_or(""),
                    holder,
                    task,
                    terminal,
                    mechanism,
                )
                .await;
        }
        if action.kind == K::Scroll {
            let n = match &element {
                Some(e) => Some(self.node(&a.bundle_id, e)?),
                None => None,
            };
            let r = self.scroll(&a, action, holder, mechanism).await?;
            let f = self
                .facts(&a, n.as_ref().map(|n| &n.r#ref), None, None, false)
                .await;
            return Ok(with(r, Some(f)));
        }
        let Some(element) = element else {
            if focused_typing(action) {
                let (r, into, landed) = self
                    .type_focused(
                        &a,
                        action.text.as_deref().unwrap_or(""),
                        holder,
                        task,
                        terminal,
                    )
                    .await?;
                let f = self
                    .facts(&a, landed.as_ref(), None, Some(into), true)
                    .await;
                return Ok(with(r, Some(f)));
            }
            return refused(NEEDS_ELEMENT);
        };
        let n = self.node(&a.bundle_id, &element)?;
        if !matches!(action.kind, K::Type | K::SetValue | K::Edit) {
            self.armed.lock().expect("lock").remove(&a.pid);
        }
        if matches!(action.kind, K::Press | K::Click) {
            let (a2, r, g) = (a.clone(), n.r#ref.clone(), terminal.cloned());
            if let Some(why) = self
                .blk(move |d| d.files_refusal(&a2, &[d.element_file(Some(&r))], g.as_ref()))
                .await
            {
                return refused(why);
            }
            if n.ax_role == "AXMenuItem" {
                let r = n.r#ref.clone();
                if let Some(combo) = self.blk(move |d| d.item_combo(Some(&r))).await {
                    if let Some(h) = self.guarded_combo(&a, &combo, task).await? {
                        return Ok(h);
                    }
                }
            }
        }
        if n.secret && matches!(action.kind, K::SetValue | K::Type | K::Edit) {
            return refused(SECURE_TYPED);
        }
        let (r, into, received, after) = self
            .act_element(&a, &n, action, holder, task, terminal, mechanism)
            .await?;
        let f = self
            .facts(&a, Some(&n.r#ref), received.as_ref(), Some(into), after)
            .await;
        Ok(with(r, Some(f)))
    }

    async fn act_key(
        self: &Arc<Self>,
        a: &A,
        combo: &str,
        holder: &str,
        task: Option<&str>,
        terminal: Option<&TerminalGrant>,
        mechanism: &str,
    ) -> Result<ActionResult> {
        if combo.is_empty() || !keys::valid_combo(combo) {
            return refused(format!("unsupported key {}", py_repr(combo)));
        }
        if let Some(h) = self.guarded_combo(a, combo, task).await? {
            return Ok(h);
        }
        let (a2, g) = (a.clone(), terminal.cloned());
        if let Some(r) = self
            .blk(move |d| d.files_refusal(&a2, &d.selected_files(&a2), g.as_ref()))
            .await
        {
            return refused(r);
        }
        let (a2, c) = (a.clone(), combo.to_string());
        let (path, control) = self.blk(move |d| d.key_run(&a2, &c)).await;
        if path.is_some() || control.is_some() {
            let (a2, g) = (a.clone(), terminal.cloned());
            if let Some(r) = self
                .blk(move |d| d.run_refusal(&a2, g.as_ref(), path.as_deref(), control.as_deref()))
                .await
            {
                return refused(r);
            }
        }
        let a2 = a.clone();
        let focus = self.blk(move |d| d.focus_of(&a2)).await;
        let f2 = focus.clone();
        if keys::enters_text(combo) && self.blk(move |d| d.secure(f2.as_ref())).await {
            return refused(SECURE_KEYS);
        }
        let into = self
            .terminal_check(a, focus.as_ref(), terminal, task, None, Some(combo), None)
            .await?;
        let r = self
            .key(a, combo, holder, mechanism, focus.as_ref())
            .await?;
        if into || self.armed.lock().expect("lock").contains_key(&a.pid) {
            self.terminal_typed(a, focus.as_ref(), None, task, false);
        }
        let f = self.facts(a, focus.as_ref(), None, Some(into), true).await;
        Ok(with(r, Some(f)))
    }

    #[allow(clippy::too_many_arguments)]
    async fn act_element(
        self: &Arc<Self>,
        a: &A,
        n: &AxNode,
        action: &ComputerAction,
        holder: &str,
        task: Option<&str>,
        terminal: Option<&TerminalGrant>,
        mechanism: &str,
    ) -> Result<(ActionResult, bool, Option<El>, bool)> {
        let text = action.text.clone().unwrap_or_default();
        match action.kind {
            K::Press | K::Click => {
                let (a2, r) = (a.clone(), n.r#ref.clone());
                if self.blk(move |d| d.runs(&a2, Some(&r))).await.is_some() {
                    let (a2, g, l) = (a.clone(), terminal.cloned(), n.label.clone());
                    if let Some(r) = self
                        .blk(move |d| d.run_refusal(&a2, g.as_ref(), None, Some(&l)))
                        .await
                    {
                        return refused(r);
                    }
                }
                let (mut focus, mut into) = (None, false);
                let r = n.r#ref.clone();
                if n.ax_role == "AXMenuItem" && self.blk(move |d| d.item_pastes(&r)).await {
                    let a2 = a.clone();
                    focus = self.blk(move |d| d.focus_of(&a2)).await;
                    let f2 = focus.clone();
                    if self.blk(move |d| d.secure(f2.as_ref())).await {
                        return refused(SECURE_KEYS);
                    }
                    into = self
                        .terminal_check(
                            a,
                            focus.as_ref(),
                            terminal,
                            task,
                            None,
                            Some("cmd+v"),
                            None,
                        )
                        .await?;
                } else if n.ax_role == "AXMenuItem" && {
                    let a2 = a.clone();
                    self.blk(move |d| d.terminal_app(&a2)).await
                } {
                    let a2 = a.clone();
                    focus = self.blk(move |d| d.focus_of(&a2)).await;
                    self.terminal_check(
                        a,
                        focus.as_ref(),
                        terminal,
                        task,
                        None,
                        None,
                        Some(&n.label),
                    )
                    .await?;
                }
                let r = if mechanism == "foreground" {
                    let center = self.ax.center(n);
                    let g = self.gestures.clone();
                    self.foreground_element(
                        a,
                        &n.r#ref,
                        center,
                        move |x, y| g.click_steps(x, y, None),
                        holder,
                        "click",
                        task,
                    )
                    .await?
                } else {
                    self.press(a, n).await?
                };
                if into {
                    self.terminal_typed(a, focus.as_ref(), terminal, task, r.ok);
                }
                Ok((r, into, Some(n.r#ref.clone()), false))
            }
            K::Type if mechanism == "foreground" => {
                let (r, into) = self
                    .type_foreground(a, n, &text, holder, task, terminal)
                    .await?;
                Ok((r, into, None, true))
            }
            K::Cancel => {
                let r = n.r#ref.clone();
                let ok = self.blk(move |d| d.ax.cancel(&r)).await?;
                Ok((
                    ok_result(
                        ok,
                        if ok {
                            "AXCancel accepted"
                        } else {
                            "AXCancel refused"
                        },
                        json!({"method": "ax", "accepted": ok, "outcome_verified": false}),
                    ),
                    false,
                    Some(n.r#ref.clone()),
                    false,
                ))
            }
            K::Focus => {
                let r = n.r#ref.clone();
                let ok = self.blk(move |d| d.ax.focus(&r)).await;
                self.profiles.record(&a.bundle_id, "ax_focus", ok);
                Ok((
                    ok_result(
                        ok,
                        if ok {
                            "AX focus request accepted"
                        } else {
                            "AX focus request refused"
                        },
                        json!({"method": "ax", "accepted": ok, "outcome_verified": false}),
                    ),
                    false,
                    None,
                    true,
                ))
            }
            K::SetValue => {
                let into = self
                    .terminal_check(a, Some(&n.r#ref), terminal, task, Some(&text), None, None)
                    .await?;
                let r = self.set_value(a, n, &text).await?;
                if into {
                    self.terminal_typed(a, Some(&n.r#ref), terminal, task, r.ok);
                }
                Ok((r, into, Some(n.r#ref.clone()), false))
            }
            K::Type => {
                let (r, into) = self.type_bg(a, n, &text, holder, task, terminal).await?;
                Ok((r, into, None, true))
            }
            K::Edit => {
                let into = match &action.text {
                    Some(t) => {
                        self.terminal_check(a, Some(&n.r#ref), terminal, task, Some(t), None, None)
                            .await?
                    }
                    None => false,
                };
                let r = self
                    .edit(a, n, &action.select, action.text.as_deref(), mechanism)
                    .await?;
                if into {
                    self.terminal_typed(a, Some(&n.r#ref), terminal, task, r.ok);
                }
                Ok((r, into, Some(n.r#ref.clone()), false))
            }
            K::Select => Ok((
                self.select(a, n, &text).await?,
                false,
                Some(n.r#ref.clone()),
                false,
            )),
            other => refused(format!("unknown action {}", kind_str(other))),
        }
    }

    fn chunk_guard(
        self: &Arc<Self>,
        a: &A,
        steps: Vec<Step>,
        landed: Arc<Mutex<Vec<Option<El>>>>,
        strict: Arc<Mutex<bool>>,
    ) -> Vec<Step> {
        let total = steps.len();
        steps
            .into_iter()
            .enumerate()
            .map(|(i, s)| {
                let (d, a, landed, strict) =
                    (self.clone(), a.clone(), landed.clone(), strict.clone());
                s.guarded(move || {
                    let l = landed.lock().expect("lock").clone();
                    if i > 0 && i % TYPE_CHUNK == 0 && !l.is_empty() {
                        d.focus_moved(&a, l[0].as_ref(), i, total, *strict.lock().expect("lock"))?;
                    }
                    Ok(())
                })
            })
            .collect()
    }

    async fn type_foreground(
        self: &Arc<Self>,
        a: &A,
        n: &AxNode,
        text: &str,
        holder: &str,
        task: Option<&str>,
        terminal: Option<&TerminalGrant>,
    ) -> Result<(ActionResult, bool)> {
        let landed: Arc<Mutex<Vec<Option<El>>>> = Arc::new(Mutex::new(vec![]));
        let strict = Arc::new(Mutex::new(false));
        let steps = self.gestures.type_steps(text, None);
        let steps = self.chunk_guard(a, steps, landed.clone(), strict.clone());
        let prepare = {
            let (d, a, r, landed, strict, g, t, tx) = (
                self.clone(),
                a.clone(),
                n.r#ref.clone(),
                landed.clone(),
                strict.clone(),
                terminal.cloned(),
                task.map(String::from),
                text.to_string(),
            );
            async move {
                let r2 = r.clone();
                d.blk(move |d| d.ax.focus(&r2)).await;
                let (pid, r2) = (a.pid, r.clone());
                if d.blk(move |d| d.ax.has_focus(pid, &r2)).await != Some(true) {
                    return refused(FOCUS_ELSEWHERE);
                }
                let a2 = a.clone();
                let focus = d.blk(move |d| d.focus_of(&a2)).await;
                let (a2, f2, r2) = (a.clone(), focus.clone(), r.clone());
                let target = d.blk(move |d| d.landing(&a2, f2.as_ref(), Some(&r2))).await;
                landed.lock().expect("lock").push(Some(focus.unwrap_or(r)));
                if d.terminal_check(
                    &a,
                    target.as_ref(),
                    g.as_ref(),
                    t.as_deref(),
                    Some(&tx),
                    None,
                    None,
                )
                .await?
                {
                    landed.lock().expect("lock").push(None);
                    *strict.lock().expect("lock") = true;
                }
                Ok(())
            }
        };
        let r = self
            .real(a, steps, holder, "typing", task, Some(Box::pin(prepare)))
            .await?;
        let l = landed.lock().expect("lock").clone();
        let into = l.len() > 1;
        if into {
            self.terminal_typed(a, l[0].as_ref(), terminal, task, r.ok);
        }
        Ok((r, into))
    }

    async fn type_focused(
        self: &Arc<Self>,
        a: &A,
        text: &str,
        holder: &str,
        task: Option<&str>,
        terminal: Option<&TerminalGrant>,
    ) -> Result<(ActionResult, bool, Option<El>)> {
        let landed: Arc<Mutex<Vec<Option<El>>>> = Arc::new(Mutex::new(vec![]));
        let strict = Arc::new(Mutex::new(false));
        let steps = self.gestures.type_steps(text, None);
        let steps = self.chunk_guard(a, steps, landed.clone(), strict.clone());
        let prepare = {
            let (d, a, landed, strict, g, t, tx) = (
                self.clone(),
                a.clone(),
                landed.clone(),
                strict.clone(),
                terminal.cloned(),
                task.map(String::from),
                text.to_string(),
            );
            async move {
                let a2 = a.clone();
                let focus = d.blk(move |d| d.focus_of(&a2)).await;
                let a2 = a.clone();
                if focus.is_none() && !d.blk(move |d| d.hosts(&a2)).await {
                    return refused(super::no_focus(&a.name));
                }
                let f2 = focus.clone();
                if d.blk(move |d| d.secure(f2.as_ref())).await {
                    return refused(SECURE_TYPED);
                }
                landed.lock().expect("lock").push(focus.clone());
                if d.terminal_check(
                    &a,
                    focus.as_ref(),
                    g.as_ref(),
                    t.as_deref(),
                    Some(&tx),
                    None,
                    None,
                )
                .await?
                {
                    landed.lock().expect("lock").push(None);
                    *strict.lock().expect("lock") = true;
                }
                Ok(())
            }
        };
        let r = self
            .real(a, steps, holder, "typing", task, Some(Box::pin(prepare)))
            .await?;
        let l = landed.lock().expect("lock").clone();
        let into = l.len() > 1;
        if into {
            self.terminal_typed(a, l[0].as_ref(), terminal, task, r.ok);
        }
        Ok((r, into, l.first().cloned().flatten()))
    }

    async fn press(self: &Arc<Self>, a: &A, n: &AxNode) -> Result<ActionResult> {
        let r = n.r#ref.clone();
        let ok = self.blk(move |d| d.ax.press(&r)).await?;
        self.profiles.record(&a.bundle_id, "ax_press", ok);
        Ok(ok_result(
            ok,
            if ok {
                "AX press request accepted; inspect the outcome"
            } else {
                "AX press request refused; check what happened before trying again (if it may have gone through, a repeat needs the owner)"
            },
            json!({"method": "ax", "accepted": ok, "outcome_verified": false}),
        ))
    }

    async fn value_of(self: &Arc<Self>, el: &El) -> Option<String> {
        let e = el.clone();
        self.blk(move |d| d.ax.value(&e)).await
    }

    async fn value_changed(
        self: &Arc<Self>,
        n: &AxNode,
        before: Option<&str>,
    ) -> (bool, Option<String>) {
        let deadline = self.now() + SETTLE_S;
        loop {
            let after = self.value_of(&n.r#ref).await;
            if after.as_deref() != before {
                return (true, after);
            }
            if self.now() >= deadline {
                return (false, after);
            }
            self.sleep(0.05).await;
        }
    }

    async fn value_reads(self: &Arc<Self>, n: &AxNode, text: &str) -> Option<String> {
        let deadline = self.now() + SETTLE_S;
        loop {
            let observed = self.value_of(&n.r#ref).await;
            if observed.as_deref() == Some(text) || self.now() >= deadline {
                return observed;
            }
            self.sleep(0.05).await;
        }
    }

    async fn set_value(self: &Arc<Self>, a: &A, n: &AxNode, text: &str) -> Result<ActionResult> {
        let (r, t) = (n.r#ref.clone(), text.to_string());
        let accepted = self.blk(move |d| d.ax.set_value(&r, &t)).await?;
        let observed = if accepted {
            self.value_reads(n, text).await
        } else {
            self.value_of(&n.r#ref).await
        };
        let matches = observed.as_deref() == Some(text);
        let ok = accepted && matches;
        self.profiles.record(&a.bundle_id, "ax_value", ok);
        let mut facts =
            json!({"method": "ax_value", "accepted": accepted, "value_matches": matches});
        if ok {
            return Ok(ok_result(true, "value set and read back", facts));
        }
        if let Some(o) = &observed {
            facts["field_empty"] = json!(o.is_empty());
            facts["field_length"] = json!(o.chars().count());
        }
        Ok(ok_result(false, "AXValue result not confirmed; no typing attempted. The field may already hold text: read it in a fresh snapshot before typing again (typing adds to what is there)", facts))
    }

    pub(super) fn landing(&self, a: &A, focus: Option<&El>, el: Option<&El>) -> Option<El> {
        if let Some(f) = focus {
            return Some(f.clone());
        }
        if self.hosts(a) {
            None
        } else {
            el.cloned()
        }
    }

    async fn type_bg(
        self: &Arc<Self>,
        a: &A,
        n: &AxNode,
        text: &str,
        holder: &str,
        task: Option<&str>,
        terminal: Option<&TerminalGrant>,
    ) -> Result<(ActionResult, bool)> {
        let r = n.r#ref.clone();
        self.blk(move |d| d.ax.focus(&r)).await;
        let (pid, r) = (a.pid, n.r#ref.clone());
        if self.blk(move |d| d.ax.has_focus(pid, &r)).await != Some(true) {
            return refused(FOCUS_ELSEWHERE);
        }
        let a2 = a.clone();
        let focus = self.blk(move |d| d.focus_of(&a2)).await;
        let (a2, f2, r) = (a.clone(), focus.clone(), n.r#ref.clone());
        let landed = self
            .blk(move |d| d.landing(&a2, f2.as_ref(), Some(&r)))
            .await;
        let into = self
            .terminal_check(a, landed.as_ref(), terminal, task, Some(text), None, None)
            .await?;
        let r = self.typed(a, n, text, holder, focus.as_ref(), into).await?;
        if into {
            self.terminal_typed(a, landed.as_ref(), terminal, task, r.ok);
        }
        Ok((r, into))
    }

    /// driver.py:2174: between chunks of typed text: approved terminal typing stops when the focus moved at all;
    /// other typing stops when it moved into a terminal (or can't be read where a terminal could be).
    fn focus_moved(
        &self,
        a: &A,
        landed: Option<&El>,
        done: usize,
        total: usize,
        strict: bool,
    ) -> Result<()> {
        let now = self.focus_of(a);
        let moved = match landed {
            None => now.is_some(),
            Some(l) => now.as_ref().is_none_or(|n| !self.ax.same(n, l)),
        };
        if strict && moved {
            return Err(HandError::PointerInterrupted(format!("the keyboard focus moved after {done} of {total} characters, so the rest wasn't typed; check the field and the app before trying again")));
        }
        if !strict && now.is_none() && landed.is_some() && self.hosts(a) {
            return Err(HandError::PointerInterrupted(format!("FERB couldn't read where the keyboard focus is after {done} of {total} characters (it could be a terminal), so the rest wasn't typed; check the field and the app before trying again")));
        }
        if !strict
            && now.is_some()
            && !self.same_el(now.as_ref(), landed)
            && self.terminal_why(a, now.as_ref()).is_some()
        {
            return Err(HandError::PointerInterrupted(format!("the keyboard focus moved into a terminal after {done} of {total} characters, so the rest wasn't typed; check the field and the app before trying again")));
        }
        Ok(())
    }

    async fn typed(
        self: &Arc<Self>,
        a: &A,
        n: &AxNode,
        text: &str,
        holder: &str,
        landed: Option<&El>,
        strict: bool,
    ) -> Result<ActionResult> {
        let before = self.value_of(&n.r#ref).await;
        let sig = self.signature(a, Some(&n.r#ref)).await;
        let steps = self.gestures.type_steps(text, Some(a.pid));
        let land = Arc::new(Mutex::new(vec![landed.cloned()]));
        let steps = self.chunk_guard(a, steps, land, Arc::new(Mutex::new(strict)));
        self.input
            .to_process(steps, holder, None, Some(TYPE_CHUNK))
            .await?;
        self.sleep(SETTLE_S).await;
        let after = self.value_of(&n.r#ref).await;
        let changed = if before.is_some() {
            after != before
        } else {
            self.signature(a, Some(&n.r#ref)).await != sig
        };
        self.profiles.record(&a.bundle_id, "pid_keys", changed);
        Ok(ok_result(
            true,
            format!(
                "typing delivered in the background; {}",
                if changed {
                    "a value change was observed"
                } else {
                    "result unconfirmed; inspect before retrying"
                }
            ),
            json!({"method": "pid", "delivered": true, "observed_change": changed, "outcome_verified": false, "replayed": false}),
        ))
    }

    /// driver.py:2221: insert or replace text at an exact range.
    async fn edit(
        self: &Arc<Self>,
        a: &A,
        n: &AxNode,
        select: &Value,
        text: Option<&str>,
        mechanism: &str,
    ) -> Result<ActionResult> {
        let value = self.value_of(&n.r#ref).await;
        let length = value
            .as_deref()
            .map(|v| v.chars().count() as i64)
            .unwrap_or(0);
        let rng: (i64, i64) = match select {
            Value::Null => self.caret(n).await?,
            Value::String(s) if s.is_empty() || s == "caret" => self.caret(n).await?,
            Value::String(s) if s == "all" => (0, length),
            Value::String(s) if s == "end" => (length, 0),
            Value::String(s) if s == "start" => (0, 0),
            Value::Array(v)
                if v.len() == 2
                    && v.iter()
                        .all(|x| x.as_i64().is_some() || x.as_f64().is_some()) =>
            {
                let i = |x: &Value| {
                    x.as_i64()
                        .unwrap_or_else(|| x.as_f64().unwrap_or(0.0).trunc() as i64)
                };
                (i(&v[0]), i(&v[1]))
            }
            other => {
                return refused(format!(
                    "select must be (start, length), \"all\", \"end\" or \"start\", not {}",
                    crate::tools::py_value_repr(other)
                ))
            }
        };
        let (start, count) = rng;
        if value.is_none() || start < 0 || count < 0 || start + count > length {
            return refused("edit range cannot be verified against the current full value");
        }
        if mechanism != "ax" && mechanism != "ax_value" {
            return refused("edit uses ax selected text or explicit ax_value rewrite");
        }
        let ok = mechanism == "ax" && {
            let r = n.r#ref.clone();
            self.blk(move |d| d.ax.set_selected_range(&r, start, count))
                .await
        };
        let Some(text) = text else {
            if ok {
                let r = n.r#ref.clone();
                let got = self.blk(move |d| d.ax.selected_range(&r)).await;
                self.profiles.record(
                    &a.bundle_id,
                    "ax_selected_text",
                    got == Some((start, count)),
                );
                if got == Some((start, count)) {
                    return Ok(ok_result(
                        true,
                        format!("selected {count} characters from {start}"),
                        json!({}),
                    ));
                }
                let shown = got
                    .map(|(x, y)| format!("({x}, {y})"))
                    .unwrap_or_else(|| "None".into());
                return Ok(ok_result(
                    false,
                    format!("the selection didn't take (the box reports {shown})"),
                    json!({}),
                ));
            }
            return Ok(ok_result(
                false,
                "that box doesn't let its selection be set",
                json!({}),
            ));
        };
        let v = value.clone().unwrap_or_default();
        let chars: Vec<char> = v.chars().collect();
        let expected: String = chars[..start as usize].iter().collect::<String>()
            + text
            + &chars[(start + count) as usize..].iter().collect::<String>();
        if ok && {
            let (r, t) = (n.r#ref.clone(), text.to_string());
            self.blk(move |d| d.ax.set_selected_text(&r, &t)).await
        } {
            let (landed, after) = self.value_changed(n, value.as_deref()).await;
            if after.as_deref() == Some(expected.as_str()) || (value.is_none() && landed) {
                self.profiles.record(&a.bundle_id, "ax_selected_text", true);
                let n_text = text.chars().count();
                return Ok(ok_result(
                    true,
                    if count > 0 {
                        format!("edited: {count} characters at {start} replaced with the text ({n_text} characters)")
                    } else {
                        format!("inserted {n_text} characters at {start}")
                    },
                    json!({}),
                ));
            }
        }
        if mechanism == "ax" {
            self.profiles
                .record(&a.bundle_id, "ax_selected_text", false);
        }
        if mechanism == "ax_value" {
            let (r, e) = (n.r#ref.clone(), expected.clone());
            if self.blk(move |d| d.ax.set_value(&r, &e)).await?
                && self.value_of(&n.r#ref).await.as_deref() == Some(expected.as_str())
            {
                self.profiles.record(&a.bundle_id, "ax_value", true);
                return Ok(ok_result(
                    true,
                    "edited by rewriting the box's whole value (the app's undo sees one change)",
                    json!({}),
                ));
            }
        }
        Ok(ok_result(
            false,
            "the selected edit method did not confirm the requested value; inspect before retrying",
            json!({"method": mechanism, "outcome_verified": false, "replayed": false}),
        ))
    }

    async fn caret(self: &Arc<Self>, n: &AxNode) -> Result<(i64, i64)> {
        let r = n.r#ref.clone();
        match self.blk(move |d| d.ax.selected_range(&r)).await {
            Some(rng) => Ok(rng),
            None => refused(
                "caret range unavailable; choose an explicit range after inspecting the value",
            ),
        }
    }

    async fn key(
        self: &Arc<Self>,
        a: &A,
        combo: &str,
        holder: &str,
        mechanism: &str,
        focus: Option<&El>,
    ) -> Result<ActionResult> {
        let parts = keys::normalize(combo);
        let (key, mods) = (
            parts[parts.len() - 1].clone(),
            parts[..parts.len() - 1].to_vec(),
        );
        if mechanism == "foreground" {
            let steps = self.gestures.key_steps(combo, None);
            return self.real(a, steps, holder, combo, None, None).await;
        }
        if mechanism == "ax" {
            let pid = a.pid;
            let found = self
                .blk(move |d| d.ax.menu_item_for_shortcut(pid, &key, &mods))
                .await;
            let Some((item, path, enabled)) = found else {
                return Ok(ok_result(
                    false,
                    "no unique menu item exposes that key equivalent; inspect mac.menu",
                    json!({"method": "ax", "performed": false}),
                ));
            };
            if !enabled {
                return Ok(ok_result(
                    false,
                    format!("menu item {} is disabled", py_repr(&path)),
                    json!({"method": "ax", "performed": false}),
                ));
            }
            let ok = self.blk(move |d| d.ax.press(&item)).await?;
            return Ok(ok_result(
                ok,
                format!(
                    "AX menu request {}: {path}",
                    if ok { "accepted" } else { "refused" }
                ),
                json!({"method": "ax", "accepted": ok, "outcome_verified": false}),
            ));
        }
        if mechanism != "pid" {
            return refused("key takes mechanism pid, ax (its menu item) or foreground");
        }
        let before = self.signature(a, focus).await;
        self.input
            .to_process(
                self.gestures.key_steps(combo, Some(a.pid)),
                holder,
                None,
                None,
            )
            .await?;
        self.sleep(SETTLE_S).await;
        let changed = self.signature(a, focus).await != before;
        if changed {
            self.profiles.record(&a.bundle_id, "pid_keycodes", true);
        }
        Ok(ok_result(
            true,
            format!(
                "{combo} sent in the background; {}",
                if changed {
                    "state changed; inspect the outcome"
                } else {
                    "nothing visibly changed; result unconfirmed"
                }
            ),
            json!({"method": mechanism, "delivered": true, "observed_change": changed, "outcome_verified": false, "replayed": false}),
        ))
    }

    fn scroll_boundary(position: &ScrollPosition, direction: &str) -> bool {
        let value = if direction == "up" || direction == "down" {
            position.vertical
        } else {
            position.horizontal
        };
        value.is_some_and(|v| {
            if direction == "down" || direction == "right" {
                v == 1.0
            } else {
                v == 0.0
            }
        })
    }

    /// driver.py:2305.
    async fn scroll(
        self: &Arc<Self>,
        a: &A,
        action: &ComputerAction,
        holder: &str,
        mechanism: &str,
    ) -> Result<ActionResult> {
        let direction = ferb_sanitize::parity::lower(action.direction.as_deref().unwrap_or("down"));
        if !["up", "down", "left", "right"].contains(&direction.as_str()) {
            return refused("scroll direction must be up, down, left or right");
        }
        let amount = action.amount.unwrap_or(1);
        let amount = if amount == 0 { 1 } else { amount };
        if !(1..=50).contains(&amount) {
            return refused("scroll amount must be 1–50 per bounded call");
        }
        let n = match action.element.as_deref().filter(|e| !e.is_empty()) {
            Some(e) => Some(self.node(&a.bundle_id, e)?),
            None => None,
        };
        if let Some(n) = n
            .as_ref()
            .filter(|n| n.ax_role == "AXSlider" || n.ax_role == "AXStepper")
        {
            let act = if direction == "up" || direction == "right" {
                "AXIncrement"
            } else {
                "AXDecrement"
            };
            let mut done = 0;
            for _ in 0..amount {
                let r = n.r#ref.clone();
                if !self.blk(move |d| d.ax.perform(&r, act)).await? {
                    break;
                }
                done += 1;
            }
            let now = self.value_of(&n.r#ref).await;
            return Ok(ok_result(
                done > 0,
                format!(
                    "slider: {done} step(s); value now {}",
                    now.unwrap_or_else(|| "None".into())
                ),
                json!({}),
            ));
        }
        let area = match &n {
            Some(n) => {
                let r = n.r#ref.clone();
                self.blk(move |d| d.ax.scroll_area_of(&r)).await
            }
            None => {
                let (pid, w) = (a.pid, action.window.clone().filter(|w| !w.is_empty()));
                self.blk(move |d| {
                    let win = match &w {
                        Some(w) => d.ax.window_named(pid, w),
                        None => d.ax.app_window(pid),
                    };
                    win.and_then(|w| d.ax.scroll_area_in(&w))
                })
                .await
            }
        };
        let Some(area) = area else {
            return Ok(ok_result(false, "no unique scroll container: choose an element inside the intended pane, or use mac.pointer action=scroll at a fresh screenshot point", json!({})));
        };
        if mechanism == "foreground" {
            let ar = area.clone();
            let Some(rect) = self.blk(move |d| d.ax.frame(&ar)).await else {
                return Ok(ok_result(
                    false,
                    "scroll pane has no current bounds",
                    json!({"performed": false}),
                ));
            };
            let lines = if direction == "down" || direction == "right" {
                -10
            } else {
                10
            };
            let horizontal = direction == "left" || direction == "right";
            let g = self.gestures.clone();
            return self
                .foreground_element(
                    a,
                    &area,
                    (rect.0 + rect.2 / 2.0, rect.1 + rect.3 / 2.0),
                    move |x, y| {
                        g.wheel_steps(
                            x,
                            y,
                            lines,
                            None,
                            horizontal,
                            None,
                            false,
                            false,
                            (0.0, 0.0),
                        )
                    },
                    holder,
                    "scroll",
                    None,
                )
                .await;
        }
        if mechanism != "ax" && mechanism != "pid" {
            return refused(
                "scroll supports ax scrollbar, pid text page keys or explicit foreground",
            );
        }
        let ar = area.clone();
        let content = self.blk(move |d| d.ax.scroll_content(&ar)).await;
        let vertical = direction == "up" || direction == "down";
        let ar = area.clone();
        let bar = self.blk(move |d| d.ax.scroll_bar(&ar, vertical)).await;
        let text_view = n.as_ref().is_some_and(|n| n.ax_role == "AXTextArea") && vertical;
        if text_view && mechanism == "pid" {
            let (a2, r) = (a.clone(), n.as_ref().map(|n| n.r#ref.clone()));
            if self
                .blk(move |d| d.terminal_why(&a2, r.as_ref()))
                .await
                .is_some()
            {
                return Ok(ok_result(false, "that's a terminal: FERB sends it no Page keys unasked (D31); scroll it with mac.pointer action=scroll at a fresh screenshot point", json!({"performed": false})));
            }
        }
        let (mut moved, mut route, mut stop) =
            (0, String::new(), "batch limit reached".to_string());
        let position = |d: &Arc<Self>| {
            let (d, ar, c) = (d.clone(), area.clone(), content.clone());
            async move {
                d.blk(move |d| d.ax.scroll_position(Some(&ar), c.as_ref()))
                    .await
            }
        };
        let mut before = position(self).await;
        for _ in 0..amount {
            let front = self.blk(|d| d.apps.frontmost()).await;
            if (self.input.owner_active)() && front.is_some_and(|f| f.pid == a.pid) {
                stop = "owner is using this app".into();
                break;
            }
            if Self::scroll_boundary(&before, &direction) {
                stop = "scroll bar boundary reached; verify content before claiming the last item"
                    .into();
                break;
            }
            if let (Some(b), "ax") = (&bar, mechanism) {
                let act = if direction == "down" || direction == "right" {
                    "AXIncrement"
                } else {
                    "AXDecrement"
                };
                let b = b.clone();
                if !self.blk(move |d| d.ax.perform(&b, act)).await? {
                    stop = "AX scrollbar increment refused; no alternate route attempted".into();
                    break;
                }
                route = "scroll bar".into();
            } else if text_view && mechanism == "pid" {
                let r = n.as_ref().expect("text view").r#ref.clone();
                if !self.blk(move |d| d.ax.focus(&r)).await {
                    stop = "text view could not take focus".into();
                    break;
                }
                let k = if direction == "up" {
                    "pageup"
                } else {
                    "pagedown"
                };
                self.input
                    .to_process(self.gestures.key_steps(k, Some(a.pid)), holder, None, None)
                    .await?;
                route = if direction == "up" {
                    "Page Up".into()
                } else {
                    "Page Down".into()
                };
            } else {
                stop = "no directional AX route; use mac.pointer action=scroll with a fresh screenshot".into();
                break;
            }
            self.sleep(SETTLE_S).await;
            let after = position(self).await;
            if after == before {
                stop = "movement not confirmed; this does not establish the end".into();
                break;
            }
            moved += 1;
            before = after;
        }
        let detail = format!(
            "scrolled {direction}: {moved}/{amount} step(s) via {}; {stop}",
            if route.is_empty() { mechanism } else { &route }
        );
        if moved > 0 || stop.contains("boundary reached") {
            return Ok(ok_result(true, detail, json!({})));
        }
        Ok(ok_result(
            false,
            detail,
            json!({"method": mechanism, "observed_steps": moved, "replayed": false}),
        ))
    }

    /// driver.py:2396: wheel steps at the captured point, each guarded; stops on unconfirmed progress.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn pointer_scroll(
        self: &Arc<Self>,
        a: &A,
        shot: &Shot,
        point: (f64, f64),
        direction: &str,
        amount: i64,
        holder: &str,
        route: bool,
        guard: Arc<dyn Fn() -> Result<()> + Send + Sync>,
        distance: f64,
    ) -> Result<ActionResult> {
        let tree = self
            .state
            .lock()
            .expect("lock")
            .get(&a.bundle_id)
            .and_then(|s| s.tree.clone());
        let mut area = None;
        if let Some(t) = tree {
            let mut nodes = t.nodes.clone();
            nodes.sort_by(|x, y| {
                (x.rect.2 * x.rect.3)
                    .partial_cmp(&(y.rect.2 * y.rect.3))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            for node in nodes {
                let (x, y, w, h) = node.rect;
                if x <= point.0 && point.0 < x + w && y <= point.1 && point.1 < y + h {
                    let r = node.r#ref.clone();
                    area = self.blk(move |d| d.ax.scroll_area_of(&r)).await;
                    if area.is_some() {
                        break;
                    }
                }
            }
        }
        let content = match &area {
            Some(ar) => {
                let ar = ar.clone();
                self.blk(move |d| d.ax.scroll_content(&ar)).await
            }
            None => None,
        };
        let observe = |d: &Arc<Self>| {
            let (d, ar, c) = (d.clone(), area.clone(), content.clone());
            async move {
                match ar {
                    Some(ar) => {
                        d.blk(move |d| d.ax.scroll_position(Some(&ar), c.as_ref()))
                            .await
                    }
                    None => ScrollPosition::default(),
                }
            }
        };
        let mut before = observe(self).await;
        if let Some(s) = self.state.lock().expect("lock").get_mut(&a.bundle_id) {
            s.shot = None;
        }
        let (mut moved, mut sent, mut stop) = (0, 0, "batch limit reached".to_string());
        let viewport = match &area {
            Some(ar) => {
                let ar = ar.clone();
                self.blk(move |d| d.ax.frame(&ar)).await
            }
            None => Some(shot.bounds),
        };
        let vertical = direction == "up" || direction == "down";
        let span = match viewport {
            Some(v) => {
                if vertical {
                    v.3
                } else {
                    v.2
                }
            }
            None => shot.bounds.3,
        };
        let pixels = ((span * distance).round_ties_even() as i64).clamp(1, 800) as i32;
        let lines = if direction == "down" || direction == "right" {
            -pixels
        } else {
            pixels
        };
        for _ in 0..amount {
            if !before.is_empty() && Self::scroll_boundary(&before, direction) {
                stop = "scroll bar boundary reached; verify content".into();
                break;
            }
            let steps = self.gestures.wheel_steps(
                point.0,
                point.1,
                lines,
                Some(a.pid),
                !vertical,
                Some(shot.window as u32),
                route,
                true,
                (shot.bounds.0, shot.bounds.1),
            );
            let steps: Vec<Step> = steps
                .into_iter()
                .map(|s| {
                    let g = guard.clone();
                    s.guarded(move || g())
                })
                .collect();
            if let Err(e) = self.input.to_process(steps, holder, None, None).await {
                if sent > 0 && matches!(e, HandError::Refused(_) | HandError::StaleRef(_)) {
                    return Err(HandError::PointerInterrupted(format!(
                        "scroll interrupted after {sent} ticks; inspect the result: {e}"
                    )));
                }
                return Err(e);
            }
            sent += 1;
            self.sleep(SETTLE_S).await;
            let after = observe(self).await;
            if after.is_empty() || after == before {
                stop =
                    "movement not confirmed; take a fresh screenshot; end not established".into();
                break;
            }
            moved += 1;
            before = after;
        }
        Ok(ok_result(moved > 0 || stop.contains("boundary reached"), format!("scroll {direction} ({pixels}px/step): sent {sent}/{amount} viewport-sized steps, {moved} observed changes; {stop}"), json!({})))
    }

    /// driver.py:2449: a pop-up button's item by exact title.
    async fn select(self: &Arc<Self>, a: &A, n: &AxNode, title: &str) -> Result<ActionResult> {
        let want = ferb_sanitize::parity::lower(ferb_py::value::strip(title));
        if want.is_empty() {
            return refused("select needs the item's title in text");
        }
        let r = n.r#ref.clone();
        if self.blk(move |d| d.ax.press(&r)).await? {
            let (mut menu, mut items) = (None, vec![]);
            for _ in 0..8 {
                self.sleep(0.2).await;
                let (r, pid) = (n.r#ref.clone(), a.pid);
                menu = self.blk(move |d| d.ax.popup_menu(&r, pid)).await;
                if let Some(m) = menu.clone() {
                    items = self.blk(move |d| d.ax.menu_items(&m)).await;
                    if !items.is_empty() {
                        break;
                    }
                }
            }
            let matches: Vec<El> = items
                .iter()
                .filter(|(_, t)| t == title)
                .map(|(e, _)| e.clone())
                .collect();
            if matches.len() == 1 {
                let hit = matches[0].clone();
                if self.blk(move |d| d.ax.press(&hit)).await? {
                    self.profiles.record(&a.bundle_id, "ax_press", true);
                    self.sleep(0.3).await;
                    let now = self.value_of(&n.r#ref).await;
                    let shown = now.as_deref().map(py_repr).unwrap_or_else(|| "None".into());
                    return Ok(ok_result(
                        true,
                        format!(
                            "AX selection press accepted for {}; observed value: {shown}",
                            py_repr(title)
                        ),
                        json!({"method": "ax", "accepted": true, "observed_value": now, "outcome_verified": false}),
                    ));
                }
            }
            if let Some(m) = menu {
                let _ = self.blk(move |d| d.ax.cancel(&m)).await;
            }
            if !items.is_empty() {
                let candidates: Vec<Value> =
                    items.iter().map(|(_, t)| json!({"label": t})).collect();
                return Ok(ok_result(
                    false,
                    format!(
                        "no unique exact item {}; inspect the returned candidates",
                        py_repr(title)
                    ),
                    json!({"code": "target_unresolved", "matches": matches.len(), "candidates": candidates, "performed": false}),
                ));
            }
        }
        Ok(ok_result(
            false,
            "no unique exact menu item; inspect candidates and choose an explicit element",
            json!({"code": "target_unresolved", "performed": false}),
        ))
    }
}
