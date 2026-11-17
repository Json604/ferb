//! The phone link's helper in this program (stage 3 S4, kb T-1204; PHN-441, PHN-143): FERB.app's
//! `Contents/Helpers/ferb-link` (signed `com.ferb.link`), started and supervised by the daemon, stopped with it.
//!
//! - **When it runs:** while the phone feature is on (`[phone] enabled` in FERB's config). The owner's first action on
//!   the link in Settings → Phone (a pairing code, a relay) turns it on; only `phone.enable {on: false}` turns it off
//!   (paired phones stay paired and can't connect then). A fresh install runs nothing. Idle, the helper keeps one
//!   relay connection and iroh's own checks of the network (about 10 kB a minute, kb PHN-222).
//! - **Supervised:** a helper that exits while the feature is on is journaled `daemon.loop_failed` (loop `phone link`)
//!   and started again after the guard's doubling pause (compose/guard.rs); one the owner turned off is stopped.
//! - **Its keys** (the link's identity key, the state file's seal key, the owner's relay token) are ONE Keychain item,
//!   service `ferb`, account `phone-link`, read and written only through the pinned secrets helper (`ferb-secrets`,
//!   REL-404), the way every one of FERB's own secrets is: the item's access belongs to that stable signature, so no
//!   install or update asks (kb PHN-143). The daemon makes the two keys once, and hands all three to the helper as the
//!   first line of its standard input: never argv, the environment, a file or a log. The helper never touches the
//!   Keychain and holds no permission.
//! - **Stopped with the daemon:** the helper's standard input stays open while it runs; the daemon's end (a shutdown or
//!   a crash) closes it, and the helper stops. `kill_on_drop` covers the rest.
//! - **Contained:** the real daemon starts it under `sandbox-exec` with [`profile`]: no reading or writing of the
//!   owner's home or FERB's data but its own state folder and the daemon's socket, no programs started, network allowed
//!   (the relays and phones). What it says goes to FERB's log, redacted, line by line.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ferb_core::config::Config;
use ferb_daemon::phone::link::LinkControl;
use ferb_daemon::phone::Phones;
use ferb_sanitize::redact::Redactor;
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::watch;

/// The Keychain account of the link's one item (service `ferb`).
pub const ACCOUNT: &str = "phone-link";
/// The keys the item holds (ferb-link `secrets`).
pub const IDENTITY: &str = "identity";
pub const STATE_KEY: &str = "state-key";
pub const RELAY_TOKEN: &str = "relay-token";
/// The helper inside FERB.app (ferb-rpc `link_peer::HELPER`).
pub const BUNDLED: &str = "Contents/Helpers/ferb-link";
/// The helper's state folder in FERB's home.
pub const STATE: &str = "phone-link";

/// Where the link's one Keychain item is kept: the secrets helper in the real daemon, memory in a test.
pub trait LinkKeys: Send + Sync {
    /// The item (JSON: name -> base64), when there is one. Never logged.
    fn load(&self) -> Result<Option<ferb_identity::Secret>, String>;
    fn store(&self, item: &ferb_identity::Secret) -> Result<(), String>;
}

/// The item through FERB's pinned secrets helper (`ferb-secrets`): service `ferb`, account `phone-link`.
pub struct HelperKeys(pub ferb_identity::Helper);

impl LinkKeys for HelperKeys {
    /// The helper answers a lookup of an item that isn't there with a failure, so the first start asks whether the
    /// item is there before reading it (no value read, no prompt): not there is `None`, and the keys are then made.
    /// Live, 2026-11-17, build 202611171534: the first pairing never started the link ("the Keychain didn't show
    /// this item to FERB"), because the lookup's failure ended every start before the keys were made.
    fn load(&self) -> Result<Option<ferb_identity::Secret>, String> {
        if !self
            .0
            .present(ferb_identity::Service::Ferb, ACCOUNT)
            .map_err(|e| e.to_string())?
        {
            return Ok(None);
        }
        self.0
            .lookup(ferb_identity::Service::Ferb, ACCOUNT)
            .map_err(|e| e.to_string())
    }
    fn store(&self, item: &ferb_identity::Secret) -> Result<(), String> {
        self.0
            .store(ferb_identity::Service::Ferb, ACCOUNT, item)
            .map_err(|e| e.to_string())
    }
}

/// The item in memory: a test daemon's (the Keychain is never touched by a test, kb TEST-508).
#[derive(Default)]
pub struct MemoryKeys(pub std::sync::Mutex<Option<String>>);

