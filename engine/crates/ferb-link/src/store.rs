//! The Mac's link state: the own-relay setting, the limits, which relay setting each paired phone was seen with, the
//! day's file counter and the link's own audit. One folder (0700), files 0600, every write atomic. Nothing here is
//! reachable from the phone's method list (kb SEC-6); only the owner's side changes it.
//!
//! The paired phones are not here: since S4 (kb T-1204) they are the daemon's own table (kb PHN-469), read by
//! `link.devices` and followed on `link.events`. What is secret (the identity key, the relay's token) is never in a
//! file (`secrets`). What is left in `link.json` is sealed: a keyed hash under a key that is secret too. A file that
//! something else changed does not match its seal: with keys a person keeps in files the link refuses it until the
//! owner says the change is his (kb SEC-24); in FERB (keys handed over by the daemon) it is set aside as
//! `link.json.changed` and the link starts from the defaults, so nothing another program wrote is used and the
//! phones still connect (the own relay is set again in Settings → Phone).

use crate::errors::{self, LinkError, LinkResult};
use crate::limits::Limits;
use crate::net::RelaySpec;
use crate::secrets::{Keys, Secrets};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use iroh::SecretKey;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Saved {
    /// Per paired phone (its endpoint id): the own-relay setting it is known to have (`RelaySpec::mark`), set when it
    /// was seen connected through that relay. While a paired phone's differs from the Mac's setting, the Mac stays
    /// findable on the shared relays too (kb NET-20). The paired phones themselves are the daemon's (kb PHN-469).
    #[serde(default)]
    relay_marks: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    own_relay: Option<RelaySpec>,
    /// PHN-591: when the relay setting last changed (unix seconds): the shared relays' second endpoint closes at most
    /// `BEACON_MAX_DAYS` after it. Absent in an older file (its bytes, and so its seal, unchanged).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    relay_set_at: Option<u64>,
    #[serde(default)]
    limits: Limits,
    /// (day number, bytes relayed through the shared relay that day)
    #[serde(default)]
    shared_day: (u64, u64),
    /// (day number, files fetched from the Mac that day, their bytes)
    #[serde(default)]
    file_day: (u64, u64, u64),
    /// The keyed hash of everything above, under the state key (in the Keychain).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    seal: String,
}

pub struct Store {
    dir: PathBuf,
    saved: Mutex<Saved>,
    secrets: Secrets,
    state_key: [u8; 32],
}

/// What the seal covers: the state as it is written, as compact JSON without the `seal` member (and never the
/// relay's token: that is in the Keychain, the file does not hold it).
fn sealed_body(s: &Saved) -> Vec<u8> {
    let mut plain = s.clone();
    plain.seal = String::new();
    if let Some(r) = plain.own_relay.as_mut() {
        r.token = None;
    }
    serde_json::to_vec(&plain).unwrap_or_default()
}
fn seal_of(key: &[u8; 32], body: &[u8]) -> String {
    B64.encode(blake3::keyed_hash(key, body).as_bytes())
}
/// The same two things read back from a file's own text: what was sealed, and the seal. The text is taken as it
/// stands on disk (only the spaces and line breaks between its parts are dropped, which say nothing), never
/// read into this build's idea of the state and written out again: a build that knows a setting more, or one
/// fewer, than the build that wrote the file would otherwise see a file nobody touched as changed. The seal is
/// the last member, as every build writes it. None: the text has no seal in that place.
fn sealed_parts(file: &[u8]) -> Option<(Vec<u8>, String)> {
    let mut compact = Vec::with_capacity(file.len());
    let (mut in_string, mut escaped) = (false, false);
    for &b in file {
        if in_string {
            compact.push(b);
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
        } else if !matches!(b, b' ' | b'\n' | b'\r' | b'\t') {
            in_string = b == b'"';
            compact.push(b);
        }
    }
    const MEMBER: &[u8] = b"\"seal\":\"";
    let rest = compact.strip_suffix(b"\"}")?;
    let at = rest.windows(MEMBER.len()).rposition(|w| w == MEMBER)?;
    let seal = &rest[at + MEMBER.len()..];
    if seal.is_empty()
        || !seal
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return None;
    }
    let mut body = match rest[..at].split_last()? {
        (b',', before) => before.to_vec(),
        (b'{', []) => b"{".to_vec(),
        _ => return None,
    };
    body.push(b'}');
    Some((body, String::from_utf8(seal.to_vec()).ok()?))
}

