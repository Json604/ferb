//! The setup commands' environment (`ferb/cli/setup.py` `Env`) for this program: the installer over this Mac (its
//! LaunchAgents folder, `launchctl`, `codesign`), the daemon's socket, the terminal, the device scan, the permission
//! and store facts, the runtimes' presence. The service definition names this program as `ferbd daemon`
//! (ferb-device `Programs::Rust`); its native host is `ferb-native-host` beside it.
//!
//! In the test build (`stand-ins`), `FERBD_FAKE_RUN` names a file: every program the commands would run is written
//! there as one JSON line and answered as a success, and the waits are skipped, so a test never runs `launchctl` and
//! never writes outside its temporary HOME.
//!
//! Not composed: a checkout's per-browser steps (DMN-508: FERB.app's setup lists no browser folder), the store's
//! one-time clean-up (`setup --cleanup`: its episode-card side, `CardSources`, has no implementation in the Rust
//! build yet; it answers so), downloading the embedding model (FERB.app ships it).

use std::io::Write;
use std::path::{Path, PathBuf};

use ferb_device::cli::setup::{Env, PermissionFacts, RunOpts, RuntimeFacts, StoreFacts};
use ferb_device::cli::{CliError, NoSignals, Rpc};
use ferb_device::install::{
    CodexPin, Installer, Output, Programs, Runner, RustPrograms, Sources, SystemClock,
};
use serde_json::Value;

/// Runs a program as `subprocess.run` does, or (test build) records it.
fn run_program(argv: &[String], opts: &RunOpts) -> Result<Output, String> {
    #[cfg(feature = "stand-ins")]
    if let Some(out) = fake_run(argv) {
        return Ok(out);
    }
    let Some((program, args)) = argv.split_first() else {
        return Err("no program".into());
    };
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    for (k, v) in &opts.env {
        cmd.env(k, v);
    }
    if !opts.capture {
        cmd.stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit());
    }
    let child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(if opts.capture {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::inherit()
        })
        .stderr(if opts.capture {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::inherit()
        })
        .spawn()
        .map_err(|e| e.to_string())?;
    let out = match opts.timeout {
        None => child.wait_with_output().map_err(|e| e.to_string())?,
        Some(t) => {
            let pid = child.id();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(child.wait_with_output());
            });
            match rx.recv_timeout(std::time::Duration::from_secs_f64(t.max(0.0))) {
                Ok(r) => r.map_err(|e| e.to_string())?,
                Err(_) => {
                    // SAFETY: the pid is our own child's, still running: it is ended so nothing outlives the wait.
                    unsafe {
                        libc::kill(pid as i32, libc::SIGKILL);
                    }
                    return Err(format!(
                        "TimeoutExpired: {} timed out after {t} seconds",
                        argv[0]
                    ));
                }
            }
        }
    };
    Ok(Output {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// The test build's recording runner: each argv one JSON line in `FERBD_FAKE_RUN`'s file, answered as a success.
#[cfg(feature = "stand-ins")]
fn fake_run(argv: &[String]) -> Option<Output> {
    let path = std::env::var_os("FERBD_FAKE_RUN")?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()?;
    let _ = writeln!(f, "{}", serde_json::to_string(argv).unwrap_or_default());
    // `FERBD_FAKE_ANSWERS`: [[argv prefix, {"code", "stdout", "stderr"}], ...], the first matching prefix answers
    let answers: Value = std::env::var_os("FERBD_FAKE_ANSWERS")
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    for pair in answers.as_array().into_iter().flatten() {
        let prefix: Vec<String> = serde_json::from_value(pair[0].clone()).unwrap_or_default();
        // an item "*tail" matches any argument ending with `tail`
        let matches = argv.len() >= prefix.len()
            && prefix
                .iter()
                .zip(argv)
                .all(|(p, a)| match p.strip_prefix('*') {
                    Some(tail) => a.ends_with(tail),
                    None => p == a,
                });
        if matches {
            let a = &pair[1];
            return Some(Output {
                code: a["code"].as_i64().unwrap_or(0) as i32,
                stdout: a["stdout"].as_str().unwrap_or("").into(),
                stderr: a["stderr"].as_str().unwrap_or("").into(),
            });
        }
    }
    Some(Output::default())
}

/// The test build's FERB.app to install from (`FERBD_APP_BUNDLE`: a folder laid out as FERB.app in the test's
/// temporary root); a plain build never reads it.
fn test_bundle() -> Option<PathBuf> {
    #[cfg(feature = "stand-ins")]
    if let Some(b) = std::env::var_os("FERBD_APP_BUNDLE") {
        return Some(PathBuf::from(b));
    }
    None
}

fn faking() -> bool {
    cfg!(feature = "stand-ins") && std::env::var_os("FERBD_FAKE_RUN").is_some()
}

/// The installer's runner.
struct Programs_;

impl Runner for Programs_ {
    fn run(&mut self, argv: &[String]) -> Output {
        run_program(
            argv,
            &RunOpts {
                env: Vec::new(),
                capture: true,
                timeout: None,
            },
        )
        .unwrap_or_else(|e| Output {
            code: 127,
            stdout: String::new(),
            stderr: e,
        })
    }
}

/// The installer's clock; the test build's waits are skipped.
struct Clock(SystemClock);

impl ferb_device::install::Clock for Clock {
    fn monotonic(&mut self) -> f64 {
        self.0.monotonic()
    }
    fn sleep(&mut self, seconds: f64) {
        if !faking() {
            self.0.sleep(seconds)
        }
    }
}

/// `permissions.missing(report_rows(report))` and `fda_ready` (ferb-mac-hands).
struct Permissions;

impl PermissionFacts for Permissions {
    fn missing(&mut self, report: &Value) -> Vec<(String, String)> {
        ferb_mac_hands::permissions::missing(&ferb_mac_hands::permissions::report_rows(report))
    }
    fn fda_ready(&mut self, containment: &Value) -> bool {
        ferb_mac_hands::permissions::fda_ready(&crate::basics::object(containment.clone()))
    }
}

/// The store's size and pending clean-up (ferb-store); the clean-up itself isn't composed (see the module's text).
struct Store;

pub const CLEANUP_NOT_COMPOSED: &str =
    "not available in this build: the store's one-time clean-up isn't part of \
the Rust program yet (its episode-card side has no Rust implementation)";

impl StoreFacts for Store {
    fn preview(&mut self, _path: &Path) -> Value {
        serde_json::json!({"error": CLEANUP_NOT_COMPOSED})
    }
    fn apply(
        &mut self,
        _home: &Path,
        _ok: &str,
        _stop: &dyn Fn() -> bool,
        _say: &mut dyn FnMut(&str),
    ) -> Result<Value, (String, String)> {
        Err(("FerbError".into(), CLEANUP_NOT_COMPOSED.into()))
    }
    fn pending(&mut self, path: &Path) -> bool {
        ferb_store::cleanup::pending(path)
    }
    fn size(&mut self, path: &Path) -> u64 {
        ferb_store::store_size(path)
    }
}

/// The runtimes' presence (catalog.py `Runtime.available`): signed in, a model chosen. The Codex binary as the
/// daemon finds it; an API key only through FERB's signed helper (none in a home without one: never the Keychain
/// directly).
struct Runtimes {
    home: PathBuf,
    bundle: Option<PathBuf>,
}

struct SignIn<'a>(&'a Runtimes);

impl ferb_runtimes::catalog::SignInFacts for SignIn<'_> {
    fn codex_binary(&self) -> bool {
        use ferb_daemon::areas_b::runtimes::Programs;
        crate::compose::runtimes::MacPrograms {
            home: self.0.home.clone(),
            app_bundle: self.0.bundle.clone(),
        }
        .codex_binary()
        .is_ok()
    }
    /// keys.py `has_api_key`: presence only, never a value (the test build reads nothing).
    fn has_api_key(&self, provider: &str) -> bool {
        if faking() {
            return false;
        }
        use ferb_daemon::areas_b::runtimes::keys;
        keys::Keys {
            helper: std::sync::Arc::new(ferb_platform_macos::identity::helper(&self.0.home)),
            security: std::sync::Arc::new(keys::SystemSecurity),
            cache: std::sync::Arc::new(keys::KeyCache::default()),
        }
        .has_api_key(provider)
    }
}

