//! The phone link for the apps: ferb-link's `LinkClient` as an object with pairing, status and close, plugged into
//! the core as its transport (`door::DoorTransport` over the client's JSON door).
//!
//! One pump thread (the door's) reads every event of the client: a call's result goes to the caller waiting on it, a
//! watch's frames to the core, and the link's own events here: `state`, `bulk` and `relay_state` to the attached
//! cores (`Mobile::link_event`) and the app's `LinkListener`; the Mac's pushed `status` to the cores; `persist` (the link's state, with the phone's private
//! link key) to the app to store; `prove` to the app's `DeviceSigner`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

use ferb_link::ffi::LinkClient;
use ferb_mobile::door::{DoorTransport, LinkDoor};
use ferb_mobile::transport::LinkFailure;
use serde_json::{Value, json};

use crate::callbacks::DeviceSigner;
use crate::error::{FerbError, guard, guard_or};
use crate::records::LinkPath;

/// How the app starts the link.
#[derive(Debug, Clone, uniffi::Record)]
pub struct LinkConfig {
    /// What the last `LinkListener.persist` handed over (opaque; holds the phone's private link key), or none on the
    /// first start (a new key is made and handed over at once).
    #[uniffi(default = None)]
    pub state: Option<Vec<u8>>,
    /// The phone's name as the Mac lists it.
    pub name: String,
    /// The app and its version, for the Mac's record.
    pub app: String,
    /// The phone's lock-bound public key (P-256, SubjectPublicKeyInfo DER, base64), when it has a screen lock.
    #[uniffi(default = None)]
    pub unlock_pub: Option<String>,
    /// The phone's device key (P-256, hardware-held, no prompt).
    #[uniffi(default = None)]
    pub device_pub: Option<String>,
    /// The phone's fingerprint-only key (P-256), when a strong biometric is enrolled.
    #[uniffi(default = None)]
    pub strong_pub: Option<String>,
    /// QUIC segmentation offload (on, as the lab ran; iroh #4647).
    #[uniffi(default = true)]
    pub segmentation_offload: bool,
    /// Tests only: relays to use instead of the public ones.
    #[uniffi(default = [])]
    pub dev_shared_relays: Vec<String>,
    /// Tests only: never a direct path.
    #[uniffi(default = false)]
    pub dev_force_relay: bool,
    /// Port mapping at the phone's gateway (on; tests turn it off).
    #[uniffi(default = true)]
    pub dev_portmapper: bool,
}

/// Where the link is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LinkPhase {
    /// Not paired with a Mac (or forgotten).
    Idle,
    /// In the background: closed, nothing dials (Stay connected keeps it active).
    Paused,
    /// Reaching the Mac with a pairing code.
    Pairing,
    /// The Mac is asking the owner: both screens show `pairing_code`.
    PairingWaitForMac,
    Connecting,
    Online,
    /// The last try failed (`problem`); the next is in `retry_in_s`.
    Waiting,
    /// Stopped for good (`problem`): not paired any more, or a pairing code that no longer works.
    Stopped,
    /// A phase this library does not know (a newer link): read `problem`.
    Other,
}

/// A failure the link reported: its stable code (ferb-link `errors.rs`), its words, and what the owner may do.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LinkProblem {
    pub code: String,
    pub detail: String,
    pub allow: Option<String>,
}

/// The phone's relay, as the link uses it (PHN-418, PHN-425).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum RelayStatus {
    /// No own relay set: the public relays.
    Shared { connected: bool },
    /// The owner's relay is set and connected.
    Own,
    /// The owner's relay is set but not connected (yet).
    OwnDown { problem: Option<LinkProblem> },
    /// The owner's relay can't be reached and the public relays are in use until it is back.
    OwnFallback { problem: Option<LinkProblem> },
}

