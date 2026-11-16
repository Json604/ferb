//! The app's side for the tests: the seams a Kotlin or Swift app supplies (clock, ids, listener, signer, transport),
//! written in Rust against this crate's exported traits, and ferb-mobile's fake Mac (its tests' `common`, shared by
//! path) behind the exported `Transport`.
#![allow(dead_code)]

// another crate's files: never formatted from here (its own `cargo fmt` and edition own them)
#[rustfmt::skip]
#[path = "../../../ferb-mobile/tests/common/mod.rs"]
pub mod shared;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ferb_mobile::transport::{LinkFailure, Transport as _, WatchHandle};
use ferb_mobile_ffi::records::Change;
use ferb_mobile_ffi::{
    Clock, FerbError, Ids, Listener, Mobile, MobileConfig, Signer, Transport, TransportError,
    WatchSink,
};

pub use shared::mac::FakeMac;

pub const KEY: [u8; 32] = [7u8; 32];

pub struct FixedClock;
impl Clock for FixedClock {
    fn now_iso(&self) -> String {
        "2026-01-02T10:00:00+00:00".into()
    }
}

#[derive(Default)]
pub struct Counted(AtomicU64);
impl Ids for Counted {
    fn new_id(&self) -> String {
        format!("sub-{}", self.0.fetch_add(1, Ordering::SeqCst) + 1)
    }
}

#[derive(Default)]
pub struct Heard(pub Mutex<Vec<Change>>);
impl Listener for Heard {
    fn changed(&self, change: Change) {
        self.0.lock().unwrap().push(change);
    }
}

/// A listener whose every call panics (an app's bug in its callback).
pub struct Throws;
impl Listener for Throws {
    fn changed(&self, _change: Change) {
        panic!("the app's listener failed");
    }
}

/// Signs with a fixed signature and records what it was asked to sign.
#[derive(Default)]
pub struct Signs(pub Mutex<Vec<String>>);
impl Signer for Signs {
    fn sign(&self, payload: String) -> Option<String> {
        self.0.lock().unwrap().push(payload);
        Some("SIG".into())
    }
}

/// The owner cancelled the fingerprint prompt.
pub struct Cancels;
impl Signer for Cancels {
    fn sign(&self, _payload: String) -> Option<String> {
        None
    }
}

fn te(f: LinkFailure) -> TransportError {
    TransportError::Failed {
        code: f.code,
        detail: f.message,
        allow: f.allow,
        retry_after_s: f.retry_after_s,
    }
}

/// The fake Mac as an app-supplied transport: what a Kotlin test's fake would be.
pub struct ForeignMac {
    pub mac: FakeMac,
    watches: Mutex<HashMap<u64, Box<dyn WatchHandle>>>,
    next: AtomicU64,
}

impl ForeignMac {
    pub fn new(mac: FakeMac) -> Arc<ForeignMac> {
        Arc::new(ForeignMac {
            mac,
            watches: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
        })
    }
}

struct Forward(Arc<WatchSink>);
impl ferb_mobile::WatchSink for Forward {
    fn frame(&self, frame_json: String) {
        self.0.frame(frame_json);
    }
    fn ended(&self, failure: Option<LinkFailure>) {
        let (code, detail) = match failure {
            Some(f) => (Some(f.code), Some(f.message)),
            None => (None, None),
        };
        self.0.ended(code, detail);
    }
}

impl Transport for ForeignMac {
    fn call(
        &self,
        method: String,
        params_json: String,
        proof_json: Option<String>,
    ) -> Result<String, TransportError> {
        self.mac
            .call(&method, &params_json, proof_json.as_deref())
            .map_err(te)
    }
    fn send(&self, params_json: String) -> Result<String, TransportError> {
        self.mac.send(&params_json).map_err(te)
    }
    fn watch(&self, params_json: String, sink: Arc<WatchSink>) -> Result<u64, TransportError> {
        let handle = self
            .mac
            .watch(&params_json, Arc::new(Forward(sink)))
            .map_err(te)?;
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        self.watches.lock().unwrap().insert(id, handle);
        Ok(id)
    }
    fn close_watch(&self, watch_id: u64) {
        if let Some(h) = self.watches.lock().unwrap().remove(&watch_id) {
            h.close();
        }
    }
}

pub fn config(client: &str) -> MobileConfig {
    MobileConfig {
        client_id: client.into(),
        utc_offset_s: 0,
        page_limit: None,
        max_bytes: None,
        piece_chars: None,
    }
}

pub fn open(
    dir: &std::path::Path,
    cfg: MobileConfig,
    heard: Arc<Heard>,
) -> Result<Arc<Mobile>, FerbError> {
    Mobile::open(
        dir.join("ferb-phone.db").to_string_lossy().into_owned(),
        KEY.to_vec(),
        cfg,
        Arc::new(FixedClock),
        Arc::new(Counted::default()),
        heard,
    )
}

pub fn online(tier: &str) -> String {
    format!(r#"{{"type":"state","phase":"online","tier":"{tier}"}}"#)
}

pub const OFFLINE: &str = r#"{"type":"state","phase":"connecting"}"#;
