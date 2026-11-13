//! Connection roles, the method table and scoped run tokens. Port of `ferb/daemon/auth.py`.
//!
//! - cli: proven by socket ownership: the peer's uid must be the owner's.
//! - mcp_shim: proven by a run token (scoped to one worker, expiring, revocable; only its hash is kept).
//! - native_host: the relay a browser launches for the FERB extension. The socket peer must be the owner's uid
//!   and the caller origin one of FERB's extension ids; after that the connection is a message channel whose
//!   every message is authenticated by pairing, not by method lists.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

/// auth.py:21-24.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Cli,
    McpShim,
    NativeHost,
}

impl Role {
    pub const ALL: [Role; 3] = [Role::Cli, Role::McpShim, Role::NativeHost];

    /// The role's name on the wire (`Role.value`).
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Cli => "cli",
            Role::McpShim => "mcp_shim",
            Role::NativeHost => "native_host",
        }
    }

    pub fn from_wire(name: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|role| role.as_str() == name)
    }
}

/// One RPC method: who may call it, and whether only FERB.app's own process may (auth.py:61-69).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Method {
    pub name: &'static str,
    pub roles: &'static [Role],
    pub app_only: bool,
}

const fn m(name: &'static str, roles: &'static [Role], app_only: bool) -> Method {
    Method {
        name,
        roles,
        app_only,
    }
}

