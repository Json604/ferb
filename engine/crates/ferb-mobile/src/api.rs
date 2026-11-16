//! The phone's core as one object (`Mobile`), what slice S8 wraps for Kotlin and Swift. README.md, "The API",
//! describes it and its threading model.
//!
//! The data lives on the Mac (ENG-10): the phone reads the current conversation like from a server (the newest page,
//! older pages on demand, the changes after a cursor, the watch), keeps what it fetched in its store until "Forget this
//! Mac" (PHN-408), folds it as FERB.app does, layers the watch's live frames over it, and keeps messages written while
//! the Mac is unreachable in a visible, cancellable outbox sent in order when the link returns (PHN-401), each with a
//! submission id the engine dedupes by (ENG-14), so a retry after an unknown outcome never makes a second message.
//! Answers are never queued (PHN-448, CONTRACT "The phone never queues an answer").

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use base64::Engine;
use serde::Serialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::entries::{Applied, EntrySet};
use crate::entry::ChatEntry;
use crate::fold::{Item, QuestionItem};
use crate::live::{LiveState, LiveView};
use crate::store::{Cursor, Older, OutboxRow, Store, StoreError};
use crate::transport::{FailureKind, LinkFailure, Transport, WatchHandle, WatchSink};

// ---- configuration and the app's seams ------------------------------------------------------------------------------

/// What the app tells the core at open.
#[derive(Debug, Clone)]
pub struct Config {
    /// This phone's id among the conversation's clients (ENG-14): made once by the app and kept; it names the
    /// phone's turns (`client_id`) and its marks.
    pub client_id: String,
    /// The phone's offset from UTC now (seconds): whether a later answer was said on its turn's day.
    pub utc_offset_s: i32,
    /// Entries per page: the link's `shared_page_max` (50, PHN-461), the engine's own page size.
    pub page_limit: u32,
    /// A reply's largest size (bytes): the link's control frame (64 KiB) less the 1 KiB the bridge keeps, as the
    /// bridge caps a public-relay page itself (ferb-link bridge.rs `public_relay_rules`).
    pub max_bytes: u32,
    /// Characters per piece of a long entry read whole (`chat.entries.get max_chars`): at most 4 bytes each, so a
    /// piece and its entry fit one control frame.
    pub piece_chars: u32,
}

impl Config {
    pub fn new(client_id: impl Into<String>) -> Config {
        Config {
            client_id: client_id.into(),
            utc_offset_s: 0,
            page_limit: 50,
            max_bytes: 63 * 1024,
            piece_chars: 12_000,
        }
    }
}

/// The time now, as the engine writes times (ISO 8601 with an offset). Injected so tests use a fixed clock.
pub trait Clock: Send + Sync {
    fn now_iso(&self) -> String;
}

/// New ids (a message's submission id). Injected so tests are repeatable; the app gives random UUIDs.
pub trait Ids: Send + Sync {
    fn new_id(&self) -> String;
}

/// Runs work the core starts by itself (a sync when the link comes online, an older page offered again when the
/// path improves): the app's default spawns a thread; tests queue the jobs and run them.
pub trait Spawner: Send + Sync {
    fn spawn(&self, job: Box<dyn FnOnce() + Send>);
}

/// Signs one exact text with the phone's lock-bound key (after its fingerprint prompt, PHN-417): the core never
/// holds a key. None: the owner cancelled, or the key is gone.
pub trait Signer: Send + Sync {
    fn sign(&self, payload: &str) -> Option<String>;
}

/// What changed, so the app reads it again. Called on the core's threads with nothing locked.
pub trait Listener: Send + Sync {
    fn changed(&self, change: Change);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A conversation's stored entries changed: read its transcript again.
    Transcript {
        conversation: String,
    },
    /// Only what is live changed (typing, a streamed line, a step's state).
    Live {
        conversation: String,
    },
    /// The list of conversations or which one is current.
    Conversations,
    Outbox,
    Link,
    /// Whether older entries can be fetched changed.
    Older {
        conversation: String,
    },
    /// "Forget this Mac" ran or the core closed: every call now answers `closed`.
    Closed,
}

/// Spawns a thread per job.
pub struct ThreadSpawner;

impl Spawner for ThreadSpawner {
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
        let _ = std::thread::Builder::new()
            .name("ferb-mobile".into())
            .spawn(job);
    }
}

/// The app's seams together.
#[derive(Clone)]
pub struct Env {
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn Ids>,
    pub spawner: Arc<dyn Spawner>,
    pub listener: Arc<dyn Listener>,
}

// ---- errors -----------------------------------------------------------------------------------------------------------

/// The stable codes of every failure the core returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Forgotten or closed: open again.
    Closed,
    /// The key is not 32 bytes.
    BadKey,
    /// The store is not this key's.
    WrongKey,
    /// The store was written by a newer app.
    StoreNewer,
    /// The store failed.
    Store,
    /// No link to the Mac now (not connected, offline, or the link's own failure): `link_code` says which.
    Offline,
    /// The public relay refuses this (PHN-444): available when connected directly or through the owner's relay.
    NeedsBetterPath,
    /// The phone must be unlocked toward the Mac first.
    NeedsUnlock,
    /// The Mac refused it: `link_code` and the words say why.
    Refused,
    /// A hidden answer needs the phone's key, and the call had no signer.
    NeedsSigner,
    /// The owner cancelled the fingerprint prompt (or the key is gone): nothing was sent.
    NotSigned,
    /// No such outbox message or question.
    NotFound,
    /// The message is being sent now: it can't be cancelled at this moment.
    InFlight,
    /// The Mac's reply could not be read.
    BadReply,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Closed => "closed",
            ErrorCode::BadKey => "bad_key",
            ErrorCode::WrongKey => "wrong_key",
            ErrorCode::StoreNewer => "store_newer",
            ErrorCode::Store => "store",
            ErrorCode::Offline => "offline",
            ErrorCode::NeedsBetterPath => "needs_better_path",
            ErrorCode::NeedsUnlock => "needs_unlock",
            ErrorCode::Refused => "refused",
            ErrorCode::NeedsSigner => "needs_signer",
            ErrorCode::NotSigned => "not_signed",
            ErrorCode::NotFound => "not_found",
            ErrorCode::InFlight => "in_flight",
            ErrorCode::BadReply => "bad_reply",
        }
    }
}

/// One error type for every call: a stable code, the words to show, and the link's own code when it came from there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileError {
    pub code: ErrorCode,
    pub message: String,
    pub link_code: Option<String>,
}

