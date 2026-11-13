//! The setup commands' environment (`ferb/cli/setup.py` `Env`) for this program: the installer over this Mac (its
//! LaunchAgents folder, `launchctl`, `codesign`), the daemon's socket, the terminal, the device scan, the permission
//! and store facts, the runtimes' presence. The service definition names this program as `ferbd daemon`
//! (ferb-device `Programs::Rust`); its native host is `ferb-native-host` beside it.
//!
//! In the test build (`stand-ins`), `FERBD_FAKE_RUN` names a file: every program the commands would run is written
//! there as one JSON line and answered as a success, and the waits are skipped, so a test never runs `launchctl` and
//! never writes outside its temporary HOME.
//!
//! The browser steps are a checkout's only (DMN-507, DMN-508): FERB.app's setup lists no browser folder and reads no
//! profile (its background service connects the browsers once it has Full Disk Access); uninstall lists every known
//! browser's native-host folder; doctor, a developer's command, looks at each installed browser's. Not composed:
//! downloading the embedding model (FERB.app ships it).

use std::io::Write;
use std::path::{Path, PathBuf};

use ferb_browser::profiles::Platform;
use ferb_device::cli::setup::{
    BrowserSetup, Env, PermissionFacts, RunOpts, RuntimeFacts, StoreFacts,
};
use ferb_device::cli::{CliError, Rpc};
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
    if let Some(out) = dry_run(argv) {
        return out;
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

/// The development dry run: with `FERB_DEV_DRY_RUN` naming a file, every program the setup commands would run is
/// written there as one JSON line. Only `codesign`'s reads (display and
/// verify, never a signature) run for real; `launchctl print` answers 113, launchctl's word for a service it doesn't
/// know (the temporary home's service was never loaded); every other program (`launchctl bootstrap`, `security`,
/// `tccutil`...) is answered as a success without running. Like `FERB_DEV_HOME` (crate::host::DEV_HOME) it is honored
/// only by a build signed ad hoc; a certificate-signed build refuses every program while it is set (nothing runs), so
/// a dry run asked of a release build can never become a real one.
pub const DEV_DRY_RUN: &str = "FERB_DEV_DRY_RUN";

fn dry_run_file() -> Option<std::ffi::OsString> {
    std::env::var_os(DEV_DRY_RUN).filter(|v| !v.is_empty())
}

fn dry_run(argv: &[String]) -> Option<Result<Output, String>> {
    let path = dry_run_file()?;
    if !crate::host::ad_hoc_signed() {
        return Some(Err(format!(
            "{DEV_DRY_RUN} is honored only by a development build signed ad hoc: nothing was run"
        )));
    }
    let line = serde_json::to_string(argv).unwrap_or_default();
    let wrote = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| writeln!(f, "{line}"));
    if let Err(e) = wrote {
        return Some(Err(format!("{DEV_DRY_RUN}: {e}")));
    }
    let program = argv.first().map(|p| p.rsplit('/').next().unwrap_or(p));
    let reads_only = program == Some("codesign")
        && argv
            .iter()
            .any(|a| a == "-d" || a == "-dv" || a == "--verify" || a == "--display")
        && !argv
            .iter()
            .any(|a| a == "-s" || a == "--sign" || a == "-f" || a == "--force");
    if reads_only {
        return None; // run for real: it reads a signature and changes nothing
    }
    if program == Some("launchctl") && argv.get(1).map(String::as_str) == Some("print") {
        return Some(Ok(Output {
            code: 113,
            ..Output::default()
        }));
    }
    Some(Ok(Output::default()))
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

/// The test build's recording runner or the development dry run: nothing outside the temporary folders is read
/// (no Keychain presence check).
fn faking() -> bool {
    skip_waits() || (dry_run_file().is_some() && crate::host::ad_hoc_signed())
}

/// The test build skips the installer's waits; a dry run keeps them (it waits for a daemon nothing started, then
/// says so).
fn skip_waits() -> bool {
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
        if !skip_waits() {
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

/// The store's size, pending clean-up and the one-time clean-up itself (ferb-store, its episode-card side over
/// ferb-learning: [`crate::cleanup_cards`]).
struct Store;

impl StoreFacts for Store {
    fn preview(&mut self, path: &Path) -> Value {
        crate::cleanup_cards::preview(path)
    }
    fn apply(
        &mut self,
        home: &Path,
        ok: &str,
        stop: &dyn Fn() -> bool,
        say: &mut dyn FnMut(&str),
    ) -> Result<Value, (String, String)> {
        crate::cleanup_cards::apply(home, ok, stop, say)
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
pub(crate) struct Catalog;

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
    installer_at(home, user_home())
}

/// [`installer`] for the account home `user` (its LaunchAgents folder).
pub fn installer_at(home: &Path, user: PathBuf) -> Installer {
    // the real path: `ferb` may run through the `setup --cli` link
    let exe = crate::host::real_exe().unwrap_or_default();
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
    // Inside FERB.app the agent's executable (BUNDLE_AGENT) and Contents/Helpers/ferb-native-host
    // (BUNDLE_RUST_NATIVE_HOST); a checkout's `ferbd` and `ferb-native-host` beside this program (`ferbd` itself, or
    // the `ferb` program cargo built next to it)
    inst.programs = Programs::Rust(RustPrograms::ferbd(&beside));
    inst.bundle = bundle;
    let config = ferb_core::config::Config::new(home);
    inst.extension_ids = crate::compose::tools::browser::extension_ids(&config)
        .into_iter()
        .collect();
    inst.extension_ids.sort();
    inst
}

/// setup.py:85 `installer(home, SUPPORT, every_browser)`'s browser folders: none from FERB.app (DMN-507, DMN-508:
/// nothing looks at another app's data before the service has Full Disk Access); a checkout's, each installed
/// browser's; uninstall's (`every_browser`), every known browser's.
pub fn native_hosts_dirs(
    platform: &dyn Platform,
    support: &Path,
    from_app: bool,
    every_browser: bool,
) -> Vec<PathBuf> {
    if from_app && !every_browser {
        return Vec::new();
    }
    let browsers: Vec<&str> = if every_browser {
        ferb_browser::profiles::BROWSERS
            .iter()
            .map(|b| b.key)
            .collect()
    } else {
        ferb_browser::profiles::installed(platform, support)
    };
    ferb_browser::autoinstall::native_host_dirs(support, &browsers)
}

/// A checkout's per-browser steps (setup.py:107 `extension_setup`, :1007 uninstall's requests, :899 doctor's
/// native-host folders) over ferb-browser's autoinstall. The only browser files FERB ever deletes are its own: the
/// install requests it recorded (External Extensions/<id>.json) and its native-host manifest; it never removes,
/// disables or uninstalls an extension any other way.
pub struct Browsers<P: Platform> {
    pub platform: P,
}

impl<P: Platform> BrowserSetup for Browsers<P> {
    fn setup_lines(
        &mut self,
        home: &Path,
        support: &Path,
        paired: Option<Vec<(String, String)>>,
    ) -> Vec<String> {
        use ferb_browser::autoinstall as a;
        let config = ferb_core::config::Config::new(home);
        let read = config.get("browser", "store_extension_id").and_then(|s| {
            config
                .get("browser", "extension_ids")
                .map(|ids| (a::store_extension_id(s.as_ref()), ids))
        });
        let (store, ids) = match read {
            Ok((store, ids)) => {
                let ids = a::accepted_extension_ids(ids.as_ref(), &store);
                (store, ids)
            }
            Err(ferb_core::config::ConfigError::Insecure(e)) => {
                return vec![format!(
                    "Browser extension: not set up, config.toml can't be trusted ({e}); `ferb doctor` shows the fix."
                )]
            }
            Err(e) => {
                eprintln!("ferb: {e}");
                return Vec::new();
            }
        };
        // requests FERB wrote for another id (changed, cleared, mistyped) go before the current one is written
        let keep = if a::valid_id(&store) {
            store.as_str()
        } else {
            ""
        };
        let withdrawn = a::withdraw(home, keep).unwrap_or_else(|e| {
            eprintln!("ferb: {}", ferb_shell::py_os_error(&e));
            Vec::new()
        });
        let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let states = match a::prepare(
            &self.platform,
            support,
            &store,
            &id_refs,
            paired.as_deref(),
            Some(home),
        ) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("ferb: {}", ferb_shell::py_os_error(&e));
                return Vec::new();
            }
        };
        let unsupported: Vec<String> = ferb_browser::profiles::unsupported(&self.platform)
            .into_iter()
            .map(|u| u.app)
            .collect();
        a::describe(
            &states,
            &store,
            &ferb_device::extensions::extension_path(home),
            &withdrawn,
            &unsupported,
        )
    }

    fn remove_requests(&mut self, home: &Path, support: &Path) -> Vec<PathBuf> {
        use ferb_browser::autoinstall as a;
        // an untrusted config: no current id (the record in FERB's home still lists every request FERB wrote)
        let store = match ferb_core::config::Config::new(home).get("browser", "store_extension_id")
        {
            Ok(v) => a::store_extension_id(v.as_ref()),
            Err(_) => String::new(),
        };
        a::remove_requests(home, support, &store).unwrap_or_else(|e| {
            eprintln!("ferb: {}", ferb_shell::py_os_error(&e));
            Vec::new()
        })
    }

    fn native_host_dirs(&mut self, support: &Path) -> Vec<PathBuf> {
        let installed = ferb_browser::profiles::installed(&self.platform, support);
        ferb_browser::autoinstall::native_host_dirs(support, &installed)
    }
}

/// setup.py:827 `code_hash(ferb package)`, for this build: the hash of the `ferbd` a checkout's service runs (the
/// daemon reports its own executable's, areas_c `code_hash`); None from FERB.app (doctor compares only a checkout's).
fn checkout_code_hash(inst: &Installer) -> Option<String> {
    if inst.bundle.is_some() {
        return None;
    }
    match &inst.programs {
        Programs::Rust(r) => Some(crate::areas_c::code_hash(
            &r.checkout_daemon.to_string_lossy(),
        ))
        .filter(|h| !h.is_empty()),
        Programs::Python => None,
    }
}

/// `Env(...)` for `home`, printing to `out`; `every_browser`: uninstall's (cli/__init__.py:234).
pub fn env(home: &Path, out: Box<dyn Write>, every_browser: bool) -> Env<'static> {
    let user = user_home();
    let support = user.join("Library").join("Application Support"); // profiles.py:17 SUPPORT
    let mut inst = installer_at(home, user.clone());
    inst.native_hosts_dirs = native_hosts_dirs(
        &ferb_browser::macos::MacPlatform,
        &support,
        inst.bundle.is_some(),
        every_browser,
    );
    inst.cli_dir = Some(user.join(".local").join("bin")); // where FERB.app links `ferb`
    let code_hash = checkout_code_hash(&inst);
    let socket = ferb_core::paths::socket_path(home);
    let bundle = inst.bundle.clone();
    let (h1, h2, h3, h4) = (
        home.to_path_buf(),
        home.to_path_buf(),
        home.to_path_buf(),
        home.to_path_buf(),
    );
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
            if !skip_waits() {
                std::thread::sleep(std::time::Duration::from_secs_f64(s.max(0.0)))
            }
        }),
        run: Box::new(run_program),
        // o200k_base is built into this program (ferb-context): nothing to fetch or copy
        tokenizer: Box::new(|_home: &Path| {
            "Tokenizer: FERB's pinned copy is in place.".to_string()
        }),
        scan: Box::new(move |id: &str| {
            // scan.py:92 `datetime.now().astimezone().isoformat(timespec="seconds")`: aware, whole seconds (a naive
            // `now_local` with microseconds was written here before)
            ferb_device::scan::scan(
                id,
                &ferb_device::scan::app_roots(&user),
                &Catalog,
                &crate::areas_c::macos_version(),
                &ferb_device::scan::now_local(),
            )
        }),
        browser_support: Some(support),
        browsers: Some(Box::new(Browsers {
            platform: ferb_browser::macos::MacPlatform,
        })),
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
        checkout_code_hash: code_hash,
        // setup.py:150 `_finish_install` (DMN-537): a stop while the service is reinstalled waits until it is loaded
        signals: Box::new(ferb_device::cli::signals::Held::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Launch Services knows only Google Chrome.
    struct OnlyChrome;
    impl Platform for OnlyChrome {
        fn has_app(&self, bundle_id: &str) -> bool {
            bundle_id == "com.google.Chrome"
        }
    }

    fn chrome(support: &Path) -> PathBuf {
        let data = support.join("Google/Chrome");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("Local State"), b"{}").unwrap();
        data
    }

    /// setup.py:85: FERB.app's setup lists no browser folder (DMN-507, DMN-508); a checkout's lists each installed
    /// browser's; uninstall lists every known browser's.
    // decision: DMN-508
    #[test]
    fn only_a_checkout_or_uninstall_lists_browser_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let support = tmp.path().join("Application Support");
        let data = chrome(&support);
        assert!(native_hosts_dirs(&OnlyChrome, &support, true, false).is_empty());
        assert_eq!(
            native_hosts_dirs(&OnlyChrome, &support, false, false),
            vec![data.join("NativeMessagingHosts")]
        );
        let every = native_hosts_dirs(&OnlyChrome, &support, true, true);
        assert!(every.contains(&data.join("NativeMessagingHosts")));
        assert!(every.len() >= ferb_browser::profiles::BROWSERS.len());
    }

    /// setup.py:1007: uninstall deletes only FERB's own install requests (External Extensions/<FERB's id>.json);
    /// another extension's request and the installed extensions themselves are never touched.
    // decision: DMN-508
    #[test]
    fn uninstall_removes_only_ferbs_own_request() {
        let tmp = tempfile::tempdir().unwrap();
        let (home, support) = (tmp.path().join("ferb"), tmp.path().join("support"));
        std::fs::create_dir_all(&home).unwrap();
        let data = chrome(&support);
        let requests = data.join("External Extensions");
        std::fs::create_dir_all(&requests).unwrap();
        let store = ferb_browser::autoinstall::STORE_EXTENSION_ID;
        let other = "abcdefghijklmnopabcdefghijklmnop";
        std::fs::write(requests.join(format!("{store}.json")), b"{}").unwrap();
        std::fs::write(requests.join(format!("{other}.json")), b"{}").unwrap();
        let installed = data.join("Default/Extensions").join(store);
        std::fs::create_dir_all(&installed).unwrap();
        let mut b = Browsers {
            platform: OnlyChrome,
        };
        let removed = b.remove_requests(&home, &support);
        assert_eq!(removed, vec![requests.join(format!("{store}.json"))]);
        assert!(requests.join(format!("{other}.json")).exists());
        assert!(installed.exists());
    }

    /// setup.py:107: a checkout's browser step writes FERB's Web Store request into each installed browser and
    /// says so; a config.toml others can read is named instead (InsecureConfig), and nothing is written.
    // decision: DMN-508
    #[test]
    fn a_checkouts_browser_step_requests_ferb_from_the_store() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let (home, support) = (tmp.path().join("ferb"), tmp.path().join("support"));
        std::fs::create_dir_all(&home).unwrap();
        let data = chrome(&support);
        let mut b = Browsers {
            platform: OnlyChrome,
        };
        let lines = b.setup_lines(&home, &support, None);
        let request = data.join(format!(
            "External Extensions/{}.json",
            ferb_browser::autoinstall::STORE_EXTENSION_ID
        ));
        assert!(request.is_file(), "{lines:?}");
        assert!(
            lines.iter().any(|l| l.contains("Google Chrome")),
            "{lines:?}"
        );
        std::fs::remove_file(&request).unwrap();
        let config = home.join("config.toml");
        std::fs::write(&config, "[browser]\n").unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o644)).unwrap();
        let lines = b.setup_lines(&home, &support, None);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("Browser extension: not set up, config.toml can't be trusted ("),
            "{lines:?}"
        );
        assert!(!request.exists());
    }

    /// setup.py:827: a checkout compares the daemon's code with the `ferbd` its service runs; FERB.app doesn't.
    // decision: DMN-509
    #[test]
    fn a_checkouts_code_hash_is_its_built_daemons() {
        let tmp = tempfile::tempdir().unwrap();
        let (home, user, built) = (
            tmp.path().join("ferb"),
            tmp.path().join("user"),
            tmp.path().join("built"),
        );
        std::fs::create_dir_all(&built).unwrap();
        std::fs::write(built.join("ferbd"), b"invented build").unwrap();
        let mut inst = installer_at(&home, user);
        inst.programs = Programs::Rust(RustPrograms::ferbd(&built));
        inst.bundle = None;
        assert_eq!(
            checkout_code_hash(&inst),
            Some(crate::areas_c::code_hash(
                &built.join("ferbd").to_string_lossy()
            ))
        );
        inst.bundle = Some(tmp.path().join("FERB.app"));
        assert_eq!(checkout_code_hash(&inst), None);
    }
}
