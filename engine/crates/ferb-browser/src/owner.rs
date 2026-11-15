//! The rest of `Router` (the owner's profiles: their open tabs, the profile list, the paired ones
//! not connected) and of `FerbIdentity` (FERB's inbox codes, sealed: `otp_candidates`, `fill_otp`,
//! `_read_feed`, `session_ok`), and the owner's line for a tab a task's cleanup couldn't let go of
//! (decision: HAND-419).

use std::collections::HashSet;
use std::sync::Arc;

use ferb_core::types::{ActionResult, ElementRef, Identity};
use ferb_identity::mailbox::GmailFeedMailbox;
use ferb_identity::secrets::SecretRef;
use serde_json::{json, Map, Value};

use crate::bridge::Bridge;
use crate::driver::{Driver, Kind, Tab};
use crate::error::{Error, Result};
use crate::ops::py_int;
use crate::profiles::{self, BrowserProfile};

/// The profiles the daemon knows (discovery, FERB's pairing records, why discovery sees none).
pub trait ProfilesView: Send + Sync {
    /// Discovered from each installed browser's Local State.
    fn discovered(&self) -> Vec<BrowserProfile>;
    /// `known`: discovered, plus the paired ones of a browser discovery found none in (HAND-407).
    fn known(&self) -> Vec<BrowserProfile>;
    /// Why discovery can't see the owner's profiles now (Full Disk Access off), else `None`.
    fn unseen(&self) -> Option<String>;
}

/// FERB's inbox (`FerbIdentity`'s mailbox seams): its address and the feed's address.
#[derive(Clone)]
pub struct Inbox {
    pub address: Arc<dyn Fn() -> String + Send + Sync>,
    pub feed_url: String,
}

type InventoryKey = (Option<String>, String, String);

/// The driver's random source as the mailbox's (its candidate tokens: `secrets.token_urlsafe`).
struct Tokens(Arc<dyn crate::pairing::Random>);

impl ferb_identity::mailbox::TokenRandom for Tokens {
    fn fill(&self, buf: &mut [u8]) -> bool {
        let got = self.0.bytes(buf.len());
        buf.copy_from_slice(&got);
        true
    }
}

/// FERB's mailbox inventories (`_otp_inventory`) and the messages whose code was used.
#[derive(Default)]
pub struct Otp {
    used: HashSet<String>,
    inventory: Vec<(InventoryKey, GmailFeedMailbox)>,
}

impl Driver {
    fn known(&self) -> Vec<BrowserProfile> {
        self.seams().profiles.map(|p| p.known()).unwrap_or_default()
    }

    /// `_leftover` (HAND-419): a tab the cleanup couldn't let go of, as the owner knows it.
    pub(crate) fn leftover(&self, t: &Tab, error: &str) -> Value {
        let p = profiles::named(&self.known(), &t.tab.browser, &t.tab.profile);
        let browser = self
            .hub
            .browser_info(&t.tab.browser)
            .map_or_else(|| profiles::capitalize(&t.tab.browser), |b| b.label);
        let host = if t.url.starts_with("http://") || t.url.starts_with("https://") {
            crate::url::hostname(&t.url)
                .ok()
                .flatten()
                .unwrap_or_default()
        } else {
            String::new()
        };
        let title = if t.title.is_empty() {
            (self.tasks.redact)(&host)
        } else {
            t.title.clone()
        };
        json!({"tab": t.tab.tab_id, "browser": browser, "profile": p.display_name, "account": p.email,
               "title": title, "borrowed": t.attached, "kept_open": t.keep_open, "error": error})
    }

    /// `profile_candidates`: every owner profile with its pairing and connection facts.
    pub fn profile_candidates(&self) -> Result<Vec<Map<String, Value>>> {
        let mut out = vec![];
        for p in self.known() {
            let br = self.hub.bridge(&p.browser, &p.folder)?;
            if br
                .as_ref()
                .is_some_and(|b| b.binding.identity == Identity::Ferb)
            {
                continue;
            }
            let connected = br.as_ref().is_some_and(|b| b.is_connected());
            let mut row = Map::new();
            row.insert(
                "profile".into(),
                format!("{}/{}", p.browser, p.folder).into(),
            );
            row.insert("name".into(), p.display_name.clone().into());
            row.insert("account".into(), p.email.clone().into());
            row.insert("paired".into(), br.is_some().into());
            row.insert("connected".into(), connected.into());
            if let Some(b) = br.filter(|_| !connected) {
                if let Some(problem) = self.hub.problem(&b.binding) {
                    row.insert("refused".into(), problem.into());
                }
            }
            out.push(row);
        }
        Ok(out)
    }