/// The fixed method list (auth.py:28-50 `METHODS`, auth.py:61-69 `APP_ONLY`), sorted by name. New RPC methods
/// declare their roles here, never per connection. The native host has none: after its hello the connection
/// is the extension's message channel. Checked against `rust/fixtures/rpc/table.json`, which Python wrote.
pub const METHODS: &[Method] = &[
    m("about.get", &[Role::Cli], false),
    m("about.guide", &[Role::Cli], true),
    m("attention.ack", &[Role::Cli], false),
    m("automation.request", &[Role::Cli], false),
    m("browser.show_handoff", &[Role::Cli], false),
    m("browsers.installed", &[Role::Cli], false),
    m("browsers.list", &[Role::Cli], false),
    m("browsers.pair", &[Role::Cli], false),
    m("browsers.unpair", &[Role::Cli], false),
    m("chat.answer", &[Role::Cli], false),
    m("chat.approve", &[Role::Cli], false),
    m("chat.attach", &[Role::Cli], false),
    m("chat.correct", &[Role::Cli], false),
    m("chat.history.days", &[Role::Cli], true),
    m("chat.history.import", &[Role::Cli], true),
    m("chat.history.page", &[Role::Cli], true),
    m("chat.history.put", &[Role::Cli], true),
    m("chat.send", &[Role::Cli], false),
    m("chat.status", &[Role::Cli], false),
    m("clipboard.request", &[Role::Cli], false),
    m("conversations.change", &[Role::Cli], false),
    m("conversations.list", &[Role::Cli], false),
    m("doctor.check", &[Role::Cli], false),
    m("identity.check", &[Role::Cli], false),
    m("identity.choose", &[Role::Cli], true),
    m("identity.find", &[Role::Cli], true),
    m("identity.pair", &[Role::Cli], true),
    m("intro.preview", &[Role::Cli], true),
    m("intro.suggest", &[Role::Cli], true),
    m("jev.enable", &[Role::Cli], false),
    m("jev.reload", &[Role::Cli], false),
    m("jev.stats", &[Role::Cli], false),
    m("keys.delete", &[Role::Cli], true),
    m("keys.store", &[Role::Cli], true),
    m("logs.subscribe", &[Role::Cli], false),
    m("memory.add", &[Role::Cli], false),
    m("memory.edit", &[Role::Cli], false),
    m("memory.forget", &[Role::Cli], false),
    m("memory.import.cancel", &[Role::Cli], false),
    m("memory.import.estimate", &[Role::Cli], false),
    m("memory.import.start", &[Role::Cli], false),
    m("memory.import.status", &[Role::Cli], false),
    m("memory.learning", &[Role::Cli], false),
    m("memory.list", &[Role::Cli], false),
    m("memory.profile", &[Role::Cli], false),
    m("memory.repeats", &[Role::Cli], false),
    m("memory.restore", &[Role::Cli], false),
    m("memory.retract", &[Role::Cli], false),
    m("memory.show", &[Role::Cli], false),
    m("notices.list", &[Role::Cli], false),
    m("notifications.categories", &[Role::Cli], false),
    m("notifications.subscribe", &[Role::Cli], false),
    m("ping", &[Role::Cli, Role::McpShim], false),
    m("ps.attach", &[Role::Cli], false),
    m("ps.kill", &[Role::Cli], false),
    m("ps.list", &[Role::Cli], false),
    m("ps.send", &[Role::Cli], false),
    m("questions.picture", &[Role::Cli], true),
    m("recovery.list", &[Role::Cli], false),
    m("recovery.restore", &[Role::Cli], false),
    m("reminders.cancel", &[Role::Cli], false),
    m("reminders.list", &[Role::Cli], false),
    m("reports.build", &[Role::Cli], false),
    m("reports.diagnostic", &[Role::Cli], false),
    m("reports.send", &[Role::Cli], false),
    m("results.ack", &[Role::Cli], false),
    m("runtimes.check", &[Role::Cli], false),
    m("runtimes.check_calls", &[Role::Cli], true),
    m("runtimes.model", &[Role::Cli], false),
    m("runtimes.models", &[Role::Cli], false),
    m("runtimes.sign_in", &[Role::Cli], false),
    m("runtimes.status", &[Role::Cli], false),
    m("runtimes.switch_task", &[Role::Cli], false),
    m("runtimes.use", &[Role::Cli], false),
    m("schedules.change", &[Role::Cli], false),
    m("schedules.list", &[Role::Cli], false),
    m("secrets.delete", &[Role::Cli], true),
    m("secrets.saved", &[Role::Cli], true),
    m("seen.image", &[Role::Cli], true),
    m("seen.list", &[Role::Cli], true),
    m("sense.pause", &[Role::Cli], false),
    m("sense.resume", &[Role::Cli], false),
    m("sense.status", &[Role::Cli], false),
    m("settings.get", &[Role::Cli], false),
    m("settings.set", &[Role::Cli], false),
    m("setup.browser_open", &[Role::Cli], false),
    m("setup.browser_pair", &[Role::Cli], false),
    m("setup.browsers", &[Role::Cli], false),
    m("setup.browsers_pair_all", &[Role::Cli], false),
    m("setup.browsers_pairing", &[Role::Cli], false),
    m("setup.finish", &[Role::Cli], true),
    m("setup.permission_request", &[Role::Cli], false),
    m("setup.permissions", &[Role::Cli], false),
    m("setup.persona", &[Role::Cli], false),
    m("stop.engage", &[Role::Cli], false),
    m("stop.lift", &[Role::Cli], false),
    m("task.stop", &[Role::Cli], false),
    m("tasks.list", &[Role::Cli], false),
    m("tasks.preview", &[Role::Cli], true),
    m("timeline.get", &[Role::Cli], false),
    m("timers.repeat", &[Role::Cli], false),
    m("timers.states", &[Role::Cli], false),
    m("tools.call", &[Role::McpShim], false),
    m("tools.list", &[Role::McpShim], false),
    m("trace.get", &[Role::Cli], false),
    m("ui.click_through", &[Role::Cli], false),
    m("update.prepare", &[Role::Cli], false),
    m("verdict.set", &[Role::Cli], false),
];

/// The method's row, when FERB has such a method at all.
pub fn method(name: &str) -> Option<&'static Method> {
    METHODS
        .binary_search_by(|m| m.name.cmp(name))
        .ok()
        .map(|at| &METHODS[at])
}

/// auth.py:72-73.
pub fn allowed(role: Role, name: &str) -> bool {
    method(name).is_some_and(|m| m.roles.contains(&role))
}

