//! Which models each connection can offer the owner (ferb/runtimes/models.py; RUN-429, RUN-431..433, RUN-464): an
//! API key's live list from its provider's models endpoint, the models FERB's Codex and Claude sign-ins know, and
//! each model's documented effort levels. Never chooses a model; never logs or returns a key.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

pub const TIMEOUT_S: f64 = 20.0;

pub fn url(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "https://api.anthropic.com/v1/models?limit=1000",
        "openai" => "https://api.openai.com/v1/models",
        _ => "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000",
    }
}

fn headers(provider: &str, key: &str) -> Vec<(String, String)> {
    match provider {
        "anthropic" => vec![
            ("x-api-key".into(), key.into()),
            ("anthropic-version".into(), "2023-06-01".into()),
        ],
        "openai" => vec![("Authorization".into(), format!("Bearer {key}"))],
        _ => vec![("x-goog-api-key".into(), key.into())],
    }
}

/// models.py `parse`: the model ids in a provider's list answer, sorted.
pub fn parse(provider: &str, data: &Value) -> Vec<String> {
    let mut ids: Vec<String> = if provider == "google" {
        data.get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|m| {
                m.get("name").is_some_and(ferb_py::value::truthy)
                    && m.get("supportedGenerationMethods")
                        .and_then(Value::as_array)
                        .is_some_and(|a| a.iter().any(|x| x == "generateContent"))
            })
            .map(|m| {
                let n = ferb_py::value::py_str(&m["name"]);
                n.strip_prefix("models/").map(str::to_string).unwrap_or(n)
            })
            .collect()
    } else {
        data.get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|m| m.get("id").is_some_and(ferb_py::value::truthy))
            .map(|m| ferb_py::value::py_str(&m["id"]))
            .collect()
    };
    ids.sort();
    ids.dedup();
    ids
}

/// models.py `provider_models`: the models the provider offers this key, or the provider's (or transport's) words.
/// `origin` replaces the provider's host in tests.
pub async fn provider_models(
    provider: &str,
    key: &ferb_identity::Secret,
    origin: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut target = url(provider).to_string();
    if let Some(o) = origin {
        if let Some(i) = target
            .find("://")
            .and_then(|i| target[i + 3..].find('/').map(|j| i + 3 + j))
        {
            target = format!("{o}{}", &target[i..]);
        }
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs_f64(TIMEOUT_S))
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.get(&target);
    for (k, v) in headers(provider, key.expose()) {
        req = req.header(k, v);
    }
    let resp = req.send().await.map_err(|e| {
        let kind = if e.is_timeout() {
            "ReadTimeout"
        } else {
            "ConnectError"
        };
        format!("{kind}: {}", e.to_string().replace(key.expose(), "[key]"))
    })?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    let parsed = ferb_py::json::loads(&text);
    if status != 200 {
        let words = match &parsed {
            Ok(Value::Object(m)) => match m.get("error") {
                Some(Value::Object(e)) => {
                    ferb_py::value::py_str(e.get("message").unwrap_or(&Value::Null))
                }
                Some(e) if ferb_py::value::truthy(e) => ferb_py::value::py_str(e),
                _ => text.clone(),
            },
            _ => text.clone(),
        };
        return Err(format!(
            "HTTP {status}: {}",
            ferb_py::value::prefix(&words, 300)
        ));
    }
    parsed
        .map(|d| parse(provider, &d))
        .map_err(|_| "the answer wasn't a model list".to_string())
}

/// models.py `known_models`: Codex's catalog for FERB's login (else the pinned one's); Claude's documented ids.
pub fn known_models(name: &str, home: &Path) -> Vec<String> {
    match name {
        "codex" => {
            let rows = crate::codex::catalog(home)
                .and_then(|(d, _)| crate::codex::models(&d))
                .unwrap_or_default();
            let mut out: Vec<String> = rows
                .iter()
                .filter(|m| m.get("slug").is_some_and(ferb_py::value::truthy))
                .map(|m| ferb_py::value::py_str(&m["slug"]))
                .collect();
            out.sort();
            out.dedup();
            out
        }
        "claude" => {
            let mut out: Vec<String> = crate::api::anthropic::LIMITS
                .iter()
                .map(|(m, _, _)| m.to_string())
                .collect();
            out.sort();
            out
        }
        _ => Vec::new(),
    }
}

const ALL5: &[&str] = &["low", "medium", "high", "xhigh", "max"];
const NO_XHIGH: &[&str] = &["low", "medium", "high", "max"];

