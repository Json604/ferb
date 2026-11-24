//! FERB.app's sign-in to the owner's FERB account (APP-1128, PHN-830; the spec's "Daemon RPCs"): `account.get`,
//! `account.save`, `account.clear`, FERB.app's own (`app_only`; a phone's class is `never`). FERB.app signs in on
//! FERB's site and hands the daemon the account and its tokens; the daemon keeps them as ONE Keychain item (service
//! "ferb", account "ferb.account") written and read only through FERB's signed secret helper ([`KeyHelper`], as the
//! API keys are: GATE-543), and hands them back to FERB.app alone.
//!
//! - **Secrets** (rule 7): the tokens' values go to the redactor when saved and when first read, so they are masked in
//!   whatever FERB writes or sends; they are in no journal row, span, log or reply but `account.get`'s. The journal
//!   keeps structural facts only (`account.saved {provider_count, changed}`, `account.cleared {}`), never a token or an
//!   email.
//! - **The Mac's account** ([`Account::mac_account`]) is what the phone area matches a pairing against (PHN-828).
//! - **An account change** (PHN-828): saving an account removes every live phone bound to another account through
//!   `phone.remove`'s path (`by` "account"); the reply names them (`removed_phones`) so FERB.app says so once. A phone
//!   paired before accounts (none recorded) stays and binds itself when it signs in (`phone.bind_account`). Signing out
//!   (`account.clear`) removes nothing: signing back in to the same account keeps the phones.

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use ferb_identity::Secret;
use ferb_rpc::server::{HandlerError, HandlerResult};
use ferb_rpc::{Params, Server, Session};
use ferb_sanitize::redact::Redactor;
use serde_json::{json, Map, Value};

use crate::areas_b::core::refusal;
use crate::areas_b::runtimes::keys::{HelperError, KeyHelper};
use crate::phone::account::MacAccount;
use crate::phone::Phones;

/// The Keychain item's account (service "ferb").
pub const KEYCHAIN_ACCOUNT: &str = "ferb.account";

/// The longest values taken (the site's are far shorter).
const ID_MAX: usize = 64;
const EMAIL_MAX: usize = 320;
const NAME_MAX: usize = 200;
const TOKEN_MAX: usize = 512;
const PROVIDERS_MAX: usize = 8;
const PROVIDER_MAX: usize = 32;

/// A journal row.
pub type Note = dyn Fn(&str, Value) + Send + Sync;

/// What the account area takes from the daemon.
pub struct Seams {
    /// FERB's signed secret helper (service "ferb"); a test's fake Keychain.
    pub helper: Arc<dyn KeyHelper>,
    /// The tokens' values are registered with it (rule 7).
    pub redactor: Arc<Redactor>,
    pub journal: Arc<Note>,
}

/// The stored sign-in: `{"account": {...}, "tokens": {...}}`, exactly as FERB.app saved it after the checks.
#[derive(Clone)]
struct Stored(Map<String, Value>);

impl Stored {
    fn account(&self) -> &Map<String, Value> {
        self.0
            .get("account")
            .and_then(Value::as_object)
            .expect("checked when read")
    }

    fn tokens(&self) -> &Map<String, Value> {
        self.0
            .get("tokens")
            .and_then(Value::as_object)
            .expect("checked when read")
    }

    fn id(&self) -> &str {
        self.account()
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    fn mac_account(&self) -> MacAccount {
        MacAccount {
            id: self.id().to_string(),
            email: self
                .account()
                .get("email")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }
    }

    fn token_values(&self) -> Vec<&str> {
        ["access_token", "refresh_token"]
            .iter()
            .filter_map(|k| self.tokens().get(*k).and_then(Value::as_str))
            .collect()
    }
}

/// The account area. Cheap to clone.
#[derive(Clone)]
pub struct Account {
    inner: Arc<Inner>,
}

struct Inner {
    seams: Seams,
    /// What the Keychain holds, once read or written this run: `Some(None)` signed out. One lock over each read,
    /// write and delete, so a save and a read never cross.
    cache: Mutex<Option<Option<Stored>>>,
    /// The phone area, once ferbd put it in place (an account change removes phones of another account).
    phones: OnceLock<Phones>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn keychain_fault(e: HelperError) -> HandlerError {
    match e {
        HelperError::Refused(why) => refusal(format!(
            "FERB couldn't keep your sign-in in the Keychain ({why})"
        )),
        HelperError::Fault(f) => HandlerError::fault(&f.class, f.message),
    }
}

impl Account {
    pub fn new(seams: Seams) -> Account {
        Account {
            inner: Arc::new(Inner {
                seams,
                cache: Mutex::new(None),
                phones: OnceLock::new(),
            }),
        }
    }

