//! The iroh endpoint both sides use, which relays it may use, and which tier a live connection is on.
//!
//! Tiers are facts about the path the Mac sends on, read from iroh; nothing here guesses. An endpoint is built
//! once: relay changes go through `insert_relay`/`remove_relay`, never a rebuild (kb NET-3).

use crate::errors::{self, LinkError, LinkResult};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayConfig, RelayMap, RelayMode, RelayUrl, SecretKey,
    TransportAddr, Watcher,
    endpoint::{Connection, QuicTransportConfig, presets},
    tls::CaTlsConfig,
};
use serde::{Deserialize, Serialize};
use std::{
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const ALPN: &[u8] = b"ferb/phone/1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Direct,
    OwnRelay,
    SharedRelay,
}
impl Tier {
    /// The tier's wire name (its JSON form), as the daemon is told it per call.
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Direct => "direct",
            Tier::OwnRelay => "own_relay",
            Tier::SharedRelay => "shared_relay",
        }
    }
    pub fn is_shared(self) -> bool {
        self == Tier::SharedRelay
    }
    /// Whether pictures, previews, files, older chat pages and large replies may cross this path. The public
    /// relays carry control only (kb PHN-444, PHN-410); a direct path and the owner's own relay carry everything
    /// (kb PHN-202).
    pub fn carries_bulk(self) -> bool {
        self != Tier::SharedRelay
    }
    /// The words used in every message about this tier.
    pub fn words(self) -> &'static str {
        match self {
            Tier::Direct => "a direct connection",
            Tier::OwnRelay => "your own relay",
            Tier::SharedRelay => "the shared relay",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelaySpec {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// The relay's own certificate authority (DER, base64) when it has no public certificate: a relay on a shared
    /// machine, reached by address and port. Only a server holding a certificate from it is accepted there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca: Option<String>,
}
pub const RELAY_LINK_SCHEME: &str = "ferbrelay";
const AUTHORITY_KEY: &str = "key:";
impl RelaySpec {
    pub fn new(url: impl Into<String>, token: Option<String>) -> Self {
        Self {
            url: url.into(),
            token,
            ca: None,
        }
    }
    /// What the owner pastes into "Use my own relay": the one-line link the setup script prints
    /// (`ferbrelay://host[:port]?tok=…[&ca=…]`), or a plain https address with an optional token.
    pub fn from_text(text: &str, token: Option<&str>) -> LinkResult<Self> {
        // a pasted link may arrive wrapped: spaces and line breaks inside it are never part of it
        let joined: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        let text = if joined.starts_with(&format!("{RELAY_LINK_SCHEME}://")) {
            joined.as_str()
        } else {
            text.trim()
        };
        if let Some(rest) = text.strip_prefix(&format!("{RELAY_LINK_SCHEME}://")) {
            let u = url::Url::parse(&format!("https://{rest}")).map_err(|e| {
                LinkError::new(
                    errors::OWN_RELAY_URL,
                    format!("This relay link could not be read ({e})."),
                )
                .allow("Paste the whole line the relay's setup printed")
            })?;
            let get = |k: &str| {
                u.query_pairs()
                    .find(|(n, _)| n == k)
                    .map(|(_, v)| v.to_string())
                    .filter(|v| !v.is_empty())
            };
            let mut plain = u.clone();
            plain.set_query(None);
            return Ok(Self {
                url: plain.to_string().trim_end_matches('/').to_string(),
                token: get("tok"),
                ca: get("ca"),
            });
        }
        Ok(Self {
            url: text.to_string(),
            token: token.filter(|t| !t.is_empty()).map(str::to_string),
            ca: None,
        })
    }
    pub fn to_link(&self) -> String {
        let mut u = url::Url::parse(&self.url)
            .unwrap_or_else(|_| url::Url::parse("https://invalid").unwrap());
        {
            let mut q = u.query_pairs_mut();
            if let Some(t) = &self.token {
                q.append_pair("tok", t);
            }
            if let Some(c) = &self.ca {
                q.append_pair("ca", c);
            }
        }
        u.to_string()
            .replacen("https://", &format!("{RELAY_LINK_SCHEME}://"), 1)
    }
    fn ca_der(ca: &str) -> LinkResult<Vec<u8>> {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(ca.trim_end_matches('='))
            .or_else(|_| base64::engine::general_purpose::STANDARD.decode(ca))
            .map_err(|_| {
                LinkError::new(
                    errors::OWN_RELAY_URL,
                    "The relay's certificate in this link is damaged.",
                )
                .allow("Paste the whole line the relay's setup printed")
            })
    }
    /// The authority as a short pairing code carries it: `key:<name>.<public key>` (both base64url) in place of
    /// the whole certificate. Only on a phone between scanning the code and the Mac's welcome, which brings the
    /// certificate itself.
    pub fn authority_text(name: &[u8], spki: &[u8]) -> String {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
        format!("{AUTHORITY_KEY}{}.{}", B64.encode(name), B64.encode(spki))
    }
    /// The name and public key of this relay's own authority: what a TLS check of the relay needs from its
    /// certificate, and nothing else of it.
    pub fn authority(&self) -> LinkResult<(Vec<u8>, Vec<u8>)> {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
        let damaged = || {
            LinkError::new(
                errors::OWN_RELAY_URL,
                "The relay's certificate in this link is damaged.",
            )
            .allow("Paste the whole line the relay's setup printed")
        };
        let ca = self.ca.as_deref().ok_or_else(damaged)?;
        if let Some(rest) = ca.strip_prefix(AUTHORITY_KEY) {
            let (name, spki) = rest.split_once('.').ok_or_else(damaged)?;
            return Ok((
                B64.decode(name).map_err(|_| damaged())?,
                B64.decode(spki).map_err(|_| damaged())?,
            ));
        }
        let mut store = rustls::RootCertStore::empty();
        store
            .add(rustls::pki_types::CertificateDer::from(Self::ca_der(ca)?))
            .map_err(|_| damaged())?;
        let anchor = store.roots.first().ok_or_else(damaged)?;
        Ok((
            anchor.subject.as_ref().to_vec(),
            anchor.subject_public_key_info.as_ref().to_vec(),
        ))
    }
    /// The trusted roots for this relay: the public ones, plus its own authority when it brought one.
    pub fn roots(&self) -> LinkResult<Option<CaTlsConfig>> {
        let Some(ca) = &self.ca else { return Ok(None) };
        if ca.starts_with(AUTHORITY_KEY) {
            // only the authority's name and key are known (a short pairing code): this relay's authority and
            // nothing else is accepted, checked the same way as with its whole certificate
            let (name, spki) = self.authority()?;
            return Ok(Some(CaTlsConfig::custom_server_cert_verifier(Arc::new(
                move |provider| {
                    let anchor = rustls::pki_types::TrustAnchor {
                        subject: name.clone().into(),
                        subject_public_key_info: spki.clone().into(),
                        name_constraints: None,
                    };
                    let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
                        Arc::new(rustls::RootCertStore {
                            roots: vec![anchor],
                        }),
                        provider,
                    )
                    .build()
                    .map_err(std::io::Error::other)?;
                    Ok(verifier as Arc<dyn rustls::client::danger::ServerCertVerifier>)
                },
            ))));
        }
        Ok(Some(CaTlsConfig::default().with_extra_roots([
            rustls::pki_types::CertificateDer::from(Self::ca_der(ca)?),
        ])))
    }
    /// A TLS client configuration that accepts this relay.
    pub fn client_tls(&self) -> LinkResult<rustls::ClientConfig> {
        self.roots()?
            .unwrap_or_default()
            .client_config(iroh_relay::tls::default_provider())
            .map_err(|e| {
                LinkError::new(
                    errors::OWN_RELAY_TLS,
                    format!("The relay's certificate could not be used ({e})."),
                )
            })
    }
    pub fn parsed(&self) -> LinkResult<RelayUrl> {
        parse_relay_url(&self.url)
    }
    /// A short name for exactly this setting (address, token, authority), to tell whether a phone has it. Says
    /// nothing of the token.
    pub fn mark(&self) -> String {
        use base64::Engine;
        let all = format!(
            "{}\n{}\n{}",
            self.url,
            self.token.as_deref().unwrap_or(""),
            self.ca.as_deref().unwrap_or("")
        );
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(&blake3::hash(all.as_bytes()).as_bytes()[..12])
    }
    pub fn config(&self) -> LinkResult<Arc<RelayConfig>> {
        let mut c = RelayConfig::from(self.parsed()?);
        if let Some(t) = &self.token {
            c = c.with_auth_token(t.clone());
        }
        Ok(Arc::new(c))
    }
}

