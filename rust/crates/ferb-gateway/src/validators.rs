//! The argument models' own validators, which a JSON schema doesn't carry: `field_validator`s (run on a field once
//! it is valid; their errors join the other fields') and `model_validator(mode="after")` (run once every field of
//! its model is valid). Each refuses with pydantic's `value_error` and may change a value, as its Python does:
//!
//! - `audio.listen` (`ferb/act/listen.py:217`): exactly one of app or tab.
//! - `mac.open` url (`ferb/act/mac/tools.py:277`): an http(s) address, stripped.
//! - `mac.menu` path (`tools.py:379`): one spelling of a menu path.
//! - `*.pdf` (`ferb/act/browser/tools.py:676`), `*.media` (`:803`), `*.act` (`:908`), both browser families.
//! - `trail.query` since / until (`ferb/sense/tools.py:70`): ISO dates.
//! - `wait.for` (`ferb/clock/wait.py:111,135`): each condition's fields, one script per wait.

use serde_json::{Map, Value};

use crate::pydantic::Hooks;

/// The validators of one tool's model (by the tool's name).
pub struct ToolHooks<'a>(pub &'a str);

fn text<'a>(m: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    m.get(k).and_then(Value::as_str)
}

fn null(m: &Map<String, Value>, k: &str) -> bool {
    m.get(k).is_none_or(Value::is_null)
}

/// Python truth of a validated value.
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn verb(tool: &str) -> Option<&str> {
    tool.strip_prefix("browser.")
        .or_else(|| tool.strip_prefix("ferb_browser."))
}

/// Python's `str.strip()`: Unicode whitespace and the four separator controls.
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

impl Hooks for ToolHooks<'_> {
    fn field(&self, class: &str, field: &str, value: &Value) -> Option<Result<Value, String>> {
        if class == "Condition" || class == "Contact" {
            return None;
        }
        match (self.0, field) {
            ("mac.open", "url") => Some(match value {
                Value::String(v) => {
                    let s = py_strip(v);
                    let low = s.get(..8).unwrap_or(s).to_ascii_lowercase();
                    if low.starts_with("http://") || low.starts_with("https://") {
                        Ok(Value::String(s.to_string()))
                    } else {
                        Err("url takes an http:// or https:// address".into())
                    }
                }
                _ => Ok(Value::Null),
            }),
            ("mac.menu", "path") => {
                let raw = value.as_str().unwrap_or("");
                let items: Vec<&str> = raw
                    .split('>')
                    .map(py_strip)
                    .filter(|p| !p.is_empty())
                    .collect();
                Some(Ok(if items.is_empty() {
                    Value::Null
                } else {
                    Value::String(items.join(" > "))
                }))
            }
            ("trail.query", "since" | "until") => Some(match value.as_str() {
                Some(v) => time_bound(v).map(|()| value.clone()),
                None => Ok(Value::Null),
            }),
            _ => None,
        }
    }

    fn after(&self, class: &str, f: &mut Map<String, Value>, _input: &Value) -> Result<(), String> {
        if class == "Condition" {
            return condition(f);
        }
        if class == "Contact" {
            return Ok(());
        }
        match self.0 {
            "audio.listen" => {
                if null(f, "app") == null(f, "tab") {
                    return Err("give exactly one of app or tab".into());
                }
                Ok(())
            }
            "wait.for" => {
                let scripts = f
                    .get("any_of")
                    .and_then(Value::as_array)
                    .map(|c| {
                        c.iter()
                            .filter(|c| c.get("kind").and_then(Value::as_str) == Some("script"))
                            .count()
                    })
                    .unwrap_or(0);
                if scripts > 1 {
                    return Err(
                        "one script condition per wait: put both checks in one command".into(),
                    );
                }
                Ok(())
            }
            tool => match verb(tool) {
                Some("pdf") => {
                    if truthy(f.get("tab")) == truthy(f.get("html_path")) {
                        return Err("give exactly one of tab or html_path".into());
                    }
                    if !text(f, "path")
                        .unwrap_or("")
                        .to_lowercase()
                        .ends_with(".pdf")
                    {
                        return Err("path must end in .pdf".into());
                    }
                    Ok(())
                }
                Some("media") => {
                    if (text(f, "action") == Some("seek")) != !null(f, "to") {
                        return Err("seek needs `to` (seconds); only seek takes it".into());
                    }
                    if !null(f, "index") && !null(f, "media_id") {
                        return Err("name the element by index or by media_id, one of them".into());
                    }
                    Ok(())
                }
                Some("act") => {
                    let action = text(f, "action").unwrap_or("");
                    if ["type", "select", "press", "paste"].contains(&action) && null(f, "text") {
                        return Err(format!(
                            "{action} requires explicit text; no default key or replacement value"
                        ));
                    }
                    if !null(f, "mode") && action != "type" {
                        return Err("mode is for type only".into());
                    }
                    if !null(f, "html") && action != "paste" {
                        return Err("html is for paste only".into());
                    }
                    if truthy(f.get("modifiers"))
                        && (!["click", "dblclick", "rightclick"].contains(&action)
                            || truthy(f.get("page_mode")))
                    {
                        return Err(
                            "modifiers go with a real click, dblclick or rightclick only (not page_mode)".into(),
                        );
                    }
                    if action == "press" && !truthy(f.get("text")) {
                        return Err("press requires explicit text naming a key".into());
                    }
                    Ok(())
                }
                _ => Ok(()),
            },
        }
    }
}

