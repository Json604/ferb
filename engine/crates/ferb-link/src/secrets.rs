//! What the Mac side keeps secret: its link identity key, the own relay's token, and the key that seals the state file
//! (kb PHN-143, PHN-124).
//!
//! In FERB the helper never touches the Keychain (kb PHN-143, S4): the three are one Keychain item that only FERB's
//! daemon reads, through the pinned secrets helper `ferb-secrets` (service `ferb`, account `phone-link`), whose stable
//! signature is what the item's access is tied to, so no install or update asks. The daemon hands them to this program
//! once, at its start, on its standard input (never argv or the environment): that is [`Keys::Given`]. A change made
//! here (a new relay token) is kept in memory; the daemon, which set it, writes the item itself.
//!
//! `Files` is the lab's 0600-files form, for tests and the lab's tools (`ferb-link phone`): any program running as the
//! owner can read those.

use crate::errors::{self, LinkError, LinkResult};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The names the one item holds.
pub const IDENTITY: &str = "identity";
pub const STATE_KEY: &str = "state-key";
pub const RELAY_TOKEN: &str = "relay-token";

#[derive(Clone, PartialEq)]
pub enum Keys {
    /// Handed over by FERB's daemon at the start (name -> bytes); the daemon holds them in the Keychain.
    Given(BTreeMap<String, Vec<u8>>),
    /// 0600 files in the state folder.
    Files,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // never a key's bytes (rule 7)
        match self {
            Keys::Given(m) => write!(f, "Given({:?})", m.keys().collect::<Vec<_>>()),
            Keys::Files => f.write_str("Files"),
        }
    }
}

pub struct Secrets {
    kind: Keys,
    dir: PathBuf,
    /// The given keys as they are now (a relay token set here changes them).
    given: Mutex<BTreeMap<String, Vec<u8>>>,
}

fn err(what: &str, e: impl std::fmt::Display) -> LinkError {
    LinkError::new(errors::FERB_ERROR, format!("{what}: {e}"))
}

impl Secrets {
    pub fn new(kind: Keys, state_dir: &Path) -> Self {
        let given = match &kind {
            Keys::Given(m) => m.clone(),
            Keys::Files => BTreeMap::new(),
        };
        Self {
            kind,
            dir: state_dir.to_path_buf(),
            given: Mutex::new(given),
        }
    }
    /// Whether the daemon provides the keys (they are never made here then).
    pub fn given(&self) -> bool {
        matches!(self.kind, Keys::Given(_))
    }
    pub fn words(&self) -> &'static str {
        match self.kind {
            Keys::Given(_) => "the Keychain, through FERB",
            Keys::Files => "files in the state folder (any program of yours can read them)",
        }
    }
    fn file(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.key"))
    }

    pub fn get(&self, name: &str) -> LinkResult<Option<Vec<u8>>> {
        match &self.kind {
            Keys::Files => match std::fs::read(self.file(name)) {
                Ok(b) => Ok(Some(b)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(err(name, e)),
            },
            Keys::Given(_) => Ok(self.given.lock().unwrap().get(name).cloned()),
        }
    }
    pub fn set(&self, name: &str, value: &[u8]) -> LinkResult<()> {
        match &self.kind {
            Keys::Files => {
                crate::store::write_private(&self.file(name), value).map_err(|e| err(name, e))
            }
            Keys::Given(_) => {
                self.given
                    .lock()
                    .unwrap()
                    .insert(name.to_string(), value.to_vec());
                Ok(())
            }
        }
    }
    pub fn delete(&self, name: &str) -> LinkResult<()> {
        match &self.kind {
            Keys::Files => match std::fs::remove_file(self.file(name)) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(err(name, e)),
            },
            Keys::Given(_) => {
                self.given.lock().unwrap().remove(name);
                Ok(())
            }
        }
    }
}

/// The keys as the daemon hands them over: one JSON line `{"keys": {name: base64}}` (the first line of standard
/// input). Unknown names are kept; a value that is not base64 is refused.
pub fn parse_given(line: &str) -> LinkResult<BTreeMap<String, Vec<u8>>> {
    use base64::Engine;
    let v: serde_json::Value = serde_json::from_str(line.trim())
        .map_err(|_| err("the keys FERB handed over", "not JSON"))?;
    let Some(map) = v.get("keys").and_then(serde_json::Value::as_object) else {
        return Err(err("the keys FERB handed over", "no keys"));
    };
    let mut out = BTreeMap::new();
    for (k, val) in map {
        let bytes = val
            .as_str()
            .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
            .ok_or_else(|| err("the keys FERB handed over", format!("{k} is not base64")))?;
        out.insert(k.clone(), bytes);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keys arrive only as handed over; a change stays in memory; nothing is written beside the state.
    // decision: PHN-487
    // decision: PHN-143
    #[test]
    fn given_keys_stay_in_memory_and_never_print() {
        let dir = std::env::temp_dir().join(format!("ferb-link-secrets-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let given = parse_given(r#"{"keys": {"identity": "AAEC", "state-key": "AwQF"}}"#).unwrap();
        assert_eq!(given["identity"], vec![0, 1, 2]);
        let s = Secrets::new(Keys::Given(given), &dir);
        assert!(s.given());
        assert_eq!(s.get(STATE_KEY).unwrap(), Some(vec![3, 4, 5]));
        s.set(RELAY_TOKEN, b"invented-token").unwrap();
        assert_eq!(
            s.get(RELAY_TOKEN).unwrap().as_deref(),
            Some(&b"invented-token"[..])
        );
        s.delete(RELAY_TOKEN).unwrap();
        assert_eq!(s.get(RELAY_TOKEN).unwrap(), None);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let shown = format!(
            "{:?}",
            Keys::Given(parse_given(r#"{"keys": {"identity": "AAEC"}}"#).unwrap())
        );
        assert!(
            !shown.contains("AAEC") && !shown.contains("[0, 1, 2]"),
            "{shown}"
        );
        assert!(parse_given("{}").is_err() && parse_given(r#"{"keys": {"a": "!"}}"#).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
