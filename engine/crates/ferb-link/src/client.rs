//! The phone side of the link, used as is by the Android app (through `ffi`), by `ferb-link phone` (a phone
//! simulator on the Mac) and by the tests. It keeps no files: its state is a small JSON value handed to the host
//! in a `persist` event, which the host stores encrypted (CONTRACT.md §8).
//!
//! It dials with backoff and jitter, stops for errors a redial cannot fix, never rebuilds its endpoint to
//! retry, and does nothing in the background unless the host says the link should stay up.

use crate::{
    MIN_VERSION, VERSION, diag,
    errors::{self, LinkError, LinkResult},
    limits::Limits,
    net::{self, NetConfig, RelayPolicy, RelaySpec, RelayState, Shared},
    pairing::{self, Invite},
    wire::{self, K_ACK, K_CHUNK, K_ERROR, K_EVENT, K_HELLO, K_REQ, K_RESULT, K_WELCOME},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use iroh::{
    Endpoint, EndpointId, RelayUrl, SecretKey,
    endpoint::{Connection, RecvStream, SendStream},
    tls::CaTlsConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{Notify, mpsc};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct MacTarget {
    pub id: String,
    /// Relays the Mac was last known to be reachable at.
    #[serde(default)]
    pub relays: Vec<String>,
    #[serde(default)]
    pub own: Option<RelaySpec>,
    #[serde(default)]
    pub paired: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhoneState {
    /// The phone's link identity (base64 of 32 bytes). The host keeps this state encrypted at rest.
    pub key: String,
    #[serde(default)]
    pub mac: Option<MacTarget>,
}

/// Signs a text with the phone's device key (base64 of the signature), or None.
pub type DeviceSign = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

pub struct PhoneConfig {
    pub state: Option<PhoneState>,
    pub name: String,
    pub app: String,
    /// The phone's lock-bound public key (P-256, SubjectPublicKeyInfo DER, base64), when it has a screen lock.
    pub unlock_pub: Option<String>,
    /// The phone's device key (P-256, hardware-held, no prompt): proves a connection comes from this very phone.
    pub device_pub: Option<String>,
    /// The phone's fingerprint-only key (P-256), when it has a strong biometric enrolled.
    pub strong_pub: Option<String>,
    /// Signs with that device key (base64 of the signature). Given, every new connection proves by itself that it
    /// comes from this phone; the Android app does the same from its side, with the key in the phone's keystore.
    pub device_sign: Option<DeviceSign>,
    pub shared: Shared,
    pub roots: Option<CaTlsConfig>,
    pub force_relay: bool,
    /// The link version this app speaks (tests change it).
    pub version: (u32, u32),
    /// The feature level this app says it has (`crate::LEVEL`; tests change it).
    pub level: u32,
    /// Redial timing, until the Mac's welcome replaces it with the Mac's limits.
    pub limits: Limits,
    /// Tests only: start over also when the Mac says no direct path can be had. (The test relays offer no address
    /// discovery, so every network report there says UDP does not work, and the Mac says just that.)
    pub start_over_without_hope: bool,
    /// QUIC's segmentation offload (`net::NetConfig::segmentation_offload`; upstream iroh #4647). On, as the lab ran.
    pub segmentation_offload: bool,
    /// Port mapping at the phone's gateway (`net::NetConfig::portmapper`). On; tests turn it off.
    pub portmapper: bool,
    /// Tests only: IP on 127.0.0.1 alone (`net::NetConfig::loopback_only`).
    pub loopback_only: bool,
}
impl Default for PhoneConfig {
    fn default() -> Self {
        Self {
            state: None,
            name: "Phone".into(),
            app: "lab".into(),
            unlock_pub: None,
            device_pub: None,
            strong_pub: None,
            device_sign: None,
            shared: Shared::N0,
            roots: None,
            force_relay: false,
            version: (VERSION, MIN_VERSION),
            level: crate::LEVEL,
            limits: Limits::default(),
            start_over_without_hope: false,
            segmentation_offload: true,
            portmapper: true,
            loopback_only: false,
        }
    }
}

struct Online {
    conn: Connection,
    since: Instant,
}

/// A connection made on a spare endpoint while the session went on over the old one: greeted, welcomed, and (when
/// the phone has a device key) proven to come from this very phone. The connection loop takes it over.
struct Prepared {
    conn: Connection,
    tx: SendStream,
    rx: RecvStream,
    welcome: Value,
    started: Instant,
    proved: bool,
}

/// Why and since when a relayed session is being started over on a schedule (kb NET-24).
struct Trigger {
    at: Instant,
    why: &'static str,
    /// When the next start-over is due.
    next: Instant,
    tries: u32,
    /// Start-overs in a row that did not get through.
    fails: u32,
    /// The endpoint's counters when the schedule began or last started over: what it did since, while relayed.
    base: diag::Counters,
}

/// How long calls are held at most while a start-over moves the session (they then go out on the new connection).
/// It covers a start-over's own limits below, so no call goes out on the old connection while the relay sends
/// to the new one.
const SWAP_HOLD: Duration = Duration::from_secs(12);
/// What one start-over may take: for calls under way to end, for the new endpoint, for its connection (the dial,
/// the hello, the proof), and in that for the app's signature. Past any of them it gives up.
const SWAP_QUIET: Duration = Duration::from_secs(2);
const SWAP_BIND: Duration = Duration::from_secs(3);
const SWAP_CONNECT: Duration = Duration::from_secs(6);
const SWAP_PROOF: Duration = Duration::from_secs(3);

struct PInner {
    /// Changes of network the app reported; start-overs that moved the session, and ones that did not get through.
    handovers: std::sync::atomic::AtomicU64,
    fresh_starts: std::sync::atomic::AtomicU64,
    failed_starts: std::sync::atomic::AtomicU64,
    /// A start-over is moving the session right now: nothing dials, and calls wait for the new connection.
    swap: tokio::sync::watch::Sender<bool>,
    /// The connection a start-over prepared, for the connection loop to take over.
    handoff: Mutex<Option<Prepared>>,
    /// The schedule of start-overs, while there is one.
    trigger: Mutex<Option<Trigger>>,
    /// The kind of network the app last said the phone is on.
    network: Mutex<Option<String>>,
    /// What the last start-over did (kinds, counts, times), for the Mac's record and the tests.
    last_start: Mutex<Value>,
    /// Counts the sessions of this run: the app proves the phone's presence again when it changes.
    sessions: std::sync::atomic::AtomicU64,
    /// Signatures the app owes for a start-over (it holds the device key): by request number.
    proofs: Mutex<std::collections::HashMap<u64, tokio::sync::oneshot::Sender<Option<String>>>>,
    next_proof: std::sync::atomic::AtomicU64,
    travelling: Arc<std::sync::atomic::AtomicU64>,
    /// Calls under way (a start-over waits for none).
    in_flight: Arc<std::sync::atomic::AtomicU64>,
    /// For the log: events the Mac pushed (how many, their bytes) and calls this phone made.
    pushed: std::sync::atomic::AtomicU64,
    pushed_bytes: std::sync::atomic::AtomicU64,
    calls: std::sync::atomic::AtomicU64,
    /// The feature level the Mac said in its welcome.
    mac_level: std::sync::atomic::AtomicU32,
    policy: Arc<RelayPolicy>,
    cfg: PhoneConfig,
    state: Mutex<PhoneState>,
    invite: Mutex<Option<Invite>>,
    /// The server's signed statement of this phone's FERB account (`/api/app/assertion`), sent in the pairing hello for
    /// the Mac's daemon to check (PHN-828); with the invite, used once.
    pair_account: Mutex<Option<String>>,
    online: Mutex<Option<Online>>,
    active: Mutex<bool>,
    stopped: Mutex<Option<LinkError>>,
    limits: Mutex<Limits>,
    wake: Notify,
    events: mpsc::UnboundedSender<Value>,
    phase: Mutex<Value>,
    dials: Mutex<Vec<Instant>>,
    redial_now: Mutex<bool>,
    failed_dials: std::sync::atomic::AtomicU32,
    closed: std::sync::atomic::AtomicBool,
}

#[derive(Clone)]
pub struct Phone {
    inner: Arc<PInner>,
}

pub struct Backoff {
    step: usize,
    failing_since: Option<Instant>,
}
impl Backoff {
    pub fn new() -> Self {
        Self {
            step: 0,
            failing_since: None,
        }
    }
    pub fn reset(&mut self) {
        *self = Self::new();
    }
    /// The wait before the next dial: the steps, then the last step for ever; after `redial_long_after_s` of
    /// failing, the long interval. Jitter spreads phones that failed together.
    pub fn next(&mut self, l: &Limits) -> Duration {
        let since = *self.failing_since.get_or_insert_with(Instant::now);
        let base = if since.elapsed().as_secs() >= l.redial_long_after_s {
            l.redial_long_every_s
        } else {
            let s = l
                .redial_steps_s
                .get(self.step)
                .or(l.redial_steps_s.last())
                .copied()
                .unwrap_or(60);
            self.step += 1;
            s
        };
        let j = l.redial_jitter_pct.min(90) as f64 / 100.0;
        let f = 1.0 + (rand::random::<f64>() * 2.0 - 1.0) * j;
        Duration::from_secs_f64((base as f64 * f).max(0.2))
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

fn link_closed() -> LinkError {
    LinkError::new(
        errors::MAC_UNREACHABLE,
        "The phone is not connected to FERB's Mac right now.",
    )
    .allow("It reconnects by itself; check that the Mac is awake and online")
}

impl Phone {
    pub async fn start(cfg: PhoneConfig) -> LinkResult<(Self, mpsc::UnboundedReceiver<Value>)> {
        let (events, rx) = mpsc::unbounded_channel();
        let fresh = cfg.state.is_none();
        diag::trace_if_asked();
        diag::log(&format!(
            "the link starts: build {}, {}",
            crate::build(),
            if cfg
                .state
                .as_ref()
                .is_some_and(|s| s.mac.as_ref().is_some_and(|m| m.paired))
            {
                "paired"
            } else {
                "not paired"
            }
        ));
        let mut state = cfg.state.clone().unwrap_or_else(|| {
            let mut k = [0u8; 32];
            rand::RngCore::fill_bytes(&mut rand::rng(), &mut k);
            PhoneState {
                key: B64.encode(k),
                mac: None,
            }
        });
        // a pairing that never finished does not survive a restart (its one-time secret is gone)
        state.mac = state.mac.filter(|m| m.paired);
        let key: [u8; 32] = B64
            .decode(&state.key)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| {
                LinkError::new(errors::BAD_REQUEST, "The saved link state is damaged.")
            })?;
        let net = NetConfig {
            roots: cfg.roots.clone(),
            force_relay: cfg.force_relay,
            relay_only: cfg.limits.own_relay_always,
            segmentation_offload: cfg.segmentation_offload,
            portmapper: cfg.portmapper,
            loopback_only: cfg.loopback_only,
            ..NetConfig::new(
                SecretKey::from_bytes(&key),
                state.mac.as_ref().and_then(|m| m.own.clone()),
                cfg.shared.clone(),
            )
        };
        let policy = RelayPolicy::start(net).await?;
        let limits = cfg.limits.clone();
        let inner = Arc::new(PInner {
            policy: policy.clone(),
            cfg,
            state: Mutex::new(state),
            mac_level: std::sync::atomic::AtomicU32::new(0),
            handovers: Default::default(),
            fresh_starts: Default::default(),
            failed_starts: Default::default(),
            swap: tokio::sync::watch::channel(false).0,
            handoff: Mutex::new(None),
            trigger: Mutex::new(None),
            network: Mutex::new(None),
            last_start: Mutex::new(Value::Null),
            sessions: Default::default(),
            proofs: Default::default(),
            next_proof: Default::default(),
            travelling: Default::default(),
            in_flight: Default::default(),
            pushed: Default::default(),
            pushed_bytes: Default::default(),
            calls: Default::default(),
            invite: Mutex::new(None),
            pair_account: Mutex::new(None),
            online: Mutex::new(None),
            active: Mutex::new(true),
            stopped: Mutex::new(None),
            limits: Mutex::new(limits),
            wake: Notify::new(),
            events,
            phase: Mutex::new(json!({"phase": "idle"})),
            dials: Default::default(),
            redial_now: Default::default(),
            failed_dials: Default::default(),
            closed: Default::default(),
        });
        let me = Self { inner };
        {
            let (i, j) = (me.inner.clone(), me.inner.clone());
            tokio::spawn(policy.clone().run(
                move || {
                    let l = i.limits.lock().unwrap();
                    (
                        Duration::from_secs(l.own_relay_fallback_after_s),
                        Duration::from_secs(l.own_relay_probe_s),
                        l.own_relay_fallback,
                        l.own_relay_always,
                    )
                },
                move || j.closed.load(std::sync::atomic::Ordering::Relaxed),
            ));
        }
        // The phone's own relay state, for the app: falling back to the public relay when the owner's relay cannot be
        // reached, and coming back to it (kb PHN-425, PHN-208), as an event the app can show.
        {
            let me = me.clone();
            let mut changes = policy.subscribe();
            tokio::spawn(async move {
                while changes.changed().await.is_ok() {
                    if me.inner.closed.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    let state = me.inner.policy.state();
                    diag::log(&format!(
                        "this phone's relay: {}",
                        serde_json::to_value(&state)
                            .ok()
                            .and_then(|v| v["state"].as_str().map(str::to_string))
                            .unwrap_or_default()
                    ));
                    me.emit(json!({"type": "relay_state", "relay": state}));
                }
            });
        }
        if fresh {
            me.persist();
        }
        tokio::spawn(me.clone().run());
        tokio::spawn(me.clone().say_counters());
        tokio::spawn(me.clone().start_over_schedule());
        Ok((me, rx))
    }

    pub fn id(&self) -> String {
        self.ep().id().to_string()
    }
    pub fn endpoint(&self) -> Endpoint {
        self.ep()
    }
    fn ep(&self) -> Endpoint {
        self.inner.policy.endpoint()
    }
    fn emit(&self, v: Value) {
        let _ = self.inner.events.send(v);
    }
    fn persist(&self) {
        self.emit(json!({"type": "persist", "state": *self.inner.state.lock().unwrap()}));
    }
    fn set_phase(&self, v: Value) {
        let brief = |p: &Value| {
            format!(
                "{}{}{}{}",
                p["phase"].as_str().unwrap_or("?"),
                p["tier"]
                    .as_str()
                    .map(|t| format!(" on {t}"))
                    .unwrap_or_default(),
                p["error"]["code"]
                    .as_str()
                    .map(|c| format!(" ({c})"))
                    .unwrap_or_default(),
                p["retry_in_s"]
                    .as_u64()
                    .map(|s| format!(", next dial in {s} s"))
                    .unwrap_or_default()
            )
        };
        let (was, now) = (brief(&self.inner.phase.lock().unwrap()), brief(&v));
        if was != now {
            diag::log(&format!("state: {now}"));
        }
        // What this path carries (kb PHN-444): pictures, files and older pages are refused on the public relay
        // and offered again when the path improves; the app hears each change once.
        let carries = |p: &Value| {
            (p["phase"] == "online")
                .then(|| {
                    serde_json::from_value::<net::Tier>(p["tier"].clone())
                        .ok()
                        .map(|t| t.carries_bulk())
                })
                .flatten()
        };
        let (could, can) = (carries(&self.inner.phase.lock().unwrap()), carries(&v));
        *self.inner.phase.lock().unwrap() = v.clone();
        if let Some(allowed) = can.filter(|c| could != Some(*c)) {
            self.emit(json!({"type": "bulk", "allowed": allowed, "tier": v["tier"]}));
        }
        let mut e = self.with_own_relay(v);
        e["type"] = json!("state");
        self.emit(e);
    }
    /// Where the link is. Every phase carries `own_relay`: the owner's relay as this phone last learned it from the
    /// Mac (the pairing code, then each welcome or relay change, as saved), or null when none is set or no Mac is
    /// paired. So the app shows the owner's relay while connecting, offline or on the public relay too, not only
    /// once a welcome came (kb PHN-418, PHN-425).
    pub fn phase(&self) -> Value {
        let v = self.inner.phase.lock().unwrap().clone();
        self.with_own_relay(v)
    }
    /// The phase with the saved own relay's address in `own_relay` (null without one).
    fn with_own_relay(&self, mut v: Value) -> Value {
        let own = self
            .inner
            .state
            .lock()
            .unwrap()
            .mac
            .as_ref()
            .and_then(|m| m.own.as_ref())
            .map(|o| o.url.clone());
        if let Some(m) = v.as_object_mut() {
            m.insert("own_relay".into(), json!(own));
        }
        v
    }
    pub fn is_online(&self) -> bool {
        self.inner.online.lock().unwrap().is_some()
    }
    pub fn limits(&self) -> Limits {
        self.inner.limits.lock().unwrap().clone()
    }
    /// Once a minute, one line for whoever reads the phone's log: where the link is and what the endpoint did
    /// in that minute (bytes by the way they travelled, address discovery rounds, tries for a direct path).
    async fn say_counters(self) {
        let mut before = diag::counters(&self.ep());
        let take =
            |n: &std::sync::atomic::AtomicU64| n.swap(0, std::sync::atomic::Ordering::Relaxed);
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if self.inner.closed.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            let ep = self.ep();
            let now = diag::counters(&ep);
            let facts = net::facts(&ep);
            let paths = self
                .connection()
                .map(|c| diag::paths(&c))
                .unwrap_or_else(|| "no connection".into());
            diag::log(&format!(
                "last minute: {} · the Mac pushed {} events ({} bytes), this phone made {} calls · paths: {paths} · this side offers: {} · home relays connected: {} · dials so far {} · network changes {} · start-overs {} (not through: {})",
                now.since(&before).words(),
                take(&self.inner.pushed),
                take(&self.inner.pushed_bytes),
                take(&self.inner.calls),
                if facts.offers.is_empty() {
                    "nothing".to_string()
                } else {
                    facts.offers.join(" ")
                },
                net::home_relays(&ep).len(),
                self.inner.dials.lock().unwrap().len(),
                self.inner
                    .handovers
                    .load(std::sync::atomic::Ordering::Relaxed),
                self.fresh_starts(),
                self.inner
                    .failed_starts
                    .load(std::sync::atomic::Ordering::Relaxed)
            ));
            before = now;
        }
    }
    /// How many dials this phone has made (tests: backoff).
    pub fn dial_times(&self) -> Vec<Instant> {
        self.inner.dials.lock().unwrap().clone()
    }

    /// Start pairing from the scanned QR text. Replaces any earlier pairing on this phone. No account statement: the
    /// Mac's daemon refuses it unless it is a test's (PHN-828); the apps use [`Phone::pair_with_account`].
    pub fn pair(&self, qr: &str) -> LinkResult<()> {
        self.pair_with_account(qr, None)
    }

    /// Start pairing from the scanned QR text with the server's signed statement of this phone's FERB account, which
    /// the hello carries for the Mac's daemon to check against its own account (PHN-828). PHN-102's proof is unchanged.
    pub fn pair_with_account(&self, qr: &str, account: Option<&str>) -> LinkResult<()> {
        let invite = Invite::parse(qr)?;
        // A pairing link never replaces the Mac this phone is paired with: the owner forgets that Mac first.
        if self
            .inner
            .state
            .lock()
            .unwrap()
            .mac
            .as_ref()
            .is_some_and(|m| m.paired)
        {
            return Err(LinkError::new(
                errors::ALREADY_PAIRED,
                "This phone is already paired with a Mac. A pairing code cannot replace it.",
            )
            .allow("To pair with another Mac, choose Forget this Mac first"));
        }
        {
            let mut s = self.inner.state.lock().unwrap();
            s.mac = Some(MacTarget {
                id: invite.mac.to_string(),
                relays: invite.relays.clone(),
                own: invite.own.clone(),
                paired: false,
            });
        }
        *self.inner.invite.lock().unwrap() = Some(invite);
        *self.inner.pair_account.lock().unwrap() = account
            .map(str::trim)
            .filter(|a| !a.is_empty() && a.len() <= 4096)
            .map(str::to_owned);
        *self.inner.stopped.lock().unwrap() = None;
        self.drop_conn();
        self.inner.wake.notify_one();
        Ok(())
    }
    /// Forget the Mac on this phone. (The Mac's list keeps the phone until the owner removes it there.)
    pub fn forget(&self) {
        self.inner.state.lock().unwrap().mac = None;
        *self.inner.invite.lock().unwrap() = None;
        self.drop_conn();
        self.persist();
        self.set_phase(json!({"phase": "idle"}));
        self.inner.wake.notify_one();
    }
    /// Foreground/background. In the background the link is closed and nothing dials, unless the host keeps
    /// it up on purpose (Android's "stay connected" service calls `set_active(true)`).
    pub fn set_active(&self, on: bool) {
        *self.inner.active.lock().unwrap() = on;
        if !on {
            self.drop_conn();
            self.set_phase(json!({"phase": "paused"}));
        } else {
            // the app was opened: a session that is on a relay is worth starting over again, and the owner's relay is
            // looked at afresh (probed at once when the public relay stands in for it)
            self.arm("opened");
            self.inner.policy.recheck();
        }
        self.inner.wake.notify_one();
    }
    /// The phone's network changed: iroh is told, and one dial happens at once if the link is down.
    pub async fn network_changed(&self) {
        self.network_changed_to(None).await
    }
    /// The same, with the kind of network the phone is on now when the app knows it (for the log).
    pub async fn network_changed_to(&self, kind: Option<&str>) {
        diag::log(&format!(
            "the app says the phone's network changed{}",
            kind.map(|k| format!(" (now {k})")).unwrap_or_default()
        ));
        *self.inner.network.lock().unwrap() = kind.map(str::to_string);
        self.inner
            .handovers
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.ep().network_change().await;
        // the owner's relay is looked at afresh on the new network (probed at once during a fallback)
        self.inner.policy.recheck();
        self.inner.wake.notify_one();
        self.arm("network");
    }

    // ---- starting over while a session stays on a relay (kb NET-24, NET-25, NET-27) --------------------------
    //
    // Most sessions find a direct path again by themselves within seconds of a change of network. One that does
    // not was seen to stay on the relay for as long as it kept its sockets (the owner's phone, Wi-Fi to mobile
    // data): neither side's direct packets reached the other, while a connection begun later on new sockets was
    // direct at once. When that is cannot be known in advance (it was 150 s once, 300 s another time), so while
    // the session is on a relay the phone starts over every `handover_retry_every_s`, then slower, then not at
    // all, counted from the last change of network or opening of the app.
    //
    // Starting over costs the session nothing: the new endpoint is made and connected, greeted and proven to be
    // this phone while the session goes on over the old one; then the session moves and the old endpoint is
    // closed. Calls made in that moment wait for the new connection. It never happens on a direct connection
    // and never while a call or a file is under way.

    /// Begins (or begins again) the schedule of start-overs.
    fn arm(&self, why: &'static str) {
        let every = self.limits().handover_retry_every_s;
        if every == 0 {
            return;
        }
        let base = diag::counters(&self.ep());
        let now = Instant::now();
        let mut t = self.inner.trigger.lock().unwrap();
        // (the app coming back to the front many times must not push a try that is nearly due further away)
        let next = match t.as_ref() {
            Some(old) if why == "opened" => old.next.min(now + Duration::from_secs(every)),
            _ => now + Duration::from_secs(every),
        };
        *t = Some(Trigger {
            at: now,
            why,
            next,
            tries: 0,
            fails: 0,
            base,
        });
    }
    /// The seconds after the schedule's start at which the next try is due, after one at `at` (None: no more).
    pub fn next_start_over(l: &Limits, at: u64) -> Option<u64> {
        if l.handover_retry_every_s == 0 {
            return None;
        }
        let next = if at < l.handover_retry_fast_for_s {
            at + l.handover_retry_every_s
        } else if l.handover_retry_slow_every_s > 0 {
            at + l.handover_retry_slow_every_s
        } else {
            return None;
        };
        (next <= l.handover_retry_stop_after_s).then_some(next)
    }
    /// Where the session is, as this phone itself sees its connection: on a relay?
    fn relayed(&self) -> bool {
        let phase = self.phase();
        let own = self.inner.policy.own_url();
        phase["phase"] == "online"
            && phase["tier"].as_str().is_some_and(|t| t != "direct")
            && self
                .connection()
                .is_some_and(|c| net::tier(&c, own.as_ref()) != net::Tier::Direct)
    }
    async fn start_over_schedule(self) {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if self.inner.closed.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            let l = self.limits();
            let phase = self.phase();
            let run = {
                let mut t = self.inner.trigger.lock().unwrap();
                let Some(tr) = t.as_mut() else { continue };
                let el = tr.at.elapsed().as_secs();
                if phase["phase"] == "online" && phase["tier"] == "direct" {
                    // direct: the schedule is over; nothing here ever touches a direct session
                    *t = None;
                    continue;
                }
                if l.handover_retry_every_s == 0 || el > l.handover_retry_stop_after_s {
                    *t = None;
                    continue;
                }
                if Instant::now() < tr.next || !*self.inner.active.lock().unwrap() {
                    continue;
                }
                // due. Not while the link is down or redialing, nor in the first seconds of a connection (a dial
                // is a new connection already), not while a call or a file is under way (it stays due), not when
                // the Mac says a direct path cannot be had
                let young = Duration::from_millis(l.handover_retry_every_s.min(10) * 500);
                if !self.relayed()
                    || self.held_open()
                    || self.session_age().is_none_or(|a| a < young)
                {
                    continue;
                }
                // decision: PHN-445: on the public relays no timed start-overs (each costs about 135 kB there);
                // iroh's own try for a direct path goes on. The owner's relay keeps the lab's schedule.
                let public = phase["tier"] == json!("shared_relay");
                let pointless = public
                    || (phase["direct"]["trying"] == json!(false)
                        && !self.inner.cfg.start_over_without_hope);
                let (n, why, base) = (tr.tries + 1, tr.why, tr.base);
                match Self::next_start_over(&l, el) {
                    Some(n) => tr.next = tr.at + Duration::from_secs(n),
                    None => *t = None, // that was the last one
                }
                if pointless {
                    continue;
                }
                if let Some(tr) = t.as_mut() {
                    tr.tries = n;
                }
                (n, el, why, base)
            };
            let (n, el, why, base) = run;
            let mut facts = self.start_over().await;
            facts["n"] = json!(n);
            facts["since_s"] = json!(el);
            facts["trigger"] = json!(why);
            facts["network"] = json!(self.inner.network.lock().unwrap().clone());
            // what the endpoint that was in use did while relayed: tries for a direct path, bytes on IP either way
            let before = facts["before"].take();
            if let Ok(c) = serde_json::from_value::<[u64; 5]>(before) {
                let d = |now: u64, was: u64| now.saturating_sub(was);
                facts["relayed"] = json!({"tries": d(c[0], base.punches), "tx_v4": d(c[1], base.tx_v4), "rx_v4": d(c[2], base.rx_v4), "tx_v6": d(c[3], base.tx_v6), "rx_v6": d(c[4], base.rx_v6)});
            }
            let moved = facts["moved"] == json!(true);
            // one that found the session busy or direct after all is no failure; one that could not connect is
            let tried = !matches!(
                facts["reason"].as_str(),
                Some(
                    "busy"
                        | "not_on_a_relay"
                        | "went_direct_by_itself"
                        | "link_stopped"
                        | "already_starting_over"
                )
            );
            {
                let mut t = self.inner.trigger.lock().unwrap();
                if let Some(tr) = t.as_mut() {
                    tr.base = diag::counters(&self.ep());
                    tr.fails = if moved { 0 } else { tr.fails + tried as u32 };
                    if tr.fails >= self.limits().handover_retry_fails_max.max(1) {
                        diag::log(&format!(
                            "{} start-overs in a row did not get through: no more until the network changes or the app is opened",
                            tr.fails
                        ));
                        *t = None;
                    }
                }
            }
            diag::log(&format!(
                "start-over {n}, {el} s after {}: {}",
                if why == "network" {
                    "the network changed"
                } else {
                    "the app was opened"
                },
                if moved {
                    format!(
                        "the session moved to a new endpoint in {} ms (new ports: {}), calls were held {} ms",
                        facts["took_ms"], facts["ports_changed"], facts["held_ms"]
                    )
                } else {
                    format!("not through ({})", facts["reason"].as_str().unwrap_or("?"))
                }
            ));
            *self.inner.last_start.lock().unwrap() = facts.clone();
            // the Mac keeps the record (a Mac from before this ignores the note)
            let me = self.clone();
            tokio::spawn(async move {
                if moved {
                    // (a little later, so the new endpoint's first seconds are in its cost)
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    let c = diag::counters(&me.ep());
                    facts["cost"] = json!({"tx": c.tx_relay + c.tx_v4 + c.tx_v6, "rx": c.rx_relay + c.rx_v4 + c.rx_v6});
                    facts["direct_3s"] = json!(!me.relayed() && me.is_online());
                    *me.inner.last_start.lock().unwrap() = facts.clone();
                }
                let _ = me
                    .call("link.note", json!({"handover": facts}), None, |_| {})
                    .await;
            });
        }
    }
    /// One start-over: a spare endpoint, a connection on it, the session moved to it. What happened, as facts.
    async fn start_over(&self) -> Value {
        let i = &self.inner;
        let not = |reason: &str| json!({"moved": false, "reason": reason});
        if i.swap.send_replace(true) {
            return not("already_starting_over");
        }
        let held = Instant::now();
        let give_up = |reason: &str| {
            i.failed_starts
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            i.swap.send_replace(false);
            i.wake.notify_one();
            let mut v = not(reason);
            v["held_ms"] = json!(held.elapsed().as_millis() as u64);
            v
        };
        // calls that began before this moment end first; new ones wait
        let quiet = tokio::time::timeout(SWAP_QUIET, async {
            while self.busy() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .is_ok();
        let target = i.state.lock().unwrap().mac.clone().filter(|m| m.paired);
        let (Some(old_conn), Some(target)) = (
            self.connection()
                .filter(|_| quiet && !self.held_open() && self.relayed()),
            target,
        ) else {
            return give_up(if quiet { "not_on_a_relay" } else { "busy" });
        };
        let old_ep = self.ep();
        let c = diag::counters(&old_ep);
        let before = json!([c.punches, c.tx_v4, c.rx_v4, c.tx_v6, c.rx_v6]);
        let spare = match tokio::time::timeout(SWAP_BIND, i.policy.spare()).await {
            Ok(Ok(ep)) => ep,
            _ => {
                let mut v = give_up("no_new_endpoint");
                v["before"] = before;
                return v;
            }
        };
        let prepared = tokio::time::timeout(SWAP_CONNECT, self.prepare(&spare, &target)).await;
        let still = !i.closed.load(std::sync::atomic::Ordering::Relaxed)
            && *i.active.lock().unwrap()
            && i.stopped.lock().unwrap().is_none();
        // (the session may have found a direct path by itself meanwhile: then it stays where it is)
        let went_direct = self
            .connection()
            .is_some_and(|c| net::tier(&c, i.policy.own_url().as_ref()) == net::Tier::Direct);
        let prepared = match prepared {
            Ok(Ok(p)) if still && !went_direct => p,
            other => {
                let reason = match &other {
                    Ok(Ok(_)) if went_direct => "went_direct_by_itself".to_string(),
                    Ok(Ok(_)) => "link_stopped".to_string(),
                    Ok(Err(e)) => e.code.clone(),
                    Err(_) => "no_answer".to_string(),
                };
                if let Ok(Ok(p)) = other {
                    p.conn.close(0u32.into(), b"bye");
                }
                // the spare goes completely; the relay then sends to the old endpoint again
                spare.close().await;
                let mut v = give_up(&reason);
                v["before"] = before;
                return v;
            }
        };
        let ports_changed = net::local_ports(&spare)
            .iter()
            .all(|p| !net::local_ports(&old_ep).contains(p));
        let (took, proved) = (held.elapsed().as_millis() as u64, prepared.proved);
        *i.handoff.lock().unwrap() = Some(prepared);
        let old = i.policy.install(spare);
        old_conn.close(0u32.into(), b"moved");
        i.fresh_starts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        i.wake.notify_one();
        tokio::spawn(async move { old.close().await });
        // until the connection loop serves the new connection (it opens the gate for calls)
        let mut gate = i.swap.subscribe();
        let _ = tokio::time::timeout(Duration::from_secs(5), gate.wait_for(|s| !*s)).await;
        json!({"moved": true, "ports_changed": ports_changed, "proved": proved, "took_ms": took, "held_ms": held.elapsed().as_millis() as u64, "before": before})
    }
    /// A connection on the spare endpoint, greeted and welcomed, and proven to come from this phone.
    async fn prepare(&self, spare: &Endpoint, target: &MacTarget) -> LinkResult<Prepared> {
        let started = Instant::now();
        let conn = self.connect_on(spare, target).await?;
        let (tx, rx, welcome) = match self.greet(&conn, false).await {
            Ok(x) => x,
            Err(e) => {
                conn.close(0u32.into(), b"bye");
                return Err(e);
            }
        };
        // The phone's unlock carries over only to a connection that proves the phone's device key (kb SEC-26):
        // proven before the session moves, so no call ever meets a locked Mac in between. A phone without that
        // key moves all the same and unlocks again, as after any reconnect.
        let proved = self.prove_on(&conn).await;
        Ok(Prepared {
            conn,
            tx,
            rx,
            welcome,
            started,
            proved,
        })
    }
    async fn prove_on(&self, conn: &Connection) -> bool {
        if self.inner.cfg.device_pub.is_none() {
            return false;
        }
        let Ok(ch) = self
            .call_on(conn, "link.challenge", json!({}), None, |_| {})
            .await
        else {
            return false;
        };
        let payload = Self::call_payload(
            ch["challenge"].as_str().unwrap_or_default(),
            "link.present",
            &json!({}),
        );
        let sig = match self.inner.cfg.device_sign.clone() {
            Some(sign) => sign(&payload),
            None => {
                // the app holds the key: it is asked for the signature and answers with the `proof` command
                let (tx, rx) = tokio::sync::oneshot::channel();
                let id = self
                    .inner
                    .next_proof
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1;
                self.inner.proofs.lock().unwrap().insert(id, tx);
                self.emit(json!({"type": "prove", "id": id, "payload": payload}));
                let got = tokio::time::timeout(SWAP_PROOF, rx)
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .flatten();
                self.inner.proofs.lock().unwrap().remove(&id);
                got
            }
        };
        let Some(sig) = sig else { return false };
        self.call_on(
            conn,
            "link.present",
            json!({}),
            Some(json!({"payload": payload, "sig": sig})),
            |_| {},
        )
        .await
        .is_ok_and(|r| r["present"] == json!(true))
    }
    /// The app's answer to a `prove` event: the device key's signature over that event's payload (None: it has none).
    pub fn proof(&self, id: u64, sig: Option<String>) {
        let tx = self.inner.proofs.lock().unwrap().remove(&id);
        if let Some(tx) = tx {
            let _ = tx.send(sig);
        }
    }
    /// A file is travelling or a call that stays open (a task's stream) is open: nothing is started over then.
    fn held_open(&self) -> bool {
        self.inner
            .travelling
            .load(std::sync::atomic::Ordering::Relaxed)
            > 0
    }
    /// Any call or file is under way right now (a start-over lets the short ones end first).
    fn busy(&self) -> bool {
        self.held_open()
            || self
                .inner
                .in_flight
                .load(std::sync::atomic::Ordering::Relaxed)
                > 0
    }
    /// How often this phone moved its session to a new endpoint (tests, and the status).
    pub fn fresh_starts(&self) -> u64 {
        self.inner
            .fresh_starts
            .load(std::sync::atomic::Ordering::Relaxed)
    }
    /// Start-overs that did not get through (the session stayed where it was).
    pub fn failed_starts(&self) -> u64 {
        self.inner
            .failed_starts
            .load(std::sync::atomic::Ordering::Relaxed)
    }
    /// What the last start-over did.
    pub fn last_start(&self) -> Value {
        self.inner.last_start.lock().unwrap().clone()
    }
    /// Whether a schedule of start-overs is running: the seconds until its next try.
    pub fn start_over_due(&self) -> Option<u64> {
        self.inner
            .trigger
            .lock()
            .unwrap()
            .as_ref()
            .map(|t| t.next.saturating_duration_since(Instant::now()).as_secs())
    }
    /// Try again now (the owner pressed Retry), also after a final error such as an updated app.
    pub fn retry_now(&self) {
        *self.inner.stopped.lock().unwrap() = None;
        self.inner.wake.notify_one();
    }
    fn drop_conn(&self) {
        let online = self.inner.online.lock().unwrap().take();
        if let Some(o) = online {
            o.conn.close(0u32.into(), b"bye");
        }
    }
    pub async fn close(&self) {
        self.inner
            .closed
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.drop_conn();
        self.inner.wake.notify_one();
        self.ep().close().await;
    }

    /// The connection for a new call. While a start-over moves the session the call waits for the new connection
    /// (it would get no answer on the old one).
    async fn conn_ready(&self) -> LinkResult<Connection> {
        if *self.inner.swap.borrow() {
            let mut gate = self.inner.swap.subscribe();
            let _ = tokio::time::timeout(SWAP_HOLD, gate.wait_for(|s| !*s)).await;
        }
        self.conn()
    }
    fn conn(&self) -> LinkResult<Connection> {
        match &*self.inner.online.lock().unwrap() {
            Some(o) => Ok(o.conn.clone()),
            None => Err(self
                .inner
                .stopped
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| {
                    self.phase()
                        .get("error")
                        .and_then(|e| serde_json::from_value(e.clone()).ok())
                        .unwrap_or_else(link_closed)
                })),
        }
    }

    // ---- the connection loop ------------------------------------------------------------------------------

    async fn run(self) {
        let mut backoff = Backoff::new();
        loop {
            if self.inner.closed.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            let target = self.inner.state.lock().unwrap().mac.clone();
            // (a start-over that prepared a connection hands it over here; while one is under way nothing dials)
            let handed = self.inner.handoff.lock().unwrap().take();
            let wanted = *self.inner.active.lock().unwrap()
                && self.inner.stopped.lock().unwrap().is_none()
                && (handed.is_some() || !*self.inner.swap.borrow());
            let Some(target) = target.filter(|_| wanted) else {
                if let Some(p) = handed {
                    p.conn.close(0u32.into(), b"bye");
                    self.inner.swap.send_replace(false);
                    continue;
                }
                self.inner.wake.notified().await;
                backoff.reset();
                continue;
            };
            let ended = |conn: &Connection, started: Instant, e: &LinkError| {
                diag::log(&format!(
                    "the connection ended after {} s ({}); paths at the end: {}; closed by: {}",
                    started.elapsed().as_secs(),
                    e.code,
                    diag::paths(conn),
                    match conn.close_reason() {
                        Some(iroh::endpoint::ConnectionError::ApplicationClosed(_)) => "the Mac",
                        Some(iroh::endpoint::ConnectionError::LocallyClosed) => "this phone",
                        Some(iroh::endpoint::ConnectionError::TimedOut) => "silence (timed out)",
                        Some(_) => "the transport",
                        None => "nobody yet",
                    }
                ));
            };
            let err = if let Some(p) = handed {
                // the session goes on over the connection a start-over made: no dial, and the phase stays online
                let (conn, started) = (p.conn.clone(), p.started);
                let e = self
                    .serve(p.conn, p.tx, p.rx, p.welcome, true, p.proved)
                    .await;
                ended(&conn, started, &e);
                if started.elapsed().as_secs() >= self.limits().healthy_reset_s {
                    backoff.reset();
                }
                e
            } else {
                // the phone uses the same own relay as the Mac (learned from the QR or over the link)
                if self.inner.policy.own() != target.own {
                    let _ = self.inner.policy.set_own(target.own.clone()).await;
                }
                self.set_phase(
                    json!({"phase": if target.paired { "connecting" } else { "pairing" }}),
                );
                self.inner.dials.lock().unwrap().push(Instant::now());
                let started = Instant::now();
                match self.connect_once(&target).await {
                    Ok(conn) => {
                        self.inner
                            .failed_dials
                            .store(0, std::sync::atomic::Ordering::Relaxed);
                        diag::log(&format!(
                            "dial answered after {} ms; paths: {}",
                            started.elapsed().as_millis(),
                            diag::paths(&conn)
                        ));
                        let e = self.online(conn.clone()).await;
                        ended(&conn, started, &e);
                        if started.elapsed().as_secs() >= self.limits().healthy_reset_s {
                            backoff.reset();
                        }
                        e
                    }
                    Err(e) => {
                        self.inner
                            .failed_dials
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        diag::log(&format!(
                            "dial failed after {} ms ({}); home relays connected: {}",
                            started.elapsed().as_millis(),
                            e.code,
                            net::home_relays(&self.ep()).len()
                        ));
                        e
                    }
                }
            };
            self.inner.online.lock().unwrap().take();
            if self.inner.handoff.lock().unwrap().is_some() {
                continue; // the session moved to a new endpoint: it goes on there
            }
            if !*self.inner.active.lock().unwrap() || self.inner.state.lock().unwrap().mac.is_none()
            {
                continue;
            }
            if std::mem::take(&mut *self.inner.redial_now.lock().unwrap()) {
                continue; // the relay setting changed: one dial on the new relay, no wait
            }
            // a phone holding a pairing code that the Mac turned away without a word: the code is used up or expired
            let err = if err.code == errors::NOT_PAIRED && !target.paired {
                LinkError::new(
                    errors::PAIR_WINDOW_CLOSED,
                    "The pairing code is no longer valid (it works once, for a short time).",
                )
                .allow("Choose Pair a phone on the Mac again and scan the new code")
            } else {
                err
            };
            if err.is_final() {
                if matches!(
                    err.code.as_str(),
                    errors::NOT_PAIRED | errors::DEVICE_REVOKED
                ) || !target.paired
                {
                    // this phone is no longer (or never became) a paired phone: stop for good
                    let mut s = self.inner.state.lock().unwrap();
                    if let Some(m) = s.mac.as_mut() {
                        m.paired = false;
                    }
                }
                *self.inner.invite.lock().unwrap() = None;
                *self.inner.stopped.lock().unwrap() = Some(err.clone());
                self.persist();
                self.set_phase(json!({"phase": "stopped", "error": err}));
                continue;
            }
            // a rate-limit signal from the Mac is respected; otherwise the backoff decides
            let wait = match err.retry_after_s {
                Some(s) => Duration::from_secs(s).max(backoff.next(&self.limits())),
                None => backoff.next(&self.limits()),
            };
            self.set_phase(json!({"phase": "waiting", "error": err, "retry_in_s": wait.as_secs_f64().ceil() as u64}));
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = self.inner.wake.notified() => {}
            }
        }
    }

    /// Why a dial failed, from facts: is the phone itself connected to a relay?
    fn unreachable(&self, target: &MacTarget, why: &str) -> LinkError {
        if net::home_relays(&self.ep()).is_empty() {
            return match self.inner.policy.state() {
                RelayState::OwnDown { error: Some(e) }
                | RelayState::OwnFallback { error: Some(e) } => e,
                _ if target.own.is_some() => LinkError::new(
                    errors::OWN_RELAY_UNREACHABLE,
                    "The phone cannot reach your relay.",
                )
                .allow(
                    "Check the phone's internet connection and that your relay server is running",
                ),
                _ => LinkError::new(
                    errors::RELAY_UNREACHABLE,
                    "The phone cannot reach the relay.",
                )
                .allow("Check the phone's internet connection"),
            };
        }
        LinkError::new(errors::MAC_UNREACHABLE, format!("FERB's Mac did not answer ({why}). It may be asleep, offline, or FERB is not running."))
            .allow("Wake the Mac and check that FERB is running; the phone keeps trying")
    }

    async fn connect_once(&self, target: &MacTarget) -> Result<Connection, LinkError> {
        self.connect_on(&self.ep(), target).await
    }
    /// One dial on that endpoint (the one in use, or the spare of a start-over).
    async fn connect_on(&self, ep: &Endpoint, target: &MacTarget) -> Result<Connection, LinkError> {
        let l = self.limits();
        let id = EndpointId::from_str(&target.id)
            .map_err(|_| LinkError::new(errors::BAD_REQUEST, "The saved Mac id is damaged."))?;
        // Where to look for the Mac, all in one dial: iroh sends the connection's first packet to every relay of
        // the address at once and the first to answer carries it (kb PHN-740). With an own relay: only there, so
        // the connection cannot settle on a shared relay by accident; the shared relays are added when the own
        // relay is in fallback, or after two dials in a row found nobody (the Mac may have fallen back while this
        // phone still reaches the own relay). Without one: where the Mac was last seen and every shared relay,
        // since the Mac's home relay moves by itself with the network's latencies (on the owner's network every
        // 5 to 20 minutes) and a phone that was closed meanwhile cannot know where to.
        let fallback = matches!(self.inner.policy.state(), RelayState::OwnFallback { .. });
        let widen = fallback
            || self
                .inner
                .failed_dials
                .load(std::sync::atomic::Ordering::Relaxed)
                >= 2;
        let own = target.own.iter().map(|o| o.url.clone());
        let last = target.relays.iter().cloned();
        let shared = self
            .inner
            .cfg
            .shared
            .urls()
            .into_iter()
            .map(|u| u.to_string());
        let wanted: Vec<String> = match (target.own.is_some(), widen) {
            (true, false) => own.collect(),
            (true, true) => own.chain(last).chain(shared).collect(),
            (false, _) => last.chain(shared).collect(),
        };
        let mut relays: Vec<RelayUrl> = vec![];
        for r in wanted {
            if let Ok(u) = RelayUrl::from_str(&r)
                && !relays.contains(&u)
            {
                relays.push(u);
            }
        }
        // No waiting for this phone's own home relay first (kb PHN-741): iroh connects to each relay it sends
        // through and holds the first packet until that relay connection is up (QUIC sends it again if that took
        // longer), and the Mac answers through the relay the packet came by. The home relay comes from iroh's first
        // network report, which took 4 s on the owner's phone: that was the dial's first 4 s.
        let addr = net::dial_addr(id, &relays);
        let conn = match tokio::time::timeout(
            Duration::from_secs(l.dial_deadline_s),
            ep.connect(addr, net::ALPN),
        )
        .await
        {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => return Err(self.unreachable(target, &format!("{e:#}"))),
            Err(_) => {
                return Err(
                    self.unreachable(target, &format!("no answer in {} s", l.dial_deadline_s))
                );
            }
        };
        Ok(conn)
    }

    /// Hello, (pairing), welcome, then the event stream until the connection ends. Returns why it ended.
    async fn online(&self, conn: Connection) -> LinkError {
        match self.greet(&conn, true).await {
            Ok((tx, rx, welcome)) => {
                // the phone's presence is proved on this connection before the session goes online, as before a
                // session moves: no call ever meets a locked Mac between "online" and the proof (kb PHN-799)
                let proved = self.prove_on(&conn).await;
                self.serve(conn, tx, rx, welcome, false, proved).await
            }
            Err(e) => e,
        }
    }
    /// The hello and the Mac's welcome on a new connection (`pairing`: with the pairing code's proof, if one waits).
    async fn greet(
        &self,
        conn: &Connection,
        pairing: bool,
    ) -> Result<(SendStream, RecvStream, Value), LinkError> {
        let l = self.limits();
        let stall = Duration::from_secs(l.stall_s);
        let Ok((mut tx, mut rx)) = conn.open_bi().await else {
            return Err(why_closed(conn, link_closed()).await);
        };
        let invite = self
            .inner
            .invite
            .lock()
            .unwrap()
            .clone()
            .filter(|_| pairing);
        let proof = invite
            .as_ref()
            .map(|i| B64.encode(pairing::proof(&i.secret, i.mac, self.ep().id())));
        let account = invite
            .as_ref()
            .and_then(|_| self.inner.pair_account.lock().unwrap().clone());
        let hello = json!({"v": self.inner.cfg.version.0, "min_v": self.inner.cfg.version.1, "name": self.inner.cfg.name,
                           "app": self.inner.cfg.app, "level": self.inner.cfg.level, "build": crate::build(), "pair": proof, "unlock_pub": self.inner.cfg.unlock_pub, "device_pub": self.inner.cfg.device_pub, "strong_pub": self.inner.cfg.strong_pub,
                           "account": account});
        if let Err(e) = wire::write_json(&mut tx, K_HELLO, &hello).await {
            return Err(why_closed(conn, e).await);
        }
        // until the welcome: at most the owner's thinking time during pairing
        let welcome: Value = loop {
            let wait = if invite.is_some() {
                Duration::from_secs(l.pair_confirm_s + 15)
            } else {
                stall
            };
            match wire::read_frame(&mut rx, l.control_frame_max, wait, false).await {
                Ok(Some((K_WELCOME, b))) => match wire::parse(&b) {
                    Ok(v) => break v,
                    Err(e) => return Err(e),
                },
                Ok(Some((K_EVENT, b))) => {
                    if let Ok(v) = wire::parse::<Value>(&b)
                        && v.get("type").and_then(Value::as_str) == Some("pairing")
                    {
                        let code = invite.as_ref().map(|i| {
                            pairing::code(&pairing::proof(&i.secret, i.mac, self.ep().id()))
                        });
                        self.set_phase(json!({"phase": "pairing_wait_for_mac", "code": code}));
                    }
                }
                Ok(Some((K_ERROR, b))) => {
                    return Err(wire::parse::<LinkError>(&b).unwrap_or_else(|e| e));
                }
                Ok(_) => return Err(why_closed(conn, link_closed()).await),
                Err(e) => return Err(why_closed(conn, e).await),
            }
        };
        Ok((tx, rx, welcome))
    }
    /// The session on a welcomed connection: the event stream until the connection ends. `moved`: this connection
    /// takes a running session over from another endpoint; `proved`: it proved the phone's presence already.
    async fn serve(
        &self,
        conn: Connection,
        _tx: SendStream,
        mut rx: RecvStream,
        welcome: Value,
        moved: bool,
        proved: bool,
    ) -> LinkError {
        let l = self.limits();
        let stall = Duration::from_secs(l.stall_s);
        // paired (or still paired): remember where the Mac is and which relay to use
        *self.inner.invite.lock().unwrap() = None;
        if let Ok(lim) = serde_json::from_value::<Limits>(welcome["limits"].clone()) {
            *self.inner.limits.lock().unwrap() = lim;
        }
        self.learn_relay(&welcome["relay"], true).await;
        *self.inner.online.lock().unwrap() = Some(Online {
            conn: conn.clone(),
            since: Instant::now(),
        });
        // which build the Mac runs (a Mac from before levels says none: level 0)
        let mac_level = welcome["level"].as_u64().unwrap_or(0) as u32;
        self.inner
            .mac_level
            .store(mac_level, std::sync::atomic::Ordering::Relaxed);
        // (`session` counts this run's sessions; `proved`: this one already proved the phone's presence, so the
        // app need not; a session that moved keeps the phase online throughout)
        let session = self
            .inner
            .sessions
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        self.set_phase(json!({"phase": "online", "tier": welcome["tier"], "tier_words": welcome["tier_words"], "device": welcome["device"],
                              "mac": {"level": mac_level, "build": welcome["build"], "older": mac_level < self.inner.cfg.level, "newer": mac_level > self.inner.cfg.level},
                              "relay": welcome["relay"]["state"], "session": session, "proved": proved}));
        if moved {
            // calls that waited for the move go out now, on this connection
            self.inner.swap.send_replace(false);
        }

        if let Some(sign) = self.inner.cfg.device_sign.clone().filter(|_| !proved) {
            let me = self.clone();
            tokio::spawn(async move {
                let _ = me.present(|payload| sign(payload)).await;
            });
        }
        // the relay told this phone to slow down: the Mac, which enforces the budget, is told. (Its own task:
        // reading a frame must never be cancelled half way.)
        let me = self.clone();
        let watcher = tokio::spawn(async move {
            let mut seen = net::relay_counters(&me.ep()).ratelimited;
            let mut told = net::NetFacts::default();
            let mut told_at = Instant::now();
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                // what this phone's network looks like, for the Mac's "why not direct" line
                // (told again when something about the network changed, and the count of attempts at most twice a minute)
                let facts = net::facts(&me.ep());
                let same_but_attempts = net::NetFacts {
                    attempts: told.attempts,
                    ..facts.clone()
                } == told;
                if facts.known
                    && facts != told
                    && (!same_but_attempts || told_at.elapsed() >= Duration::from_secs(30))
                {
                    told = facts.clone();
                    told_at = Instant::now();
                    let _ = me
                        .call("link.note", json!({"net": facts}), None, |_| {})
                        .await;
                }
                let now = net::relay_counters(&me.ep()).ratelimited;
                if now > seen {
                    let _ = me
                        .call("link.note", json!({"ratelimited": true}), None, |_| {})
                        .await;
                }
                seen = now;
            }
        });
        let ended = loop {
            match wire::read_frame(&mut rx, l.control_frame_max, stall, true).await {
                Ok(Some((K_EVENT, b))) => {
                    self.inner
                        .pushed
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    self.inner
                        .pushed_bytes
                        .fetch_add(b.len() as u64, std::sync::atomic::Ordering::Relaxed);
                    let Ok(ev) = wire::parse::<Value>(&b) else {
                        continue;
                    };
                    match ev.get("type").and_then(Value::as_str) {
                        Some("relay") => {
                            let mut p = self.phase();
                            p["relay"] = ev["relay"]["state"].clone();
                            self.set_phase(p);
                            self.learn_relay(&ev["relay"], false).await
                        }
                        Some("tier") => {
                            let mut p = self.phase();
                            p["tier"] = ev["tier"].clone();
                            p["tier_words"] = ev["tier_words"].clone();
                            p["why"] = ev["why"].clone();
                            p["direct"] = ev["direct"].clone();
                            self.set_phase(p);
                        }
                        other => {
                            if other == Some("status") {
                                let n = |k: &str| {
                                    ev["status"][k].as_array().map(|a| a.len()).unwrap_or(0)
                                };
                                diag::log(&format!(
                                    "the Mac's status arrived: {} running, {} questions, {} finished",
                                    n("running"),
                                    n("approvals"),
                                    n("results")
                                ));
                            }
                            self.emit(ev)
                        }
                    }
                }
                Ok(_) => break why_closed(&conn, link_closed()).await,
                Err(e) => break why_closed(&conn, e).await,
            }
        };
        watcher.abort();
        ended
    }

    async fn learn_relay(&self, relay: &Value, paired_now: bool) {
        let own: Option<RelaySpec> = serde_json::from_value(relay["own"].clone()).ok().flatten();
        let homes: Vec<String> = serde_json::from_value(relay["homes"].clone()).unwrap_or_default();
        let changed = {
            let mut s = self.inner.state.lock().unwrap();
            let Some(m) = s.mac.as_mut() else { return };
            let before = m.clone();
            m.own = own.clone();
            if !homes.is_empty() {
                m.relays = homes;
            }
            if paired_now {
                m.paired = true;
            }
            *m != before
        };
        if changed {
            self.persist();
        }
        if self.inner.policy.own() != own {
            let _ = self.inner.policy.set_own(own).await;
            if !paired_now {
                // the relay setting changed on the Mac: this connection rode the old relay; dial afresh on the
                // new one (once, on purpose; not a retry)
                *self.inner.redial_now.lock().unwrap() = true;
                self.drop_conn();
            }
        }
    }

    // ---- calls ---------------------------------------------------------------------------------------------------

    /// One call. Stream methods deliver their events to `on_event`. The error says what stopped it.
    pub async fn call(
        &self,
        method: &str,
        params: Value,
        proof: Option<Value>,
        on_event: impl FnMut(Value),
    ) -> LinkResult<Value> {
        let conn = self.conn_ready().await?;
        let _under_way = Travelling::new(&self.inner.in_flight);
        // (a call that stays open, such as a task's own stream, counts like a file: nothing is started over under it)
        let _open = crate::role::is_stream(method).then(|| Travelling::new(&self.inner.travelling));
        self.call_on(&conn, method, params, proof, on_event).await
    }
    /// The same on a given connection (a start-over speaks on its new connection before the session moves there).
    async fn call_on(
        &self,
        conn: &Connection,
        method: &str,
        params: Value,
        proof: Option<Value>,
        mut on_event: impl FnMut(Value),
    ) -> LinkResult<Value> {
        self.inner
            .calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let l = self.limits();
        let stall = Duration::from_secs(l.stall_s);
        let streaming = crate::role::is_stream(method);
        let work = async {
            let (mut tx, mut rx) = conn.open_bi().await.map_err(|_| link_closed())?;
            let _ = tx.set_priority(1);
            wire::write_json(
                &mut tx,
                K_REQ,
                &json!({"method": method, "params": params, "proof": proof}),
            )
            .await?;
            let mut bulk: Option<Vec<u8>> = None;
            loop {
                match wire::read_frame(
                    &mut rx,
                    l.control_frame_max.max(l.chunk_bytes + 4),
                    stall,
                    true,
                )
                .await?
                {
                    // a reply too large for a control frame arrives as acknowledged chunks, under the tier's rules
                    Some((K_EVENT, b))
                        if bulk.is_none()
                            && wire::parse::<Value>(&b)
                                .is_ok_and(|v| v["bulk"]["meta"]["json"] == json!(true)) =>
                    {
                        bulk = Some(Vec::new())
                    }
                    Some((K_CHUNK, b)) if bulk.is_some() && b.len() >= 4 => {
                        bulk.as_mut().unwrap().extend_from_slice(&b[4..]);
                        wire::write_frame(&mut tx, K_ACK, &b[..4]).await?;
                    }
                    Some((K_EVENT, b)) => on_event(wire::parse(&b)?),
                    Some((K_RESULT, b)) => {
                        let _ = tx.finish();
                        return match bulk {
                            Some(data) => wire::parse::<Value>(&data),
                            None => wire::parse::<Value>(&b),
                        };
                    }
                    Some((K_ERROR, b)) => return Err(self.explain(wire::parse::<LinkError>(&b)?)),
                    _ => return Err(close_error(conn).unwrap_or_else(link_closed)),
                }
            }
        };
        if streaming {
            return work.await;
        }
        match tokio::time::timeout(Duration::from_secs(l.call_deadline_s + 5), work).await {
            Ok(r) => r,
            Err(_) => {
                self.probe_after_timeout(conn.clone());
                Err(LinkError::new(
                    errors::MAC_UNREACHABLE,
                    format!(
                        "FERB's Mac did not answer \"{method}\" in {} s.",
                        l.call_deadline_s + 5
                    ),
                )
                .allow("The phone is checking the connection"))
            }
        }
    }

    /// A refusal as the owner should read it. A Mac from before feature levels answers a method it does not know
    /// with "do it in FERB on the Mac"; from a Mac that is behind this app, that means its link is the older one.
    fn explain(&self, e: LinkError) -> LinkError {
        explain_refusal(
            e,
            self.inner
                .mac_level
                .load(std::sync::atomic::Ordering::Relaxed),
            self.inner.cfg.level,
        )
    }

    /// A call got no reply: say "reconnecting", and if the link is still silent at the give-up time, drop it so
    /// the dial loop takes over. QUIC alone would wait much longer.
    fn probe_after_timeout(&self, conn: Connection) {
        let me = self.clone();
        tokio::spawn(async move {
            let l = me.limits();
            let mut p = me.phase();
            p["slow"] = json!(true);
            me.set_phase(p);
            let left = Duration::from_secs(
                l.give_up_after_s
                    .saturating_sub(l.call_deadline_s + 5)
                    .max(5),
            );
            let ok = tokio::time::timeout(left, async {
                let (mut tx, mut rx) = conn.open_bi().await.ok()?;
                wire::write_json(
                    &mut tx,
                    K_REQ,
                    &json!({"method": "link.status", "params": {}}),
                )
                .await
                .ok()?;
                let _ = tx.finish();
                wire::read_frame(&mut rx, 64 * 1024, left, false)
                    .await
                    .ok()?
            })
            .await;
            if !matches!(ok, Ok(Some(_))) {
                conn.close(0u32.into(), b"silent");
            } else {
                let mut p = me.phase();
                if let Some(m) = p.as_object_mut() {
                    m.remove("slow");
                }
                me.set_phase(p);
            }
        });
    }

    /// A fresh one-time challenge from the Mac, for one sensitive call or one `link.unlock`.
    pub async fn challenge(&self) -> LinkResult<String> {
        let r = self.call("link.challenge", json!({}), None, |_| {}).await?;
        Ok(r["challenge"].as_str().unwrap_or_default().to_string())
    }
    /// Proves to the Mac that this connection comes from the paired phone itself: its device key signs a fresh
    /// challenge (no prompt). The Mac then lets the phone's unlock carry over a reconnect, and may word what waits.
    pub async fn present(&self, sign: impl FnOnce(&str) -> Option<String>) -> LinkResult<Value> {
        let ch = self.challenge().await?;
        let payload = Self::call_payload(&ch, "link.present", &json!({}));
        let sig = sign(&payload).ok_or_else(|| {
            LinkError::new(
                errors::BAD_REQUEST,
                "The phone's device key could not sign.",
            )
        })?;
        self.call(
            "link.present",
            json!({}),
            Some(json!({"payload": payload, "sig": sig})),
            |_| {},
        )
        .await
    }
    /// The text a lock-bound key signs for a sensitive call (or `link.unlock`, with empty params).
    pub fn call_payload(challenge: &str, method: &str, params: &Value) -> String {
        json!({"challenge": challenge, "method": method, "params": params}).to_string()
    }

    /// The text a lock-bound key signs for a hidden answer: the value only as its SHA-256, so what is signed and
    /// sent as proof never carries it.
    pub fn sealed_payload(approval_id: &str, challenge: &str, text: &str) -> String {
        json!({"approval_id": approval_id, "challenge": challenge, "method": "chat.answer", "sealed": B64.encode(Sha256::digest(text.as_bytes()))}).to_string()
    }

    /// The text a lock-bound key signs for an answer that needs the phone's unlock.
    pub fn answer_payload(
        approval_id: &str,
        challenge: &str,
        method: &str,
        params: &Value,
    ) -> String {
        json!({"approval_id": approval_id, "challenge": challenge, "method": method, "params": params}).to_string()
    }

    /// A picture or a file from the Mac. Returns (what the Mac said about it, the bytes).
    pub async fn get(
        &self,
        params: Value,
        progress: impl FnMut(u64, u64),
    ) -> LinkResult<(Value, Vec<u8>)> {
        let mut data = Vec::new();
        let r = self
            .get_with(params, |part| data.extend_from_slice(part), progress)
            .await?;
        Ok((r, data))
    }

    /// The same, handing each chunk to `sink` as it arrives (a file being written, so a paused or broken fetch
    /// keeps what it has and goes on later with `offset`). The reply's `received` says how many bytes came.
    pub async fn get_with(
        &self,
        params: Value,
        mut sink: impl FnMut(&[u8]),
        mut progress: impl FnMut(u64, u64),
    ) -> LinkResult<Value> {
        let conn = self.conn_ready().await?;
        let _busy = Travelling::new(&self.inner.travelling);
        let l = self.limits();
        let stall = Duration::from_secs(l.stall_s);
        let (mut tx, mut rx) = conn.open_bi().await.map_err(|_| link_closed())?;
        let _ = tx.set_priority(-1);
        let mut params = params;
        let proof = params.as_object_mut().and_then(|m| m.remove("proof")); // (a fresh unlock for a high-value file)
        wire::write_json(
            &mut tx,
            K_REQ,
            &json!({"method": "link.get", "params": params, "proof": proof}),
        )
        .await?;
        let (mut head, mut got, mut hash) = (Value::Null, 0u64, Sha256::new());
        loop {
            match wire::read_frame(
                &mut rx,
                l.control_frame_max.max(l.chunk_bytes + 4),
                stall.max(Duration::from_secs(l.call_deadline_s + 5)),
                false,
            )
            .await?
            {
                Some((K_EVENT, b)) => head = wire::parse::<Value>(&b)?["bulk"].take(),
                Some((K_CHUNK, b)) if b.len() >= 4 => {
                    hash.update(&b[4..]);
                    sink(&b[4..]);
                    got += b.len() as u64 - 4;
                    wire::write_frame(&mut tx, K_ACK, &b[..4]).await?;
                    progress(got, head["size"].as_u64().unwrap_or(0));
                }
                Some((K_RESULT, b)) => {
                    let mut r = wire::parse::<Value>(&b)?;
                    if got > 0
                        && head["sha256"].as_str() != Some(B64.encode(hash.finalize()).as_str())
                    {
                        return Err(LinkError::new(
                            errors::LINK_CLOSED,
                            "The transfer arrived damaged.",
                        ));
                    }
                    if got > 0 {
                        r["meta"] = head["meta"].take();
                    }
                    r["received"] = json!(got);
                    return Ok(r);
                }
                Some((K_ERROR, b)) => return Err(self.explain(wire::parse::<LinkError>(&b)?)),
                _ => return Err(close_error(&conn).unwrap_or_else(link_closed)),
            }
        }
    }

    /// Send bytes to the Mac's phone folder; the returned `upload_id` can ride with `chat.send` as `attachments`.
    pub async fn put(
        &self,
        name: &str,
        bytes: &[u8],
        progress: impl FnMut(u64, u64),
    ) -> LinkResult<Value> {
        self.put_from(
            json!({"name": name}),
            bytes.len() as u64,
            |at, out| {
                let at = at as usize;
                let n = out.len().min(bytes.len().saturating_sub(at));
                out[..n].copy_from_slice(&bytes[at..at + n]);
                Ok(n)
            },
            |_| {},
            progress,
        )
        .await
    }

    /// Send a file from the phone's storage, read piece by piece (never whole in memory). `opts` may carry `resume`
    /// (the id of an upload that stopped part way: the Mac says where to go on), and `group` + `rel` (a file inside a
    /// folder being sent). `accepted` hears the Mac's go-ahead (`upload_id`, `offset`) before the first byte.
    pub async fn put_file(
        &self,
        path: &std::path::Path,
        mut opts: Value,
        accepted: impl FnMut(&Value),
        progress: impl FnMut(u64, u64),
    ) -> LinkResult<Value> {
        use std::os::unix::fs::FileExt;
        let file = std::fs::File::open(path).map_err(|e| {
            LinkError::new(
                errors::NOT_FOUND,
                format!("The file could not be read on the phone ({e})."),
            )
        })?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        if opts.get("name").is_none() {
            opts["name"] = json!(
                path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "file".into())
            );
        }
        self.put_from(
            opts,
            size,
            |at, out| file.read_at(out, at),
            accepted,
            progress,
        )
        .await
    }

    async fn put_from(
        &self,
        mut params: Value,
        size: u64,
        read_at: impl Fn(u64, &mut [u8]) -> std::io::Result<usize>,
        mut accepted: impl FnMut(&Value),
        mut progress: impl FnMut(u64, u64),
    ) -> LinkResult<Value> {
        let conn = self.conn_ready().await?;
        let _busy = Travelling::new(&self.inner.travelling);
        let l = self.limits();
        let stall = Duration::from_secs(l.stall_s);
        let (mut tx, mut rx) = conn.open_bi().await.map_err(|_| link_closed())?;
        let _ = tx.set_priority(-1);
        params["size"] = json!(size);
        wire::write_json(
            &mut tx,
            K_REQ,
            &json!({"method": "link.put", "params": params}),
        )
        .await?;
        let accept = match wire::read_frame(
            &mut rx,
            l.control_frame_max,
            stall.max(Duration::from_secs(l.call_deadline_s + 5)),
            false,
        )
        .await?
        {
            Some((K_EVENT, b)) => wire::parse::<Value>(&b)?["accept"].take(),
            Some((K_ERROR, b)) => return Err(self.explain(wire::parse::<LinkError>(&b)?)),
            _ => return Err(close_error(&conn).unwrap_or_else(link_closed)),
        };
        accepted(&accept);
        let chunk = accept["chunk"].as_u64().unwrap_or(16 * 1024) as usize;
        let window = accept["window"].as_u64().unwrap_or(1).max(1) as u32;
        // (a Mac that knows nothing of resuming says no offset: the file goes from its start)
        let mut at = accept["offset"].as_u64().unwrap_or(0).min(size);
        let (mut sent, mut acked) = (0u32, 0u32);
        let mut buf = vec![0u8; chunk + 4];
        let ack = async |rx: &mut iroh::endpoint::RecvStream| -> LinkResult<()> {
            match wire::read_frame(
                rx,
                l.control_frame_max,
                stall.max(Duration::from_secs(l.call_deadline_s + 5)),
                false,
            )
            .await?
            {
                Some((K_ACK, _)) => Ok(()),
                Some((K_ERROR, b)) => Err(self.explain(wire::parse::<LinkError>(&b)?)),
                _ => Err(link_closed()),
            }
        };
        progress(at, size);
        while at < size {
            while sent - acked >= window {
                ack(&mut rx).await?;
                acked += 1;
            }
            let want = chunk.min((size - at) as usize);
            let mut got = 0;
            while got < want {
                let n = read_at(at + got as u64, &mut buf[4 + got..4 + want]).map_err(|e| {
                    LinkError::new(
                        errors::NOT_FOUND,
                        format!("The file could not be read on the phone ({e})."),
                    )
                })?;
                if n == 0 {
                    return Err(LinkError::new(
                        errors::NOT_FOUND,
                        "The file changed on the phone while it was being sent.",
                    ));
                }
                got += n;
            }
            buf[..4].copy_from_slice(&sent.to_be_bytes());
            wire::write_frame(&mut tx, K_CHUNK, &buf[..4 + want]).await?;
            sent += 1;
            at += want as u64;
            progress(at, size);
        }
        while acked < sent {
            ack(&mut rx).await?;
            acked += 1;
        }
        match wire::read_frame(
            &mut rx,
            l.control_frame_max,
            stall.max(Duration::from_secs(l.call_deadline_s + 5)),
            false,
        )
        .await?
        {
            Some((K_RESULT, b)) => wire::parse(&b),
            Some((K_ERROR, b)) => Err(self.explain(wire::parse::<LinkError>(&b)?)),
            _ => Err(close_error(&conn).unwrap_or_else(link_closed)),
        }
    }

    /// The live QUIC connection (tests speak raw frames on it).
    #[doc(hidden)]
    pub fn connection(&self) -> Option<Connection> {
        self.inner
            .online
            .lock()
            .unwrap()
            .as_ref()
            .map(|o| o.conn.clone())
    }
    pub fn session_age(&self) -> Option<Duration> {
        self.inner
            .online
            .lock()
            .unwrap()
            .as_ref()
            .map(|o| o.since.elapsed())
    }
}