/// Everything the app shows about the link.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LinkStatus {
    pub phase: LinkPhase,
    /// The path when online (Offline otherwise).
    pub path: LinkPath,
    /// The words for the path ("a direct connection", "your own relay", "the shared relay"), when online.
    pub tier_words: Option<String>,
    /// The six digits both screens show while the Mac asks the owner (`123 456`).
    pub pairing_code: Option<String>,
    /// This phone is paired with a Mac.
    pub paired: bool,
    /// Pictures, files and older pages cross the path (false on the public relay, PHN-444).
    pub bulk_allowed: bool,
    /// A call went unanswered and the link is checking the connection.
    pub slow: bool,
    /// The owner's relay, when the Mac set one (the Mac's setting: FERB on the Mac changes it).
    pub own_relay_url: Option<String>,
    /// The Mac's "Always use my relay" (PHN-453): with the owner's relay in use, no direct path is tried.
    pub own_relay_always: bool,
    /// The phone's relay state, once the link has said it.
    pub relay: Option<RelayStatus>,
    pub problem: Option<LinkProblem>,
    pub retry_in_s: Option<u64>,
    /// The Mac's link is older or newer than this app's (a feature may be missing until both are updated).
    pub mac_older: bool,
    pub mac_newer: bool,
}

/// What the link says the app must know: its status changed, or its state (with the phone's private link key) must
/// be stored. Called on the link's pump thread: post to the UI thread or a store queue and return.
#[uniffi::export(foreign)]
pub trait LinkListener: Send + Sync {
    fn status_changed(&self, status: LinkStatus);
    /// The link's state to keep (opaque bytes, JSON text inside): it holds the phone's PRIVATE link key and where the
    /// Mac is. Store it encrypted under a Keystore/Keychain key that never leaves the phone, exclude it from backups
    /// and device transfer, and hand it back as `LinkConfig.state` at the next start. A newer one replaces the older.
    fn persist(&self, state: Vec<u8>);
}

/// ferb-link's client as ferb-mobile's door (the orphan rule wants a type of this crate between them).
struct ClientDoor(Arc<LinkClient>);

impl LinkDoor for ClientDoor {
    fn command(&self, text: &str) -> String {
        catch_unwind(AssertUnwindSafe(|| self.0.command(text))).unwrap_or_else(|_| {
            json!({"error": {"code": ferb_mobile::transport::codes::LINK_INTERNAL_ERROR,
                             "message": "The phone link failed inside."}})
            .to_string()
        })
    }
    fn poll(&self, wait_ms: u64) -> String {
        if self.0.is_closed() {
            // a closed client answers at once: wait here so the pump does not spin before it stops
            std::thread::sleep(std::time::Duration::from_millis(wait_ms.min(500)));
            return String::new();
        }
        catch_unwind(AssertUnwindSafe(|| self.0.poll(wait_ms))).unwrap_or_default()
    }
}

#[derive(Default)]
struct Seen {
    paired: bool,
    /// The last session whose presence the device key set out to prove (or the link proved before it went online).
    presented: Option<u64>,
    /// The last session whose presence proof was answered (taken or not): [`LinkInner::wait_presented`].
    settled: Option<u64>,
    bulk: Option<bool>,
    relay: Option<Value>,
    /// The Mac's link level from its welcome (PHN-313): what it knows of the phone's newer calls.
    mac_level: u32,
}

pub(crate) struct LinkInner {
    client: Arc<LinkClient>,
    door: OnceLock<Arc<DoorTransport>>,
    listener: Arc<dyn LinkListener>,
    device_signer: Option<Arc<dyn DeviceSigner>>,
    cores: Mutex<Vec<Weak<ferb_mobile::Mobile>>>,
    seen: Mutex<Seen>,
    /// Told when a session's presence proof was answered (with `seen`).
    settled: std::sync::Condvar,
    closed: AtomicBool,
    /// The app is unlocked and in front (PHN-648): presence says so, and is sent again every [`PRESENCE_EVERY`].
    open: AtomicBool,
    presence_loop: AtomicBool,
}

/// How often the device key's presence keeps the Mac's window open while the app is unlocked and in front
/// (PHN-648): well inside the Mac's five idle minutes.
const PRESENCE_EVERY: std::time::Duration = std::time::Duration::from_secs(120);

