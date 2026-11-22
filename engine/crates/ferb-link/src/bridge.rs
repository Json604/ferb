//! The Mac side of the phone link. It accepts phones over iroh, checks a new phone's proof of the QR's one-time secret
//! and has the daemon ask the owner (`link.pair_request`, `link.pair_wait`: the Yes is FERB.app's), follows the
//! daemon's paired phones (`link.devices`, `link.events`: a removed phone's connections close at once), decides the
//! tier of every connection from the live path, enforces the link's limits, forwards the phone's allowed calls to
//! FERB's daemon as role `phone`, and answers what Settings → Phone asks through the daemon (`ask` on `link.events`,
//! `link.answer`: the pairing code, the relay, the status). Since S4 (kb T-1204) it keeps no phones of its own and
//! never calls FERB as the owner.
//!
//! Everything the phone is refused is refused here or in the daemon, never only on the phone. What rests on the
//! phone's unlock (the owner's data, a sensitive call, a hard Yes, a file) is decided by the daemon alone: the
//! bridge passes the phone's proof through and never verifies, opens or writes anything for a phone itself.

use crate::{
    daemon::{Daemon, wipe},
    errors::{self, LinkError, LinkResult, close_code},
    limits::Limits,
    net::{self, NetConfig, RelayPolicy, RelaySpec, RelayState, Shared, Tier},
    pairing::{Checked, Invite, Window},
    relaytest, role,
    secrets::Keys,
    store::{self, Store},
    wire::{self, K_ACK, K_CHUNK, K_ERROR, K_EVENT, K_HELLO, K_REQ, K_RESULT, K_WELCOME},
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD as B64S, URL_SAFE_NO_PAD as B64},
};
use iroh::{
    Endpoint,
    endpoint::{Connection, RecvStream, SendStream},
    tls::CaTlsConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{Semaphore, broadcast, mpsc};

pub use crate::{MIN_VERSION, VERSION};

/// PHN-590 (owner 2026-11-17, "ok do it"): the shared relays are kept open for paired phones that haven't connected since
/// the relay setting changed at most this many days after the change (the number awaits the owner's confirmation).
/// A phone that comes back later can't find the Mac until the owner turns his relay off for a moment or pairs it again.
pub const BEACON_MAX_DAYS: u64 = 30;

/// PHN-220, PHN-591: the second endpoint is open while a paired phone hasn't been seen on the own relay, and never past
/// `closes_at` (unix seconds).
pub fn beacon_open(phones_waiting: usize, closes_at: Option<u64>, now: u64) -> bool {
    phones_waiting > 0 && closes_at.is_none_or(|at| now < at)
}

pub struct BridgeConfig {
    pub state_dir: PathBuf,
    pub daemon_socket: PathBuf,
    pub shared: Shared,
    pub roots: Option<CaTlsConfig>,
    pub force_relay: bool,
    /// Where the link key, the relay token and the state's seal key are kept.
    pub keys: Keys,
    /// The owner says a state file changed outside the link is his (`--accept-state`).
    pub accept_state: bool,
    /// Port mapping at the Mac's gateway (`net::NetConfig::portmapper`): on in the product; tests turn it off.
    pub portmapper: bool,
    /// Tests only: IP on 127.0.0.1 alone (`net::NetConfig::loopback_only`).
    pub loopback_only: bool,
}

#[derive(Debug, Deserialize)]
struct Hello {
    v: u32,
    #[serde(default)]
    min_v: u32,
    #[serde(default)]
    name: String,
    #[serde(default)]
    app: String,
    #[serde(default)]
    pair: Option<String>,
    /// The app's feature level and build (`crate::LEVEL`, `crate::build`); absent from apps before levels.
    #[serde(default)]
    level: u32,
    #[serde(default)]
    build: String,
    #[serde(default)]
    unlock_pub: Option<String>,
    #[serde(default)]
    device_pub: Option<String>,
    #[serde(default)]
    strong_pub: Option<String>,
}

/// A paired phone as the daemon lists it (`link.devices`): the daemon's own table is the authority (kb PHN-469).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Device {
    /// The phone's endpoint id (its public key).
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub paired_at: String,
    #[serde(default)]
    pub last_seen: String,
    /// It registered a lock-bound key (its signature is the proof of an unlock).
    #[serde(default)]
    pub unlock: bool,
    #[serde(default)]
    pub device_key: bool,
    #[serde(default)]
    pub fingerprint_key: bool,
}

/// A phone that proved the QR's secret, waiting for the owner's answer in FERB.app (the daemon's pairing).
struct Pending {
    phone: String,
    name: String,
    code: String,
    request_id: String,
}
#[derive(Default)]
struct PairState {
    window: Option<Window>,
    pending: Option<Pending>,
}

/// The public relay's slow-down notice (kb PHN-307). No bulk crosses the public relays any more (kb PHN-444), so
/// what is left of the lab's budget is the notice itself and how long ago it came.
struct SharedBudget {
    pause_until: Option<Instant>,
    seen_ratelimited: u64,
}

struct Live {
    /// What the phone last said of its own network (kinds and counts, never addresses).
    phone_net: Option<net::NetFacts>,
    /// What the app said of itself in its hello.
    level: u32,
    build: String,
    conn: Connection,
    events: mpsc::Sender<Value>,
    tier: Tier,
    why: Option<String>,
    /// What the phone said of its last start-over (kinds, counts, times; kb NET-25).
    handover: Option<Value>,
}

struct Inner {
    store: Store,
    daemon: Daemon,
    /// The daemon's paired phones, as `link.devices` last said (kept current by `link.events`).
    devices: Mutex<Vec<Device>>,
    /// The most phones the daemon keeps (kb PHN-419).
    devices_max: std::sync::atomic::AtomicU32,
    policy: Arc<RelayPolicy>,
    pairing: Mutex<PairState>,
    live: Mutex<HashMap<String, Vec<Arc<Mutex<Live>>>>>,
    unpaired: Arc<Semaphore>,
    /// Phones whose own "Forget this Mac" is under way (PHN-646): their removal cuts them only after the reply left.
    forgetting: Mutex<std::collections::HashSet<String>>,
    budget: Mutex<SharedBudget>,
    connects: Mutex<HashMap<String, VecDeque<Instant>>>,
    admin: broadcast::Sender<Value>,
    closed: std::sync::atomic::AtomicBool,
    /// Files on their way right now, either direction (shown on the Mac; gone when they end).
    transfers: Mutex<HashMap<u64, Value>>,
    next_transfer: std::sync::atomic::AtomicU64,
    /// Per phone: whether a direct connection is being tried for, and how long the upgrade took.
    direct_times: Mutex<HashMap<String, Value>>,
    /// Per phone: its start-overs since it last changed network (how many, how many got through, the last one).
    handovers: Mutex<HashMap<String, Value>>,
}

#[derive(Clone)]
pub struct Bridge {
    inner: Arc<Inner>,
}

fn b64_any(s: &str) -> Option<Vec<u8>> {
    B64.decode(s.trim_end_matches('='))
        .ok()
        .or_else(|| B64S.decode(s).ok())
}

