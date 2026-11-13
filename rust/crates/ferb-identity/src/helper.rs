//! The caller of FERB's pinned, signed secret helper (`ferb-secrets`). A port of
//! `ferb/identity/helper.py`; the helper itself (`ferb/identity/secret_helper.swift`, pinned in
//! `assets/secret-helper/`, decision: REL-404) stays Swift and unchanged.
//!
//! This module is the part every platform shares: the request and reply protocol, what a failure
//! means and what may be said about it. Finding the helper program, holding it to its pin and
//! running it is the platform's part, behind [`Launcher`] (`ferb-platform-macos` on the Mac:
//! `/usr/bin/codesign` against the pinned requirement, then a child with a scrubbed environment).
//!
//! What holds for every call (decisions: GATE-548, GATE-543, HAND-410; AGENTS.md rules 6 and 7):
//! - The helper is the program the [`Launcher`] verified for this home, verified again before
//!   every use. No pin, no file or a refused signature means no run; there is no unsigned
//!   fallback and no other way to choose the program (`helper.py:29-38`).
//! - A value travels only in the request body the launcher puts on the child's stdin and in the
//!   reply it reads from the child's stdout: never in argv, never in the environment.
//! - The request and the reply live in buffers zeroed on drop; a value leaves this module only as
//!   a [`Secret`].
//! - A failure says why as far as FERB can tell and never carries the request, the value or the
//!   Keychain account (`helper.py:58-85`).

use std::{
    fmt, io,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::Arc,
};

use serde_json::{Map, Value};
use zeroize::{Zeroize, Zeroizing};

use crate::{py, Error, Result, Secret};

/// The platform's part of every helper call (`helper.py:29-47`).
pub trait Launcher: Send + Sync {
    /// The helper program of `home`, only once the platform accepted its signature against the
    /// pin setup wrote beside it. A refusal is an [`Error::Refused`] in Python's words.
    fn verified(&self, home: &Path) -> Result<PathBuf>;
    /// Runs `program` with `body` on its stdin: whether it failed (any exit but 0, a signal
    /// included) and everything it wrote on stdout, in a buffer zeroed on drop. No argument,
    /// private pipes, stderr discarded. A platform that can check the started program itself
    /// writes `body` only once it is the pinned code; otherwise it refuses with
    /// `io::Error::other(Error::Refused(..))`, which [`Error`]'s `From<io::Error>` turns back
    /// into that refusal (decision: ENG-6).
    fn run(&self, program: &Path, body: &[u8]) -> io::Result<(bool, Zeroizing<Vec<u8>>)>;
}

/// The Keychain services the helper accepts (`secret_helper.swift:17`); nothing else can be named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    /// `"ferb"`: FERB's own configured secrets and API keys (`secrets.py:13`, `runtimes/keys.py`).
    Ferb,
    /// `"ferb.owner-secrets"`: the owner's saved secrets (`owner_secrets.py:41`).
    OwnerSecrets,
}

impl Service {
    pub fn as_str(self) -> &'static str {
        match self {
            Service::Ferb => "ferb",
            Service::OwnerSecrets => "ferb.owner-secrets",
        }
    }
}