/// `Condition._fields` (`wait.py:111`).
fn condition(c: &Map<String, Value>) -> Result<(), String> {
    let t = |k: &str| text(c, k).unwrap_or("");
    let n = |k: &str| c.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let kind = t("kind");
    let need = match kind {
        "process" => !t("id").is_empty(),
        "file" => !t("path").is_empty() && !t("state").is_empty(),
        "page" | "url" | "download" => !t("tab").is_empty(),
        "mail" => {
            !t("tab").is_empty()
                && !(t("sender").is_empty() && t("subject").is_empty() && t("text").is_empty())
        }
        "time" => n("in_minutes") != 0.0 || !t("at").is_empty(),
        "script" => !py_strip(t("command")).is_empty(),
        _ => true,
    };
    if !need {
        return Err(match kind {
            "process" => "a process condition needs id",
            "file" => "a file condition needs path and state",
            "page" => "a page condition needs tab",
            "url" => "a url condition needs tab",
            "mail" => "a mail condition needs tab and one of sender, subject, text",
            "download" => "a download condition needs tab",
            "time" => "a time condition needs in_minutes or at",
            _ => "a script condition needs command",
        }
        .into());
    }
    if c.get("regex") == Some(&Value::Bool(true)) && !t("match").is_empty() {
        // Python's `re.compile`: which patterns it refuses, and its words, are read from fancy-regex here (a
        // named difference: Python's own error text isn't reproduced)
        if let Err(e) = fancy_regex::Regex::new(t("match")) {
            return Err(format!("match isn't a valid regular expression: {e}"));
        }
    }
    Ok(())
}

/// `time_bound`'s parse (`ferb/sense/tools.py:29`): ten characters read as `date.fromisoformat`, else
/// `datetime.fromisoformat`; only whether it reads, with Python's error words.
fn time_bound(value: &str) -> Result<(), String> {
    let v = py_strip(value);
    if v.is_empty() {
        return Ok(());
    }
    let bad = || format!("Invalid isoformat string: {}", crate::words::str_repr(v));
    if v.chars().count() == 10 {
        return if date_ok(v) { Ok(()) } else { Err(bad()) };
    }
    if ferb_py::time::DateTime::from_isoformat(v).is_ok()
        || ferb_py::time::DateTime::from_isoformat(&format!("{v}+00:00")).is_ok()
    {
        Ok(())
    } else {
        Err(bad())
    }
}

fn date_ok(v: &str) -> bool {
    let b = v.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let num = |s: &str| {
        if s.bytes().all(|c| c.is_ascii_digit()) {
            s.parse::<u32>().ok()
        } else {
            None
        }
    };
    let (Some(y), Some(m), Some(d)) = (num(&v[..4]), num(&v[5..7]), num(&v[8..])) else {
        return false;
    };
    y >= 1 && chrono::NaiveDate::from_ymd_opt(y as i32, m, d).is_some()
}