/// https only, except plain http to this Mac itself (a relay in dev mode).
pub fn parse_relay_url(text: &str) -> LinkResult<RelayUrl> {
    let bad = |why: &str| {
        LinkError::new(
            errors::OWN_RELAY_URL,
            format!("\"{text}\" is not a relay address: {why}."),
        )
        .allow("Enter it as https://relay.example.com")
    };
    let url = url::Url::parse(text.trim()).map_err(|e| bad(&e.to_string()))?;
    let host = url.host_str().ok_or_else(|| bad("it has no host name"))?;
    let local = matches!(host, "localhost" | "127.0.0.1" | "[::1]");
    match url.scheme() {
        "https" => {}
        "http" if local => {}
        "http" => return Err(bad("a relay on another machine must use https")),
        _ => return Err(bad("it must start with https://")),
    }
    RelayUrl::from_str(url.as_str()).map_err(|e| bad(&e.to_string()))
}

/// The relays everyone may use when no own relay is set (and as the fallback): n0's, unless a test replaces them.
#[derive(Debug, Clone, Default)]
pub enum Shared {
    #[default]
    N0,
    Custom(Vec<RelaySpec>),
}
impl Shared {
    pub fn configs(&self) -> LinkResult<Vec<Arc<RelayConfig>>> {
        match self {
            Shared::N0 => Ok(iroh::defaults::prod::default_relay_map().relays::<Vec<_>>()),
            Shared::Custom(v) => v.iter().map(|s| s.config()).collect(),
        }
    }
    pub fn urls(&self) -> Vec<RelayUrl> {
        self.configs()
            .map(|v| v.iter().map(|c| c.url.clone()).collect())
            .unwrap_or_default()
    }
}

