//! Listen-only text capture (`ferb/sense/watchers/text_input.py`; decision: SENSE-401). Before any key's text is
//! extracted: Secure Input (`IsSecureEventInputEnabled`), terminal apps and terminals inside other apps, FERB's own
//! apps, excluded apps, and the focused field's AX role, subrole and password metadata are checked; a secure field,
//! a password field (a shown one too) or a terminal is never captured (rule 7, `sanitize/redact.py:244-248`).
//! Accessibility is read only when a key arrives or a held burst is flushed, never polled otherwise.

use std::sync::{Arc, Mutex};

use ferb_core::interfaces::BoxFuture;
use serde_json::Value;

use super::accessibility::AxElement;
use super::base::{poll, Focused, ReadFocused};
use super::owner_input::{KeyEvent, OwnerInput};
use crate::capture::{password_element, text_role, Field, TypedFields};
use crate::front_window::FERB_APPS;
use crate::manager::{Emit, Stop, Watcher};
use crate::observation::{Dict, Redact};

/// CGEventFlags: Command and Control (a shortcut, not text).
pub const FLAG_COMMAND: u64 = 1 << 20;
pub const FLAG_CONTROL: u64 = 1 << 18;

/// The terminal rules capture shares with the typing floor (`ferb/policy/terminal.py`, D31), the policy part's.
pub trait Terminals: Send + Sync {
    /// A terminal by the app alone: its bundle id (built in or the owner's list), its Shell role, or a terminal it
    /// can't show (text_input.py:27 `terminal_app`).
    fn app(&self, app: &Focused) -> bool;
    /// The focused field is inside a terminal by its own and its ancestors' facts, never its value
    /// (text_input.py:35 `in_terminal`; `facts` is `accessibility.surface_facts(field)`).
    fn field(&self, app: &Focused, facts: &[Dict]) -> bool;
}

/// The focused element of an app, with what `in_terminal` reads about it.
pub trait FocusedElement: AxElement {
    /// `accessibility.surface_facts(field)`: the element's and its ancestors' kinds and labels.
    fn surface_facts(&self) -> Vec<Dict>;
}

/// The native reads text capture makes.
pub trait FieldReads: Send + Sync {
    /// Secure Input is on (`IsSecureEventInputEnabled`); true when it can't be told (no primary defense: capture
    /// nothing).
    fn secure_input(&self) -> bool;
    /// The focused element of this app (`AXFocusedUIElement` of its application element).
    fn focused_element(&self, pid: i64) -> Option<Box<dyn FocusedElement>>;
}

fn text(v: Option<Value>) -> String {
    match v {
        Some(v) if ferb_py::value::truthy(&v) => ferb_py::value::py_str(&v),
        _ => String::new(),
    }
}

/// text_input.py:65 `focused_field`: the focused text field when it may be captured, else None. The value is read
/// only for a field already known to be neither secure nor a password field (to see a masked one).
pub fn focused_field(
    app: Option<Focused>,
    excluded: &dyn Fn(&str, &str) -> bool,
    terminals: &dyn Terminals,
    reads: &dyn FieldReads,
) -> Option<Field> {
    let app = app?;
    if terminals.app(&app) || excluded(&app.bundle_id, &app.name) {
        return None;
    }
    if FERB_APPS.contains(&ferb_sanitize::parity::casefold(&app.bundle_id).as_str()) {
        return None; // rule 17 (owner, 2026-10-25): FERB's own apps are never read per keystroke
    }
    let field = reads.focused_element(app.pid);
    let (role, subrole) = match &field {
        Some(f) => (text(f.attr("AXRole")), text(f.attr("AXSubrole"))),
        None => (String::new(), String::new()),
    };
    let password = field.as_ref().is_some_and(|f| {
        password_element(&role, &|name: &str| match f.attr(name) {
            Some(Value::String(s)) => Some(s),
            _ => None,
        })
    });
    if !text_role(&role) || role.contains("Secure") || subrole.contains("Secure") || password {
        return None; // a shown password is a plain AXTextField: its metadata still says so
    }
    let field = field?;
    if terminals.field(&app, &field.surface_facts()) {
        return None; // a terminal inside another app (D31's facts): never captured
    }
    let masked = match field.attr("AXValue") {
        Some(Value::String(v)) => !v.is_empty() && v.chars().all(|c| "•●*·".contains(c)),
        _ => false,
    };
    let field_id = match field.attr("AXIdentifier") {
        Some(v) if ferb_py::value::truthy(&v) => ferb_py::value::py_str(&v),
        _ => field.hash_id(),
    };
    let window_id = match field.window() {
        None => String::new(),
        Some(w) => match w.attr("AXWindowNumber") {
            Some(v) if ferb_py::value::truthy(&v) => ferb_py::value::py_str(&v),
            _ => w.hash_id(),
        },
    };
    let label = match field.attr("AXTitle") {
        Some(v) if ferb_py::value::truthy(&v) => ferb_py::value::py_str(&v),
        _ => text(field.attr("AXDescription")),
    };
    Some(Field {
        bundle_id: app.bundle_id,
        app: app.name,
        pid: app.pid,
        window_id,
        field_id,
        role,
        label,
        masked,
    })
}

