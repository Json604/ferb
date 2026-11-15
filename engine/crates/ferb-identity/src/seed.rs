//! The check of FERB's dedicated Chrome profile for setup and doctor: a port of
//! `ferb/identity/seed.py`. It reads Chrome's `Local State` and each profile's `Preferences` for
//! account names only; it never copies credentials, reads cookies or picks among several
//! eligible profiles.

use std::{collections::BTreeSet, fmt, fs, io, path::Path};

use serde_json::Value;

use crate::py;

/// `seed.py:12`: why no profile was chosen, in words for the owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedError(pub String);

impl fmt::Display for SeedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SeedError {}

/// `seed.py:16-20`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromeProfile {
    /// "Profile 7"
    pub folder: String,
    /// What the owner calls it ("FERB").
    pub name: String,
    /// The Chrome accounts signed into it (lowercased emails), sorted.
    pub accounts: Vec<String>,
}

/// `str(value)` for what Chrome's JSON holds where a name is expected.
fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        other => py::dumps(other),
    }
}

fn io_name(e: &io::Error) -> &'static str {
    match e.kind() {
        io::ErrorKind::NotFound => "FileNotFoundError",
        io::ErrorKind::PermissionDenied => "PermissionError",
        io::ErrorKind::IsADirectory => "IsADirectoryError",
        io::ErrorKind::NotADirectory => "NotADirectoryError",
        _ => "OSError",
    }
}

/// `seed.py:23-37`: every Chrome profile with the accounts signed into it. Where Python would
/// stop with an uncaught `TypeError` / `AttributeError` (a `Local State` whose shape is not
/// Chrome's), this refuses with the same sentence.
pub fn profiles(chrome_data: &Path) -> Result<Vec<ChromeProfile>, SeedError> {
    let unreadable = |kind: &str| SeedError(format!("can't read Chrome's Local State ({kind})"));
    let bytes = fs::read(chrome_data.join("Local State")).map_err(|e| unreadable(io_name(&e)))?;
    let state = std::str::from_utf8(&bytes).map_err(|_| unreadable("UnicodeDecodeError"))?;
    let state: Value = serde_json::from_str(state).map_err(|_| unreadable("JSONDecodeError"))?;
    let step = |value: &Value, key: &str| -> Result<Value, SeedError> {
        match value {
            Value::Object(map) => map.get(key).cloned().ok_or_else(|| unreadable("KeyError")),
            _ => Err(unreadable("TypeError")),
        }
    };
    let cache = step(&step(&state, "profile")?, "info_cache")?;
    let Value::Object(cache) = cache else {
        return Err(unreadable("TypeError"));
    };
    let mut out = Vec::new();
    for (folder, info) in &cache {
        let Value::Object(info) = info else {
            return Err(unreadable("TypeError"));
        };
        let mut accounts = BTreeSet::new();
        if let Some(user) = info.get("user_name").filter(|v| py::truthy(v)) {
            accounts.insert(text(user).to_lowercase());
        }
        accounts
            .extend(signed_in(&chrome_data.join(folder).join("Preferences")).unwrap_or_default());
        accounts.remove("");
        let name = info
            .get("name")
            .filter(|v| py::truthy(v))
            .map_or_else(|| folder.clone(), text);
        out.push(ChromeProfile {
            folder: folder.clone(),
            name,
            accounts: accounts.into_iter().collect(),
        });
    }
    Ok(out)
}

/// `seed.py:31-35`: the emails of a profile's `account_info`; `None` (nothing added) when the
/// file can't be read or any part of it is not shaped as Chrome writes it.
fn signed_in(preferences: &Path) -> Option<Vec<String>> {
    let prefs: Value =
        serde_json::from_str(std::str::from_utf8(&fs::read(preferences).ok()?).ok()?).ok()?;
    let Some(info) = prefs.as_object()?.get("account_info") else {
        return Some(Vec::new());
    };
    let mut out = Vec::new();
    for account in info.as_array()? {
        out.push(
            account
                .as_object()?
                .get("email")
                .map(text)
                .unwrap_or_default()
                .to_lowercase(),
        );
    }
    Some(out)
}

/// `seed.py:40-46`.
fn only_ferb(p: &ChromeProfile, address: &str) -> Result<ChromeProfile, SeedError> {
    if p.accounts.len() == 1 && p.accounts[0] == address {
        return Ok(p.clone());
    }
    let holds = p.accounts.iter().any(|a| a == address);
    let others = p.accounts.iter().any(|a| a != address);
    let why = if others && holds {
        "also holds other accounts".to_owned()
    } else {
        format!("isn't signed into {address}")
    };
    Err(SeedError(format!(
        "profile {} ({}) {why}; FERB's profile must hold only {address}",
        p.name, p.folder
    )))
}

/// `seed.py:49-62`. Setup: the one profile holding FERB's account and nothing else; anything
/// ambiguous or mixed is refused.
pub fn choose(
    all: &[ChromeProfile],
    address: &str,
    name: Option<&str>,
) -> Result<ChromeProfile, SeedError> {
    let address = address.to_lowercase();
    let hits: Vec<&ChromeProfile> = all
        .iter()
        .filter(|p| p.accounts.contains(&address))
        .filter(|p| name.is_none_or(|n| n == p.name || n == p.folder))
        .collect();
    match hits.as_slice() {
        [] => Err(SeedError(format!(
            "no Chrome profile{} is signed into {address}. Make a dedicated Chrome profile, sign FERB's account \
             into it (nothing else), and run this again.",
            name.filter(|n| !n.is_empty()).map(|n| format!(" named {}", py::repr(n))).unwrap_or_default()
        ))),
        [one] => only_ferb(one, &address),
        several => Err(SeedError(format!(
            "several Chrome profiles hold FERB's account: {}. Pick one with --profile NAME.",
            several.iter().map(|p| format!("{} ({})", p.name, p.folder)).collect::<Vec<_>>().join(", ")
        ))),
    }
}

/// `seed.py:65-70`. Doctor and pairing: the configured folder still exists and holds exactly
/// FERB's account.
pub fn verify(address: &str, folder: &str, chrome_data: &Path) -> Result<ChromeProfile, SeedError> {
    match profiles(chrome_data)?.iter().find(|p| p.folder == folder) {
        Some(p) => only_ferb(p, &address.to_lowercase()),
        None => Err(SeedError(format!(
            "Chrome has no profile folder {} any more",
            py::repr(folder)
        ))),
    }
}