#[derive(Clone)]
pub struct NetConfig {
    pub secret: SecretKey,
    pub own_relay: Option<RelaySpec>,
    pub shared: Shared,
    /// Extra trusted roots (tests: the local relay's self-signed certificate).
    pub roots: Option<CaTlsConfig>,
    /// Tests only: remove the IP transports so every byte crosses a relay.
    pub force_relay: bool,
    pub accept: bool,
    pub streams_max: u32,
    /// "Always use my relay" (kb PHN-219, PHN-453): with an own relay set and in use, no IP transports, so the
    /// session stays on the owner's relay even when a direct path exists. Off by default: direct is used whenever
    /// one exists. It never applies while the public relays stand in for the own relay (then direct is preferred).
    pub relay_only: bool,
    /// QUIC's generic segmentation offload (GSO). On by default, as the lab ran; upstream iroh #4647 reports that
    /// on Android over cellular GSO batches are dropped silently and the direct path dies under upload, fixed by
    /// turning it off. A switch for a by-hand test on a phone, not a decision yet.
    pub segmentation_offload: bool,
    /// Asking the local gateway for a port mapping (UPnP, PCP, NAT-PMP): on in the product, as the lab ran; tests
    /// on 127.0.0.1 turn it off so nothing is sent beyond loopback.
    pub portmapper: bool,
    /// Tests only: the endpoint's one IP socket is 127.0.0.1, so the only direct path it offers or tries is this
    /// Mac's loopback; with port mapping off and relays on localhost, nothing it sends leaves the Mac (the test
    /// guard's sandbox, TEST-511, ends a test binary that sends anywhere else). Off in the product.
    pub loopback_only: bool,
}
impl NetConfig {
    /// The defaults for everything but the identity and the relays.
    pub fn new(secret: SecretKey, own_relay: Option<RelaySpec>, shared: Shared) -> Self {
        Self {
            secret,
            own_relay,
            shared,
            roots: None,
            force_relay: false,
            accept: false,
            streams_max: 1,
            relay_only: false,
            segmentation_offload: true,
            portmapper: true,
            loopback_only: false,
        }
    }
}

pub async fn bind(cfg: &NetConfig) -> LinkResult<Endpoint> {
    let map = match &cfg.own_relay {
        Some(own) => RelayMap::from_iter([own.config()?]),
        None => RelayMap::from_iter(cfg.shared.configs()?),
    };
    // Minimal preset: no address lookup is configured, so the Mac's id and relay are not published to n0's
    // DNS (kb NET-4). The phone learns the relay from the QR and over the link.
    let transport = QuicTransportConfig::builder()
        .max_concurrent_bidi_streams(cfg.streams_max.into())
        .max_concurrent_uni_streams(0u32.into())
        .enable_segmentation_offload(cfg.segmentation_offload)
        .build();
    let mut b = Endpoint::builder(presets::Minimal)
        .secret_key(cfg.secret.clone())
        .relay_mode(RelayMode::Custom(map))
        .transport_config(transport);
    if cfg.accept {
        b = b.alpns(vec![ALPN.to_vec()]);
    }
    // decision: PHN-453 (the Always use my relay switch: only with an own relay in use)
    if cfg.force_relay || (cfg.relay_only && cfg.own_relay.is_some()) {
        b = b.clear_ip_transports();
    } else if cfg.loopback_only {
        b = b
            .clear_ip_transports()
            .bind_addr("127.0.0.1:0")
            .map_err(|e| LinkError::new(errors::FERB_ERROR, format!("loopback: {e}")))?;
    }
    if !cfg.portmapper {
        b = b.portmapper_config(iroh::endpoint::PortmapperConfig::Disabled);
    }
    let roots = match (&cfg.roots, &cfg.own_relay) {
        (Some(r), _) => Some(r.clone()),
        (None, Some(own)) => own.roots()?,
        _ => None,
    };
    if let Some(roots) = roots {
        b = b.ca_tls_config(roots);
    }
    b.bind().await.map_err(|e| {
        LinkError::new(
            errors::RELAY_UNREACHABLE,
            format!("The network endpoint could not start ({e:#})."),
        )
    })
}

/// The tier of a live connection, from the path iroh currently sends application data on. With no selected
/// path yet the answer is the most restrictive one.
pub fn tier(conn: &Connection, own: Option<&RelayUrl>) -> Tier {
    for p in conn.paths().iter() {
        if p.is_selected() {
            return match p.remote_addr() {
                TransportAddr::Ip(_) => Tier::Direct,
                TransportAddr::Relay(url) if Some(url) == own => Tier::OwnRelay,
                _ => Tier::SharedRelay,
            };
        }
    }
    Tier::SharedRelay
}

