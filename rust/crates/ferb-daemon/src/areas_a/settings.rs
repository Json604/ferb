//! The settings FERB.app's window may change in config.toml through `settings.set` (L34 Phase E). Port of
//! `ferb/daemon/settings.py`, and of the two handlers in `ferb/daemon/areas/rpc.py:36-51` that read and write them.
//!
//! Rule 10: these writes come only through `settings.set`, a method of the owner's CLI role alone (ferb-rpc's role
//! table). Each value is checked here (rule 5). A key not in [`SETTINGS`] can't be written this way. FERB's own
//! account is written only by [`write_identity`].

use std::sync::Arc;

use ferb_core::config::{Config, DEFAULT_EXCLUDE_APPS};
use ferb_store::retention::{MIN_DAYS, RULES};
use serde_json::{json, Map, Value};

use crate::shared::py_repr;

/// An app's bundle id or name, as `[privacy] exclude_apps` holds it.
pub const APP_CHARS: usize = 200;

/// `ferb/act/browser/profiles.py` `BROWSERS`' keys (checked by `rust/fixtures/daemon-a`).
pub const BROWSERS: [&str; 13] = [
    "chrome",
    "chrome-beta",
    "chrome-dev",
    "chrome-canary",
    "chromium",
    "brave",
    "edge",
    "edge-beta",
    "edge-dev",
    "edge-canary",
    "vivaldi",
    "arc",
    "opera",
];

/// The daemon applies these at once (the Redactor, the egress proxy, hearing).
pub const PRIVACY: [&str; 2] = ["privacy.exclude_apps", "privacy.exclude_domains"];

/// `BadSetting`: a key the app may not write, or a value of the wrong kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadSetting(pub String);

impl std::fmt::Display for BadSetting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BadSetting {}

fn bad<T>(message: impl Into<String>) -> Result<T, BadSetting> {
    Err(BadSetting(message.into()))
}

#[derive(Clone, Copy)]
enum Check {
    Bool,
    Days,
    Apps,
    Hosts,
    Profile,
}

/// One setting: its config place, its check and its default.
#[derive(Clone)]
pub struct Setting {
    pub section: &'static str,
    pub key: &'static str,
    check: Check,
    pub default: Value,
}

impl Setting {
    pub fn name(&self) -> String {
        format!("{}.{}", self.section, self.key)
    }

    fn check(&self, value: &Value) -> Result<Value, BadSetting> {
        match self.check {
            Check::Bool => match value {
                Value::Bool(_) => Ok(value.clone()),
                _ => bad("must be true or false"),
            },
            Check::Days => match value.as_i64() {
                Some(n) if value.is_i64() && n >= MIN_DAYS => Ok(value.clone()),
                // a whole number Python's int holds past i64 is still a whole number of days
                _ if value.is_u64() => Ok(value.clone()),
                _ => bad(format!(
                    "must be a whole number of days, at least {MIN_DAYS}"
                )),
            },
            Check::Apps => apps(value),
            Check::Hosts => hosts(value),
            Check::Profile => profile(value),
        }
    }
}

fn controls(text: &str) -> bool {
    text.chars().any(|c| (c as u32) < 32)
}

fn apps(value: &Value) -> Result<Value, BadSetting> {
    let Value::Array(items) = value else {
        return bad("must be a list of apps");
    };
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let text = item
            .as_str()
            .map(|s| ferb_py::value::strip(s).to_owned())
            .unwrap_or_default();
        if text.is_empty() || text.chars().count() > APP_CHARS || controls(&text) {
            return bad(format!(
                "has {}, which isn't an app's bundle id or name",
                py_repr(item)
            ));
        }
        if !out.contains(&text) {
            out.push(text);
        }
    }
    Ok(json!(out))
}

/// `HOST.fullmatch`: `[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*`.
fn is_host(host: &str) -> bool {
    !host.is_empty()
        && host.split('.').all(|label| {
            let b = label.as_bytes();
            let alnum = |c: &u8| c.is_ascii_lowercase() || c.is_ascii_digit();
            !b.is_empty()
                && alnum(&b[0])
                && alnum(&b[b.len() - 1])
                && b.iter().all(|c| alnum(c) || *c == b'-')
        })
}