    /// `profiles_note` (HAND-407): why discovery found no profiles, when FERB knows.
    pub fn profiles_note(&self) -> Option<String> {
        let p = self.seams().profiles?;
        if p.discovered().is_empty() {
            p.unseen()
        } else {
            None
        }
    }

    /// `_owner_bridges`: the owner's profiles' bridges (connected ones only, unless not asked),
    /// only the one named when a profile is named.
    fn owner_bridges(
        &self,
        profile: Option<&str>,
        connected_only: bool,
    ) -> Result<Vec<Arc<Bridge>>> {
        let choice = match (profile.filter(|p| !p.is_empty()), &self.kind) {
            (Some(p), Kind::Owner { choose, .. }) => Some(choose(Some(p))?),
            _ => None,
        };
        Ok(self
            .hub
            .bridges()
            .into_iter()
            .filter(|br| !connected_only || br.is_connected())
            .filter(|br| br.binding.identity == Identity::Owner)
            .filter(|br| {
                choice
                    .as_ref()
                    .is_none_or(|(b, f)| *b == br.binding.browser && *f == br.binding.folder)
            })
            .collect())
    }

    /// `not_connected`: the paired owner profiles a listing of every profile skipped.
    pub fn not_connected(&self) -> Result<Vec<Map<String, Value>>> {
        Ok(self
            .owner_bridges(None, false)?
            .into_iter()
            .filter(|br| !br.is_connected())
            .map(|br| {
                let why = self.hub.problem(&br.binding).unwrap_or_else(|| {
                    "not connected (usually the profile just isn't running): browser_open with this profile starts it \
                     in the background"
                        .into()
                });
                [("profile".to_string(), Value::from(br.name())), ("why".into(), why.into())]
                    .into_iter()
                    .collect()
            })
            .collect())
    }

