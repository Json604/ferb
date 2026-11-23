//! `Mobile`, the phone's core (ferb-mobile `api::Mobile`) as the apps call it.

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use zeroize::Zeroize;

use crate::callbacks::{
    Clock, ClockAdapter, GuardedSpawner, Ids, IdsAdapter, Listener, ListenerAdapter, Signer,
    SignerAdapter, TransferAdapter, TransferListener, Transport, TransportAdapter,
};
use crate::error::{FerbError, guard, guard_or};
use crate::link::{Link, LinkInner};
use crate::records::{
    Attachment, CameraFrame, Cancelled, Conv, ConversationInfo, DeviceReport, Fetched, Folder,
    FoundFiles, GroupStarted, Insomnia, InsomniaEnded, InsomniaStarted, LinkState, LocationFix,
    MacInfo, MacLocked, MemoryReview, OlderState, OutboxItem, PhoneStatus, PreviewFrame,
    RecentFiles, ScreenUse, ScreenUseEnd, SessionSent, SessionStartPlan, SessionTimeline,
    SessionsList, SettingValue, TaskPage, TranscriptView, Tree, Uploaded,
};

fn signer_of(s: Arc<dyn Signer>) -> Arc<dyn ferb_mobile::Signer> {
    Arc::new(SignerAdapter(s))
}

/// Where a live preview's answers go (`Mobile.watch_live`), on the preview's own thread: post and return.
#[uniffi::export(foreign)]
pub trait PreviewSink: Send + Sync {
    /// One answer: a new frame (`jpeg`), or the state and line over the last one.
    fn frame(&self, frame: PreviewFrame);
    /// The preview stopped by itself: `code` is the stable code of why (`needs_better_path` on the public relay,
    /// `needs_unlock` after the lock, `closed`, `refused`), none when `stop` was called.
    fn ended(&self, code: Option<String>, detail: Option<String>);
}

/// A task's live preview under way (`Mobile.watch_live`): frames in memory only, at the link's pace. `stop` ends it
/// (idempotent); so does dropping it.
#[derive(uniffi::Object)]
pub struct LivePreview {
    stop: Arc<std::sync::atomic::AtomicBool>,
}