fn hosts(value: &Value) -> Result<Value, BadSetting> {
    let Value::Array(items) = value else {
        return bad("must be a list of sites");
    };
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let host = item
            .as_str()
            .map(|s| {
                ferb_py::value::strip(s)
                    .to_lowercase()
                    .trim_matches('.')
                    .to_owned()
            })
            .unwrap_or_default();
        if !is_host(&host) {
            return bad(format!(
                "has {}, which isn't a site name like example.com (no https:// or path)",
                py_repr(item)
            ));
        }
        if !out.contains(&host) {
            out.push(host);
        }
    }
    Ok(json!(out))
}

fn profile(value: &Value) -> Result<Value, BadSetting> {
    let Some(raw) = value.as_str() else {
        return bad("must be a browser profile as browser/folder, or empty");
    };
    let text = ferb_py::value::strip(raw);
    let (browser, folder) = text.split_once('/').unwrap_or((text, ""));
    if !text.is_empty()
        && (!BROWSERS.contains(&browser)
            || ferb_py::value::strip(folder).is_empty()
            || text.chars().count() > APP_CHARS
            || controls(text))
    {
        return bad(format!(
            "has {}, which isn't a browser profile as browser/folder",
            py_repr(value)
        ));
    }
    Ok(json!(text))
}

/// `SETTINGS`, in Python's order.
pub fn settings() -> Vec<Setting> {
    let mut out = vec![
        Setting {
            section: "privacy",
            key: "exclude_apps",
            check: Check::Apps,
            default: json!(DEFAULT_EXCLUDE_APPS),
        },
        Setting {
            section: "privacy",
            key: "exclude_domains",
            check: Check::Hosts,
            default: json!([]),
        },
        Setting {
            section: "sense",
            key: "window_titles",
            check: Check::Bool,
            default: json!(true),
        },
    ];
    for r in RULES.iter() {
        if let (Some((section, key)), Some(default)) = (r.setting, r.default) {
            out.push(Setting {
                section,
                key,
                check: Check::Days,
                default: json!(default),
            });
        }
    }
    out.push(Setting {
        section: "browsing",
        key: "owner_profile",
        check: Check::Profile,
        default: json!(""),
    });
    out
}

/// What reading config.toml failed with.
pub type ConfigError = ferb_core::config::ConfigError;

/// `read`: every listed setting's value: config.toml's, else its default.
pub fn read(config: &Config) -> Result<Map<String, Value>, ConfigError> {
    let mut values = Map::new();
    for s in settings() {
        let raw = config.get(s.section, s.key)?;
        values.insert(
            s.name(),
            match raw {
                Some(v) if !v.is_null() => v,
                _ => s.default.clone(),
            },
        );
    }
    Ok(values)
}

/// Why a write failed: the value, or the config file.
#[derive(Debug)]
pub enum WriteError {
    Bad(BadSetting),
    Config(ConfigError),
}

/// `write`: check one value and write it in one atomic save; returns what was stored.
pub fn write(config: &Config, name: &str, value: &Value) -> Result<Value, WriteError> {
    let Some(setting) = settings().into_iter().find(|s| s.name() == name) else {
        let shown = if name.is_empty() { "(no key)" } else { name };
        return Err(WriteError::Bad(BadSetting(format!(
            "{shown} can't be changed from FERB's window"
        ))));
    };
    let checked = setting
        .check(value)
        .map_err(|e| WriteError::Bad(BadSetting(format!("{name} {e}"))))?;
    config
        .set(setting.section, setting.key, &checked)
        .map_err(WriteError::Config)?;
    Ok(checked)
}

/// `EMAIL`: `[^@\s]+@[^@\s]+\.[^@\s]+`, matched whole.
fn is_email(text: &str) -> bool {
    let bad = |c: char| c == '@' || ferb_py::value::is_py_space(c);
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    if local.is_empty() || local.chars().any(bad) || domain.chars().any(bad) {
        return false;
    }
    // the domain's last dot with something on both sides (the regex backtracks to any such dot)
    domain
        .char_indices()
        .any(|(i, c)| c == '.' && i > 0 && i + 1 < domain.len())
}

const IDENTITY_KEYS: [(&str, &str); 4] = [
    ("mailbox", "provider"),
    ("mailbox", "address"),
    ("identity", "browser"),
    ("identity", "profile"),
];