impl RuntimeFacts for Runtimes {
    fn available(&mut self, name: &str, home: &Path) -> Option<(bool, String)> {
        let rt = ferb_runtimes::catalog::CATALOG
            .iter()
            .find(|r| r.name == name)?;
        let config = ferb_core::config::Config::new(home);
        let view = ferb_daemon::areas_b::runtimes::cli::View(&config);
        Some(ferb_runtimes::catalog::available(
            rt,
            home,
            &view,
            &SignIn(self),
            None,
        ))
    }
}

/// browsers.known / pair_key and profiles.BROWSERS, for the device scan.
struct Catalog;

impl ferb_device::scan::BrowserCatalog for Catalog {
    fn known(&self, bundle_id: &str) -> bool {
        ferb_mac_hands::apps::known(bundle_id)
    }
    fn pair_key(&self, bundle_id: &str) -> String {
        ferb_mac_hands::apps::pair_key(bundle_id)
    }
    fn paired(&self, bundle_id: &str) -> bool {
        ferb_browser::profiles::BROWSERS
            .iter()
            .any(|b| b.bundle_id == bundle_id)
    }
}

struct Conn(ferb_device::cli::client::Client);

impl Rpc for Conn {
    fn call(&mut self, method: &str, params: Option<Value>) -> Result<Value, CliError> {
        self.0.call(method, params)
    }
    fn close(&mut self) {
        self.0.close()
    }
    fn set_timeout(&mut self, seconds: Option<f64>) {
        self.0.set_timeout(seconds)
    }
    fn stream(
        &mut self,
        method: &str,
        params: Option<Value>,
        on_event: &mut dyn FnMut(Value) -> bool,
    ) -> Result<(), CliError> {
        self.0.stream(method, params, on_event)
    }
}

fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(ferb_core::paths::account_home)
        .unwrap_or_default()
}

/// The installer for FERB's home `home`: this program as the service (`ferbd daemon`), from FERB.app when it runs
/// from one, else from where it was built.
pub fn installer(home: &Path) -> Installer {
    let user = user_home();
    let exe = std::env::current_exe().unwrap_or_default();
    let beside = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let bundle = test_bundle().or_else(crate::home::app_bundle);
    let repo = crate::home::package_folder()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let mut inst = Installer::new(
        home.to_path_buf(),
        user.join("Library/LaunchAgents"),
        Box::new(Programs_),
        Box::new(Clock(SystemClock::default())),
        user,
        CodexPin {
            version: ferb_runtimes::codex::CODEX_VERSION.into(),
            native_bin: ferb_runtimes::codex::native_bin(if cfg!(target_arch = "x86_64") {
                "x86_64"
            } else {
                "arm64"
            }),
        },
        Sources {
            helper_source: repo.join("ferb/identity/secret_helper.swift"),
            extension: repo.join("extensions/chromium"),
            repo_root: repo,
        },
    );
    inst.programs = Programs::Rust(RustPrograms {
        bundle_daemon: bundle
            .as_ref()
            .map(|b| b.join(ferb_device::install::BUNDLE_AGENT))
            .unwrap_or_default(),
        bundle_native_host: bundle
            .as_ref()
            .map(|b| b.join("Contents/MacOS/ferb-native-host"))
            .unwrap_or_default(),
        checkout_daemon: exe,
        checkout_native_host: beside.join("ferb-native-host"),
        daemon_args: vec!["daemon".into()],
    });
    inst.bundle = bundle;
    let config = ferb_core::config::Config::new(home);
    inst.extension_ids = crate::compose::tools::browser::extension_ids(&config)
        .into_iter()
        .collect();
    inst.extension_ids.sort();
    inst
}

/// `Env(...)` for `home`, printing to `out`.
pub fn env(home: &Path, out: Box<dyn Write>) -> Env<'static> {
    let inst = installer(home);
    let socket = ferb_core::paths::socket_path(home);
    let bundle = inst.bundle.clone();
    let (h1, h2, h3, h4) = (
        home.to_path_buf(),
        home.to_path_buf(),
        home.to_path_buf(),
        home.to_path_buf(),
    );
    let user = user_home();
    Env {
        inst,
        connect: Box::new(move || {
            ferb_device::cli::client::Client::connect(&socket, "cli", None, 10.0)
                .map(|c| Box::new(Conn(c)) as Box<dyn Rpc>)
        }),
        ask: Box::new(|prompt: &str| {
            let mut o = std::io::stdout();
            let _ = write!(o, "{prompt}");
            let _ = o.flush();
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            line.trim_end_matches('\n').to_string()
        }),
        out,
        sleep: Box::new(|s: f64| {
            if !faking() {
                std::thread::sleep(std::time::Duration::from_secs_f64(s.max(0.0)))
            }
        }),
        run: Box::new(run_program),
        // o200k_base is built into this program (ferb-context): nothing to fetch or copy
        tokenizer: Box::new(|_home: &Path| {
            "Tokenizer: FERB's pinned copy is in place.".to_string()
        }),
        scan: Box::new(move |id: &str| {
            let now = ferb_py::time::DateTime::now_local();
            ferb_device::scan::scan(
                id,
                &ferb_device::scan::app_roots(&user),
                &Catalog,
                &crate::areas_c::macos_version(),
                &now.isoformat(ferb_py::time::Timespec::Auto),
            )
        }),
        browser_support: None,
        browsers: None,
        permissions: Box::new(Permissions),
        store: Box::new(Store),
        runtimes: Box::new(Runtimes {
            home: h1,
            bundle: bundle.clone(),
        }),
        protection: Box::new(move |_home: &Path| {
            let config = ferb_core::config::Config::new(&h2);
            crate::compose::gateway::protection(&config, &user_home(), &h2, bundle.as_deref())
                .map(|p| (p.reads.len(), p.writes.len()))
        }),
        codex_catalog: Box::new(move |home: &Path| {
            use ferb_daemon::areas_b::runtimes::Programs;
            let programs = crate::compose::runtimes::MacPrograms {
                home: h3.clone(),
                app_bundle: crate::home::app_bundle(),
            };
            let Ok(binary) = programs.codex_binary() else {
                return false;
            };
            let environ: std::collections::BTreeMap<String, String> = std::env::vars().collect();
            ferb_runtimes::codex::bundled_catalog(home, &binary, &environ).is_some()
        }),
        claude_cli: Box::new(move || {
            use ferb_daemon::areas_b::runtimes::Programs;
            crate::compose::runtimes::MacPrograms {
                home: h4.clone(),
                app_bundle: crate::home::app_bundle(),
            }
            .claude_cli()
            .map_err(|e| e.to_string())
        }),
        checkout_code_hash: None,
        signals: Box::new(NoSignals),
    }
}
