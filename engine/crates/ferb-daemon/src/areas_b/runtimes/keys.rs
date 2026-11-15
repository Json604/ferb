//! The provider API keys FERB keeps in the Keychain (ferb/runtimes/keys.py; D32, GATE-543, GATE-547). A key is the
//! item `api.sealed/<provider>` of service "ferb", written and read only through FERB's signed secret helper
//! ([`ferb_identity::Helper`]); an older item `security` wrote (`api/<provider>`) is moved whenever one exists. A key
//! lives only as a [`Secret`]: never logged, stored or returned anywhere else. Presence checks never read a value.
//! One lock per provider covers each read, move, store and delete; a key read once stays in memory for the daemon's
//! run (fewer Keychain reads, T-502).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ferb_identity::{Error as IdError, Helper, Secret, Service};
use serde_json::{json, Value};

use ferb_runtimes::catalog::api_providers;

/// The `security` program as keys.py runs it (`security <args>`, stderr discarded): its exit status and stdout.
/// Only `find-generic-password` (presence, or `-w` for an older item's value, read only to move it) and
/// `delete-generic-password` are ever run.
pub trait Security: Send + Sync {
    fn run(&self, args: &[&str]) -> (i32, Secret);
}

/// The Keychain's service for FERB's own items (ferb/identity/secrets.py SERVICE).
pub const SERVICE: &str = "ferb";

/// A problem to journal (`keys.old_item_not_deleted`, provider only).
pub type Problems = Vec<Value>;

/// keys.py's module state: the keys read this run, one lock per provider.
#[derive(Default)]
pub struct KeyCache {
    cache: Mutex<HashMap<String, Secret>>,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl KeyCache {
    fn lock(&self, provider: &str) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(provider.to_string())
            .or_default()
            .clone()
    }

    fn get(&self, provider: &str) -> Option<Secret> {
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(provider)
            .map(Secret::duplicate)
    }

    fn put(&self, provider: &str, value: &Secret) {
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(provider.to_string(), value.duplicate());
    }

    fn pop(&self, provider: &str) {
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(provider);
    }
}

/// Why a key operation stopped: the helper or a program couldn't be run at all (Python lets the OSError through).
#[derive(Debug)]
pub struct KeyFault {
    pub class: String,
    pub message: String,
}

/// What a helper call gave instead of an answer.
#[derive(Debug)]
pub enum HelperError {
    /// Python's `Refused` (a `HelperFailed` included): keys.py treats it as "no item" / "not stored".
    Refused(String),
    /// The helper couldn't be run at all: the caller's error.
    Fault(KeyFault),
}

impl From<IdError> for HelperError {
    fn from(e: IdError) -> Self {
        match e {
            IdError::Io(io) => {
                let r = ferb_runtimes::error::os_error(&io);
                HelperError::Fault(KeyFault {
                    class: r.class,
                    message: r.message,
                })
            }
            other => HelperError::Refused(other.to_string()),
        }
    }
}

/// FERB's signed secret helper as keys.py calls it (service "ferb"): store, lookup, delete. Implemented by
/// [`ferb_identity::Helper`]; tests give a fake Keychain.
pub trait KeyHelper: Send + Sync {
    fn store(&self, account: &str, value: &Secret) -> Result<(), HelperError>;
    fn lookup(&self, account: &str) -> Result<Option<Secret>, HelperError>;
    fn delete(&self, account: &str) -> Result<bool, HelperError>;
}

impl KeyHelper for Helper {
    fn store(&self, account: &str, value: &Secret) -> Result<(), HelperError> {
        Helper::store(self, Service::Ferb, account, value).map_err(HelperError::from)
    }
    fn lookup(&self, account: &str) -> Result<Option<Secret>, HelperError> {
        Helper::lookup(self, Service::Ferb, account).map_err(HelperError::from)
    }
    fn delete(&self, account: &str) -> Result<bool, HelperError> {
        Helper::delete(self, Service::Ferb, account).map_err(HelperError::from)
    }
}

/// The key store of one daemon: the helper, `security`, and the run's memory.
#[derive(Clone)]
pub struct Keys {
    pub helper: Arc<dyn KeyHelper>,
    pub security: Arc<dyn Security>,
    pub cache: Arc<KeyCache>,
}

/// keys.py `account`: the older item's account. The provider is one of the API runtimes (callers check it).
pub fn account(provider: &str) -> String {
    format!("api/{provider}")
}

/// keys.py `sealed`: the helper's item for this provider's key.
pub fn sealed(provider: &str) -> String {
    format!("api.sealed/{provider}")
}

impl Keys {
    fn present(&self, acct: &str) -> bool {
        self.security
            .run(&["find-generic-password", "-s", SERVICE, "-a", acct])
            .0
            == 0
    }

    fn sealed_value(&self, provider: &str) -> Result<Option<Secret>, KeyFault> {
        match self.helper.lookup(&sealed(provider)) {
            Ok(v) => Ok(v.filter(|s| !s.expose().is_empty())),
            Err(HelperError::Refused(_)) => Ok(None),
            Err(HelperError::Fault(f)) => Err(f),
        }
    }

