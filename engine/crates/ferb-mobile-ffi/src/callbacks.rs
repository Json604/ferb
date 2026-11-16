//! What the app supplies (UniFFI foreign traits: Kotlin interfaces, Swift protocols) and the adapters that hand each
//! one to the core.
//!
//! A foreign callback that throws where its Rust signature has no error makes UniFFI panic on the calling thread;
//! every adapter here catches that, so an app's bug in a callback never takes down the pump or the core's threads.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use ferb_mobile::Spawner;
use ferb_mobile::transport::{LinkFailure, WatchHandle as CoreWatchHandle};

use crate::error::TransportError;
use crate::records::{Change, Conv};

/// The time now as the engine writes times: ISO 8601 with an offset (`2026-11-16T09:30:00+05:30`).
#[uniffi::export(foreign)]
pub trait Clock: Send + Sync {
    fn now_iso(&self) -> String;
}

/// A new id for a message's submission (a random UUID).
#[uniffi::export(foreign)]
pub trait Ids: Send + Sync {
    fn new_id(&self) -> String;
}

/// What changed, so the app reads it again. Called on the library's threads (the link's pump, a sync), with nothing
/// locked: post to the UI thread and return; never call back into a blocking method from here.
#[uniffi::export(foreign)]
pub trait Listener: Send + Sync {
    fn changed(&self, change: Change);
}

/// Signs one exact text with the phone's lock-bound key after the fingerprint prompt (base64 of the signature), or
/// none when the owner cancelled or the key is gone. Called on the thread that called `Mobile.answer`.
#[uniffi::export(foreign)]
pub trait Signer: Send + Sync {
    fn sign(&self, payload: String) -> Option<String>;
}

/// Signs with the phone's device key (hardware-held, no prompt): the link proves a new connection comes from this
/// very phone (ferb-link's `prove` event). Called on a thread of its own; must return within 3 s.
#[uniffi::export(foreign)]
pub trait DeviceSigner: Send + Sync {
    fn sign(&self, payload: String) -> Option<String>;
}

/// A way to the Mac the app supplies instead of the link (a fake Mac in tests, or a later transport). JSON as text,
/// as ferb-mobile's `Transport`. Each call is bounded by the transport itself.
#[uniffi::export(foreign)]
pub trait Transport: Send + Sync {
    /// One call: the reply's JSON, or the failure. `proof_json`: `{payload, sig}` for a call the Mac checks.
    fn call(
        &self,
        method: String,
        params_json: String,
        proof_json: Option<String>,
    ) -> Result<String, TransportError>;
    /// `chat.send`: the stream's `ack` frame as JSON once it arrives, or the refusal.
    fn send(&self, params_json: String) -> Result<String, TransportError>;
    /// Starts `chat.entries.watch`; frames go to `sink` until it ends or `close_watch` is called. Returns an id of
    /// the transport's choosing for `close_watch`.
    fn watch(&self, params_json: String, sink: Arc<WatchSink>) -> Result<u64, TransportError>;
    /// Stops a watch (idempotent, bounded).
    fn close_watch(&self, watch_id: u64);
    /// `link.get` (a file or a picture): the reply's JSON (`meta`, `received`, and `saved_to` or base64 `data`).
    /// `id` names it for `cancel`; progress goes to `progress`. With `save_to` the bytes go into that file (appended when
    /// `append`).
    fn get(
        &self,
        id: String,
        params_json: String,
        save_to: Option<String>,
        append: bool,
        progress: Arc<TransferProgress>,
    ) -> Result<String, TransportError>;
    /// `link.put`: the file at `path` to the Mac; `opts_json` carries `name`, `resume`, `group`, `rel`. The reply's JSON
    /// (`upload_id`, `saved`, `size`, `sha256`); the go-ahead and progress go to `progress`.
    fn put(
        &self,
        id: String,
        path: String,
        opts_json: String,
        progress: Arc<TransferProgress>,
    ) -> Result<String, TransportError>;
    /// Stops the transfer `id`: its call then fails with code `cancelled`.
    fn cancel(&self, id: String);
}

