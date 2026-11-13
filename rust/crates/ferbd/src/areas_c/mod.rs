//! The RPC handlers no ported area owned yet (`ferb/daemon/areas/rpc.py`'s own handlers and three of
//! `areas/chat.py`'s): the task records (`trace.get`, `verdict.set`, `recovery.*`, `reports.*`), the owner's
//! permissions and health (`doctor.check`, `setup.permissions`, `setup.permission_request`, `setup.finish`,
//! `clipboard.request`, `automation.request`, `identity.check`), FERB.app's own calls (`ui.click_through`,
//! `tasks.preview`, `questions.picture`), the delegated conversations (`conversations.list`, `conversations.change`)
//! and the `logs.subscribe` stream.
//!
//! Each handler reads the daemon's parts through [`Deps`]; what touches the Mac (TCC reads, prompts, the pasteboard,
//! the self-test) is the [`Mac`] seam, so a daemon started for a test answers the recorder's facts.

pub mod logs;
pub mod mac;
pub mod records;
pub mod setup;

use std::path::PathBuf;
use std::sync::Arc;

use ferb_core::config::Config;
use ferb_core::interfaces::{BoxFuture, IdSource};
use ferb_daemon::areas_a::State as AState;
use ferb_daemon::shared::Local;
use ferb_gateway::gateway::Gateway;
use ferb_rpc::{HandlerError, Server};
use ferb_sanitize::redact::Redactor;
use serde_json::{Map, Value};

use crate::basics::{Journal, SharedDb, Stop};

/// What touches the Mac itself (`ferb/device/permissions.py`, `selftest.py`, the pasteboard, the Mac driver's
/// click-through record). A daemon whose Mac parts are off gets [`NoMac`]; a test, the recorder's answers.
pub trait Mac: Send + Sync {
    /// `permissions.check_all()`: the daemon's four grants as macOS reports them now (never a prompt).
    fn grants(&self) -> Map<String, Value>;
    /// `permissions.request(name)`: macOS's own prompt for one requestable grant; its status afterwards.
    fn request(&self, permission: &str) -> String;
    /// `_clipboard_access`: the pasteboard's accessBehavior as a word, None when it can't be read (never contents).
    fn clipboard_access(&self) -> Option<String>;
    /// `request_access`: one read so macOS shows its paste alert now; nothing read is kept. The state afterwards.
    fn clipboard_request(&self) -> String;
    /// `permissions.automation(bundle, ask)`: Apple Events permission to that app (`ask` shows macOS's prompt).
    fn automation(&self, bundle_id: &str, ask: bool) -> String;
    /// `selftest.run_all()`: the passive self-test (the Dock's AX tree, a listen-only tap).
    fn selftest(&self) -> Vec<Value>;
    /// `_fresh_grants`: a fresh process's reading of the grants macOS caches per process.
    fn fresh_grants(&self) -> BoxFuture<'static, Map<String, Value>>;
    /// The bundle ids of the apps running now.
    fn running(&self) -> Vec<String>;
    /// `report_click_through(pid, windows)`: true when `pid` is one of FERB's own running apps (recorded then).
    fn click_through(&self, pid: i32, windows: &[i64]) -> bool;
}

/// A daemon without its Mac parts: nothing is read or asked; every grant is "unknown".
pub struct NoMac;

impl Mac for NoMac {
    fn grants(&self) -> Map<String, Value> {
        ferb_mac_hands::permissions::DAEMON_PERMISSIONS
            .iter()
            .map(|p| (p.to_string(), Value::from("unknown")))
            .collect()
    }
    fn request(&self, _permission: &str) -> String {
        "unknown".into()
    }
    fn clipboard_access(&self) -> Option<String> {
        None
    }
    fn clipboard_request(&self) -> String {
        "unknown".into()
    }
    fn automation(&self, _bundle_id: &str, _ask: bool) -> String {
        "unknown".into()
    }
    fn selftest(&self) -> Vec<Value> {
        Vec::new()
    }
    fn fresh_grants(&self) -> BoxFuture<'static, Map<String, Value>> {
        Box::pin(async { Map::new() })
    }
    fn running(&self) -> Vec<String> {
        Vec::new()
    }
    fn click_through(&self, _pid: i32, _windows: &[i64]) -> bool {
        false
    }
}

/// What this program is, for `doctor.check` and the reports (`ferb.__version__`, `platform.mac_ver()`,
/// `sys.executable`, `code_hash`).
#[derive(Clone)]
pub struct Facts {
    pub version: String,
    pub macos: String,
    pub executable: String,
    pub code_hash: String,
    /// `token_method()`: FERB's pinned o200k copy or the byte ceiling.
    pub tokenizer: String,
}