/// A stream failed: give the close frame a moment to arrive, then prefer the Mac's stated reason.
async fn why_closed(conn: &Connection, fallback: LinkError) -> LinkError {
    let _ = tokio::time::timeout(Duration::from_secs(1), conn.closed()).await;
    close_error(conn).unwrap_or_else(|| {
        if fallback.code == errors::LINK_CLOSED {
            LinkError::new(errors::MAC_UNREACHABLE, "The connection to FERB's Mac was lost. It may be asleep, offline, or FERB is not running.")
                .allow("Wake the Mac and check that FERB is running; the phone keeps trying")
        } else {
            fallback
        }
    })
}

/// The Mac's reason for closing the connection, from the close frame's reason bytes (a stable code).
fn close_error(conn: &Connection) -> Option<LinkError> {
    use iroh::endpoint::ConnectionError;
    let ConnectionError::ApplicationClosed(c) = conn.close_reason()? else {
        return None;
    };
    let code = String::from_utf8_lossy(&c.reason).to_string();
    Some(match code.as_str() {
        errors::NOT_PAIRED => errors::not_paired(),
        errors::DEVICE_REVOKED => errors::revoked(),
        errors::PAIR_BUSY => {
            LinkError::new(errors::PAIR_BUSY, "The Mac is busy with another pairing.").retry(5)
        }
        errors::TOO_MANY_REQUESTS => LinkError::new(
            errors::TOO_MANY_REQUESTS,
            "This phone reconnected too often.",
        )
        .retry(60),
        "" | "bye" | "silent" | "moved" => return None,
        other => LinkError::new(other, format!("The Mac closed the connection ({other}).")),
    })
}

/// See `Phone::explain`. (The words matched are the ones builds before levels used for an unknown method.)
pub fn explain_refusal(e: LinkError, mac_level: u32, app_level: u32) -> LinkError {
    const UNKNOWN: &str = "\" is not something FERB's phone link knows.";
    if e.code == errors::METHOD_NOT_ALLOWED && mac_level < app_level && e.message.ends_with(UNKNOWN)
    {
        return errors::mac_link_older(e.message.trim_end_matches(UNKNOWN).trim_start_matches('"'));
    }
    e
}

/// Counts a transfer or a call for as long as it runs.
struct Travelling(Arc<std::sync::atomic::AtomicU64>);
impl Travelling {
    fn new(n: &Arc<std::sync::atomic::AtomicU64>) -> Self {
        n.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self(n.clone())
    }
}
impl Drop for Travelling {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}