/// How long an unlock statement waits for the new connection's presence proof before it goes anyway (kb PHN-684):
/// the Mac takes a statement only on a connection that proved the device key, and the proof is on its way from the
/// moment the link is online (a challenge, the signature, `link.present`: the device signer has 3 s).
pub(crate) const PRESENCE_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn problem(v: &Value) -> Option<LinkProblem> {
    let code = v["code"].as_str()?;
    Some(LinkProblem {
        code: code.to_string(),
        detail: v["message"].as_str().unwrap_or_default().to_string(),
        allow: v["allow"].as_str().map(str::to_string),
    })
}

fn path_of(tier: &Value) -> LinkPath {
    match tier.as_str() {
        Some("direct") => LinkPath::Direct,
        Some("own_relay") => LinkPath::OwnRelay,
        _ => LinkPath::PublicRelay,
    }
}

fn relay_status(v: &Value) -> Option<RelayStatus> {
    Some(match v["state"].as_str()? {
        "shared" => RelayStatus::Shared {
            connected: v["connected"].as_bool().unwrap_or(false),
        },
        "own" => RelayStatus::Own,
        "own_down" => RelayStatus::OwnDown {
            problem: problem(&v["error"]),
        },
        "own_fallback" => RelayStatus::OwnFallback {
            problem: problem(&v["error"]),
        },
        _ => return None,
    })
}

/// The words the chat shows while the owner's relay is down and the public one is used: the link's own words when it
/// gave a reason, else a plain sentence. None in every other relay state (ferb-mobile keeps `relay_note` as given).
fn relay_note(v: &Value) -> Value {
    if v["state"] != "own_fallback" {
        return Value::Null;
    }
    match v["error"]["message"].as_str() {
        Some(m) if !m.is_empty() => json!(m),
        _ => json!("Your relay can't be reached; the public relay is used until it is back."),
    }
}

fn paired_in(state: &Value) -> bool {
    state["mac"]["paired"].as_bool() == Some(true)
}

