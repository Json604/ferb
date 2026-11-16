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
    Attachment, Cancelled, Conv, ConversationInfo, Fetched, Folder, LinkState, MacInfo, OlderState,
    OutboxItem, PhoneStatus, PreviewFrame, SettingValue, TaskPage, TranscriptView, Tree, Uploaded,
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

/// The least time between two frame requests: the link's least time between two pictures on a direct path or the
/// owner's relay (ferb-link `Limits::good_preview_every_ms`, 200 ms); asking sooner is refused there.
pub const PREVIEW_LEAST_MS: u64 = 200;
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
            // the app is unlocked: the conversation is read again (PHN-125)
            core.unlocked();
            // what waited for the unlock goes now; a failure of that is the sync's own, reported to the listener
            let _ = core.sync();
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

    /// One folder of the Mac (`~` when none). A key place's folder needs `signer` (the fingerprint-only key, for this
    /// very path): without one it is `NeedsUnlock` with link code `fresh_unlock_required`. Network.
    pub fn files(
        &self,
        path: Option<String>,
        signer: Option<Arc<dyn Signer>>,
    ) -> Result<Folder, FerbError> {
        guard(|| {
            Ok(self
                .core()?
                .files(path.as_deref(), signer.map(signer_of))?
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
        guard(|| {
            let core = self.core()?;
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag = stop.clone();
            let weak = Arc::downgrade(&core);
            drop(core);
            std::thread::Builder::new()
                .name("ferb-preview".into())
                .spawn(move || {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        live_loop(weak, task_id, sink, flag)
                    }));
                })
                .map_err(|_| FerbError::internal())?;
            Ok(Arc::new(LivePreview { stop }))
        })
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

/// The live preview's loop (`Mobile::watch_live`): one answer at a time, at the link's pace, until stopped, the core
/// closes, or the path or the lock refuses it.
fn live_loop(
    core: std::sync::Weak<ferb_mobile::Mobile>,
    task_id: String,
    sink: Arc<dyn PreviewSink>,
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
                Some("Unlock to watch live.".into()),
            );
        }
        let answer = c.preview(&task_id, have);
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
                next.max(PREVIEW_LEAST_MS)
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