/// Where an app-supplied transport reports a transfer's progress.
#[derive(uniffi::Object)]
pub struct TransferProgress {
    inner: Arc<dyn ferb_mobile::transport::TransferSink>,
}

#[uniffi::export]
impl TransferProgress {
    pub fn progress(&self, done: u64, of: u64) {
        let _ = catch_unwind(AssertUnwindSafe(|| self.inner.progress(done, of)));
    }
    pub fn accepted(&self, upload_id: String, offset: u64) {
        let _ = catch_unwind(AssertUnwindSafe(|| self.inner.accepted(&upload_id, offset)));
    }
}

/// What the app hears of one of its transfers (a file fetched or sent): bytes so far of the whole, and for an upload
/// the Mac's go-ahead (its upload id, where it goes on from) before the first byte. On the link's threads: post and
/// return.
#[uniffi::export(foreign)]
pub trait TransferListener: Send + Sync {
    fn progress(&self, done: u64, of: u64);
    fn accepted(&self, upload_id: String, offset: u64);
}

pub(crate) struct TransferAdapter(pub Arc<dyn TransferListener>);

impl ferb_mobile::transport::TransferSink for TransferAdapter {
    fn progress(&self, done: u64, of: u64) {
        let _ = catch_unwind(AssertUnwindSafe(|| self.0.progress(done, of)));
    }
    fn accepted(&self, upload_id: &str, offset: u64) {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            self.0.accepted(upload_id.to_string(), offset)
        }));
    }
}

/// Where an app-supplied transport delivers a watch's frames.
#[derive(uniffi::Object)]
pub struct WatchSink {
    inner: Arc<dyn ferb_mobile::WatchSink>,
}

#[uniffi::export]
impl WatchSink {
    /// One frame's JSON (`change`, `caught_up`, `live`, `behind`, `reset_required`, `cursor_ahead`).
    pub fn frame(&self, frame_json: String) {
        let _ = catch_unwind(AssertUnwindSafe(|| self.inner.frame(frame_json)));
    }

    /// The stream ended: by itself (no code) or with the link's failure code and words.
    pub fn ended(&self, code: Option<String>, detail: Option<String>) {
        let failure = code.map(|c| LinkFailure::new(&c, detail.unwrap_or_default()));
        let _ = catch_unwind(AssertUnwindSafe(|| self.inner.ended(failure)));
    }
}

// ---- adapters ----------------------------------------------------------------------------------------------------

pub(crate) struct ClockAdapter(pub Arc<dyn Clock>);

impl ferb_mobile::Clock for ClockAdapter {
    fn now_iso(&self) -> String {
        catch_unwind(AssertUnwindSafe(|| self.0.now_iso())).unwrap_or_else(|_| utc_now_iso())
    }
}

pub(crate) struct IdsAdapter(pub Arc<dyn Ids>, pub AtomicU64);

impl ferb_mobile::Ids for IdsAdapter {
    fn new_id(&self) -> String {
        catch_unwind(AssertUnwindSafe(|| self.0.new_id())).unwrap_or_else(|_| {
            // the app's generator threw: an id that is still unique on this phone
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            format!("local-{nanos:x}-{}", self.1.fetch_add(1, Ordering::SeqCst))
        })
    }
}

pub(crate) struct ListenerAdapter(pub Arc<dyn Listener>);

impl ferb_mobile::Listener for ListenerAdapter {
    fn changed(&self, change: ferb_mobile::Change) {
        let _ = catch_unwind(AssertUnwindSafe(|| self.0.changed(change.conv())));
    }
}

pub(crate) struct SignerAdapter(pub Arc<dyn Signer>);

impl ferb_mobile::Signer for SignerAdapter {
    fn sign(&self, payload: &str) -> Option<String> {
        catch_unwind(AssertUnwindSafe(|| self.0.sign(payload.to_string())))
            .ok()
            .flatten()
    }
}

