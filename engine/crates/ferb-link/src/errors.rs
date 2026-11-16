//! Every refusal and failure the link can report. Codes are stable (CONTRACT.md §6): the phone and the Mac
//! match on `code`, never on the words. Each error says what stopped the request and, where something would
//! allow it, what (`allow`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LinkError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_s: Option<u64>,
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        if let Some(a) = &self.allow {
            write!(f, " ({a})")?;
        }
        Ok(())
    }
}
impl std::error::Error for LinkError {}

pub type LinkResult<T> = Result<T, LinkError>;

impl LinkError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            allow: None,
            retry_after_s: None,
        }
    }
    pub fn allow(mut self, what: impl Into<String>) -> Self {
        self.allow = Some(what.into());
        self
    }
    pub fn retry(mut self, s: u64) -> Self {
        self.retry_after_s = Some(s);
        self
    }
    /// Errors that a redial cannot fix: the phone stops dialing and shows the message.
    pub fn is_final(&self) -> bool {
        matches!(
            self.code.as_str(),
            NOT_PAIRED
                | DEVICE_REVOKED
                | VERSION_MISMATCH
                | PAIR_WINDOW_CLOSED
                | PAIR_SECRET_WRONG
                | PAIR_DENIED
                | DEVICE_LIMIT
        )
    }
}

// ---- pairing and identity ---------------------------------------------------------------------------
pub const NOT_PAIRED: &str = "not_paired";
pub const DEVICE_REVOKED: &str = "device_revoked";
pub const PAIR_WINDOW_CLOSED: &str = "pair_window_closed";
pub const PAIR_SECRET_WRONG: &str = "pair_secret_wrong";
pub const PAIR_DENIED: &str = "pair_denied";
pub const PAIR_BUSY: &str = "pair_busy";
pub const DEVICE_LIMIT: &str = "device_limit";
pub const ALREADY_PAIRED: &str = "already_paired";
pub const VERSION_MISMATCH: &str = "version_mismatch";
// ---- authorization ------------------------------------------------------------------------------------
pub const METHOD_NOT_ALLOWED: &str = "method_not_allowed";
pub const MAC_LINK_OLDER: &str = "mac_link_older";
pub const APP_OLDER: &str = "app_older";
pub const PARAM_NOT_ALLOWED: &str = "param_not_allowed";
pub const UNLOCK_REQUIRED: &str = "unlock_required";
pub const UNLOCK_INVALID: &str = "unlock_invalid";
pub const QUESTION_NOT_WAITING: &str = "question_not_waiting";
// ---- tiers and limits -------------------------------------------------------------------------------
pub const NEEDS_BETTER_PATH: &str = "needs_direct_or_own_relay";
pub const TRANSFER_TOO_LARGE: &str = "transfer_too_large";
pub const BULK_BUDGET: &str = "bulk_budget_exhausted";
pub const RELAY_RATE_LIMITED: &str = "relay_rate_limited";
pub const DAILY_RELAY_CAP: &str = "daily_relay_cap";
pub const PREVIEW_TOO_SOON: &str = "preview_too_soon";
pub const TOO_MANY_REQUESTS: &str = "too_many_requests";
pub const FRAME_TOO_LARGE: &str = "frame_too_large";
pub const STALLED: &str = "stalled";
pub const BAD_REQUEST: &str = "bad_request";
// ---- reachability ---------------------------------------------------------------------------------------
pub const MAC_UNREACHABLE: &str = "mac_unreachable";
pub const RELAY_UNREACHABLE: &str = "relay_unreachable";
pub const OWN_RELAY_UNREACHABLE: &str = "own_relay_unreachable";
pub const OWN_RELAY_DNS: &str = "own_relay_dns";
pub const OWN_RELAY_TLS: &str = "own_relay_tls";
pub const OWN_RELAY_NOT_RELAY: &str = "own_relay_not_a_relay";
pub const OWN_RELAY_ACCESS_DENIED: &str = "own_relay_access_denied";
pub const OWN_RELAY_VERSION: &str = "own_relay_version";
pub const OWN_RELAY_URL: &str = "own_relay_bad_url";
pub const FERB_NOT_RUNNING: &str = "ferb_not_running";
pub const FERB_ERROR: &str = "ferb_error";
pub const LINK_CLOSED: &str = "link_closed";
pub const NOT_FOUND: &str = "not_found";
/// A file that changed while it was being sent (the daemon pins its size, time and identity: S3b).
pub const FILE_CHANGED: &str = "file_changed";
// ---- the phone as a full remote ---------------------------------------------------------------------------
pub const NOT_FROM_PHONE: &str = "not_from_phone";
pub const LOCKED: &str = "locked";
pub const FRESH_UNLOCK: &str = "fresh_unlock_required";
pub const PROTECTED_FOLDER: &str = "protected_folder";
pub const FILE_NOT_ALLOWED: &str = "file_not_allowed";
pub const NOT_A_FILE: &str = "not_a_file";
pub const MAC_DISK_FULL: &str = "mac_disk_full";
/// A prompt on the Mac's screen that macOS would not let software click (kb SEC-20).
pub const NEEDS_THE_MAC: &str = "needs_the_mac";