impl LinkInner {
    fn door(&self) -> Result<&Arc<DoorTransport>, FerbError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(FerbError::link_closed());
        }
        self.door.get().ok_or_else(FerbError::link_closed)
    }

    pub(crate) fn transport(&self) -> Result<Arc<DoorTransport>, FerbError> {
        self.door().cloned()
    }

    pub(crate) fn status(&self) -> LinkStatus {
        let phase = self.client.phone().phase();
        let limits = self.client.phone().limits();
        let seen = lock(&self.seen);
        let online = phase["phase"] == "online";
        let path = if online {
            path_of(&phase["tier"])
        } else {
            LinkPath::Offline
        };
        LinkStatus {
            phase: match phase["phase"].as_str().unwrap_or_default() {
                "idle" => LinkPhase::Idle,
                "paused" => LinkPhase::Paused,
                "pairing" => LinkPhase::Pairing,
                "pairing_wait_for_mac" => LinkPhase::PairingWaitForMac,
                "connecting" => LinkPhase::Connecting,
                "online" => LinkPhase::Online,
                "waiting" => LinkPhase::Waiting,
                "stopped" => LinkPhase::Stopped,
                _ => LinkPhase::Other,
            },
            path,
            tier_words: online
                .then(|| phase["tier_words"].as_str().map(str::to_string))
                .flatten(),
            pairing_code: phase["code"].as_str().map(str::to_string),
            paired: seen.paired || online,
            bulk_allowed: online && seen.bulk.unwrap_or(path != LinkPath::PublicRelay),
            slow: phase["slow"] == json!(true),
            own_relay_url: phase["own_relay"].as_str().map(str::to_string),
            own_relay_always: limits.own_relay_always,
            relay: seen.relay.as_ref().and_then(relay_status),
            problem: problem(&phase["error"]),
            retry_in_s: phase["retry_in_s"].as_u64(),
            mac_older: phase["mac"]["older"] == json!(true),
            mac_newer: phase["mac"]["newer"] == json!(true),
        }
    }

    /// Hands a link event to every core still attached.
    fn forward(&self, event: &str) {
        let cores: Vec<Arc<ferb_mobile::Mobile>> = {
            let mut cores = lock(&self.cores);
            cores.retain(|w| w.strong_count() > 0);
            cores.iter().filter_map(Weak::upgrade).collect()
        };
        for c in cores {
            let _ = catch_unwind(AssertUnwindSafe(|| c.link_event(event)));
        }
    }

    fn tell(&self) {
        let status = self.status();
        let _ = catch_unwind(AssertUnwindSafe(|| self.listener.status_changed(status)));
    }

    /// One event of the link's own (the pump's thread).
    fn on_event(self: &Arc<Self>, text: &str) {
        let Ok(ev) = serde_json::from_str::<Value>(text) else {
            return;
        };
        match ev["type"].as_str().unwrap_or_default() {
            "state" => {
                if ev["phase"] == "idle" {
                    lock(&self.seen).bulk = None;
                }
                if let Some(l) = ev["mac"]["level"].as_u64() {
                    lock(&self.seen).mac_level = l as u32;
                }
                // the presence proof is marked as on its way before the app hears "online": an unlock statement
                // handed over at once then waits for it (`wait_presented`) instead of being refused as `locked`
                self.present(&ev);
                self.forward(text);
                self.tell();
            }
            "bulk" => {
                lock(&self.seen).bulk = ev["allowed"].as_bool();
                self.forward(text);
                self.tell();
            }
            "relay_state" => {
                lock(&self.seen).relay = Some(ev["relay"].clone());
                self.forward(
                    &json!({"type": "relay_state", "relay": relay_note(&ev["relay"])}).to_string(),
                );
                self.tell();
            }
            // the Mac's pushed `chat.status` (the bridge's, when it changes): what waits, for the notifications
            "status" => self.forward(text),
            // the Mac asks for one precise fix after the owner's Yes for a task (T-1214)
            "location_wanted" => self.forward(text),
            "persist" => {
                let state = ev["state"].clone();
                lock(&self.seen).paired = paired_in(&state);
                let bytes = state.to_string().into_bytes();
                let _ = catch_unwind(AssertUnwindSafe(|| self.listener.persist(bytes)));
                self.tell();
            }
            "prove" => {
                let (id, payload) = (
                    ev["id"].as_u64().unwrap_or(0),
                    ev["payload"].as_str().unwrap_or_default().to_string(),
                );
                let me = self.clone();
                // never on the pump: the signature may take a moment, and the pump carries every call's result
                let _ = std::thread::Builder::new()
                    .name("ferb-link-prove".into())
                    .spawn(move || {
                        let sig = me.device_signer.as_ref().and_then(|s| {
                            catch_unwind(AssertUnwindSafe(|| s.sign(payload)))
                                .ok()
                                .flatten()
                        });
                        let _ = me
                            .client
                            .command(&json!({"cmd": "proof", "id": id, "sig": sig}).to_string());
                    });
            }
            _ => {}
        }
    }

    /// A new session. The link proves this phone's presence before a session goes online (it asks the app with the
    /// `prove` event, kb PHN-799), so the Mac lets the phone's unlock carry over a reconnect before any call: then
    /// nothing is on its way, and only an app that is open says so at once (PHN-648). A session that went online
    /// without that proof (`proved` false: the signature didn't come in time, an older link) is proved here: the
    /// device key signs the Mac's fresh challenge for `link.present`, on a thread of its own, once per session.
    fn present(self: &Arc<Self>, ev: &Value) {
        let (Some(signer), Some(session)) = (self.device_signer.clone(), ev["session"].as_u64())
        else {
            return;
        };
        if ev["phase"] != "online" {
            return;
        }
        let proved = ev["proved"] == json!(true);
        {
            let mut seen = lock(&self.seen);
            if seen.presented == Some(session) {
                return;
            }
            seen.presented = Some(session);
            if proved {
                seen.settled = Some(session);
                self.settled.notify_all();
            }
        }
        if !proved {
            self.send_presence(signer, Some(session));
        } else if self.open.load(Ordering::SeqCst) {
            self.send_presence(signer, None);
        }
    }

    /// Waits, at most [`PRESENCE_WAIT`], for the presence proof of the newest session to be answered, when one is on
    /// its way (kb PHN-684). Nothing on its way (no device signer, a session the link proved itself): no wait.
    pub(crate) fn wait_presented(&self) {
        let seen = lock(&self.seen);
        let _ = self
            .settled
            .wait_timeout_while(seen, PRESENCE_WAIT, |s| {
                s.presented.is_some()
                    && s.settled != s.presented
                    && !self.closed.load(Ordering::SeqCst)
            })
            .map_err(|p| p.into_inner());
    }

    /// The device key's proof for `link.present`, with `{"active": true}` while the app says it is unlocked and in
    /// front ([`Link::set_open`], PHN-648). On a thread of its own.
    fn send_presence(self: &Arc<Self>, signer: Arc<dyn DeviceSigner>, session: Option<u64>) {
        // an older Mac's daemon takes only `{}`: the flag goes to a Mac at level 3 or above (PHN-648)
        let open = self.open.load(Ordering::SeqCst) && lock(&self.seen).mac_level >= 3;
        let me = self.clone();
        // whatever the proof's outcome, a statement waiting for this session's proof goes on (kb PHN-684)
        let settle = move || {
            if let Some(session) = session {
                lock(&me.seen).settled = Some(session);
                me.settled.notify_all();
            }
        };
        let Ok(door) = self.transport() else {
            return settle();
        };
        let spawned = std::thread::Builder::new()
            .name("ferb-link-present".into())
            .spawn(move || {
                struct Settle<F: Fn()>(F);
                impl<F: Fn()> Drop for Settle<F> {
                    fn drop(&mut self) {
                        (self.0)()
                    }
                }
                let _settle = Settle(settle);
                let _ = catch_unwind(AssertUnwindSafe(|| {
                    use ferb_mobile::Transport as _;
                    let Ok(reply) = door.call("link.challenge", "{}", None) else {
                        return;
                    };
                    let Some(challenge) = serde_json::from_str::<Value>(&reply)
                        .ok()
                        .and_then(|v| v["challenge"].as_str().map(str::to_string))
                    else {
                        return;
                    };
                    // the app is unlocked and in front: this presence also keeps the Mac's window open (PHN-648)
                    let params = if open {
                        json!({"active": true})
                    } else {
                        json!({})
                    };
                    let payload =
                        ferb_link::client::Phone::call_payload(&challenge, "link.present", &params);
                    let Some(sig) = signer.sign(payload.clone()) else {
                        return;
                    };
                    let proof = json!({"payload": payload, "sig": sig}).to_string();
                    let _ = door.call("link.present", "{}", Some(&proof));
                }));
            });
        if spawned.is_err() {
            // no thread: nothing is on its way, so nothing waits for it
            if let Some(session) = session {
                lock(&self.seen).settled = Some(session);
                self.settled.notify_all();
            }
        }
    }

    /// A core takes the link's events from now on; it hears the link's current state at once.
    pub(crate) fn attach(&self, core: &Arc<ferb_mobile::Mobile>) {
        {
            let mut cores = lock(&self.cores);
            cores.retain(|w| w.strong_count() > 0 && !std::ptr::eq(w.as_ptr(), Arc::as_ptr(core)));
            cores.push(Arc::downgrade(core));
        }
        let mut now = self.client.phone().phase();
        now["type"] = json!("state");
        let _ = catch_unwind(AssertUnwindSafe(|| core.link_event(&now.to_string())));
        let seen = lock(&self.seen);
        if let Some(allowed) = seen.bulk.filter(|_| now["phase"] == "online") {
            let ev = json!({"type": "bulk", "allowed": allowed, "tier": now["tier"]}).to_string();
            drop(seen);
            let _ = catch_unwind(AssertUnwindSafe(|| core.link_event(&ev)));
        } else if let Some(relay) = seen.relay.clone() {
            drop(seen);
            let ev = json!({"type": "relay_state", "relay": relay_note(&relay)}).to_string();
            let _ = catch_unwind(AssertUnwindSafe(|| core.link_event(&ev)));
        }
    }

    pub(crate) fn detach(&self, core: &Arc<ferb_mobile::Mobile>) {
        lock(&self.cores)
            .retain(|w| w.strong_count() > 0 && !std::ptr::eq(w.as_ptr(), Arc::as_ptr(core)));
    }

    /// A command whose reply is `{"ok":true,..}` or `{"error":{..}}`.
    fn command(&self, cmd: Value) -> Result<Value, FerbError> {
        self.door()?;
        let reply: Value = serde_json::from_str(&self.client.command(&cmd.to_string()))
            .unwrap_or_else(|_| json!({"error": {"code": ferb_mobile::transport::codes::LINK_INTERNAL_ERROR, "message": "The phone link's reply could not be read."}}));
        match reply.get("error") {
            Some(e) => Err(FerbError::from_link(LinkFailure::from_json(e))),
            None => Ok(reply),
        }
    }

    fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        // the pump first (so it stops polling), then the client (the goodbye, bounded by ferb-link's 5 s)
        if let Some(d) = self.door.get() {
            d.close();
        }
        self.client.close();
        let cores: Vec<_> = std::mem::take(&mut *lock(&self.cores));
        let ev = json!({"type": "state", "phase": "closed"}).to_string();
        for c in cores.iter().filter_map(Weak::upgrade) {
            let _ = catch_unwind(AssertUnwindSafe(|| c.link_event(&ev)));
        }
    }
}

