//! Every cap, budget and timer of the link, as one named settings record (kb LIM-3). The values are
//! the proposals of the validation's limits table; the ones that change what FERB does are the owner's to
//! confirm (kb task T-2). They are enforced by the Mac side only; the phone is told them so it can explain.

use serde::{Deserialize, Serialize};

pub const KIB: u64 = 1024;
pub const MIB: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Limits {
    // ---- wire (agent) ---------------------------------------------------------------------------------
    /// One control frame (a request, a reply, an event). Refused at the header, before allocating.
    pub control_frame_max: u64,
    /// Bulk travels in chunks of this size, each acknowledged by the receiver.
    pub chunk_bytes: u64,
    /// Unacknowledged chunks in flight on the shared relay / on a direct path or the owner's relay.
    pub shared_window: u32,
    pub good_window: u32,
    /// Streams one phone may hold open at once (QUIC enforces it; more simply wait).
    pub streams_max: u32,
    /// A stream that makes no progress for this long is reset. Never a whole-frame deadline.
    pub stall_s: u64,
    /// How long a control call may wait for FERB's daemon to answer.
    pub call_deadline_s: u64,

    // ---- shared relay ----------------------------------------------------------------------------------------
    /// The public relays carry control only (kb PHN-444): every bulk transfer is refused there before its first byte,
    /// so the eight caps below no longer apply to anything. They are kept, with their values, because apps of the lab
    /// read them from the welcome and the owner's settings name them (as `handover_fresh_*` below).
    /// The latest page of chat on the public relays: at most this many entries (older pages are refused there, and
    /// the page must fit one control frame). 50 is the engine's own page (ferb-supervisor `entries_rpc::PAGE`).
    #[serde(default = "shared_page_max_default")]
    pub shared_page_max: u64,
    /// One transfer (either direction) while on the shared relay.
    pub shared_transfer_max: u64,
    /// One on-demand preview picture while on the shared relay, and the least time between two.
    pub shared_preview_max: u64,
    pub shared_preview_every_s: u64,
    /// Bulk budget on the shared relay: a token bucket, both directions counted.
    pub shared_bulk_per_min: u64,
    pub shared_bulk_burst: u64,
    /// After the relay's rate-limit notice: no bulk for this long, then half the budget until the relay
    /// connection is replaced by itself. FERB never reconnects to clear it.
    pub ratelimit_pause_s: u64,
    /// Soft cap on everything relayed through the shared relay in one day; over it bulk is refused.
    pub shared_daily_max: u64,

    // ---- direct / own relay ---------------------------------------------------------------------------------
    /// One transfer on a direct path or the owner's relay (0 = no cap).
    pub good_transfer_max: u64,
    /// Least time between two preview pictures there (live preview).
    pub good_preview_every_ms: u64,

    // ---- pairing and devices (owner to confirm) -------------------------------------------------------------
    pub pair_window_s: u64,
    pub pair_wrong_max: u32,
    /// How long the Mac waits for the owner's Yes after a valid proof.
    pub pair_confirm_s: u64,
    pub devices_max: u32,
    /// A paired phone not seen for this many days is removed (0 = never).
    pub device_unused_days: u64,
    /// A Yes to one of FERB's own checks needs the phone's unlock (a signature from its lock-bound key).
    pub unlock_before_yes: bool,

    // ---- the phone as a full remote (owner to confirm) ---------------------------------------------------------
    /// After an unlock on the phone, the owner's data (files, memory, settings, traces) can be read for this long
    /// without another; every such read starts the time again.
    pub unlock_idle_s: u64,
    /// However much it is used, one unlock never lasts longer than this (12 hours: a session that is in use is
    /// not interrupted by it in any ordinary day; the idle time and the app's own lock are what close it).
    pub unlock_max_s: u64,
    /// The audit is kept this many days (the owner: 90). It is cut into pieces of `audit_max_bytes`; at most
    /// `audit_keep` pieces are kept whatever their age (the safety limit on size: 20 x 5 MB).
    pub audit_keep_days: u64,
    pub audit_max_bytes: u64,
    pub audit_keep: u32,
    /// The Mac says so, once per burst, when a phone took more than this many files or bytes within the window.
    /// A notice, never a cap.
    pub fetch_notice_files: u64,
    pub fetch_notice_bytes: u64,
    pub fetch_notice_window_s: u64,
    /// A one-time challenge for a sensitive call lives this long.
    pub challenge_ttl_s: u64,
    /// The most entries one folder listing returns.
    pub list_max: u64,
    /// The most files one folder may hold to be sent to the phone as a whole.
    pub tree_max: u64,
    /// An upload from a phone is refused when it would leave less than this free on the Mac's disk.
    pub upload_free_floor: u64,
    /// An upload that stopped part way is kept this long on the Mac so it can go on where it stopped.
    pub upload_partial_keep_s: u64,

    // ---- abuse (agent) -----------------------------------------------------------------------------------------
    /// Connections from endpoints that are not paired, handled at once, and how long each gets to prove itself.
    pub unpaired_slots: u32,
    pub unpaired_hello_s: u64,
    /// New connections one paired phone may make per minute before it is told to slow down.
    pub connects_per_min: u32,
    /// Control calls one phone may make per 10 s.
    pub calls_per_10s: u32,

    // ---- own relay (owner to confirm) ---------------------------------------------------------------------------
    /// With an own relay set and unreachable this long, FERB falls back to the shared relays (when allowed).
    pub own_relay_fallback_after_s: u64,
    pub own_relay_fallback: bool,
    /// How often the unreachable own relay is tried again.
    pub own_relay_probe_s: u64,
    /// "Always use my relay" (kb PHN-219, PHN-453): with an own relay set, stay on it even when a direct path exists.
    /// Off: a direct path is used whenever one exists, the own relay when none does (the owner, 2026-11-16).
    #[serde(default)]
    pub own_relay_always: bool,
    /// What apps from before `handover_fresh_at_s` read: one fresh start this many seconds after a change of
    /// network. Kept so such an app still gets its number; nothing in this build reads it.
    #[serde(default = "handover_fresh_after_default")]
    pub handover_fresh_after_s: u64,
    /// What the app of build 4da4f8c reads: start over at each of these seconds after a change of network. Kept so
    /// that app still gets its list; nothing in this build reads it (the schedule below replaced it, kb NET-24).
    #[serde(default = "handover_fresh_at_default")]
    pub handover_fresh_at_s: Vec<u64>,
    /// While a session is still on a relay after the phone's network changed or the app was opened, the phone starts
    /// its connection over on a new endpoint this often (the owner: every 30 seconds, kb NET-24). 0: never. Never
    /// on a direct connection, never while a call or a file is under way.
    #[serde(default = "handover_retry_every_default")]
    pub handover_retry_every_s: u64,
    /// How long that pace is kept after the change of network or the opening of the app; after it the slower pace
    /// below. (agent: kb NET-27; asked: T-30)
    #[serde(default = "handover_retry_fast_for_default")]
    pub handover_retry_fast_for_s: u64,
    /// The slower pace after that (0: stop when the fast pace ends). (agent: kb NET-27; asked: T-30)
    #[serde(default = "handover_retry_slow_every_default")]
    pub handover_retry_slow_every_s: u64,
    /// After this long without a new change of network or opening of the app nothing more is started over; iroh's
    /// own try once a minute goes on. (agent: kb NET-27; asked: T-30)
    #[serde(default = "handover_retry_stop_after_default")]
    pub handover_retry_stop_after_s: u64,
    /// After this many start-overs in a row that could not connect (each holds calls for some seconds), no more
    /// until the next change of network or opening of the app. (agent: kb NET-27; asked: T-30)
    #[serde(default = "handover_retry_fails_max_default")]
    pub handover_retry_fails_max: u32,

    // ---- phone redial (agent; the long-outage step is the owner's) ------------------------------------------------
    pub dial_deadline_s: u64,
    pub redial_steps_s: Vec<u64>,
    pub redial_jitter_pct: u32,
    pub redial_long_after_s: u64,
    pub redial_long_every_s: u64,
    pub healthy_reset_s: u64,
    /// The phone shows "reconnecting" after this long without a reply and gives the session up after the second.
    pub reconnecting_after_s: u64,
    pub give_up_after_s: u64,
}