#[uniffi::export]
impl LivePreview {
    /// Stops asking for frames (the one under way, if any, is not shown).
    pub fn stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn is_stopped(&self) -> bool {
        self.stop.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Drop for LivePreview {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Where the Mac's camera's answers go in insomnia mode (`Mobile.watch_camera`, APP-983), on the camera's own thread:
/// post and return.
#[uniffi::export(foreign)]
pub trait CameraSink: Send + Sync {
    /// One answer: a new frame (`jpeg`), or the state and words over the last one.
    fn frame(&self, frame: CameraFrame);
    /// It stopped by itself: `code` is the stable code of why (`needs_better_path` on the public relay,
    /// `needs_unlock` after the lock, `closed`, `refused`), none when `stop` was called.
    fn ended(&self, code: Option<String>, detail: Option<String>);
}

/// The Mac's camera is asked at most once every this many milliseconds (it makes a frame about every half second; each
/// ask keeps it on 6 s more on the Mac).
pub const CAMERA_EVERY_MS: u64 = 500;

/// The least time between two frame requests: the link's least time between two pictures on a direct path or the
/// owner's relay (ferb-link `Limits::good_preview_every_ms`, 200 ms); asking sooner is refused there.
pub const PREVIEW_LEAST_MS: u64 = 200;
/// The work row's picture: at most one answer every this many milliseconds (a small picture on the chat, not the
/// full screen of Watch live).
pub const THUMBNAIL_LEAST_MS: u64 = 2_000;
/// After a passing failure (the link dropped a moment, a capture failed), the wait before asking again (FERB.app's
/// own: "A brief failure keeps the last frame", then a second).
pub const PREVIEW_RETRY_MS: u64 = 1000;

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
    /// The screen-use question's Yes "until I'm back at my Mac" (GATE-615): also covers later screen-use questions
    /// until the owner is back at the Mac. Offer it only where the question's `needs.until_back`. Needs the unlock
    /// window (`NeedsUnlock` when closed: unlock, then answer again); never signed (a signer passed is not used).
    UntilBack,
    /// Free words (`chat.answer`; empty declines).
    Words { text: String },
    /// The owner's words for a safety check's task (`chat.correct`).
    Correct { text: String },
    /// A hidden input (a login code, a password): sent on the sealed path, signed over its SHA-256 only; never
    /// stored or logged. Needs a `Signer`. Prefer `Mobile.answer_hidden` with a `SecretInput`: this variant's text
    /// crosses the library's boundary in a buffer nobody overwrites.
    Hidden { secret: String },
}

/// The UTF-16 units a `SecretInput` takes before it grows (a login code, a password: far fewer).
const SECRET_UNITS: usize = 256;

/// A hidden answer typed on the phone, handed over one UTF-16 unit at a time (`push`), so it never crosses the
/// library's boundary as one string or byte buffer: each unit is a plain number passed by value, and the library's
/// only copy is overwritten when the answer is sent, `clear`ed or dropped (kb S8's note; T-1211 D). What the app's
/// own text field kept is the app's to clear.
#[derive(uniffi::Object)]
pub struct SecretInput {
    units: Mutex<Vec<u16>>,
}

impl Default for SecretInput {
    fn default() -> Self {
        SecretInput {
            units: Mutex::new(Vec::with_capacity(SECRET_UNITS)),
        }
    }
}

impl SecretInput {
    /// The secret as text, and the units overwritten (an empty input after).
    fn take_text(&self) -> Result<String, FerbError> {
        let mut units = lock(&self.units);
        let mut text = String::with_capacity(units.len() * 3);
        let mut bad = false;
        for c in char::decode_utf16(units.iter().copied()) {
            match c {
                Ok(c) => text.push(c),
                Err(_) => bad = true,
            }
        }
        units.zeroize();
        units.clear();
        if bad {
            text.zeroize();
            return Err(FerbError::bad_argument(
                "The hidden answer had a broken character; type it again.",
            ));
        }
        Ok(text)
    }
}

#[uniffi::export]
impl SecretInput {
    #[uniffi::constructor]
    pub fn new() -> Arc<SecretInput> {
        Arc::new(SecretInput::default())
    }

    /// One UTF-16 unit of the secret (Kotlin's `Char.code`, Swift's `utf16`), in order.
    pub fn push(&self, unit: u16) {
        let mut units = lock(&self.units);
        if units.len() == units.capacity() {
            // grow by hand, overwriting the old buffer (a plain push would leave a copy behind)
            let mut more = Vec::with_capacity(units.capacity().max(SECRET_UNITS) * 2);
            more.extend_from_slice(&units);
            units.zeroize();
            *units = more;
        }
        units.push(unit);
    }

    /// How many units it holds.
    pub fn len(&self) -> u32 {
        lock(&self.units).len() as u32
    }

    pub fn is_empty(&self) -> bool {
        lock(&self.units).is_empty()
    }

    /// Overwrites and empties it (the sheet closed without sending).
    pub fn clear(&self) {
        let mut units = lock(&self.units);
        units.zeroize();
        units.clear();
    }
}

impl Drop for SecretInput {
    fn drop(&mut self) {
        self.units
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .zeroize();
    }
}

impl std::fmt::Debug for AnswerInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnswerInput::Approve { granted, choice } => {
                write!(f, "Approve {{ granted: {granted}, choice: {choice:?} }}")
            }
            AnswerInput::UntilBack => f.write_str("UntilBack"),
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
            AnswerInput::UntilBack => ferb_mobile::Answer::UntilBack,
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
        // each taken out with its lock let go before it is used or dropped (a link's end closes its door)
        let transport = lock(&self.transport).take();
        let link = lock(&self.link).take();
        if let (Some(c), Some(link)) = (&core, &link) {
            link.detach(c);
        }
        drop(link);
        drop(transport);
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

    /// Answers a hidden input (a login code, a password) from a `SecretInput`, which is emptied and overwritten
    /// whatever happens: sent on the sealed path, signed over the value's SHA-256 only (`signer` is required).
    /// Network and the fingerprint prompt: not on the UI thread.
    pub fn answer_hidden(
        &self,
        approval_id: String,
        secret: Arc<SecretInput>,
        signer: Arc<dyn Signer>,
    ) -> Result<String, FerbError> {
        let text = secret.take_text();
        guard(|| {
            let answer = ferb_mobile::Answer::Hidden { secret: text? };
            Ok(self
                .core()?
                .answer(&approval_id, answer, Some(signer_of(signer)))?)
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
            // the app is unlocked: the conversation is read again (PHN-125)
            core.unlocked();
            // what waited for the unlock goes now; a failure of that is the sync's own, reported to the listener
            let _ = core.sync();
            Ok(())
        })
    }

    /// The unlock the phone's lock key signed in the app's opening prompt before the link was up (kb PHN-649):
    /// `payload` is the phone's own statement `{domain, nonce, made}`, `sig` its signature, `age_ms` how long ago the
    /// prompt passed by the phone's steady clock. The Mac takes it once, fresh, on a connection that proved the device
    /// key; then, as after `unlock`, the conversation is read again. `unlock_required` when it was too old (the app asks
    /// again), `unlock_invalid` when the Mac doesn't know this phone's keys. Network: not on the UI thread.
    pub fn unlock_with_statement(
        &self,
        payload: String,
        sig: String,
        age_ms: u64,
    ) -> Result<(), FerbError> {
        guard(|| {
            let core = self.core()?;
            let t = lock(&self.transport)
                .clone()
                .ok_or_else(|| FerbError::Offline {
                    detail: "FERB's Mac can't be reached now.".into(),
                    link_code: None,
                })?;
            // the Mac takes a statement only on a connection that proved the device key: when that proof is on its
            // way (the link just came online), it is waited for, bounded, instead of the statement being refused as
            // `locked` and sent again a second later (kb PHN-684). The age told is the age when it goes.
            let waited = std::time::Instant::now();
            let link = lock(&self.link).clone();
            if let Some(link) = link {
                link.wait_presented();
            }
            let age_ms = age_ms.saturating_add(waited.elapsed().as_millis() as u64);
            let proof = serde_json::json!({"payload": payload, "sig": sig}).to_string();
            let params = serde_json::json!({"age_ms": age_ms}).to_string();
            t.call("link.unlock_statement", &params, Some(&proof))
                .map_err(FerbError::from_link)?;
            core.unlocked();
            let _ = core.sync();
            Ok(())
        })
    }

    /// "Forget this Mac" on the phone (kb PHN-646): the Mac removes this phone, and only it (`link.forget`, from a
    /// connection that proved the device key). The app wipes everything after, whatever this answers. Network: not on
    /// the UI thread.
    pub fn forget_me(&self) -> Result<(), FerbError> {
        guard(|| {
            let t = lock(&self.transport)
                .clone()
                .ok_or_else(|| FerbError::Offline {
                    detail: "FERB's Mac can't be reached now.".into(),
                    link_code: None,
                })?;
            t.call("link.forget", "{}", None)
                .map_err(FerbError::from_link)?;
            Ok(())
        })
    }

    /// The app locked (PHN-15): the Mac is told (its unlock window for this phone ends now) and the conversation's
    /// watch stops until the next `unlock` (PHN-125); what waits still arrives as the Mac's status. Returns whether the
    /// Mac was told (false offline: its window ends by itself after its idle time). Network: not on the UI thread.
    pub fn lock(&self) -> Result<bool, FerbError> {
        guard(|| Ok(self.core()?.lock()?))
    }

    /// The app unlocked without reaching the Mac (offline): the conversation is read again from the next sync, when
    /// the Mac admits it. `unlock` does this itself.
    pub fn unlocked(&self) -> Result<(), FerbError> {
        guard(|| {
            self.core()?.unlocked();
            Ok(())
        })
    }

    /// "Stop everything on the Mac" (`stop.engage`): FERB stops all it does until the stop is lifted on the Mac. Works
    /// while locked (PHN-125); never queued. Network: not on the UI thread.
    pub fn stop_everything(&self) -> Result<(), FerbError> {
        guard(|| Ok(self.core()?.stop_everything()?))
    }

    /// What FERB has waiting now (`chat.status`, allowed while locked): each question's needs (routine, the
    /// fingerprint, only on the Mac), what runs, what finished. Network: not on the UI thread.
    pub fn status(&self) -> Result<PhoneStatus, FerbError> {
        guard(|| Ok(self.core()?.status()?.conv()))
    }

    /// The last status the Mac pushed or `status` read (memory only; none until one arrives). Safe on the UI thread.
    pub fn last_status(&self) -> Result<Option<PhoneStatus>, FerbError> {
        guard(|| Ok(self.core()?.last_status()?.conv()))
    }

    /// Whether FERB uses the Mac's screen on a phone's "until I'm back at my Mac" now (`screen_use.status`, GATE-615;
    /// allowed while locked, no unlock); also kept in `last_status`. Network: not on the UI thread.
    pub fn screen_use(&self) -> Result<ScreenUse, FerbError> {
        guard(|| Ok(self.core()?.screen_use()?.conv()))
    }

    /// Ends FERB's use of the screen "until I'm back" now (`screen_use.end`; allowed while locked, no unlock; never
    /// queued). Network: not on the UI thread.
    pub fn end_screen_use(&self) -> Result<ScreenUseEnd, FerbError> {
        guard(|| Ok(self.core()?.end_screen_use()?.conv()))
    }

    /// Insomnia mode as the Mac has it now (`insomnia.status`; needs the unlock); also kept in `last_status`, where
    /// the Mac's pushed status (allowed while locked) brings it too. Network: not on the UI thread.
    pub fn insomnia_status(&self) -> Result<Insomnia, FerbError> {
        guard(|| Ok(self.core()?.insomnia_status()?.conv()))
    }

    /// "Keep my Mac working" from the phone, after the app's warning (APP-980): `variant` "shield" or "locked",
    /// `screen_yes` the one Yes to bring apps forward under the cover (APP-984). A sensitive change: signed by
    /// `signer` (the fingerprint-only key where the phone has one, PHN-133). Network and the prompt.
    pub fn start_insomnia(
        &self,
        variant: String,
        screen_yes: bool,
        signer: Arc<dyn Signer>,
    ) -> Result<InsomniaStarted, FerbError> {
        guard(|| {
            Ok(self
                .core()?
                .start_insomnia(&variant, screen_yes, signer_of(signer))?
                .conv())
        })
    }

    // ---- the owner's review of what an import brought in (kb MEM-460..457) --------------------------------------------

    /// The review of what an import brought in (`memory.review`; needs the unlock). Network.
    pub fn memory_review(&self) -> Result<MemoryReview, FerbError> {
        guard(|| Ok(self.core()?.memory_review()?.conv()))
    }

    /// The owner's answer to items of one part (`always_on`: approve / decline; `still_true`: yes / no / unsure;
    /// `offers`: open / done / not interested), one or all at once; the review as it stands after it. Network.
    pub fn answer_memory_review(
        &self,
        part: String,
        item_ids: Vec<String>,
        answer: String,
    ) -> Result<MemoryReview, FerbError> {
        guard(|| {
            Ok(self
                .core()?
                .answer_memory_review(&part, &item_ids, &answer)?
                .conv())
        })
    }

    // ---- the owner's coding-agent sessions (kb HAND-651..666) ------------------------------------------------------------

    /// The owner's coding sessions on the Mac (`sessions.list`; needs the unlock). Network.
    pub fn sessions_list(&self) -> Result<SessionsList, FerbError> {
        guard(|| Ok(self.core()?.sessions_list()?.conv()))
    }

    /// One session's conversation: the newest `limit` (`after` and `before` 0), those after `after`, or those before
    /// `before`, with the agent's open cards. Network.
    pub fn session_timeline(
        &self,
        session: String,
        after: u64,
        before: u64,
        limit: u32,
    ) -> Result<SessionTimeline, FerbError> {
        guard(|| {
            Ok(self
                .core()?
                .session_timeline(&session, after, before, limit)?
                .conv())
        })
    }

    /// The owner's words into a session: sent when this phone controls it, else `needs_yes` (nothing sent): show the
    /// question with the exact text, then `session_control` on his Yes. Network.
    pub fn session_send(&self, session: String, text: String) -> Result<SessionSent, FerbError> {
        guard(|| Ok(self.core()?.session_send(&session, &text)?.conv()))
    }

    /// The owner's Yes to typing into a session (rule 6, GATE-622): this phone controls it from now on and `text` (may be
    /// empty) is sent; `terminal` is the one he chose of those the session offers (HAND-683; "" for none). Signed by
    /// `signer` (the fingerprint key where registered). Network and the prompt.
    pub fn session_control(
        &self,
        session: String,
        text: String,
        terminal: String,
        signer: Arc<dyn Signer>,
    ) -> Result<SessionSent, FerbError> {
        guard(|| {
            Ok(self
                .core()?
                .session_control(&session, &text, &terminal, signer_of(signer))?
                .conv())
        })
    }

    /// Lets go of a session ("" for all of this phone's); allowed while locked. Network.
    pub fn session_release(&self, session: String) -> Result<u32, FerbError> {
        guard(|| Ok(self.core()?.session_release(&session)?))
    }

    /// Answers the agent's permission prompt or question (`choice`: a card choice's id); signed each time. Network
    /// and the prompt.
    pub fn session_answer(
        &self,
        session: String,
        card: String,
        choice: String,
        signer: Arc<dyn Signer>,
    ) -> Result<bool, FerbError> {
        guard(|| {
            Ok(self
                .core()?
                .session_answer(&session, &card, &choice, signer_of(signer))?)
        })
    }

    /// Stops the agent's turn (this phone must control the session). Network.
    pub fn session_interrupt(&self, session: String) -> Result<bool, FerbError> {
        guard(|| Ok(self.core()?.session_interrupt(&session)?))
    }

    /// The folders a new session may start in; with `folder`, the exact command and its question. Network.
    pub fn session_prepare(
        &self,
        agent: String,
        folder: String,
        text: String,
    ) -> Result<SessionStartPlan, FerbError> {
        guard(|| Ok(self.core()?.session_prepare(&agent, &folder, &text)?.conv()))
    }

    /// Opens a new terminal on the Mac (signed; `agent` "terminal", `text` the command it types, if any: HAND-691).
    /// Answers the command. Network and the prompt.
    pub fn session_start(
        &self,
        agent: String,
        folder: String,
        text: String,
        signer: Arc<dyn Signer>,
    ) -> Result<String, FerbError> {
        guard(|| {
            Ok(self
                .core()?
                .session_start(&agent, &folder, &text, signer_of(signer))?)
        })
    }

    /// Hands a session to FERB with the owner's goal (signed): his words become a FERB task in the chat. Answers the
    /// message. Network and the prompt.
    pub fn session_delegate(
        &self,
        session: String,
        goal: String,
        signer: Arc<dyn Signer>,
    ) -> Result<String, FerbError> {
        guard(|| {
            Ok(self
                .core()?
                .session_delegate(&session, &goal, signer_of(signer))?)
        })
    }

    /// A session's window, live (PHN-454: a direct path or the owner's relay only, in memory only, ended by the lock),
    /// at Watch live's pace. Safe on the UI thread (it only starts the thread).
    pub fn watch_session(
        &self,
        session: String,
        sink: Arc<dyn PreviewSink>,
    ) -> Result<Arc<LivePreview>, FerbError> {
        watch_paced_target(self, Target::Session(session), PREVIEW_LEAST_MS, sink)
    }

    /// Ends insomnia mode now (`insomnia.end`; allowed while locked, no unlock; never queued). Network.
    pub fn end_insomnia(&self) -> Result<InsomniaEnded, FerbError> {
        guard(|| Ok(self.core()?.end_insomnia()?.conv()))
    }

    /// Lock my Mac (`insomnia.lock`, APP-983: only on the owner's word; allowed while locked, no unlock). Network.
    pub fn lock_mac(&self) -> Result<MacLocked, FerbError> {
        guard(|| Ok(self.core()?.lock_mac()?.conv()))
    }

    /// It's fine: the notice `notice_id` is answered (`insomnia.dismiss`; allowed while locked, no unlock). Network.
    pub fn dismiss_insomnia_notice(&self, notice_id: String) -> Result<Insomnia, FerbError> {
        guard(|| Ok(self.core()?.dismiss_insomnia_notice(&notice_id)?.conv()))
    }

    /// One answer of the Mac's camera (APP-983; in memory only). Refused on the public relay before anything is
    /// asked (`NeedsBetterPath`). Network.
    pub fn mac_camera(&self, have: Option<i64>) -> Result<CameraFrame, FerbError> {
        guard(|| Ok(self.core()?.mac_camera(have)?.conv()))
    }

    /// Show me (APP-983): the Mac's camera while the camera view is on screen, asked on a thread of its own at most
    /// every `CAMERA_EVERY_MS`, each answer to `sink`; it ends by itself on the public relay, after the lock, or when
    /// closed (`sink.ended`), and by `stop` (the Mac turns the camera off a few seconds after the last ask). Frames
    /// live in memory only. Safe on the UI thread (it only starts the thread).
    pub fn watch_camera(&self, sink: Arc<dyn CameraSink>) -> Result<Arc<LivePreview>, FerbError> {
        guard(|| {
            let core = self.core()?;
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag = stop.clone();
            let weak = Arc::downgrade(&core);
            drop(core);
            std::thread::Builder::new()
                .name("ferb-camera".into())
                .spawn(move || {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        camera_loop(weak, sink, flag)
                    }));
                })
                .map_err(|_| FerbError::internal())?;
            Ok(Arc::new(LivePreview { stop }))
        })
    }

    /// Marks the conversation read up to the newest entry this phone holds. Network: not on the UI thread.
    pub fn mark_read_latest(&self, conversation: String) -> Result<(), FerbError> {
        guard(|| Ok(self.core()?.mark_read_latest(&conversation)?))
    }

    /// A message with files: this phone's finished uploads on the Mac (`upload`'s ids), kept in the outbox like any
    /// message. Network when online: not on the UI thread.
    pub fn send_with(
        &self,
        text: String,
        attachments: Vec<Attachment>,
    ) -> Result<OutboxItem, FerbError> {
        guard(|| {
            let files: Vec<ferb_mobile::store::Attachment> =
                attachments.into_iter().map(Into::into).collect();
            Ok(self.core()?.send_with(&text, &files)?.conv())
        })
    }

    /// One page of a folder of the Mac (`~` when none); `after`: the last page's `next` ("load more", PHN-589; a
    /// marker of another folder is `Refused`). A key place's folder needs `signer` (the fingerprint-only key, for this
    /// very path, whichever page): without one it is `NeedsUnlock` with link code `fresh_unlock_required`. Network.
    #[uniffi::method(default(after = None))]
    pub fn files(
        &self,
        path: Option<String>,
        signer: Option<Arc<dyn Signer>>,
        after: Option<String>,
    ) -> Result<Folder, FerbError> {
        guard(|| {
            Ok(self
                .core()?
                .files(path.as_deref(), signer.map(signer_of), after.as_deref())?
                .conv())
        })
    }

    /// Every file under a folder, to fetch it whole (direct or the owner's relay only). Network.
    pub fn tree(&self, path: String, signer: Option<Arc<dyn Signer>>) -> Result<Tree, FerbError> {
        guard(|| Ok(self.core()?.tree(&path, signer.map(signer_of))?.conv()))
    }

    /// Fetches one file into `save_to` (app-private storage) from `offset` (what a paused fetch saved: appended).
    /// `id` names it for `cancel_transfer`. A key place's file needs `signer`. Direct or the owner's relay only.
    /// Blocks until done: on a background thread.
    pub fn fetch(
        &self,
        id: String,
        path: String,
        save_to: String,
        offset: u64,
        signer: Option<Arc<dyn Signer>>,
        listener: Arc<dyn TransferListener>,
    ) -> Result<Fetched, FerbError> {
        guard(|| {
            Ok(self
                .core()?
                .fetch(
                    &id,
                    &path,
                    &save_to,
                    offset,
                    signer.map(signer_of),
                    Arc::new(TransferAdapter(listener)),
                )?
                .conv())
        })
    }

    /// Starts a folder sent from the phone: the id its files carry as `group`. Network.
    pub fn put_group(&self, name: String) -> Result<String, FerbError> {
        guard(|| Ok(self.core()?.put_group(&name)?))
    }

    /// Starts a folder sent from the phone: the id its files carry as `group`, and the folder it lands in, said
    /// plainly (`saved_in`, PHN-581). Network.
    pub fn begin_group(&self, name: String) -> Result<GroupStarted, FerbError> {
        guard(|| Ok(self.core()?.begin_group(&name)?.conv()))
    }

    /// Sends the file at `local_path` (app-private storage) to the Mac's upload folder as `name`; `resume` goes on
    /// with an upload that stopped part way (its id). `id` names it for `cancel_transfer` (a pause: the Mac keeps what
    /// arrived for a while). Direct or the owner's relay only. Blocks until done: on a background thread.
    #[allow(clippy::too_many_arguments)]
    pub fn upload(
        &self,
        id: String,
        local_path: String,
        name: String,
        resume: Option<String>,
        group: Option<String>,
        rel: Option<String>,
        listener: Arc<dyn TransferListener>,
    ) -> Result<Uploaded, FerbError> {
        guard(|| {
            Ok(self
                .core()?
                .upload(
                    &id,
                    &local_path,
                    &name,
                    resume.as_deref(),
                    group.as_deref(),
                    rel.as_deref(),
                    Arc::new(TransferAdapter(listener)),
                )?
                .conv())
        })
    }

    /// Fetches the `index`-th file a finished task made (its result card's file) into `save_to`, like `fetch`.
    /// Direct or the owner's relay only. Blocks until done: on a background thread.
    pub fn fetch_task_file(
        &self,
        id: String,
        task_id: String,
        index: u64,
        save_to: String,
        listener: Arc<dyn TransferListener>,
    ) -> Result<Fetched, FerbError> {
        guard(|| {
            Ok(self
                .core()?
                .fetch_task_file(
                    &id,
                    &task_id,
                    index,
                    &save_to,
                    Arc::new(TransferAdapter(listener)),
                )?
                .conv())
        })
    }

    /// The Mac's recent files for this phone: what FERB made and what this phone fetched or sent, newest first
    /// (needs the unlock). Network.
    pub fn recent_files(&self, limit: u32) -> Result<RecentFiles, FerbError> {
        guard(|| Ok(self.core()?.recent_files(limit)?.conv()))
    }

    /// Finds files and folders whose names hold `query` under `path` on the Mac (`~` when none); never by contents;
    /// key places are left out and counted (needs the unlock). A page at a time: `after` is the last page's `next`
    /// ("load more", PHN-589; a marker of another folder or query is `Refused`). Network.
    #[uniffi::method(default(after = None))]
    pub fn search_files(
        &self,
        query: String,
        path: Option<String>,
        after: Option<String>,
    ) -> Result<FoundFiles, FerbError> {
        guard(|| {
            Ok(self
                .core()?
                .search_files(&query, path.as_deref(), after.as_deref())?
                .conv())
        })
    }

    // ---- the phone's own record (T-1214) ----------------------------------------------------------------------

    /// The phone's facts as the app read them now: sent to the Mac at once when online and changed, and again on
    /// each new connection; memory only on both sides. Returns whether it went now. Network: not on the UI thread.
    pub fn report_device(&self, report: DeviceReport) -> Result<bool, FerbError> {
        guard(|| Ok(self.core()?.report_device(report.into())?))
    }

    /// The Mac's open request for one precise fix (its id), if any.
    pub fn location_wanted(&self) -> Result<Option<String>, FerbError> {
        guard(|| Ok(self.core()?.location_wanted()?))
    }

    /// Answers the Mac's request for one precise fix: `fix`, or `why_not` (the owner said no here, Android refused,
    /// no fix in time). Network: not on the UI thread.
    pub fn answer_location(
        &self,
        request_id: String,
        fix: Option<LocationFix>,
        why_not: Option<String>,
    ) -> Result<(), FerbError> {
        guard(|| {
            let fix = fix.map(|f| ferb_mobile::Fix {
                latitude: f.latitude,
                longitude: f.longitude,
                accuracy_m: f.accuracy_m,
            });
            Ok(self
                .core()?
                .answer_location(&request_id, fix, why_not.as_deref())?)
        })
    }

    /// Stops a transfer under way (its `fetch` or `upload` then fails with `Cancelled`).
    pub fn cancel_transfer(&self, id: String) -> Result<(), FerbError> {
        guard(|| Ok(self.core()?.cancel_transfer(&id)?))
    }

    /// One live preview answer (the frame in memory only, PHN-454). Refused on the public relay before anything is
    /// asked (`NeedsBetterPath`). Network.
    pub fn preview(&self, task_id: String, have: Option<i64>) -> Result<PreviewFrame, FerbError> {
        guard(|| Ok(self.core()?.preview(&task_id, have)?.conv()))
    }

    /// A task's live preview: asks for frames on a thread of its own, at the link's pace (FERB.app's rule: at once
    /// after a fresh frame, 200 ms while busy, else a second; never sooner than the link's 200 ms), each answer to
    /// `sink`; it ends by itself on the public relay, after the lock, or when closed (`sink.ended`), and by `stop`.
    /// Frames live in memory only (PHN-454). Safe on the UI thread (it only starts the thread).
    pub fn watch_live(
        &self,
        task_id: String,
        sink: Arc<dyn PreviewSink>,
    ) -> Result<Arc<LivePreview>, FerbError> {
        watch_paced(self, task_id, None, PREVIEW_LEAST_MS, sink)
    }

    /// The work row's small live picture (kb PHN-720..; FERB.app's round pictures by "N active tasks", APP-900): the
    /// same preview as `watch_live`, asked at most once every `every_ms` (never under `THUMBNAIL_LEAST_MS`); the Mac
    /// leaves a frame out when it is the one the phone shows. The same rules: in memory only, refused on the public
    /// relay, ended by the lock. Safe on the UI thread (it only starts the thread).
    pub fn watch_thumbnail(
        &self,
        task_id: String,
        every_ms: u64,
        sink: Arc<dyn PreviewSink>,
    ) -> Result<Arc<LivePreview>, FerbError> {
        watch_paced(self, task_id, None, every_ms.max(THUMBNAIL_LEAST_MS), sink)
    }

    /// A card row's or checklist line's own small live picture (kb PHN-726; FERB.app's RowPreview, APP-888): the
    /// preview of one step (`worker`, its step task id), at the work row's pace and under its rules.
    pub fn watch_step_thumbnail(
        &self,
        task_id: String,
        worker: String,
        every_ms: u64,
        sink: Arc<dyn PreviewSink>,
    ) -> Result<Arc<LivePreview>, FerbError> {
        watch_paced(
            self,
            task_id,
            Some(worker),
            every_ms.max(THUMBNAIL_LEAST_MS),
            sink,
        )
    }

    /// FERB's settings (`settings.get`; needs the unlock). Network.
    pub fn settings(&self) -> Result<Vec<SettingValue>, FerbError> {
        guard(|| Ok(self.core()?.settings()?.conv()))
    }

    /// Changes one of FERB's settings (`settings.set`): signed by `signer` (the fingerprint-only key where the phone
    /// has one, PHN-133). Returns the value the Mac stored, as JSON text. Network and the prompt.
    pub fn set_setting(
        &self,
        key: String,
        value_json: String,
        signer: Arc<dyn Signer>,
    ) -> Result<String, FerbError> {
        guard(|| {
            Ok(self
                .core()?
                .set_setting(&key, &value_json, signer_of(signer))?)
        })
    }

    /// Puts one of FERB's alerts away on every device (`attention.ack`, APP-1106, PHN-791). Network.
    pub fn dismiss_alert(&self, id: String) -> Result<(), FerbError> {
        guard(|| Ok(self.core()?.dismiss_alert(&id)?))
    }

    /// The Mac's tasks, newest first (`tasks.list`; needs the unlock). Network.
    pub fn tasks(&self, limit: u32, before: Option<String>) -> Result<TaskPage, FerbError> {
        guard(|| Ok(self.core()?.tasks(limit, before.as_deref())?.conv()))
    }

    /// What the Mac says about itself and this phone (its name, this phone's pairing date), kept. Network.
    pub fn mac_info(&self) -> Result<MacInfo, FerbError> {
        guard(|| Ok(self.core()?.mac_info()?.conv()))
    }

    /// The last `mac_info` kept (no network).
    pub fn cached_mac_info(&self) -> Result<Option<MacInfo>, FerbError> {
        guard(|| Ok(self.core()?.cached_mac_info()?.conv()))
    }

    /// Registers keys made after pairing (the device key, the fingerprint-only key), signed by `signer` (the
    /// fingerprint-only key where the Mac already holds one, else the lock key). Network and the prompt.
    pub fn register_keys(
        &self,
        device_pub: Option<String>,
        strong_pub: Option<String>,
        signer: Arc<dyn Signer>,
    ) -> Result<(), FerbError> {
        guard(|| {
            Ok(self.core()?.register_keys(
                device_pub.as_deref(),
                strong_pub.as_deref(),
                signer_of(signer),
            )?)
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

/// A live preview at `least_ms` at most (`watch_live`, `watch_thumbnail`).
/// What a live preview shows: a task's work (and one step of it), or a coding session's window (PHN-797).
enum Target {
    Task(String, Option<String>),
    Session(String),
}

fn watch_paced(
    m: &Mobile,
    task_id: String,
    worker: Option<String>,
    least_ms: u64,
    sink: Arc<dyn PreviewSink>,
) -> Result<Arc<LivePreview>, FerbError> {
    watch_paced_target(m, Target::Task(task_id, worker), least_ms, sink)
}

fn watch_paced_target(
    m: &Mobile,
    target: Target,
    least_ms: u64,
    sink: Arc<dyn PreviewSink>,
) -> Result<Arc<LivePreview>, FerbError> {
    guard(|| {
        let core = m.core()?;
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        let weak = Arc::downgrade(&core);
        drop(core);
        std::thread::Builder::new()
            .name("ferb-preview".into())
            .spawn(move || {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    live_loop(weak, target, sink, flag, least_ms)
                }));
            })
            .map_err(|_| FerbError::internal())?;
        Ok(Arc::new(LivePreview { stop }))
    })
}

