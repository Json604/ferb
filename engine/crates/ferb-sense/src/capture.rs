//! Capture guards and per-field text aggregation (`ferb/sense/capture.py`). Raw keys are never kept; only redacted,
//! complete field bursts leave here, and nothing infers authority from observed text (decisions: SENSE-401,
//! SENSE-403).

use std::sync::Mutex;

use serde_json::Value;

use crate::observation::{dict, Dict, Observation, Redact};

/// capture.py:9: pasteboard markers whose contents are never read.
pub const CONCEALED: [&str; 2] = [
    "org.nspasteboard.ConcealedType",
    "org.nspasteboard.TransientType",
];
/// capture.py:10.
pub const TEXT_ROLES: [&str; 3] = ["AXTextField", "AXTextArea", "AXComboBox"];
/// capture.py:12: AX attributes that can name a password field, also while "Show password" makes it a plain field.
pub const FIELD_METADATA_ATTRS: [&str; 5] = [
    "AXTitle",
    "AXDescription",
    "AXPlaceholderValue",
    "AXIdentifier",
    "AXDOMIdentifier",
];

pub fn text_role(role: &str) -> bool {
    TEXT_ROLES.contains(&role)
}

/// capture.py:16 `password_element`: a text element whose own AX metadata names it a password field (its value is a
/// secret). `attr(name)` reads one attribute as text (None: absent or not text).
pub fn password_element(role: &str, attr: &dyn Fn(&str) -> Option<String>) -> bool {
    if !text_role(role) {
        return false;
    }
    let meta: Vec<String> = FIELD_METADATA_ATTRS
        .iter()
        .filter_map(|a| attr(a))
        .collect();
    let refs: Vec<&str> = meta.iter().map(String::as_str).collect();
    ferb_sanitize::password_field(role, &refs)
}

static CLIPBOARD_WRITE: Mutex<Option<(i64, Option<String>)>> = Mutex::new(None);

/// capture.py:21: called only after FERB's native pasteboard write succeeds; later changes never inherit it.
pub fn mark_clipboard_write(change_count: i64, task_id: Option<&str>) {
    *CLIPBOARD_WRITE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some((change_count, task_id.map(str::to_string)));
}

/// capture.py:27.
pub fn clipboard_write() -> Option<(i64, Option<String>)> {
    CLIPBOARD_WRITE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// capture.py:32: the focused text field, as Accessibility describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub bundle_id: String,
    pub app: String,
    pub pid: i64,
    pub window_id: String,
    pub field_id: String,
    pub role: String,
    pub label: String,
    pub masked: bool,
}

impl Field {
    /// capture.py:43.
    pub fn safe(&self) -> bool {
        !self.field_id.is_empty() && text_role(&self.role) && !self.masked
    }

    /// capture.py:46: which element this is, whatever its state.
    pub fn place(&self) -> (&str, i64, &str, &str) {
        (&self.bundle_id, self.pid, &self.window_id, &self.field_id)
    }

    /// capture.py:50.
    pub fn data(&self) -> Dict {
        dict([
            ("bundle_id", Value::from(self.bundle_id.clone())),
            ("app", Value::from(self.app.clone())),
            ("pid", Value::from(self.pid)),
            ("window_id", Value::from(self.window_id.clone())),
            ("field_id", Value::from(self.field_id.clone())),
            ("role", Value::from(self.role.clone())),
            ("field", Value::from(self.label.clone())),
        ])
    }
}

struct Pending {
    field: Field,
    actor: String,
    text: Vec<char>,
    at: f64,
}

type Emit = Box<dyn Fn(Observation) + Send + Sync>;

/// capture.py:63: text held in volatile memory for the decided hold; only redacted complete field bursts leave.
pub struct TypedFields {
    emit: Emit,
    redact: std::sync::Arc<dyn Redact>,
    pub hold_s: f64,
    pending: Mutex<Option<Pending>>,
}