/// `write_identity`: FERB's own Google account and the browser profile it is signed into (APP-1), in one checked
/// write; or, with every value empty, none (all four keys go). Returns what was stored.
pub fn write_identity(
    config: &Config,
    address: &str,
    browser: &str,
    folder: &str,
) -> Result<Map<String, Value>, WriteError> {
    let address = ferb_py::value::strip(address).to_lowercase();
    let (browser, folder) = (
        ferb_py::value::strip(browser).to_owned(),
        ferb_py::value::strip(folder).to_owned(),
    );
    let mut doc = config.load().map_err(WriteError::Config)?;
    if address.is_empty() && browser.is_empty() && folder.is_empty() {
        let mut present = false;
        for (section, key) in IDENTITY_KEYS {
            present |= doc.remove(section, key).map_err(WriteError::Config)?;
        }
        if present {
            config.save(&doc).map_err(WriteError::Config)?;
        }
        return Ok(object3("", "", ""));
    }
    if !is_email(&address) || address.chars().count() > 320 {
        return Err(WriteError::Bad(BadSetting(format!(
            "{} isn't an email address",
            ferb_clock::py::repr(&address)
        ))));
    }
    if !BROWSERS.contains(&browser.as_str())
        || folder.is_empty()
        || folder.chars().count() > APP_CHARS
        || controls(&folder)
    {
        return Err(WriteError::Bad(BadSetting(
            "FERB's profile must be one of your browsers' profiles".into(),
        )));
    }
    for ((section, key), value) in
        IDENTITY_KEYS
            .iter()
            .zip(["gmail", address.as_str(), browser.as_str(), folder.as_str()])
    {
        doc.set(section, key, &json!(value))
            .map_err(WriteError::Config)?;
    }
    config.save(&doc).map_err(WriteError::Config)?;
    Ok(object3(&address, &browser, &folder))
}

fn object3(address: &str, browser: &str, profile: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("address".into(), json!(address));
    m.insert("browser".into(), json!(browser));
    m.insert("profile".into(), json!(profile));
    m
}

/// What `settings.set` changes at once besides config.toml: the privacy exclusions reach the Redactor (and through
/// it the egress proxy and hearing) now, not at restart (`_apply_privacy`), and the change is journaled.
pub trait SettingsHost: Send + Sync {
    /// `_apply_privacy`.
    fn apply_privacy(&self);
    /// `self._journal().append(kind, data, origin=...)`.
    fn journal(&self, kind: &str, data: Value, origin: &str);
}

/// `settings.get` and `settings.set` (`areas/rpc.py:36-51`), on `server`. Registered by [`super::register_settings`].
pub(crate) fn handlers(
    config: Arc<Config>,
    host: Arc<dyn SettingsHost>,
    server: &ferb_rpc::Server,
) {
    let c = config.clone();
    server.register_unary("settings.get", move |_s, _p| {
        let c = c.clone();
        async move {
            let values = read(&c).map_err(config_fault)?;
            Ok(crate::shared::object(json!({"values": values})))
        }
    });
    server.register_unary("settings.set", move |_s, params: ferb_rpc::wire::Params| {
        let (c, host) = (config.clone(), host.clone());
        async move {
            let name = crate::shared::text(&params, "key");
            let value = params.get("value").cloned().unwrap_or(Value::Null);
            let stored = match write(&c, &name, &value) {
                Ok(v) => v,
                Err(WriteError::Bad(e)) => {
                    return Err(crate::shared::refusal_as("BadSetting", e.0))
                }
                Err(WriteError::Config(e)) => return Err(config_fault(e)),
            };
            if PRIVACY.contains(&name.as_str()) {
                host.apply_privacy();
            }
            host.journal(
                "settings.changed",
                json!({"key": name, "value": stored}),
                "owner",
            );
            Ok(crate::shared::object(json!({"key": name, "value": stored})))
        }
    });
}

fn config_fault(e: ConfigError) -> ferb_rpc::server::HandlerError {
    // a config FERB refuses to read is FERB's own error (ConfigError is a FerbError in Python)
    crate::shared::refusal_as("ConfigError", e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_and_emails() {
        assert!(is_host("example.com"));
        assert!(is_host("a-b.c0"));
        assert!(!is_host("-a.com"));
        assert!(!is_host("a..com"));
        assert!(!is_host("https://a.com"));
        assert!(is_email("me@example.com"));
        assert!(is_email("me@a.b.c"));
        assert!(!is_email("me@example"));
        assert!(!is_email("me@.com"));
        assert!(!is_email("m e@a.com"));
    }
}
