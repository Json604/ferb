//! `Mobile`, the phone's core (ferb-mobile `api::Mobile`) as the apps call it.

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use zeroize::Zeroize;

use crate::callbacks::{
    Clock, ClockAdapter, GuardedSpawner, Ids, IdsAdapter, Listener, ListenerAdapter, Signer,
    SignerAdapter, Transport, TransportAdapter,
};
use crate::error::{FerbError, guard, guard_or};
use crate::link::{Link, LinkInner};
use crate::records::{
    Cancelled, Conv, ConversationInfo, LinkState, OlderState, OutboxItem, TranscriptView,
};

/// What the app tells the core at open.
#[derive(Debug, Clone, uniffi::Record)]
pub struct MobileConfig {
    /// This phone's id among the conversation's clients: made once by the app (a random UUID) and kept.
    pub client_id: String,
    /// The phone's offset from UTC now, in seconds (whether a later answer was said on its turn's day).
    pub utc_offset_s: i32,
    /// Entries per page (default 50: the link's `shared_page_max`).
    #[uniffi(default = None)]
    pub page_limit: Option<u32>,
    /// A reply's largest size in bytes (default 63 KiB: the link's control frame less 1 KiB).
    #[uniffi(default = None)]
    pub max_bytes: Option<u32>,
    /// Characters per piece of a long entry read whole (default 12 000).
    #[uniffi(default = None)]
    pub piece_chars: Option<u32>,
}

/// An answer to one of FERB's questions. Never queued: it goes now or not at all.
#[derive(uniffi::Enum)]
pub enum AnswerInput {
    /// A choice (`chat.approve`).
    Approve { granted: bool, choice: Option<i64> },
    /// Free words (`chat.answer`; empty declines).
    Words { text: String },
    /// The owner's words for a safety check's task (`chat.correct`).
    Correct { text: String },
    /// A hidden input (a login code, a password): sent on the sealed path, signed over its SHA-256 only; never
    /// stored or logged. Needs a `Signer`.
    Hidden { secret: String },
}

impl std::fmt::Debug for AnswerInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnswerInput::Approve { granted, choice } => {
                write!(f, "Approve {{ granted: {granted}, choice: {choice:?} }}")
            }
            AnswerInput::Words { .. } => f.write_str("Words { .. }"),
            AnswerInput::Correct { .. } => f.write_str("Correct { .. }"),
            AnswerInput::Hidden { .. } => f.write_str("Hidden { <hidden> }"),
        }
    }
}

impl AnswerInput {
    /// The core's answer; the words move into it (the core zeroizes a hidden value when it drops). (No `Drop` here:
    /// UniFFI's lowering moves the fields out, so the enum can't have one; `answer` takes it first thing.)
    fn take(mut self) -> ferb_mobile::Answer {
        match &mut self {
            AnswerInput::Approve { granted, choice } => ferb_mobile::Answer::Approve {
                granted: *granted,
                choice: *choice,
            },
            AnswerInput::Words { text } => ferb_mobile::Answer::Words {
                text: std::mem::take(text),
            },
            AnswerInput::Correct { text } => ferb_mobile::Answer::Correct {
                text: std::mem::take(text),
            },
            AnswerInput::Hidden { secret } => ferb_mobile::Answer::Hidden {
                secret: std::mem::take(secret),
            },
        }
    }
}

/// The phone's core: the stored chat, sync, the outbox, answers. Every method is blocking and bounded; README.md
/// ("Threading") lists the ones safe on the UI thread. `shutdown` (or `forget_this_mac`) ends it; every later call
/// answers `closed`.
#[derive(uniffi::Object)]
pub struct Mobile {
    core: RwLock<Option<Arc<ferb_mobile::Mobile>>>,
    link: Mutex<Option<Arc<LinkInner>>>,
    /// The transport the core was given, for `unlock` (the core keeps its own copy).
    transport: Mutex<Option<Arc<dyn ferb_mobile::Transport>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Mobile {
    fn core(&self) -> Result<Arc<ferb_mobile::Mobile>, FerbError> {
        self.core
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or_else(FerbError::closed)
    }

    /// Takes the core out (closed from now on) and lets go of the link's events.
    fn take(&self) -> Option<Arc<ferb_mobile::Mobile>> {
        let core = self.core.write().unwrap_or_else(|p| p.into_inner()).take();
        lock(&self.transport).take();
        if let (Some(c), Some(link)) = (&core, lock(&self.link).take()) {
            link.detach(c);
        }
        core
    }
}

#[uniffi::export]
impl Mobile {
    /// Opens the phone's store at `path` with the app's 32-byte key (unwrapped from the Keystore/Keychain for this
    /// call; the library zeroizes its copy and never stores it). Not on the UI thread (it opens and may upgrade the
    /// encrypted database).
    #[uniffi::constructor]
    pub fn open(
        path: String,
        key: Vec<u8>,
        config: MobileConfig,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn Ids>,
        listener: Arc<dyn Listener>,
    ) -> Result<Arc<Mobile>, FerbError> {
        let mut key = key;
        let out = guard(|| {
            let mut c = ferb_mobile::Config::new(config.client_id.clone());
            c.utc_offset_s = config.utc_offset_s;
            if let Some(n) = config.page_limit {
                c.page_limit = n;
            }
            if let Some(n) = config.max_bytes {
                c.max_bytes = n;
            }
            if let Some(n) = config.piece_chars {
                c.piece_chars = n;
            }
            let env = ferb_mobile::Env {
                clock: Arc::new(ClockAdapter(clock)),
                ids: Arc::new(IdsAdapter(ids, AtomicU64::new(0))),
                spawner: Arc::new(GuardedSpawner),
                listener: Arc::new(ListenerAdapter(listener)),
            };
            let core = ferb_mobile::Mobile::open(path, std::mem::take(&mut key), c, env)?;
            Ok(Arc::new(Mobile {
                core: RwLock::new(Some(Arc::new(core))),
                link: Mutex::new(None),
                transport: Mutex::new(None),
            }))
        });
        key.zeroize();
        out
    }