impl MobileError {
    fn new(code: ErrorCode, message: impl Into<String>) -> MobileError {
        MobileError {
            code,
            message: message.into(),
            link_code: None,
        }
    }
    fn closed() -> MobileError {
        MobileError::new(
            ErrorCode::Closed,
            "This phone's copy of FERB's chat is closed.",
        )
    }
    fn offline() -> MobileError {
        MobileError::new(ErrorCode::Offline, "FERB's Mac can't be reached now.")
    }
    fn from_link(f: LinkFailure) -> MobileError {
        let code = match f.kind() {
            FailureKind::Transient => ErrorCode::Offline,
            FailureKind::NeedsUnlock => ErrorCode::NeedsUnlock,
            FailureKind::NeedsBetterPath => ErrorCode::NeedsBetterPath,
            FailureKind::Refused => ErrorCode::Refused,
        };
        MobileError {
            code,
            message: f.message,
            link_code: Some(f.code),
        }
    }
}

impl std::fmt::Display for MobileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for MobileError {}

impl From<StoreError> for MobileError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::BadKey => {
                MobileError::new(ErrorCode::BadKey, "The store's key must be 32 bytes.")
            }
            StoreError::WrongKey => MobileError::new(
                ErrorCode::WrongKey,
                "The store can't be opened with this key.",
            ),
            StoreError::Newer(v) => MobileError::new(
                ErrorCode::StoreNewer,
                format!("The store was written by a newer app (schema {v})."),
            ),
            StoreError::Closed => MobileError::closed(),
            StoreError::Sql(m) => MobileError::new(ErrorCode::Store, m),
        }
    }
}

pub type MobileResult<T> = Result<T, MobileError>;

// ---- what the app reads -----------------------------------------------------------------------------------------------

/// The path the link is on now (ferb-link's tiers, PHN-418/PHN-453).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkPath {
    Offline,
    Direct,
    OwnRelay,
    PublicRelay,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkState {
    pub path: LinkPath,
    /// Pictures, files and older pages cross this path (false on the public relay, PHN-444).
    pub bulk_allowed: bool,
    /// The owner's relay is down and the link fell back to the public one (PHN-425): the link's own words.
    pub relay_note: Option<String>,
    /// The watch is running: changes and live frames arrive as they happen.
    pub watching: bool,
}

/// Whether older entries can be fetched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OlderState {
    /// Nothing fetched yet.
    Unknown,
    /// Older entries exist: `load_older` fetches the next page.
    Available,
    /// Everything back to the conversation's start is here.
    Complete,
    /// The public relay refuses older pages (PHN-444): "available when connected directly or through your relay".
    /// Fetched by itself once the path carries them (the link's `bulk {allowed: true}`).
    NeedsBetterPath { message: String },
}

/// The words a phone shows for an older page the public relay refuses.
pub const OLDER_NEEDS_BETTER_PATH: &str =
    "Older messages are available when connected directly or through your relay.";

/// A message in the outbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutboxItem {
    pub id: String,
    pub text: String,
    pub created_at: String,
    /// waiting (for the link, or for the phone's unlock: `waiting_for`), sending, failed (`reason`).
    pub state: String,
    pub waiting_for: Option<String>,
    pub reason: Option<String>,
    /// A try's outcome was unknown (the link dropped after sending): the Mac may already have it. Sending again is
    /// safe (the engine dedupes by its id); cancelling then may leave the Mac's copy, which the chat then shows.
    pub tried: bool,
}

/// What cancelling did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Cancelled {
    /// An earlier try's outcome was unknown: the Mac may have the message already.
    pub may_have_arrived: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConversationInfo {
    pub id: String,
    pub current: bool,
    pub entries: i64,
    pub updated_at: String,
}

/// A conversation as the app draws it: the folded transcript, its questions, what is live, the outbox (on the
/// current conversation) and whether older entries can be fetched.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TranscriptView {
    pub conversation: String,
    pub items: Vec<Item>,
    pub questions: Vec<QuestionItem>,
    pub live: Vec<LiveView>,
    pub outbox: Vec<OutboxItem>,
    pub older: OlderState,
}

/// An answer to one of FERB's questions. Never queued: it goes now or not at all.
pub enum Answer {
    /// A choice (`chat.approve {granted, choice?}`).
    Approve { granted: bool, choice: Option<i64> },
    /// Free words (`chat.answer {text}`; empty declines).
    Words { text: String },
    /// The owner's words for a safety check's task (`chat.correct`, GATE-510).
    Correct { text: String },
    /// A hidden input (a login code, the admin password, a secret): sent on the sealed path with a proof whose
    /// signed text carries only the value's SHA-256 (CONTRACT "Hidden answers"); never logged, stored or echoed.
    Hidden { secret: String },
}

impl std::fmt::Debug for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Answer::Approve { granted, choice } => {
                write!(f, "Approve {{ granted: {granted}, choice: {choice:?} }}")
            }
            Answer::Words { .. } => f.write_str("Words { .. }"),
            Answer::Correct { .. } => f.write_str("Correct { .. }"),
            Answer::Hidden { .. } => f.write_str("Hidden { <hidden> }"),
        }
    }
}

impl Drop for Answer {
    fn drop(&mut self) {
        if let Answer::Hidden { secret } = self {
            secret.zeroize();
        }
    }
}

// ---- the core ---------------------------------------------------------------------------------------------------------

struct Conv {
    set: EntrySet,
    live: LiveState,
    older: OlderState,
}

struct State {
    store: Option<Store>,
    convs: HashMap<String, Conv>,
    current: Option<String>,
    link: LinkState,
    transport: Option<Arc<dyn Transport>>,
    watch: Option<(u64, String, Box<dyn WatchHandle>)>,
    generation: u64,
    /// Conversations whose older page waits for a path that carries it.
    older_waiting: HashSet<String>,
}

struct Shared {
    state: Mutex<State>,
    /// One sync at a time (reads, the watch's start).
    sync_lock: Mutex<()>,
    /// One outbox flush at a time, so messages go in order and once.
    outbox_lock: Mutex<()>,
    config: Config,
    env: Env,
    path: PathBuf,
}