/// models.py ANTHROPIC_LEVELS (platform.claude.com effort page, read 2026-11-10).
fn anthropic_levels(model: &str) -> Option<(&'static [&'static str], &'static str)> {
    const TABLE: &[(&str, &[&str], &str)] = &[
        ("claude-fable-5-2", ALL5, "high"),
        ("claude-mythos-5-2", ALL5, "high"),
        ("claude-fable-5-1", ALL5, "high"),
        ("claude-mythos-5-1", ALL5, "high"),
        ("claude-fable-5", ALL5, "high"),
        ("claude-mythos-5", ALL5, "high"),
        ("claude-opus-5", ALL5, "high"),
        ("claude-opus-4-8", ALL5, "high"),
        ("claude-opus-4-7", ALL5, "high"),
        ("claude-sonnet-5-5", ALL5, "high"),
        ("claude-sonnet-5", ALL5, "high"),
        ("claude-opus-5-5", ALL5, "medium"),
        ("claude-haiku-5-5", ALL5, "medium"),
        ("claude-mythos-preview", NO_XHIGH, "high"),
        ("claude-opus-4-6", NO_XHIGH, "high"),
        ("claude-sonnet-4-6", NO_XHIGH, "high"),
        ("claude-opus-4-5", &["low", "medium", "high"], "high"),
    ];
    documented(TABLE, model)
}

/// models.py GOOGLE_LEVELS (the Gemini thinking page's table, read 2026-11-10).
fn google_levels(model: &str) -> Option<(&'static [&'static str], &'static str)> {
    const TABLE: &[(&str, &[&str], &str)] = &[
        ("gemini-4-argon", &["low", "medium", "high"], "medium"),
        ("gemini-3.8-flash", &["low", "medium", "high"], "medium"),
        (
            "gemini-3.6-flash",
            &["minimal", "low", "medium", "high"],
            "medium",
        ),
        (
            "gemini-3.5-flash-lite",
            &["minimal", "low", "medium", "high"],
            "minimal",
        ),
        ("gemini-3.1-pro-preview", &["low", "medium", "high"], "high"),
        (
            "gemini-3.1-flash-lite-image",
            &["minimal", "high"],
            "minimal",
        ),
        (
            "gemini-3-flash-preview",
            &["minimal", "low", "medium", "high"],
            "high",
        ),
        ("gemini-3-pro-preview", &["low", "high"], "high"),
    ];
    documented(TABLE, model)
}

/// models.py OPENAI_DEFAULTS.
fn openai_default(model: &str) -> Option<&'static str> {
    const TABLE: &[(&str, &[&str], &str)] = &[
        ("gpt-5.5", &[], "medium"),
        ("gpt-5.6", &[], "medium"),
        ("gpt-6.1-sol", &[], "medium"),
        ("gpt-6-sol", &[], "medium"),
        ("gpt-6-luna", &[], "medium"),
    ];
    documented(TABLE, model).map(|(_, d)| d)
}

/// models.py `_documented`: the longest documented id the model is or starts with (followed by "-").
fn documented(
    table: &[(&str, &'static [&'static str], &'static str)],
    model: &str,
) -> Option<(&'static [&'static str], &'static str)> {
    table
        .iter()
        .filter(|(k, _, _)| model == *k || model.starts_with(&format!("{k}-")))
        .max_by_key(|(k, _, _)| k.chars().count())
        .map(|(_, l, d)| (*l, *d))
}

/// models.py `effort_levels`: each model's levels the owner can pick and its documented default.
#[allow(clippy::type_complexity)]
pub fn effort_levels(
    name: &str,
    home: &Path,
    models: &[String],
) -> (BTreeMap<String, Vec<String>>, BTreeMap<String, String>) {
    let mut levels = BTreeMap::new();
    let mut defaults = BTreeMap::new();
    if name == "codex" {
        let rows = crate::codex::catalog(home)
            .and_then(|(d, _)| crate::codex::models(&d))
            .unwrap_or_default();
        for row in rows {
            let Some(slug) = row
                .get("slug")
                .filter(|s| ferb_py::value::truthy(s))
                .map(ferb_py::value::py_str)
            else {
                continue;
            };
            let list: Vec<String> = row
                .get("supported_reasoning_levels")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|r| r.get("effort").filter(|e| ferb_py::value::truthy(e)))
                .map(ferb_py::value::py_str)
                .filter(|e| e != "ultra")
                .collect();
            levels.insert(slug.clone(), list);
            if let Some(d) = row
                .get("default_reasoning_level")
                .filter(|d| ferb_py::value::truthy(d))
            {
                defaults.insert(slug, ferb_py::value::py_str(d));
            }
        }
        return (levels, defaults);
    }
    for model in models {
        match name {
            "claude" | "anthropic" => {
                if let Some((l, d)) = anthropic_levels(model) {
                    levels.insert(model.clone(), l.iter().map(|s| s.to_string()).collect());
                    if name == "anthropic" {
                        defaults.insert(model.clone(), d.to_string());
                    }
                }
            }
            "google" => {
                if let Some((l, d)) = google_levels(model) {
                    levels.insert(model.clone(), l.iter().map(|s| s.to_string()).collect());
                    defaults.insert(model.clone(), d.to_string());
                }
            }
            "openai" => {
                if let Some(d) = openai_default(model) {
                    defaults.insert(model.clone(), d.to_string());
                }
            }
            _ => {}
        }
    }
    (levels, defaults)
}