/// Whether only FERB.app's own process may call it (`method in APP_ONLY`; DMN-23, DMN-402).
pub fn app_only(name: &str) -> bool {
    method(name).is_some_and(|m| m.app_only)
}

/// A connection or run token failed authentication (`core.errors.AuthError`); its words go to the caller with
/// `UNAUTHORIZED`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct AuthError(pub String);

fn refused<T>(why: &str) -> Result<T, AuthError> {
    Err(AuthError(why.to_owned()))
}

/// auth.py:76-78.
pub fn check_cli_peer(peer_uid: u32, owner_uid: u32) -> Result<(), AuthError> {
    if peer_uid != owner_uid {
        return refused("cli role requires the owner's uid");
    }
    Ok(())
}

/// auth.py:155-160.
pub fn check_native_host(
    peer_uid: u32,
    owner_uid: u32,
    origin: &str,
    extension_ids: &HashSet<String>,
) -> Result<(), AuthError> {
    if peer_uid != owner_uid {
        return refused("native_host role requires the owner's uid");
    }
    const SCHEME: &str = "chrome-extension://";
    match origin.strip_prefix(SCHEME) {
        Some(rest) if extension_ids.contains(rest.trim_end_matches('/')) => Ok(()),
        _ => refused("unknown extension origin"),
    }
}

/// Whose browser profile and apps a worker acts with (`core.types.Identity`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Identity {
    /// FERB's own profile and credentials.
    Ferb,
    /// The owner's logged-in browsers and apps; no credentials ever.
    Owner,
}

/// The monotonic clock run tokens expire by (`core.interfaces.Clock.monotonic`), in seconds.
pub trait Clock: Send + Sync {
    fn monotonic(&self) -> f64;
}

/// The process's own monotonic clock.
#[derive(Debug)]
pub struct SystemClock(std::time::Instant);

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock(std::time::Instant::now())
    }
}

impl Clock for SystemClock {
    fn monotonic(&self) -> f64 {
        self.0.elapsed().as_secs_f64()
    }
}

/// What a run token allows (auth.py:81-92).
#[derive(Clone, Debug, PartialEq)]
pub struct RunGrant {
    pub task_id: String,
    pub worker_id: String,
    pub scopes: BTreeSet<String>,
    pub identity: Identity,
    /// `Clock::monotonic()` deadline.
    pub expires_at: f64,
    /// The final checker (V3, W4): the gateway allows observation tools only.
    pub observe_only: bool,
    /// The planner's lookups (ORCH-516): when not empty, the gateway offers and runs only these tools, at most
    /// `max_calls` calls for this grant (0: no limit).
    pub tools: BTreeSet<String>,
    pub max_calls: u64,
}

/// What `RunTokens::issue` takes besides the task and worker.
#[derive(Clone, Debug, PartialEq)]
pub struct GrantSpec {
    pub scopes: BTreeSet<String>,
    pub identity: Identity,
    pub ttl_s: f64,
    pub observe_only: bool,
    pub tools: BTreeSet<String>,
    pub max_calls: u64,
}

impl GrantSpec {
    /// A grant with no scopes, tool list or call limit, as `issue(task, worker, set(), identity, ttl_s)`.
    pub fn new(identity: Identity, ttl_s: f64) -> Self {
        GrantSpec {
            scopes: BTreeSet::new(),
            identity,
            ttl_s,
            observe_only: false,
            tools: BTreeSet::new(),
            max_calls: 0,
        }
    }
}

/// Issue / validate / expire / revoke (auth.py:95-152). Tokens are returned once; only SHA-256 hashes are kept.
pub struct RunTokens {
    clock: Arc<dyn Clock>,
    grants: Mutex<HashMap<String, Arc<RunGrant>>>, // sha256(token) in hex -> grant
    source: Option<Arc<TokenSource>>,
}

/// Where a new token's text comes from instead of the system's random bytes (`secrets.token_urlsafe(32)`): a test's
/// recorded tokens. A daemon never sets one.
pub type TokenSource = dyn Fn() -> String + Send + Sync;