/// The phone's core. Every method is blocking and bounded (store work, and at most the transport's own call deadline
/// per network call); none holds a lock across a network call. `close` (or `forget_this_mac`) ends it.
pub struct Mobile {
    shared: Arc<Shared>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn parse(reply: &str) -> MobileResult<Value> {
    serde_json::from_str(reply)
        .map_err(|_| MobileError::new(ErrorCode::BadReply, "FERB's reply could not be read."))
}

/// A JSON object built with its keys in sorted order, so its text is the same whether serde_json keeps insertion
/// order or sorts (the signed payloads; ferb-link writes them sorted).
fn sorted(pairs: Vec<(&str, Value)>) -> Value {
    let mut pairs = pairs;
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

/// The text a lock-bound key signs for an answer that needs the phone's unlock (ferb-link `Phone::answer_payload`).
pub fn answer_payload(approval_id: &str, challenge: &str, method: &str, params: &Value) -> String {
    sorted(vec![
        ("approval_id", json!(approval_id)),
        ("challenge", json!(challenge)),
        ("method", json!(method)),
        ("params", params.clone()),
    ])
    .to_string()
}

/// The text a lock-bound key signs for a hidden answer: the value only as its SHA-256 (base64url, no padding), so
/// what is signed and sent as proof never carries it (ferb-link `Phone::sealed_payload`).
pub fn sealed_payload(approval_id: &str, challenge: &str, text: &str) -> String {
    let digest =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(text.as_bytes()));
    sorted(vec![
        ("approval_id", json!(approval_id)),
        ("challenge", json!(challenge)),
        ("method", json!("chat.answer")),
        ("sealed", json!(digest)),
    ])
    .to_string()
}

impl Mobile {
    /// Opens the phone's store at `path` with the app's key (32 bytes, zeroized here) and the app's seams. A message
    /// left `sending` by a process that ended goes back to waiting (its outcome is unknown; sending again is safe).
    pub fn open(
        path: impl Into<PathBuf>,
        key: Vec<u8>,
        config: Config,
        env: Env,
    ) -> MobileResult<Mobile> {
        let path = path.into();
        let store = Store::open(&path, key)?;
        for mut row in store.outbox()? {
            if row.state == "sending" {
                row.state = "waiting".into();
                row.attempts = row.attempts.max(1);
                store.outbox_update(&row)?;
            }
        }
        let current = store.meta("current")?;
        Ok(Mobile {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    store: Some(store),
                    convs: HashMap::new(),
                    current,
                    link: LinkState {
                        path: LinkPath::Offline,
                        bulk_allowed: false,
                        relay_note: None,
                        watching: false,
                    },
                    transport: None,
                    watch: None,
                    generation: 0,
                    older_waiting: HashSet::new(),
                }),
                sync_lock: Mutex::new(()),
                outbox_lock: Mutex::new(()),
                config,
                env,
                path,
            }),
        })
    }

    /// Plugs the link in (or a new one after the app restarted it). Nothing is called until the link says it is
    /// online (`link_event`) or the app calls `sync`.
    pub fn connect(&self, transport: Arc<dyn Transport>) -> MobileResult<()> {
        let mut st = self.shared.open_state()?;
        st.transport = Some(transport);
        Ok(())
    }

    /// Unplugs the link: the watch stops and nothing live stays.
    pub fn disconnect(&self) {
        let watch = {
            let mut st = lock(&self.shared.state);
            st.transport = None;
            st.link.path = LinkPath::Offline;
            st.link.watching = false;
            for c in st.convs.values_mut() {
                c.live.reset();
            }
            st.watch.take()
        };
        if let Some((_, _, h)) = watch {
            h.close();
        }
        self.shared.notify(Change::Link);
    }

    /// One event of the link's own (its `state`, `bulk` and `relay_state` events, as JSON). Coming online starts a
    /// sync (on the spawner); a path that carries older pages again fetches a refused one once.
    pub fn link_event(&self, event_json: &str) {
        Shared::link_event(&self.shared, event_json);
    }

    /// Reads what changed on the Mac and starts the watch: the current conversation (a new one starts from its
    /// newest page), the changes after the cursor (a cursor of another epoch or past the head reads again from the
    /// newest page), entries whose words a reply cut, then the outbox. Blocking; bounded by the transport's calls.
    pub fn sync(&self) -> MobileResult<()> {
        Shared::sync(&self.shared)
    }

    /// The conversations held, the current one marked.
    pub fn conversations(&self) -> MobileResult<Vec<ConversationInfo>> {
        let st = self.shared.open_state()?;
        let store = st.store.as_ref().ok_or_else(MobileError::closed)?;
        Ok(store
            .conversations()?
            .into_iter()
            .map(|c| ConversationInfo {
                current: st.current.as_deref() == Some(c.id.as_str()),
                id: c.id,
                entries: c.entries,
                updated_at: c.updated_at,
            })
            .collect())
    }

    /// The conversation FERB writes in now (as the Mac said at the last sync).
    pub fn current_conversation(&self) -> MobileResult<Option<String>> {
        Ok(self.shared.open_state()?.current.clone())
    }

    /// A conversation as the app draws it.
    pub fn transcript(&self, conversation: &str) -> MobileResult<TranscriptView> {
        let mut st = self.shared.open_state()?;
        let tz = self.shared.config.utc_offset_s;
        let current = st.current.as_deref() == Some(conversation);
        let outbox = if current {
            outbox_items(&st)?
        } else {
            Vec::new()
        };
        let conv = st.conv(conversation)?;
        let t = conv.set.transcript(tz);
        let live = conv.live.view(&t);
        Ok(TranscriptView {
            conversation: conversation.to_string(),
            items: t.items,
            questions: t.questions,
            live,
            outbox,
            older: conv.older.clone(),
        })
    }

    /// The outbox, oldest first.
    pub fn outbox(&self) -> MobileResult<Vec<OutboxItem>> {
        let st = self.shared.open_state()?;
        outbox_items(&st)
    }

    /// Writes a message: it goes to the outbox at once (visible, cancellable), and is sent now when the link is up.
    pub fn send(&self, text: &str) -> MobileResult<OutboxItem> {
        let item = {
            let st = self.shared.open_state()?;
            let store = st.store.as_ref().ok_or_else(MobileError::closed)?;
            let row = store.outbox_add(
                &self.shared.env.ids.new_id(),
                text,
                &self.shared.env.clock.now_iso(),
            )?;
            outbox_item(&row)
        };
        self.shared.notify(Change::Outbox);
        if self.shared.online() {
            Shared::flush(&self.shared);
        }
        let st = self.shared.open_state()?;
        Ok(outbox_items(&st)?
            .into_iter()
            .find(|i| i.id == item.id)
            .unwrap_or(item))
    }

    /// Sends what waits in the outbox, in order (also done by `sync`).
    pub fn flush_outbox(&self) -> MobileResult<()> {
        drop(self.shared.open_state()?);
        Shared::flush(&self.shared);
        Ok(())
    }

    /// Takes a waiting or failed message out of the outbox. One being sent right now can't be.
    pub fn cancel(&self, id: &str) -> MobileResult<Cancelled> {
        let out = {
            let st = self.shared.open_state()?;
            let store = st.store.as_ref().ok_or_else(MobileError::closed)?;
            let row = store.outbox_get(id)?.ok_or_else(|| {
                MobileError::new(ErrorCode::NotFound, "That message isn't in the outbox.")
            })?;
            if row.state == "sending" {
                return Err(MobileError::new(
                    ErrorCode::InFlight,
                    "That message is being sent now.",
                ));
            }
            store.outbox_remove(id)?;
            Cancelled {
                may_have_arrived: row.state == "waiting" && row.attempts > 0,
            }
        };
        self.shared.notify(Change::Outbox);
        Ok(out)
    }

    /// Answers one of FERB's questions now (never queued). `signer` signs the exact answer when the Mac checks it
    /// against the phone's key (a hidden input always; a Yes to one of FERB's own checks, PHN-417). The hidden value
    /// is zeroized when the answer is dropped; it is never stored or logged.
    pub fn answer(
        &self,
        approval_id: &str,
        answer: Answer,
        signer: Option<Arc<dyn Signer>>,
    ) -> MobileResult<String> {
        Shared::answer(&self.shared, approval_id, &answer, signer)
    }

    /// Stops these tasks (`task.stop`; works with the phone locked, PHN-125). Never queued.
    pub fn stop(&self, task_ids: &[String]) -> MobileResult<()> {
        let t = self.shared.transport_online()?;
        t.call(
            "task.stop",
            &json!({"task_ids": task_ids}).to_string(),
            None,
        )
        .map_err(MobileError::from_link)?;
        Ok(())
    }

    /// Fetches the next older page of a conversation. On the public relay it is refused before anything is asked
    /// (PHN-444) and becomes `NeedsBetterPath`, fetched by itself once when the path carries it again.
    pub fn load_older(&self, conversation: &str) -> MobileResult<OlderState> {
        Shared::load_older(&self.shared, conversation)
    }

    /// Tells the Mac how far the owner has read (`chat.entries.mark read_pos`), when it moved.
    pub fn mark_read(&self, conversation: &str, pos: i64) -> MobileResult<()> {
        let t = self.shared.transport_online()?;
        {
            let st = self.shared.open_state()?;
            let store = st.store.as_ref().ok_or_else(MobileError::closed)?;
            if store.read_mark(conversation)?.is_some_and(|m| m >= pos) {
                return Ok(());
            }
        }
        let params = json!({"client_id": self.shared.config.client_id, "conversation": conversation, "read_pos": pos});
        t.call("chat.entries.mark", &params.to_string(), None)
            .map_err(MobileError::from_link)?;
        let st = self.shared.open_state()?;
        st.store
            .as_ref()
            .ok_or_else(MobileError::closed)?
            .set_read_mark(conversation, pos)?;
        Ok(())
    }

    pub fn link_state(&self) -> LinkState {
        lock(&self.shared.state).link.clone()
    }

    /// "Forget this Mac" (PHN-408): the watch stops, the link is unplugged, the store's files are deleted and every
    /// call answers `closed`. The app then forgets its wrapped key and unpairs the link (S9).
    pub fn forget_this_mac(&self) -> MobileResult<()> {
        let (store, watch) = {
            let mut st = lock(&self.shared.state);
            st.transport = None;
            st.convs.clear();
            st.current = None;
            st.older_waiting.clear();
            st.link.path = LinkPath::Offline;
            st.link.watching = false;
            (st.store.take(), st.watch.take())
        };
        if let Some((_, _, h)) = watch {
            h.close();
        }
        let r = match store {
            Some(s) => s.forget().map_err(MobileError::from),
            None => Err(MobileError::closed()),
        };
        self.shared.notify(Change::Closed);
        r
    }

    /// Closes the core: the watch stops, the store closes (its files stay). Idempotent.
    pub fn close(&self) {
        let (store, watch) = {
            let mut st = lock(&self.shared.state);
            st.transport = None;
            st.convs.clear();
            st.link.watching = false;
            (st.store.take(), st.watch.take())
        };
        if let Some((_, _, h)) = watch {
            h.close();
        }
        let was_open = store.is_some();
        drop(store);
        if was_open {
            self.shared.notify(Change::Closed);
        }
    }

    /// Where the store lives.
    pub fn store_path(&self) -> PathBuf {
        self.shared.path.clone()
    }
}