pub fn path_words(conn: &Connection) -> String {
    conn.paths()
        .iter()
        .find(|p| p.is_selected())
        .map(|p| match p.remote_addr() {
            TransportAddr::Ip(_) => "direct".to_string(),
            TransportAddr::Relay(url) => format!("relay {url}"),
            _ => "other".into(),
        })
        .unwrap_or_else(|| "none".into())
}

/// What the selected path is, as facts and without the address itself: `direct ipv4 public`, `direct ipv4 lan`,
/// `direct ipv4 carrier-nat`, `direct ipv6 public`, `relay`, … (kb NET-13). Where iroh learned the address from
/// (the relay, a port mapping, the peer) is not something iroh 1.4.1 reports per path.
pub fn path_facts(conn: &Connection) -> serde_json::Value {
    let Some(p) = conn
        .paths()
        .iter()
        .find(|p| p.is_selected())
        .map(|p| p.remote_addr().clone())
    else {
        return serde_json::json!({"kind": "none", "words": "no path selected yet"});
    };
    match p {
        TransportAddr::Ip(addr) => {
            let ip = match addr.ip() {
                std::net::IpAddr::V6(v6) => v6
                    .to_ipv4_mapped()
                    .map(std::net::IpAddr::V4)
                    .unwrap_or(std::net::IpAddr::V6(v6)),
                v4 => v4,
            };
            let (family, scope) = match ip {
                std::net::IpAddr::V4(a) => (
                    "ipv4",
                    if a.is_loopback() {
                        "this Mac"
                    } else if a.is_private() {
                        "lan"
                    } else if a.is_link_local() {
                        "link-local"
                    } else if a.octets()[0] == 100 && (a.octets()[1] & 0xc0) == 64 {
                        "carrier-nat"
                    } else {
                        "public"
                    },
                ),
                std::net::IpAddr::V6(a) => (
                    "ipv6",
                    if a.is_loopback() {
                        "this Mac"
                    } else if (a.segments()[0] & 0xfe00) == 0xfc00 {
                        "lan"
                    } else if (a.segments()[0] & 0xffc0) == 0xfe80 {
                        "link-local"
                    } else {
                        "public"
                    },
                ),
            };
            serde_json::json!({"kind": "direct", "family": family, "scope": scope, "words": format!("direct, {family}, the phone's address as the Mac sees it is {}", match scope { "lan" => "on the same local network", "carrier-nat" => "in a carrier's shared range", "public" => "a public one", other => other })})
        }
        TransportAddr::Relay(_) => serde_json::json!({"kind": "relay", "words": "through a relay"}),
        _ => serde_json::json!({"kind": "other", "words": "another transport"}),
    }
}