    /// Plugs the phone link in: the core's way to the Mac, and the link's state, bulk and relay events, from now on
    /// (the link's current state at once: online starts a sync). Replaces an earlier link or transport.
    pub fn connect(&self, link: Arc<Link>) -> Result<(), FerbError> {
        guard(|| {
            let core = self.core()?;
            let transport: Arc<dyn ferb_mobile::Transport> = link.inner.transport()?;
            core.connect(transport.clone())?;
            *lock(&self.transport) = Some(transport);
            let old = lock(&self.link).replace(link.inner.clone());
            if let Some(old) = old.filter(|o| !Arc::ptr_eq(o, &link.inner)) {
                old.detach(&core);
            }
            link.inner.attach(&core);
            Ok(())
        })
    }

    /// Plugs in a transport the app supplies instead of the link (a fake Mac in tests). The app forwards that
    /// transport's link events with `link_event`.
    pub fn connect_transport(&self, transport: Arc<dyn Transport>) -> Result<(), FerbError> {
        guard(|| {
            let core = self.core()?;
            if let Some(old) = lock(&self.link).take() {
                old.detach(&core);
            }
            let transport: Arc<dyn ferb_mobile::Transport> = Arc::new(TransportAdapter(transport));
            core.connect(transport.clone())?;
            *lock(&self.transport) = Some(transport);
            Ok(())
        })
    }

    /// One event of the link's own (`state`, `bulk`, `relay_state`, as JSON), for a transport from
    /// `connect_transport` (a `Link` forwards its own).
    pub fn link_event(&self, event_json: String) -> Result<(), FerbError> {
        guard(|| {
            self.core()?.link_event(&event_json);
            Ok(())
        })
    }

    /// Unplugs the link: the watch stops and nothing live stays.
    pub fn disconnect(&self) -> Result<(), FerbError> {
        guard(|| {
            let core = self.core()?;
            if let Some(old) = lock(&self.link).take() {
                old.detach(&core);
            }
            lock(&self.transport).take();
            core.disconnect();
            Ok(())
        })
    }

    /// Reads what changed on the Mac and starts the watch, then sends the outbox. Network: not on the UI thread.
    pub fn sync(&self) -> Result<(), FerbError> {
        guard(|| Ok(self.core()?.sync()?))
    }

    /// The conversations held, the current one marked.
    pub fn conversations(&self) -> Result<Vec<ConversationInfo>, FerbError> {
        guard(|| Ok(self.core()?.conversations()?.conv()))
    }

    /// The conversation FERB writes in now (as the Mac said at the last sync).
    pub fn current_conversation(&self) -> Result<Option<String>, FerbError> {
        guard(|| Ok(self.core()?.current_conversation()?))
    }

    /// A conversation as the app draws it.
    pub fn transcript(&self, conversation: String) -> Result<TranscriptView, FerbError> {
        guard(|| Ok(self.core()?.transcript(&conversation)?.conv()))
    }

    /// The outbox, oldest first.
    pub fn outbox(&self) -> Result<Vec<OutboxItem>, FerbError> {
        guard(|| Ok(self.core()?.outbox()?.conv()))
    }

    /// Writes a message: in the outbox at once (visible, cancellable), sent now when the link is up (which is network
    /// work: not on the UI thread).
    pub fn send(&self, text: String) -> Result<OutboxItem, FerbError> {
        guard(|| Ok(self.core()?.send(&text)?.conv()))
    }

    /// Sends what waits in the outbox, in order. Network: not on the UI thread.
    pub fn flush_outbox(&self) -> Result<(), FerbError> {
        guard(|| Ok(self.core()?.flush_outbox()?))
    }

    /// Takes a waiting or failed message out of the outbox (`in_flight` while it is being sent).
    pub fn cancel(&self, outbox_id: String) -> Result<Cancelled, FerbError> {
        guard(|| Ok(self.core()?.cancel(&outbox_id)?.conv()))
    }

