//! The driver's terminal facts (D31, rule 10; driver.py:1281-1916): what input would reach a terminal, a run
//! command or a file that runs when opened, for the gateway's question before the call and the refusal right before
//! delivery; and the structural facts of an input (GATE-202). Blocking: the gateway and the driver run these in a
//! worker thread.

use ferb_core::types::{
    ActionFacts, ActionResult, ComputerAction, ComputerActionKind as K, ReceivedBy, TerminalGrant,
    TerminalInput,
};
use serde_json::json;

use super::{
    file_facts, focused_typing, pastes, pastes_item, MacDriver, A, RUNS_OPENED, WINDOW_WALK_NODES,
    WINDOW_WALK_S,
};
use crate::ax::El;
use crate::error::{refused, HandError, Result};
use crate::keys;

type Files = Vec<(String, String, String)>;

impl MacDriver {
    pub(super) fn focus_of(&self, a: &A) -> Option<El> {
        self.ax.focused_element(a.pid)
    }

    pub(super) fn same_el(&self, x: Option<&El>, y: Option<&El>) -> bool {
        match (x, y) {
            (Some(x), Some(y)) => self.ax.same(x, y),
            _ => false,
        }
    }

    /// driver.py:1292: why code counts this element a terminal; no facts in an app that can host one counts too.
    pub(super) fn terminal_why(&self, a: &A, el: Option<&El>) -> Option<String> {
        let chain = el
            .and_then(|e| self.ax.surface_facts(e))
            .unwrap_or_default();
        self.policy.surface(
            &a.bundle_id,
            &chain,
            &(self.terminal_apps)(),
            self.apps.declares_shell(&a.bundle_id),
            chain.is_empty(),
        )
    }

    pub(super) fn app_terminal_why(&self, a: &A) -> Option<String> {
        self.policy.surface(
            &a.bundle_id,
            &[],
            &(self.terminal_apps)(),
            self.apps.declares_shell(&a.bundle_id),
            false,
        )
    }

    pub(super) fn hosts(&self, a: &A) -> bool {
        self.policy.can_host(
            &a.bundle_id,
            &(self.terminal_apps)(),
            self.apps.declares_shell(&a.bundle_id),
        )
    }

    pub(super) fn role(&self, el: Option<&El>) -> String {
        el.map(|e| self.ax.role_of(e)).unwrap_or_default()
    }

    /// driver.py:1314: what a run press would run (the active editor's selection and file).
    pub(super) fn run_facts(&self, a: &A) -> (Option<String>, Option<String>) {
        let mut editor = self.ax.active_editor(a.pid);
        if editor.is_none() {
            let focus = self.focus_of(a);
            editor = focus.filter(|f| {
                self.terminal_why(a, Some(f)).is_none()
                    && ["AXTextArea", "AXTextField"].contains(&self.ax.role_of(f).as_str())
            });
        }
        match editor {
            Some(e) => (self.ax.selected_text(&e), self.ax.document_of(&e)),
            None => (None, None),
        }
    }

    pub(super) fn run_input(
        &self,
        a: &A,
        menu: Option<&str>,
        control: Option<&str>,
        why: &str,
        keys: Option<&str>,
    ) -> TerminalInput {
        let (selection, file) = self.run_facts(a);
        let where_ = match menu {
            Some(m)
                if self
                    .policy
                    .terminal_menu(m.split(" > ").next().unwrap_or("")) =>
            {
                format!("{}'s terminal", a.name)
            }
            _ => a.name.clone(),
        };
        TerminalInput {
            app: a.name.clone(),
            bundle_id: a.bundle_id.clone(),
            surface: if why.is_empty() {
                format!("a menu or control that runs code in {where_}")
            } else {
                why.into()
            },
            keys: keys.map(String::from),
            menu: menu.map(String::from),
            control: control.map(String::from),
            runs: true,
            selection,
            file,
            runs_where: where_,
            ..Default::default()
        }
    }

    pub(super) fn run_refusal(
        &self,
        a: &A,
        grant: Option<&TerminalGrant>,
        menu: Option<&str>,
        control: Option<&str>,
    ) -> Option<String> {
        if let Some(g) = grant
            .map(|g| &g.input)
            .filter(|g| g.bundle_id == a.bundle_id)
        {
            if g.runs
                && ((menu.is_some() && g.menu.as_deref() == menu)
                    || (control.is_some() && g.control.as_deref() == control))
            {
                if self.run_facts(a) == (g.selection.clone(), g.file.clone()) {
                    return None;
                }
                return Some("what this would run changed since the owner's Yes (the editor's selection or file), so nothing was pressed; call it again and FERB asks with what it would run now (D31)".into());
            }
        }
        Some(self.policy.unasked(
            &format!(
                "a menu or control that runs code ({})",
                menu.or(control).unwrap_or("")
            ),
            &a.name,
        ))
    }

