//! What a call returns (`Gateway._replier`, `gateway.py:1017-1027`): its content redacted (once), whether it is an
//! error, and the footer: the time line (`clock/context.py` `footer`) and, when the result has origins, the origin
//! line, redacted like content (labels name hosts and senders).

use std::collections::BTreeSet;

use ferb_core::when::DateTime;
use ferb_sanitize::Redactor;
use serde_json::{Map, Value};

use crate::words::origin_line;

/// The tool result a model receives.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub content: Value,
    pub is_error: bool,
    pub footer: String,
    /// The tool's image, to the model only (base64 JPEG).
    pub image: Option<String>,
}

impl Reply {
    /// The reply as the RPC carries it: `{"content", "is_error", "footer", "image"?}`.
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("content".into(), self.content.clone());
        m.insert("is_error".into(), Value::Bool(self.is_error));
        m.insert("footer".into(), Value::String(self.footer.clone()));
        if let Some(data) = &self.image {
            let mut i = Map::new();
            i.insert("mime".into(), Value::String("image/jpeg".into()));
            i.insert("data".into(), Value::String(data.clone()));
            m.insert("image".into(), Value::Object(i));
        }
        Value::Object(m)
    }

    /// What the model reads (`ferb/cli/mcp.py:97`, `ferb/runtimes/api.py:470`):
    /// `json.dumps(content, ensure_ascii=False) + "\n" + footer`.
    pub fn model_text(&self) -> String {
        format!(
            "{}\n{}",
            ferb_py::json::dumps_utf8(&self.content),
            self.footer
        )
    }
}

/// The zone name `strftime('%Z')` writes for the clock's time (a fixed offset's: "UTC", "UTC+05:30").
pub fn zone_name(now: &DateTime) -> String {
    ferb_clock::py::fixed_tzname(now.offset().local_minus_utc(), None)
}

/// `_replier(rec)`'s `reply`: `redacted` when the content was already redacted once.
#[allow(clippy::too_many_arguments)]
pub fn reply(
    redactor: &Redactor,
    now: &DateTime,
    now_name: &str,
    started: Option<&DateTime>,
    deadline: Option<&DateTime>,
    content: Value,
    is_error: bool,
    origins: Option<&BTreeSet<String>>,
    redacted: bool,
) -> Reply {
    let mut footer = ferb_clock::context::footer(now, now_name, started, deadline);
    if let Some(o) = origins.filter(|o| !o.is_empty()) {
        let line = redactor.value(&Value::String(origin_line(o)));
        footer.push('\n');
        footer.push_str(line.as_str().unwrap_or_default());
    }
    Reply {
        content: if redacted {
            content
        } else {
            redactor.value(&content)
        },
        is_error,
        footer,
        image: None,
    }
}