impl Drop for LinkInner {
    fn drop(&mut self) {
        self.close();
    }
}

/// The phone link: pairing, its status, and the way to the Mac for `Mobile.connect`. End it with `shutdown`.
#[derive(uniffi::Object)]
pub struct Link {
    pub(crate) inner: Arc<LinkInner>,
}

#[uniffi::export]
impl Link {
    /// Starts the link from the app's config. Blocking, at most 20 s (ferb-link `START_DEADLINE`): not on the UI
    /// thread. A first start (no `state`) makes the phone's link key and hands it to `listener.persist` at once.
    #[uniffi::constructor]
    pub fn start(
        config: LinkConfig,
        listener: Arc<dyn LinkListener>,
        device_signer: Option<Arc<dyn DeviceSigner>>,
    ) -> Result<Arc<Link>, FerbError> {
        guard(|| {
            let state = match &config.state {
                None => Value::Null,
                Some(bytes) => serde_json::from_slice::<Value>(bytes)
                    .ok()
                    .filter(|v| v["key"].is_string())
                    .ok_or_else(|| {
                        FerbError::bad_argument(
                            "The link's saved state can't be read; it is not what the link handed over.",
                        )
                    })?,
            };
            let paired = paired_in(&state);
            let cfg = json!({
                "state": state, "name": config.name, "app": config.app,
                "unlock_pub": config.unlock_pub, "device_pub": config.device_pub, "strong_pub": config.strong_pub,
                "segmentation_offload": config.segmentation_offload,
                // (a test's link: its own relays and no port mapping; it also stays on 127.0.0.1, so nothing it sends
                // leaves the Mac under the test guard's sandbox: ferb-link `NetConfig::loopback_only`)
                "dev": {"shared_relays": config.dev_shared_relays, "force_relay": config.dev_force_relay,
                        "portmapper": config.dev_portmapper,
                        "loopback": !config.dev_portmapper && !config.dev_shared_relays.is_empty()},
            });
            let client = Arc::new(LinkClient::start(&cfg.to_string()).map_err(|e| {
                FerbError::from_link(LinkFailure {
                    code: e.code,
                    message: e.message,
                    allow: e.allow,
                    retry_after_s: e.retry_after_s,
                })
            })?);
            let inner = Arc::new(LinkInner {
                client: client.clone(),
                door: OnceLock::new(),
                listener,
                device_signer,
                cores: Mutex::new(Vec::new()),
                seen: Mutex::new(Seen {
                    paired,
                    ..Seen::default()
                }),
                settled: std::sync::Condvar::new(),
                closed: AtomicBool::new(false),
                open: AtomicBool::new(false),
                presence_loop: AtomicBool::new(false),
            });
            let weak: Weak<LinkInner> = Arc::downgrade(&inner);
            let door = DoorTransport::start(
                Arc::new(ClientDoor(client)),
                Arc::new(move |text: String| {
                    if let Some(inner) = weak.upgrade() {
                        let _ = catch_unwind(AssertUnwindSafe(|| inner.on_event(&text)));
                    }
                }),
            );
            let _ = inner.door.set(Arc::new(door));
            Ok(Arc::new(Link { inner }))
        })
    }