    /// driver.py:1354: why pressing this element runs code.
    pub(super) fn runs(&self, a: &A, el: Option<&El>) -> Option<String> {
        let el = el?;
        let chain = self.ax.surface_facts(el)?;
        if let Some(why) = self
            .policy
            .run_control(&chain, self.policy.editor_app(&a.bundle_id))
        {
            return Some(why);
        }
        let labelled = self.policy.run_control(&chain, true)?;
        if self.hosts_terminal(el) {
            Some(labelled)
        } else {
            None
        }
    }

    /// driver.py:1366: the element's window holds a terminal by structure; past the budget FERB can't tell (asks).
    pub(super) fn hosts_terminal(&self, el: &El) -> bool {
        let Some(facts) = self.ax.window_facts(el) else {
            return false;
        };
        let deadline = self.now() + WINDOW_WALK_S;
        for (read, f) in facts.iter().enumerate() {
            let Some(f) = f else { return true };
            if read + 1 > WINDOW_WALK_NODES || self.now() > deadline {
                return true;
            }
            if self.policy.structural(f) {
                return true;
            }
        }
        false
    }

    fn shortcut(parts: &[String]) -> bool {
        parts.len() > 1
            || parts.last().is_some_and(|k| {
                let d = k.strip_prefix('f').unwrap_or("");
                k.starts_with('f')
                    && (1..=2).contains(&d.len())
                    && d.chars().all(|c| c.is_ascii_digit())
            })
    }

    fn shortcut_menu(&self, a: &A, parts: &[String]) -> Option<String> {
        if parts.is_empty() || !Self::shortcut(parts) {
            return None;
        }
        self.ax
            .menu_item_for_shortcut(a.pid, &parts[parts.len() - 1], &parts[..parts.len() - 1])
            .map(|f| f.1)
    }

    /// driver.py:1394.
    pub(super) fn key_run(&self, a: &A, combo: &str) -> (Option<String>, Option<String>) {
        let parts = keys::normalize(combo);
        if let Some(path) = self.shortcut_menu(a, &parts) {
            let items: Vec<String> = path.split(" > ").map(String::from).collect();
            if self.policy.run_menu(&items) {
                return (Some(path), None);
            }
        }
        if parts == ["space"] || parts == ["return"] {
            if let Some(focus) = self.focus_of(a) {
                if self.runs(a, Some(&focus)).is_some() {
                    let label = self.ax.place_of(&focus).1;
                    return (
                        None,
                        Some(if label.is_empty() {
                            "a run control".into()
                        } else {
                            label
                        }),
                    );
                }
            }
        }
        (None, None)
    }

    pub(super) fn terminal_app(&self, a: &A) -> bool {
        self.app_terminal_why(a).is_some()
    }

    pub(super) fn menu_pastes(&self, a: &A, path: &[String]) -> bool {
        pastes_item(self.ax.menu_item_shortcut(a.pid, path))
    }

    pub(super) fn item_pastes(&self, el: &El) -> bool {
        pastes_item(self.ax.item_shortcut(el))
    }

    fn paste_input(&self, a: &A, why: &str) -> TerminalInput {
        let (count, text, concealed) = (self.clipboard)();
        let enter = text.as_ref().is_some_and(|t| !self.policy.newline(t));
        TerminalInput {
            app: a.name.clone(),
            bundle_id: a.bundle_id.clone(),
            surface: why.into(),
            text,
            paste: true,
            concealed,
            clipboard: Some(count),
            enter,
            ..Default::default()
        }
    }

    fn placed(&self, mut inp: TerminalInput, el: Option<&El>) -> TerminalInput {
        if let Some(el) = el {
            let (window, label) = self.ax.place_of(el);
            inp.window = window;
            inp.label = label;
        }
        inp
    }

    fn base(&self, a: &A, why: &str) -> TerminalInput {
        TerminalInput {
            app: a.name.clone(),
            bundle_id: a.bundle_id.clone(),
            surface: why.into(),
            ..Default::default()
        }
    }