impl RunTokens {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        RunTokens {
            clock,
            grants: Mutex::new(HashMap::new()),
            source: None,
        }
    }

    /// As [`RunTokens::new`], with each token's text from `source` (Python's `secrets.token_urlsafe` patched in a
    /// recorder): the tests that replay recorded token values.
    pub fn with_source(clock: Arc<dyn Clock>, source: Arc<TokenSource>) -> Self {
        RunTokens {
            clock,
            grants: Mutex::new(HashMap::new()),
            source: Some(source),
        }
    }

    fn digest(token: &str) -> String {
        hex::encode(Sha256::digest(token.as_bytes()))
    }

    fn grants(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<RunGrant>>> {
        self.grants
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A new token for `worker_id` of `task_id`: 32 random bytes as URL-safe base64 without padding, like
    /// `secrets.token_urlsafe(32)` (auth.py:106-112).
    pub fn issue(&self, task_id: &str, worker_id: &str, spec: GrantSpec) -> String {
        let token = match &self.source {
            Some(source) => source(),
            None => {
                let mut random = [0u8; 32];
                // No token is better than a guessable one: without the system's random source the daemon must not
                // go on.
                getrandom::fill(&mut random).expect("the system's random source is unavailable");
                base64_url(&random)
            }
        };
        let grant = RunGrant {
            task_id: task_id.to_owned(),
            worker_id: worker_id.to_owned(),
            scopes: spec.scopes,
            identity: spec.identity,
            expires_at: self.clock.monotonic() + spec.ttl_s,
            observe_only: spec.observe_only,
            tools: spec.tools,
            max_calls: spec.max_calls,
        };
        self.grants().insert(Self::digest(&token), Arc::new(grant));
        token
    }

    /// auth.py:114-123. An expired token is forgotten when it is seen.
    pub fn validate(&self, token: &str) -> Result<Arc<RunGrant>, AuthError> {
        let digest = Self::digest(token);
        let mut grants = self.grants();
        // every stored digest is compared, each in constant time (don't leak which prefix matched)
        let mut found = None;
        for (stored, grant) in grants.iter() {
            if same(stored.as_bytes(), digest.as_bytes()) {
                found = Some(Arc::clone(grant));
            }
        }
        let Some(grant) = found else {
            return refused("unknown or revoked run token");
        };
        if self.clock.monotonic() >= grant.expires_at {
            grants.remove(&digest);
            return refused("run token expired");
        }
        Ok(grant)
    }

    /// For an already-authenticated connection: the grant must still be registered (this very grant, not an
    /// equal one) and unexpired, so a revoke or expiry takes effect on open connections at their next call
    /// (auth.py:125-132).
    pub fn recheck(&self, grant: &Arc<RunGrant>) -> Result<Arc<RunGrant>, AuthError> {
        if !self.grants().values().any(|held| Arc::ptr_eq(held, grant)) {
            return refused("run token revoked");
        }
        if self.clock.monotonic() >= grant.expires_at {
            return refused("run token expired");
        }
        Ok(Arc::clone(grant))
    }

    fn revoke(&self, doomed: impl Fn(&RunGrant) -> bool) -> usize {
        let mut grants = self.grants();
        let before = grants.len();
        grants.retain(|_, grant| !doomed(grant));
        before - grants.len()
    }

    /// auth.py:134-138.
    pub fn revoke_worker(&self, worker_id: &str) -> usize {
        self.revoke(|grant| grant.worker_id == worker_id)
    }

    /// A task tree ended: every token of its tasks goes, so a token never outlives its task (auth.py:140-147).
    pub fn revoke_tree(&self, tree_id: &str) -> usize {
        let root = tree_id.split('.').next().unwrap_or(tree_id);
        let children = format!("{root}.");
        self.revoke(|grant| grant.task_id == root || grant.task_id.starts_with(&children))
    }

    /// auth.py:149-152.
    pub fn revoke_all(&self) -> usize {
        self.revoke(|_| true)
    }

    /// How many tokens are live (for tests and `ferb stats`-like facts; never the tokens themselves).
    pub fn len(&self) -> usize {
        self.grants().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl std::fmt::Debug for RunTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunTokens")
            .field("live", &self.len())
            .finish()
    }
}

/// `hmac.compare_digest`: the time taken depends on the length only.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

fn base64_url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, b| (n << 8) | u32::from(*b)) << (8 * (3 - chunk.len()));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeClock(Mutex<f64>);

    impl Clock for FakeClock {
        fn monotonic(&self) -> f64 {
            *self.0.lock().unwrap()
        }
    }

    fn tokens() -> (Arc<FakeClock>, RunTokens) {
        let clock = Arc::new(FakeClock(Mutex::new(1000.0)));
        (Arc::clone(&clock), RunTokens::new(clock))
    }

    fn owner(ttl_s: f64) -> GrantSpec {
        GrantSpec::new(Identity::Owner, ttl_s)
    }

    // tests/test_auth.py:11-15
    #[test]
    fn each_role_only_gets_its_own_methods() {
        assert!(allowed(Role::Cli, "trace.get"));
        assert!(!allowed(Role::McpShim, "trace.get"));
        assert!(!allowed(Role::McpShim, "logs.subscribe"));
        assert!(allowed(Role::McpShim, "ping") && allowed(Role::Cli, "ping"));
        assert!(METHODS.iter().all(|m| !m.roles.contains(&Role::NativeHost)));
        assert!(!allowed(Role::Cli, "auth.hello") && !allowed(Role::Cli, ""));
    }

    #[test]
    fn the_table_is_sorted_so_lookups_find_every_row() {
        assert!(METHODS.windows(2).all(|pair| pair[0].name < pair[1].name));
        assert!(METHODS.iter().all(|m| method(m.name) == Some(m)));
    }

    // decision: DMN-33
    #[test]
    fn restore_stays_on_the_cli_role_and_is_not_app_only() {
        for name in ["recovery.list", "recovery.restore"] {
            assert!(allowed(Role::Cli, name) && !app_only(name));
        }
    }

    // decision: DMN-23
    #[test]
    fn app_only_methods_are_cli_methods() {
        for name in [
            "secrets.saved",
            "secrets.delete",
            "identity.find",
            "identity.choose",
            "identity.pair",
            "about.guide",
            "setup.finish",
            "seen.list",
            "seen.image",
            "tasks.preview",
        ] {
            assert!(app_only(name), "{name}");
        }
        assert!(METHODS
            .iter()
            .filter(|m| m.app_only)
            .all(|m| m.roles == [Role::Cli]));
        assert!(!app_only("ping") && !app_only("no.such.method"));
    }

    // tests/test_auth.py:18-21
    #[test]
    fn cli_requires_the_owner_uid() {
        assert!(check_cli_peer(501, 501).is_ok());
        assert_eq!(
            check_cli_peer(502, 501),
            refused("cli role requires the owner's uid")
        );
    }

    // tests/test_auth.py:24-31
    #[test]
    fn native_host_needs_the_owner_uid_and_a_known_extension_origin() {
        let id = "jaclnphefmmgmpofnabffahdkbjjompm";
        let ids: HashSet<String> = [id.to_owned()].into();
        assert!(check_native_host(501, 501, &format!("chrome-extension://{id}/"), &ids).is_ok());
        assert!(check_native_host(501, 501, &format!("chrome-extension://{id}"), &ids).is_ok());
        assert!(check_native_host(501, 501, &format!("chrome-extension://{id}///"), &ids).is_ok());
        for (uid, origin) in [
            (502, format!("chrome-extension://{id}/")),
            (501, format!("chrome-extension://{}/", "a".repeat(32))),
            (501, format!("https://{id}/")),
            (501, format!("chrome-extension://{id}/x")),
            (501, id.to_owned()),
            (501, String::new()),
        ] {
            assert!(
                check_native_host(uid, 501, &origin, &ids).is_err(),
                "{origin}"
            );
        }
    }

    // tests/test_auth.py:34-50
    #[test]
    fn run_token_lifecycle_expire_and_revoke() {
        let (clock, tokens) = tokens();
        let mut spec = owner(60.0);
        spec.scopes.insert("fs:read".to_owned());
        let t = tokens.issue("task-1", "w-1", spec);
        let g = tokens.validate(&t).unwrap();
        assert_eq!(
            (g.task_id.as_str(), g.worker_id.as_str(), g.identity),
            ("task-1", "w-1", Identity::Owner)
        );
        assert!(g.scopes.contains("fs:read") && g.expires_at == 1060.0);
        *clock.0.lock().unwrap() += 61.0;
        assert_eq!(tokens.validate(&t), refused("run token expired"));
        assert_eq!(tokens.validate(&t), refused("unknown or revoked run token")); // forgotten when seen expired
        let t2 = tokens.issue("task-1", "w-2", owner(60.0));
        let g2 = tokens.validate(&t2).unwrap();
        assert_eq!(tokens.revoke_all(), 1);
        assert!(tokens.validate(&t2).is_err());
        // an open connection loses access at its next call
        assert_eq!(tokens.recheck(&g2), refused("run token revoked"));
    }

    #[test]
    fn a_token_expires_at_its_deadline_and_recheck_wants_the_very_grant() {
        let (clock, tokens) = tokens();
        let t = tokens.issue("t", "w", owner(60.0));
        let g = tokens.validate(&t).unwrap();
        assert!(tokens.recheck(&g).is_ok());
        assert_eq!(
            tokens.recheck(&Arc::new((*g).clone())),
            refused("run token revoked")
        );
        *clock.0.lock().unwrap() += 60.0;
        assert_eq!(tokens.recheck(&g), refused("run token expired"));
    }

    // tests/test_auth.py:53-60
    #[test]
    fn revoke_one_worker_only() {
        let (_, tokens) = tokens();
        let a = tokens.issue("t", "w-a", owner(60.0));
        let b = tokens.issue("t", "w-b", owner(60.0));
        assert_eq!(tokens.revoke_worker("w-a"), 1);
        assert!(tokens.validate(&a).is_err());
        assert!(tokens.validate(&b).is_ok());
    }

    #[test]
    fn a_tree_s_tokens_go_with_the_tree_and_no_other() {
        let (_, tokens) = tokens();
        let root = tokens.issue("abc", "w1", owner(60.0));
        let child = tokens.issue("abc.2.1", "w2", owner(60.0));
        let other = tokens.issue("abcd", "w3", owner(60.0));
        assert_eq!(tokens.revoke_tree("abc.2"), 2); // named by any of its tasks
        assert!(tokens.validate(&root).is_err() && tokens.validate(&child).is_err());
        assert!(tokens.validate(&other).is_ok());
    }

    // tests/test_auth.py:63-68
    #[test]
    fn token_value_is_never_stored() {
        let (_, tokens) = tokens();
        let t = tokens.issue("t", "w", owner(60.0));
        assert_eq!(t.len(), 43); // secrets.token_urlsafe(32)
        assert!(t
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert!(!tokens.grants().contains_key(&t));
        let stored = format!("{:?}", tokens.grants());
        assert!(!stored.contains(&t) && !format!("{tokens:?}").contains(&t));
        assert!(tokens.validate(&format!("{t}x")).is_err());
        assert_ne!(t, tokens.issue("t", "w", owner(60.0)));
    }

    #[test]
    fn base64_is_the_url_safe_alphabet_without_padding() {
        assert_eq!(base64_url(b""), "");
        assert_eq!(base64_url(b"f"), "Zg");
        assert_eq!(base64_url(b"fo"), "Zm8");
        assert_eq!(base64_url(b"foo"), "Zm9v");
        assert_eq!(base64_url(&[0xfb, 0xff, 0xfe]), "-__-");
    }
}