pub fn home_relays(ep: &Endpoint) -> Vec<RelayUrl> {
    ep.home_relay_status()
        .get()
        .iter()
        .filter(|s| s.is_connected())
        .map(|s| s.url().clone())
        .collect()
}
/// Why the home relay is not connected, in the relay's or iroh's words.
pub fn home_problem(ep: &Endpoint) -> Option<(bool, String)> {
    ep.home_relay_status()
        .get()
        .iter()
        .find(|s| !s.is_connected())
        .and_then(|s| {
            s.auth_denied_reason()
                .map(|r| (true, r.to_string()))
                .or_else(|| s.last_error().map(|e| (false, format!("{e:#}"))))
        })
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct RelayCounters {
    pub sent: u64,
    pub received: u64,
    pub ratelimited: u64,
    pub conns: u64,
}
pub fn relay_counters(ep: &Endpoint) -> RelayCounters {
    let m = &ep.metrics().socket;
    RelayCounters {
        sent: m.send_relay.get(),
        received: m.recv_data_relay.get(),
        ratelimited: m.relay_conns_ratelimited.get(),
        conns: m.relay_home_change.get(),
    }
}

/// The Mac's address as the phone should dial it: its id plus every relay it may be found at.
pub fn dial_addr(id: EndpointId, relays: &[RelayUrl]) -> EndpointAddr {
    let mut a = EndpointAddr::new(id);
    for r in relays {
        a = a.with_relay_url(r.clone());
    }
    a
}

// ---- relay policy: own relay first, the shared relays only as a visible fallback -------------------------
//
// An iroh endpoint remembers the relays it and its peers were at, and keeps using them; taking a relay out of
// its map does not move a live connection. So a change of the relay set (the owner changed the setting, the
// fallback began or ended) builds a new endpoint with exactly the relays that may be used and closes the old
// one. That happens only on those events, never as a retry (kb NET-3).

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RelayState {
    /// No own relay set: the shared relays.
    Shared { connected: bool },
    /// The own relay is set and connected.
    Own,
    /// The own relay is set but not connected (yet); `error` says why when known.
    OwnDown { error: Option<LinkError> },
    /// The own relay is unreachable and the shared relays are in use until it is back.
    OwnFallback { error: Option<LinkError> },
}

/// How long the own relay has been seen unreachable, counting only time the policy watched (kb PHN-208, PHN-425:
/// 30 s of observed failure). A gap between two checks longer than [`WATCH_GAP`] (the process was frozen or the
/// device asleep; the checks are 500 ms apart) starts the window again instead of counting toward it: what happened
/// while nobody looked is not known, and the relay usually answers again within seconds of the process waking.
#[derive(Debug, Default, Clone)]
pub struct DownWindow {
    since: Option<Instant>,
    last: Option<Instant>,
}

/// The longest gap between two checks that still counts as watching (ten 500 ms ticks).
pub const WATCH_GAP: Duration = Duration::from_secs(5);

impl DownWindow {
    /// A check at `now` found the own relay unreachable: how long it has been, as watched.
    pub fn down(&mut self, now: Instant) -> Duration {
        let watched = self
            .last
            .is_some_and(|l| now.saturating_duration_since(l) <= WATCH_GAP);
        let since = match self.since {
            Some(s) if watched => s,
            _ => now,
        };
        self.since = Some(since);
        self.last = Some(now);
        now.saturating_duration_since(since)
    }
    /// The relay answered, or the window starts afresh (a change of network, the app back in front).
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

pub struct RelayPolicy {
    cfg: Mutex<NetConfig>,
    ep: Mutex<Endpoint>,
    state: Mutex<RelayState>,
    changed: tokio::sync::watch::Sender<u64>,
    /// Bumped whenever the endpoint is replaced.
    generation: tokio::sync::watch::Sender<u64>,
    /// With an own relay set: a second endpoint (the same identity) on the shared relays, kept only while a
    /// paired phone has not been seen with the current relay setting, so that phone still finds the Mac where
    /// it last knew it and is handed the setting there (kb NET-20). Nothing else uses it.
    beacon: Mutex<Option<Endpoint>>,
    beacon_changed: tokio::sync::watch::Sender<u64>,
    /// Set by [`RelayPolicy::recheck`], taken by the next check.
    recheck: std::sync::atomic::AtomicBool,
}

impl RelayPolicy {
    pub async fn start(cfg: NetConfig) -> LinkResult<Arc<Self>> {
        let ep = bind(&cfg).await?;
        let state = match cfg.own_relay {
            Some(_) => RelayState::OwnDown { error: None },
            None => RelayState::Shared { connected: false },
        };
        Ok(Arc::new(Self {
            cfg: Mutex::new(cfg),
            ep: Mutex::new(ep),
            state: Mutex::new(state),
            changed: tokio::sync::watch::channel(0).0,
            generation: tokio::sync::watch::channel(0).0,
            beacon: Mutex::new(None),
            beacon_changed: tokio::sync::watch::channel(0).0,
            recheck: Default::default(),
        }))
    }
    /// The current endpoint. Hold it only for one operation; it is replaced when the relay set changes.
    pub fn endpoint(&self) -> Endpoint {
        self.ep.lock().unwrap().clone()
    }
    pub fn own(&self) -> Option<RelaySpec> {
        self.cfg.lock().unwrap().own_relay.clone()
    }
    pub fn own_url(&self) -> Option<RelayUrl> {
        self.own().and_then(|o| o.parsed().ok())
    }
    pub fn shared(&self) -> Shared {
        self.cfg.lock().unwrap().shared.clone()
    }
    pub fn state(&self) -> RelayState {
        self.state.lock().unwrap().clone()
    }
    /// Fires when the relay state changes.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changed.subscribe()
    }
    /// Fires when the endpoint was replaced.
    pub fn generations(&self) -> tokio::sync::watch::Receiver<u64> {
        self.generation.subscribe()
    }
    fn set_state(&self, s: RelayState) {
        let mut cur = self.state.lock().unwrap();
        if *cur != s {
            *cur = s;
            self.changed.send_modify(|n| *n += 1);
        }
    }
    /// Every relay the endpoint may be reached at right now (what a phone should dial).
    pub fn reachable_at(&self) -> Vec<RelayUrl> {
        home_relays(&self.endpoint())
    }

    pub fn beacon(&self) -> Option<Endpoint> {
        self.beacon.lock().unwrap().clone()
    }
    /// Fires when the second endpoint was opened or closed.
    pub fn beacon_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.beacon_changed.subscribe()
    }
    /// Opens or closes the second endpoint on the shared relays. It is never open without an own relay set or
    /// while the shared relays stand in for it: the main endpoint is on the shared relays then.
    pub async fn set_beacon(&self, on: bool) -> LinkResult<()> {
        let on =
            on && self.own().is_some() && !matches!(self.state(), RelayState::OwnFallback { .. });
        if on == self.beacon.lock().unwrap().is_some() {
            return Ok(());
        }
        let old = if on {
            let mut cfg = self.cfg.lock().unwrap().clone();
            cfg.own_relay = None;
            let new = bind(&cfg).await?;
            self.beacon.lock().unwrap().replace(new)
        } else {
            self.beacon.lock().unwrap().take()
        };
        self.beacon_changed.send_modify(|n| *n += 1);
        if let Some(old) = old {
            tokio::spawn(async move { old.close().await });
        }
        Ok(())
    }

    /// A new endpoint (same identity) whose relay map is the own relay, or the shared relays.
    async fn rebuild(&self, use_own: bool) -> LinkResult<()> {
        let mut cfg = self.cfg.lock().unwrap().clone();
        if !use_own {
            cfg.own_relay = None;
            // the main endpoint goes to the shared relays itself: two of one identity must not sit there
            let old = self.beacon.lock().unwrap().take();
            if let Some(old) = old {
                self.beacon_changed.send_modify(|n| *n += 1);
                old.close().await;
            }
        }
        let new = bind(&cfg).await?;
        let old = std::mem::replace(&mut *self.ep.lock().unwrap(), new);
        self.generation.send_modify(|n| *n += 1);
        tokio::spawn(async move { old.close().await });
        Ok(())
    }

    /// A second endpoint of the same identity, not yet the one in use: new sockets (new local ports), new address
    /// discovery, its own relay connection. The phone connects with it while the session goes on over the old one,
    /// then `install`s it and closes the old one, or closes it when it did not get through (kb NET-25). A relay
    /// that sees two connections of one identity sends to the newer one and goes back to the older when the newer
    /// closes (iroh-relay 1.4.1 `server/clients.rs`).
    pub async fn spare(&self) -> LinkResult<Endpoint> {
        let mut cfg = self.cfg.lock().unwrap().clone();
        if matches!(self.state(), RelayState::OwnFallback { .. }) {
            cfg.own_relay = None;
        }
        bind(&cfg).await
    }
    /// Makes `new` the endpoint in use and hands back the one it replaces (the caller closes it).
    pub fn install(&self, new: Endpoint) -> Endpoint {
        let old = std::mem::replace(&mut *self.ep.lock().unwrap(), new);
        self.generation.send_modify(|n| *n += 1);
        old
    }

    /// A test switch: with it on this side has no IP transports at all, so every byte crosses a relay even when a
    /// direct path would work (to watch a relay carry a transfer on the same Wi-Fi). Connections drop and redial.
    pub async fn set_force_relay(&self, on: bool) -> LinkResult<()> {
        if self.cfg.lock().unwrap().force_relay == on {
            return Ok(());
        }
        self.cfg.lock().unwrap().force_relay = on;
        let use_own = !matches!(self.state(), RelayState::OwnFallback { .. });
        self.rebuild(use_own).await
    }
    pub fn force_relay(&self) -> bool {
        self.cfg.lock().unwrap().force_relay
    }
    /// "Always use my relay" switched on or off (kb PHN-219, PHN-453). The endpoint is rebuilt only when it
    /// changes what the endpoint may use: an own relay is set and is not standing aside for the public relays.
    pub async fn set_relay_only(&self, on: bool) -> LinkResult<()> {
        let (was, own) = {
            let cfg = self.cfg.lock().unwrap();
            (cfg.relay_only, cfg.own_relay.is_some())
        };
        if was == on {
            return Ok(());
        }
        self.cfg.lock().unwrap().relay_only = on;
        if own && !matches!(self.state(), RelayState::OwnFallback { .. }) {
            self.rebuild(true).await?;
        }
        Ok(())
    }
    pub fn relay_only(&self) -> bool {
        self.cfg.lock().unwrap().relay_only
    }

    /// Switch the own relay on, to another one, or off.
    pub async fn set_own(&self, new: Option<RelaySpec>) -> LinkResult<()> {
        if self.own() == new {
            return Ok(());
        }
        if let Some(n) = &new {
            n.parsed()?;
        }
        self.cfg.lock().unwrap().own_relay = new.clone();
        self.rebuild(true).await?;
        self.set_state(match new {
            Some(_) => RelayState::OwnDown { error: None },
            None => RelayState::Shared { connected: false },
        });
        Ok(())
    }

    /// The phone's network changed or the app came back to the front: the next check (within 500 ms) probes the own
    /// relay at once when the shared relays stand in for it, so a relay that answers on the new network is back in
    /// seconds rather than at the next `own_relay_probe_s`; otherwise the window of unreachability starts afresh,
    /// since what was seen on the old network says nothing about the new one (kb PHN-208, PHN-425).
    pub fn recheck(&self) {
        self.recheck
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Watches the home relay and applies the fallback rule. The values are read on every turn so a settings change
    /// applies at once: (fall back after, probe every, fallback allowed, always use my relay).
    pub async fn run(
        self: Arc<Self>,
        limits: impl Fn() -> (Duration, Duration, bool, bool) + Send + 'static,
        stop: impl Fn() -> bool + Send + 'static,
    ) {
        let mut down = DownWindow::default();
        let mut last_probe = Instant::now();
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if stop() {
                return;
            }
            let recheck = self
                .recheck
                .swap(false, std::sync::atomic::Ordering::SeqCst);
            if recheck {
                down.clear();
            }
            let (fallback_after, probe_every, allow, relay_only) = limits();
            let _ = self.set_relay_only(relay_only).await;
            let ep = self.endpoint();
            let Some(own) = self.own() else {
                self.set_state(RelayState::Shared {
                    connected: !home_relays(&ep).is_empty(),
                });
                down.clear();
                continue;
            };
            let Ok(own_url) = own.parsed() else { continue };
            if let RelayState::OwnFallback { .. } = self.state() {
                // the own relay is tried again by a plain test; when it answers, the shared relays go
                if recheck || last_probe.elapsed() >= probe_every {
                    last_probe = Instant::now();
                    if crate::relaytest::test(&own, ep.secret_key().clone())
                        .await
                        .is_ok()
                        && self.rebuild(true).await.is_ok()
                    {
                        self.set_state(RelayState::OwnDown { error: None });
                        down.clear();
                    }
                }
                continue;
            }
            if home_relays(&ep).contains(&own_url) {
                self.set_state(RelayState::Own);
                down.clear();
                continue;
            }
            let down_for = down.down(Instant::now());
            let error = home_problem(&ep).map(|(denied, why)| {
                if denied {
                    LinkError::new(errors::OWN_RELAY_ACCESS_DENIED, format!("Your relay refused this device ({why})."))
                        .allow("Check the relay's access token in FERB's settings")
                } else {
                    LinkError::new(errors::OWN_RELAY_UNREACHABLE, format!("Your relay {} cannot be reached ({why}).", own.url))
                        .allow("Check that the relay server is running; FERB's Test button says what is wrong")
                }
            });
            if allow && down_for >= fallback_after {
                if self.rebuild(false).await.is_ok() {
                    last_probe = Instant::now();
                    self.set_state(RelayState::OwnFallback { error });
                }
            } else {
                self.set_state(RelayState::OwnDown { error });
            }
        }
    }
}