    /// keys.py `_store_sealed`: stored through the helper and read back the same.
    fn store_sealed(&self, provider: &str, value: &Secret) -> Result<bool, KeyFault> {
        match self.helper.store(&sealed(provider), value) {
            Ok(()) => {}
            Err(HelperError::Refused(_)) => return Ok(false),
            Err(HelperError::Fault(f)) => return Err(f),
        }
        Ok(self
            .sealed_value(provider)?
            .is_some_and(|v| v.expose() == value.expose()))
    }

    /// keys.py `_delete_old`: a failure is a problem with the provider only; true when the older item is gone.
    fn delete_old(&self, provider: &str, problems: &mut Problems) -> bool {
        let gone = self
            .security
            .run(&[
                "delete-generic-password",
                "-s",
                SERVICE,
                "-a",
                &account(provider),
            ])
            .0
            == 0;
        if !gone && self.present(&account(provider)) {
            problems.push(json!({"provider": provider}));
            return false;
        }
        true
    }

    /// keys.py `store_key`: the owner's key, stored and read back; an older item then goes (the new key replaces it).
    pub fn store_key(
        &self,
        provider: &str,
        value: &Secret,
        problems: &mut Problems,
    ) -> Result<bool, KeyFault> {
        let lock = self.cache.lock(provider);
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        if !self.store_sealed(provider, value)? {
            return Ok(false);
        }
        self.cache.put(provider, value);
        if self.present(&account(provider)) {
            self.delete_old(provider, problems);
        }
        Ok(true)
    }

    /// keys.py `delete_key`: the helper's item and any older one go; true when neither is left.
    pub fn delete_key(&self, provider: &str, problems: &mut Problems) -> Result<bool, KeyFault> {
        let lock = self.cache.lock(provider);
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        self.cache.pop(provider);
        match self.helper.delete(&sealed(provider)) {
            Ok(_) | Err(HelperError::Refused(_)) => {}
            Err(HelperError::Fault(f)) => return Err(f),
        }
        if self.present(&account(provider)) {
            self.delete_old(provider, problems);
        }
        self.cache.pop(provider);
        Ok(!self.has_api_key(provider))
    }

    /// keys.py `read_key`: from memory, else an older item (the newer key: only an older build writes it) moved
    /// through the helper, else the helper's item.
    pub fn read_key(
        &self,
        provider: &str,
        problems: &mut Problems,
    ) -> Result<Option<Secret>, KeyFault> {
        let lock = self.cache.lock(provider);
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = self.cache.get(provider) {
            return Ok(Some(v));
        }
        let mut value: Option<Secret> = None;
        if self.present(&account(provider)) {
            let (code, out) = self.security.run(&[
                "find-generic-password",
                "-s",
                SERVICE,
                "-a",
                &account(provider),
                "-w",
            ]);
            let old = if code == 0 {
                Secret::new(ferb_py::value::strip(out.expose()).to_string())
            } else {
                Secret::new(String::new())
            };
            if !old.expose().is_empty() && self.store_sealed(provider, &old)? {
                self.delete_old(provider, problems);
            }
            if !old.expose().is_empty() {
                value = Some(old);
            }
        }
        if value.is_none() {
            value = self.sealed_value(provider)?;
        }
        if let Some(v) = &value {
            self.cache.put(provider, v);
        }
        Ok(value)
    }

    /// keys.py `stored_api_keys`: the keys of `providers` that are API runtimes, for the redactor's known values
    /// only; a provider with no key is skipped and no other provider's key is read.
    pub fn stored_api_keys(
        &self,
        providers: &[String],
        problems: &mut Problems,
    ) -> Result<Vec<Secret>, KeyFault> {
        let mut chosen: Vec<&str> = api_providers()
            .into_iter()
            .filter(|p| providers.iter().any(|q| q == p))
            .collect();
        chosen.sort_unstable();
        let mut found = Vec::new();
        for provider in chosen {
            if self.has_api_key(provider) {
                if let Some(key) = self.read_key(provider, problems)? {
                    found.push(key);
                }
            }
        }
        Ok(found)
    }

    /// keys.py `has_api_key`: presence only (the helper's item, or an older one not moved yet).
    pub fn has_api_key(&self, provider: &str) -> bool {
        self.present(&sealed(provider)) || self.present(&account(provider))
    }
}

/// The real `security` (/usr/bin/security), stderr discarded.
#[cfg(target_os = "macos")]
pub struct SystemSecurity;

#[cfg(target_os = "macos")]
impl Security for SystemSecurity {
    fn run(&self, args: &[&str]) -> (i32, Secret) {
        match std::process::Command::new("security")
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
        {
            Ok(out) => (
                out.status.code().unwrap_or(-1),
                Secret::new(String::from_utf8_lossy(&out.stdout).into_owned()),
            ),
            Err(_) => (-1, Secret::new(String::new())),
        }
    }
}