    /// This phone's link id (its endpoint id).
    pub fn endpoint_id(&self) -> String {
        guard_or(String::new(), || self.inner.client.id())
    }

    /// Pairs from a scanned QR code's text or a typed `FERB-` code. Returns at once; the status then shows the
    /// pairing, the six digits to compare (`pairing_code`), and online once the owner said Yes on the Mac. A phone
    /// already paired is refused (`refused`, link code `already_paired`): forget first.
    pub fn pair(&self, code: String) -> Result<(), FerbError> {
        guard(|| {
            self.inner
                .command(json!({"cmd": "pair", "qr": code}))
                .map(|_| ())
        })
    }

    /// The link's status now.
    pub fn status(&self) -> LinkStatus {
        let inner = self.inner.clone();
        catch_unwind(AssertUnwindSafe(move || inner.status())).unwrap_or(LinkStatus {
            phase: LinkPhase::Other,
            path: LinkPath::Offline,
            tier_words: None,
            pairing_code: None,
            paired: false,
            bulk_allowed: false,
            slow: false,
            own_relay_url: None,
            own_relay_always: false,
            relay: None,
            problem: None,
            retry_in_s: None,
            mac_older: false,
            mac_newer: false,
        })
    }

    /// The app is in front (true) or in the background (false: the link closes and nothing dials, unless Stay
    /// connected keeps it active).
    /// The app is unlocked and in front (true), or locked or left (false), kb PHN-648. While true the device key's
    /// presence (no prompt) says so, now and every two minutes, and the Mac keeps this phone's unlock window open
    /// (never past its 12 h limit, never after `phone.lock`). Non-blocking.
    pub fn set_open(&self, on: bool) {
        let inner = &self.inner;
        inner.open.store(on, Ordering::SeqCst);
        let Some(signer) = inner.device_signer.clone() else {
            return;
        };
        if !on || inner.presence_loop.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = inner.clone();
        let _ = std::thread::Builder::new()
            .name("ferb-link-open".into())
            .spawn(move || {
                // at once (the app just opened or came back), then every PRESENCE_EVERY while it stays open
                while me.open.load(Ordering::SeqCst) && !me.closed.load(Ordering::SeqCst) {
                    if lock(&me.seen).presented.is_some() {
                        me.send_presence(signer.clone(), None);
                    }
                    let mut waited = std::time::Duration::ZERO;
                    while waited < PRESENCE_EVERY
                        && me.open.load(Ordering::SeqCst)
                        && !me.closed.load(Ordering::SeqCst)
                    {
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        waited += std::time::Duration::from_millis(500);
                    }
                }
                me.presence_loop.store(false, Ordering::SeqCst);
            });
    }