// ---- why a connection is not direct: only what iroh's own network report observed ---------------------------

/// What one side's network looked like to iroh's probes, and what that side offers the other for a direct
/// connection. `known` is false until a report exists. The last three are additions of feature level 2: a side
/// from before says none of them (`None`, 0).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NetFacts {
    pub known: bool,
    pub udp_v4: bool,
    pub udp_v6: bool,
    pub public_v4: bool,
    pub public_v6: bool,
    /// The public port differs per destination (solid when true; unknown with a single relay).
    pub varies: Option<bool>,
    pub captive: Option<bool>,
    /// Among the addresses this side offers the other, one that can be reached from outside its own network:
    /// its public IPv4 address as a relay saw it or as the router mapped it, or a global IPv6 address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offers_public: Option<bool>,
    /// The kinds of address offered, never the addresses: `lan`, `public-v4`, `carrier-v4`, `global-v6`, `local-v6`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub offers: Vec<String>,
    /// How many times this side has started a round of trying a direct path (the side that dialed starts them).
    #[serde(default)]
    pub attempts: u64,
}
pub fn facts(ep: &Endpoint) -> NetFacts {
    let mut kinds: Vec<String> = ep
        .addr()
        .ip_addrs()
        .map(|a| match a.ip().to_canonical() {
            std::net::IpAddr::V4(v) if v.is_private() || v.is_loopback() || v.is_link_local() => {
                "lan"
            }
            std::net::IpAddr::V4(v) if v.octets()[0] == 100 && (v.octets()[1] & 0xc0) == 64 => {
                "carrier-v4"
            }
            std::net::IpAddr::V4(_) => "public-v4",
            std::net::IpAddr::V6(v) if (v.segments()[0] & 0xe000) == 0x2000 => "global-v6",
            std::net::IpAddr::V6(_) => "local-v6",
        })
        .map(str::to_string)
        .collect();
    kinds.sort();
    kinds.dedup();
    let offers_public = Some(kinds.iter().any(|k| k == "public-v4" || k == "global-v6"));
    let attempts = ep.metrics().socket.holepunch_attempts.get();
    match ep.net_report().get() {
        Some(r) => {
            // Whether the public port differs per destination is only learned by a report that asked more than
            // one relay: the full ones (at the start, after a change of network, every five minutes). The
            // reports between them ask the one connection that is kept (kb NET-22) and say nothing of it; the
            // last answer stands until a full report gives a new one.
            static VARIES: Mutex<Option<std::collections::HashMap<EndpointId, bool>>> =
                Mutex::new(None);
            let varies = {
                let mut all = VARIES.lock().unwrap();
                let all = all.get_or_insert_with(Default::default);
                match r.mapping_varies_by_dest_ipv4 {
                    Some(v) => {
                        all.insert(ep.id(), v);
                        Some(v)
                    }
                    None if r.udp_v4 => all.get(&ep.id()).copied(),
                    None => None,
                }
            };
            NetFacts {
                known: true,
                udp_v4: r.udp_v4,
                udp_v6: r.udp_v6,
                public_v4: r.global_v4.is_some(),
                public_v6: r.global_v6.is_some(),
                varies,
                captive: r.captive_portal,
                offers_public,
                offers: kinds,
                attempts,
            }
        }
        None => NetFacts {
            offers_public,
            offers: kinds,
            attempts,
            ..Default::default()
        },
    }
}