impl Facts {
    /// This program's own facts.
    pub fn real() -> Facts {
        let executable = std::env::current_exe()
            .ok()
            .and_then(|p| std::fs::canonicalize(p).ok())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        Facts {
            version: crate::compose::VERSION.into(),
            macos: macos_version(),
            code_hash: code_hash(&executable),
            executable,
            // o200k_base is built into this program: always available
            tokenizer: ferb_context::tokens::Tokenizer::O200k.method().into(),
        }
    }
}

/// `platform.mac_ver()[0]`: the ProductVersion of SystemVersion.plist, "" when it can't be read.
pub fn macos_version() -> String {
    let Ok(text) = std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist")
    else {
        return String::new();
    };
    let Some(at) = text.find("<key>ProductVersion</key>") else {
        return String::new();
    };
    let rest = &text[at..];
    let (Some(a), Some(b)) = (rest.find("<string>"), rest.find("</string>")) else {
        return String::new();
    };
    rest.get(a + 8..b).unwrap_or("").trim().to_string()
}

/// `code_hash`: the first 16 hex of the SHA-256 of this program's own bytes (the Rust build has one executable where
/// Python hashed its package folder).
pub fn code_hash(executable: &str) -> String {
    use sha2::Digest;
    match std::fs::read(executable) {
        Ok(bytes) => hex::encode(sha2::Sha256::digest(&bytes))[..16].to_string(),
        Err(_) => String::new(),
    }
}

/// `doctor.check`'s containment row and setup's, read now (`_setup_containment` and the live check).
pub type Containment = Arc<dyn Fn(bool) -> Map<String, Value> + Send + Sync>;
/// `_hearing_status(probe)`.
pub type Hearing = Arc<dyn Fn(bool) -> BoxFuture<'static, Value> + Send + Sync>;
/// `_sense_status()`.
pub type SenseStatus = Arc<dyn Fn() -> Result<Value, HandlerError> + Send + Sync>;
/// The protection lists' answer for a path and an access: the rule's words, None when open.
pub type Closed = Arc<dyn Fn(&str, &str) -> Option<String> + Send + Sync>;
/// FERB's own browser session checked through the extension.
pub type SessionCheck = Arc<dyn Fn() -> BoxFuture<'static, Result<bool, String>> + Send + Sync>;

/// Everything these handlers read.
#[derive(Clone)]
pub struct Deps {
    pub home: PathBuf,
    pub user_home: PathBuf,
    pub app_bundle: Option<PathBuf>,
    pub config: Arc<Config>,
    pub redactor: Arc<Redactor>,
    pub journal: Arc<Journal>,
    pub db: SharedDb,
    pub clock: Arc<dyn ferb_clock::clock::ZonedClock>,
    pub ids: Arc<dyn IdSource>,
    pub stop: Arc<Stop>,
    /// Group A's thread (the learning store a verdict's card is captured on).
    pub a: Local<AState>,
    pub gateway: Arc<Gateway>,
    pub runtimes: ferb_daemon::areas_b::runtimes::Runtimes,
    /// The browser area's context (FERB's own account, the pairings).
    pub browsers: Option<ferb_daemon::areas_b::browsers::Cx>,
    /// The browsers' versions for a report (`_report_facts`' extensions).
    pub extensions: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
    pub mac: Arc<dyn Mac>,
    pub facts: Facts,
    pub containment: Containment,
    pub hearing: Hearing,
    pub sense_status: SenseStatus,
    /// The protection lists' answer for a path and an access ("read", "write"): the rule's words, None when open.
    pub closed: Closed,
    /// `KeychainSecretStore.present(ref)`: one of FERB's own secrets is in the Keychain (no value read).
    pub secret_present: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    /// `ferb_identity.session_ok()` through the extension; None when FERB's own browser isn't attached.
    pub session_ok: Option<SessionCheck>,
    /// The in-process bus `logs.subscribe` reads (every published event, redacted once).
    pub bus: ferb_core::bus::Bus,
    /// `reports.RELAY_URL`: the developer's report relay when config names none ("" in a daemon started for a
    /// test: nothing reaches the deployed relay, as conftest's `no_report_relay`).
    pub relay_default: String,
    /// The snapshot facts for `doctor.check`: (snapshots on, the last failure).
    pub snapshots: Arc<dyn Fn() -> (bool, Value) + Send + Sync>,
}

/// Registers every method of this group.
pub fn register(server: &Server, deps: &Deps) {
    records::register(server, deps);
    setup::register(server, deps);
    logs::register(server, deps);
}

/// The redactor's text pass.
pub(crate) fn redact_text(d: &Deps, t: &str) -> String {
    d.redactor.text(t)
}

pub(crate) fn ferb_error(message: impl Into<String>) -> HandlerError {
    HandlerError::refusal("FerbError", message)
}

pub(crate) fn ok(v: Value) -> Result<Map<String, Value>, HandlerError> {
    Ok(crate::basics::object(v))
}