impl Drop for Mobile {
    fn drop(&mut self) {
        self.close();
    }
}

fn outbox_item(row: &OutboxRow) -> OutboxItem {
    let waiting_for = (row.state == "waiting").then(|| {
        if row.reason.is_some() {
            "unlock".to_string()
        } else {
            "link".to_string()
        }
    });
    OutboxItem {
        id: row.id.clone(),
        text: row.text.clone(),
        created_at: row.created_at.clone(),
        state: row.state.clone(),
        waiting_for,
        reason: row.reason.clone(),
        tried: row.attempts > 0,
    }
}

fn outbox_items(st: &State) -> MobileResult<Vec<OutboxItem>> {
    let store = st.store.as_ref().ok_or_else(MobileError::closed)?;
    Ok(store.outbox()?.iter().map(outbox_item).collect())
}

impl State {
    fn store(&mut self) -> MobileResult<&mut Store> {
        self.store.as_mut().ok_or_else(MobileError::closed)
    }

    /// The conversation's state, loaded from the store on first use.
    fn conv(&mut self, id: &str) -> MobileResult<&mut Conv> {
        if !self.convs.contains_key(id) {
            let store = self.store.as_ref().ok_or_else(MobileError::closed)?;
            let set = EntrySet::from_entries(store.entries(id)?);
            let older = match store.older(id)? {
                Older::Unknown => OlderState::Unknown,
                Older::Before(_) => OlderState::Available,
                Older::Complete => OlderState::Complete,
            };
            self.convs.insert(
                id.to_string(),
                Conv {
                    set,
                    live: LiveState::new(),
                    older,
                },
            );
        }
        Ok(self.convs.get_mut(id).expect("loaded above"))
    }
}