    pub fn set_active(&self, on: bool) -> Result<(), FerbError> {
        guard(|| {
            self.inner
                .command(json!({"cmd": "active", "on": on}))
                .map(|_| ())
        })
    }

    /// The phone's network changed (`wifi`, `cellular`, … when known): one dial at once if the link is down.
    pub fn network_changed(&self, kind: Option<String>) -> Result<(), FerbError> {
        guard(|| {
            self.inner
                .command(json!({"cmd": "network_changed", "id": "network", "kind": kind}))
                .map(|_| ())
        })
    }

    /// Dials now instead of waiting for the next try.
    pub fn retry(&self) -> Result<(), FerbError> {
        guard(|| self.inner.command(json!({"cmd": "retry"})).map(|_| ()))
    }

    /// Forgets the Mac on this phone (the Mac's list keeps the phone until the owner removes it there). The new
    /// state, without the Mac, reaches `persist`; the phone's link key stays. With "Forget this Mac" the app also
    /// calls `Mobile.forget_this_mac`.
    pub fn forget(&self) -> Result<(), FerbError> {
        guard(|| self.inner.command(json!({"cmd": "forget"})).map(|_| ()))
    }

    /// Closes the link: calls under way fail, the connection says goodbye, the endpoint closes. At most about 5 s
    /// (not on the UI thread); a second call does nothing. Every later call answers `closed`. (`shutdown`, not
    /// `close`: the bindings' own `close()` frees the handle, which also shuts the link down.)
    pub fn shutdown(&self) {
        guard_or((), || self.inner.close())
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }
}