/// text_input.py:114.
pub struct TextInputWatcher {
    owner_input: Arc<OwnerInput>,
    redact: Arc<dyn Redact>,
    excluded: Arc<dyn Fn(&str, &str) -> bool + Send + Sync>,
    terminals: Arc<dyn Terminals>,
    reads: Arc<dyn FieldReads>,
    focused: ReadFocused,
    /// monotonic seconds
    now: Arc<dyn Fn() -> f64 + Send + Sync>,
}

struct Capture {
    fields: TypedFields,
    failure: Mutex<Option<String>>,
    stop: Stop,
    watcher: Arc<TextInputWatcher>,
}

impl Capture {
    fn field(&self) -> Option<Field> {
        let w = &self.watcher;
        focused_field(
            (w.focused)(),
            w.excluded.as_ref(),
            w.terminals.as_ref(),
            w.reads.as_ref(),
        )
    }

    /// text_input.py:131 `key`: checks first, the key's text last.
    fn key(&self, event: &dyn KeyEvent, marked: bool) {
        let w = &self.watcher;
        if self.stop.is_set() || w.reads.secure_input() {
            self.fields.clear();
            return;
        }
        let field = self.field();
        match &field {
            Some(f) if f.safe() => {}
            _ => {
                self.fields.leave(field.as_ref()); // an earlier field's burst is kept; this key never is
                return;
            }
        }
        if event.flags() & (FLAG_COMMAND | FLAG_CONTROL) != 0 {
            return;
        }
        let text = event.text();
        if !text.is_empty() {
            let actor = if marked { "ferb" } else { "owner" };
            self.fields
                .feed(field.as_ref(), &text, actor, (w.now)(), false);
        }
    }

    /// text_input.py:153 `flush`.
    fn flush(&self) -> Result<(), String> {
        if let Some(kind) = self
            .failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return Err(format!("RuntimeError: text capture adapter failed: {kind}"));
        }
        if !self.fields.holding() {
            return Ok(()); // nothing typed since the last flush: no Accessibility reads
        }
        let w = &self.watcher;
        if w.reads.secure_input() {
            self.fields.clear(); // AXValue is never queried while Secure Input is on
            return Ok(());
        }
        let field = self.field();
        self.fields
            .flush(field.as_ref(), w.reads.secure_input(), (w.now)());
        Ok(())
    }
}

impl TextInputWatcher {
    pub fn new(
        owner_input: Arc<OwnerInput>,
        redact: Arc<dyn Redact>,
        excluded: Arc<dyn Fn(&str, &str) -> bool + Send + Sync>,
        terminals: Arc<dyn Terminals>,
        reads: Arc<dyn FieldReads>,
        focused: ReadFocused,
        now: Arc<dyn Fn() -> f64 + Send + Sync>,
    ) -> TextInputWatcher {
        TextInputWatcher {
            owner_input,
            redact,
            excluded,
            terminals,
            reads,
            focused,
            now,
        }
    }
}

impl Watcher for TextInputWatcher {
    fn source(&self) -> &str {
        "typed_text"
    }

    fn run(self: Arc<Self>, emit: Emit, stop: Stop) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            if !self.owner_input.is_running() {
                return Err("RuntimeError: listen-only input tap is unavailable".into());
            }
            let e = emit.clone();
            let capture = Arc::new(Capture {
                fields: TypedFields::new(move |o| e(o), self.redact.clone()),
                failure: Mutex::new(None),
                stop: stop.clone(),
                watcher: self.clone(),
            });
            let c = capture.clone();
            self.owner_input
                .set_on_text(Some(Arc::new(move |event: &dyn KeyEvent, marked| {
                    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        c.key(event, marked)
                    }));
                    if run.is_err() {
                        c.fields.clear(); // a failure cannot validate previously held text
                        *c.failure
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) =
                            Some("RuntimeError".into());
                    }
                })));
            let c = capture.clone();
            let result = poll(0.1, &stop, Arc::new(move || c.flush())).await;
            self.owner_input.set_on_text(None);
            capture.fields.clear();
            result
        })
    }
}