/// One or two plain sentences after the tier line: what was observed on both sides, never a guess at a cause
/// nobody saw. None on a direct path. It does not say whether a direct connection is still being tried: that
/// is the caller's own line (`direct.trying`).
pub fn why_not_direct(tier: Tier, mac: &NetFacts, phone: Option<&NetFacts>) -> Option<String> {
    if tier == Tier::Direct {
        return None;
    }
    let own = if tier == Tier::OwnRelay {
        "No limits apply here. "
    } else {
        ""
    };
    let Some(phone) = phone.filter(|p| p.known && mac.known) else {
        return Some(format!(
            "{own}Checking whether a direct connection is possible…"
        ));
    };
    let no_udp = |f: &NetFacts| !f.udp_v4 && !f.udp_v6;
    let both_v6 = mac.udp_v6 && phone.udp_v6;
    let reason = if no_udp(mac) || no_udp(phone) {
        let who = match (no_udp(mac), no_udp(phone)) {
            (true, true) => "Neither network lets",
            (true, false) => "The Mac's network does not let",
            _ => "The phone's network does not let",
        };
        let portal = if mac.captive == Some(true) || phone.captive == Some(true) {
            " If it has a sign-in page, sign in."
        } else {
            ""
        };
        format!(
            "{who} through the kind of traffic a direct connection uses (or FERB could not check it there).{portal}"
        )
    } else if phone.offers_public == Some(false) || mac.offers_public == Some(false) {
        // our own side of it: an address the other could reach was never learned, so nothing could be tried
        let who = match (
            mac.offers_public == Some(false),
            phone.offers_public == Some(false),
        ) {
            (true, true) => "Neither the Mac nor the phone has",
            (true, false) => "The Mac has not",
            _ => "The phone has not",
        };
        format!(
            "{who} learned an address it can be reached at from outside its own network yet, so there is nothing for the other side to connect to."
        )
    } else if (phone.varies == Some(true) || mac.varies == Some(true)) && !both_v6 {
        let who = if phone.varies == Some(true) {
            "The phone's network gives it"
        } else {
            "The Mac's network gives it"
        };
        format!(
            "{who} a new public port for every destination, so the other side cannot reach it directly, and the two networks do not both have IPv6 to go around that."
        )
    } else {
        let tried = match mac.attempts + phone.attempts {
            0 => "No attempt has been made yet.".to_string(),
            1 => "One attempt has been made and nothing came through.".to_string(),
            n => format!("{n} attempts have been made and nothing came through."),
        };
        format!(
            "Each side knows an address of the other and neither network shows a known obstacle. {tried} What stops the packets cannot be seen from either side."
        )
    };
    Some(format!("{own}{reason}"))
}