impl LinkKeys for MemoryKeys {
    fn load(&self) -> Result<Option<ferb_identity::Secret>, String> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .map(ferb_identity::Secret::new))
    }
    fn store(&self, item: &ferb_identity::Secret) -> Result<(), String> {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(item.expose().to_owned());
        Ok(())
    }
}

/// The helper program of this build: FERB.app's, else (a checkout) a `ferb-link` beside this program.
pub fn program(app_bundle: Option<&Path>) -> Option<PathBuf> {
    if let Some(app) = app_bundle {
        return Some(app.join(BUNDLED));
    }
    let exe = std::env::current_exe().ok()?;
    let beside = exe.parent()?.join("ferb-link");
    beside.is_file().then_some(beside)
}

/// The item's keys, the two the daemon makes once added (the system's random bytes) and stored before use.
fn keys(store: &dyn LinkKeys) -> Result<Map<String, Value>, String> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let found = store.load()?;
    let mut map: Map<String, Value> = match &found {
        Some(item) => serde_json::from_str(item.expose())
            .map_err(|_| "the phone link's Keychain item can't be read".to_string())?,
        None => Map::new(),
    };
    let mut made = false;
    for name in [IDENTITY, STATE_KEY] {
        if map.get(name).and_then(Value::as_str).is_none() {
            let mut bytes = zeroize::Zeroizing::new([0u8; 32]);
            getrandom::fill(bytes.as_mut())
                .map_err(|e| format!("the system's random source: {e}"))?;
            map.insert(name.into(), json!(b64.encode(bytes.as_ref())));
            made = true;
        }
    }
    if made {
        store.store(&ferb_identity::Secret::new(
            Value::Object(map.clone()).to_string(),
        ))?;
    }
    Ok(map)
}

/// The macOS sandbox the helper runs under (sandbox-exec). The last matching rule wins: everything a network program
/// needs from the system stays allowed, then the owner's home (FERB's data in it) is closed to reading and writing,
/// writing anywhere is closed, and starting programs is closed; then its own state folder and the daemon's socket are
/// opened again.
pub fn profile(
    user_home: &Path,
    home: &Path,
    state: &Path,
    socket: &Path,
    program: &Path,
) -> String {
    let q = |p: &Path| {
        let s = p
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        format!("\"{s}\"")
    };
    format!(
        "(version 1)\n\
         (allow default)\n\
         (deny file-read* file-write* (subpath {uh}))\n\
         (deny file-read* file-write* (subpath {h}))\n\
         (deny file-write* (subpath \"/\"))\n\
         (allow file-write* (literal \"/dev/null\") (literal \"/dev/dtracehelper\") (regex #\"^/dev/tty\"))\n\
         (deny process-exec* process-fork)\n\
         (allow process-exec (literal {pr}))\n\
         (allow file-read-metadata (literal {uh}) (literal {h}))\n\
         (allow file-read* file-write* (subpath {st}))\n\
         (allow file-read* network-outbound (literal {so}))\n",
        uh = q(user_home),
        h = q(home),
        st = q(state),
        so = q(socket),
        pr = q(program),
    )
}

/// How the daemon runs the helper.
pub struct Runner {
    pub program: Option<PathBuf>,
    pub home: PathBuf,
    pub user_home: PathBuf,
    pub socket: PathBuf,
    pub config: Arc<Config>,
    pub keys: Arc<dyn LinkKeys>,
    pub redactor: Arc<Redactor>,
    /// Run it under [`profile`] (the real daemon); a test's helper runs as it is (sandboxes don't nest).
    pub sandbox: bool,
    /// Extra arguments (a test's `--shared-relay`, `--no-portmap`).
    pub args: Vec<String>,
    enabled: watch::Sender<bool>,
}

/// The phone feature's switch, as Settings → Phone sets it (`[phone] enabled`).
fn enabled_in(config: &Config) -> bool {
    matches!(config.get("phone", "enabled"), Ok(Some(Value::Bool(true))))
}