fn io(what: &str, e: std::io::Error) -> LinkError {
    LinkError::new(errors::FERB_ERROR, format!("{what}: {e}"))
}

pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

impl Store {
    /// `accept_changed`: the owner says, at the Mac, that a state file changed outside the link is his: it is taken
    /// as it is and sealed again.
    pub fn open(dir: &Path, keys: Keys, accept_changed: bool) -> LinkResult<Self> {
        fs::create_dir_all(dir).map_err(|e| io("state folder", e))?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .map_err(|e| io("state folder", e))?;
        let secrets = Secrets::new(keys, dir);
        let file = dir.join("link.json");
        let on_disk = match fs::read(&file) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(io("link.json", e)),
        };
        let mut saved: Option<Saved> = match &on_disk {
            Some(b) => Some(serde_json::from_slice(b).map_err(|e| {
                LinkError::new(
                    errors::FERB_ERROR,
                    format!(
                        "{} cannot be read ({e}); it was left untouched",
                        file.display()
                    ),
                )
            })?),
            None => None,
        };
        let had_key = secrets
            .get("state-key")?
            .and_then(|b| <[u8; 32]>::try_from(b).ok());
        let changed = |why: &str| {
            LinkError::new(errors::FERB_ERROR, format!("{} {why}. FERB's phone link did not write it that way, so it was not loaded: another program could have chosen a relay or a limit by editing it. Nothing was removed.", file.display()))
                .allow("If you edited the file yourself, start the link once with --accept-state (scripts/mac.sh FOLDER --accept-state). If you did not, remove it: the link starts from the defaults")
        };
        // In FERB a changed or unverifiable file is set aside and the link starts from the defaults: nothing in it is
        // a paired phone any more, and a phone must never be kept off by a file nobody can accept at a prompt.
        let set_aside = |why: &str| -> LinkResult<()> {
            let aside = dir.join("link.json.changed");
            fs::rename(&file, &aside).map_err(|e| io("setting link.json aside", e))?;
            eprintln!(
                "ferb-link: link.json {why}; it was set aside as {} and the link starts from the defaults",
                aside.display()
            );
            Ok(())
        };
        if secrets.given() && !accept_changed {
            let Some(k) = had_key else {
                return Err(LinkError::new(
                    errors::FERB_ERROR,
                    "FERB handed over no key for the link's state",
                ));
            };
            if let Some(s) = &saved {
                let fine = !s.seal.is_empty()
                    && matches!(sealed_parts(on_disk.as_deref().unwrap_or_default()),
                                Some((body, seal)) if seal_of(&k, &body) == seal);
                if !fine {
                    set_aside(if s.seal.is_empty() {
                        "has lost its seal"
                    } else {
                        "was changed since it was sealed, or sealed under another key"
                    })?;
                    saved = None;
                }
            }
        }
        let state_key = match (had_key, &saved) {
            (Some(k), Some(s)) if !accept_changed => {
                if s.seal.is_empty() {
                    return Err(changed("has lost its seal"));
                }
                // judged from the text on disk, not from this build's reading of it (see `sealed_parts`)
                match sealed_parts(on_disk.as_deref().unwrap_or_default()) {
                    Some((body, seal)) if seal_of(&k, &body) == seal => {}
                    _ => return Err(changed("was changed since it was sealed")),
                }
                k
            }
            (Some(k), _) => k,
            (None, Some(s)) if !s.seal.is_empty() && !accept_changed => {
                return Err(LinkError::new(errors::FERB_ERROR, format!("{} is sealed, but the key that seals it is not in {} any more, so it cannot be checked. Nothing was removed.", file.display(), secrets.words()))
                    .allow("If it is as you expect, start the link once with --accept-state to take the file as it is, or remove it: the link starts from the defaults"));
            }
            (None, _) if secrets.given() => {
                return Err(LinkError::new(
                    errors::FERB_ERROR,
                    "FERB handed over no key for the link's state",
                ));
            }
            (None, _) => {
                // first start (or the first with a build that seals): the state as found is sealed now
                let mut k = [0u8; 32];
                rand::RngCore::fill_bytes(&mut rand::rng(), &mut k);
                secrets.set(crate::secrets::STATE_KEY, &k)?;
                k
            }
        };
        // the relay's token lives with the secrets; one found in an older file moves there
        if let Some(r) = saved.as_mut().and_then(|s| s.own_relay.as_mut()) {
            if let Some(t) = r.token.take() {
                secrets.set(crate::secrets::RELAY_TOKEN, t.as_bytes())?
            }
            r.token = secrets
                .get(crate::secrets::RELAY_TOKEN)?
                .and_then(|b| String::from_utf8(b).ok());
        }
        let fresh = saved.is_none();
        let me = Self {
            dir: dir.to_path_buf(),
            saved: Mutex::new(saved.unwrap_or_default()),
            secrets,
            state_key,
        };
        if !fresh {
            let s = me.saved.lock().unwrap();
            me.save(&s)?;
        }
        Ok(me)
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    /// Where the secrets are kept, in words (the harness says it at start).
    pub fn keys_words(&self) -> &'static str {
        self.secrets.words()
    }
    fn save(&self, s: &Saved) -> LinkResult<()> {
        let mut out = s.clone();
        if let Some(r) = out.own_relay.as_mut() {
            r.token = None;
        }
        let body = sealed_body(&out);
        out.seal = seal_of(&self.state_key, &body);
        let text = serde_json::to_vec_pretty(&out).unwrap();
        // what is written must be what the next start (of this build or another) will find sealed
        if sealed_parts(&text) != Some((body, out.seal.clone())) {
            return Err(LinkError::new(
                errors::FERB_ERROR,
                "link.json was not saved: its seal would not match what was written",
            ));
        }
        write_private(&self.dir.join("link.json"), &text).map_err(|e| io("saving link.json", e))
    }
    /// The Mac's identity key. Handed over by FERB's daemon (kb PHN-143), which made it once; with `Keys::Files`
    /// made here once, in a 0600 file.
    pub fn identity(&self) -> LinkResult<SecretKey> {
        match self.secrets.get(crate::secrets::IDENTITY)? {
            Some(b) => {
                let bytes: [u8; 32] = b.try_into().map_err(|_| {
                    LinkError::new(errors::FERB_ERROR, "the Mac's link key is damaged")
                })?;
                Ok(SecretKey::from_bytes(&bytes))
            }
            None if self.secrets.given() => Err(LinkError::new(
                errors::FERB_ERROR,
                "FERB handed over no link key",
            )),
            None => {
                let mut bytes = [0u8; 32];
                rand::RngCore::fill_bytes(&mut rand::rng(), &mut bytes);
                self.secrets.set(crate::secrets::IDENTITY, &bytes)?;
                Ok(SecretKey::from_bytes(&bytes))
            }
        }
    }

    pub fn limits(&self) -> Limits {
        self.saved.lock().unwrap().limits.clone()
    }
    pub fn set_limits(&self, l: Limits) -> LinkResult<()> {
        let mut s = self.saved.lock().unwrap();
        s.limits = l;
        self.save(&s)
    }
    pub fn own_relay(&self) -> Option<RelaySpec> {
        self.saved.lock().unwrap().own_relay.clone()
    }
    pub fn set_own_relay(&self, r: Option<RelaySpec>) -> LinkResult<()> {
        let mut s = self.saved.lock().unwrap();
        match r.as_ref().and_then(|r| r.token.as_ref()) {
            Some(t) => self
                .secrets
                .set(crate::secrets::RELAY_TOKEN, t.as_bytes())?,
            None => self.secrets.delete(crate::secrets::RELAY_TOKEN)?,
        }
        s.own_relay = r;
        s.relay_set_at = Some(now());
        self.save(&s)
    }

    /// PHN-591: when the relay setting last changed; an own relay set by an older build without the time gets it
    /// now (the bound starts at this upgrade).
    pub fn relay_set_at(&self) -> Option<u64> {
        let mut s = self.saved.lock().unwrap();
        if s.own_relay.is_some() && s.relay_set_at.is_none() {
            s.relay_set_at = Some(now());
            let _ = self.save(&s);
        }
        s.relay_set_at
    }

    /// The own-relay setting this phone is known to have. True when that was news.
    pub fn set_device_relay(&self, id: &str, mark: &str) -> bool {
        let mut s = self.saved.lock().unwrap();
        if s.relay_marks.get(id).map(String::as_str) == Some(mark) {
            return false;
        }
        s.relay_marks.insert(id.to_string(), mark.to_string());
        let _ = self.save(&s);
        true
    }
    pub fn device_relay(&self, id: &str) -> Option<String> {
        self.saved.lock().unwrap().relay_marks.get(id).cloned()
    }
    /// Forgets the marks of phones no longer paired (the daemon's list is the authority).
    pub fn keep_relay_marks(&self, paired: &[String]) {
        let mut s = self.saved.lock().unwrap();
        let n = s.relay_marks.len();
        s.relay_marks.retain(|k, _| paired.contains(k));
        if s.relay_marks.len() != n {
            let _ = self.save(&s);
        }
    }

    /// Counts one file sent to a phone today (shown on the Mac; there is no cap: kb SEC-16).
    pub fn count_file(&self, size: u64) {
        let mut s = self.saved.lock().unwrap();
        let day = now() / 86_400;
        if s.file_day.0 != day {
            s.file_day = (day, 0, 0);
        }
        s.file_day.1 += 1;
        s.file_day.2 += size;
        let _ = self.save(&s);
    }
    pub fn files_today(&self) -> (u64, u64) {
        let s = self.saved.lock().unwrap();
        if s.file_day.0 == now() / 86_400 {
            (s.file_day.1, s.file_day.2)
        } else {
            (0, 0)
        }
    }
    /// The audit of what phones did: one JSON line per call, appended (0600). No phone method reads or writes it.
    pub fn audit(&self, line: &serde_json::Value) {
        use std::io::Write;
        // Kept `audit_keep_days` (the owner: 90). The file is cut into pieces of `audit_max_bytes`; a piece whose
        // newest line is older than that goes. `audit_keep` pieces is the safety limit on size: when it is hit the
        // oldest piece goes early, and the audit says so.
        let (max, keep, days) = {
            let s = self.saved.lock().unwrap();
            (
                s.limits.audit_max_bytes,
                s.limits.audit_keep.max(1),
                s.limits.audit_keep_days,
            )
        };
        let file = self.dir.join("audit.jsonl");
        let mut early = false;
        if max > 0 && fs::metadata(&file).map(|m| m.len() > max).unwrap_or(false) {
            early = self.dir.join(format!("audit.jsonl.{keep}")).exists();
            for i in (1..keep).rev() {
                let _ = fs::rename(
                    self.dir.join(format!("audit.jsonl.{i}")),
                    self.dir.join(format!("audit.jsonl.{}", i + 1)),
                );
            }
            let _ = fs::rename(&file, self.dir.join("audit.jsonl.1"));
            self.audit_expire(days);
        }
        if let Ok(mut f) = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(self.dir.join("audit.jsonl"))
        {
            // one write per line: `writeln!` writes a value in pieces, and two calls audited at once interleaved their
            // pieces into lines nobody could read back (found in S4: a start-over's line went missing under load)
            if early {
                let note = serde_json::json!({"t": now(), "method": "mac.audit_cut_early", "class": "mac", "result": "ok", "why": "the audit reached its size limit before its oldest part was as old as it is kept"});
                let _ = f.write_all(format!("{note}\n").as_bytes());
            }
            let _ = f.write_all(format!("{line}\n").as_bytes());
        }
    }
    /// Removes the pieces of the audit whose newest line is older than `days` (0: never by age).
    pub fn audit_expire(&self, days: u64) {
        if days == 0 {
            return;
        }
        for i in 1..=200u32 {
            let piece = self.dir.join(format!("audit.jsonl.{i}"));
            let Ok(meta) = fs::metadata(&piece) else {
                break;
            };
            let age = meta
                .modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if age > days * 86_400 {
                let _ = fs::remove_file(&piece);
            }
        }
    }
    pub fn audit_tail(&self, n: usize) -> Vec<serde_json::Value> {
        let text = fs::read_to_string(self.dir.join("audit.jsonl")).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(n)..]
            .iter()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// Adds to today's shared-relay total and returns it.
    pub fn add_shared_bytes(&self, n: u64) -> u64 {
        let mut s = self.saved.lock().unwrap();
        let day = now() / 86_400;
        if s.shared_day.0 != day {
            s.shared_day = (day, 0);
        }
        s.shared_day.1 += n;
        if n > 0 {
            let _ = self.save(&s);
        }
        s.shared_day.1
    }
}

/// A device name as the phone gave it: one line, at most 40 characters, no control characters.
pub fn clean_name(name: &str) -> String {
    let s: String = name.chars().filter(|c| !c.is_control()).take(40).collect();
    let s = s.trim().to_string();
    if s.is_empty() { "Phone".into() } else { s }
}