/// The local UDP ports of an endpoint's sockets. Only ever compared (did a start-over get new ports?), never said.
pub fn local_ports(ep: &Endpoint) -> Vec<u16> {
    let mut p: Vec<u16> = ep.bound_sockets().iter().map(|a| a.port()).collect();
    p.sort();
    p
}

pub async fn wait_home(ep: &Endpoint, d: Duration) -> bool {
    tokio::time::timeout(d, async {
        while home_relays(ep).is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Duration = Duration::from_millis(500);

    // decision: PHN-208, PHN-425
    #[test]
    fn only_watched_time_counts_toward_the_fallback() {
        let t0 = Instant::now();
        // watched without a break: 30 s of checks reach 30 s
        let mut w = DownWindow::default();
        let mut now = t0;
        let mut seen = Duration::ZERO;
        for _ in 0..=60 {
            seen = w.down(now);
            now += TICK;
        }
        assert_eq!(seen, Duration::from_secs(30));

        // seen down once, then the process was frozen for ten minutes: the first check after it starts again
        let mut w = DownWindow::default();
        assert_eq!(w.down(t0), Duration::ZERO);
        let thawed = t0 + Duration::from_secs(600);
        assert_eq!(
            w.down(thawed),
            Duration::ZERO,
            "the frozen minutes don't count"
        );
        // and the fallback comes only after 30 s more of watching
        let mut now = thawed;
        for _ in 0..59 {
            now += TICK;
            assert!(w.down(now) < Duration::from_secs(30));
        }
        assert_eq!(w.down(now + TICK), Duration::from_secs(30));

        // a gap of up to WATCH_GAP (a slow check) still counts; one past it does not
        let mut w = DownWindow::default();
        w.down(t0);
        assert_eq!(w.down(t0 + WATCH_GAP), WATCH_GAP);
        let mut w = DownWindow::default();
        w.down(t0);
        assert_eq!(w.down(t0 + WATCH_GAP + TICK), Duration::ZERO);

        // a change of network clears what was seen on the old one
        let mut w = DownWindow::default();
        w.down(t0);
        w.down(t0 + Duration::from_secs(4));
        w.clear();
        assert_eq!(w.down(t0 + Duration::from_secs(5)), Duration::ZERO);
    }
}