/// One request, in the helper's protocol (`secret_helper.swift:6-47`): one JSON object on stdin,
/// one JSON object on stdout, exit 1 with `{"error":"secret operation failed"}` on any failure.
/// The keys are written in the order Python's callers write them.
pub enum Request<'a> {
    /// `{"operation": "store", "service", "account", "value"}` -> `{"ok": true}`.
    Store {
        service: Service,
        account: &'a str,
        value: &'a Secret,
    },
    /// `{"operation": "lookup", "service", "account"}` -> `{"value": ...}`.
    Lookup { service: Service, account: &'a str },
    /// `{"operation": "present", "service", "account"}` -> `{"present": bool}`; reads no value.
    Present { service: Service, account: &'a str },
    /// `{"operation": "delete", "service", "account"}` -> `{"deleted": bool}`; reads no value
    /// (decision: GATE-2).
    Delete { service: Service, account: &'a str },
    /// `{"operation": "seal", "key", "value", "command", "origin"}` -> `{"nonce", "ct"}`
    /// (`helper.py:98-101`): AES-GCM under a 32-byte key, bound to `command|origin`.
    Seal {
        key: &'a [u8],
        value: &'a Secret,
        command: &'a str,
        origin: &'a str,
    },
}

impl Request<'_> {
    fn operation(&self) -> &'static str {
        match self {
            Request::Store { .. } => "store",
            Request::Lookup { .. } => "lookup",
            Request::Present { .. } => "present",
            Request::Delete { .. } => "delete",
            Request::Seal { .. } => "seal",
        }
    }

    fn service(&self) -> &'static str {
        match self {
            Request::Store { service, .. }
            | Request::Lookup { service, .. }
            | Request::Present { service, .. }
            | Request::Delete { service, .. } => service.as_str(),
            Request::Seal { .. } => "",
        }
    }

    /// The bytes Python's `json.dumps(body).encode()` gives (`helper.py:42`), in a buffer sized
    /// up front so it never moves and is zeroed on drop.
    fn body(&self) -> Zeroizing<Vec<u8>> {
        let key_text = match self {
            Request::Seal { key, .. } => Zeroizing::new(py::b64encode(key)),
            _ => Zeroizing::new(String::new()),
        };
        let mut fields: Vec<(&str, &str)> = vec![("operation", self.operation())];
        match self {
            Request::Store {
                service,
                account,
                value,
            } => {
                fields.extend([
                    ("service", service.as_str()),
                    ("account", *account),
                    ("value", value.expose()),
                ]);
            }
            Request::Lookup { service, account }
            | Request::Present { service, account }
            | Request::Delete { service, account } => {
                fields.extend([("service", service.as_str()), ("account", *account)]);
            }
            Request::Seal {
                value,
                command,
                origin,
                ..
            } => {
                fields.extend([
                    ("key", key_text.as_str()),
                    ("value", value.expose()),
                    ("command", *command),
                    ("origin", *origin),
                ]);
            }
        }
        let bound: usize = fields
            .iter()
            .map(|(k, v)| py::string_bound(k) + py::string_bound(v) + 4)
            .sum();
        let mut out = Zeroizing::new(Vec::with_capacity(bound + 2));
        out.push(b'{');
        for (i, (key, value)) in fields.iter().enumerate() {
            if i > 0 {
                out.extend_from_slice(b", ");
            }
            py::string_into(key, &mut out);
            out.extend_from_slice(b": ");
            py::string_into(value, &mut out);
        }
        out.push(b'}');
        debug_assert!(out.len() <= bound + 2);
        out
    }
}

/// Never the account, the key or the value: only which operation on which service.
impl fmt::Debug for Request<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Request({} on {:?})", self.operation(), self.service())
    }
}

/// A successful reply: the value when the helper sent one (as a [`Secret`]), and its other fields
/// (`ok`, `present`, `deleted`, `nonce`, `ct`).
#[derive(Debug)]
pub struct Reply {
    pub value: Option<Secret>,
    pub fields: Map<String, Value>,
}

/// What is known about a helper failure (`helper.py:60-76`; decision: HAND-410), for the
/// daemon's journal (`secret.helper_failed`). Never the request, the value or the account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperFacts {
    pub operation: String,
    pub service: String,
    /// The Keychain's OSStatus, when the helper reports one (the pinned one doesn't).
    pub status: Option<i64>,
    /// Security's words for the status, spaces joined, at most 200 characters.
    pub message: Option<String>,
    /// For a failed lookup with no status: whether the helper's `present` check saw the item.
    pub found: Option<bool>,
}

impl HelperFacts {
    /// The dict Python hands `on_failure`, keys in Python's order.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("operation".into(), self.operation.clone().into());
        out.insert("service".into(), self.service.clone().into());
        if let Some(status) = self.status {
            out.insert("status".into(), status.into());
        }
        if let Some(message) = &self.message {
            out.insert("message".into(), message.clone().into());
        }
        if let Some(found) = self.found {
            out.insert("found".into(), found.into());
        }
        Value::Object(out)
    }
}