/// The live preview's loop (`Mobile::watch_live`): one answer at a time, at the link's pace, until stopped, the core
/// closes, or the path or the lock refuses it.
fn live_loop(
    core: std::sync::Weak<ferb_mobile::Mobile>,
    target: Target,
    sink: Arc<dyn PreviewSink>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    least_ms: u64,
) {
    use std::sync::atomic::Ordering;
    let stopped = || stop.load(Ordering::SeqCst);
    let ended = |code: Option<String>, detail: Option<String>| {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink.ended(code, detail)));
    };
    let mut have: Option<i64> = None;
    loop {
        if stopped() {
            return ended(None, None);
        }
        let Some(c) = core.upgrade() else {
            return ended(Some("closed".into()), None);
        };
        if c.is_locked() {
            return ended(
                Some("needs_unlock".into()),
                Some("Unlock to watch live.".into()),
            );
        }
        let answer = match &target {
            Target::Task(task_id, worker) => c.preview_of(task_id, worker.as_deref(), have),
            Target::Session(session) => c.session_preview(session, have),
        };
        drop(c);
        let wait = match answer {
            Ok(f) => {
                let next = f.next_after_ms();
                if f.jpeg.is_some() {
                    have = f.seq;
                } else if f.gone {
                    have = None;
                }
                if stopped() {
                    return ended(None, None);
                }
                let frame: PreviewFrame = f.conv();
                let _ =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink.frame(frame)));
                next.max(least_ms)
            }
            Err(e) => {
                let e = FerbError::from(e);
                match e {
                    FerbError::NeedsBetterPath { .. }
                    | FerbError::NeedsUnlock { .. }
                    | FerbError::Closed { .. } => {
                        return ended(Some(e.code().to_string()), Some(e.detail().to_string()));
                    }
                    FerbError::Refused { .. } if e.link_code() != Some("preview_too_soon") => {
                        return ended(Some(e.code().to_string()), Some(e.detail().to_string()));
                    }
                    _ => PREVIEW_RETRY_MS,
                }
            }
        };
        // wait in short steps, so a stop is heard at once
        let mut left = wait;
        while left > 0 && !stopped() {
            let step = left.min(50);
            std::thread::sleep(std::time::Duration::from_millis(step));
            left -= step;
        }
    }
}