/// The core's own work (a sync when the link comes online, an older page offered again) on a thread per job, a
/// panic in it caught.
pub(crate) struct GuardedSpawner;

impl Spawner for GuardedSpawner {
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
        let _ = std::thread::Builder::new()
            .name("ferb-mobile".into())
            .spawn(move || {
                let _ = catch_unwind(AssertUnwindSafe(job));
            });
    }
}

pub(crate) struct TransportAdapter(pub Arc<dyn Transport>);

fn thrown() -> LinkFailure {
    LinkFailure::new(
        ferb_mobile::transport::codes::LINK_INTERNAL_ERROR,
        "The app's transport failed inside.",
    )
}

impl ferb_mobile::Transport for TransportAdapter {
    fn call(
        &self,
        method: &str,
        params_json: &str,
        proof_json: Option<&str>,
    ) -> Result<String, LinkFailure> {
        catch_unwind(AssertUnwindSafe(|| {
            self.0
                .call(
                    method.to_string(),
                    params_json.to_string(),
                    proof_json.map(str::to_string),
                )
                .map_err(LinkFailure::from)
        }))
        .unwrap_or_else(|_| Err(thrown()))
    }

    fn send(&self, params_json: &str) -> Result<String, LinkFailure> {
        catch_unwind(AssertUnwindSafe(|| {
            self.0
                .send(params_json.to_string())
                .map_err(LinkFailure::from)
        }))
        .unwrap_or_else(|_| Err(thrown()))
    }

    fn watch(
        &self,
        params_json: &str,
        sink: Arc<dyn ferb_mobile::WatchSink>,
    ) -> Result<Box<dyn CoreWatchHandle>, LinkFailure> {
        let sink = Arc::new(WatchSink { inner: sink });
        let id = catch_unwind(AssertUnwindSafe(|| {
            self.0
                .watch(params_json.to_string(), sink)
                .map_err(LinkFailure::from)
        }))
        .unwrap_or_else(|_| Err(thrown()))?;
        Ok(Box::new(ForeignWatch {
            transport: self.0.clone(),
            id,
        }))
    }

    fn get(
        &self,
        id: &str,
        params_json: &str,
        save_to: Option<&str>,
        append: bool,
        sink: Arc<dyn ferb_mobile::transport::TransferSink>,
    ) -> Result<String, LinkFailure> {
        let progress = Arc::new(TransferProgress { inner: sink });
        catch_unwind(AssertUnwindSafe(|| {
            self.0
                .get(
                    id.to_string(),
                    params_json.to_string(),
                    save_to.map(str::to_string),
                    append,
                    progress,
                )
                .map_err(LinkFailure::from)
        }))
        .unwrap_or_else(|_| Err(thrown()))
    }

    fn put(
        &self,
        id: &str,
        path: &str,
        opts_json: &str,
        sink: Arc<dyn ferb_mobile::transport::TransferSink>,
    ) -> Result<String, LinkFailure> {
        let progress = Arc::new(TransferProgress { inner: sink });
        catch_unwind(AssertUnwindSafe(|| {
            self.0
                .put(
                    id.to_string(),
                    path.to_string(),
                    opts_json.to_string(),
                    progress,
                )
                .map_err(LinkFailure::from)
        }))
        .unwrap_or_else(|_| Err(thrown()))
    }

    fn cancel(&self, id: &str) {
        let _ = catch_unwind(AssertUnwindSafe(|| self.0.cancel(id.to_string())));
    }
}

struct ForeignWatch {
    transport: Arc<dyn Transport>,
    id: u64,
}

impl CoreWatchHandle for ForeignWatch {
    fn close(&self) {
        let _ = catch_unwind(AssertUnwindSafe(|| self.transport.close_watch(self.id)));
    }
}

/// UTC now as `YYYY-MM-DDTHH:MM:SS+00:00`: only when the app's clock threw.
pub(crate) fn utc_now_iso() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // days since 1970-01-01 to a civil date (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}+00:00",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}