/// The watch's sink: frames of one watch generation; a stale watch's frames are ignored.
struct Sink {
    shared: Weak<Shared>,
    generation: u64,
    conversation: String,
    behind: Mutex<bool>,
}

impl WatchSink for Sink {
    fn frame(&self, frame_json: String) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        if lock(&shared.state).generation != self.generation {
            return;
        }
        let Ok(frame) = serde_json::from_str::<Value>(&frame_json) else {
            return;
        };
        let c = self.conversation.clone();
        match frame["type"].as_str().unwrap_or_default() {
            "change" => {
                let entries = vec![frame["entry"].clone()];
                let cut = match Shared::apply(&shared, &c, &entries) {
                    Ok(cut) => cut,
                    Err(_) => return,
                };
                if let Some(seq) = frame["entry"]["seq"].as_i64() {
                    shared.advance(&c, None, seq);
                }
                if cut {
                    let s = shared.clone();
                    shared.env.spawner.spawn(Box::new(move || {
                        let _ = Shared::fetch_cut(&s, &c);
                    }));
                }
            }
            "caught_up" => {
                if let (Some(epoch), Some(head)) = (frame["epoch"].as_str(), frame["head"].as_i64())
                {
                    shared.advance(&c, Some(epoch), head);
                }
            }
            "live" => {
                let changed = {
                    let mut st = lock(&shared.state);
                    match st.conv(&c) {
                        Ok(conv) => conv
                            .live
                            .frame(frame["request_id"].as_str(), &frame["frame"]),
                        Err(_) => false,
                    }
                };
                if changed {
                    shared.notify(Change::Live { conversation: c });
                }
            }
            "behind" => *lock(&self.behind) = true,
            "reset_required" | "cursor_ahead" => {
                // another store, or a cursor it never gave: read again from the newest page
                let s = shared.clone();
                shared.env.spawner.spawn(Box::new(move || {
                    if let Ok(mut st) = s.open_state() {
                        if let Ok(store) = st.store() {
                            let _ = store.drop_conversation(&c);
                        }
                        st.convs.remove(&c);
                    }
                    let _ = Shared::sync(&s);
                }));
            }
            _ => {}
        }
    }

    fn ended(&self, _failure: Option<LinkFailure>) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        {
            let mut st = lock(&shared.state);
            if st.generation != self.generation {
                return;
            }
            st.watch = None;
            st.link.watching = false;
            if let Some(conv) = st.convs.get_mut(&self.conversation) {
                conv.live.reset();
            }
        }
        shared.notify(Change::Live {
            conversation: self.conversation.clone(),
        });
        shared.notify(Change::Link);
        // too slow to keep up: read the changes from the cursor and watch again (once per `behind`, never a loop on
        // a failure: a watch that failed waits for the link to come online again or the app's sync)
        if std::mem::take(&mut *lock(&self.behind)) {
            let s = shared.clone();
            shared.env.spawner.spawn(Box::new(move || {
                let _ = Shared::sync(&s);
            }));
        }
    }
}

impl Shared {
    fn notify(&self, change: Change) {
        self.env.listener.changed(change);
    }