    /// Where the paired phones are (set once by ferbd, after the phone area is built).
    pub fn set_phones(&self, phones: Phones) {
        let _ = self.inner.phones.set(phones);
    }

    /// Registers `account.get`, `account.save` and `account.clear` on `server`.
    pub fn register(&self, server: &Server) {
        let me = self.clone();
        server.register_unary("account.get", move |_s: Session, _p: Params| {
            let me = me.clone();
            async move {
                tokio::task::spawn_blocking(move || me.get())
                    .await
                    .map_err(join_fault)?
            }
        });
        let me = self.clone();
        server.register_unary("account.save", move |_s: Session, p: Params| {
            let me = me.clone();
            async move {
                tokio::task::spawn_blocking(move || me.save(&p))
                    .await
                    .map_err(join_fault)?
            }
        });
        let me = self.clone();
        server.register_unary("account.clear", move |_s: Session, _p: Params| {
            let me = me.clone();
            async move {
                tokio::task::spawn_blocking(move || me.clear())
                    .await
                    .map_err(join_fault)?
            }
        });
    }

    /// The Mac's FERB account now (PHN-828), read from the Keychain the first time; `None` while signed out or when
    /// the Keychain can't be read now (tried again next time). Blocking: call it off the async runtime.
    pub fn mac_account(&self) -> Option<MacAccount> {
        let mut cache = lock(&self.inner.cache);
        self.load(&mut cache)
            .ok()
            .flatten()
            .map(|s| s.mac_account())
    }

    /// What the Keychain holds, read once per run; the tokens go to the redactor on that first read.
    fn load(&self, cache: &mut Option<Option<Stored>>) -> Result<Option<Stored>, HandlerError> {
        if let Some(held) = cache.as_ref() {
            return Ok(held.clone());
        }
        let found = match self.inner.seams.helper.lookup(KEYCHAIN_ACCOUNT) {
            Ok(v) => v,
            // the helper's "no such item"
            Err(HelperError::Refused(_)) => None,
            Err(e) => return Err(keychain_fault(e)),
        };
        let stored = found
            .and_then(|s| serde_json::from_str::<Value>(s.expose()).ok())
            .and_then(|v| match v {
                Value::Object(m) => checked(&m).ok(),
                _ => None,
            });
        if let Some(s) = &stored {
            for t in s.token_values() {
                self.inner.seams.redactor.add_secret_value(t);
            }
        }
        *cache = Some(stored.clone());
        Ok(stored)
    }

    /// `account.get {}`: `{signed_in, account, tokens}` (FERB.app's alone: the only reply that carries the tokens).
    pub fn get(&self) -> HandlerResult {
        let mut cache = lock(&self.inner.cache);
        let stored = self.load(&mut cache)?;
        let mut out = Params::new();
        out.insert("signed_in".into(), json!(stored.is_some()));
        out.insert(
            "account".into(),
            stored
                .as_ref()
                .map_or(Value::Null, |s| Value::Object(s.account().clone())),
        );
        out.insert(
            "tokens".into(),
            stored
                .as_ref()
                .map_or(Value::Null, |s| Value::Object(s.tokens().clone())),
        );
        Ok(out)
    }

    /// `account.save {account, tokens}`: kept in the Keychain and read back; the tokens go to the redactor first. An
    /// account other than the one the live phones paired under removes those phones (PHN-828).
    pub fn save(&self, params: &Params) -> HandlerResult {
        let stored = checked(params).map_err(refusal)?;
        for t in stored.token_values() {
            self.inner.seams.redactor.add_secret_value(t);
        }
        let mut cache = lock(&self.inner.cache);
        // what was there before, to say whether the account changed (a Keychain that can't be read: changed)
        let before = self.load(&mut cache).ok().flatten();
        let changed = before.as_ref().map(Stored::id) != Some(stored.id());
        let text = Secret::new(Value::Object(stored.0.clone()).to_string());
        self.inner
            .seams
            .helper
            .store(KEYCHAIN_ACCOUNT, &text)
            .map_err(keychain_fault)?;
        let back = self
            .inner
            .seams
            .helper
            .lookup(KEYCHAIN_ACCOUNT)
            .map_err(keychain_fault)?;
        if back.as_ref().map(Secret::expose) != Some(text.expose()) {
            // what the Keychain holds now is read again next time
            *cache = None;
            return Err(refusal(
                "FERB couldn't keep your sign-in in the Keychain (it didn't read back the same)",
            ));
        }
        *cache = Some(Some(stored.clone()));
        drop(cache);
        let providers = stored
            .account()
            .get("providers")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        (self.inner.seams.journal)(
            "account.saved",
            json!({"provider_count": providers, "changed": changed}),
        );
        // decision: PHN-828 — the phones bound to another account go; this account's and unbound ones stay
        let removed = match self.inner.phones.get() {
            Some(phones) => phones.account_changed(stored.id())?,
            None => Vec::new(),
        };
        let mut out = Params::new();
        out.insert("saved".into(), json!(true));
        out.insert(
            "removed_phones".into(),
            Value::Array(
                removed
                    .into_iter()
                    .map(|(id, name)| json!({"id": id, "name": name}))
                    .collect(),
            ),
        );
        Ok(out)
    }