type FailureHook = Arc<dyn Fn(&HelperFacts) + Send + Sync>;

/// The helper of one FERB home.
#[derive(Clone)]
pub struct Helper {
    home: PathBuf,
    launcher: Arc<dyn Launcher>,
    on_failure: Option<FailureHook>,
}

impl fmt::Debug for Helper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Helper")
            .field("home", &self.home)
            .finish_non_exhaustive()
    }
}

/// The home Python uses when a caller names none (`helper.py:30`): `$FERB_HOME`, else `~/.ferb`.
pub fn default_home() -> PathBuf {
    match std::env::var_os("FERB_HOME") {
        Some(home) => PathBuf::from(home),
        None => std::env::home_dir().unwrap_or_default().join(".ferb"),
    }
}

impl Helper {
    /// The helper installed in `home` (FERB's data folder; `<home>/bin/ferb-secrets`), verified
    /// and run by `launcher`.
    pub fn new(home: impl Into<PathBuf>, launcher: Arc<dyn Launcher>) -> Self {
        Helper {
            home: home.into(),
            launcher,
            on_failure: None,
        }
    }

    /// Python's `helper.on_failure` (`helper.py:16-18`): told each failure's facts, from whichever
    /// thread ran the helper. A hook that panics never changes the answer (`helper.py:81-84`).
    pub fn on_failure(mut self, hook: impl Fn(&HelperFacts) + Send + Sync + 'static) -> Self {
        self.on_failure = Some(Arc::new(hook));
        self
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    /// `helper.py:41-47`: run the helper with the request on its stdin; whether it failed and its
    /// reply when that is JSON.
    fn run(&self, binary: &Path, body: &[u8]) -> io::Result<(bool, Option<Value>)> {
        let (failed, reply) = self.launcher.run(binary, body)?;
        Ok((failed, serde_json::from_slice::<Value>(&reply).ok()))
    }

    /// `helper.py:88-95`: verify, run, and turn the reply into a [`Reply`] or a refusal.
    pub fn request(&self, request: &Request<'_>) -> Result<Reply> {
        let binary = self.launcher.verified(&self.home)?;
        let (failed, out) = self.run(&binary, &request.body())?;
        let said_error =
            matches!(&out, Some(Value::Object(map)) if map.get("error").is_some_and(py::truthy));
        if failed || said_error {
            let failure = self.failure(&binary, request, out.as_ref());
            wipe(out);
            return Err(failure?);
        }
        match out {
            Some(Value::Object(mut fields)) => {
                let value = match fields.shift_remove("value") {
                    Some(Value::String(text)) => Some(Secret::new(text)),
                    Some(Value::Null) | None => None,
                    Some(other) => {
                        // Python would hand a non-text "value" on as it is; the helper only ever
                        // writes text there, so anything else is no valid result.
                        wipe(Some(other));
                        wipe(Some(Value::Object(fields)));
                        return Err(invalid_result());
                    }
                };
                Ok(Reply { value, fields })
            }
            other => {
                wipe(other);
                Err(invalid_result())
            }
        }
    }

    /// `helper.py:58-85`. `Ok` is the refusal to return; `Err` only when the `present` check
    /// could not be started at all.
    fn failure(&self, binary: &Path, request: &Request<'_>, out: Option<&Value>) -> Result<Error> {
        let mut facts = HelperFacts {
            operation: request.operation().into(),
            service: request.service().into(),
            status: None,
            message: None,
            found: None,
        };
        let why = if let Some((status, message)) = status(out) {
            facts.status = Some(status);
            let shown = if message.is_empty() {
                String::new()
            } else {
                format!(": {message}")
            };
            if !message.is_empty() {
                facts.message = Some(message);
            }
            format!("the Keychain refused (status {status}{shown})")
        } else {
            let checked = match request {
                Request::Lookup { service, account } if !account.is_empty() => {
                    let check = Request::Present {
                        service: *service,
                        account,
                    };
                    let (failed, seen) = self.run(binary, &check.body())?;
                    let found = match (&seen, failed) {
                        (Some(Value::Object(map)), false) => {
                            map.get("present").and_then(Value::as_bool)
                        }
                        _ => None,
                    };
                    wipe(seen);
                    found
                }
                _ => None,
            };
            facts.found = checked;
            match checked {
                Some(true) => "the Keychain has this item but didn't hand it to FERB (a Keychain prompt was \
                               refused or couldn't be shown); the helper doesn't report the Keychain's status"
                    .to_owned(),
                Some(false) => "the Keychain didn't show this item to FERB (it isn't saved there, or the \
                                keychain is locked)"
                    .to_owned(),
                None => "the secret operation failed and the helper gave no reason".to_owned(),
            }
        };
        if let Some(hook) = &self.on_failure {
            let _ = catch_unwind(AssertUnwindSafe(|| hook(&facts)));
        }
        Ok(Error::HelperFailed {
            message: why,
            facts,
        })
    }

    /// Save `value` as the item `account` of `service`, replacing what is there.
    pub fn store(&self, service: Service, account: &str, value: &Secret) -> Result<()> {
        self.request(&Request::Store {
            service,
            account,
            value,
        })
        .map(|_| ())
    }

    /// The item's value; `None` when the helper's reply carried none.
    pub fn lookup(&self, service: Service, account: &str) -> Result<Option<Secret>> {
        Ok(self.request(&Request::Lookup { service, account })?.value)
    }

    /// Whether the Keychain shows the item; no value is read and no prompt is shown.
    pub fn present(&self, service: Service, account: &str) -> Result<bool> {
        let reply = self.request(&Request::Present { service, account })?;
        Ok(reply.fields.get("present").is_some_and(py::truthy))
    }

    /// Delete the item without reading it; `false` when there was none (decision: GATE-2).
    pub fn delete(&self, service: Service, account: &str) -> Result<bool> {
        let reply = self.request(&Request::Delete { service, account })?;
        Ok(reply.fields.get("deleted").is_some_and(py::truthy))
    }

    /// `helper.py:98-101`: the sealed fill for one command at one origin.
    pub fn seal(
        &self,
        key: &[u8],
        value: &Secret,
        command: &str,
        origin: &str,
    ) -> Result<crate::seal::Sealed> {
        let reply = self.request(&Request::Seal {
            key,
            value,
            command,
            origin,
        })?;
        match (reply.fields.get("nonce"), reply.fields.get("ct")) {
            (Some(Value::String(nonce)), Some(Value::String(ct))) => Ok(crate::seal::Sealed {
                nonce: nonce.clone(),
                ct: ct.clone(),
            }),
            _ => Err(invalid_result()),
        }
    }
}

fn invalid_result() -> Error {
    Error::Refused("the secret helper returned an invalid result".into())
}

/// `helper.py:50-55`: the OSStatus and Security's words for it, when the reply carries them. A
/// status is a JSON integer (never `true`, never a fraction).
fn status(out: Option<&Value>) -> Option<(i64, String)> {
    let map = out?.as_object()?;
    let status = map.get("status")?.as_i64()?;
    let message = match map.get("message") {
        Some(Value::String(text)) => py::join_split(text).chars().take(200).collect(),
        _ => String::new(),
    };
    Some((status, message))
}

/// Zero every string of a reply that is being thrown away (a failed lookup's reply should hold
/// no value, but nothing here depends on that).
fn wipe(value: Option<Value>) {
    match value {
        Some(Value::String(mut text)) => text.zeroize(),
        Some(Value::Array(items)) => items.into_iter().for_each(|item| wipe(Some(item))),
        Some(Value::Object(map)) => map.into_iter().for_each(|(mut key, item)| {
            key.zeroize();
            wipe(Some(item));
        }),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_never_prints_its_account_or_value() {
        let value = Secret::from("invented canary");
        let request = Request::Store {
            service: Service::OwnerSecrets,
            account: "https://login.example · password",
            value: &value,
        };
        let shown = format!("{request:?}");
        assert!(
            !shown.contains("canary") && !shown.contains("login.example"),
            "{shown}"
        );
        let reply = Reply {
            value: Some(value),
            fields: Map::new(),
        };
        assert!(!format!("{reply:?}").contains("canary"));
    }
}