fn shared_page_max_default() -> u64 {
    50
}
fn handover_fresh_after_default() -> u64 {
    10
}
fn handover_fresh_at_default() -> Vec<u64> {
    vec![150, 300]
}
fn handover_retry_every_default() -> u64 {
    30
}
fn handover_retry_fast_for_default() -> u64 {
    300
}
fn handover_retry_slow_every_default() -> u64 {
    300
}
fn handover_retry_stop_after_default() -> u64 {
    3600
}
fn handover_retry_fails_max_default() -> u32 {
    2
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            control_frame_max: 64 * KIB,
            chunk_bytes: 16 * KIB,
            shared_window: 1,
            good_window: 16,
            streams_max: 8,
            stall_s: 10,
            call_deadline_s: 10,
            shared_page_max: shared_page_max_default(),
            shared_transfer_max: MIB,
            shared_preview_max: 200_000,
            shared_preview_every_s: 5,
            shared_bulk_per_min: 1_000_000,
            shared_bulk_burst: 2_000_000,
            ratelimit_pause_s: 60,
            shared_daily_max: 100_000_000,
            good_transfer_max: 0,
            good_preview_every_ms: 200,
            pair_window_s: 120,
            pair_wrong_max: 3,
            pair_confirm_s: 120,
            devices_max: 5,
            device_unused_days: 30,
            unlock_before_yes: true,
            unlock_idle_s: 300,
            unlock_max_s: 12 * 3600,
            audit_keep_days: 90,
            audit_max_bytes: 5_000_000,
            audit_keep: 20,
            fetch_notice_files: 40,
            fetch_notice_bytes: 1024 * MIB,
            fetch_notice_window_s: 600,
            challenge_ttl_s: 120,
            list_max: 500,
            tree_max: 2000,
            upload_free_floor: 1024 * MIB,
            upload_partial_keep_s: 3600,
            unpaired_slots: 4,
            unpaired_hello_s: 5,
            connects_per_min: 12,
            calls_per_10s: 60,
            own_relay_fallback_after_s: 30,
            own_relay_fallback: true,
            own_relay_probe_s: 60,
            own_relay_always: false,
            handover_fresh_after_s: handover_fresh_after_default(),
            handover_fresh_at_s: handover_fresh_at_default(),
            handover_retry_every_s: handover_retry_every_default(),
            handover_retry_fast_for_s: handover_retry_fast_for_default(),
            handover_retry_slow_every_s: handover_retry_slow_every_default(),
            handover_retry_stop_after_s: handover_retry_stop_after_default(),
            handover_retry_fails_max: handover_retry_fails_max_default(),
            dial_deadline_s: 10,
            redial_steps_s: vec![1, 2, 4, 8, 15, 30, 60],
            redial_jitter_pct: 20,
            redial_long_after_s: 600,
            redial_long_every_s: 300,
            healthy_reset_s: 60,
            reconnecting_after_s: 10,
            give_up_after_s: 30,
        }
    }
}

/// A token bucket for the shared relay's bulk budget.
#[derive(Debug)]
pub struct Bucket {
    tokens: f64,
    last: std::time::Instant,
}
impl Bucket {
    pub fn full(burst: u64) -> Self {
        Self {
            tokens: burst as f64,
            last: std::time::Instant::now(),
        }
    }
    fn refill(&mut self, per_min: u64, burst: u64) {
        let now = std::time::Instant::now();
        self.tokens = (self.tokens
            + now.duration_since(self.last).as_secs_f64() * per_min as f64 / 60.0)
            .min(burst as f64);
        self.last = now;
    }
    pub fn available(&mut self, per_min: u64, burst: u64) -> u64 {
        self.refill(per_min, burst);
        self.tokens.max(0.0) as u64
    }
    /// Takes `n` bytes if they are there. Otherwise the seconds until they would be.
    pub fn take(&mut self, n: u64, per_min: u64, burst: u64) -> Result<(), u64> {
        self.refill(per_min, burst);
        if self.tokens >= n as f64 {
            self.tokens -= n as f64;
            Ok(())
        } else {
            Err((((n as f64 - self.tokens) * 60.0 / per_min.max(1) as f64).ceil() as u64).max(1))
        }
    }
}
