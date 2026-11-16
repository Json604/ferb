//! Test world: local relays, the stand-in daemon, a bridge and phones, all on 127.0.0.1.
#![allow(dead_code)]
mod standin;
use ferb_link::{
    bridge::{Bridge, BridgeConfig},
    client::{Phone, PhoneConfig, PhoneState},
    errors::LinkError,
    net::{RelaySpec, Shared},
};
use iroh_relay::server::{
    Access, AccessControl, ClientRateLimit, ClientRequest, Limits, RelayConfig, Server,
    ServerConfig,
};
use p256::ecdsa::{SigningKey, signature::Signer};
use p256::pkcs8::EncodePublicKey;
use serde_json::{Value, json};
pub use standin::Standin;
use std::{
    net::Ipv4Addr,
    num::NonZeroU32,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::mpsc::UnboundedReceiver;

pub fn b64(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}

#[derive(Debug)]
struct Token(String);
impl AccessControl for Token {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        match request.auth_token() {
            Some(t) if t == self.0 => Access::Allow,
            _ => Access::Deny { reason: None },
        }
    }
}

pub struct Relay {
    server: Option<Server>,
    pub port: u16,
    token: Option<String>,
    limit: Option<u32>,
}
impl Relay {
    fn config(port: u16, token: &Option<String>, limit: Option<u32>) -> ServerConfig {
        let mut relay = RelayConfig::new((Ipv4Addr::LOCALHOST, port));
        if let Some(t) = token {
            relay.access = Arc::new(Token(t.clone()));
        }
        if let Some(rate) = limit.and_then(NonZeroU32::new) {
            let mut l = Limits::default();
            let mut rx = ClientRateLimit::new(rate);
            rx.max_burst_bytes = NonZeroU32::new(rate.get() * 2);
            l.client_rx = Some(rx);
            relay.limits = l;
        }
        let mut c = ServerConfig::default();
        c.relay = Some(relay);
        c
    }
    pub async fn start(token: Option<&str>, limit: Option<u32>) -> Self {
        let token = token.map(str::to_string);
        let server = Server::spawn(Self::config(0, &token, limit))
            .await
            .expect("relay");
        let port = server.http_addr().unwrap().port();
        Self {
            server: Some(server),
            port,
            token,
            limit,
        }
    }
    pub fn url(&self) -> String {
        format!("http://localhost:{}", self.port)
    }
    pub fn spec(&self) -> RelaySpec {
        RelaySpec::new(self.url(), self.token.clone())
    }
    pub async fn stop(&mut self) {
        if let Some(s) = self.server.take() {
            s.shutdown().await.ok();
        }
    }
    pub async fn restart(&mut self) {
        for _ in 0..50 {
            if let Ok(s) = Server::spawn(Self::config(self.port, &self.token, self.limit)).await {
                self.server = Some(s);
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("relay did not restart");
    }
}

pub fn tmp(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("{name}{:04x}", rand::random::<u16>()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// What the test phone remembers from its events.
#[derive(Default)]
pub struct Seen {
    pub events: Vec<Value>,
    pub state: Option<PhoneState>,
}
pub struct TestPhone {
    pub phone: Phone,
    pub seen: Arc<Mutex<Seen>>,
    pub unlock: SigningKey,
    /// The fingerprint-only key (a software key in tests): key places and sensitive changes are signed with it.
    pub strong: SigningKey,
}
impl TestPhone {
    pub async fn start(
        name: &str,
        shared: &Shared,
        force_relay: bool,
        state: Option<PhoneState>,
        lock: bool,
    ) -> Self {
        Self::start_with(name, shared, force_relay, state, lock, |_| {}).await
    }
    pub async fn start_with(
        name: &str,
        shared: &Shared,
        force_relay: bool,
        state: Option<PhoneState>,
        lock: bool,
        tweak: impl FnOnce(&mut PhoneConfig),
    ) -> Self {
        let unlock = SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let unlock_pub = lock.then(|| {
            b64(unlock
                .verifying_key()
                .to_public_key_der()
                .unwrap()
                .as_bytes())
        });
        // the phone's device key (in tests a software key): each connection proves by itself that it is this phone
        let device = SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let device_pub = lock.then(|| {
            b64(device
                .verifying_key()
                .to_public_key_der()
                .unwrap()
                .as_bytes())
        });
        let sign: ferb_link::client::DeviceSign = Arc::new(move |payload: &str| {
            let sig: p256::ecdsa::Signature = device.sign(payload.as_bytes());
            Some(b64(sig.to_der().as_bytes()))
        });
        let strong = SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let strong_pub = lock.then(|| {
            b64(strong
                .verifying_key()
                .to_public_key_der()
                .unwrap()
                .as_bytes())
        });
        let mut cfg = PhoneConfig {
            portmapper: false,
            loopback_only: true,
            state,
            name: name.into(),
            app: "test".into(),
            unlock_pub,
            device_pub,
            strong_pub,
            device_sign: lock.then_some(sign),
            shared: shared.clone(),
            force_relay,
            ..Default::default()
        };
        tweak(&mut cfg);
        let (phone, rx) = Phone::start(cfg).await.expect("phone");
        let seen = Arc::new(Mutex::new(Seen::default()));
        tokio::spawn(collect(rx, seen.clone()));
        Self {
            phone,
            seen,
            unlock,
            strong,
        }
    }
    pub fn phase(&self) -> String {
        self.phone.phase()["phase"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
    pub fn tier(&self) -> String {
        self.phone.phase()["tier"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
    pub fn stop_code(&self) -> String {
        self.phone.phase()["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
    pub async fn wait_phase(&self, want: &str, secs: u64) -> bool {
        wait(secs, || self.phase() == want).await
    }
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, LinkError> {
        self.phone.call(method, params, None, |_| {}).await
    }
    /// The newest status the Mac pushed.
    pub fn status(&self) -> Value {
        self.seen
            .lock()
            .unwrap()
            .events
            .iter()
            .rev()
            .find(|e| e["type"] == "status")
            .map(|e| e["status"].clone())
            .unwrap_or(Value::Null)
    }
    pub fn approval(&self, id: &str) -> Option<Value> {
        self.status()["approvals"]
            .as_array()
            .and_then(|a| a.iter().find(|x| x["approval_id"] == id).cloned())
    }
    /// An unlock proof for this answer, signed by the phone's (software, in tests) fingerprint-only key, as the
    /// app does for a Yes to one of FERB's checks where the phone has a fingerprint.
    pub fn proof(&self, approval_id: &str, challenge: &str, method: &str, params: &Value) -> Value {
        self.proof_for(&Phone::answer_payload(
            approval_id,
            challenge,
            method,
            params,
        ))
    }
    /// The same signed with the lock key, which the PIN can use too.
    pub fn pin_proof(
        &self,
        approval_id: &str,
        challenge: &str,
        method: &str,
        params: &Value,
    ) -> Value {
        self.pin_proof_for(&Phone::answer_payload(
            approval_id,
            challenge,
            method,
            params,
        ))
    }
    /// Unlock the phone towards the Mac (a signed proof over a fresh challenge), as the app does after the prompt.
    pub async fn unlock(&self) -> Result<Value, LinkError> {
        let ch = self.phone.challenge().await?;
        let proof = self.proof_for(&Phone::call_payload(&ch, "link.unlock", &json!({})));
        self.phone
            .call("link.unlock", json!({}), Some(proof), |_| {})
            .await
    }
    /// A sensitive call: a fresh challenge, signed for exactly this call.
    pub async fn sensitive(&self, method: &str, params: Value) -> Result<Value, LinkError> {
        let ch = self.phone.challenge().await?;
        let proof = self.proof_for(&Phone::call_payload(&ch, method, &params));
        self.phone.call(method, params, Some(proof), |_| {}).await
    }
    /// A proof for a call (a sensitive change, a key place, the unlock): signed with the fingerprint-only key, as
    /// the app does where the phone has a fingerprint.
    pub fn proof_for(&self, payload: &str) -> Value {
        let sig: p256::ecdsa::Signature = self.strong.sign(payload.as_bytes());
        json!({"payload": payload, "sig": b64(sig.to_der().as_bytes())})
    }
    /// The same signed with the lock key, which the PIN can use too.
    pub fn pin_proof_for(&self, payload: &str) -> Value {
        let sig: p256::ecdsa::Signature = self.unlock.sign(payload.as_bytes());
        json!({"payload": payload, "sig": b64(sig.to_der().as_bytes())})
    }
    /// The unlock proof for a hidden answer: the value enters the signed text only as its SHA-256.
    pub fn sealed_proof(&self, approval_id: &str, challenge: &str, text: &str) -> Value {
        let payload = Phone::sealed_payload(approval_id, challenge, text);
        let sig: p256::ecdsa::Signature = self.unlock.sign(payload.as_bytes());
        json!({"payload": payload, "sig": b64(sig.to_der().as_bytes())})
    }
    pub fn saved(&self) -> Option<PhoneState> {
        self.seen.lock().unwrap().state.clone()
    }
}
async fn collect(mut rx: UnboundedReceiver<Value>, seen: Arc<Mutex<Seen>>) {
    while let Some(e) = rx.recv().await {
        let mut s = seen.lock().unwrap();
        if e["type"] == "persist" {
            s.state = serde_json::from_value(e["state"].clone()).ok();
        }
        s.events.push(e);
    }
}

pub async fn wait(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    f()
}

pub struct World {
    pub dir: PathBuf,
    pub standin: Standin,
    pub bridge: Bridge,
    pub shared: Shared,
    pub force_relay: bool,
}
impl World {
    /// `shared`: the relays standing in for n0's (or n0's own for the n0 test); `own`: the owner's relay.
    pub async fn start(shared: Shared, own: Option<RelaySpec>, force_relay: bool) -> Self {
        let dir = tmp("w");
        let standin = Standin::start(&dir).await;
        if let Some(own) = &own {
            std::fs::create_dir_all(dir.join("mac")).unwrap();
            std::fs::write(
                dir.join("mac/link.json"),
                json!({"own_relay": own}).to_string(),
            )
            .unwrap();
        }
        let bridge = Bridge::start(BridgeConfig {
            portmapper: false,
            loopback_only: true,
            state_dir: dir.join("mac"),
            daemon_socket: standin.socket.clone(),
            shared: shared.clone(),
            roots: None,
            force_relay,
            keys: ferb_link::secrets::Keys::Files,
            accept_state: false,
        })
        .await
        .expect("bridge");
        assert!(
            ferb_link::net::wait_home(&bridge.endpoint(), Duration::from_secs(30)).await,
            "the Mac found no relay"
        );
        Self {
            dir,
            standin,
            bridge,
            shared,
            force_relay,
        }
    }
    pub async fn phone(&self, name: &str) -> TestPhone {
        TestPhone::start(name, &self.shared, self.force_relay, None, true).await
    }
    /// Pair a phone the proper way: QR, proof, the owner's Yes.
    pub async fn pair(&self, p: &TestPhone) {
        self.pair_with(p, "code").await
    }
    /// The same, with the short code the QR shows (`code`) or the link older apps read (`url`).
    pub async fn pair_with(&self, p: &TestPhone, form: &str) {
        let mut admin = self.bridge.admin_events();
        let qr = self.bridge.pair_start().expect("pair_start");
        assert!(
            qr["code"].as_str().unwrap().starts_with("FERB-")
                && qr["url"].as_str().unwrap().starts_with("ferblink://"),
            "{qr}"
        );
        p.phone.pair(qr[form].as_str().unwrap()).unwrap();
        let asked = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if admin.recv().await.unwrap()["event"] == "pair_request" {
                    return;
                }
            }
        })
        .await;
        assert!(
            asked.is_ok(),
            "the Mac never asked; phone says {}",
            p.phone.phase()
        );
        // both screens show the same short code while the Mac asks
        let mac_code = self.bridge.status()["pairing"]["code"]
            .as_str()
            .unwrap_or("mac?")
            .to_string();
        assert!(
            wait(5, || p.phone.phase()["code"].as_str()
                == Some(mac_code.as_str()))
            .await,
            "codes differ: Mac {mac_code}, phone {}",
            p.phone.phase()
        );
        assert_eq!(mac_code.len(), 7);
        self.decide(true).unwrap();
        assert!(
            p.wait_phase("online", 20).await,
            "phone not online: {}",
            p.phone.phase()
        );
    }
    /// The owner's answer in FERB.app to the phone waiting (the daemon's `phone.pair_decide`); an error when no phone
    /// waits.
    pub fn decide(&self, yes: bool) -> Result<bool, String> {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(paired) = self.standin.pair_decide(yes) {
                return Ok(paired);
            }
            if Instant::now() > end {
                return Err("No phone is waiting for your answer.".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    /// The owner removes a phone in FERB.app (the daemon's `phone.remove`); returns once the bridge has dropped it.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        if !self.standin.remove(id) {
            return Err("No such phone.".into());
        }
        let end = Instant::now() + Duration::from_secs(5);
        while self.bridge.devices().iter().any(|d| d.id == id) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
    /// The owner renames a phone in FERB.app (the daemon's `phone.rename`); returns once the bridge has the name.
    pub fn rename(&self, id: &str, name: &str) -> Result<(), String> {
        if !self.standin.rename(id, name) {
            return Err("No such phone.".into());
        }
        let clean: String = name.chars().filter(|c| !c.is_control()).take(40).collect();
        let end = Instant::now() + Duration::from_secs(5);
        while !self
            .bridge
            .devices()
            .iter()
            .any(|d| d.id == id && d.name == clean.trim())
            && Instant::now() < end
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
    /// The link's limits, and the daemon's own numbers among them (the unlock window, the challenge's life, the upload
    /// floor: the lab's bridge handed them to the daemon in `phone.setup`; since S4 they are the daemon's).
    pub fn limits_set(&self, v: &Value) -> Result<ferb_link::limits::Limits, LinkError> {
        self.standin.configure(v);
        self.bridge.limits_set(v)
    }
    /// A paired phone as it is right after the owner opened the app: unlocked towards the Mac (the Mac gives a
    /// connection nothing of FERB's before that; a test of the locked state calls `link.lock` first).
    pub async fn paired_phone(&self, name: &str) -> TestPhone {
        let p = self.phone(name).await;
        self.pair(&p).await;
        p.unlock().await.expect("unlock");
        p
    }
}

pub fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if v.is_empty() {
        f64::NAN
    } else {
        v[((v.len() as f64 - 1.0) * p).round() as usize]
    }
}
pub fn code<T: std::fmt::Debug>(r: Result<T, LinkError>) -> String {
    match r {
        Ok(v) => format!("OK {v:?}"),
        Err(e) => e.code,
    }
}