    /// driver.py:1433: the input this action would put into a terminal, by code's facts now.
    pub fn terminal_input(
        &self,
        app: &str,
        action: &ComputerAction,
        task: Option<&str>,
    ) -> Result<Option<TerminalInput>> {
        let a = self.app(app)?;
        if focused_typing(action) {
            let focus = self.focus_of(&a);
            let text = action.text.clone().unwrap_or_default();
            return Ok(self.terminal_why(&a, focus.as_ref()).map(|why| {
                let enter = !self.policy.newline(&text);
                self.placed(
                    TerminalInput {
                        text: Some(text),
                        enter,
                        ..self.base(&a, &why)
                    },
                    focus.as_ref(),
                )
            }));
        }
        let element = action.element.clone().filter(|e| !e.is_empty());
        if matches!(action.kind, K::Type | K::SetValue | K::Edit)
            && action.text.is_some()
            && element.is_some()
        {
            let r = self
                .node(&a.bundle_id, element.as_deref().unwrap_or(""))?
                .r#ref;
            let text = action.text.clone().unwrap_or_default();
            return Ok(self.terminal_why(&a, Some(&r)).map(|why| {
                let enter = !self.policy.newline(&text);
                self.placed(
                    TerminalInput {
                        text: Some(text),
                        enter,
                        ..self.base(&a, &why)
                    },
                    Some(&r),
                )
            }));
        }
        let mut is_paste = false;
        let mut item: Option<String> = None;
        if matches!(action.kind, K::Press | K::Click) && element.is_some() {
            let n = self.node(&a.bundle_id, element.as_deref().unwrap_or(""))?;
            if let Some(opened) = self.files_input(&a, &[self.element_file(Some(&n.r#ref))]) {
                return Ok(Some(opened));
            }
            if let Some(why_run) = self.runs(&a, Some(&n.r#ref)) {
                return Ok(Some(self.run_input(
                    &a,
                    None,
                    Some(&n.label),
                    &why_run,
                    None,
                )));
            }
            is_paste = n.ax_role == "AXMenuItem" && self.item_pastes(&n.r#ref);
            item = if n.ax_role == "AXMenuItem" && !is_paste && self.terminal_app(&a) {
                Some(n.label.clone())
            } else {
                None
            };
            if !is_paste && item.is_none() {
                return Ok(None);
            }
        } else if action.kind != K::Key
            || action.text.as_deref().unwrap_or("").is_empty()
            || !keys::valid_combo(action.text.as_deref().unwrap_or(""))
        {
            return Ok(None);
        } else if let Some(opened) = self.files_input(&a, &self.selected_files(&a)) {
            return Ok(Some(opened));
        }
        let combo = action.text.clone().unwrap_or_default();
        if action.kind == K::Key {
            let (path, control) = self.key_run(&a, &combo);
            if path.is_some() || control.is_some() {
                let k = keys::normalize(&combo).join("+");
                return Ok(Some(self.run_input(
                    &a,
                    path.as_deref(),
                    control.as_deref(),
                    "",
                    Some(&k),
                )));
            }
        }
        let focus = self.focus_of(&a);
        let Some(why) = self.terminal_why(&a, focus.as_ref()) else {
            return Ok(None);
        };
        if let Some(item) = item {
            return Ok(Some(self.placed(
                TerminalInput {
                    menu: Some(item),
                    ..self.base(&a, &why)
                },
                focus.as_ref(),
            )));
        }
        let parts = keys::normalize(&combo);
        if is_paste || pastes(&parts) {
            return Ok(Some(
                self.placed(self.paste_input(&a, &why), focus.as_ref()),
            ));
        }
        let armed = self.armed.lock().expect("lock").get(&a.pid).cloned();
        if parts == ["return"] {
            if let Some(armed) = armed {
                if armed.task.as_deref() == task && self.same_el(armed.el.as_ref(), focus.as_ref())
                {
                    return Ok(Some(self.placed(
                        TerminalInput {
                            keys: Some("return".into()),
                            covered_by: Some(armed.confirmation_id),
                            ..self.base(&a, &why)
                        },
                        focus.as_ref(),
                    )));
                }
            }
        }
        Ok(Some(self.placed(
            TerminalInput {
                keys: Some(parts.join("+")),
                ..self.base(&a, &why)
            },
            focus.as_ref(),
        )))
    }

    fn script_why(&self, a: &A) -> Option<String> {
        self.policy
            .code_runner(&ferb_sanitize::parity::casefold(&a.bundle_id))
            .or_else(|| self.app_terminal_why(a))
    }

    fn script_paths(&self, script: &str) -> Vec<String> {
        let home = std::env::var("HOME").unwrap_or_default();
        crate::applescript::named_paths(script, &home, std::path::Path::new("/Volumes"))
            .into_iter()
            .filter(|p| std::path::Path::new(p).is_file())
            .collect()
    }

    /// driver.py:1493.
    pub fn script_terminal_input(&self, app: &str, script: &str) -> Result<Option<TerminalInput>> {
        let a = self.app(app)?;
        let Some(why) = self.script_why(&a) else {
            return Ok(None);
        };
        let paths = self.script_paths(script);
        let opt: Vec<Option<String>> = paths.iter().cloned().map(Some).collect();
        let files = self.open_files(&opt, true);
        let opened: Vec<&String> = files.iter().map(|f| &f.0).collect();
        let mut named: Vec<String> = vec![];
        for p in &paths {
            if !named.contains(p) && !opened.contains(&p) {
                named.push(p.clone());
            }
        }
        Ok(Some(TerminalInput {
            script: Some(script.into()),
            files,
            named,
            opens: self
                .policy
                .opens_files(&ferb_sanitize::parity::casefold(&a.bundle_id)),
            ..self.base(&a, &why)
        }))
    }

    /// driver.py:1508.
    pub fn script_refusal(
        &self,
        a: &A,
        script: &str,
        grant: Option<&TerminalGrant>,
    ) -> Option<String> {
        let why = self.script_why(a)?;
        if let Some(g) = grant {
            if g.input.bundle_id == a.bundle_id && g.input.script.as_deref() == Some(script) {
                let opt: Vec<Option<String>> =
                    self.script_paths(script).into_iter().map(Some).collect();
                if self.files_covered(&opt, Some(g), true) {
                    return None;
                }
            }
        }
        Some(self.policy.unasked(&why, &a.name))
    }

    fn opens_in_terminal(&self, path: &str) -> bool {
        let Some(bundle) = self.apps.default_app_for(path).filter(|b| !b.is_empty()) else {
            return false;
        };
        self.policy
            .surface(
                &bundle,
                &[],
                &(self.terminal_apps)(),
                self.apps.declares_shell(&bundle),
                false,
            )
            .is_some()
    }

    fn runs_in(&self, bundle_id: Option<&str>, path: &str) -> bool {
        self.apps.runs_when_opened(path)
            && !bundle_id.is_some_and(|b| !b.is_empty() && self.apps.edits(b, path))
    }

    fn open_files(&self, paths: &[Option<String>], scripted: bool) -> Files {
        let mut seen: Vec<&String> = vec![];
        for p in paths.iter().flatten() {
            if !p.is_empty() && !seen.contains(&p) {
                seen.push(p);
            }
        }
        seen.into_iter()
            .filter(|p| {
                (if scripted {
                    self.apps.runs_when_opened(p)
                } else {
                    self.runs_in(self.apps.default_app_for(p).as_deref(), p)
                }) || self.opens_in_terminal(p)
            })
            .map(|p| file_facts(p))
            .collect()
    }

    fn files_covered(
        &self,
        paths: &[Option<String>],
        grant: Option<&TerminalGrant>,
        scripted: bool,
    ) -> bool {
        let need = self.open_files(paths, scripted);
        let approved: Vec<(String, String)> = grant
            .map(|g| {
                g.input
                    .files
                    .iter()
                    .map(|f| (f.0.clone(), f.1.clone()))
                    .collect()
            })
            .unwrap_or_default();
        need.iter()
            .all(|(p, h, _)| approved.contains(&(p.clone(), h.clone())))
    }

    pub(super) fn element_file(&self, el: Option<&El>) -> Option<String> {
        el.and_then(|e| self.ax.file_of(e))
    }

    pub(super) fn selected_files(&self, a: &A) -> Vec<Option<String>> {
        self.ax
            .selected_items(a.pid)
            .iter()
            .map(|e| self.element_file(Some(e)))
            .collect()
    }

    fn files_input(&self, a: &A, paths: &[Option<String>]) -> Option<TerminalInput> {
        let files = self.open_files(paths, false);
        if files.is_empty() {
            None
        } else {
            Some(TerminalInput {
                files,
                ..self.base(a, RUNS_OPENED)
            })
        }
    }

    pub(super) fn files_refusal(
        &self,
        a: &A,
        paths: &[Option<String>],
        grant: Option<&TerminalGrant>,
    ) -> Option<String> {
        if self.open_files(paths, false).is_empty() {
            return None;
        }
        if grant.is_some_and(|g| g.input.bundle_id == a.bundle_id)
            && self.files_covered(paths, grant, false)
        {
            return None;
        }
        Some(self.policy.unasked(RUNS_OPENED, &a.name))
    }

    fn opens_to_run(&self, bundle_id: &str, path: &str) -> (A, Files) {
        let a = self
            .apps
            .find(bundle_id)
            .unwrap_or_else(|| A::new(bundle_id, bundle_id, 0, false));
        let by_default = self
            .policy
            .opens_files(&ferb_sanitize::parity::casefold(bundle_id));
        let receiver = if by_default {
            self.apps.default_app_for(path)
        } else {
            Some(bundle_id.to_string())
        };
        let runs = self.runs_in(receiver.as_deref(), path)
            || self.app_terminal_why(&a).is_some()
            || (by_default && self.opens_in_terminal(path));
        (a, if runs { vec![file_facts(path)] } else { vec![] })
    }

    /// driver.py:1618.
    pub fn open_terminal_input(&self, bundle_id: &str, path: &str) -> Option<TerminalInput> {
        let (a, files) = self.opens_to_run(bundle_id, path);
        if files.is_empty() {
            None
        } else {
            Some(TerminalInput {
                files,
                ..self.base(&a, RUNS_OPENED)
            })
        }
    }

    /// driver.py:1624.
    pub fn open_refusal(
        &self,
        bundle_id: &str,
        path: &str,
        grant: Option<&TerminalGrant>,
    ) -> Option<String> {
        let (a, files) = self.opens_to_run(bundle_id, path);
        if files.is_empty() {
            return None;
        }
        let approved: Vec<(String, String)> = match grant {
            Some(g) if g.input.bundle_id == a.bundle_id => g
                .input
                .files
                .iter()
                .map(|f| (f.0.clone(), f.1.clone()))
                .collect(),
            _ => vec![],
        };
        if files
            .iter()
            .all(|(p, h, _)| approved.contains(&(p.clone(), h.clone())))
        {
            return None;
        }
        Some(self.policy.unasked(RUNS_OPENED, &a.name))
    }

    /// The element under (x, y) of the app's latest screenshot.
    pub(super) fn hit_in_shot(&self, a: &A, shot_id: &str, x: f64, y: f64) -> Option<El> {
        let shot = self.latest_shot(&a.bundle_id)?;
        if shot.id != shot_id {
            return None;
        }
        let (px, py) = super::shot_to_screen(&shot, x, y);
        self.ax.hit_element(a.pid, px, py)
    }

    /// driver.py:1649: the browser's own page this input would act on.
    pub fn browser_page(&self, app: &str, action: &ComputerAction) -> Result<Option<String>> {
        let a = self.app(app)?;
        if !self.browser_now(&a.bundle_id) {
            return Ok(None);
        }
        if matches!(action.kind, K::Type | K::SetValue | K::Edit)
            && self.policy.browser_page(action.text.as_deref())
        {
            return Ok(Some(
                ferb_py::value::strip(action.text.as_deref().unwrap_or("")).to_string(),
            ));
        }
        let el = if let Some(e) = action.element.as_deref().filter(|e| !e.is_empty()) {
            Some(self.node(&a.bundle_id, e)?.r#ref)
        } else if matches!(action.kind, K::Type | K::Key | K::Edit) {
            self.focus_of(&a)
        } else {
            return Ok(None);
        };
        Ok(self.browser_page_at(el.as_ref()))
    }

    pub fn pointer_browser_page(
        &self,
        app: &str,
        shot_id: &str,
        x: f64,
        y: f64,
    ) -> Result<Option<String>> {
        let a = self.app(app)?;
        if !self.browser_now(&a.bundle_id) {
            return Ok(None);
        }
        Ok(self.browser_page_at(self.hit_in_shot(&a, shot_id, x, y).as_ref()))
    }

    fn browser_page_at(&self, el: Option<&El>) -> Option<String> {
        let url = el.and_then(|e| self.ax.page_url(e)).flatten();
        if self.policy.browser_page(url.as_deref()) {
            url
        } else {
            None
        }
    }

    /// driver.py:1679.
    pub fn pointer_terminal_input(
        &self,
        app: &str,
        shot_id: &str,
        x: f64,
        y: f64,
        kind: &str,
        end: Option<(f64, f64)>,
    ) -> Result<Option<TerminalInput>> {
        let a = self.app(app)?;
        if kind == "dblclick" {
            if let Some(opened) = self.files_input(
                &a,
                &[self.element_file(self.hit_in_shot(&a, shot_id, x, y).as_ref())],
            ) {
                return Ok(Some(opened));
            }
        }
        if kind == "click" || kind == "dblclick" {
            let item = self.hit_in_shot(&a, shot_id, x, y);
            if let Some(why_run) = self.runs(&a, item.as_ref()) {
                let title = self.ax.place_of(item.as_ref().expect("runs needs one")).1;
                let t = if title.is_empty() {
                    "a run button".to_string()
                } else {
                    title
                };
                return Ok(Some(self.run_input(&a, None, Some(&t), &why_run, None)));
            }
            let Some(item) = item.filter(|i| self.ax.role_of(i) == "AXMenuItem") else {
                return Ok(None);
            };
            let is_paste = self.item_pastes(&item);
            if !is_paste && !self.terminal_app(&a) {
                return Ok(None);
            }
            let focus = self.focus_of(&a);
            let Some(why) = self.terminal_why(&a, focus.as_ref()) else {
                return Ok(None);
            };
            if is_paste {
                return Ok(Some(
                    self.placed(self.paste_input(&a, &why), focus.as_ref()),
                ));
            }
            let title = self.ax.place_of(&item).1;
            let menu = if title.is_empty() {
                "a menu item".to_string()
            } else {
                title
            };
            return Ok(Some(self.placed(
                TerminalInput {
                    menu: Some(menu),
                    ..self.base(&a, &why)
                },
                focus.as_ref(),
            )));
        }
        if kind == "drag" {
            if let Some(end) = end {
                let target = self.hit_in_shot(&a, shot_id, end.0, end.1);
                let Some(why) = self.terminal_why(&a, target.as_ref()) else {
                    return Ok(None);
                };
                let source = self.hit_in_shot(&a, shot_id, x, y);
                let text = source.and_then(|s| self.ax.selected_text(&s));
                return Ok(Some(self.placed(
                    TerminalInput {
                        text,
                        drag: true,
                        ..self.base(&a, &why)
                    },
                    target.as_ref(),
                )));
            }
        }
        Ok(None)
    }

    /// driver.py:1716.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn pointer_refusal(
        &self,
        a: &A,
        shot_id: &str,
        x: f64,
        y: f64,
        kind: &str,
        end: Option<(f64, f64)>,
        grant: Option<&TerminalGrant>,
        task: Option<&str>,
    ) -> Option<String> {
        if kind == "dblclick" {
            if let Some(r) = self.files_refusal(
                a,
                &[self.element_file(self.hit_in_shot(a, shot_id, x, y).as_ref())],
                grant,
            ) {
                return Some(r);
            }
        }
        if kind == "click" || kind == "dblclick" {
            let item = self.hit_in_shot(a, shot_id, x, y);
            if let Some(it) = &item {
                if self.runs(a, Some(it)).is_some() {
                    let title = self.ax.place_of(it).1;
                    let t = if title.is_empty() {
                        "a run button".to_string()
                    } else {
                        title
                    };
                    return self.run_refusal(a, grant, None, Some(&t));
                }
            }
            let item = item.filter(|i| self.ax.role_of(i) == "AXMenuItem")?;
            let focus = self.focus_of(a);
            if self.item_pastes(&item) {
                return self.terminal_refusal(
                    a,
                    focus.as_ref(),
                    grant,
                    task,
                    None,
                    Some("cmd+v"),
                    None,
                );
            }
            if self.terminal_app(a) {
                let title = self.ax.place_of(&item).1;
                let t = if title.is_empty() {
                    "a menu item".to_string()
                } else {
                    title
                };
                return self.terminal_refusal(a, focus.as_ref(), grant, task, None, None, Some(&t));
            }
            return None;
        }
        if kind == "drag" {
            if let Some(end) = end {
                let target = self.hit_in_shot(a, shot_id, end.0, end.1);
                let why = self.terminal_why(a, target.as_ref())?;
                if grant.is_some_and(|g| g.input.bundle_id == a.bundle_id && g.input.drag) {
                    return None;
                }
                return Some(self.policy.unasked(&why, &a.name));
            }
        }
        None
    }

    /// driver.py:1745.
    pub fn secret_terminal_input(
        &self,
        app: &str,
        element: &str,
        name: &str,
    ) -> Result<Option<TerminalInput>> {
        let a = self.app(app)?;
        let r = self.node(&a.bundle_id, element)?.r#ref;
        Ok(self.terminal_why(&a, Some(&r)).map(|why| {
            self.placed(
                TerminalInput {
                    secret: Some(name.into()),
                    ..self.base(&a, &why)
                },
                Some(&r),
            )
        }))
    }

    pub fn secret_refusal(&self, a: &A, el: &El, grant: Option<&TerminalGrant>) -> Option<String> {
        let why = self.terminal_why(a, Some(el))?;
        if grant.is_some_and(|g| {
            g.input.bundle_id == a.bundle_id
                && g.input.secret.as_deref().is_some_and(|s| !s.is_empty())
        }) {
            return None;
        }
        Some(self.policy.unasked(&why, &a.name))
    }

    /// driver.py:1761.
    pub fn menu_terminal_input(&self, app: &str, path: &[String]) -> Result<Option<TerminalInput>> {
        let a = self.app(app)?;
        if self.policy.run_menu(path) {
            let why = format!(
                "a menu that runs things in {}'s terminal ({})",
                a.name,
                path.first().cloned().unwrap_or_default()
            );
            return Ok(Some(self.run_input(
                &a,
                Some(&path.join(" > ")),
                None,
                &why,
                None,
            )));
        }
        if let Some(opened) = self.files_input(&a, &self.selected_files(&a)) {
            return Ok(Some(opened));
        }
        let is_paste = self.menu_pastes(&a, path);
        if !is_paste && !self.terminal_app(&a) {
            return Ok(None);
        }
        let focus = self.focus_of(&a);
        let Some(why) = self.terminal_why(&a, focus.as_ref()) else {
            return Ok(None);
        };
        if is_paste {
            return Ok(Some(
                self.placed(self.paste_input(&a, &why), focus.as_ref()),
            ));
        }
        Ok(Some(self.placed(
            TerminalInput {
                menu: Some(path.join(" > ")),
                ..self.base(&a, &why)
            },
            focus.as_ref(),
        )))
    }

    /// driver.py:1781: right before input is delivered: None when it may go to `el`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn terminal_refusal(
        &self,
        a: &A,
        el: Option<&El>,
        grant: Option<&TerminalGrant>,
        task: Option<&str>,
        text: Option<&str>,
        keys_: Option<&str>,
        menu: Option<&str>,
    ) -> Option<String> {
        let why = self.terminal_why(a, el)?;
        if let Some(grant) = grant.filter(|g| g.input.bundle_id == a.bundle_id) {
            let g = &grant.input;
            let parts = keys_.map(keys::normalize).unwrap_or_default();
            if let Some(menu) = menu {
                if g.menu.as_deref() == Some(menu) {
                    return None;
                }
            } else if !parts.is_empty() && pastes(&parts) {
                if g.paste && Some((self.clipboard)().0) == g.clipboard {
                    return None;
                }
            } else if parts == ["return"] && g.covered_by.as_deref().is_some_and(|c| !c.is_empty())
            {
                let armed = self.armed.lock().expect("lock").get(&a.pid).cloned();
                if let Some(armed) = armed {
                    if Some(&armed.confirmation_id) == g.covered_by.as_ref()
                        && armed.confirmation_id == grant.confirmation_id
                        && armed.task.as_deref() == task
                        && el.is_some()
                        && self.same_el(armed.el.as_ref(), el)
                    {
                        return None;
                    }
                }
            } else if !parts.is_empty() {
                if !g.paste
                    && g.covered_by.as_deref().unwrap_or("").is_empty()
                    && g.keys
                        .as_deref()
                        .is_some_and(|k| keys::normalize(k) == parts)
                {
                    return None;
                }
            } else if let Some(text) = text {
                if !g.paste && g.text.as_deref() == Some(text) {
                    return None;
                }
            }
        }
        Some(self.policy.unasked(&why, &a.name))
    }

    pub(super) fn terminal_typed(
        &self,
        a: &A,
        el: Option<&El>,
        grant: Option<&TerminalGrant>,
        task: Option<&str>,
        delivered: bool,
    ) {
        let mut armed = self.armed.lock().expect("lock");
        armed.remove(&a.pid);
        if let (true, Some(g), Some(el)) = (delivered, grant, el) {
            if g.input.enter {
                armed.insert(
                    a.pid,
                    super::Armed {
                        el: Some(el.clone()),
                        confirmation_id: g.confirmation_id.clone(),
                        task: task.map(String::from),
                    },
                );
            }
        }
    }

    pub(super) fn secure(&self, el: Option<&El>) -> bool {
        el.is_some_and(|e| self.ax.secure_element(e))
    }

    /// driver.py:1836: an element's structural facts (role, subrole, secure field, its page's address in a browser).
    pub(super) fn element_facts(&self, a: &A, el: Option<&El>) -> ActionFacts {
        let mut out = ActionFacts::default();
        let Some(el) = el else { return out };
        let role = self.ax.role_of(el);
        let subrole = self.ax.subrole_of(el);
        out.target_role = if role.is_empty() { None } else { Some(role) };
        out.target_subrole = if subrole.is_empty() {
            None
        } else {
            Some(subrole)
        };
        out.secure_field = Some(self.secure(Some(el)));
        if self.is_browser(&a.bundle_id) {
            if let Some(url) = self.ax.page_url(el) {
                out.page_url = url.filter(|u| !u.is_empty());
            }
        }
        out
    }

    fn received(&self, a: &A, el: Option<&El>) -> ReceivedBy {
        let (window, _) = el.map(|e| self.ax.place_of(e)).unwrap_or_default();
        let role = self.role(el);
        ReceivedBy {
            bundle_id: Some(a.bundle_id.clone()),
            window: if window.is_empty() {
                None
            } else {
                Some(window)
            },
            role: if role.is_empty() { None } else { Some(role) },
        }
    }

    /// driver.py:1863: the structural facts of one input (GATE-202), read after it.
    pub(super) fn facts_now(
        &self,
        a: &A,
        target: Option<&El>,
        received: Option<&El>,
        terminal: Option<bool>,
        after: bool,
    ) -> ActionFacts {
        let mut found = self.element_facts(a, target);
        let got = if after {
            self.focus_of(a)
        } else {
            received.cloned()
        };
        found.received_by = Some(self.received(a, got.as_ref().or(target)));
        found.terminal_target = terminal;
        found
    }

    pub(super) fn menu_combo(&self, a: &A, path: &[String]) -> Option<String> {
        self.ax
            .menu_item_shortcut(a.pid, path)
            .map(|(c, m)| keys::shortcut_combo(&c, m))
    }

    pub(super) fn item_combo(&self, el: Option<&El>) -> Option<String> {
        let el = el?;
        if self.ax.role_of(el) != "AXMenuItem" {
            return None;
        }
        self.ax
            .item_shortcut(el)
            .map(|(c, m)| keys::shortcut_combo(&c, m))
    }

    /// driver.py:1896 (HAND-200): screen-wide refused; ⌘Q / ⌘H through FERB's safe quit / hide.
    pub(super) async fn guarded_combo(
        &self,
        a: &A,
        combo: &str,
        task: Option<&str>,
    ) -> Result<Option<ActionResult>> {
        if keys::screen_wide(combo) {
            return refused(keys::SCREEN_WIDE);
        }
        let Some(what) = keys::quit_or_hide(combo) else {
            return Ok(None);
        };
        if what == "hide_others" {
            return refused("⌥⌘H hides every other app, the owner's included, so FERB doesn't press it (HAND-200)");
        }
        let (Some(launched), Some(task)) = (&self.launched, task) else {
            return refused(format!(
                "FERB quits or hides only apps it opened for the task, and it can't tell that here, so it didn't send {combo} to {}",
                a.name
            ));
        };
        let detail = if what == "quit" {
            launched.quit_now(task, a).await?
        } else {
            launched.hide_now(task, a).await?
        };
        Ok(Some(ActionResult {
            ok: true,
            detail,
            facts: json!({"method": format!("ferb_{what}"), "delivered": false, "outcome_verified": false}).as_object().cloned().unwrap_or_default(),
            observed: Some(ActionFacts { received_by: Some(ReceivedBy { bundle_id: Some(a.bundle_id.clone()), window: None, role: None }), ..Default::default() }),
        }))
    }
}

#[allow(dead_code)]
fn _e(_: HandError) {}
