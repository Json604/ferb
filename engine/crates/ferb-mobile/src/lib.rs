//! ferb-mobile: the phone's core, shared by the Android and iPhone apps (stage 3, slice S7; kb T-1207). README.md
//! describes the public API and its threading model.
//!
//! - `fold`: stored chat entries -> the transcript FERB.app shows (the Mac's fold is the golden).
//! - `entries`: what was fetched of a conversation, order-independent.
//! - `live`: the watch's live frames over the transcript, never stored.
//! - `store`: the SQLCipher store (PHN-448), kept until Forget this Mac (PHN-408).
//! - `transport`: how the core reaches the Mac (a trait; the real link plugs in through `door`).
//! - `device`: the phone's own record for FERB (T-1214) and finding the Mac's files (recent, search).
//! - `rich`, `shown`: FERB's words and answers laid out as FERB.app draws them (Markdown, the sites row, cards).
//! - `notify`: what a phone notification says, in FERB.app's words (a task's end, a question).
//! - `api`: `Mobile`, the object the apps use: sync, watch, outbox (PHN-401), answers, stop, older pages.

pub mod api;
pub mod device;
pub mod door;
pub mod entries;
pub mod entry;
pub mod fold;
pub mod live;
pub mod notify;
pub mod question;
pub mod remote;
pub mod rich;
pub mod shown;
pub mod status;
pub mod store;
pub mod time;
pub mod transport;

pub use api::{
    Answer, Cancelled, Change, Clock, Config, ConversationInfo, Env, ErrorCode, Ids, LinkPath,
    LinkState, Listener, Mobile, MobileError, OlderState, OutboxItem, Signer, Spawner,
    ThreadSpawner, TranscriptView,
};
pub use device::{DeviceReport, Fix, FoundFile, FoundFiles, Place, RecentFiles};
pub use transport::{LinkFailure, Transport, WatchHandle, WatchSink};
