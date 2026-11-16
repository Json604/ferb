//! The phone session's entry on the local socket (stage 3, S3; CONTRACT §12). Only the phone link's own helper, proven
//! by its code signature (`Server::set_is_link`, [`crate::link_peer`]), may say `auth.hello {role: "phone", device,
//! session}`; every other caller naming "phone" (or "link") gets the refusal Python's parser gives an unknown role.
//!
//! On a phone session the server asks the daemon's [`PhoneGate`]: at the hello whether `device` is one of its paired
//! phones; before each call what the method's class needs (the unlock window, a proof for this exact call, the answer
//! rules; `crate::classes`), instead of the local roles' method lists and the FERB.app check; after a unary call what
//! the phone gets (a locked phone's status with its words held). The handlers are the ones FERB.app's calls reach, with
//! the session's [`PhoneSession`] beside the role, so a turn a phone sends is the owner's, from that phone.
//!
//! Refusals are JSON-RPC errors with their own codes (−32011 … −32022, the lab's `standin/phone.py CODES`, which
//! ferb-link's `daemon::REFUSALS` reads), so the helper can tell the phone what stopped a call and what would allow it.

use serde_json::{Map, Value};

use crate::server::HandlerResult;
use crate::wire::Params;

/// The phone and its one link connection a session speaks for, and the proof of the call being answered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhoneSession {
    /// The phone's link identity (its endpoint id), as the daemon's paired-phone table holds it.
    pub device: String,
    /// The link connection: its challenges end with it.
    pub session: String,
    /// The call's proof as the phone sent it (`{payload, sig}`), passed through untouched; `None` without one.
    pub proof: Option<Map<String, Value>>,
    /// The path the link's connection takes now, as the helper says it in the hello (`direct`, `own_relay`,
    /// `shared_relay`; S4): the helper opens one daemon connection per call, so it is the call's. The daemon refuses
    /// files on the shared relay too (kb PHN-444), behind the helper's own refusal; `None` from an older helper.
    pub tier: Option<String>,
}

/// A refusal of the phone gate: the code and the words for the phone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub code: i64,
    pub message: String,
}

impl Refusal {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Refusal {
            code,
            message: message.into(),
        }
    }
}

// The refusals' codes (CONTRACT §6 and §12; ferb-link `daemon::REFUSALS`).
pub const LOCKED: i64 = -32011;
pub const UNLOCK_REQUIRED: i64 = -32012;
pub const UNLOCK_INVALID: i64 = -32013;
pub const FRESH_UNLOCK_REQUIRED: i64 = -32014;
pub const PROTECTED_FOLDER: i64 = -32015;
pub const FILE_NOT_ALLOWED: i64 = -32016;
pub const NOT_A_FILE: i64 = -32017;
pub const NOT_FOUND: i64 = -32018;
pub const QUESTION_NOT_WAITING: i64 = -32019;
pub const UNKNOWN_PHONE: i64 = -32020;
pub const MAC_DISK_FULL: i64 = -32021;
pub const NEEDS_THE_MAC: i64 = -32022;
/// A pairing while the most phones are paired (PHN-419; the link's `device_limit`). New in S3: ferb-link's
/// `daemon::REFUSALS` gains it in S4.
pub const DEVICE_LIMIT: i64 = -32023;
/// A file that changed while it was being sent to the phone (its size, time or identity moved since it was opened):
/// the transfer stops, nothing more is sent (stage 3 S3b, T-1203). New: ferb-link's `daemon::REFUSALS` gains it in S4.
pub const FILE_CHANGED: i64 = -32024;
/// A malformed call (JSON-RPC's invalid request, which the lab used for its `bad_request`).
pub const BAD_REQUEST: i64 = crate::wire::INVALID_REQUEST;

/// The codes by the names the link and the phone apps use.
pub const CODES: &[(&str, i64)] = &[
    ("locked", LOCKED),
    ("unlock_required", UNLOCK_REQUIRED),
    ("unlock_invalid", UNLOCK_INVALID),
    ("fresh_unlock_required", FRESH_UNLOCK_REQUIRED),
    ("protected_folder", PROTECTED_FOLDER),
    ("file_not_allowed", FILE_NOT_ALLOWED),
    ("not_a_file", NOT_A_FILE),
    ("not_found", NOT_FOUND),
    ("question_not_waiting", QUESTION_NOT_WAITING),
    ("unknown_phone", UNKNOWN_PHONE),
    ("mac_disk_full", MAC_DISK_FULL),
    ("needs_the_mac", NEEDS_THE_MAC),
    ("device_limit", DEVICE_LIMIT),
    ("file_changed", FILE_CHANGED),
    ("bad_request", BAD_REQUEST),
];

/// The daemon's phone area, as the server asks it about a phone session.
pub trait PhoneGate: Send + Sync + 'static {
    /// The link's hello for a phone: `Ok` when `device` is a live paired phone (the session is noted).
    fn open(&self, device: &str, session: &str) -> Result<(), Refusal>;
    /// Before a call (after the role's check that its class is not `never`): what the class needs, the answer rules,
    /// the parameters a phone may not send; the parameters may be adjusted (a phone's own client id). The device is
    /// rechecked here on every call, as a run token is (a removed phone stops at once).
    fn admit(&self, phone: &PhoneSession, method: &str, params: &mut Params)
        -> Result<(), Refusal>;
    /// After a unary call: what the phone gets (a locked phone's status with its words held, a Mac prompt's click),
    /// and the audit's outcome.
    fn reply(&self, phone: &PhoneSession, method: &str, result: HandlerResult) -> HandlerResult;
}

// A socket connection is not the session: the helper may open one connection per call under the same `session`
// (ferb-link `Daemon::call`); the session ends with `phone.session_end`, or with the daemon.