    /// `Router.open_tabs`: the owner's open tabs in their connected profiles; a tab this task already
    /// took over with its tab id; FERB's own tabs and other tasks' left out.
    pub async fn open_tabs(
        &self,
        profile: Option<&str>,
        task: Option<&str>,
    ) -> Result<Vec<Map<String, Value>>> {
        let known = self.known();
        let profile = profile.filter(|p| !p.is_empty());
        let selected = self.owner_bridges(profile, false)?;
        if profile.is_some() && self.wake.is_some() {
            for br in &selected {
                self.connected(Arc::clone(br)).await?;
            }
        }
        let missing: Vec<&Arc<Bridge>> = selected.iter().filter(|b| !b.is_connected()).collect();
        if !missing.is_empty() && profile.is_some() {
            let details: Vec<String> = missing
                .iter()
                .map(|br| {
                    format!(
                        "{}: {}",
                        br.name(),
                        self.hub.problem(&br.binding).unwrap_or_else(|| {
                            "paired but not connected (usually the profile just isn't running)"
                                .into()
                        })
                    )
                })
                .collect();
            return Err(Error::refused(format!(
                "{}. No tab inventory is available for these profiles; this is a connection problem, not an empty tab \
                 list. Preserve the requested profile. browser.open with this profile loads it without a window and \
                 opens the tab itself: in one of its open windows or, when it has none, in FERB's own minimized window, \
                 made once; nothing to ask the owner.",
                details.join("; ")
            )));
        }
        let mut out = vec![];
        for br in self.owner_bridges(profile, true)? {
            let v = br.call("list_tabs", json!({}), 10.0, None).await?;
            for t in v
                .get("tabs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if t.get("ferb").is_some_and(ferb_py::value::truthy) {
                    continue; // FERB's own minimized window: never the owner's
                }
                let id = py_int(t.get("tab_id"))
                    .ok_or_else(|| Error::driver("a listed tab without an id"))?;
                let mine = task.and_then(|task| {
                    self.st()
                        .tabs
                        .values()
                        .find(|x| {
                            Arc::ptr_eq(&x.bridge, &br)
                                && x.chrome_id == id
                                && x.attached
                                && self.accessible(x, task)
                        })
                        .cloned()
                });
                if mine.is_none() && self.owner_of(&br, id).is_some_and(|o| o.is_some()) {
                    continue; // FERB's own tabs, or another task's
                }
                let shown = profiles::shown(
                    &profiles::named(&known, &br.binding.browser, &br.binding.folder),
                    Some(&profiles::capitalize(&br.binding.browser)),
                );
                let text = |k: &str| crate::rules::text_or_empty(t.get(k));
                let mut entry = Map::new();
                entry.insert(
                    "tab_ref".into(),
                    format!(
                        "{}:{}:{}",
                        br.binding.browser,
                        br.binding.folder,
                        ferb_py::value::py_str(&t["tab_id"])
                    )
                    .into(),
                );
                entry.insert("profile".into(), shown.into());
                entry.insert(
                    "profile_id".into(),
                    format!("{}/{}", br.binding.browser, br.binding.folder).into(),
                );
                entry.insert("url".into(), text("url").into());
                entry.insert("title".into(), text("title").into());
                entry.insert("site".into(), text("host").into());
                entry.insert(
                    "active".into(),
                    t.get("active").is_some_and(ferb_py::value::truthy).into(),
                );
                entry.insert(
                    "window_id".into(),
                    t.get("window_id").cloned().unwrap_or(Value::Null),
                );
                for k in ["audible", "muted"] {
                    if let Some(x) = t.get(k) {
                        entry.insert(k.into(), ferb_py::value::truthy(x).into());
                    }
                }
                if let Some(mine) = mine {
                    entry.insert("taken_over_as".into(), mine.tab.tab_id.clone().into());
                    if !mine.taken_over_from.is_empty() {
                        entry.insert(
                            "taken_over_from".into(),
                            Value::Object(mine.taken_over_from.clone()),
                        );
                    }
                }
                out.push(entry);
            }
        }
        Ok(out)
    }

    fn otp(&self) -> std::sync::MutexGuard<'_, Otp> {
        self.otp_lock()
    }

    fn inbox(&self) -> Inbox {
        self.seams().inbox.unwrap_or_else(|| Inbox {
            address: Arc::new(String::new),
            feed_url: ferb_identity::mailbox::FEED_URL.into(),
        })
    }

    /// `_read_feed`: FERB's inbox feed in a background tab of FERB's profile (no task owns it), sealed.
    async fn read_feed(&self, url: &str) -> Result<String> {
        let br = self.ferb_bridge()?;
        let br = self.connected(br).await?;
        let key = self
            .hub
            .pairings
            .key(&br.binding.install_id)
            .map_err(|e| Error::driver(e.to_string()))?
            .ok_or_else(|| Error::refused("FERB's identity profile isn't paired"))?;
        let tab = self
            .open_in(Arc::clone(&br), url, None, None, false, false, None, None)
            .await?;
        let read = async {
            let t = self.live_tab(&tab)?;
            let cid = format!("r-{}", crate::pairing::token_hex(self.random(), 8));
            let v = br
                .call(
                    "read_source",
                    json!({"tab_id": t.chrome_id}),
                    20.0,
                    Some(cid.clone()),
                )
                .await?;
            let sealed = v
                .get("sealed")
                .and_then(crate::seal::from_wire)
                .ok_or_else(|| Error::driver("the feed didn't come back sealed"))?;
            let text =
                crate::seal::unseal(&crate::seal::seal_key(key.expose())[..], &sealed, &cid, url)
                    .map_err(|_| Error::driver("the feed's seal didn't open"))?;
            Ok::<String, Error>(text.expose().to_string())
        }
        .await;
        // Python's `finally`: a failed close is what the caller sees, even after a read that worked
        self.close(&tab, None).await?;
        read
    }

    /// `otp_candidates`: FERB's unread inbox's opaque numeric candidates for this account and the
    /// field's origin; codes never leave the driver.
    pub async fn otp_candidates(
        &self,
        el: &ElementRef,
        secret: &SecretRef,
    ) -> Result<Map<String, Value>> {
        let Kind::Ferb { secrets, .. } = &self.kind else {
            return Err(Error::refused(
                "FERB's inbox codes are never used in the owner's profiles",
            ));
        };
        if !secrets.has(secret) {
            return Err(Error::refused(format!(
                "no account {} is set up",
                secret.key
            )));
        }
        let (t, frame, ..) = self.resolve(el)?;
        let origin = self.checked_origin(&t, frame, secret).await?;
        let inbox = self.inbox();
        let mut mailbox = GmailFeedMailbox::with_random(
            &(inbox.address)(),
            &inbox.feed_url,
            Arc::new(Tokens(self.random_arc())),
        );
        mailbox.used = self.otp().used.clone();
        let text = self.read_feed(&inbox.feed_url).await?;
        let result = mailbox.candidates(&text);
        let key = (t.owner.clone(), secret.key.clone(), origin.clone());
        {
            let mut otp = self.otp();
            otp.inventory.retain(|(k, _)| *k != key);
            otp.inventory.push((key, mailbox));
        }
        let mut out = result.as_object().cloned().unwrap_or_default();
        out.insert("destination_origin".into(), origin.into());
        Ok(out)
    }

    /// `fill_otp`: the explicitly selected inbox candidate into its origin-bound field, sealed.
    pub async fn fill_otp(
        &self,
        el: &ElementRef,
        secret: &SecretRef,
        candidate: &str,
        intent_id: &str,
    ) -> Result<ActionResult> {
        let Kind::Ferb { secrets, .. } = &self.kind else {
            return Err(Error::refused(
                "FERB's inbox codes are never used in the owner's profiles",
            ));
        };
        if !secrets.has(secret) {
            return Err(Error::refused(format!(
                "no account {} is set up",
                secret.key
            )));
        }
        let (t, frame, snap, local, info) = self.resolve(el)?;
        if info.terminal {
            return Err(Error::refused(crate::driver::SECRET_TERMINAL));
        }
        let origin = self.checked_origin(&t, frame, secret).await?;
        let key = (t.owner.clone(), secret.key.clone(), origin.clone());
        let found = {
            let mut otp = self.otp();
            let used = otp.used.clone();
            let Some((_, mailbox)) = otp.inventory.iter_mut().find(|(k, _)| *k == key) else {
                return Err(Error::refused(
                    "inspect OTP candidates for this account and destination first",
                ));
            };
            mailbox.used = used;
            mailbox.take(candidate).ok_or_else(|| {
                Error::refused(
                    "OTP candidate is unavailable, consumed or belongs to another inventory",
                )
            })?
        };
        let message_id = found.message_id.clone();
        let r = self
            .sealed_fill(&t, frame, snap, local, &origin, found.code, intent_id)
            .await?;
        if r.ok {
            self.otp().used.insert(message_id.clone());
        }
        let mut facts = Map::new();
        facts.insert("message_id".into(), message_id.into());
        facts.insert("source".into(), self.inbox().feed_url.into());
        facts.insert("destination_origin".into(), origin.into());
        Ok(ActionResult {
            ok: r.ok,
            detail: format!("{} (explicitly selected inbox candidate)", r.detail),
            facts,
            observed: None,
        })
    }

    /// `FerbIdentity.release`'s end: the task's inbox inventories go.
    pub(crate) fn forget_otp(&self, owner: &str) {
        self.otp()
            .inventory
            .retain(|((o, _, _), _)| o.as_deref() != Some(owner));
    }

    /// `session_ok`: FERB signed in to its own inbox? FERB's profile not reachable is an error.
    pub async fn session_ok(&self) -> Result<bool> {
        let br = self.ferb_bridge()?;
        self.connected(br).await?;
        let inbox = self.inbox();
        let mailbox = GmailFeedMailbox::new(&(inbox.address)(), &inbox.feed_url);
        let text = self.read_feed(&inbox.feed_url).await?;
        Ok(mailbox.signed_in(&text))
    }
}