impl Bridge {
    pub async fn start(cfg: BridgeConfig) -> LinkResult<Self> {
        let store = Store::open(&cfg.state_dir, cfg.keys.clone(), cfg.accept_state)?;
        let limits = store.limits();
        let net = NetConfig {
            roots: cfg.roots.clone(),
            force_relay: cfg.force_relay,
            accept: true,
            streams_max: limits.streams_max,
            relay_only: limits.own_relay_always,
            portmapper: cfg.portmapper,
            loopback_only: cfg.loopback_only,
            ..NetConfig::new(store.identity()?, store.own_relay(), cfg.shared.clone())
        };
        let policy = RelayPolicy::start(net).await?;
        let inner = Arc::new(Inner {
            daemon: Daemon {
                socket: cfg.daemon_socket,
            },
            devices: Default::default(),
            devices_max: std::sync::atomic::AtomicU32::new(limits.devices_max),
            policy: policy.clone(),
            pairing: Default::default(),
            live: Default::default(),
            unpaired: Arc::new(Semaphore::new(limits.unpaired_slots as usize)),
            forgetting: Mutex::new(std::collections::HashSet::new()),
            budget: Mutex::new(SharedBudget {
                pause_until: None,
                seen_ratelimited: 0,
            }),
            connects: Default::default(),
            admin: broadcast::channel(256).0,
            store,
            closed: Default::default(),
            transfers: Default::default(),
            next_transfer: Default::default(),
            direct_times: Default::default(),
            handovers: Default::default(),
        });
        let me = Self { inner };
        // the daemon's paired phones, before any phone is let in: a paired phone must never be told it is not paired
        // because the list wasn't read yet
        if let Err(e) = me.refresh_devices().await {
            me.close().await;
            return Err(e);
        }
        tokio::spawn(me.clone().events_loop());
        {
            let (i, j) = (me.inner.clone(), me.inner.clone());
            tokio::spawn(policy.run(
                move || {
                    let l = i.store.limits();
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
        crate::diag::trace_if_asked();
        if crate::diag::enabled() {
            // (asked for with FERB_LINK_DIAG: the Mac's own minute lines, the same as the phone's)
            let me = me.clone();
            tokio::spawn(async move {
                let mut before = crate::diag::counters(&me.ep());
                while !me.is_closed() {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    let ep = me.ep();
                    let now = crate::diag::counters(&ep);
                    let paths: Vec<String> = me
                        .inner
                        .live
                        .lock()
                        .unwrap()
                        .values()
                        .flatten()
                        .map(|l| crate::diag::paths(&l.lock().unwrap().conn))
                        .collect();
                    crate::diag::log(&format!(
                        "last minute: {} · phones connected {} · their paths: {} · this side offers: {}",
                        now.since(&before).words(),
                        paths.len(),
                        if paths.is_empty() {
                            "none".to_string()
                        } else {
                            paths.join(" | ")
                        },
                        net::facts(&ep).offers.join(" ")
                    ));
                    before = now;
                }
            });
        }
        tokio::spawn(me.clone().accept_loop());
        tokio::spawn(me.clone().beacon_loop());
        tokio::spawn(me.clone().beacon_accept_loop());
        tokio::spawn(me.clone().watch_loop());
        Ok(me)
    }

    pub fn id(&self) -> String {
        self.ep().id().to_string()
    }
    /// The current endpoint (replaced when the relay set changes).
    pub fn endpoint(&self) -> Endpoint {
        self.ep()
    }
    fn ep(&self) -> Endpoint {
        self.inner.policy.endpoint()
    }
    pub fn admin_events(&self) -> broadcast::Receiver<Value> {
        self.inner.admin.subscribe()
    }
    fn tell_admin(&self, v: Value) {
        // pairing attempts, wrong proofs, removals and relay changes are part of the audit too
        if let Some(e) = v["event"].as_str().filter(|e| {
            matches!(
                *e,
                "pair_request"
                    | "pair_ended"
                    | "pair_wrong_secret"
                    | "device_revoked"
                    | "device_expired"
                    | "relay"
                    | "relay_rate_limited"
                    | "heavy_fetch"
            )
        }) {
            let mut line = json!({"t": store::now(), "method": format!("mac.{e}"), "class": "mac", "result": "ok"});
            for k in [
                "phone",
                "id",
                "name",
                "paired",
                "window_closed",
                "files",
                "bytes",
            ] {
                if !v[k].is_null() {
                    line[if k == "phone" || k == "id" {
                        "device"
                    } else {
                        k
                    }] = v[k].clone();
                }
            }
            if e == "relay" {
                line["state"] = v["relay"]["state"]["state"].clone();
            }
            self.inner.store.audit(&line);
        }
        let _ = self.inner.admin.send(v);
    }
    fn limits(&self) -> Limits {
        self.inner.store.limits()
    }
    /// Reads the daemon's paired phones again (`link.devices`).
    pub async fn refresh_devices(&self) -> LinkResult<()> {
        let v = self
            .inner
            .daemon
            .link_call("link.devices", json!({}), Duration::from_secs(5))
            .await?;
        let list: Vec<Device> = serde_json::from_value(v["devices"].clone()).map_err(|e| {
            LinkError::new(
                errors::FERB_ERROR,
                format!("FERB's list of phones could not be read ({e})"),
            )
        })?;
        if let Some(max) = v["max"].as_u64() {
            self.inner
                .devices_max
                .store(max as u32, std::sync::atomic::Ordering::Relaxed);
        }
        let ids: Vec<String> = list.iter().map(|d| d.id.clone()).collect();
        self.inner.store.keep_relay_marks(&ids);
        *self.inner.devices.lock().unwrap() = list;
        // a phone the daemon no longer lists is cut off at once, whatever the event that said so
        let gone: Vec<String> = self
            .inner
            .live
            .lock()
            .unwrap()
            .keys()
            .filter(|id| !ids.contains(id))
            .cloned()
            .collect();
        for id in gone {
            self.cut(&id);
        }
        Ok(())
    }
    fn device(&self, id: &str) -> Option<Device> {
        self.inner
            .devices
            .lock()
            .unwrap()
            .iter()
            .find(|d| d.id == id)
            .cloned()
    }
    /// The daemon's changes to the paired phones and its asks (`link.events`), for as long as the link runs; a lost
    /// connection is made again and the list read again (a daemon that went away takes this program with it: FERB
    /// starts it with its standard input open and it ends when that closes).
    async fn events_loop(self) {
        while !self.is_closed() {
            let me = self.clone();
            let ended = match self.inner.daemon.link().await {
                Ok(mut conn) => {
                    let _ = self.refresh_devices().await;
                    conn.stream("link.events", json!({}), None, |ev| {
                        let me = me.clone();
                        async move {
                            me.on_daemon_event(ev).await;
                            !me.is_closed()
                        }
                    })
                    .await
                }
                Err(e) => Err(e),
            };
            if self.is_closed() {
                return;
            }
            if let Err(e) = ended {
                crate::diag::log(&format!("FERB's events: {}", e.message));
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    async fn on_daemon_event(&self, ev: Value) {
        match ev["event"].as_str() {
            Some("ask") => {
                let me = self.clone();
                tokio::spawn(async move { me.answer(ev).await });
            }
            Some("heavy_fetch") => {
                // the daemon noticed a phone taking unusually much (S3b, kb PHN-485): the phone's own chat says it once
                let id = ev["id"].as_str().unwrap_or_default().to_string();
                self.tell_admin(json!({"event": "heavy_fetch", "id": id, "name": ev["name"], "files": ev["files"], "bytes": ev["bytes"], "error": ev["error"]}));
                let senders: Vec<mpsc::Sender<Value>> = self
                    .inner
                    .live
                    .lock()
                    .unwrap()
                    .get(&id)
                    .into_iter()
                    .flatten()
                    .map(|l| l.lock().unwrap().events.clone())
                    .collect();
                for s in senders {
                    let _ = s.try_send(json!({"type": "notice", "error": ev["error"]}));
                }
            }
            Some("location_wanted") => {
                // FERB asks this phone for one precise fix after the owner's Yes for a task (T-1214): a few bytes of
                // text, carried on any path (the public relay too); the phone answers with phone.device_report
                let id = ev["id"].as_str().unwrap_or_default().to_string();
                let senders: Vec<mpsc::Sender<Value>> = self
                    .inner
                    .live
                    .lock()
                    .unwrap()
                    .get(&id)
                    .into_iter()
                    .flatten()
                    .map(|l| l.lock().unwrap().events.clone())
                    .collect();
                for s in senders {
                    let _ = s.try_send(
                        json!({"type": "location_wanted", "request_id": ev["request_id"]}),
                    );
                }
            }
            Some("device_removed") => {
                if let Some(id) = ev["id"].as_str() {
                    if self.inner.forgetting.lock().unwrap().remove(id) {
                        // the phone's own forget: its reply goes first, then the connection closes
                        let (b, id) = (self.clone(), id.to_owned());
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            b.cut(&id);
                        });
                    } else {
                        self.cut(id);
                    }
                    self.tell_admin(json!({"event": "device_revoked", "id": id}));
                }
                let _ = self.refresh_devices().await;
            }
            Some(_) => {
                let _ = self.refresh_devices().await;
            }
            None => {}
        }
    }
    /// One of the daemon's asks (Settings → Phone through FERB's daemon), answered with `link.answer`.
    async fn answer(&self, ev: Value) {
        let id = ev["ask_id"].clone();
        let p = ev.get("params").cloned().unwrap_or(json!({}));
        let out = self
            .asked(ev["what"].as_str().unwrap_or_default(), &p)
            .await;
        let reply = match out {
            Ok(v) => json!({"ask_id": id, "result": v}),
            Err(e) => json!({"ask_id": id, "error": e}),
        };
        if let Err(e) = self
            .inner
            .daemon
            .link_call("link.answer", reply, Duration::from_secs(5))
            .await
        {
            crate::diag::log(&format!("answering FERB: {}", e.message));
        }
    }
    /// What the daemon may ask (only it can: this program has no socket of its own for anyone to ask on).
    pub async fn asked(&self, what: &str, p: &Value) -> LinkResult<Value> {
        Ok(match what {
            "status" => self.status(),
            "pair_start" => self.pair_start()?,
            "pair_cancel" => {
                self.pair_cancel();
                json!({"ok": true})
            }
            "relay_test" => serde_json::to_value(self.relay_test(&relay_spec(p)?).await?).unwrap(),
            "relay_set" => {
                // the token the setting now holds goes back to the daemon alone, which keeps it in the link's one
                // Keychain item (kb PHN-143); the daemon takes it out of what FERB.app is told
                let (spec, mut out) = if p.get("off").and_then(Value::as_bool) == Some(true) {
                    (None, self.relay_set(None).await?)
                } else {
                    let spec = relay_spec(p)?;
                    (Some(spec.clone()), self.relay_set(Some(spec)).await?)
                };
                let token = spec
                    .and_then(|s| s.token)
                    .map(|t| B64S.encode(t.as_bytes()));
                out["keys"] = json!({crate::secrets::RELAY_TOKEN: token});
                out
            }
            "always" => {
                let on = p.get("on").and_then(Value::as_bool) == Some(true);
                self.limits_set(&json!({"own_relay_always": on}))?;
                json!({"own_relay_always": on})
            }
            other => {
                return Err(LinkError::new(
                    errors::BAD_REQUEST,
                    format!("the link does not know \"{other}\""),
                ));
            }
        })
    }
    pub async fn close(&self) {
        self.inner
            .closed
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = self.inner.policy.set_beacon(false).await;
        self.ep().close().await;
    }
    fn is_closed(&self) -> bool {
        self.inner.closed.load(std::sync::atomic::Ordering::Relaxed)
    }

    // ---- the owner's side (admin socket / harness) -----------------------------------------------------

    pub fn status(&self) -> Value {
        let i = &self.inner;
        let l = self.limits();
        let live = i.live.lock().unwrap();
        let devices: Vec<Value> = self
            .devices()
            .into_iter()
            .map(|d| {
                let tiers: Vec<Tier> = live.get(&d.id).map(|v| v.iter().map(|x| x.lock().unwrap().tier).collect()).unwrap_or_default();
                // (a phone that starts over holds two connections for a moment: the newest one speaks for it)
                let first = live.get(&d.id).and_then(|v| v.last().cloned());
                let facts = first.as_ref().map(|x| net::path_facts(&x.lock().unwrap().conn));
                let (path, why) = first.as_ref().map(|x| { let x = x.lock().unwrap(); (Some(net::path_words(&x.conn)), x.why.clone()) }).unwrap_or((None, None));
                let app = first.as_ref().map(|x| { let x = x.lock().unwrap(); json!({"level": x.level, "build": x.build, "older": x.level < crate::LEVEL, "newer": x.level > crate::LEVEL}) });
                let net = first.as_ref().map(|x| x.lock().unwrap().phone_net.clone());
                let paths = first.as_ref().map(|x| crate::diag::paths(&x.lock().unwrap().conn));
                let handover = i.handovers.lock().unwrap().get(&d.id).cloned();
                let direct = i.direct_times.lock().unwrap().get(&d.id).filter(|_| !tiers.is_empty()).map(|e| {
                    json!({"trying": e["trying"], "for_s": e["off_direct"].as_u64().map(|t| store::now().saturating_sub(t)), "time_to_direct_ms": e["time_to_direct_ms"], "last_time_to_direct_ms": e["last_time_to_direct_ms"]})
                });
                json!({"id": d.id, "name": d.name, "app": d.app, "paired_at": d.paired_at, "last_seen": d.last_seen, "direct": direct,
                       "unlock": d.unlock, "fingerprint_key": d.fingerprint_key, "device_key": d.device_key, "online": !tiers.is_empty(), "tier": tiers.last(), "path": path, "path_facts": facts, "why": why, "app_build": app, "net": net,
                       "paths": paths, "handover": handover})
            })
            .collect();
        let pairing = {
            let p = i.pairing.lock().unwrap();
            match (&p.pending, &p.window) {
                (Some(pend), _) => {
                    json!({"state": "waiting_for_your_yes", "phone": pend.phone, "name": pend.name, "code": pend.code, "request_id": pend.request_id})
                }
                (None, Some(w)) if !w.expired() => {
                    json!({"state": "open", "seconds_left": w.seconds_left()})
                }
                _ => json!({"state": "closed"}),
            }
        };
        let paused = i
            .budget
            .lock()
            .unwrap()
            .pause_until
            .map(|t| t.saturating_duration_since(Instant::now()).as_secs())
            .filter(|s| *s > 0);
        json!({
            "id": self.id(),
            "version": VERSION, "level": crate::LEVEL, "build": crate::build(),
            "net": net::facts(&self.ep()),
            "relay": i.policy.state(),
            "force_relay": i.policy.force_relay(),
            // (the token never: `has_token` says whether one is kept)
            "own_relay": i.policy.own().map(|o| json!({"url": o.url, "token_set": o.token.is_some(), "has_token": o.token.is_some()})),
            "own_relay_always": l.own_relay_always,
            "home_relays": net::home_relays(&self.ep()).iter().map(|u| u.to_string()).collect::<Vec<_>>(),
            // with an own relay set: the phones not yet seen with this relay setting, for which the Mac also stays
            // findable on the shared relays (kb NET-20)
            // (PHN-591: `closes_at`, unix seconds, when the shared relays stop being kept open for them; `closed`: past it
            // with phones still waiting, which can't find the Mac until the owner reconnects or re-pairs them)
            "relay_handover": {"phones_waiting": self.phones_without_the_relay(), "on_shared_relays": i.policy.beacon().is_some(),
                               "closes_at": self.beacon_closes_at(),
                               "closed": self.phones_without_the_relay() > 0
                                   && self.beacon_closes_at().is_some_and(|at| crate::store::now() >= at)},
            "devices": devices,
            "pairing": pairing,
            // (kb PHN-444: the public relays carry control only; `paused_s`: since the relay's slow-down notice)
            "shared_relay": {"bulk": "refused", "page_max": l.shared_page_max, "paused_s": paused},
            "files_today": {"count": i.store.files_today().0, "bytes": i.store.files_today().1},
            "keys": i.store.keys_words(),
            "transfers": i.transfers.lock().unwrap().values().cloned().collect::<Vec<_>>(),
            "counters": net::relay_counters(&self.ep()),
        })
    }

    /// Opens the pairing window and returns what the QR shows. A new window replaces an open one.
    pub fn pair_start(&self) -> LinkResult<Value> {
        let l = self.limits();
        let max = self
            .inner
            .devices_max
            .load(std::sync::atomic::Ordering::Relaxed);
        if self.devices().len() as u32 >= max {
            return Err(LinkError::new(
                errors::DEVICE_LIMIT,
                format!("FERB already has {max} paired phones, the most it keeps."),
            )
            .allow("Remove one first"));
        }
        let relays = self.inner.policy.reachable_at();
        if relays.is_empty() {
            let why = match self.inner.policy.state() {
                RelayState::OwnDown { error: Some(e) }
                | RelayState::OwnFallback { error: Some(e) } => e,
                RelayState::OwnDown { .. } => LinkError::new(
                    errors::OWN_RELAY_UNREACHABLE,
                    "Your relay is not connected yet.",
                )
                .allow("Wait a moment, or press Test in the relay setting"),
                _ => LinkError::new(
                    errors::RELAY_UNREACHABLE,
                    "This Mac is not connected to a relay yet, so a phone could not find it.",
                )
                .allow("Check the Mac's internet connection and try again"),
            };
            return Err(why);
        }
        let (window, secret) = Window::open(l.pair_window_s);
        let invite = Invite {
            mac: self.ep().id(),
            relays: relays.iter().map(|u| u.to_string()).collect(),
            own: self.inner.policy.own(),
            secret,
        };
        let mut p = self.inner.pairing.lock().unwrap();
        if let Some(old) = p.pending.take() {
            self.cancel_request(old.request_id);
        }
        p.window = Some(window);
        // `url` is the link every build reads; `code` the short form the QR shows (the link itself when the
        // short form cannot carry this setup). Both name the same window.
        let url = invite.to_url();
        let code = invite.to_code().unwrap_or_else(|_| url.clone());
        Ok(
            json!({"url": url, "code": code, "qr": crate::qr::rows(&code), "qr_old": crate::qr::rows(&url), "seconds": l.pair_window_s}),
        )
    }
    pub fn pair_cancel(&self) {
        let mut p = self.inner.pairing.lock().unwrap();
        p.window = None;
        if let Some(old) = p.pending.take() {
            self.cancel_request(old.request_id);
        }
    }
    /// The daemon forgets a pairing the phone or the owner left before the Yes.
    fn cancel_request(&self, request_id: String) {
        let daemon = self.inner.daemon.clone();
        tokio::spawn(async move {
            let _ = daemon
                .link_call(
                    "link.pair_cancel",
                    json!({"request_id": request_id}),
                    Duration::from_secs(5),
                )
                .await;
        });
    }
    pub fn devices(&self) -> Vec<Device> {
        self.inner.devices.lock().unwrap().clone()
    }
    /// Closes every connection of a phone the daemon removed; a call already running gets no reply.
    pub fn cut(&self, id: &str) {
        if let Some(conns) = self.inner.live.lock().unwrap().remove(id) {
            for c in conns {
                c.lock().unwrap().conn.close(
                    close_code(errors::DEVICE_REVOKED).into(),
                    errors::DEVICE_REVOKED.as_bytes(),
                );
            }
        }
    }
    pub async fn relay_test(&self, spec: &RelaySpec) -> LinkResult<relaytest::RelayOk> {
        relaytest::test(spec, self.ep().secret_key().clone()).await
    }
    /// "Use my own relay": tested first; saved and applied only when the test passes (or `None` to switch off).
    /// Paired phones that are connected learn it at once, the others at their next connection.
    pub async fn relay_set(&self, spec: Option<RelaySpec>) -> LinkResult<Value> {
        let tested = match &spec {
            Some(s) => Some(self.relay_test(s).await?),
            None => None,
        };
        self.inner.store.set_own_relay(spec.clone())?;
        // tell the phones before the Mac moves, while the old path still carries the message; they then
        // dial the new relay
        self.push_all(json!({"type": "relay", "relay": self.relay_info_with(spec.clone())}))
            .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        self.inner.policy.set_own(spec).await?;
        Ok(json!({"saved": true, "test": tested}))
    }
    /// What phones did, newest last (the audit file in the state folder; no phone call reads or changes it).
    pub fn audit(&self, n: usize) -> Vec<Value> {
        self.inner.store.audit_tail(n)
    }
    /// Test switch: make every byte cross a relay even when a direct path works. Phones reconnect.
    pub async fn force_relay(&self, on: bool) -> LinkResult<Value> {
        self.inner.policy.set_force_relay(on).await?;
        Ok(json!({"force_relay": on}))
    }
    pub fn limits_get(&self) -> Limits {
        self.limits()
    }
    /// Merges the given fields into the limits. Unknown names are refused.
    pub fn limits_set(&self, patch: &Value) -> LinkResult<Limits> {
        let mut cur = serde_json::to_value(self.limits()).unwrap();
        let (Some(map), Some(p)) = (cur.as_object_mut(), patch.as_object()) else {
            return Err(LinkError::new(
                errors::BAD_REQUEST,
                "limits must be an object",
            ));
        };
        for (k, v) in p {
            if !map.contains_key(k) {
                return Err(LinkError::new(
                    errors::BAD_REQUEST,
                    format!("\"{k}\" is not a limit"),
                ));
            }
            map.insert(k.clone(), v.clone());
        }
        let l: Limits = serde_json::from_value(cur)
            .map_err(|e| LinkError::new(errors::BAD_REQUEST, e.to_string()))?;
        self.inner.store.set_limits(l.clone())?;
        Ok(l)
    }

    // ---- shared pieces ------------------------------------------------------------------------------------------

    fn relay_info_with(&self, own: Option<RelaySpec>) -> Value {
        json!({
            "own": own,
            "homes": self.inner.policy.reachable_at().iter().map(|u| u.to_string()).collect::<Vec<_>>(),
            "state": self.inner.policy.state(),
        })
    }
    fn relay_info(&self) -> Value {
        self.relay_info_with(self.inner.policy.own())
    }
    async fn push_all(&self, v: Value) {
        let senders: Vec<mpsc::Sender<Value>> = self
            .inner
            .live
            .lock()
            .unwrap()
            .values()
            .flatten()
            .map(|l| l.lock().unwrap().events.clone())
            .collect();
        for s in senders {
            let _ = s.try_send(v.clone());
        }
    }

    /// Relay facts that change by themselves: the rate-limit notice, the relay state, unused phones.
    async fn watch_loop(self) {
        let mut relay_changes = self.inner.policy.subscribe();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut hour = Instant::now();
        // where connected phones were last told the Mac is (its home relays)
        let mut told = self.inner.policy.reachable_at();
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                r = relay_changes.changed() => {
                    if r.is_err() { return; }
                    let info = self.relay_info();
                    told = self.inner.policy.reachable_at();
                    self.tell_admin(json!({"event": "relay", "relay": info}));
                    self.push_all(json!({"type": "relay", "relay": info})).await;
                    continue;
                }
            }
            if self.is_closed() {
                return;
            }
            // decision: PHN-742
            // The home relay moved without the relay state changing (iroh switched faster than the state is
            // read): the connected phones are told where the Mac is now, as for any relay change, so their next
            // dial starts there. Phones of every build read this event.
            let homes = self.inner.policy.reachable_at();
            if !homes.is_empty() && homes != told {
                told = homes;
                crate::diag::log("the Mac's home relay moved; the connected phones are told");
                self.push_all(json!({"type": "relay", "relay": self.relay_info()}))
                    .await;
            }
            let c = net::relay_counters(&self.ep());
            let noticed = {
                let mut b = self.inner.budget.lock().unwrap();
                let new = c.ratelimited > b.seen_ratelimited;
                b.seen_ratelimited = c.ratelimited; // (a new endpoint starts its counters at 0)
                new
            };
            if noticed {
                self.note_ratelimit().await;
            }
            if hour.elapsed() > Duration::from_secs(3600) {
                hour = Instant::now();
                // (an unused phone is warned about by the daemon and never removed by itself: kb PHN-419)
                self.inner.store.audit_expire(self.limits().audit_keep_days);
            }
        }
    }

    async fn note_ratelimit(&self) {
        let l = self.limits();
        self.inner.budget.lock().unwrap().pause_until =
            Some(Instant::now() + Duration::from_secs(l.ratelimit_pause_s));
        let e = ratelimited(l.ratelimit_pause_s);
        self.tell_admin(json!({"event": "relay_rate_limited", "error": e}));
        self.push_all(json!({"type": "notice", "error": e})).await;
    }

    // ---- accepting phones ------------------------------------------------------------------------------------------

    /// PHN-591: when the shared relays' second endpoint closes even if a phone never came back: `BEACON_MAX_DAYS`
    /// after the relay setting last changed (unix seconds); None without an own relay.
    fn beacon_closes_at(&self) -> Option<u64> {
        self.inner.policy.own()?;
        self.inner
            .store
            .relay_set_at()
            .map(|at| at + BEACON_MAX_DAYS * 86_400)
    }
    /// Paired phones not yet seen with the Mac's current own-relay setting (0 with no own relay).
    fn phones_without_the_relay(&self) -> usize {
        match self.inner.policy.own() {
            Some(own) => {
                let mark = own.mark();
                self.devices()
                    .iter()
                    .filter(|d| {
                        self.inner.store.device_relay(&d.id).as_deref() != Some(mark.as_str())
                    })
                    .count()
            }
            None => 0,
        }
    }
    /// A change of the relay setting reaches only the phones connected in that moment. For every other paired
    /// phone the Mac stays findable where that phone last knew it, on the shared relays, through a second
    /// endpoint; the phone's next connection there is told the setting (the welcome carries it) and moves. When
    /// every paired phone was seen on the own relay the second endpoint is closed: nothing of an own-relay
    /// user is on the shared relays then (kb NET-20); and at the latest `BEACON_MAX_DAYS` after the relay setting
    /// changed, even if a phone never came back (PHN-590, PHN-591).
    async fn beacon_loop(self) {
        while !self.is_closed() {
            let open = beacon_open(
                self.phones_without_the_relay(),
                self.beacon_closes_at(),
                crate::store::now(),
            );
            let _ = self.inner.policy.set_beacon(open).await;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let _ = self.inner.policy.set_beacon(false).await;
    }
    async fn beacon_accept_loop(self) {
        let mut changes = self.inner.policy.beacon_changes();
        while !self.is_closed() {
            let Some(ep) = self.inner.policy.beacon() else {
                tokio::select! {
                    _ = changes.changed() => {}
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
                continue;
            };
            loop {
                tokio::select! {
                    incoming = ep.accept() => {
                        let Some(incoming) = incoming else { break };
                        let me = self.clone();
                        // the same door as the main endpoint: pairing, the method list and every check apply
                        tokio::spawn(async move {
                            let Ok(conn) = incoming.await else { return };
                            me.handle(conn).await;
                        });
                    }
                    _ = changes.changed() => break,
                }
            }
        }
    }

    async fn accept_loop(self) {
        let mut generations = self.inner.policy.generations();
        loop {
            let ep = self.ep();
            loop {
                tokio::select! {
                    incoming = ep.accept() => {
                        let Some(incoming) = incoming else { break };
                        let me = self.clone();
                        tokio::spawn(async move {
                            let Ok(conn) = incoming.await else { return };
                            me.handle(conn).await;
                        });
                    }
                    _ = generations.changed() => break, // the relay set changed: accept on the new endpoint
                }
            }
            if self.is_closed() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn refuse(conn: &Connection, e: &LinkError) {
        conn.close(close_code(&e.code).into(), e.code.as_bytes());
    }

    async fn handle(self, conn: Connection) {
        let peer = conn.remote_id().to_string();
        let l = self.limits();
        let paired = self.device(&peer).is_some();
        let _slot = if paired {
            // a paired phone that reconnects in a storm is told to slow down
            let storm = {
                let mut c = self.inner.connects.lock().unwrap();
                let q = c.entry(peer.clone()).or_default();
                while q
                    .front()
                    .is_some_and(|t| t.elapsed() > Duration::from_secs(60))
                {
                    q.pop_front();
                }
                if q.len() as u32 >= l.connects_per_min {
                    Some(
                        60u64
                            .saturating_sub(q.front().map(|t| t.elapsed().as_secs()).unwrap_or(0))
                            .max(1),
                    )
                } else {
                    q.push_back(Instant::now());
                    None
                }
            };
            if let Some(wait) = storm {
                let e = LinkError::new(
                    errors::TOO_MANY_REQUESTS,
                    "This phone reconnected too often.",
                )
                .retry(wait);
                self.fail_hello(&conn, e, Duration::from_secs(2)).await;
                return;
            }
            None
        } else {
            // Not paired: with no pairing window open there is nothing to say (the close code is the answer).
            let open = self
                .inner
                .pairing
                .lock()
                .unwrap()
                .window
                .as_ref()
                .is_some_and(|w| !w.expired());
            if !open {
                Self::refuse(&conn, &errors::not_paired());
                return;
            }
            match self.inner.unpaired.clone().try_acquire_owned() {
                Ok(s) => Some(s),
                Err(_) => {
                    Self::refuse(&conn, &LinkError::new(errors::PAIR_BUSY, "busy"));
                    return;
                }
            }
        };
        let hello_wait = Duration::from_secs(l.unpaired_hello_s);
        let first = tokio::time::timeout(hello_wait, async {
            let (tx, mut rx) = conn
                .accept_bi()
                .await
                .map_err(|e| LinkError::new(errors::LINK_CLOSED, e.to_string()))?;
            let frame = wire::read_frame(&mut rx, 4096, hello_wait, false).await?;
            Ok::<_, LinkError>((tx, rx, frame))
        })
        .await;
        let (mut tx, rx, frame) = match first {
            Ok(Ok(x)) => x,
            Ok(Err(e)) => return Self::refuse(&conn, &e),
            Err(_) => return Self::refuse(&conn, &LinkError::new(errors::STALLED, "no hello")),
        };
        let _ = tx.set_priority(1);
        let hello: Hello = match frame {
            Some((K_HELLO, body)) => match wire::parse(&body) {
                Ok(h) => h,
                Err(e) => return Self::refuse(&conn, &e),
            },
            _ => {
                return Self::refuse(
                    &conn,
                    &LinkError::new(errors::BAD_REQUEST, "expected hello"),
                );
            }
        };
        if hello.v < MIN_VERSION || hello.min_v > VERSION {
            let e = errors::version(VERSION, MIN_VERSION, hello.v, hello.min_v);
            return self.end_hello(&conn, tx, e).await;
        }
        let device = match self.device(&peer) {
            Some(d) => d,
            None => match self.pair(&conn, &mut tx, &peer, &hello).await {
                Ok(d) => d,
                Err(e) => return self.end_hello(&conn, tx, e).await,
            },
        };
        drop(_slot);
        self.session(conn, tx, rx, device, hello).await;
    }

    /// An error before the session exists: sent as an error frame on the hello stream, then the connection
    /// closes with the matching code (the phone sees one or the other, whichever arrives).
    async fn end_hello(&self, conn: &Connection, mut tx: SendStream, e: LinkError) {
        let _ = wire::write_json(&mut tx, K_ERROR, &e).await;
        let _ = tx.finish();
        let _ = tokio::time::timeout(Duration::from_secs(2), tx.stopped()).await;
        Self::refuse(conn, &e);
    }
    async fn fail_hello(&self, conn: &Connection, e: LinkError, wait: Duration) {
        if let Ok(Ok((tx, _rx))) = tokio::time::timeout(wait, conn.accept_bi()).await {
            return self.end_hello(conn, tx, e).await;
        }
        Self::refuse(conn, &e);
    }

    async fn pair(
        &self,
        conn: &Connection,
        tx: &mut SendStream,
        peer: &str,
        hello: &Hello,
    ) -> LinkResult<Device> {
        let l = self.limits();
        let Some(proof) = hello.pair.as_deref().and_then(b64_any) else {
            // an endpoint that only knows the Mac's id: nothing is shown on the Mac
            return Err(errors::not_paired());
        };
        let closed = || {
            LinkError::new(
                errors::PAIR_WINDOW_CLOSED,
                "The pairing code is no longer valid (it works once, for a short time).",
            )
            .allow("Choose Pair a phone on the Mac again and scan the new code")
        };
        {
            let mut p = self.inner.pairing.lock().unwrap();
            let Some(w) = p.window.as_mut() else {
                return Err(closed());
            };
            if w.expired() {
                p.window = None;
                return Err(closed());
            }
            match w.check(&proof, self.ep().id(), conn.remote_id(), l.pair_wrong_max) {
                Checked::Wrong { ended } => {
                    if ended {
                        p.window = None;
                    }
                    drop(p);
                    self.tell_admin(json!({"event": "pair_wrong_secret", "window_closed": ended}));
                    return Err(LinkError::new(
                        errors::PAIR_SECRET_WRONG,
                        "The pairing code did not match.",
                    )
                    .allow("Scan the code FERB shows on the Mac again"));
                }
                Checked::Right => {
                    p.window = None; // single use: the secret is gone whatever the owner answers
                }
            }
        }
        let name = store::clean_name(&hello.name);
        let code = crate::pairing::code(&proof);
        // decision: PHN-102
        // The phone proved it saw the QR: the daemon asks the owner in FERB.app, naming the phone and the six digits
        // both screens show, and adds the phone on his Yes (kb PHN-468, PHN-469). The keys are the phone's to offer;
        // the daemon keeps only real keys.
        let asked = self
            .inner
            .daemon
            .link_call(
                "link.pair_request",
                json!({"endpoint": peer, "name": name, "app": hello.app.chars().take(40).collect::<String>(), "code": code,
                       "unlock_pub": hello.unlock_pub, "device_pub": hello.device_pub, "strong_pub": hello.strong_pub}),
                Duration::from_secs(5),
            )
            .await?;
        let request_id = asked["request_id"].as_str().unwrap_or_default().to_string();
        self.inner.pairing.lock().unwrap().pending = Some(Pending {
            phone: peer.to_string(),
            name: name.clone(),
            code: code.clone(),
            request_id: request_id.clone(),
        });
        self.tell_admin(json!({"event": "pair_request", "phone": peer, "name": name, "app": hello.app, "code": code, "request_id": request_id}));
        wire::write_json(
            tx,
            K_EVENT,
            &json!({"type": "pairing", "state": "waiting_for_mac"}),
        )
        .await?;
        let wait = Duration::from_secs(l.pair_confirm_s + 10);
        let answer = tokio::select! {
            r = self.inner.daemon.link_call("link.pair_wait", json!({"request_id": request_id}), wait) => r,
            _ = conn.closed() => {
                self.cancel_request(request_id.clone());
                Ok(json!({"paired": false}))
            }
        };
        {
            let mut p = self.inner.pairing.lock().unwrap();
            if p.pending
                .as_ref()
                .is_some_and(|x| x.request_id == request_id)
            {
                p.pending = None;
            }
        }
        let yes = matches!(&answer, Ok(v) if v["paired"] == json!(true));
        if !yes {
            self.tell_admin(json!({"event": "pair_ended", "phone": peer, "paired": false}));
            answer?;
            return Err(LinkError::new(
                errors::PAIR_DENIED,
                "Pairing was not approved on the Mac.",
            )
            .allow("Choose Pair a phone on the Mac again and say Yes when it asks"));
        }
        self.refresh_devices().await?;
        let d = self.device(peer).ok_or_else(errors::not_paired)?;
        self.tell_admin(
            json!({"event": "pair_ended", "phone": peer, "paired": true, "name": d.name}),
        );
        Ok(d)
    }

    // ---- one phone's connection ---------------------------------------------------------------------------------------

    async fn session(
        self,
        conn: Connection,
        mut tx: SendStream,
        _hello_rx: RecvStream,
        device: Device,
        hello: Hello,
    ) {
        let l = self.limits();
        // (the lock key is taken at pairing only: adding one later would let whoever holds the phone's link key choose it;
        // the daemon notes the phone as seen when it opens the session)
        let own = self.inner.policy.own_url();
        let tier = net::tier(&conn, own.as_ref());
        let welcome = json!({
            "v": VERSION, "min_v": MIN_VERSION, "level": crate::LEVEL, "build": crate::build(),
            "device": {"id": device.id, "name": device.name, "unlock": device.unlock},
            "tier": tier, "tier_words": tier.words(),
            "relay": self.relay_info(),
            "limits": l,
        });
        if wire::write_json(&mut tx, K_WELCOME, &welcome)
            .await
            .is_err()
        {
            return;
        }
        let (ev_tx, mut ev_rx) = mpsc::channel::<Value>(64);
        let live = Arc::new(Mutex::new(Live {
            phone_net: None,
            level: hello.level,
            build: hello.build.chars().take(40).collect(),
            conn: conn.clone(),
            events: ev_tx.clone(),
            tier,
            why: None,
            handover: None,
        }));
        // A phone that starts over connects on a new endpoint while its session still runs on the old one
        // (kb NET-25): this connection then goes on with that session. It is the same door: the same paired
        // identity, its own challenges, and the phone's unlock only after it proved the phone's device key.
        let continues = {
            let mut all = self.inner.live.lock().unwrap();
            let v = all.entry(device.id.clone()).or_default();
            let was = v.last().map(|x| x.lock().unwrap().phone_net.clone());
            if let Some(net) = was.clone().flatten() {
                live.lock().unwrap().phone_net = Some(net);
            }
            v.push(live.clone());
            was.is_some()
        };
        self.tell_admin(json!({"event": "phone_online", "id": device.id, "name": device.name, "tier": tier, "path": net::path_words(&conn), "moved": continues}));

        let mut sid = [0u8; 12];
        rand::RngCore::fill_bytes(&mut rand::rng(), &mut sid);
        let sess = Arc::new(Session {
            bridge: self.clone(),
            conn: conn.clone(),
            device_id: device.id.clone(),
            sid: B64.encode(sid),
            events: ev_tx,
            calls: Default::default(),
            last_preview: Default::default(),
            live: live.clone(),
            phone_net: Mutex::new(live.lock().unwrap().phone_net.clone()),
            began: Instant::now(),
            continues,
        });

        // the event stream: everything FERB has to tell the phone without being asked
        let writer = tokio::spawn(async move {
            while let Some(v) = ev_rx.recv().await {
                if wire::write_json(&mut tx, K_EVENT, &v).await.is_err() {
                    break;
                }
            }
        });
        let pusher = tokio::spawn(sess.clone().push_loop());

        loop {
            let (tx, rx) = match conn.accept_bi().await {
                Ok(s) => s,
                Err(_) => break,
            };
            let _ = tx.set_priority(1);
            tokio::spawn(sess.clone().call(tx, rx));
        }
        pusher.abort();
        writer.abort();
        let gone = {
            let mut all = self.inner.live.lock().unwrap();
            if let Some(v) = all.get_mut(&device.id) {
                v.retain(|x| !Arc::ptr_eq(x, &live));
                if v.is_empty() {
                    all.remove(&device.id);
                }
            }
            !all.contains_key(&device.id)
        };
        // (uploads stay where they landed: they are the owner's files now). The daemon ends this connection's
        // challenges and unlock.
        let _ = self
            .inner
            .daemon
            .call(
                &device.id,
                &sess.sid,
                "phone.session_end",
                json!({}),
                None,
                Duration::from_secs(3),
            )
            .await;
        // (a connection its phone replaced with a newer one ended: the phone is still here)
        if gone {
            self.tell_admin(json!({"event": "phone_offline", "id": device.id}));
        }
    }
}

/// One start-over as the owner reads it in the Activity list: kinds, counts and times.
pub fn handover_words(h: &Value) -> String {
    let after = format!(
        "try {} · {} s after {}",
        h["n"].as_u64().unwrap_or(0),
        h["since_s"].as_u64().unwrap_or(0),
        match h["trigger"].as_str() {
            Some("network") => "the phone's network changed",
            Some("opened") => "the app was opened",
            _ => "it began",
        }
    );
    let on = h["network"]
        .as_str()
        .map(|k| format!(" · phone on {k}"))
        .unwrap_or_default();
    let r = &h["relayed"];
    let before = format!(
        " · before it, on the relay: the phone tried {} times for a direct path, sent {} and received {} bytes on IPv4 ({} and {} on IPv6); the Mac sent {} and received {} on IPv4",
        r["tries"].as_u64().unwrap_or(0),
        r["tx_v4"].as_u64().unwrap_or(0),
        r["rx_v4"].as_u64().unwrap_or(0),
        r["tx_v6"].as_u64().unwrap_or(0),
        r["rx_v6"].as_u64().unwrap_or(0),
        h["mac"]["tx_v4"].as_u64().unwrap_or(0),
        h["mac"]["rx_v4"].as_u64().unwrap_or(0)
    );
    if h["moved"] == json!(true) {
        format!(
            "{after}{on} · moved to a new endpoint in {} ms, new ports: {}, calls held {} ms, unlock carried: {} · 3 s later: {} · cost {} bytes sent, {} received{before}",
            h["took_ms"].as_u64().unwrap_or(0),
            yes(&h["ports_changed"]),
            h["held_ms"].as_u64().unwrap_or(0),
            yes(&h["proved"]),
            match h["direct_3s"].as_bool() {
                Some(true) => "direct",
                Some(false) => "still on the relay",
                None => "not said",
            },
            h["cost"]["tx"].as_u64().unwrap_or(0),
            h["cost"]["rx"].as_u64().unwrap_or(0)
        )
    } else {
        format!(
            "{after}{on} · not through ({}); the session stayed where it was, calls held {} ms{before}",
            h["reason"]
                .as_str()
                .unwrap_or("no reason said")
                .replace('_', " "),
            h["held_ms"].as_u64().unwrap_or(0)
        )
    }
}
fn yes(v: &Value) -> &'static str {
    match v.as_bool() {
        Some(true) => "yes",
        Some(false) => "no",
        None => "not said",
    }
}

fn ratelimited(wait: u64) -> LinkError {
    LinkError::new(errors::RELAY_RATE_LIMITED, format!("The shared relay asked FERB to slow down for {wait} s. Messages and questions still work."))
        .allow("Use the same Wi-Fi as the Mac, or set up your own relay in FERB's settings")
        .retry(wait)
}

struct Session {
    bridge: Bridge,
    conn: Connection,
    device_id: String,
    /// This connection's name towards the daemon: its challenges and its unlock live and end with it there.
    sid: String,
    events: mpsc::Sender<Value>,
    calls: Mutex<VecDeque<Instant>>,
    last_preview: Mutex<Option<Instant>>,
    live: Arc<Mutex<Live>>,
    /// What the phone's own network looks like, as the phone reported it.
    phone_net: Mutex<Option<net::NetFacts>>,
    began: Instant,
    /// This connection took a running session over from an older connection of the same phone.
    continues: bool,
}

#[derive(Deserialize)]
struct Req {
    method: String,
    #[serde(default)]
    params: Value,
    /// The phone's unlock proof `{payload, sig}`: passed to the daemon as it came; only the daemon verifies it.
    #[serde(default)]
    proof: Option<Value>,
}

/// One file on its way, listed in the Mac's status while it travels.
struct Travelling {
    bridge: Bridge,
    id: u64,
}
impl Travelling {
    fn start(sess: &Session, to_phone: bool, name: &str, total: u64, done: u64) -> Self {
        let b = sess.bridge.clone();
        let id = b
            .inner
            .next_transfer
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        b.inner.transfers.lock().unwrap().insert(id, json!({"id": id, "device": sess.device_id, "to_phone": to_phone, "name": name, "total": total, "done": done, "tier": sess.tier(), "started": store::now()}));
        Self { bridge: b, id }
    }
    fn progress(&self, done: u64, tier: Tier) {
        if let Some(t) = self
            .bridge
            .inner
            .transfers
            .lock()
            .unwrap()
            .get_mut(&self.id)
        {
            t["done"] = json!(done);
            t["tier"] = json!(tier);
        }
    }
}
impl Drop for Travelling {
    fn drop(&mut self) {
        self.bridge.inner.transfers.lock().unwrap().remove(&self.id);
    }
}

impl Session {
    fn limits(&self) -> Limits {
        self.bridge.limits()
    }
    fn tier(&self) -> Tier {
        net::tier(&self.conn, self.bridge.inner.policy.own_url().as_ref())
    }
    /// Why this connection is not direct, from both sides' network reports (None when it is).
    fn why(&self) -> Option<String> {
        net::why_not_direct(
            self.tier(),
            &net::facts(&self.bridge.ep()),
            self.phone_net.lock().unwrap().as_ref(),
        )
    }
    /// Whether a direct connection is being tried for right now, and how long the upgrade took once it happened
    /// (kb NET-14). Facts only: the path in use, the Mac's test switch, and both sides' network reports.
    /// Whether this is the phone's newest connection (an older one no longer speaks for the phone).
    fn newest(&self) -> bool {
        self.bridge
            .inner
            .live
            .lock()
            .unwrap()
            .get(&self.device_id)
            .and_then(|v| v.last())
            .is_some_and(|x| Arc::ptr_eq(x, &self.live))
    }
    fn direct(&self, tier: Tier) -> Value {
        let newest = self.newest();
        let mut all = self.bridge.inner.direct_times.lock().unwrap();
        let e = all
            .entry(self.device_id.clone())
            .or_insert_with(|| json!({}));
        if !newest {
            let for_s = e["off_direct"]
                .as_u64()
                .map(|t| store::now().saturating_sub(t));
            return json!({"trying": e["trying"], "for_s": for_s, "time_to_direct_ms": e["time_to_direct_ms"], "last_time_to_direct_ms": e["last_time_to_direct_ms"]});
        }
        if e["session"] != json!(self.sid) {
            if self.continues && !e["session"].is_null() {
                // the same session on a new endpoint: how long it has been off a direct path goes on counting
                e["session"] = json!(self.sid);
            } else {
                *e = json!({"session": self.sid, "since": store::now(), "last_time_to_direct_ms": e["time_to_direct_ms"].as_u64().or(e["last_time_to_direct_ms"].as_u64())});
            }
        }
        if tier == Tier::Direct {
            // how long this stretch off a direct path lasted: from the session's start, or from the moment it
            // last fell back to a relay
            if e["time_to_direct_ms"].is_null() {
                let since = e["off_direct"]
                    .as_u64()
                    .map(|t| store::now().saturating_sub(t) * 1000)
                    .unwrap_or(self.began.elapsed().as_millis() as u64);
                e["time_to_direct_ms"] = json!(since);
            }
            e["off_direct"] = Value::Null;
            e["trying"] = json!(false);
        } else {
            // off a direct path: an earlier change to direct is history now, never this stretch's result
            if let Some(ms) = e["time_to_direct_ms"].as_u64() {
                e["last_time_to_direct_ms"] = json!(ms);
                e["time_to_direct_ms"] = Value::Null;
            }
            if e["off_direct"].is_null() {
                e["off_direct"] = json!(if e["last_time_to_direct_ms"].is_null() {
                    e["since"].as_u64().unwrap_or_else(store::now)
                } else {
                    store::now()
                });
            }
            let mac = net::facts(&self.bridge.ep());
            let udp = |f: &net::NetFacts| !f.known || f.udp_v4 || f.udp_v6;
            let phone_ok = self.phone_net.lock().unwrap().as_ref().is_none_or(udp);
            e["trying"] = json!(!self.bridge.inner.policy.force_relay() && udp(&mac) && phone_ok);
        }
        // (`for_s`: how long it has been off a direct path, while it is)
        let for_s = e["off_direct"]
            .as_u64()
            .map(|t| store::now().saturating_sub(t));
        json!({"trying": e["trying"], "for_s": for_s, "time_to_direct_ms": e["time_to_direct_ms"], "last_time_to_direct_ms": e["last_time_to_direct_ms"]})
    }
    fn still_paired(&self) -> LinkResult<Device> {
        self.bridge
            .device(&self.device_id)
            .ok_or_else(errors::revoked)
    }
    fn wait(&self) -> Duration {
        Duration::from_secs(self.limits().call_deadline_s)
    }
    /// A connection to the daemon as this phone. The daemon not knowing the phone means it was removed there: the
    /// connection is cut, whatever the event that would say so.
    async fn connect(&self) -> LinkResult<crate::daemon::Conn> {
        match self
            .bridge
            .inner
            .daemon
            .connect(&self.device_id, &self.sid, self.tier().as_str())
            .await
        {
            Err(e) if e.code == crate::daemon::UNKNOWN_PHONE => {
                let e = errors::revoked();
                Bridge::refuse(&self.conn, &e);
                Err(e)
            }
            r => r,
        }
    }
    /// One call to the daemon as this phone, with the phone's proof passed through.
    async fn daemon(&self, method: &str, params: Value, proof: Option<Value>) -> LinkResult<Value> {
        self.connect()
            .await?
            .call_with(method, params, proof, false, self.wait())
            .await
    }

    /// Pushes status changes, tier changes and FERB-not-running to the phone. The daemon is polled locally;
    /// only changes cross the link.
    async fn push_loop(self: Arc<Self>) {
        let mut last_status = String::new();
        let mut last_tier = None;
        let mut last_why: Option<String> = None;
        let mut last_trying = None;
        let mut down = false;
        let mut knows_relay = false;
        let mut n = 0u64;
        loop {
            let tier = self.tier();
            let why = self.why();
            let trying = self.direct(tier);
            // (`for_s` counts up by itself: it travels with the next change, it is not one)
            let state = {
                let mut t = trying.clone();
                t["for_s"] = Value::Null;
                t
            };
            if last_tier != Some(tier) || why != last_why || last_trying != Some(state.clone()) {
                if last_tier.is_some() || why.is_some() || trying["trying"] == json!(true) {
                    let _ = self.events.send(json!({"type": "tier", "tier": tier, "tier_words": tier.words(), "why": why, "path_facts": net::path_facts(&self.conn), "direct": trying})).await;
                }
                self.live.lock().unwrap().why = why.clone();
                last_why = why;
                last_trying = Some(state);
            }
            if last_tier != Some(tier) {
                if last_tier.is_some() {
                    self.bridge.tell_admin(json!({"event": "phone_tier", "id": self.device_id, "tier": tier, "path": net::path_words(&self.conn), "path_facts": net::path_facts(&self.conn)}));
                    self.bridge.inner.store.audit(&json!({"t": store::now(), "device": self.device_id, "method": "link.path", "class": "link", "tier": tier, "result": "ok", "via": net::path_facts(&self.conn)["words"], "paths": crate::diag::paths(&self.conn),
                                                           "net": {"mac": net::facts(&self.bridge.ep()), "phone": self.phone_net.lock().unwrap().clone()}}));
                }
                last_tier = Some(tier);
                self.live.lock().unwrap().tier = tier;
            }
            if !knows_relay && n.is_multiple_of(2) {
                // seen on a path through the own relay as it is set now: this phone has the setting (its address,
                // token and authority), so the Mac need not stay findable elsewhere for it
                if let Some(own) = self.bridge.inner.policy.own().filter(|_| {
                    !matches!(
                        self.bridge.inner.policy.state(),
                        net::RelayState::OwnFallback { .. }
                    )
                }) {
                    let url = own.parsed().ok();
                    if self.conn.paths().iter().any(|p| matches!(p.remote_addr(), iroh::TransportAddr::Relay(u) if Some(u) == url.as_ref())) {
                        self.bridge.inner.store.set_device_relay(&self.device_id, &own.mark());
                        knows_relay = true;
                    }
                }
            }
            if n.is_multiple_of(3) {
                // (the daemon adds what the phone needs to answer each question: whether a Yes needs its unlock,
                // and the one-time challenge to sign)
                match self
                    .daemon("chat.status", json!({"peek": true}), None)
                    .await
                {
                    Ok(mut st) => {
                        down = false;
                        // decision: PHN-763 — the planner's question goes only to an app that knows it
                        crate::status_for_level(&mut st, self.live.lock().unwrap().level);
                        let plain = st.to_string();
                        if plain != last_status {
                            last_status = plain;
                            fit(&mut st, self.limits().control_frame_max as usize - 256);
                            let _ = self
                                .events
                                .send(json!({"type": "status", "status": st}))
                                .await;
                        }
                    }
                    Err(e) => {
                        if !down {
                            down = true;
                            last_status.clear();
                            let _ = self
                                .events
                                .send(json!({"type": "notice", "error": e}))
                                .await;
                        }
                    }
                }
            }
            n += 1;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn call(self: Arc<Self>, mut tx: SendStream, mut rx: RecvStream) {
        let l = self.limits();
        let stall = Duration::from_secs(l.stall_s);
        let mut req: Req = match wire::read_frame(&mut rx, l.control_frame_max, stall, false).await
        {
            Ok(Some((K_REQ, mut body))) => {
                let parsed = wire::parse(&body);
                body.fill(0); // the frame may have carried a hidden answer
                match parsed {
                    Ok(r) => r,
                    Err(e) => return finish(tx, Err(e)).await,
                }
            }
            Ok(_) => {
                return finish(
                    tx,
                    Err(LinkError::new(errors::BAD_REQUEST, "expected a request")),
                )
                .await;
            }
            Err(e) => {
                if e.code == errors::FRAME_TOO_LARGE {
                    // refused at the header; the rest of that frame is not read
                    let _ = rx.stop(close_code(errors::FRAME_TOO_LARGE).into());
                } else if e.code == errors::STALLED {
                    let _ = tx.reset(close_code(errors::STALLED).into());
                    return;
                }
                return finish(tx, Err(e)).await;
            }
        };
        // revocation is rechecked on every call
        if let Err(e) = self.still_paired() {
            let _ = wire::write_json(&mut tx, K_ERROR, &e).await;
            Bridge::refuse(&self.conn, &e);
            return;
        }
        let flood = {
            let mut q = self.calls.lock().unwrap();
            while q
                .front()
                .is_some_and(|t| t.elapsed() > Duration::from_secs(10))
            {
                q.pop_front();
            }
            if q.len() as u32 >= l.calls_per_10s {
                Some(
                    10u64
                        .saturating_sub(q.front().map(|t| t.elapsed().as_secs()).unwrap_or(0))
                        .max(1),
                )
            } else {
                q.push_back(Instant::now());
                None
            }
        };
        if let Some(wait) = flood {
            return finish(
                tx,
                Err(LinkError::new(
                    errors::TOO_MANY_REQUESTS,
                    "Too many requests from this phone in a short time.",
                )
                .retry(wait)),
            )
            .await;
        }
        if req.proof.as_ref().is_some_and(Value::is_null) {
            req.proof = None;
        }
        let method: String = req
            .method
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_'))
            .take(60)
            .collect();
        let about = req
            .params
            .get("path")
            .and_then(Value::as_str)
            .map(|p| p.chars().take(300).collect::<String>());
        let what = req
            .params
            .get("what")
            .and_then(Value::as_str)
            .map(str::to_string);
        let out = match req.method.as_str() {
            "link.status" => Ok(self.link_status()),
            "link.challenge" => self.daemon("phone.challenge", json!({}), None).await,
            "link.unlock" => {
                self.daemon("phone.unlock", json!({}), req.proof.take())
                    .await
            }
            "link.lock" => self.daemon("phone.lock", json!({}), None).await,
            // PHN-649: the unlock signed in the app's opening prompt before the link was up; its age by the phone's
            // steady clock rides beside the signed statement
            "link.unlock_statement" => {
                let age = req.params.get("age_ms").cloned().unwrap_or(Value::Null);
                self.daemon(
                    "phone.unlock_statement",
                    json!({"age_ms": age}),
                    req.proof.take(),
                )
                .await
            }
            // PHN-646: "Forget this Mac" on the phone: the daemon removes this phone only (its device key proved on
            // this connection); the removal then closes its connections as any removal does
            "link.forget" => {
                self.bridge
                    .inner
                    .forgetting
                    .lock()
                    .unwrap()
                    .insert(self.device_id.clone());
                let out = self.daemon("phone.forget_self", json!({}), None).await;
                if out.is_err() {
                    self.bridge
                        .inner
                        .forgetting
                        .lock()
                        .unwrap()
                        .remove(&self.device_id);
                }
                out
            }
            // this connection comes from the paired phone itself: a signature by its device key (no prompt)
            "link.present" => {
                self.daemon("phone.present", json!({}), req.proof.take())
                    .await
            }
            // the phone's device key and fingerprint-only key, for a phone paired before they existed: registered
            // only under an unlock signed for exactly these keys (the daemon checks); the owner's store keeps them
            "link.keys" => match self
                .daemon("phone.keys", req.params.clone(), req.proof.take())
                .await
            {
                Ok(_) => {
                    let _ = self.bridge.refresh_devices().await;
                    Ok(json!({"enrolled": true}))
                }
                Err(e) => Err(e),
            },
            "link.files" => {
                let path = req.params.get("path").cloned().unwrap_or(json!("~"));
                let mut ask = json!({"path": path});
                // decision: PHN-589 ("load more": the Mac's marker for the next page, which the Mac checks is this
                // phone's, folder's and query's; a key place's proof stays over the path alone)
                if let Some(after) = req.params.get("after").filter(|a| !a.is_null()) {
                    ask["after"] = after.clone();
                }
                self.daemon("phone.files", ask, req.proof.take())
                    .await
                    .and_then(|mut v| {
                        // decision: PHN-444 (a listing the public relay would have to cut is refused there, never cut)
                        self.fits_the_path(&v, "This folder's listing")?;
                        fit(&mut v, l.control_frame_max as usize - 256);
                        Ok(v)
                    })
            }
            // a whole folder's file list is the start of fetching its files, and a folder upload's start that of
            // sending them: neither on the public relays (kb PHN-444)
            "link.tree" => match self.bulk_path("Fetching a folder") {
                Err(e) => Err(e),
                Ok(_) => match self
                    .daemon(
                        "phone.tree",
                        json!({"path": req.params.get("path")}),
                        req.proof.take(),
                    )
                    .await
                {
                    Ok(v) => self.reply_whole(&mut tx, &mut rx, v).await,
                    Err(e) => Err(e),
                },
            },
            "link.put_group" => match self.bulk_path("Sending a folder") {
                Err(e) => Err(e),
                Ok(_) => {
                    self.daemon(
                        "phone.put_group",
                        json!({"name": req.params.get("name")}),
                        None,
                    )
                    .await
                }
            },
            "link.note" => {
                if req.params.get("ratelimited").and_then(Value::as_bool) == Some(true)
                    && self.tier().is_shared()
                {
                    self.bridge.note_ratelimit().await;
                }
                if let Some(f) = req
                    .params
                    .get("net")
                    .and_then(|n| serde_json::from_value::<net::NetFacts>(n.clone()).ok())
                {
                    self.live.lock().unwrap().phone_net = Some(f.clone());
                    *self.phone_net.lock().unwrap() = Some(f);
                }
                if let Some(h) = req.params.get("handover") {
                    self.note_handover(h);
                }
                Ok(json!({"ok": true}))
            }
            "link.get" => {
                let _ = tx.set_priority(-1);
                let proof = req.proof.take();
                self.bulk_get(&mut tx, &mut rx, &req.params, proof).await
            }
            "link.put" => {
                let _ = tx.set_priority(-1);
                self.bulk_put(&mut tx, &mut rx, &req.params).await
            }
            // the phone session's own calls and the file calls go through the link's own (`link.*`), which apply
            // the connection's limits; the phone area's other calls (`phone.list`, `phone.remove`, the phone's own
            // device record, recent files and the name search) go on to FERB by their rows
            m if ferb_rpc::auth::PHONE_SESSION.contains(&m)
                || ferb_rpc::auth::PHONE_FILES.contains(&m) =>
            {
                Err(LinkError::new(
                    errors::BAD_REQUEST,
                    format!(
                        "{m} is asked through the link's own calls (link.get, link.files, link.put), which apply the limits of the connection."
                    ),
                ))
            }
            _ => self.forward(&mut tx, &mut rx, req).await,
        };
        // the audit: what was asked and how it ended; never a parameter's value (a path on the Mac is not one)
        let class = match method.as_str() {
            "link.unlock" | "link.unlock_statement" | "link.keys" => "sensitive",
            "link.files" | "link.tree" => "private",
            "link.get" if what.as_deref() == Some("path") => "private",
            m if m.starts_with("link.") => "link",
            m => role::class(m).map(|c| c.as_str()).unwrap_or("unknown"),
        };
        if !matches!(
            method.as_str(),
            "link.status"
                | "link.note"
                | "link.challenge"
                | "link.present"
                | "chat.status"
                | "ping"
        ) || out.is_err()
        {
            let mut line = json!({"t": store::now(), "device": self.device_id, "method": method, "class": class, "tier": self.tier(),
                                  "result": match &out { Ok(_) => "ok", Err(e) => e.code.as_str() }});
            if let Some(w) = what {
                line["what"] = json!(w);
            }
            if let Some(p) = about {
                line["path"] = json!(p);
            }
            if let Some(n) = out
                .as_ref()
                .ok()
                .and_then(|v| v.get("size"))
                .and_then(Value::as_u64)
            {
                line["bytes"] = json!(n);
            }
            if let Some(s) = out
                .as_ref()
                .ok()
                .and_then(|v| v.get("saved"))
                .and_then(Value::as_str)
            {
                line["path"] = json!(s);
            }
            self.bridge.inner.store.audit(&line);
        }
        finish(tx, out).await;
    }

    /// The phone says what its last start-over did (kb NET-25). Kept as kinds, counts and times only, whatever
    /// was sent; shown in the owner's device list and written to the audit.
    fn note_handover(&self, h: &Value) {
        let n = |v: &Value| v.as_u64().map(|x| x.min(u32::MAX as u64));
        let word = |v: &Value| {
            v.as_str().map(|s| {
                s.chars()
                    .filter(|c| c.is_ascii_lowercase() || matches!(c, ' ' | '_' | '-'))
                    .take(40)
                    .collect::<String>()
            })
        };
        let counts = |v: &Value, keys: &[&str]| -> Value {
            keys.iter()
                .filter_map(|k| n(&v[*k]).map(|x| (k.to_string(), json!(x))))
                .collect::<serde_json::Map<_, _>>()
                .into()
        };
        let mut last = json!({"t": store::now(), "n": n(&h["n"]), "since_s": n(&h["since_s"]), "trigger": word(&h["trigger"]), "network": word(&h["network"]),
            "moved": h["moved"].as_bool(), "reason": word(&h["reason"]), "ports_changed": h["ports_changed"].as_bool(), "proved": h["proved"].as_bool(),
            "took_ms": n(&h["took_ms"]), "held_ms": n(&h["held_ms"]), "direct_3s": h["direct_3s"].as_bool(),
            "relayed": counts(&h["relayed"], &["tries", "tx_v4", "rx_v4", "tx_v6", "rx_v6"]), "cost": counts(&h["cost"], &["tx", "rx"])});
        // the Mac's own side of the same stretch: bytes on IP (to anyone) since the note before, and its paths now
        let c = crate::diag::counters(&self.bridge.ep());
        let tier = self.tier();
        let words = {
            let mut all = self.bridge.inner.handovers.lock().unwrap();
            let e = all
                .entry(self.device_id.clone())
                .or_insert_with(|| json!({}));
            let first = last["n"].as_u64() == Some(1) || e["tries"].is_null();
            let base = (
                e["mac_tx_v4"]
                    .as_u64()
                    .filter(|_| !first)
                    .unwrap_or(c.tx_v4),
                e["mac_rx_v4"]
                    .as_u64()
                    .filter(|_| !first)
                    .unwrap_or(c.rx_v4),
            );
            last["mac"] = json!({"tx_v4": c.tx_v4.saturating_sub(base.0), "rx_v4": c.rx_v4.saturating_sub(base.1), "paths": crate::diag::paths(&self.conn)});
            let (tries, moved) = if first {
                (0, 0)
            } else {
                (
                    e["tries"].as_u64().unwrap_or(0),
                    e["moved"].as_u64().unwrap_or(0),
                )
            };
            let since = if first {
                store::now().saturating_sub(last["since_s"].as_u64().unwrap_or(0))
            } else {
                e["since"].as_u64().unwrap_or_else(store::now)
            };
            *e = json!({"since": since, "tries": tries + 1, "moved": moved + (last["moved"] == json!(true)) as u64, "last": last, "mac_tx_v4": c.tx_v4, "mac_rx_v4": c.rx_v4});
            handover_words(&e["last"])
        };
        self.live.lock().unwrap().handover = Some(last.clone());
        self.bridge.inner.store.audit(&json!({"t": store::now(), "device": self.device_id, "method": "link.startover", "class": "link", "tier": tier,
                                               "result": if last["moved"] == json!(true) { "ok" } else { "not_through" }, "words": words, "facts": last}));
    }

    fn link_status(&self) -> Value {
        let tier = self.tier();
        let mut s = self.bridge.status();
        json!({"tier": tier, "tier_words": tier.words(), "why": self.why(), "path": net::path_words(&self.conn), "path_facts": net::path_facts(&self.conn),
               "direct": self.direct(tier),
               "relay": self.bridge.relay_info(), "shared_relay": s["shared_relay"].take(), "limits": self.limits()})
    }

    /// A reply too large for a control frame is data: it travels as bulk, under the tier's rules, whole (on the
    /// public relays it is refused, kb PHN-444).
    async fn reply_whole(
        &self,
        tx: &mut SendStream,
        rx: &mut RecvStream,
        v: Value,
    ) -> LinkResult<Value> {
        let body = serde_json::to_vec(&v).unwrap_or_default();
        if body.len() > self.limits().control_frame_max as usize - 256 {
            let _ = tx.set_priority(-1);
            let sent = self
                .send_bytes(tx, rx, "This reply", body, json!({"json": true}))
                .await?;
            return Ok(json!({"bulk_json": true, "size": sent["size"], "tier": sent["tier"]}));
        }
        Ok(v)
    }

    /// The path may carry bulk (kb PHN-444): pictures, files, folders, older pages, large replies. The answer is
    /// read again for every chunk (`readmit`).
    fn bulk_path(&self, what: &str) -> LinkResult<Tier> {
        let tier = self.tier();
        if tier.carries_bulk() {
            Ok(tier)
        } else {
            Err(errors::tier_refused(what, tier.words()))
        }
    }
    /// A reply that would not fit one control frame uncut is refused on the public relays (kb PHN-444).
    fn fits_the_path(&self, v: &Value, what: &str) -> LinkResult<()> {
        let size = serde_json::to_vec(v).map(|b| b.len()).unwrap_or(0) as u64;
        if size > self.limits().control_frame_max - 256 {
            self.bulk_path(what)?;
        }
        Ok(())
    }
    /// What the public relays let through of a forwarded call before it reaches FERB (kb PHN-444): no pictures, and
    /// of the chat only its latest page, at most `shared_page_max` entries, made to fit one control frame.
    fn public_relay_rules(&self, method: &str, params: &mut Value) -> LinkResult<()> {
        let tier = self.tier();
        if tier.carries_bulk() {
            return Ok(());
        }
        let l = self.limits();
        if role::PICTURES.contains(&method) {
            return Err(errors::tier_refused("A picture", tier.words()));
        }
        if let Some(older) = role::PAGES
            .iter()
            .find(|(m, _)| *m == method)
            .map(|(_, older)| *older)
        {
            if older.iter().any(|k| {
                params
                    .get(*k)
                    .is_some_and(|v| !v.is_null() && v != &json!(""))
            }) {
                return Err(errors::tier_refused("Older messages", tier.words()));
            }
            if params.get("limit").and_then(Value::as_u64).unwrap_or(0) > l.shared_page_max {
                return Err(LinkError::new(
                    errors::TRANSFER_TOO_LARGE,
                    format!(
                        "On {} the phone gets the latest {} messages at most.",
                        tier.words(),
                        l.shared_page_max
                    ),
                )
                .allow(
                    "Ask for fewer, or use the same Wi-Fi as the Mac or your own relay for more",
                ));
            }
            if params.get("limit").is_none() {
                params["limit"] = json!(l.shared_page_max);
            }
            if method == "chat.entries.page" {
                // the engine cuts the page to this size itself (`max_bytes`, kb ENG-14) and says `more`
                let cap = l.control_frame_max - 1024;
                if params
                    .get("max_bytes")
                    .and_then(Value::as_u64)
                    .is_none_or(|m| m > cap)
                {
                    params["max_bytes"] = json!(cap);
                }
            }
        }
        Ok(())
    }

    /// A call for FERB's daemon. The bridge refuses what is not on the phone's list at all; what the call's class
    /// needs (the unlock window, a proof signed for this exact call, the answer rules) the daemon checks itself,
    /// with the proof as the phone sent it.
    async fn forward(
        &self,
        tx: &mut SendStream,
        rx: &mut RecvStream,
        req: Req,
    ) -> LinkResult<Value> {
        let Req {
            method,
            mut params,
            proof,
        } = req;
        if params.is_null() {
            params = json!({});
        }
        // this phone's own uploads may ride with a message (the daemon knows them by their ids); nothing else can
        if method != "chat.send" && params.get("attachments").is_some() {
            return Err(LinkError::new(
                errors::PARAM_NOT_ALLOWED,
                format!("\"attachments\" cannot be sent with {method} from a phone."),
            ));
        }
        let class = role::check(&method, &params, self.live.lock().unwrap().level)?;
        // decision: PHN-444
        self.public_relay_rules(&method, &mut params)?;
        let sealed = class == role::Class::Sensitive || method == "chat.answer";
        if role::is_stream(&method) {
            return self.forward_stream(tx, &method, params, proof).await;
        }
        if sealed {
            // sent on like a hidden answer (a key or a setting may be a secret): straight to the daemon's own
            // method, wiped here once sent; never logged, stored, cut or echoed by the bridge
            let mut conn = match self.connect().await {
                Ok(c) => c,
                Err(e) => {
                    wipe(&mut params);
                    return Err(e);
                }
            };
            let mut out = conn
                .call_sealed(&method, params, proof, self.wait())
                .await?;
            if role::on_mac_screen(&method) && out.is_object() {
                // kb SEC-20: FERB asks the owner here, on the phone, whether it should click the prompt it opened
                out["on_mac_screen"] = json!(
                    "This opened a prompt or a window on your Mac's screen. FERB asks you here whether it should click it for you."
                );
            }
            return Ok(out);
        }
        let mut v = self.daemon(&method, params, proof).await?;
        if method == "chat.status" {
            // decision: PHN-763
            crate::status_for_level(&mut v, self.live.lock().unwrap().level);
            fit(&mut v, self.limits().control_frame_max as usize - 256);
            return Ok(v);
        }
        self.reply_whole(tx, rx, v).await
    }

    async fn forward_stream(
        &self,
        tx: &mut SendStream,
        method: &str,
        params: Value,
        proof: Option<Value>,
    ) -> LinkResult<Value> {
        let max = self.limits().control_frame_max as usize - 256;
        let mut conn = self.connect().await?;
        let tx = tokio::sync::Mutex::new(tx);
        conn.stream(method, params, proof, |mut ev| {
            let tx = &tx;
            async move {
                fit(&mut ev, max);
                // false ends the daemon stream: the phone left; the task itself keeps running in FERB
                wire::write_json(*tx.lock().await, K_EVENT, &ev)
                    .await
                    .is_ok()
            }
        })
        .await
    }

    // ---- bulk: pictures and files, in acknowledged chunks, under the tier's rules -----------------------------

    /// Tier rules for one transfer of `size` bytes: none on the public relays (kb PHN-444), refused before its first
    /// byte with the code that says a direct path or the owner's relay carries it.
    fn admit(&self, what: &str, size: u64) -> LinkResult<Tier> {
        let l = self.limits();
        let tier = self.tier();
        if !tier.carries_bulk() {
            return Err(errors::tier_refused(what, tier.words()));
        }
        if l.good_transfer_max > 0 && size > l.good_transfer_max {
            return Err(LinkError::new(
                errors::TRANSFER_TOO_LARGE,
                format!(
                    "{what} is {} and FERB sends at most {} to a phone.",
                    errors::human(size),
                    errors::human(l.good_transfer_max)
                ),
            ));
        }
        Ok(tier)
    }

    /// A tier change in the middle of a transfer: fine towards a better path; towards the public relays it stops
    /// (kb PHN-444), with what was sent kept for going on later where that resumes (`offset`, `resume`).
    fn readmit(&self, what: &str) -> LinkResult<Tier> {
        let now = self.tier();
        if !now.carries_bulk() {
            return Err(LinkError::new(errors::NEEDS_BETTER_PATH, format!("The connection moved to {} during the transfer, which carries messages, questions and Stop only, so {} stopped.", now.words(), what.to_lowercase()))
                .allow("Use the same Wi-Fi as the Mac, or set up your own relay in FERB's settings"));
        }
        Ok(now)
    }

    async fn bulk_get(
        &self,
        tx: &mut SendStream,
        rx: &mut RecvStream,
        params: &Value,
        proof: Option<Value>,
    ) -> LinkResult<Value> {
        let l = self.limits();
        let kind = params
            .get("what")
            .and_then(Value::as_str)
            .unwrap_or_default();
        // decision: PHN-444 (refused on the public relays before FERB is even asked: nothing is read or captured)
        match kind {
            "preview" => self.bulk_path(
                if params.get("live").and_then(Value::as_bool) == Some(true) {
                    "Live preview"
                } else {
                    "A picture of what FERB is doing"
                },
            )?,
            "file" | "path" => self.bulk_path("This file")?,
            // decision: APP-983 — Show me: the Mac's camera crosses only a direct path or the owner's relay
            "camera" => self.bulk_path("Your Mac's camera")?,
            _ => Tier::Direct,
        };
        match kind {
            "preview" => {
                let tier = self.tier();
                let every = Duration::from_millis(l.good_preview_every_ms);
                {
                    let mut last = self.last_preview.lock().unwrap();
                    if let Some(t) = *last
                        && t.elapsed() < every {
                            let wait = (every - t.elapsed()).as_secs().max(1);
                            return Err(LinkError::new(errors::PREVIEW_TOO_SOON, format!("On {} FERB sends a picture at most every {} s.", tier.words(), every.as_secs().max(1))).retry(wait));
                        }
                    *last = Some(Instant::now());
                }
                let mut p = json!({"task_id": params.get("task_id").cloned().unwrap_or(Value::Null)});
                // PHN-797: a coding session's window rides the same path as a task's preview
                for k in ["worker", "have", "session"] {
                    if let Some(v) = params.get(k) {
                        p[k] = v.clone();
                    }
                }
                let mut ans = self.daemon("tasks.preview", p, None).await?;
                let image = ans.as_object_mut().and_then(|m| m.remove("image")).and_then(|v| v.as_str().map(str::to_string));
                match image.and_then(|s| B64S.decode(s).ok()) {
                    Some(bytes) => self.send_bytes(tx, rx, "This picture", bytes, ans).await,
                    None => Ok(json!({"meta": ans, "size": 0})), // nothing new to show
                }
            }
            // insomnia mode's Show me (APP-983): the latest frame of the Mac's camera, in memory only, as a preview's
            "camera" => {
                let tier = self.tier();
                let every = Duration::from_millis(l.good_preview_every_ms);
                {
                    let mut last = self.last_preview.lock().unwrap();
                    if let Some(t) = *last
                        && t.elapsed() < every {
                            let wait = (every - t.elapsed()).as_secs().max(1);
                            return Err(LinkError::new(errors::PREVIEW_TOO_SOON, format!("On {} FERB sends a picture at most every {} s.", tier.words(), every.as_secs().max(1))).retry(wait));
                        }
                    *last = Some(Instant::now());
                }
                let mut p = json!({});
                if let Some(v) = params.get("have") {
                    p["have"] = v.clone();
                }
                let mut ans = self.daemon("insomnia.camera", p, None).await?;
                let image = ans.as_object_mut().and_then(|m| m.remove("image")).and_then(|v| v.as_str().map(str::to_string));
                match image.and_then(|s| B64S.decode(s).ok()) {
                    Some(bytes) => self.send_bytes(tx, rx, "This picture", bytes, ans).await,
                    None => Ok(json!({"meta": ans, "size": 0})),
                }
            }
            // a finished task's file, or any one file the owner could open (never a folder): the daemon opens it
            // (where links really lead decides there) and streams it; the bridge never touches the file
            "file" => self.send_file(tx, rx, json!({"artifact": {"task_id": params.get("task_id"), "index": params.get("index")}}), None).await,
            "path" => self.send_file(tx, rx, json!({"path": params.get("path"), "offset": params.get("offset").and_then(Value::as_u64).unwrap_or(0)}), proof).await,
            _ => Err(LinkError::new(errors::BAD_REQUEST, "link.get takes what: \"preview\", \"camera\", \"file\" or \"path\"")),
        }
    }

    /// One file from the daemon to the phone. The daemon says what it opened (`meta`), then its hash, then the
    /// bytes; the transfer is admitted for this connection's tier before the first byte is asked for.
    async fn send_file(
        &self,
        tx: &mut SendStream,
        rx: &mut RecvStream,
        ask: Value,
        proof: Option<Value>,
    ) -> LinkResult<Value> {
        let l = self.limits();
        let what = "This file";
        let mut conn = self.connect().await?;
        let id = conn.start_stream("phone.read", ask, proof).await?;
        let lost = || LinkError::new(errors::FERB_ERROR, "FERB stopped sending the file.");
        let head = conn
            .next_event("phone.read", id, self.wait())
            .await?
            .ok_or_else(lost)?;
        let meta = head.get("meta").cloned().ok_or_else(lost)?;
        let size = meta["size"].as_u64().unwrap_or(0);
        // admitted before it is read: an oversized file is never loaded
        let mut tier = self.admit(what, size)?;
        // (a phone that takes unusually much is the daemon's to notice, once per burst: `link.events heavy_fetch`)
        self.bridge.inner.store.count_file(size);
        let name = meta["name"].as_str().unwrap_or("file").to_string();
        let offset = meta["offset"].as_u64().unwrap_or(0);
        let travelling = Travelling::start(
            self,
            true,
            &name,
            meta["total"].as_u64().unwrap_or(size),
            offset,
        );
        // (hashing a very large file takes a while: this wait is long on purpose)
        let sha = conn
            .next_event("phone.read", id, Duration::from_secs(1800))
            .await?
            .and_then(|v| v["sha256"].as_str().map(str::to_string))
            .ok_or_else(lost)?;
        let stall = Duration::from_secs(l.stall_s);
        let chunk = l.chunk_bytes as usize;
        wire::write_json(tx, K_EVENT, &json!({"bulk": {"size": size, "chunk": chunk, "sha256": sha, "meta": {"name": name, "offset": offset, "total": meta["total"]}}})).await?;
        let (mut sent, mut acked, mut bytes) = (0u32, 0u32, 0u64);
        let mut buf = vec![0u8; chunk + 4];
        while let Some(ev) = conn
            .next_event("phone.read", id, Duration::from_secs(30))
            .await?
        {
            let piece = ev["d"]
                .as_str()
                .and_then(|d| B64S.decode(d).ok())
                .ok_or_else(lost)?;
            if bytes + piece.len() as u64 > size {
                return Err(LinkError::new(
                    errors::FERB_ERROR,
                    "The file grew while it was being sent.",
                ));
            }
            for part in piece.chunks(chunk) {
                tier = self.readmit(what)?;
                let window = l.good_window.max(1);
                while sent - acked >= window {
                    read_ack(rx, stall).await?;
                    acked += 1;
                }
                buf[..4].copy_from_slice(&sent.to_be_bytes());
                buf[4..4 + part.len()].copy_from_slice(part);
                wire::write_frame(tx, K_CHUNK, &buf[..part.len() + 4]).await?;
                sent += 1;
                bytes += part.len() as u64;
                travelling.progress(offset + bytes, tier);
            }
        }
        while acked < sent {
            read_ack(rx, stall).await?;
            acked += 1;
        }
        Ok(json!({"ok": true, "size": size, "chunks": sent, "tier": tier}))
    }

    /// Bytes the bridge already holds (a preview picture, a large reply), as acknowledged chunks.
    async fn send_bytes(
        &self,
        tx: &mut SendStream,
        rx: &mut RecvStream,
        what: &str,
        bytes: Vec<u8>,
        meta: Value,
    ) -> LinkResult<Value> {
        use sha2::{Digest, Sha256};
        let l = self.limits();
        let size = bytes.len() as u64;
        let mut tier = self.admit(what, size)?;
        let stall = Duration::from_secs(l.stall_s);
        let chunk = l.chunk_bytes as usize;
        wire::write_json(tx, K_EVENT, &json!({"bulk": {"size": size, "chunk": chunk, "sha256": B64.encode(Sha256::digest(&bytes)), "meta": meta}})).await?;
        let (mut sent, mut acked) = (0u32, 0u32);
        let mut buf = vec![0u8; chunk + 4];
        for part in bytes.chunks(chunk) {
            tier = self.readmit(what)?;
            let window = l.good_window.max(1);
            while sent - acked >= window {
                read_ack(rx, stall).await?;
                acked += 1;
            }
            buf[..4].copy_from_slice(&sent.to_be_bytes());
            buf[4..4 + part.len()].copy_from_slice(part);
            wire::write_frame(tx, K_CHUNK, &buf[..part.len() + 4]).await?;
            sent += 1;
        }
        while acked < sent {
            read_ack(rx, stall).await?;
            acked += 1;
        }
        Ok(json!({"ok": true, "size": size, "chunks": sent, "tier": tier}))
    }

    /// A file from the phone. The daemon takes it (behind the phone's unlock and the Mac's free-disk floor), writes
    /// it and marks it as downloaded; the bridge only carries the chunks. `resume` goes on with an upload that
    /// stopped part way; `group` and `rel` place a file inside a folder being sent.
    async fn bulk_put(
        &self,
        tx: &mut SendStream,
        rx: &mut RecvStream,
        params: &Value,
    ) -> LinkResult<Value> {
        let l = self.limits();
        // decision: PHN-444 (refused before FERB is asked to take anything: no upload is begun on the Mac)
        self.bulk_path("Sending a file")?;
        let size = params
            .get("size")
            .and_then(Value::as_u64)
            .ok_or_else(|| LinkError::new(errors::BAD_REQUEST, "link.put needs the size"))?;
        let mut begin = json!({"name": params.get("name").and_then(Value::as_str).unwrap_or("file"), "size": size});
        for k in ["resume", "group", "rel"] {
            if let Some(v) = params.get(k).filter(|v| !v.is_null()) {
                begin[k] = v.clone();
            }
        }
        let mut conn = self.connect().await?;
        let began = conn.call("phone.put_begin", begin, self.wait()).await?;
        let id = began["upload_id"].as_str().unwrap_or_default().to_string();
        let offset = began["offset"].as_u64().unwrap_or(0).min(size);
        let mut tier = match self.admit("This file", size - offset) {
            Ok(t) => t,
            Err(e) => {
                let _ = conn
                    .call(
                        "phone.put_end",
                        json!({"upload_id": id, "abort": true, "drop": offset == 0}),
                        self.wait(),
                    )
                    .await;
                return Err(e);
            }
        };
        let travelling = Travelling::start(
            self,
            false,
            params.get("name").and_then(Value::as_str).unwrap_or("file"),
            size,
            offset,
        );
        let stall = Duration::from_secs(l.stall_s);
        let mut got = offset;
        let received: LinkResult<()> = async {
            wire::write_json(tx, K_EVENT, &json!({"accept": {"chunk": l.chunk_bytes, "window": l.good_window, "upload_id": id, "offset": offset}})).await?;
            while got < size {
                tier = self.readmit("This file")?;
                match wire::read_frame(rx, l.chunk_bytes + 4, stall, false).await? {
                    Some((K_CHUNK, body)) if body.len() > 4 => {
                        let n = body.len() as u64 - 4;
                        if got + n > size {
                            return Err(LinkError::new(errors::TRANSFER_TOO_LARGE, "More bytes arrived than the transfer announced."));
                        }
                        conn.call("phone.put_chunk", json!({"upload_id": id, "d": B64S.encode(&body[4..])}), self.wait()).await?;
                        got += n;
                        travelling.progress(got, tier);
                        wire::write_frame(tx, K_ACK, &body[..4]).await?;
                    }
                    Some(_) => return Err(LinkError::new(errors::BAD_REQUEST, "expected a chunk")),
                    None => return Err(LinkError::new(errors::LINK_CLOSED, "The phone stopped sending (paused, or the connection went).")),
                }
            }
            Ok(())
        }
        .await;
        if let Err(e) = received {
            // stopped part way (a pause, a lost connection): the daemon keeps what arrived for a while, to go on with
            let _ = conn
                .call(
                    "phone.put_end",
                    json!({"upload_id": id, "abort": true}),
                    self.wait(),
                )
                .await;
            return Err(e);
        }
        // the phone's own hash of what it sent, when it gives one: the daemon checks it (S3b)
        let mut end = json!({"upload_id": id});
        if let Some(sha) = params.get("sha256").filter(|v| v.is_string()) {
            end["sha256"] = sha.clone();
        }
        let mut done = conn.call("phone.put_end", end, self.wait()).await?;
        done["tier"] = json!(tier);
        Ok(done)
    }
}

async fn read_ack(rx: &mut RecvStream, stall: Duration) -> LinkResult<()> {
    match wire::read_frame(rx, 4, stall, false).await? {
        Some((K_ACK, _)) => Ok(()),
        _ => Err(LinkError::new(
            errors::LINK_CLOSED,
            "The phone stopped receiving.",
        )),
    }
}

/// A relay setting as Settings → Phone sends it through the daemon: the link its setup printed (`ferbrelay://…`, which
/// carries the token), or an address with its token, and a certificate for a relay with its own.
fn relay_spec(p: &Value) -> LinkResult<RelaySpec> {
    let url = p
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| LinkError::new(errors::BAD_REQUEST, "url is needed"))?;
    let mut s = RelaySpec::from_text(url, p.get("token").and_then(Value::as_str))?;
    if let Some(ca) = p
        .get("ca")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
    {
        s.ca = Some(ca.to_string());
    }
    Ok(s)
}

async fn finish(mut tx: SendStream, out: LinkResult<Value>) {
    let _ = match out {
        Ok(v) => wire::write_json(&mut tx, K_RESULT, &v).await,
        Err(e) => wire::write_json(&mut tx, K_ERROR, &e).await,
    };
    let _ = tx.finish();
}

/// Keeps a reply inside one control frame: long strings are cut (and marked) until it fits.
pub fn fit(v: &mut Value, max: usize) {
    let mut keep = 8000usize;
    while serde_json::to_vec(v).map(|b| b.len()).unwrap_or(0) > max && keep >= 100 {
        cut(v, keep);
        keep /= 2;
    }
    if serde_json::to_vec(v).map(|b| b.len()).unwrap_or(0) > max {
        *v = json!({"cut": true, "note": "This reply was too large for the phone link and was left out."});
    }
}
fn cut(v: &mut Value, keep: usize) {
    match v {
        Value::String(s) if s.len() > keep => {
            let mut end = keep;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            s.truncate(end);
            s.push_str(" …[cut for the phone]");
        }
        Value::Array(a) => a.iter_mut().for_each(|x| cut(x, keep)),
        Value::Object(o) => o.values_mut().for_each(|x| cut(x, keep)),
        _ => {}
    }
}
