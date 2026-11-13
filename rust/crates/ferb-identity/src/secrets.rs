//! FERB's own configured secrets and their exact origin constraints: a port of
//! `ferb/identity/secrets.py`. The account hint and the origins are not secret; only a sealed
//! fill reveals a value, after the origin check. Errors never include a value or the helper's
//! output.
//!
//! Not ported: the `keychain=` branch (`secrets.py:80-84`), which runs `/usr/bin/security`
//! against a named keychain file; the daemon never passes one (`ferb/daemon/main.py:146`).
//! Python's methods are `async` and run the helper on a thread; these block, and the caller runs
//! them off its event loop.

use std::fmt;

use serde_json::{Map, Value};

use crate::{
    helper::{Helper, Request, Service},
    origin::{normalize_origin, BadAddress},
    Secret,
};

/// `secrets.py:13`.
pub const SERVICE: Service = Service::Ferb;

/// `ferb/core/types.py:18`: a configured secret's key, e.g. "ferb/email/password". The handle a
/// model sees; also the Keychain account.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SecretRef {
    pub key: String,
}

/// `ferb/core/types.py:23`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretSpec {
    pub secret: SecretRef,
    pub allowed_origins: Vec<String>,
    pub account_hint: String,
}

/// `secrets.py:16-21`: a configured secret couldn't be read. `reason`: why, when the helper said
/// (decision: HAND-410); never the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretUnavailable {
    pub key: String,
    pub reason: String,
}

impl fmt::Display for SecretUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key)
    }
}

impl std::error::Error for SecretUnavailable {}

/// `secrets.py:32-33`. An address no URL can be (Python's `ValueError`) is an `Err`, for the
/// origin or for any origin of the spec.
pub fn origin_allowed(origin: &str, spec: &SecretSpec) -> Result<bool, BadAddress> {
    if origin.is_empty() {
        return Ok(false);
    }
    let origin = normalize_origin(origin)?;
    let mut allowed = false;
    for o in &spec.allowed_origins {
        allowed |= normalize_origin(o)? == origin;
    }
    Ok(allowed)
}

/// `str(value)` for what a config file holds: text, whole numbers and booleans.
fn config_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        other => other.to_string(),
    }
}

/// `secrets.py:36-41`: the `[secrets]` section of FERB's config, in its own order.
pub fn specs_from_config(section: Option<&Map<String, Value>>) -> Vec<SecretSpec> {
    let Some(section) = section else {
        return Vec::new();
    };
    section
        .iter()
        .map(|(key, v)| SecretSpec {
            secret: SecretRef { key: key.clone() },
            allowed_origins: v
                .get("allowed_origins")
                .and_then(Value::as_array)
                .map(|origins| origins.iter().map(config_text).collect())
                .unwrap_or_default(),
            account_hint: v.get("account_hint").map(config_text).unwrap_or_default(),
        })
        .collect()
}

/// `secrets.py:44-84`: FERB's configured secrets, read through the signed helper.
pub struct KeychainSecretStore {
    specs: Box<dyn Fn() -> Vec<SecretSpec> + Send + Sync>,
    helper: Helper,
}

impl KeychainSecretStore {
    /// `specs` is read at call time: config edits apply without a restart (`secrets.py:46`).
    pub fn new(
        specs: impl Fn() -> Vec<SecretSpec> + Send + Sync + 'static,
        helper: Helper,
    ) -> Self {
        KeychainSecretStore {
            specs: Box::new(specs),
            helper,
        }
    }

    pub fn specs(&self) -> Vec<SecretSpec> {
        (self.specs)()
    }

    pub fn spec(&self, secret: &SecretRef) -> Option<SecretSpec> {
        self.specs().into_iter().find(|spec| spec.secret == *secret)
    }

    /// Configured; whether the Keychain holds it is [`Self::present`].
    pub fn has(&self, secret: &SecretRef) -> bool {
        self.spec(secret).is_some()
    }

    /// `secrets.py:58-59`: configured and shown by the Keychain; no value is read.
    pub fn present(&self, secret: &SecretRef) -> bool {
        self.has(secret) && self.helper.present(SERVICE, &secret.key).unwrap_or(false)
    }

    /// `secrets.py:61-67`: the value, for the sealed fill only.
    pub fn reveal(&self, secret: &SecretRef) -> Result<Secret, SecretUnavailable> {
        let unavailable = |reason: String| SecretUnavailable {
            key: secret.key.clone(),
            reason,
        };
        if !self.has(secret) {
            return Err(unavailable(String::new()));
        }
        match self.helper.request(&Request::Lookup {
            service: SERVICE,
            account: &secret.key,
        }) {
            // `str(out.get('value', ''))`: a reply with no value is the empty text.
            Ok(reply) => Ok(reply.value.unwrap_or_else(|| Secret::new(String::new()))),
            // the helper's reason (HAND-410): no value, no account
            Err(e) => Err(unavailable(e.refusal().unwrap_or_default().to_owned())),
        }
    }
}