impl TypedFields {
    pub fn new(
        emit: impl Fn(Observation) + Send + Sync + 'static,
        redact: std::sync::Arc<dyn Redact>,
    ) -> TypedFields {
        TypedFields {
            emit: Box::new(emit),
            redact,
            hold_s: 0.5,
            pending: Mutex::new(None),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Pending>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn clear(&self) {
        *self.lock() = None;
    }

    /// capture.py:74: a key arrived where nothing is captured: a burst held for another field is complete and kept;
    /// one for this same field, now not safe, is dropped.
    pub fn leave(&self, current: Option<&Field>) {
        let p = self.lock().take();
        if let Some(p) = p {
            if current.is_none_or(|c| c.place() != p.field.place()) {
                self.emit_burst(p);
            }
        }
    }

    /// capture.py:82.
    pub fn holding(&self) -> bool {
        self.lock().is_some()
    }

    /// capture.py:88.
    pub fn feed(&self, field: Option<&Field>, text: &str, actor: &str, now: f64, secure: bool) {
        let mut slot = self.lock();
        let field = match field {
            Some(f) if !secure && f.safe() => f,
            _ => {
                *slot = None;
                return;
            }
        };
        let fresh = match slot.as_ref() {
            None => true,
            Some(p) => p.field != *field || p.actor != actor,
        };
        if fresh {
            // focus moved to another safe field (or another actor typed): the former burst is complete and kept
            if let Some(old) = slot.take() {
                drop(slot);
                self.emit_burst(old);
                slot = self.lock();
            }
            *slot = Some(Pending {
                field: field.clone(),
                actor: actor.to_string(),
                text: Vec::new(),
                at: now,
            });
        }
        let p = slot.as_mut().expect("a burst is held");
        for c in text.chars() {
            match c {
                '\u{8}' | '\u{7f}' => {
                    p.text.pop();
                }
                '\r' | '\u{1b}' | '\t' => {}
                c => p.text.push(c),
            }
        }
        p.at = now;
    }

    /// capture.py:108.
    pub fn flush(&self, current: Option<&Field>, secure: bool, now: f64) {
        let mut slot = self.lock();
        let Some(p) = slot.as_ref() else {
            return;
        };
        if secure {
            *slot = None; // Secure Input or a secure field now: nothing held is kept
            return;
        }
        if let Some(c) = current {
            if *c == p.field && c.safe() && now - p.at < self.hold_s {
                return;
            }
        }
        let p = slot.take().expect("a burst is held");
        drop(slot);
        // a field that turned secret meanwhile (the same element, no longer safe) keeps nothing
        if current.is_none_or(|c| c.place() != p.field.place() || c.safe()) {
            self.emit_burst(p);
        }
    }

    fn emit_burst(&self, p: Pending) {
        if p.text.is_empty() {
            return;
        }
        let raw: String = p.text.into_iter().collect();
        let text = self.redact.field(&raw, &p.field.role, &p.field.label);
        let mut data = p.field.data();
        data.insert("text".into(), Value::from(text));
        let data = self.redact.dict(&data);
        (self.emit)(Observation::by("typed_text", data, &p.actor));
    }
}

/// capture.py:132.
pub fn clipboard_observation(
    change_count: i64,
    types: &[String],
    read: &dyn Fn() -> Option<String>,
    redact: &dyn Redact,
    ferb_count: Option<i64>,
    task_id: Option<&str>,
    context: Option<&Dict>,
) -> Option<Observation> {
    if types.iter().any(|t| CONCEALED.contains(&t.as_str())) {
        return None;
    }
    let text = read()?;
    let actor = if Some(change_count) == ferb_count {
        "ferb"
    } else {
        "unknown"
    };
    let mut data = context.cloned().unwrap_or_default();
    data.insert("text".into(), Value::from(text));
    data.insert("change_count".into(), Value::from(change_count));
    let mut hints = Dict::new();
    hints.insert("actor".into(), Value::from(actor));
    hints.insert(
        "task_id".into(),
        match (actor, task_id) {
            ("ferb", Some(t)) => Value::from(t),
            _ => Value::Null,
        },
    );
    Some(Observation::with_hints(
        "clipboard",
        redact.dict(&data),
        hints,
    ))
}