/// QUIC application close codes: a connection the Mac closes carries one, so the phone knows why even when
/// no error frame could be delivered. The close reason bytes carry the same `code` string.
pub fn close_code(code: &str) -> u32 {
    match code {
        NOT_PAIRED => 0x10,
        DEVICE_REVOKED => 0x11,
        VERSION_MISMATCH => 0x12,
        TOO_MANY_REQUESTS => 0x13,
        FRAME_TOO_LARGE => 0x14,
        PAIR_WINDOW_CLOSED => 0x15,
        PAIR_SECRET_WRONG => 0x16,
        PAIR_DENIED => 0x17,
        PAIR_BUSY => 0x18,
        DEVICE_LIMIT => 0x19,
        BAD_REQUEST => 0x1a,
        STALLED => 0x1b,
        _ => 0x1f,
    }
}

pub fn not_paired() -> LinkError {
    LinkError::new(NOT_PAIRED, "This phone is not paired with this Mac.")
        .allow("Open FERB on the Mac, choose Pair a phone and scan the code")
}
pub fn revoked() -> LinkError {
    LinkError::new(
        DEVICE_REVOKED,
        "This phone was removed from FERB on the Mac.",
    )
    .allow("Pair again from the Mac to use it")
}
/// The phone asked for something this Mac's link does not know, and the phone's build is the newer one.
pub fn mac_link_older(method: &str) -> LinkError {
    LinkError::new(MAC_LINK_OLDER, format!("The FERB link running on your Mac is older than this app: it does not know \"{method}\" yet.")).allow("On the Mac, restart FERB's phone link so its current version runs, or update FERB there")
}
/// The same, when the phone's build is the older one.
pub fn app_older(method: &str) -> LinkError {
    LinkError::new(
        APP_OLDER,
        format!(
            "This app is older than FERB on your Mac, which no longer does \"{method}\" this way."
        ),
    )
    .allow("Update the FERB app on this phone")
}
pub fn version(mac: u32, mac_min: u32, phone: u32, phone_min: u32) -> LinkError {
    let who = if phone < mac_min {
        "the phone app"
    } else {
        "FERB on the Mac"
    };
    LinkError::new(
        VERSION_MISMATCH,
        format!("The phone app (link version {phone}, needs at least {phone_min}) and FERB on the Mac (link version {mac}, needs at least {mac_min}) cannot talk to each other."),
    )
    .allow(format!("Update {who}"))
}
pub fn tier_refused(what: &str, tier: &str) -> LinkError {
    LinkError::new(
        NEEDS_BETTER_PATH,
        format!("{what} needs a direct connection or your own relay; you are on {tier}."),
    )
    .allow("Use the same Wi-Fi as the Mac, or set up your own relay in FERB's settings")
}
pub fn too_large(what: &str, size: u64, cap: u64, tier: &str) -> LinkError {
    LinkError::new(
        TRANSFER_TOO_LARGE,
        format!(
            "{what} is {} but at most {} can be sent on {tier}.",
            human(size),
            human(cap)
        ),
    )
    .allow("Use the same Wi-Fi as the Mac, or set up your own relay in FERB's settings")
}
pub fn human(n: u64) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1u64 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.0} kB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}