    /// `account.clear {}`: the Keychain item goes (none there is fine). No phone is removed.
    pub fn clear(&self) -> HandlerResult {
        let mut cache = lock(&self.inner.cache);
        match self.inner.seams.helper.delete(KEYCHAIN_ACCOUNT) {
            Ok(_) | Err(HelperError::Refused(_)) => {}
            Err(e) => {
                *cache = None;
                return Err(keychain_fault(e));
            }
        }
        let gone = match self.inner.seams.helper.lookup(KEYCHAIN_ACCOUNT) {
            Ok(v) => v.is_none(),
            Err(HelperError::Refused(_)) => true,
            Err(e) => {
                *cache = None;
                return Err(keychain_fault(e));
            }
        };
        if !gone {
            *cache = None;
            return Err(refusal(
                "FERB couldn't remove your sign-in from the Keychain",
            ));
        }
        *cache = Some(None);
        drop(cache);
        (self.inner.seams.journal)("account.cleared", json!({}));
        let mut out = Params::new();
        out.insert("cleared".into(), json!(true));
        Ok(out)
    }
}

fn join_fault(e: tokio::task::JoinError) -> HandlerError {
    HandlerError::fault("RuntimeError", e.to_string())
}

/// The sign-in as `account.save` takes it, checked: `{account: {id, email, name?, providers?}, tokens: {access_token,
/// access_expires_at?, refresh_token}}`; only these keys are kept. `Err` with a plain refusal's words.
fn checked(params: &Map<String, Value>) -> Result<Stored, String> {
    let account = params
        .get("account")
        .and_then(Value::as_object)
        .ok_or("account.save needs the account")?;
    let tokens = params
        .get("tokens")
        .and_then(Value::as_object)
        .ok_or("account.save needs the tokens")?;
    let text =
        |m: &Map<String, Value>, key: &str, max: usize, what: &str| -> Result<String, String> {
            match m.get(key) {
                Some(Value::String(s)) if !s.trim().is_empty() && s.len() <= max => Ok(s.clone()),
                _ => Err(format!("{what} must be text of at most {max} characters")),
            }
        };
    let id = text(account, "id", ID_MAX, "the account's id")?;
    let email = text(account, "email", EMAIL_MAX, "the account's email")?;
    let name = match account.get("name") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::String(s)) if s.len() <= NAME_MAX => Value::String(s.clone()),
        _ => {
            return Err(format!(
                "the account's name must be text of at most {NAME_MAX} characters"
            ))
        }
    };
    let providers = match account.get("providers") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a))
            if a.len() <= PROVIDERS_MAX
                && a.iter().all(|p| {
                    p.as_str()
                        .is_some_and(|p| !p.is_empty() && p.len() <= PROVIDER_MAX)
                }) =>
        {
            a.clone()
        }
        _ => return Err("the account's providers must be a short list of names".into()),
    };
    let access = text(tokens, "access_token", TOKEN_MAX, "the access token")?;
    let refresh = text(tokens, "refresh_token", TOKEN_MAX, "the refresh token")?;
    let expires = match tokens.get("access_expires_at") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::Number(n)) => Value::Number(n.clone()),
        Some(Value::String(s)) if s.len() <= 64 => Value::String(s.clone()),
        _ => return Err("the access token's expiry must be a time".into()),
    };
    let mut a = Map::new();
    a.insert("id".into(), Value::String(id));
    a.insert("email".into(), Value::String(email));
    a.insert("name".into(), name);
    a.insert("providers".into(), Value::Array(providers));
    let mut t = Map::new();
    t.insert("access_token".into(), Value::String(access));
    t.insert("access_expires_at".into(), expires);
    t.insert("refresh_token".into(), Value::String(refresh));
    let mut out = Map::new();
    out.insert("account".into(), Value::Object(a));
    out.insert("tokens".into(), Value::Object(t));
    Ok(Stored(out))
}