    fn open_state(&self) -> MobileResult<MutexGuard<'_, State>> {
        let st = lock(&self.state);
        if st.store.is_none() {
            return Err(MobileError::closed());
        }
        Ok(st)
    }

    fn online(&self) -> bool {
        let st = lock(&self.state);
        st.transport.is_some() && st.link.path != LinkPath::Offline
    }

    fn transport(&self) -> MobileResult<Arc<dyn Transport>> {
        self.open_state()?
            .transport
            .clone()
            .ok_or_else(MobileError::offline)
    }

    /// The transport, when the link says it is online.
    fn transport_online(&self) -> MobileResult<Arc<dyn Transport>> {
        let st = self.open_state()?;
        if st.link.path == LinkPath::Offline {
            return Err(MobileError::offline());
        }
        st.transport.clone().ok_or_else(MobileError::offline)
    }

    fn call(t: &Arc<dyn Transport>, method: &str, params: Value) -> MobileResult<Value> {
        let reply = t
            .call(method, &params.to_string(), None)
            .map_err(MobileError::from_link)?;
        parse(&reply)
    }

    fn link_event(this: &Arc<Shared>, event_json: &str) {
        let Ok(ev) = serde_json::from_str::<Value>(event_json) else {
            return;
        };
        let mut start_sync = false;
        let mut retry_older: Vec<String> = Vec::new();
        let mut watch_to_close = None;
        {
            let mut st = lock(&this.state);
            if st.store.is_none() {
                return;
            }
            match ev["type"].as_str().unwrap_or_default() {
                "state" => {
                    let path = if ev["phase"] == "online" {
                        match ev["tier"].as_str() {
                            Some("direct") => LinkPath::Direct,
                            Some("own_relay") => LinkPath::OwnRelay,
                            _ => LinkPath::PublicRelay,
                        }
                    } else {
                        LinkPath::Offline
                    };
                    let was = st.link.path;
                    st.link.path = path;
                    if path != LinkPath::Offline {
                        st.link.bulk_allowed = path != LinkPath::PublicRelay;
                    }
                    if was == LinkPath::Offline && path != LinkPath::Offline {
                        start_sync = true;
                    }
                    if path == LinkPath::Offline {
                        st.link.watching = false;
                        st.generation += 1;
                        watch_to_close = st.watch.take();
                        for c in st.convs.values_mut() {
                            c.live.reset();
                        }
                    }
                }
                "bulk" => {
                    let allowed = ev["allowed"].as_bool().unwrap_or(false);
                    st.link.bulk_allowed = allowed;
                    if st.link.path != LinkPath::Offline {
                        match ev["tier"].as_str() {
                            Some("direct") => st.link.path = LinkPath::Direct,
                            Some("own_relay") => st.link.path = LinkPath::OwnRelay,
                            Some("shared_relay") => st.link.path = LinkPath::PublicRelay,
                            _ => {}
                        }
                    }
                    if allowed {
                        retry_older = st.older_waiting.drain().collect();
                    }
                }
                "relay_state" => {
                    st.link.relay_note = match &ev["relay"] {
                        Value::Null => None,
                        Value::String(s) => Some(s.clone()),
                        other => Some(other.to_string()),
                    };
                }
                _ => return,
            }
        }
        if let Some((_, _, h)) = watch_to_close {
            h.close();
        }
        this.notify(Change::Link);
        if start_sync {
            let s = this.clone();
            this.env.spawner.spawn(Box::new(move || {
                let _ = Shared::sync(&s);
            }));
        }
        for c in retry_older {
            let s = this.clone();
            this.env.spawner.spawn(Box::new(move || {
                let _ = Shared::load_older(&s, &c);
            }));
        }
    }

    /// Applies entries the Mac sent (page, changes or a watch frame) to the store and the conversation; an entry
    /// whose words were cut is recorded to read whole instead. Returns whether any was cut.
    fn apply(this: &Arc<Shared>, conversation: &str, values: &[Value]) -> MobileResult<bool> {
        let entries: Vec<ChatEntry> = values.iter().filter_map(ChatEntry::read).collect();
        let at = this.env.clock.now_iso();
        let (changed, cut, outbox_changed) = {
            let mut st = this.open_state()?;
            let mut cut = false;
            for e in entries.iter().filter(|e| e.cut) {
                st.store()?.add_pending_get(conversation, &e.id)?;
                cut = true;
            }
            let kept = st.store()?.put_entries(conversation, &entries, &at)?;
            // a message of the outbox that reached the Mac (its entry names the submission): it is in the chat now
            let mut outbox_changed = false;
            for e in kept.iter().filter(|e| e.kind == "message") {
                if let Some(sub) = &e.submission_id {
                    if e.client_id.as_deref() == Some(this.config.client_id.as_str()) {
                        let store = st.store()?;
                        if store.outbox_get(sub)?.is_some_and(|r| r.state != "sending") {
                            outbox_changed |= store.outbox_remove(sub)?;
                        }
                    }
                }
            }
            let conv = st.conv(conversation)?;
            let mut changed = false;
            for e in kept {
                conv.live.entry(&e);
                changed |= conv.set.apply(e) == Applied::Kept;
            }
            (changed, cut, outbox_changed)
        };
        if changed {
            this.notify(Change::Transcript {
                conversation: conversation.to_string(),
            });
        }
        if outbox_changed {
            this.notify(Change::Outbox);
        }
        Ok(cut)
    }

    /// Moves the cursor forward (a new epoch replaces it).
    fn advance(&self, conversation: &str, epoch: Option<&str>, seq: i64) {
        if let Ok(st) = self.open_state() {
            if let Some(store) = st.store.as_ref() {
                let epoch = match epoch {
                    Some(e) => Some(e.to_string()),
                    None => store.cursor(conversation).ok().flatten().map(|c| c.epoch),
                };
                if let Some(epoch) = epoch {
                    let _ = store.set_cursor(
                        conversation,
                        &Cursor {
                            epoch,
                            after_seq: seq,
                        },
                    );
                }
            }
        }
    }

    fn set_older(
        this: &Arc<Shared>,
        conversation: &str,
        older: OlderState,
        stored: Option<Older>,
    ) -> MobileResult<()> {
        {
            let mut st = this.open_state()?;
            if let Some(o) = stored {
                st.store()?.set_older(conversation, &o)?;
            }
            st.conv(conversation)?.older = older;
        }
        this.notify(Change::Older {
            conversation: conversation.to_string(),
        });
        Ok(())
    }

    /// The newest page of a conversation, from nothing (what was held is dropped first).
    fn first_page(
        this: &Arc<Shared>,
        t: &Arc<dyn Transport>,
        conversation: &str,
    ) -> MobileResult<()> {
        let r = Self::call(
            t,
            "chat.entries.page",
            json!({"conversation": conversation, "limit": this.config.page_limit, "max_bytes": this.config.max_bytes}),
        )?;
        let watch = {
            let mut st = this.open_state()?;
            st.store()?.drop_conversation(conversation)?;
            st.convs.remove(conversation);
            if st.watch.as_ref().is_some_and(|w| w.1 == conversation) {
                st.generation += 1;
                st.link.watching = false;
                st.watch.take()
            } else {
                None
            }
        };
        if let Some((_, _, h)) = watch {
            h.close();
        }
        Self::apply(
            this,
            conversation,
            r["entries"].as_array().map(Vec::as_slice).unwrap_or(&[]),
        )?;
        if let (Some(epoch), Some(head)) = (r["epoch"].as_str(), r["head"].as_i64()) {
            let st = this.open_state()?;
            st.store
                .as_ref()
                .ok_or_else(MobileError::closed)?
                .set_cursor(
                    conversation,
                    &Cursor {
                        epoch: epoch.into(),
                        after_seq: head,
                    },
                )?;
        }
        let (older, stored) = match (r["more"].as_bool(), r["before_pos"].as_i64()) {
            (Some(true), Some(p)) => (OlderState::Available, Older::Before(p)),
            _ => (OlderState::Complete, Older::Complete),
        };
        Self::set_older(this, conversation, older, Some(stored))?;
        this.notify(Change::Transcript {
            conversation: conversation.to_string(),
        });
        Ok(())
    }

    /// The changes after the cursor, page by page. Returns false when the cursor was refused (read again).
    fn catch_up(
        this: &Arc<Shared>,
        t: &Arc<dyn Transport>,
        conversation: &str,
        mut cursor: Cursor,
    ) -> MobileResult<bool> {
        loop {
            let r = Self::call(
                t,
                "chat.entries.changes",
                json!({"conversation": conversation, "epoch": cursor.epoch, "after_seq": cursor.after_seq,
                       "limit": this.config.page_limit, "max_bytes": this.config.max_bytes}),
            )?;
            if r["reset_required"] == json!(true) || r["cursor_ahead"] == json!(true) {
                return Ok(false);
            }
            Self::apply(
                this,
                conversation,
                r["entries"].as_array().map(Vec::as_slice).unwrap_or(&[]),
            )?;
            let next = r["next_seq"].as_i64().unwrap_or(cursor.after_seq);
            let epoch = r["epoch"].as_str().unwrap_or(&cursor.epoch).to_string();
            let moved = next > cursor.after_seq;
            cursor = Cursor {
                epoch,
                after_seq: next,
            };
            this.advance(conversation, Some(&cursor.epoch), cursor.after_seq);
            if r["more"] != json!(true) || !moved {
                return Ok(true);
            }
        }
    }

    /// Reads whole the entries whose words a reply cut (`chat.entries.get`, piece by piece).
    fn fetch_cut(this: &Arc<Shared>, conversation: &str) -> MobileResult<()> {
        let t = this.transport()?;
        let ids = this
            .open_state()?
            .store
            .as_ref()
            .ok_or_else(MobileError::closed)?
            .pending_gets(conversation)?;
        for id in ids {
            let mut offset = 0usize;
            let mut text = String::new();
            let whole = loop {
                let r = Self::call(
                    &t,
                    "chat.entries.get",
                    json!({"id": id, "offset": offset, "max_chars": this.config.piece_chars}),
                )?;
                let mut entry = r["entry"].clone();
                let piece = entry["text_piece"].clone();
                if piece.is_null() {
                    break entry;
                }
                text.push_str(entry["body"]["text"].as_str().unwrap_or_default());
                let chars = piece["chars"].as_u64().unwrap_or(0) as usize;
                if piece["more"] != json!(true) || chars == 0 {
                    entry["body"]["text"] = json!(text);
                    if let Some(m) = entry.as_object_mut() {
                        m.remove("text_piece");
                    }
                    break entry;
                }
                offset += chars;
            };
            Self::apply(this, conversation, &[whole])?;
        }
        Ok(())
    }

    fn start_watch(
        this: &Arc<Shared>,
        t: &Arc<dyn Transport>,
        conversation: &str,
    ) -> MobileResult<()> {
        let (generation, params) = {
            let mut st = this.open_state()?;
            if st.watch.as_ref().is_some_and(|w| w.1 == conversation) {
                return Ok(());
            }
            st.generation += 1;
            let cursor = st
                .store
                .as_ref()
                .ok_or_else(MobileError::closed)?
                .cursor(conversation)?;
            let mut params =
                json!({"conversation": conversation, "max_bytes": this.config.max_bytes});
            if let Some(c) = cursor {
                params["epoch"] = json!(c.epoch);
                params["after_seq"] = json!(c.after_seq);
            }
            (st.generation, params)
        };
        let old = lock(&this.state).watch.take();
        if let Some((_, _, h)) = old {
            h.close();
        }
        let sink = Arc::new(Sink {
            shared: Arc::downgrade(this),
            generation,
            conversation: conversation.to_string(),
            behind: Mutex::new(false),
        });
        let handle = t
            .watch(&params.to_string(), sink)
            .map_err(MobileError::from_link)?;
        let mut st = lock(&this.state);
        if st.generation == generation {
            st.watch = Some((generation, conversation.to_string(), handle));
            st.link.watching = true;
            drop(st);
            this.notify(Change::Link);
        } else {
            drop(st);
            handle.close();
        }
        Ok(())
    }

    fn sync(this: &Arc<Shared>) -> MobileResult<()> {
        let _one = lock(&this.sync_lock);
        let t = this.transport()?;
        // which conversation FERB writes in now, and its epoch: the smallest newest page
        let probe = Self::call(
            &t,
            "chat.entries.page",
            json!({"limit": 1, "max_bytes": this.config.max_bytes}),
        )?;
        let conversation = probe["conversation"]
            .as_str()
            .ok_or_else(|| {
                MobileError::new(ErrorCode::BadReply, "FERB's reply named no conversation.")
            })?
            .to_string();
        let epoch = probe["epoch"].as_str().unwrap_or_default().to_string();
        let switched = {
            let mut st = this.open_state()?;
            let switched = st.current.as_deref() != Some(conversation.as_str());
            if switched {
                st.store()?.set_meta("current", &conversation)?;
                st.current = Some(conversation.clone());
            }
            switched
        };
        if switched {
            this.notify(Change::Conversations);
        }
        let cursor = this
            .open_state()?
            .store
            .as_ref()
            .ok_or_else(MobileError::closed)?
            .cursor(&conversation)?;
        match cursor {
            Some(c) if c.epoch == epoch => {
                if !Self::catch_up(this, &t, &conversation, c)? {
                    Self::first_page(this, &t, &conversation)?;
                }
            }
            _ => Self::first_page(this, &t, &conversation)?,
        }
        Self::fetch_cut(this, &conversation)?;
        Self::start_watch(this, &t, &conversation)?;
        // the delivered mark: what this phone has stored (best effort)
        if let Ok(Some(c)) = this
            .open_state()?
            .store
            .as_ref()
            .ok_or_else(MobileError::closed)?
            .cursor(&conversation)
        {
            let _ = t.call(
                "chat.entries.mark",
                &json!({"client_id": this.config.client_id, "conversation": conversation,
                        "delivered_seq": c.after_seq})
                .to_string(),
                None,
            );
        }
        drop(_one);
        Self::flush(this);
        Ok(())
    }

    fn load_older(this: &Arc<Shared>, conversation: &str) -> MobileResult<OlderState> {
        let t = this.transport_online()?;
        let (before, carries) = {
            let mut st = this.open_state()?;
            // the link's own word on what its path carries (its `bulk` event, PHN-461)
            let carries = st.link.bulk_allowed;
            let before = st.conv(conversation)?.set.lowest_pos();
            (before, carries)
        };
        let Some(before) = before else {
            Self::first_page(this, &t, conversation)?;
            return Ok(this.open_state()?.conv(conversation)?.older.clone());
        };
        let refused = |this: &Arc<Shared>| -> MobileResult<OlderState> {
            let state = OlderState::NeedsBetterPath {
                message: OLDER_NEEDS_BETTER_PATH.into(),
            };
            lock(&this.state)
                .older_waiting
                .insert(conversation.to_string());
            Self::set_older(this, conversation, state.clone(), None)?;
            Ok(state)
        };
        if !carries {
            // refused on the public relay before anything is asked (the link would refuse it anyway)
            return refused(this);
        }
        let r = match Self::call(
            &t,
            "chat.entries.page",
            json!({"conversation": conversation, "before_pos": before, "limit": this.config.page_limit,
                   "max_bytes": this.config.max_bytes}),
        ) {
            Ok(r) => r,
            Err(e) if e.code == ErrorCode::NeedsBetterPath => return refused(this),
            Err(e) => return Err(e),
        };
        let cut = Self::apply(
            this,
            conversation,
            r["entries"].as_array().map(Vec::as_slice).unwrap_or(&[]),
        )?;
        if cut {
            Self::fetch_cut(this, conversation)?;
        }
        let (older, stored) = match (r["more"].as_bool(), r["before_pos"].as_i64()) {
            (Some(true), Some(p)) => (OlderState::Available, Older::Before(p)),
            _ => (OlderState::Complete, Older::Complete),
        };
        Self::set_older(this, conversation, older.clone(), Some(stored))?;
        Ok(older)
    }

    /// Sends the waiting messages in order: one that the link can't carry stops the flush (it and the rest wait for
    /// the link); one the Mac refuses is failed with its reason (never sent again) and the next goes on.
    fn flush(this: &Arc<Shared>) {
        let _one = lock(&this.outbox_lock);
        loop {
            let Ok(t) = this.transport_online() else {
                return;
            };
            let row = {
                let Ok(st) = this.open_state() else { return };
                let Some(store) = st.store.as_ref() else {
                    return;
                };
                match store.outbox() {
                    Ok(rows) => rows.into_iter().find(|r| r.state == "waiting"),
                    Err(_) => return,
                }
            };
            let Some(mut row) = row else { return };
            row.state = "sending".into();
            if this.update_row(&row).is_err() {
                return;
            }
            this.notify(Change::Outbox);
            let params = json!({"text": row.text, "client_id": this.config.client_id, "submission_id": row.id});
            let result = t.send(&params.to_string());
            match result {
                Ok(_ack) => {
                    // the Mac has it (a repeat answers with the same acceptance): its entry shows it in the chat
                    if let Ok(st) = this.open_state() {
                        if let Some(store) = st.store.as_ref() {
                            let _ = store.outbox_remove(&row.id);
                        }
                    }
                    this.notify(Change::Outbox);
                }
                Err(f) => {
                    row.attempts += 1;
                    match f.kind() {
                        FailureKind::Transient => {
                            row.state = "waiting".into();
                            row.reason = None;
                        }
                        FailureKind::NeedsUnlock => {
                            row.state = "waiting".into();
                            row.reason = Some(f.message.clone());
                        }
                        FailureKind::NeedsBetterPath | FailureKind::Refused => {
                            row.state = "failed".into();
                            row.reason = Some(if f.message.is_empty() {
                                f.code.clone()
                            } else {
                                f.message.clone()
                            });
                        }
                    }
                    let _ = this.update_row(&row);
                    this.notify(Change::Outbox);
                    if row.state == "waiting" {
                        return;
                    }
                }
            }
        }
    }

    fn update_row(&self, row: &OutboxRow) -> MobileResult<()> {
        let st = self.open_state()?;
        st.store
            .as_ref()
            .ok_or_else(MobileError::closed)?
            .outbox_update(row)?;
        Ok(())
    }

    /// The challenge the question carries for the phone (`phone.challenge`), else a fresh one from the link.
    fn challenge(
        this: &Arc<Shared>,
        t: &Arc<dyn Transport>,
        approval_id: &str,
    ) -> MobileResult<String> {
        let carried = {
            let st = this.open_state()?;
            st.convs.values().find_map(|c| {
                c.set.entries().find_map(|e| {
                    (e.kind == "question" && e.approval_id.as_deref() == Some(approval_id))
                        .then(|| {
                            e.body
                                .get("phone")
                                .and_then(|p| p.get("challenge"))
                                .and_then(Value::as_str)
                        })
                        .flatten()
                        .map(str::to_string)
                })
            })
        };
        if let Some(c) = carried {
            return Ok(c);
        }
        let r = Self::call(t, "link.challenge", json!({}))?;
        r["challenge"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| MobileError::new(ErrorCode::BadReply, "The link gave no challenge."))
    }

    fn answer(
        this: &Arc<Shared>,
        approval_id: &str,
        answer: &Answer,
        signer: Option<Arc<dyn Signer>>,
    ) -> MobileResult<String> {
        // never queued: with no link the answer is not kept to send later (PHN-448)
        let t = this.transport_online().map_err(|e| {
            if e.code == ErrorCode::Offline {
                MobileError::new(
                    ErrorCode::Offline,
                    "FERB's Mac can't be reached now; an answer is never kept to send later.",
                )
            } else {
                e
            }
        })?;
        let (method, mut params) = match answer {
            Answer::Approve { granted, choice } => {
                let mut p = vec![
                    ("approval_id", json!(approval_id)),
                    ("granted", json!(granted)),
                ];
                if let Some(c) = choice {
                    p.push(("choice", json!(c)));
                }
                ("chat.approve", sorted(p))
            }
            Answer::Words { text } => (
                "chat.answer",
                sorted(vec![
                    ("approval_id", json!(approval_id)),
                    ("text", json!(text)),
                ]),
            ),
            Answer::Correct { text } => (
                "chat.correct",
                sorted(vec![
                    ("approval_id", json!(approval_id)),
                    ("text", json!(text)),
                ]),
            ),
            Answer::Hidden { secret } => (
                "chat.answer",
                sorted(vec![
                    ("approval_id", json!(approval_id)),
                    ("text", json!(secret)),
                ]),
            ),
        };
        let proof = match (answer, signer) {
            (Answer::Hidden { .. }, None) => {
                scrub(&mut params);
                return Err(MobileError::new(
                    ErrorCode::NeedsSigner,
                    "A hidden answer needs the phone's key.",
                ));
            }
            (_, None) => None,
            (a, Some(signer)) => {
                let challenge = match Self::challenge(this, &t, approval_id) {
                    Ok(c) => c,
                    Err(e) => {
                        scrub(&mut params);
                        return Err(e);
                    }
                };
                let payload = match a {
                    Answer::Hidden { secret } => sealed_payload(approval_id, &challenge, secret),
                    _ => answer_payload(approval_id, &challenge, method, &params),
                };
                match signer.sign(&payload) {
                    Some(sig) => Some(json!({"payload": payload, "sig": sig}).to_string()),
                    None => {
                        scrub(&mut params);
                        return Err(MobileError::new(ErrorCode::NotSigned, "Nothing was sent."));
                    }
                }
            }
        };
        let mut text = params.to_string();
        scrub(&mut params);
        let r = t.call(method, &text, proof.as_deref());
        text.zeroize();
        r.map_err(MobileError::from_link)
    }
}

/// Overwrites a params object's words before it is dropped (a hidden answer's value).
fn scrub(v: &mut Value) {
    if let Some(Value::String(s)) = v.get_mut("text") {
        s.zeroize();
    }
}

#[cfg(test)]
mod tests {
    /// The apps hold one `Mobile` and call it from any thread.
    #[test]
    fn mobile_is_send_and_sync() {
        fn both<T: Send + Sync>() {}
        both::<super::Mobile>();
        both::<crate::door::DoorTransport>();
    }
}
