//! Authenticated extension-to-profile bindings, single-use pairing tokens and anti-replay state: a
//! port of `ferb/act/browser/pairing.py`. It must not choose a browser profile or account: the
//! caller (Pair all / Pair, a task's browser_pair after the owner's Yes) supplies the browser, the
//! folder and the identity.
//!
//! Bindings and the extension-to-daemon counter (`last_seq`) persist in the store's `ext_installs`
//! table, written as Python writes it, so either daemon reads the other's pairings. The
//! daemon-to-extension counter is per connection and never stored (`bridge::Conn`). Pairing keys
//! never enter a result a model sees, a log or an error.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use ferb_core::interfaces::Clock;
use ferb_core::types::Identity;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::protocol::{authentic, ExtMessage, ExtType, NonAscii};

/// `TOKEN_TTL_S`: a pairing token lives five minutes.
pub const TOKEN_TTL_S: f64 = 300.0;

/// A pairing key: zeroed when dropped, never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingKey(Zeroizing<Vec<u8>>);

impl PairingKey {
    pub fn new(bytes: Vec<u8>) -> PairingKey {
        PairingKey(Zeroizing::new(bytes))
    }

    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for PairingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<pairing key>")
    }
}

/// Random bytes (Python's `secrets.token_bytes`); injected so a run can be replayed.
pub trait Random: Send + Sync {
    fn bytes(&self, n: usize) -> Vec<u8>;
}

/// The system's random source.
pub struct SystemRandom;

impl Random for SystemRandom {
    fn bytes(&self, n: usize) -> Vec<u8> {
        let mut out = vec![0u8; n];
        getrandom::fill(&mut out).expect("the system gives random bytes");
        out
    }
}

/// `secrets.token_hex(n)`.
pub fn token_hex(random: &dyn Random, n: usize) -> String {
    random.bytes(n).iter().map(|b| format!("{b:02x}")).collect()
}

/// `secrets.token_urlsafe(n)`.
pub fn token_urlsafe(random: &dyn Random, n: usize) -> String {
    ferb_identity::py::b64encode_urlsafe_nopad(&random.bytes(n))
}

/// A store call failed (Python lets the database's exception through).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("pairing store: {0}")]
pub struct StoreError(pub String);

/// One `ext_installs` row's facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallRow {
    pub browser: String,
    pub folder: String,
    pub identity: Identity,
}

/// The `ext_installs` table (`pairing.py`'s SQL), behind a trait so the engine stays portable; the
/// Mac's SQLite store implements it in [`sqlite`].
pub trait InstallStore: Send + Sync {
    fn row(&self, install_id: &str) -> Result<Option<InstallRow>, StoreError>;
    /// `SELECT install_id ... WHERE browser = ? AND folder = ?` (the first row SQLite returns).
    fn install_for(&self, browser: &str, folder: &str) -> Result<Option<String>, StoreError>;
    /// Every install id, `ORDER BY browser, folder`.
    fn installs(&self) -> Result<Vec<String>, StoreError>;
    fn key(&self, install_id: &str) -> Result<Option<PairingKey>, StoreError>;
    fn key_and_seq(&self, install_id: &str) -> Result<Option<(PairingKey, i64)>, StoreError>;
    /// `ext_version` ("" stored as "", NULL as "").
    fn version(&self, install_id: &str) -> Result<Option<String>, StoreError>;
    fn set_version(&self, install_id: &str, version: &str) -> Result<(), StoreError>;
    fn set_last_seq(&self, install_id: &str, seq: i64) -> Result<(), StoreError>;
    /// Deletes the profile's row; whether there was one.
    fn unpair(&self, browser: &str, folder: &str) -> Result<bool, StoreError>;
    /// All or nothing: the rows of this profile or this install go, the new one comes in with
    /// `last_seq` 0 and `ext_version` ''.
    fn pair(
        &self,
        install_id: &str,
        row: &InstallRow,
        key: &PairingKey,
        paired_at: &str,
    ) -> Result<(), StoreError>;
}

/// `Binding`: who an install is.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Binding {
    pub install_id: String,
    pub browser: String,
    pub folder: String,
    pub identity: Identity,
}