/// The camera's loop (`Mobile::watch_camera`): one answer at a time, at most every `CAMERA_EVERY_MS`, until stopped,
/// the core closes, or the path or the lock refuses it.
fn camera_loop(
    core: std::sync::Weak<ferb_mobile::Mobile>,
    sink: Arc<dyn CameraSink>,
    stop: Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    let stopped = || stop.load(Ordering::SeqCst);
    let ended = |code: Option<String>, detail: Option<String>| {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink.ended(code, detail)));
    };
    let mut have: Option<i64> = None;
    loop {
        if stopped() {
            return ended(None, None);
        }
        let Some(c) = core.upgrade() else {
            return ended(Some("closed".into()), None);
        };
        if c.is_locked() {
            return ended(
                Some("needs_unlock".into()),
                Some("Unlock to see your Mac's camera.".into()),
            );
        }
        let answer = c.mac_camera(have);
        drop(c);
        let wait = match answer {
            Ok(f) => {
                if f.jpeg.is_some() {
                    have = f.seq;
                }
                if stopped() {
                    return ended(None, None);
                }
                let frame: CameraFrame = f.conv();
                let _ =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink.frame(frame)));
                CAMERA_EVERY_MS
            }
            Err(e) => {
                let e = FerbError::from(e);
                match e {
                    FerbError::NeedsBetterPath { .. }
                    | FerbError::NeedsUnlock { .. }
                    | FerbError::Closed { .. } => {
                        return ended(Some(e.code().to_string()), Some(e.detail().to_string()));
                    }
                    FerbError::Refused { .. } if e.link_code() != Some("preview_too_soon") => {
                        return ended(Some(e.code().to_string()), Some(e.detail().to_string()));
                    }
                    _ => PREVIEW_RETRY_MS,
                }
            }
        };
        // wait in short steps, so a stop is heard at once
        let mut left = wait.max(PREVIEW_LEAST_MS);
        while left > 0 && !stopped() {
            let step = left.min(50);
            std::thread::sleep(std::time::Duration::from_millis(step));
            left -= step;
        }
    }
}