impl Runner {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        program: Option<PathBuf>,
        home: PathBuf,
        user_home: PathBuf,
        socket: PathBuf,
        config: Arc<Config>,
        keys: Arc<dyn LinkKeys>,
        redactor: Arc<Redactor>,
        sandbox: bool,
        args: Vec<String>,
    ) -> Arc<Runner> {
        let on = enabled_in(&config);
        Arc::new(Runner {
            program,
            home,
            user_home,
            socket,
            config,
            keys,
            redactor,
            sandbox,
            args,
            enabled: watch::channel(on).0,
        })
    }

    fn state(&self) -> PathBuf {
        self.home.join(STATE)
    }

    /// One run of the helper while the feature is on: `Ok` when it was stopped because the feature went off, `Err`
    /// when it ended by itself (the guard journals it and starts it again).
    async fn run_once(&self) -> Result<(), String> {
        let mut on = self.enabled.subscribe();
        // off, or no helper in this build: wait for the switch
        loop {
            let program = self.program.clone().filter(|p| p.is_file());
            if *on.borrow_and_update() && program.is_some() {
                break;
            }
            if *on.borrow() && program.is_none() {
                crate::log::line(
                    "phone link: this build has no ferb-link helper; the phone feature can't run",
                );
            }
            if on.changed().await.is_err() {
                return Ok(());
            }
        }
        let program = self.program.clone().unwrap_or_default();
        let state = self.state();
        std::fs::create_dir_all(&state).map_err(|e| format!("the link's state folder: {e}"))?;
        let _ =
            std::fs::set_permissions(&state, std::os::unix::fs::PermissionsExt::from_mode(0o700));
        let keys = keys(self.keys.as_ref())?;
        let mut cmd = if self.sandbox {
            let mut c = tokio::process::Command::new("/usr/bin/sandbox-exec");
            c.arg("-p")
                .arg(profile(
                    &self.user_home,
                    &self.home,
                    &state,
                    &self.socket,
                    &program,
                ))
                .arg(&program);
            c
        } else {
            tokio::process::Command::new(&program)
        };
        cmd.arg("run")
            .arg("--daemon")
            .arg(&self.socket)
            .arg("--state")
            .arg(&state)
            .args(&self.args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &state)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("starting ferb-link: {e}"))?;
        let mut stdin = child.stdin.take().ok_or("the helper's input")?;
        let mut line = zeroize::Zeroizing::new(json!({"keys": keys}).to_string().into_bytes());
        line.push(b'\n');
        stdin
            .write_all(&line)
            .await
            .map_err(|e| format!("handing the keys over: {e}"))?;
        drop(line);
        // what the helper says, to FERB's log (redacted: it never says a key, but the log is FERB's)
        if let Some(err) = child.stderr.take() {
            let redactor = self.redactor.clone();
            let mut lines = BufReader::new(err).lines();
            tokio::spawn(async move {
                while let Ok(Some(l)) = lines.next_line().await {
                    crate::log::line(&redactor.text(&l));
                }
            });
        }
        crate::log::line(&format!(
            "phone link: started{}",
            if self.sandbox { " (sandboxed)" } else { "" }
        ));
        loop {
            tokio::select! {
                status = child.wait() => {
                    let status = status.map_err(|e| e.to_string())?;
                    return Err(format!("ferb-link ended by itself ({status})"));
                }
                changed = on.changed() => {
                    if changed.is_err() || !*on.borrow() {
                        // closing its input stops it; then make sure
                        drop(stdin);
                        if tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await.is_err() {
                            let _ = child.kill().await;
                        }
                        crate::log::line("phone link: stopped (the phone feature is off)");
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Supervised for the daemon's life (compose/guard.rs): a helper that ended by itself starts again after the pause.
    pub fn start(self: &Arc<Self>, daemon: &super::Daemon) {
        let me = self.clone();
        daemon.spawn_loop("phone link", move |_| {
            let me = me.clone();
            async move {
                loop {
                    me.run_once().await?;
                }
            }
        });
    }
}

impl LinkControl for Runner {
    fn enabled(&self) -> bool {
        *self.enabled.borrow()
    }
    fn set_enabled(&self, on: bool) -> Result<(), String> {
        self.config
            .set("phone", "enabled", &json!(on))
            .map_err(|e| format!("the setting couldn't be saved ({e})"))?;
        self.enabled.send_replace(on);
        Ok(())
    }
    fn keep_relay_token(&self, token: Option<&[u8]>) -> Result<(), String> {
        use base64::Engine;
        let mut map = keys(self.keys.as_ref())?;
        match token {
            Some(t) => {
                map.insert(
                    RELAY_TOKEN.into(),
                    json!(base64::engine::general_purpose::STANDARD.encode(t)),
                );
            }
            None => {
                map.remove(RELAY_TOKEN);
            }
        }
        self.keys
            .store(&ferb_identity::Secret::new(Value::Object(map).to_string()))
    }
}

/// Puts the runner in place as the phone area's control of the link.
pub fn attach(phones: &Phones, runner: &Arc<Runner>) {
    phones.set_link(runner.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two keys are made once and kept in the one item; a later read hands over the same; the relay token joins
    /// the same item.
    // decision: PHN-487
    // decision: PHN-143
    #[test]
    fn the_link_s_keys_are_one_item_made_once() {
        let store = MemoryKeys::default();
        let first = keys(&store).unwrap();
        assert_eq!(first.len(), 2);
        let again = keys(&store).unwrap();
        assert_eq!(first, again, "the keys changed between two starts");
        let item: Map<String, Value> =
            serde_json::from_str(store.0.lock().unwrap().as_deref().unwrap()).unwrap();
        assert_eq!(item, first);
    }

    /// A stand-in for the pinned secrets helper: one item kept in memory. As the real helper, a lookup of an item
    /// that isn't there fails with nothing said.
    struct OneItem(
        std::sync::Mutex<Option<String>>,
        std::sync::atomic::AtomicUsize,
    );

    impl ferb_identity::Launcher for OneItem {
        fn verified(&self, home: &Path) -> ferb_identity::Result<PathBuf> {
            Ok(home.join("bin/ferb-secrets"))
        }
        fn run(
            &self,
            _: &Path,
            body: &[u8],
        ) -> std::io::Result<(bool, zeroize::Zeroizing<Vec<u8>>)> {
            let asked: Value = serde_json::from_slice(body).unwrap();
            assert_eq!(asked["service"], "ferb");
            assert_eq!(asked["account"], ACCOUNT);
            let mut item = self.0.lock().unwrap();
            let (failed, said) = match asked["operation"].as_str().unwrap() {
                "present" => (false, json!({"present": item.is_some()})),
                "lookup" => {
                    self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    match item.as_ref() {
                        Some(v) => (false, json!({"value": v})),
                        None => (true, Value::Null),
                    }
                }
                "store" => {
                    *item = Some(asked["value"].as_str().unwrap().to_owned());
                    (false, json!({"ok": true}))
                }
                other => panic!("the link asked the helper for {other}"),
            };
            let out = if said.is_null() {
                Vec::new()
            } else {
                said.to_string().into_bytes()
            };
            Ok((failed, zeroize::Zeroizing::new(out)))
        }
    }

    /// The live failure of 2026-11-17: on a Mac whose Keychain has no link item yet, the first start makes the keys
    /// and saves them (it never reads an item that isn't there, which the helper answers with a failure); the next
    /// start reads the same keys back.
    // decision: PHN-487
    #[test]
    fn the_first_start_makes_the_keys_when_the_keychain_has_no_item_yet() {
        let keychain = Arc::new(OneItem(
            std::sync::Mutex::new(None),
            std::sync::atomic::AtomicUsize::new(0),
        ));
        let store = HelperKeys(ferb_identity::Helper::new("/nowhere", keychain.clone()));
        let first = keys(&store).expect("the first start");
        assert_eq!(first.len(), 2);
        assert_eq!(
            keychain.1.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "an item that isn't there was read"
        );
        let saved: Map<String, Value> =
            serde_json::from_str(keychain.0.lock().unwrap().as_deref().unwrap()).unwrap();
        assert_eq!(saved, first);
        assert_eq!(keys(&store).expect("the next start"), first);
    }

    /// The profile closes the owner's home and FERB's data but the helper's own folder and the socket.
    // decision: PHN-488
    #[test]
    fn the_profile_opens_only_the_helper_s_own_folder_and_the_socket() {
        let p = profile(
            Path::new("/Users/someone"),
            Path::new("/Users/someone/.ferb"),
            Path::new("/Users/someone/.ferb/phone-link"),
            Path::new("/Users/someone/.ferb/ferb.sock"),
            Path::new("/Applications/FERB.app/Contents/Helpers/ferb-link"),
        );
        let deny = p
            .find("(deny file-read* file-write* (subpath \"/Users/someone\"))")
            .unwrap();
        let own = p
            .find("(allow file-read* file-write* (subpath \"/Users/someone/.ferb/phone-link\"))")
            .unwrap();
        assert!(
            deny < own,
            "the own folder must come after the home's denial (the last match wins)"
        );
        assert!(p.contains("(deny process-exec* process-fork)"));
        assert!(p.contains(
            "(allow process-exec (literal \"/Applications/FERB.app/Contents/Helpers/ferb-link\"))"
        ));
        assert!(p.contains("network-outbound (literal \"/Users/someone/.ferb/ferb.sock\")"));
    }
}