/// The `paired` reply sent back on the pairing connection: it holds the new key.
#[derive(Clone, PartialEq, Eq)]
pub struct PairedReply {
    key: PairingKey,
}

impl PairedReply {
    /// `json.dumps(reply)`.
    pub fn wire(&self) -> String {
        ferb_py::json::dumps(
            &json!({"type": "paired", "key": ferb_identity::py::b64encode(self.key.expose())}),
        )
    }

    pub fn key(&self) -> &PairingKey {
        &self.key
    }
}

impl fmt::Debug for PairedReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairedReply { key: <pairing key> }")
    }
}

/// `Verdict`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub accepted: bool,
    pub reason: String,
    /// Sent back on this connection.
    pub reply: Option<PairedReply>,
    /// Who the message is attributed to.
    pub binding: Option<Binding>,
}

impl Verdict {
    fn no(reason: &str) -> Verdict {
        Verdict {
            accepted: false,
            reason: reason.into(),
            reply: None,
            binding: None,
        }
    }

    fn yes(reason: &str, binding: Option<Binding>) -> Verdict {
        Verdict {
            accepted: true,
            reason: reason.into(),
            reply: None,
            binding,
        }
    }
}

/// Why `handle` gave no verdict: the store failed, or the MAC wasn't ASCII (Python's `TypeError`
/// from `hmac.compare_digest`). Either ends the connection, as in Python.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandleError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("a message's MAC wasn't ASCII")]
    NonAscii,
}

impl From<NonAscii> for HandleError {
    fn from(_: NonAscii) -> Self {
        HandleError::NonAscii
    }
}

struct Token {
    browser: String,
    folder: String,
    identity: Identity,
    expires: f64,
}

/// `Pairings`.
pub struct Pairings {
    store: Arc<dyn InstallStore>,
    clock: Arc<dyn Clock>,
    random: Arc<dyn Random>,
    tokens: Mutex<HashMap<String, Token>>,
    /// Tokens that completed a pairing.
    paired: Mutex<HashSet<String>>,
}

impl Pairings {
    pub fn new(
        store: Arc<dyn InstallStore>,
        clock: Arc<dyn Clock>,
        random: Arc<dyn Random>,
    ) -> Pairings {
        Pairings {
            store,
            clock,
            random,
            tokens: Mutex::default(),
            paired: Mutex::default(),
        }
    }

    fn tokens(&self) -> std::sync::MutexGuard<'_, HashMap<String, Token>> {
        self.tokens.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn issue_token(&self, browser: &str, folder: &str, identity: Identity) -> String {
        let token = token_urlsafe(self.random.as_ref(), 24);
        let expires = self.clock.monotonic() + TOKEN_TTL_S;
        self.tokens().insert(
            token.clone(),
            Token {
                browser: browser.into(),
                folder: folder.into(),
                identity,
                expires,
            },
        );
        token
    }

    pub fn binding(&self, install_id: &str) -> Result<Option<Binding>, StoreError> {
        Ok(self.store.row(install_id)?.map(|r| Binding {
            install_id: install_id.into(),
            browser: r.browser,
            folder: r.folder,
            identity: r.identity,
        }))
    }

    pub fn for_profile(&self, browser: &str, folder: &str) -> Result<Option<Binding>, StoreError> {
        match self.store.install_for(browser, folder)? {
            Some(id) => self.binding(&id),
            None => Ok(None),
        }
    }

    pub fn all(&self) -> Result<Vec<Binding>, StoreError> {
        let mut out = vec![];
        for id in self.store.installs()? {
            if let Some(b) = self.binding(&id)? {
                out.push(b);
            }
        }
        Ok(out)
    }

    /// Daemon-internal (sealing). Never returned by any socket method, never logged.
    pub fn key(&self, install_id: &str) -> Result<Option<PairingKey>, StoreError> {
        self.store.key(install_id)
    }

    /// The extension version this install last reported in an authenticated hello ("" before any).
    pub fn version(&self, install_id: &str) -> Result<String, StoreError> {
        Ok(self.store.version(install_id)?.unwrap_or_default())
    }

    /// Kept from every authenticated hello, accepted or refused (decision: HAND-406). At most 32
    /// characters.
    pub fn record_version(&self, install_id: &str, version: &str) -> Result<(), StoreError> {
        self.store
            .set_version(install_id, ferb_py::value::prefix(version, 32))
    }