    /// Answers one of FERB's questions now (never queued); the reply's JSON. `signer` signs the exact answer when the
    /// Mac checks it against the phone's key (a hidden input always). Network and the fingerprint prompt: not on the
    /// UI thread.
    pub fn answer(
        &self,
        approval_id: String,
        answer: AnswerInput,
        signer: Option<Arc<dyn Signer>>,
    ) -> Result<String, FerbError> {
        let answer = answer.take();
        guard(|| {
            let signer = signer.map(|s| Arc::new(SignerAdapter(s)) as Arc<dyn ferb_mobile::Signer>);
            Ok(self.core()?.answer(&approval_id, answer, signer)?)
        })
    }

    /// Stops these tasks (never queued). Network: not on the UI thread.
    pub fn stop(&self, task_ids: Vec<String>) -> Result<(), FerbError> {
        guard(|| Ok(self.core()?.stop(&task_ids)?))
    }

    /// Fetches the next older page (on the public relay refused before asking: `NeedsBetterPath`). Network: not on
    /// the UI thread.
    pub fn load_older(&self, conversation: String) -> Result<OlderState, FerbError> {
        guard(|| Ok(self.core()?.load_older(&conversation)?.conv()))
    }

    /// Tells the Mac how far the owner has read. Network: not on the UI thread.
    pub fn mark_read(&self, conversation: String, pos: i64) -> Result<(), FerbError> {
        guard(|| Ok(self.core()?.mark_read(&conversation, pos)?))
    }

    /// Unlocks the phone toward the Mac (PHN-125): `signer` signs the Mac's fresh challenge for `link.unlock` after
    /// the fingerprint prompt (the key the Mac knows as the phone's lock-bound or fingerprint-only key). Then what
    /// waited for the unlock goes: a sync, which sends the outbox. Network and the prompt: not on the UI thread.
    /// `not_signed` when the owner cancelled; `offline` with no link.
    pub fn unlock(&self, signer: Arc<dyn Signer>) -> Result<(), FerbError> {
        guard(|| {
            let core = self.core()?;
            let t = lock(&self.transport)
                .clone()
                .ok_or_else(|| FerbError::Offline {
                    detail: "FERB's Mac can't be reached now.".into(),
                    link_code: None,
                })?;
            let bad = || FerbError::BadReply {
                detail: "FERB's reply could not be read.".into(),
                link_code: None,
            };
            let reply = t
                .call("link.challenge", "{}", None)
                .map_err(FerbError::from_link)?;
            let v: serde_json::Value = serde_json::from_str(&reply).map_err(|_| bad())?;
            let challenge = v["challenge"].as_str().ok_or_else(bad)?;
            let payload = ferb_link::client::Phone::call_payload(
                challenge,
                "link.unlock",
                &serde_json::json!({}),
            );
            let sig =
                ferb_mobile::Signer::sign(&SignerAdapter(signer), &payload).ok_or_else(|| {
                    FerbError::NotSigned {
                        detail: "Nothing was sent.".into(),
                        link_code: None,
                    }
                })?;
            let proof = serde_json::json!({"payload": payload, "sig": sig}).to_string();
            t.call("link.unlock", "{}", Some(&proof))
                .map_err(FerbError::from_link)?;
            // what waited for the unlock goes now; a failure of that is the sync's own, reported to the listener
            let _ = core.sync();
            Ok(())
        })
    }

    /// The chat's view of the link.
    pub fn link_state(&self) -> Result<LinkState, FerbError> {
        guard(|| Ok(self.core()?.link_state().conv()))
    }

    /// "Forget this Mac": the watch stops, the store's files are deleted, and every call then answers `closed`. The
    /// app then forgets its wrapped store key and calls `Link.forget`.
    pub fn forget_this_mac(&self) -> Result<(), FerbError> {
        guard(|| match self.take() {
            Some(core) => Ok(core.forget_this_mac()?),
            None => Err(FerbError::closed()),
        })
    }

    /// Closes the core: the watch stops, the store closes (its files stay). Idempotent; bounded (no network). Named
    /// `shutdown` because the bindings' own `close()` (Kotlin `AutoCloseable`, `use {}`) frees the object's handle:
    /// that also shuts it down (on drop), after which the bindings throw on any call; after `shutdown` the object
    /// stays and every call answers `closed`.
    pub fn shutdown(&self) {
        guard_or((), || {
            if let Some(core) = self.take() {
                core.close();
            }
        })
    }

    pub fn is_closed(&self) -> bool {
        guard_or(true, || self.core().is_err())
    }

    /// Where the store lives.
    pub fn store_path(&self) -> Result<String, FerbError> {
        guard(|| Ok(self.core()?.store_path().to_string_lossy().into_owned()))
    }
}

/// The bindings' `close()` (or the last reference going) shuts the core down too.
impl Drop for Mobile {
    fn drop(&mut self) {
        self.shutdown();
    }
}