    /// The owner unpaired the profile.
    pub fn unpair(&self, browser: &str, folder: &str) -> Result<bool, StoreError> {
        self.store.unpair(browser, folder)
    }

    /// A pairing of the profile is in progress: an unexpired token for it is still unused.
    pub fn pending(&self, browser: &str, folder: &str) -> bool {
        let now = self.clock.monotonic();
        self.tokens()
            .values()
            .any(|t| t.browser == browser && t.folder == folder && t.expires >= now)
    }

    /// A pairing wait ended: its token can no longer pair (used or not).
    pub fn revoke(&self, token: &str) {
        self.tokens().remove(token);
    }

    /// Tokens that completed a pairing.
    pub fn paired_tokens(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .paired
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect();
        out.sort();
        out
    }

    pub fn was_paired(&self, token: &str) -> bool {
        self.paired
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(token)
    }

    pub fn handle(&self, m: &ExtMessage) -> Result<Verdict, HandleError> {
        if m.mtype == ExtType::Pair {
            return Ok(self.pair(m)?);
        }
        let Some((key, last_seq)) = self.store.key_and_seq(&m.install_id)? else {
            if m.mtype == ExtType::Hello {
                return Ok(Verdict::yes("unpaired_hello", None));
            }
            return Ok(Verdict::no("unpaired_install"));
        };
        if !authentic(key.expose(), m)? {
            return Ok(Verdict::no("bad_mac"));
        }
        let seq = m.seq.expect("an authentic message has a seq");
        if seq <= last_seq {
            return Ok(Verdict::no("replay"));
        }
        self.store.set_last_seq(&m.install_id, seq)?;
        Ok(Verdict::yes("ok", self.binding(&m.install_id)?))
    }

    fn pair(&self, m: &ExtMessage) -> Result<Verdict, StoreError> {
        let Ok(body) = m.body() else {
            return Ok(Verdict::no("malformed"));
        };
        let entry = match body.get("token") {
            Some(Value::String(token)) => self.tokens().remove(token).map(|t| (token.clone(), t)),
            _ => None,
        };
        let Some((token, entry)) = entry else {
            return Ok(Verdict::no("unknown_token"));
        };
        if self.clock.monotonic() > entry.expires {
            return Ok(Verdict::no("expired_token"));
        }
        let key = PairingKey::new(self.random.bytes(32));
        let row = InstallRow {
            browser: entry.browser,
            folder: entry.folder,
            identity: entry.identity,
        };
        let paired_at = ferb_core::when::isoformat(&self.clock.now());
        self.store.pair(&m.install_id, &row, &key, &paired_at)?;
        self.paired
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(token);
        Ok(Verdict {
            accepted: true,
            reason: "paired".into(),
            reply: Some(PairedReply { key }),
            binding: Some(Binding {
                install_id: m.install_id.clone(),
                browser: row.browser,
                folder: row.folder,
                identity: row.identity,
            }),
        })
    }
}

/// The Mac's SQLite store's `ext_installs` (the same SQL as `pairing.py`).
#[cfg(target_os = "macos")]
pub mod sqlite {
    use std::sync::{Mutex, PoisonError};

    use ferb_core::types::Identity;
    use rusqlite::{params, Connection, OptionalExtension};

    use super::{InstallRow, InstallStore, PairingKey, StoreError};

    /// One connection to FERB's store, shared under a lock.
    pub struct SqliteInstalls {
        conn: Mutex<Connection>,
    }

    impl SqliteInstalls {
        pub fn new(conn: Connection) -> SqliteInstalls {
            SqliteInstalls {
                conn: Mutex::new(conn),
            }
        }

        fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
            self.conn.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    fn err(e: rusqlite::Error) -> StoreError {
        StoreError(e.to_string())
    }

    fn identity(text: &str) -> Result<Identity, StoreError> {
        Identity::ALL
            .iter()
            .copied()
            .find(|i| i.as_str() == text)
            .ok_or_else(|| StoreError(format!("{text:?} is not a valid Identity")))
    }

    impl InstallStore for SqliteInstalls {
        fn row(&self, install_id: &str) -> Result<Option<InstallRow>, StoreError> {
            let found = self
                .conn()
                .query_row(
                    "SELECT browser, folder, identity FROM ext_installs WHERE install_id = ?",
                    [install_id],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(err)?;
            found
                .map(|(browser, folder, who)| {
                    Ok(InstallRow {
                        browser,
                        folder,
                        identity: identity(&who)?,
                    })
                })
                .transpose()
        }

        fn install_for(&self, browser: &str, folder: &str) -> Result<Option<String>, StoreError> {
            self.conn()
                .query_row(
                    "SELECT install_id FROM ext_installs WHERE browser = ? AND folder = ?",
                    [browser, folder],
                    |r| r.get(0),
                )
                .optional()
                .map_err(err)
        }

        fn installs(&self) -> Result<Vec<String>, StoreError> {
            let conn = self.conn();
            let mut st = conn
                .prepare("SELECT install_id FROM ext_installs ORDER BY browser, folder")
                .map_err(err)?;
            let ids = st.query_map([], |r| r.get(0)).map_err(err)?;
            ids.collect::<Result<_, _>>().map_err(err)
        }

        fn key(&self, install_id: &str) -> Result<Option<PairingKey>, StoreError> {
            self.conn()
                .query_row(
                    "SELECT key FROM ext_installs WHERE install_id = ?",
                    [install_id],
                    |r| r.get::<_, Vec<u8>>(0),
                )
                .optional()
                .map(|k| k.map(PairingKey::new))
                .map_err(err)
        }

        fn key_and_seq(&self, install_id: &str) -> Result<Option<(PairingKey, i64)>, StoreError> {
            self.conn()
                .query_row(
                    "SELECT key, last_seq FROM ext_installs WHERE install_id = ?",
                    [install_id],
                    |r| {
                        Ok((
                            PairingKey::new(r.get::<_, Vec<u8>>(0)?),
                            r.get::<_, i64>(1)?,
                        ))
                    },
                )
                .optional()
                .map_err(err)
        }

        fn version(&self, install_id: &str) -> Result<Option<String>, StoreError> {
            self.conn()
                .query_row(
                    "SELECT ext_version FROM ext_installs WHERE install_id = ?",
                    [install_id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()
                .map(|v| v.map(Option::unwrap_or_default))
                .map_err(err)
        }

        fn set_version(&self, install_id: &str, version: &str) -> Result<(), StoreError> {
            self.conn()
                .execute(
                    "UPDATE ext_installs SET ext_version = ? WHERE install_id = ?",
                    [version, install_id],
                )
                .map(|_| ())
                .map_err(err)
        }

        fn set_last_seq(&self, install_id: &str, seq: i64) -> Result<(), StoreError> {
            self.conn()
                .execute(
                    "UPDATE ext_installs SET last_seq = ? WHERE install_id = ?",
                    params![seq, install_id],
                )
                .map(|_| ())
                .map_err(err)
        }

        fn unpair(&self, browser: &str, folder: &str) -> Result<bool, StoreError> {
            self.conn()
                .execute(
                    "DELETE FROM ext_installs WHERE browser = ? AND folder = ?",
                    [browser, folder],
                )
                .map(|n| n > 0)
                .map_err(err)
        }

        fn pair(
            &self,
            install_id: &str,
            row: &InstallRow,
            key: &PairingKey,
            paired_at: &str,
        ) -> Result<(), StoreError> {
            let conn = self.conn();
            conn.execute_batch("BEGIN").map_err(err)?;
            let done = conn
                .execute(
                    "DELETE FROM ext_installs WHERE (browser = ? AND folder = ?) OR install_id = ?",
                    [&row.browser, &row.folder, install_id],
                )
                .and_then(|_| {
                    conn.execute(
                        "INSERT INTO ext_installs (install_id, browser, folder, identity, key, last_seq, paired_at) \
                         VALUES (?, ?, ?, ?, ?, 0, ?)",
                        params![install_id, row.browser, row.folder, row.identity.as_str(), key.expose(), paired_at],
                    )
                })
                .and_then(|_| conn.execute_batch("COMMIT"));
            if let Err(e) = done {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(err(e));
            }
            Ok(())
        }
    }
}
