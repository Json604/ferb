//! The background service's definition and FERB's own files on the Mac (ferb/device/install.py): the user
//! LaunchAgent (DMN-513) or FERB.app's own registration (DMN-589), `service.json`, the native-messaging host's
//! launcher and manifests, the secret helper and its pin, the pinned Codex, the `ferb` link, uninstall and purge.
//!
//! Everything that would touch the machine goes through a seam: every external program (`launchctl`, `codesign`,
//! `security`, `kill -0`, `uv`, `npm`, `xcrun`) through [`Runner`], waiting through [`Clock`], whether a folder is on
//! a read-only volume through `read_only`, the embedding model's download through `fetch_model`. Files are written
//! only under the paths the caller names (FERB's home, the LaunchAgents folder, the browsers' folders, the `ferb`
//! link's folder), so tests point all of them into a temporary folder.
//!
//! The one named difference from Python is [`Programs`]: with [`Programs::Python`] every file is byte for byte what
//! ferb/device/install.py writes (the fixtures' mode); with [`Programs::Rust`] the daemon is the `ferbd` binary and
//! the native host is the `ferb-native-host` binary, run directly where Python writes its interpreter and `-m`
//! arguments. Nothing else changes.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value as Json};
use sha2::{Digest, Sha256};

use crate::plistlib::{self, Value as P};
use crate::py;

pub const LABEL: &str = "com.ferb.daemon";
/// FERB.app's login LaunchAgent fallback (ui/mac/FERBNotch LoginItemPlan.swift); uninstall removes only a file
/// naming this label.
pub const APP_LOGIN_LABEL: &str = "com.ferb.FERB.login";
pub const PYTHON_VERSION: &str = "3.12";
pub const MARKER: &str = ".ferb-home";
pub const NATIVE_HOST_NAME: &str = "com.ferb.native_host";
pub const NATIVE_HOST_MANIFEST: &str = "com.ferb.native_host.json";
pub const APP_BUNDLE_ID: &str = "com.ferb.FERB";
pub const BUNDLE_SERVICE: &str = "Contents/Library/LaunchAgents/com.ferb.daemon.plist";
pub const BUNDLE_MODELS: &str = "Contents/Resources/models";
pub const SERVICE_RECORD: &str = "service.json";
pub const SERVICE_IN_APP: &str = "app";
pub const SERVICE_LAUNCH_AGENT: &str = "launch_agent";
pub const KEYCHAIN_SERVICES: [&str; 2] = ["ferb", "ferb.owner-secrets"];
pub const KEYCHAIN_MAX_ITEMS: usize = 10_000;
pub const DEV_APP_BUNDLE_ID: &str = "dev.ferb.notch";
pub const BUNDLE_CLI: &str = "Contents/Resources/bin/ferb";
pub const BUNDLE_NODE: &str = "Contents/Resources/node";
pub const BUNDLE_SECRET_HELPER: &str = "Contents/Helpers/ferb-secrets";
pub const BUNDLE_AGENT_APP: &str = "Contents/Helpers/FERB.app";
pub const BUNDLE_AGENT: &str = "Contents/Helpers/FERB.app/Contents/MacOS/FERB";
/// ferb/paths.py BUNDLE_PYTHON: the app's plain interpreter.
pub const BUNDLE_PYTHON: &str = "Contents/Resources/python/bin/python3.12";
pub const BUNDLE_PYTHON_FLAGS: [&str; 2] = ["-I", "-B"];
/// The exit status of `setup --service` when the service is switched off in Login Items (FERB.app's
/// ServicePlan.switchedOffStatus).
pub const SWITCHED_OFF_STATUS: i32 = 3;
pub const SWITCHED_OFF_HOW: &str = "FERB's background service is switched off: System Settings → General → Login Items → Allow in the Background → switch FERB on.";
/// ferb/memory/embeddings.py `default_root`: `models/embeddings/<name>@<revision[:12]>` under FERB's home.
pub const EMBEDDING_MODEL_FOLDER: &str = "models/embeddings/potion-base-8M@bf8b056651a2";
/// ferb/runtimes/claude_login.py home_dir: FERB's own Claude home under FERB's home.
pub const CLAUDE_HOME: &str = "claude-home";

/// What an external program answered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == 0
    }

    /// stdout then stderr, as Python's `(r.stdout or '') + (r.stderr or '')`.
    pub fn both(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

/// Runs an external program with its output captured (`subprocess.run(cmd, capture_output=True, text=True)`).
pub trait Runner {
    fn run(&mut self, argv: &[String]) -> Output;
}

/// `time.monotonic()` and `time.sleep()`.
pub trait Clock {
    fn monotonic(&mut self) -> f64;
    fn sleep(&mut self, seconds: f64);
}

/// The real clock.
pub struct SystemClock(std::time::Instant);

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock(std::time::Instant::now())
    }
}

impl Clock for SystemClock {
    fn monotonic(&mut self) -> f64 {
        self.0.elapsed().as_secs_f64()
    }
    fn sleep(&mut self, seconds: f64) {
        std::thread::sleep(std::time::Duration::from_secs_f64(seconds.max(0.0)));
    }
}

/// The real programs (Mac only: `launchctl`, `codesign` and the others exist there).
#[derive(Default)]
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&mut self, argv: &[String]) -> Output {
        let Some((program, args)) = argv.split_first() else {
            return Output {
                code: 1,
                ..Output::default()
            };
        };
        match std::process::Command::new(program).args(args).output() {
            Ok(out) => Output {
                code: out.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            },
            Err(e) => Output {
                code: 127,
                stdout: String::new(),
                stderr: e.to_string(),
            },
        }
    }
}

/// Which programs the service definition and the native host's launcher name. This is the one deliberate difference
/// from ferb/device/install.py, and it is only in the program and its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Programs {
    /// Today's Python build, exactly: FERB.app's agent with `-m ferb.daemon` (a checkout: the venv's python with
    /// `-P -m ferb.daemon`), and the app's python with `-I -B -m ferb.native_host.main` (a checkout: `-P`).
    Python,
    /// The Rust build (rust/PLAN.md part 12): the daemon is `ferbd`, the native host `ferb-native-host`, each run
    /// directly (JOURNAL entry 6: the native host stays the browser's direct child; the launcher `exec`s it).
    Rust(RustPrograms),
}

/// `ferbd`'s argument that runs the background service (`ferbd daemon`; rust/crates/ferbd/src/run.rs).
pub const RUST_DAEMON_ARG: &str = "daemon";
/// The Rust build's native host inside FERB.app.
pub const BUNDLE_RUST_NATIVE_HOST: &str = "Contents/Helpers/ferb-native-host";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RustPrograms {
    /// Inside FERB.app: the daemon's executable (the nested `com.ferb.daemon` app's, BUNDLE_AGENT by default) and the
    /// native host.
    pub bundle_daemon: PathBuf,
    pub bundle_native_host: PathBuf,
    /// A checkout's built binaries (absolute paths).
    pub checkout_daemon: PathBuf,
    pub checkout_native_host: PathBuf,
    /// The daemon's arguments (`["daemon"]` for `ferbd`: [`RustPrograms::ferbd`]).
    pub daemon_args: Vec<String>,
}

impl RustPrograms {
    /// The Rust build's service definition: `ferbd daemon`. Inside FERB.app the nested agent app's executable is
    /// `ferbd` (BUNDLE_AGENT, so macOS's permissions stay FERB.app's agent's) and the native host is
    /// BUNDLE_RUST_NATIVE_HOST; a checkout runs the binaries cargo built in `built` (its `target/release`).
    pub fn ferbd(built: &Path) -> RustPrograms {
        RustPrograms {
            bundle_daemon: BUNDLE_AGENT.into(),
            bundle_native_host: BUNDLE_RUST_NATIVE_HOST.into(),
            checkout_daemon: built.join("ferbd"),
            checkout_native_host: built.join("ferb-native-host"),
            daemon_args: vec![RUST_DAEMON_ARG.into()],
        }
    }
}

/// The pinned Codex (ferb/runtimes/codex_exec.py CODEX_VERSION and `native_bin()` for this Mac).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexPin {
    pub version: String,
    /// The native binary inside a node folder.
    pub native_bin: PathBuf,
}

/// Where a checkout's install reads from (ferb/device/install.py `repo_root()` and the helper's Swift source) and
/// where FERB's extension comes from (ferb/device/extensions.py `extension_source()`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sources {
    pub repo_root: PathBuf,
    pub helper_source: PathBuf,
    pub extension: PathBuf,
}

/// What an install failed with. `type_name` and the message are what the Python code raised.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    /// ferb/device/install.py InstallError.
    #[error("{0}")]
    Install(String),
    /// ServiceSwitchedOff (an InstallError): nothing was loaded.
    #[error("{0}")]
    SwitchedOff(String),
    /// A file operation failed (Python's OSError).
    #[error("{0}")]
    Io(String),
    /// What other code raised (`while_stopped` work, the extension's check: ValueError, ...).
    #[error("{message}")]
    Other { type_name: String, message: String },
}

impl InstallError {
    /// The Python exception class this stands for.
    pub fn type_name(&self) -> &str {
        match self {
            InstallError::Install(_) => "InstallError",
            InstallError::SwitchedOff(_) => "ServiceSwitchedOff",
            InstallError::Io(_) => "OSError",
            InstallError::Other { type_name, .. } => type_name,
        }
    }

    pub fn is_install(&self) -> bool {
        matches!(
            self,
            InstallError::Install(_) | InstallError::SwitchedOff(_)
        )
    }
}

fn io_err(e: std::io::Error, path: &Path) -> InstallError {
    InstallError::Io(py::os_error_text(&e, Some(path)))
}

type Result<T> = std::result::Result<T, InstallError>;

/// Work run while the daemon is stopped; its error is raised again after the daemon is started.
pub type Work<'a> = &'a mut dyn FnMut() -> std::result::Result<(), InstallError>;

/// A download of the embedding model: Err((exception type, message)).
pub type FetchModel = Box<dyn FnMut(&Path) -> std::result::Result<(), (String, String)>>;

/// install.py:101 `launchctl_words`.
pub fn launchctl_words(r: &Output) -> String {
    let err = py::strip(&r.stderr);
    let said = py::last_chars(
        if err.is_empty() {
            py::strip(&r.stdout)
        } else {
            err
        },
        400,
    );
    if said.is_empty() {
        format!("launchctl exited {} and printed nothing", r.code)
    } else {
        format!("launchctl exited {}: {said}", r.code)
    }
}

/// install.py:123: the app hearing runs.
pub fn voice_app(home: &Path, bundle: Option<&Path>) -> PathBuf {
    bundle
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join("FERB.app"))
}

/// install.py:129: the designated requirement from `codesign -d -r-` output.
pub fn designated_requirement(codesign_output: &str) -> String {
    for line in py::splitlines(codesign_output) {
        for prefix in ["designated =>", "# designated =>"] {
            if let Some(rest) = line.strip_prefix(prefix) {
                return py::strip(rest).to_string();
            }
        }
    }
    String::new()
}

/// install.py:148: a `Name=value` line of `codesign -dv`.
pub fn codesign_field(codesign_output: &str, name: &str) -> String {
    let prefix = format!("{name}=");
    py::splitlines(codesign_output)
        .into_iter()
        .find(|l| l.starts_with(&prefix))
        .map(|l| py::strip(l.split_once('=').map(|x| x.1).unwrap_or("")).to_string())
        .unwrap_or_default()
}

fn is_hex(s: &str, lower_only: bool) -> bool {
    s.chars().all(|c| {
        c.is_ascii_digit() || matches!(c, 'a'..='f') || (!lower_only && matches!(c, 'A'..='F'))
    })
}

/// install.py:154.
pub fn trusted_requirement(requirement: &str, pinned_ok: bool) -> bool {
    if requirement.contains("certificate leaf") {
        return true;
    }
    pinned_ok
        && requirement
            .strip_prefix("cdhash H\"")
            .and_then(|r| r.strip_suffix('"'))
            .is_some_and(|h| h.len() == 40 && is_hex(h, true))
}

/// install.py:162: the certificate FERB's release requirement form pins, lowercased.
pub fn certificate_leaf(requirement: &str) -> Option<String> {
    let r = py::strip(requirement);
    let rest = r.strip_prefix("identifier \"")?;
    let close = rest.find('"')?;
    if close == 0 {
        return None;
    }
    let tail = rest[close + 1..].strip_prefix(" and certificate leaf = H\"")?;
    let hexed = tail.strip_suffix('"')?;
    (hexed.len() == 40 && is_hex(hexed, false)).then(|| hexed.to_ascii_lowercase())
}

/// install.py:906: `re.search(r'certificate leaf\[subject\.OU\] = "?TEAM"?(\s|\)|$)', requirement)`.
pub fn names_team(requirement: &str, team: &str) -> bool {
    let key = "certificate leaf[subject.OU] = ";
    let mut from = 0;
    while let Some(at) = requirement[from..].find(key) {
        let rest = &requirement[from + at + key.len()..];
        let firsts: Vec<&str> = match rest.strip_prefix('"') {
            Some(r) => vec![r, rest],
            None => vec![rest],
        };
        for r in firsts {
            if let Some(after) = r.strip_prefix(team) {
                let seconds: Vec<&str> = match after.strip_prefix('"') {
                    Some(a) => vec![a, after],
                    None => vec![after],
                };
                for a in seconds {
                    match a.chars().next() {
                        None => return true,
                        Some(c) if c == ')' || ferb_py::value::is_py_space(c) => return true,
                        _ => {}
                    }
                }
            }
        }
        from += at + 1;
    }
    false
}

/// install.py:138: the code hash FERB.app's own seal records for a nested helper.
pub fn sealed_cdhash(bundle: &Path, nested: &str) -> Option<String> {
    let seal = plistlib::load(&bundle.join("Contents/_CodeSignature/CodeResources"))?;
    let value = seal
        .as_dictionary()?
        .get("files2")?
        .as_dictionary()?
        .get(nested)?
        .as_dictionary()?
        .get("cdhash")?;
    value.as_data().map(hex::encode)
}

/// install.py:169: a launchd plist naming `label`.
pub fn labelled(path: &Path, label: &str) -> bool {
    plistlib::load(path)
        .and_then(|v| v.as_dictionary().and_then(|d| d.get("Label").cloned()))
        .is_some_and(|l| l.as_string() == Some(label))
}

/// install.py:177: the folder is on a read-only volume.
pub fn read_only_volume(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c` is a valid NUL-terminated path; `st` is a zeroed statvfs the call fills in.
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut st) != 0 {
            return false;
        }
        (st.f_flag & libc::ST_RDONLY) != 0
    }
}

/// install.py:181: a hash of FERB's code and packaged resources (the checkout's extension folder in place of
/// `_extension`), its first 16 hex digits.
pub fn code_hash(package_dir: &Path) -> std::io::Result<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let p = entry?.path();
            if p.is_dir() {
                walk(&p, out)?;
            } else if p.is_file() {
                out.push(p);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(package_dir, &mut files)?;
    let mut resources: BTreeMap<String, PathBuf> = BTreeMap::new();
    for f in files {
        let rel = f.strip_prefix(package_dir).unwrap_or(&f).to_path_buf();
        if rel.components().any(|c| c.as_os_str() == "__pycache__")
            || f.file_name().is_some_and(|n| n == ".DS_Store")
            || matches!(f.extension().and_then(|e| e.to_str()), Some("pyc" | "pyo"))
        {
            continue;
        }
        resources.insert(rel.to_string_lossy().into_owned(), f);
    }
    let extension = package_dir
        .parent()
        .unwrap_or(package_dir)
        .join("extensions/chromium");
    if extension.join("manifest.json").is_file() {
        resources.retain(|k, _| !k.starts_with("_extension/"));
        let mut ext = Vec::new();
        walk(&extension, &mut ext)?;
        for f in ext {
            if f.file_name().is_some_and(|n| n == ".DS_Store") {
                continue;
            }
            let rel = f
                .strip_prefix(&extension)
                .unwrap_or(&f)
                .to_string_lossy()
                .into_owned();
            resources.insert(format!("_extension/{rel}"), f);
        }
    }
    let mut h = Sha256::new();
    for (name, f) in &resources {
        h.update(name.as_bytes());
        h.update(b"\0");
        h.update(std::fs::read(f)?);
        h.update(b"\0");
    }
    Ok(hex::encode(h.finalize())[..16].to_string())
}

/// Python `json.dumps(value, indent=2)`.
fn dumps_indent2(value: &Json) -> String {
    ferb_py::json::dumps_with(
        value,
        ferb_py::json::Style {
            ascii: true,
            sort_keys: false,
            indent: Some(2),
        },
    )
}

/// `shutil.copyfile`: the bytes only (a new file gets the default mode).
fn copyfile(from: &Path, to: &Path) -> Result<()> {
    let data = std::fs::read(from).map_err(|e| io_err(e, from))?;
    let _ = std::fs::remove_file(to);
    std::fs::write(to, data).map_err(|e| io_err(e, to))
}

fn chmod(path: &Path, mode: u32) -> Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|e| io_err(e, path))
}

fn mkdirs(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|e| io_err(e, path))
}

fn rename(from: &Path, to: &Path) -> Result<()> {
    std::fs::rename(from, to).map_err(|e| io_err(e, from))
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// `path.exists() or path.is_symlink()`.
fn lexists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// install.py:438 `_remove`.
fn remove(path: &Path) -> Result<()> {
    if is_symlink(path) || path.is_file() {
        std::fs::remove_file(path).map_err(|e| io_err(e, path))
    } else if path.exists() {
        std::fs::remove_dir_all(path).map_err(|e| io_err(e, path))
    } else {
        Ok(())
    }
}

/// `path.unlink(missing_ok=True)`.
fn unlink_missing_ok(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// The manifests written, and each folder that refused one with why.
pub type HostsWritten = (Vec<PathBuf>, Vec<(PathBuf, String)>);

/// install.py:199: FERB's native-host launcher (in FERB's home) and its manifest in each of `dirs`:
/// (written, [(folder, why)]). `exec_part` is what the launcher `exec`s (with its arguments, before `"$@"`).
pub fn write_native_hosts(
    home: &Path,
    launcher: &Path,
    exec_part: &str,
    extension_ids: &[String],
    dirs: &[PathBuf],
) -> Result<HostsWritten> {
    if let Some(parent) = launcher.parent() {
        mkdirs(parent)?;
    }
    let script = format!(
        "#!/bin/sh\nFERB_HOME=\"{}\" exec {exec_part} \"$@\"\n",
        home.display()
    );
    std::fs::write(launcher, script).map_err(|e| io_err(e, launcher))?;
    chmod(launcher, 0o700)?;
    let mut manifest = Map::new();
    manifest.insert("name".into(), json!(NATIVE_HOST_NAME));
    manifest.insert("description".into(), json!("FERB native messaging host"));
    manifest.insert("path".into(), json!(launcher.to_string_lossy()));
    manifest.insert("type".into(), json!("stdio"));
    manifest.insert(
        "allowed_origins".into(),
        Json::Array(
            extension_ids
                .iter()
                .map(|i| json!(format!("chrome-extension://{i}/")))
                .collect(),
        ),
    );
    let manifest = dumps_indent2(&Json::Object(manifest));
    let mut written = Vec::new();
    let mut failed = Vec::new();
    for d in dirs {
        let target = d.join(NATIVE_HOST_MANIFEST);
        match write_one_manifest(d, &target, &manifest) {
            Ok(()) => written.push(target),
            Err(why) => failed.push((d.clone(), why)),
        }
    }
    Ok((written, failed))
}

/// One manifest written whole (staged, renamed) and read back: Err(why) as Python's `e.strerror or str(e)`.
fn write_one_manifest(d: &Path, target: &Path, manifest: &str) -> std::result::Result<(), String> {
    std::fs::create_dir_all(d).map_err(|e| py::os_words(&e))?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".ferb-")
        .suffix(".json")
        .tempfile_in(d)
        .map_err(|e| py::os_words(&e))?;
    tmp.write_all(manifest.as_bytes())
        .map_err(|e| py::os_words(&e))?;
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o644))
        .map_err(|e| py::os_words(&e))?;
    tmp.persist(target).map_err(|e| py::os_words(&e.error))?;
    match std::fs::read_to_string(target) {
        Ok(back) if back == manifest => Ok(()),
        Ok(_) => Err("the manifest read back differs from what FERB wrote".into()),
        Err(e) => Err(py::os_words(&e)),
    }
}

/// install.py:236 `Installer`: everything the installer needs to know, and its seams.
pub struct Installer {
    pub home: PathBuf,
    pub label: String,
    pub run: Box<dyn Runner>,
    pub clock: Box<dyn Clock>,
    pub launch_agents: PathBuf,
    pub native_hosts_dirs: Vec<PathBuf>,
    pub extension_ids: Vec<String>,
    /// The FERB.app to install from; None: from a checkout.
    pub bundle: Option<PathBuf>,
    /// FERB.app registers the service itself (`setup --service --app-service`); None: as setup last recorded.
    pub app_service: Option<bool>,
    pub cli_dir: Option<PathBuf>,
    pub read_only: Box<dyn Fn(&Path) -> bool>,
    pub fetch_model: Option<FetchModel>,
    pub notes: Vec<String>,
    /// `os.getuid()` (launchd's `gui/<uid>` domain).
    pub uid: u32,
    /// The account's home (`Path.home()`): purge refuses it and its Desktop.
    pub user_home: PathBuf,
    pub programs: Programs,
    pub codex: CodexPin,
    pub sources: Sources,
    stopped: Option<i64>,
}

impl Installer {
    /// An installer with the given seams; the rest as ferb/device/install.py's defaults (label `com.ferb.daemon`, no
    /// native-host folders, a checkout).
    pub fn new(
        home: PathBuf,
        launch_agents: PathBuf,
        run: Box<dyn Runner>,
        clock: Box<dyn Clock>,
        user_home: PathBuf,
        codex: CodexPin,
        sources: Sources,
    ) -> Self {
        Installer {
            home,
            label: LABEL.into(),
            run,
            clock,
            launch_agents,
            native_hosts_dirs: Vec::new(),
            extension_ids: Vec::new(),
            bundle: None,
            app_service: None,
            cli_dir: None,
            read_only: Box::new(read_only_volume),
            fetch_model: None,
            notes: Vec::new(),
            // SAFETY: getuid has no preconditions and cannot fail.
            uid: unsafe { libc::getuid() },
            user_home,
            programs: Programs::Python,
            codex,
            sources,
            stopped: None,
        }
    }

    fn cmd(&mut self, argv: &[&str]) -> Output {
        let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        self.run.run(&argv)
    }

    fn cmd_owned(&mut self, argv: Vec<String>) -> Output {
        self.run.run(&argv)
    }

    // ---------- paths ----------

    pub fn python_dir(&self) -> PathBuf {
        self.home.join("python")
    }
    pub fn venv(&self) -> PathBuf {
        self.home.join("venv")
    }
    pub fn venv_python(&self) -> PathBuf {
        self.venv().join("bin/python")
    }
    pub fn staged_venv(&self) -> PathBuf {
        self.home.join("venv.new")
    }
    pub fn retired_venv(&self) -> PathBuf {
        self.home.join("venv.old")
    }
    pub fn record_path(&self) -> PathBuf {
        self.home.join(SERVICE_RECORD)
    }
    pub fn plist_path(&self) -> PathBuf {
        self.launch_agents.join(format!("{}.plist", self.label))
    }
    pub fn domain(&self) -> String {
        format!("gui/{}", self.uid)
    }
    fn target(&self) -> String {
        format!("{}/{}", self.domain(), self.label)
    }
    pub fn native_host_launcher(&self) -> PathBuf {
        self.home.join("bin/ferb-native-host")
    }
    pub fn cli_link(&self) -> Option<PathBuf> {
        self.cli_dir.as_ref().map(|d| d.join("ferb"))
    }
    pub fn model_root(&self) -> PathBuf {
        self.home.join(EMBEDDING_MODEL_FOLDER)
    }

    /// The background service is FERB.app's own registration (DMN-589), not a user LaunchAgent.
    pub fn in_app(&self) -> bool {
        if self.bundle.is_none() {
            return false;
        }
        if let Some(app_service) = self.app_service {
            return app_service;
        }
        self.recorded().get("service").and_then(Json::as_str) == Some(SERVICE_IN_APP)
    }

    /// install.py:299.
    pub fn interpreter(&self) -> Option<PathBuf> {
        let prefix = format!("cpython-{PYTHON_VERSION}");
        let mut found: Vec<PathBuf> = std::fs::read_dir(self.python_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
            .map(|e| e.path().join(format!("bin/python{PYTHON_VERSION}")))
            .filter(|p| lexists(p))
            .collect();
        found.sort();
        found.pop()
    }

    /// What runs the daemon.
    pub fn daemon_python(&self) -> PathBuf {
        match (&self.bundle, &self.programs) {
            (Some(b), Programs::Python) => b.join(BUNDLE_AGENT),
            (Some(b), Programs::Rust(r)) => b.join(&r.bundle_daemon),
            (None, Programs::Python) => self.venv_python(),
            (None, Programs::Rust(r)) => r.checkout_daemon.clone(),
        }
    }

    /// install.py:308: the interpreter for FERB's other processes (Python only).
    pub fn python_command(&self) -> Vec<String> {
        match &self.bundle {
            Some(b) => {
                let mut v = vec![b.join(BUNDLE_PYTHON).to_string_lossy().into_owned()];
                v.extend(BUNDLE_PYTHON_FLAGS.iter().map(|s| s.to_string()));
                v
            }
            None => vec![
                self.venv_python().to_string_lossy().into_owned(),
                "-P".into(),
            ],
        }
    }

    /// install.py:312: the service's program and arguments.
    pub fn daemon_command(&self) -> Vec<String> {
        let program = self.daemon_python().to_string_lossy().into_owned();
        match (&self.programs, &self.bundle) {
            (Programs::Python, Some(_)) => vec![program, "-m".into(), "ferb.daemon".into()],
            (Programs::Python, None) => {
                vec![program, "-P".into(), "-m".into(), "ferb.daemon".into()]
            }
            (Programs::Rust(r), _) => {
                let mut v = vec![program];
                v.extend(r.daemon_args.iter().cloned());
                v
            }
        }
    }

    /// What the native host's launcher `exec`s.
    pub fn native_host_exec(&self) -> String {
        match &self.programs {
            Programs::Python => {
                let command: Vec<String> = self
                    .python_command()
                    .iter()
                    .map(|a| py::shlex_quote(a))
                    .collect();
                format!("{} -m ferb.native_host.main", command.join(" "))
            }
            Programs::Rust(r) => {
                let path = match &self.bundle {
                    Some(b) => b.join(&r.bundle_native_host),
                    None => r.checkout_native_host.clone(),
                };
                py::shlex_quote(&path.to_string_lossy())
            }
        }
    }

    pub fn granted_binary(&self) -> Option<PathBuf> {
        let p = self.daemon_python();
        p.exists().then(|| py::resolve(&p))
    }

    pub fn grant_target(&self) -> Option<PathBuf> {
        match &self.bundle {
            Some(b) => Some(b.join(BUNDLE_AGENT_APP)),
            None => self.granted_binary(),
        }
    }

    /// install.py:328.
    pub fn active_program(&self) -> Option<String> {
        let first = plistlib::load(&self.plist_path()).and_then(|v| {
            let args = v.as_dictionary()?.get("ProgramArguments")?.clone();
            if !plistlib::truthy(&args) {
                return None;
            }
            match args {
                plist::Value::Array(a) => a
                    .first()
                    .filter(|x| plistlib::truthy(x))
                    .map(plistlib::py_str),
                plist::Value::String(s) => s.chars().next().map(String::from),
                _ => None,
            }
        });
        if first.is_some() {
            return first;
        }
        let rec = self.recorded();
        let app = rec.get("app")?;
        if !ferb_py::value::truthy(app) {
            return None;
        }
        Some(
            Path::new(&ferb_py::value::py_str(app))
                .join(BUNDLE_AGENT)
                .to_string_lossy()
                .into_owned(),
        )
    }

    /// What FERB.app's setup last recorded ({} when nothing was, or it can't be read).
    pub fn recorded(&self) -> Map<String, Json> {
        std::fs::read_to_string(self.record_path())
            .ok()
            .and_then(|t| serde_json::from_str::<Json>(&t).ok())
            .and_then(|v| match v {
                Json::Object(m) => Some(m),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// install.py:348.
    pub fn write_record(&mut self) -> Result<()> {
        let bundle = self
            .bundle
            .clone()
            .expect("write_record runs for FERB.app only");
        let build = self.bundle_build()?;
        let signer = self.signer();
        let mut record = Map::new();
        record.insert("build".into(), json!(build));
        record.insert("app".into(), json!(bundle.to_string_lossy()));
        record.insert("label".into(), json!(self.label));
        record.insert("signer".into(), json!(signer));
        if self.in_app() {
            record.insert("service".into(), json!(SERVICE_IN_APP));
        } else {
            record.insert("service".into(), json!(SERVICE_LAUNCH_AGENT));
            record.insert(
                "definition".into(),
                json!(hex::encode(Sha256::digest(self.plist_bytes()?))),
            );
        }
        let data = dumps_indent2(&Json::Object(record));
        let staged = self.home.join(format!(".{SERVICE_RECORD}.new"));
        std::fs::write(&staged, format!("{data}\n")).map_err(|e| io_err(e, &staged))?;
        rename(&staged, &self.record_path())
    }

    /// install.py:365: "team:<ID>", "leaf:<SHA-1>" or "adhoc".
    pub fn signer(&mut self) -> String {
        let bundle = self.bundle.clone().expect("signer runs for FERB.app only");
        let b = bundle.to_string_lossy().into_owned();
        let shown = self.cmd(&["/usr/bin/codesign", "-dv", &b]);
        let team = codesign_field(&shown.both(), "TeamIdentifier");
        if !team.is_empty() && team != "not set" {
            return format!("team:{team}");
        }
        let own = self.cmd(&["/usr/bin/codesign", "-d", "-r-", &b]);
        match certificate_leaf(&designated_requirement(&own.both())) {
            Some(leaf) => format!("leaf:{leaf}"),
            None => "adhoc".into(),
        }
    }

    /// FERB.app's Info.plist (empty for a checkout); an unreadable one is an error, as in Python.
    pub fn bundle_info(&self) -> Result<plist::Dictionary> {
        let Some(bundle) = &self.bundle else {
            return Ok(plist::Dictionary::new());
        };
        let path = bundle.join("Contents/Info.plist");
        let bytes = std::fs::read(&path).map_err(|e| io_err(e, &path))?;
        let v = plistlib::loads(&bytes).ok_or_else(|| InstallError::Other {
            type_name: "InvalidFileException".into(),
            message: "Invalid file".into(),
        })?;
        v.into_dictionary().ok_or_else(|| InstallError::Other {
            type_name: "AttributeError".into(),
            message: "Info.plist is not a dictionary".into(),
        })
    }

    fn info_str(&self, key: &str, default: &str) -> Result<String> {
        Ok(self
            .bundle_info()?
            .get(key)
            .map(plistlib::py_str)
            .unwrap_or_else(|| default.into()))
    }

    fn bundle_build(&self) -> Result<String> {
        self.info_str("CFBundleVersion", "")
    }

    // ---------- steps ----------

    pub fn write_marker(&self) -> Result<()> {
        mkdirs(&self.home)?;
        chmod(&self.home, 0o700)?;
        let p = self.home.join(MARKER);
        std::fs::write(
            &p,
            "This folder is FERB's home. `ferb uninstall --purge` removes it.\n",
        )
        .map_err(|e| io_err(e, &p))
    }

    pub fn ensure_interpreter(&mut self) -> Result<PathBuf> {
        if let Some(existing) = self.interpreter() {
            return Ok(existing);
        }
        let dir = self.python_dir().to_string_lossy().into_owned();
        self.ok(
            &[
                "uv",
                "python",
                "install",
                PYTHON_VERSION,
                "--install-dir",
                &dir,
            ],
            "installing FERB's dedicated Python",
        )?;
        self.interpreter().ok_or_else(|| {
            InstallError::Install(format!(
                "uv installed Python but no python{PYTHON_VERSION} found under {}",
                self.python_dir().display()
            ))
        })
    }

    fn stage_venv(&mut self, interpreter: &Path) -> Result<()> {
        let root = self.sources.repo_root.to_string_lossy().into_owned();
        remove(&self.staged_venv())?;
        let staged = self.staged_venv().to_string_lossy().into_owned();
        let interp = interpreter.to_string_lossy().into_owned();
        self.ok(
            &[
                "uv",
                "venv",
                "--quiet",
                "--relocatable",
                "--python",
                &interp,
                &staged,
            ],
            "creating FERB's new venv",
        )?;
        let python = self
            .staged_venv()
            .join("bin/python")
            .to_string_lossy()
            .into_owned();
        let folder = tempfile::Builder::new()
            .prefix(".ferb-lock-")
            .tempdir_in(&self.home)
            .map_err(|e| io_err(e, &self.home))?;
        let locked = folder
            .path()
            .join("requirements.txt")
            .to_string_lossy()
            .into_owned();
        let steps = (|| -> Result<()> {
            self.ok(
                &[
                    "uv",
                    "export",
                    "--project",
                    &root,
                    "--frozen",
                    "--no-dev",
                    "--no-emit-project",
                    "--format",
                    "requirements-txt",
                    "--quiet",
                    "-o",
                    &locked,
                ],
                "reading FERB's locked dependencies (uv.lock)",
            )?;
            self.ok(
                &[
                    "uv",
                    "pip",
                    "install",
                    "--quiet",
                    "--python",
                    &python,
                    "--require-hashes",
                    "-r",
                    &locked,
                ],
                "installing FERB's locked dependencies",
            )
        })();
        drop(folder);
        steps?;
        self.ok(
            &[
                "uv",
                "pip",
                "install",
                "--quiet",
                "--python",
                &python,
                "--no-deps",
                &root,
            ],
            "installing FERB into its new venv",
        )
    }

    fn swap_venv(&self) -> Result<()> {
        if !self.staged_venv().exists() {
            return Ok(());
        }
        remove(&self.retired_venv())?;
        if self.venv().exists() || is_symlink(&self.venv()) {
            rename(&self.venv(), &self.retired_venv())?;
        }
        rename(&self.staged_venv(), &self.venv())?;
        remove(&self.retired_venv())
    }

    fn recover_venv(&self) -> Result<()> {
        if !self.venv().exists() && self.retired_venv().exists() {
            rename(&self.retired_venv(), &self.venv())?;
        }
        Ok(())
    }

    /// install.py:444: a role uses Codex ([runtimes], Codex when unset); a config others can read counts as yes.
    pub fn codex_wanted(&self) -> Result<bool> {
        let config = ferb_core::config::Config::new(&self.home);
        let get = |role: &str,
                   default: &str|
         -> std::result::Result<String, ferb_core::config::ConfigError> {
            Ok(match config.get("runtimes", role)? {
                Some(v) => ferb_py::value::py_str(&v),
                None => default.into(),
            })
        };
        let wanted = (|| {
            let worker = get("worker", "codex")?;
            for role in ["orchestrator", "worker", "checker"] {
                let default = if role == "checker" {
                    worker.as_str()
                } else {
                    "codex"
                };
                if get(role, default)? == "codex" {
                    return Ok(true);
                }
            }
            Ok(false)
        })();
        match wanted {
            Ok(w) => Ok(w),
            Err(ferb_core::config::ConfigError::Insecure(_)) => Ok(true),
            Err(e) => Err(InstallError::Other {
                type_name: "ConfigError".into(),
                message: e.to_string(),
            }),
        }
    }

    fn codex_reports_pinned(&mut self, binary: &Path) -> bool {
        let out = self.cmd_owned(vec![
            binary.to_string_lossy().into_owned(),
            "--version".into(),
        ]);
        py::split_whitespace(&out.stdout).last() == Some(&self.codex.version.as_str())
    }

    /// install.py:458.
    pub fn ensure_codex(&mut self) -> Result<()> {
        let node = self.home.join("node");
        let binary = node.join(&self.codex.native_bin);
        if self.bundle.is_none() && is_symlink(&node) {
            std::fs::remove_file(&node).map_err(|e| io_err(e, &node))?;
        }
        if binary.exists() && self.codex_reports_pinned(&binary) {
            return Ok(());
        }
        if let Some(bundle) = self.bundle.clone() {
            let bundled = bundle.join(BUNDLE_NODE);
            if !bundled.join(&self.codex.native_bin).is_file() {
                return Err(InstallError::Install(format!(
                    "{} has no pinned Codex; download FERB again",
                    bundle.display()
                )));
            }
            remove(&node)?;
            std::os::unix::fs::symlink(&bundled, &node).map_err(|e| io_err(e, &node))?;
            return Ok(());
        }
        let staged = self.home.join("node.new");
        remove(&staged)?;
        mkdirs(&staged)?;
        let attempt = (|| -> Result<()> {
            for name in ["package.json", "package-lock.json"] {
                copyfile(&self.sources.repo_root.join(name), &staged.join(name))?;
            }
            let prefix = staged.to_string_lossy().into_owned();
            self.ok(
                &[
                    "npm",
                    "ci",
                    "--prefix",
                    &prefix,
                    "--ignore-scripts",
                    "--no-fund",
                    "--no-audit",
                    "--loglevel=error",
                ],
                "installing the pinned Codex (package-lock.json)",
            )?;
            let out = self.cmd_owned(vec![
                staged
                    .join(&self.codex.native_bin)
                    .to_string_lossy()
                    .into_owned(),
                "--version".into(),
            ]);
            let words = py::split_whitespace(&out.stdout);
            let got = words.last().copied();
            if got != Some(self.codex.version.as_str()) {
                return Err(InstallError::Install(format!(
                    "package-lock.json installed Codex {}, not the pinned {}",
                    got.unwrap_or("nothing"),
                    self.codex.version
                )));
            }
            Ok(())
        })();
        if let Err(e) = attempt {
            let _ = remove(&staged);
            return Err(e);
        }
        remove(&node)?;
        rename(&staged, &node)
    }

    /// install.py:495.
    pub fn ensure_embedding_model(&mut self) {
        if self.bundle.is_some() {
            return;
        }
        let root = self.model_root();
        if let Some(fetch) = self.fetch_model.as_mut() {
            if let Err((kind, message)) = fetch(&root) {
                self.notes.push(format!(
                    "FERB's local embedding model wasn't fetched ({kind}: {message}); memory search uses keywords until the daemon fetches it"
                ));
            }
        }
    }

    /// install.py:509: the browser extension's folder.
    pub fn install_files(&self) -> Result<()> {
        crate::extensions::install_extension(&self.home, &self.sources.extension).map(|_| ())
    }

    /// install.py:514: the LaunchAgent.
    pub fn plist(&self) -> Result<BTreeMap<String, P>> {
        let logs = self.home.join("logs");
        let mut env = BTreeMap::new();
        env.insert(
            "FERB_HOME".to_string(),
            P::string(self.home.to_string_lossy()),
        );
        if self.bundle.is_some() {
            env.insert(
                "FERB_APP_BUILD".to_string(),
                P::string(self.bundle_build()?),
            );
        }
        let mut keep_alive = BTreeMap::new();
        keep_alive.insert("SuccessfulExit".to_string(), P::Bool(false));
        let mut agent = BTreeMap::new();
        agent.insert("Label".to_string(), P::string(&self.label));
        agent.insert(
            "ProgramArguments".to_string(),
            P::strings(self.daemon_command()),
        );
        agent.insert("EnvironmentVariables".to_string(), P::Dict(env));
        agent.insert(
            "WorkingDirectory".to_string(),
            P::string(self.home.to_string_lossy()),
        );
        agent.insert("RunAtLoad".to_string(), P::Bool(true));
        agent.insert("KeepAlive".to_string(), P::Dict(keep_alive));
        agent.insert("ThrottleInterval".to_string(), P::Integer(5));
        agent.insert("ProcessType".to_string(), P::string("Interactive"));
        agent.insert("LimitLoadToSessionType".to_string(), P::string("Aqua"));
        agent.insert(
            "StandardOutPath".to_string(),
            P::string(logs.join("daemon.out.log").to_string_lossy()),
        );
        agent.insert(
            "StandardErrorPath".to_string(),
            P::string(logs.join("daemon.err.log").to_string_lossy()),
        );
        if self.bundle.is_some() {
            agent.insert(
                "AssociatedBundleIdentifiers".to_string(),
                P::strings([self.info_str("CFBundleIdentifier", APP_BUNDLE_ID)?]),
            );
        }
        Ok(agent)
    }

    /// Exactly what setup writes.
    pub fn plist_bytes(&self) -> Result<Vec<u8>> {
        plistlib::dumps(&P::Dict(self.plist()?)).map_err(|e| InstallError::Other {
            type_name: "ValueError".into(),
            message: e.to_string(),
        })
    }

    pub fn definition_current(&self) -> Result<bool> {
        match std::fs::read(self.plist_path()) {
            Ok(bytes) => Ok(bytes == self.plist_bytes()?),
            Err(_) => Ok(false),
        }
    }

    /// install.py:550: writes the LaunchAgent unless it is already exactly that; true if it changed.
    pub fn write_plist(&self) -> Result<bool> {
        mkdirs(&self.home.join("logs"))?;
        mkdirs(&self.launch_agents)?;
        if self.definition_current()? {
            return Ok(false);
        }
        let path = self.plist_path();
        let staged = self
            .launch_agents
            .join(format!(".{}.plist.new", self.label));
        std::fs::write(&staged, self.plist_bytes()?).map_err(|e| io_err(e, &staged))?;
        rename(&staged, &path)?;
        Ok(true)
    }

    /// install.py:563 (DMN-514): an existing LaunchAgent that isn't exactly what setup writes is written again.
    pub fn restore_definition(&self) -> Result<bool> {
        if self.in_app() || !self.plist_path().exists() || self.definition_current()? {
            return Ok(false);
        }
        self.write_plist()
    }

    pub fn labelled_plist(&self) -> bool {
        labelled(&self.plist_path(), &self.label)
    }

    /// install.py:576: launchd lists FERB's label as disabled.
    pub fn disabled(&mut self) -> bool {
        let domain = self.domain();
        let r = self.cmd(&["launchctl", "print-disabled", &domain]);
        if !r.ok() {
            return false;
        }
        for line in py::splitlines(&r.stdout) {
            let line = py::strip(line);
            let (name, state) = match line.split_once("=>") {
                Some((n, s)) => (n, s),
                None => (line, ""),
            };
            if py::strip(name).trim_matches('"') == self.label {
                return matches!(py::strip(state), "disabled" | "true");
            }
        }
        false
    }

    fn printed(&mut self) -> Option<String> {
        let target = self.target();
        let r = self.cmd(&["launchctl", "print", &target]);
        r.ok().then_some(r.stdout)
    }

    pub fn loaded(&mut self) -> bool {
        self.printed().is_some()
    }

    fn path_line(out: &str) -> Option<String> {
        py::splitlines(out)
            .into_iter()
            .find(|l| py::strip(l).starts_with("path = "))
            .map(|l| py::strip(l.split_once('=').map(|x| x.1).unwrap_or("")).to_string())
    }

    fn loaded_from(&mut self, definition: &Path) -> bool {
        let Some(out) = self.printed() else {
            return false;
        };
        match Self::path_line(&out) {
            None => true,
            Some(p) => Path::new(&p) == definition,
        }
    }

    /// install.py:604: for a checkout, launchd runs FERB's label from another definition.
    pub fn foreign_service(&mut self) -> Option<String> {
        let out = self.printed();
        if self.bundle.is_some() {
            return None;
        }
        let path = Self::path_line(&out?)?;
        (Path::new(&path) != self.plist_path()).then_some(path)
    }

    pub fn check_not_foreign(&mut self, then: &str) -> Result<()> {
        if let Some(refusal) = self.foreign_service() {
            return Err(InstallError::Install(format!(
                "FERB.app's background service is registered ({refusal}); open FERB.app once (it moves it to a LaunchAgent) or turn it off in FERB.app (Settings → Advanced → Uninstall FERB…) {then}"
            )));
        }
        Ok(())
    }

    /// install.py:623. A `pid = ` line that isn't a number is an error, as Python's `int()`.
    pub fn agent_pid(&mut self) -> Result<Option<i64>> {
        let out = self.printed().unwrap_or_default();
        for line in py::splitlines(&out) {
            if py::strip(line).starts_with("pid = ") {
                let field = line.split('=').nth(1).unwrap_or("");
                let text = py::strip(field);
                return text
                    .parse::<i64>()
                    .map(Some)
                    .map_err(|_| InstallError::Other {
                        type_name: "ValueError".into(),
                        message: format!(
                            "invalid literal for int() with base 10: {}",
                            py::py_repr_str(field)
                        ),
                    });
            }
        }
        Ok(None)
    }

    /// install.py:630.
    pub fn load(&mut self) -> Result<()> {
        if self.disabled() {
            return Err(InstallError::SwitchedOff(SWITCHED_OFF_HOW.into()));
        }
        let end = self.clock.monotonic() + 15.0;
        loop {
            self.unload()?;
            let domain = self.domain();
            let plist = self.plist_path().to_string_lossy().into_owned();
            let r = self.cmd(&["launchctl", "bootstrap", &domain, &plist]);
            if r.ok() {
                return Ok(());
            }
            let held = self.loaded() && !self.loaded_from(&self.plist_path());
            if !held || self.clock.monotonic() >= end {
                return Err(InstallError::Install(format!(
                    "loading the LaunchAgent failed: {}",
                    launchctl_words(&r)
                )));
            }
            self.clock.sleep(0.5);
        }
    }

    /// install.py:651.
    pub fn unload(&mut self) -> Result<()> {
        self.stopped = self.agent_pid()?;
        let target = self.target();
        self.cmd(&["launchctl", "bootout", &target]);
        let end = self.clock.monotonic() + 10.0;
        while (self.loaded() || self.exiting().is_some()) && self.clock.monotonic() < end {
            self.clock.sleep(0.1);
        }
        Ok(())
    }

    /// install.py:661: FERB.app's registered service stopped but kept registered.
    pub fn stop(&mut self) -> Result<()> {
        self.stopped = self.agent_pid()?;
        if self.stopped.is_none() {
            return Ok(());
        }
        let target = self.target();
        self.cmd(&["launchctl", "kill", "SIGTERM", &target]);
        let end = self.clock.monotonic() + 10.0;
        while (self.agent_pid()?.is_some() || self.exiting().is_some())
            && self.clock.monotonic() < end
        {
            self.clock.sleep(0.1);
        }
        Ok(())
    }

    pub fn start(&mut self) -> Result<()> {
        if self.loaded() {
            let target = self.target();
            let r = self.cmd(&["launchctl", "kickstart", &target]);
            if !r.ok() {
                return Err(InstallError::Install(format!(
                    "starting the daemon failed: {}",
                    launchctl_words(&r)
                )));
            }
        }
        Ok(())
    }

    /// install.py:680.
    pub fn retire_launch_agent(&mut self) -> Result<bool> {
        if !self.labelled_plist() {
            return Ok(false);
        }
        if self.loaded() && self.loaded_from(&self.plist_path()) {
            self.unload()?;
            self.check_exited()?;
        }
        unlink_missing_ok(&self.plist_path());
        Ok(true)
    }

    pub fn running(&mut self) -> Result<bool> {
        Ok(self.agent_pid()?.is_some())
    }

    /// install.py:696.
    pub fn exiting(&mut self) -> Option<i64> {
        let pid = self.stopped?;
        let r = self.cmd_owned(vec!["/bin/kill".into(), "-0".into(), pid.to_string()]);
        r.ok().then_some(pid)
    }

    /// install.py:702.
    pub fn restart(&mut self) -> Result<()> {
        let target = self.target();
        let r = self.cmd(&["launchctl", "kickstart", "-k", &target]);
        if r.ok() {
            return Ok(());
        }
        let kicked = launchctl_words(&r);
        match self.load() {
            Ok(()) => Ok(()),
            Err(InstallError::Install(e)) => Err(InstallError::Install(format!(
                "restarting the daemon failed ({kicked}), and then {e}"
            ))),
            Err(e) => Err(e),
        }
    }

    pub fn installed(&self) -> bool {
        if self.bundle.is_some() {
            return self.record_path().exists() || self.labelled_plist();
        }
        match &self.programs {
            Programs::Python => self.venv_python().exists(),
            Programs::Rust(r) => r.checkout_daemon.exists(),
        }
    }

    /// install.py:725.
    pub fn install(&mut self, update_only: bool, while_stopped: Option<Work<'_>>) -> Result<()> {
        if self.bundle.is_some() {
            self.check_bundle()?;
            self.write_marker()?;
            return self.install_from_bundle(while_stopped);
        }
        if let Programs::Rust(r) = &self.programs {
            let r = r.clone();
            return self.install_rust_checkout(&r, update_only, while_stopped);
        }
        self.recover_venv()?;
        if update_only && !self.installed() {
            return Err(InstallError::Install(
                "FERB isn't installed yet: run `ferb setup` first".into(),
            ));
        }
        self.check_not_foreign("before running FERB from this checkout")?;
        self.write_marker()?;
        let interpreter = self.ensure_interpreter()?;
        let steps = (|| -> Result<()> {
            self.stage_venv(&interpreter)?;
            self.install_secret_helper()?;
            if self.codex_wanted()? {
                self.ensure_codex()?;
            }
            self.write_native_host()?;
            self.install_files()?;
            self.ensure_embedding_model();
            Ok(())
        })();
        if let Err(e) = steps {
            let _ = remove(&self.staged_venv());
            return Err(e);
        }
        self.write_plist()?;
        let mut work = while_stopped;
        // `while_stopped` with the venv swap as its work (a checkout's service is never FERB.app's registration).
        if self.loaded() {
            self.unload()?;
            let done = self.check_exited().and_then(|_| {
                self.swap_venv()?;
                match work.as_mut() {
                    Some(w) => w(),
                    None => Ok(()),
                }
            });
            let loaded = self.load();
            return finally(done, loaded);
        }
        let switched = self.swap_venv().and_then(|_| match work.as_mut() {
            Some(w) => w(),
            None => Ok(()),
        });
        let loaded = self.load();
        finally(switched, loaded)
    }

    /// A checkout's install of the Rust build: the checkout's built `ferbd` and `ferb-native-host` run in place (they
    /// play the venv's part: nothing is staged or swapped), with the same steps around them as a Python checkout's
    /// install (install.py:725): the secret helper, Codex when a role uses it, the native host, FERB's extension, the
    /// embedding model, the LaunchAgent and its load. A binary that isn't built is refused before anything changes.
    fn install_rust_checkout(
        &mut self,
        r: &RustPrograms,
        update_only: bool,
        while_stopped: Option<Work<'_>>,
    ) -> Result<()> {
        if update_only && !self.installed() {
            return Err(InstallError::Install(
                "FERB isn't installed yet: run `ferb setup` first".into(),
            ));
        }
        for built in [&r.checkout_daemon, &r.checkout_native_host] {
            if !built.is_file() {
                return Err(InstallError::Install(format!(
                    "{} isn't built: build the Rust programs first (cargo build --release -p ferbd -p ferb-native-host)",
                    built.display()
                )));
            }
        }
        self.check_not_foreign("before running FERB from this checkout")?;
        self.write_marker()?;
        self.install_secret_helper()?;
        if self.codex_wanted()? {
            self.ensure_codex()?;
        }
        self.write_native_host()?;
        self.install_files()?;
        self.ensure_embedding_model();
        self.write_plist()?;
        let mut work = while_stopped;
        if self.loaded() {
            self.unload()?;
            let done = self.check_exited().and_then(|_| match work.as_mut() {
                Some(w) => w(),
                None => Ok(()),
            });
            let loaded = self.load();
            return finally(done, loaded);
        }
        let done = match work.as_mut() {
            Some(w) => w(),
            None => Ok(()),
        };
        let loaded = self.load();
        finally(done, loaded)
    }

    /// install.py:767.
    fn install_from_bundle(&mut self, while_stopped: Option<Work<'_>>) -> Result<()> {
        self.install_files()?;
        self.install_secret_helper()?;
        if self.codex_wanted()? {
            self.ensure_codex()?;
        }
        self.write_native_host()?;
        self.link_cli()?;
        if self.in_app() {
            self.retire_launch_agent()?;
            self.write_record()?;
            if let Some(w) = while_stopped {
                return self.while_stopped(w);
            } else if self.loaded() {
                let target = self.target();
                let r = self.cmd(&["launchctl", "kickstart", "-k", &target]);
                if !r.ok() {
                    self.notes.push(format!(
                        "restarting the daemon failed: {}",
                        launchctl_words(&r)
                    ));
                }
            }
            return Ok(());
        }
        let changed = self.write_plist()?;
        self.write_record()?;
        if let Some(w) = while_stopped {
            self.while_stopped(w)
        } else if changed || !self.loaded_from(&self.plist_path()) {
            self.load()
        } else {
            self.restart()
        }
    }

    /// install.py:798: `work` with the daemon stopped, then started again whatever `work` does.
    pub fn while_stopped(&mut self, work: Work<'_>) -> Result<()> {
        self.pause_service()?;
        let done = self.check_exited().and_then(|_| work());
        let resumed = self.resume_service();
        finally(done, resumed)
    }

    /// `while_stopped`'s first half: FERB.app's registration stopped (kept registered), else the LaunchAgent unloaded.
    pub fn pause_service(&mut self) -> Result<()> {
        if self.in_app() {
            self.stop()
        } else {
            self.unload()
        }
    }

    /// `while_stopped`'s second half, run whatever the work did.
    pub fn resume_service(&mut self) -> Result<()> {
        if self.in_app() {
            self.start()
        } else {
            self.load()
        }
    }

    /// The process the last stop waited for still runs: nothing may run that needs it gone.
    pub fn check_exited(&mut self) -> Result<()> {
        if let Some(pid) = self.exiting() {
            return Err(InstallError::Install(format!(
                "FERB's daemon (process {pid}) hasn't exited since launchd stopped it"
            )));
        }
        Ok(())
    }

    /// install.py:824.
    pub fn check_bundle(&mut self) -> Result<()> {
        let bundle = self
            .bundle
            .clone()
            .expect("check_bundle runs for FERB.app only");
        if bundle
            .components()
            .any(|c| c.as_os_str() == "AppTranslocation")
        {
            return Err(InstallError::Install(
                "macOS is running FERB from a temporary copy: quit FERB, move FERB.app to Applications in Finder, then open it again".into(),
            ));
        }
        if (self.read_only)(&bundle) {
            return Err(InstallError::Install(
                "FERB is running from its disk image: drag FERB.app to Applications, then open it from there".into(),
            ));
        }
        let parts: Vec<PathBuf> = match &self.programs {
            Programs::Python => vec![BUNDLE_AGENT.into(), BUNDLE_PYTHON.into()],
            Programs::Rust(r) => vec![r.bundle_daemon.clone(), r.bundle_native_host.clone()],
        };
        for part in parts {
            if !bundle.join(&part).is_file() {
                return Err(InstallError::Install(format!(
                    "{} is incomplete (no {}); download FERB again",
                    bundle.display(),
                    part.display()
                )));
            }
        }
        let b = bundle.to_string_lossy().into_owned();
        self.ok(
            &["/usr/bin/codesign", "--verify", "--deep", "--strict", &b],
            "checking FERB.app's signature (download FERB again if it was damaged)",
        )
    }

    /// install.py:840: the requirement to pin for FERB.app itself.
    pub fn own_requirement(&mut self) -> Result<String> {
        let b = self
            .bundle
            .clone()
            .expect("own_requirement runs for FERB.app only")
            .to_string_lossy()
            .into_owned();
        let shown = self.cmd(&["/usr/bin/codesign", "-d", "-r-", &b]);
        let requirement = designated_requirement(&shown.both());
        if !trusted_requirement(&requirement, true) {
            return Err(InstallError::Install(
                "FERB.app's signature names neither its exact code nor a certificate; download FERB again".into(),
            ));
        }
        Ok(requirement)
    }

    /// install.py:851.
    pub fn install_secret_helper(&mut self) -> Result<()> {
        let binary = self.home.join("bin/ferb-secrets");
        mkdirs(&self.home.join("bin"))?;
        let staged = self.home.join("bin/.ferb-secrets.new");
        if let Some(bundle) = self.bundle.clone() {
            copyfile(&bundle.join(BUNDLE_SECRET_HELPER), &staged)?;
            let requirement = match self.sealed_requirement(&staged, "Helpers/ferb-secrets") {
                Ok(r) => r,
                Err(e) => {
                    unlink_missing_ok(&staged);
                    return Err(e);
                }
            };
            return swap_helper(&staged, &binary, &requirement);
        }
        let source = self.sources.helper_source.to_string_lossy().into_owned();
        let out = staged.to_string_lossy().into_owned();
        let built = (|| -> Result<String> {
            self.ok(
                &[
                    "xcrun",
                    "swiftc",
                    "-O",
                    "-o",
                    &out,
                    &source,
                    "-framework",
                    "Security",
                    "-framework",
                    "CryptoKit",
                ],
                "building the secret helper",
            )?;
            self.ok(
                &["/usr/bin/codesign", "--force", "--sign", "FERB Local Code Signing", "--identifier", "com.ferb.secrets", &out],
                "signing the secret helper (create FERB Local Code Signing in Keychain Access if missing)",
            )?;
            let shown = self.cmd(&["/usr/bin/codesign", "-d", "-r-", &out]);
            let requirement = designated_requirement(&shown.both());
            if !trusted_requirement(&requirement, false) {
                return Err(InstallError::Install(
                    "the secret helper lacks a stable certificate identity".into(),
                ));
            }
            Ok(requirement)
        })();
        match built {
            Ok(requirement) => swap_helper(&staged, &binary, &requirement),
            Err(e) => {
                unlink_missing_ok(&staged);
                Err(e)
            }
        }
    }

    /// install.py:884.
    pub fn sealed_requirement(&mut self, code: &Path, nested: &str) -> Result<String> {
        let bundle = self
            .bundle
            .clone()
            .expect("sealed_requirement runs for FERB.app only");
        let name = code
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let sealed = sealed_cdhash(&bundle, nested);
        let c = code.to_string_lossy().into_owned();
        let shown = self.cmd(&["/usr/bin/codesign", "-dv", "--verbose=4", &c]);
        let Some(sealed) =
            sealed.filter(|s| !s.is_empty() && codesign_field(&shown.both(), "CDHash") == *s)
        else {
            return Err(InstallError::Install(format!(
                "{name} isn't the one FERB.app's signature seals; download FERB again"
            )));
        };
        let shown = self.cmd(&["/usr/bin/codesign", "-d", "-r-", &c]);
        let requirement = designated_requirement(&shown.both());
        let b = bundle.to_string_lossy().into_owned();
        let app = self.cmd(&["/usr/bin/codesign", "-dv", &b]);
        let team = codesign_field(&app.both(), "TeamIdentifier");
        if requirement == format!("cdhash H\"{sealed}\"") {
            return Ok(requirement);
        }
        let own = self.cmd(&["/usr/bin/codesign", "-d", "-r-", &b]);
        let leaf = certificate_leaf(&designated_requirement(&own.both()));
        if leaf.is_some() && certificate_leaf(&requirement) == leaf {
            return Ok(requirement);
        }
        if !team.is_empty()
            && team != "not set"
            && requirement.contains("anchor apple generic")
            && names_team(&requirement, &team)
        {
            return Ok(requirement);
        }
        Err(InstallError::Install(format!(
            "{name} is pinned by neither its exact code hash nor FERB.app's own certificate or Developer ID team"
        )))
    }

    // ---------- native host ----------

    /// install.py:936.
    pub fn write_native_host(&mut self) -> Result<Vec<PathBuf>> {
        if self.native_hosts_dirs.is_empty() {
            return Ok(Vec::new());
        }
        let (written, failed) = write_native_hosts(
            &self.home,
            &self.native_host_launcher(),
            &self.native_host_exec(),
            &self.extension_ids,
            &self.native_hosts_dirs,
        )?;
        for (d, why) in failed {
            self.notes.push(format!(
                "Couldn't connect FERB to a browser: {} ({why}).",
                d.display()
            ));
        }
        Ok(written)
    }

    // ---------- `ferb` on PATH ----------

    /// A link FERB.app made: it points at some FERB.app's launcher.
    pub fn app_link(path: &Path) -> bool {
        if !is_symlink(path) {
            return false;
        }
        let Ok(target) = std::fs::read_link(path) else {
            return false;
        };
        let parts: Vec<String> = target
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        parts.len() >= 4 && parts[parts.len() - 4..] == ["Contents", "Resources", "bin", "ferb"]
    }

    /// install.py:959.
    pub fn link_cli(&self) -> Result<()> {
        let (Some(bundle), Some(link)) = (&self.bundle, self.cli_link()) else {
            return Ok(());
        };
        if lexists(&link) && !Self::app_link(&link) {
            return Ok(());
        }
        let parent = link.parent().expect("the link has a folder");
        mkdirs(parent)?;
        let staged = parent.join(".ferb.link-new");
        unlink_missing_ok(&staged);
        std::os::unix::fs::symlink(bundle.join(BUNDLE_CLI), &staged)
            .map_err(|e| io_err(e, &staged))?;
        rename(&staged, &link)
    }

    /// install.py:972.
    pub fn cli_status(&self) -> Option<String> {
        let (Some(bundle), Some(link)) = (&self.bundle, self.cli_link()) else {
            return None;
        };
        let target = bundle.join(BUNDLE_CLI);
        if !(is_symlink(&link) && py::resolve(&link) == py::resolve(&target)) {
            return Some(format!(
                "`ferb` command: left {} alone (it isn't a link FERB made); FERB's own is {}",
                link.display(),
                target.display()
            ));
        }
        Some(format!("`ferb` command: {}", link.display()))
    }

    // ---------- uninstall ----------

    /// install.py:985.
    pub fn uninstall(&mut self, purge: bool) -> Result<Vec<String>> {
        self.check_not_foreign("before uninstalling FERB from this checkout")?;
        let mut removed = Vec::new();
        if self.loaded() {
            self.unload()?;
            removed.push(format!("background service {} (stopped)", self.label));
        }
        if self.labelled_plist() {
            let p = self.plist_path();
            std::fs::remove_file(&p).map_err(|e| io_err(e, &p))?;
            removed.push(p.to_string_lossy().into_owned());
        }
        let login = self.launch_agents.join(format!("{APP_LOGIN_LABEL}.plist"));
        if labelled(&login, APP_LOGIN_LABEL) {
            std::fs::remove_file(&login).map_err(|e| io_err(e, &login))?;
            removed.push(login.to_string_lossy().into_owned());
        }
        for d in self.native_hosts_dirs.clone() {
            let m = d.join(NATIVE_HOST_MANIFEST);
            if m.exists() {
                std::fs::remove_file(&m).map_err(|e| io_err(e, &m))?;
                removed.push(m.to_string_lossy().into_owned());
            }
        }
        if let Some(link) = self.cli_link() {
            if Self::app_link(&link) {
                std::fs::remove_file(&link).map_err(|e| io_err(e, &link))?;
                removed.push(link.to_string_lossy().into_owned());
            }
        }
        let record = self.record_path();
        if record.is_file() {
            std::fs::remove_file(&record).map_err(|e| io_err(e, &record))?;
            removed.push(record.to_string_lossy().into_owned());
        }
        if purge {
            self.check_purge_target()?;
            for (service, count) in self.remove_keychain_items() {
                if count > 0 {
                    removed.push(format!("Keychain items of {service}: {count}"));
                }
            }
            std::fs::remove_dir_all(&self.home).map_err(|e| io_err(e, &self.home))?;
            removed.push(self.home.to_string_lossy().into_owned());
        }
        Ok(removed)
    }

    /// The Keychain service Claude Code uses for FERB's own Claude home (claude_login.keychain_service).
    pub fn claude_keychain_service(&self) -> String {
        let home = self.home.join(CLAUDE_HOME);
        let digest = hex::encode(Sha256::digest(home.to_string_lossy().as_bytes()));
        format!("Claude Code-credentials-{}", &digest[..8])
    }

    pub fn keychain_services(&self) -> Vec<String> {
        let mut v: Vec<String> = KEYCHAIN_SERVICES.iter().map(|s| s.to_string()).collect();
        v.push(self.claude_keychain_service());
        v
    }

    /// install.py:1027 (DMN-20): each service's items deleted one at a time, never read or listed.
    pub fn remove_keychain_items(&mut self) -> Vec<(String, usize)> {
        let mut counts = Vec::new();
        for service in self.keychain_services() {
            let mut count = 0;
            while count < KEYCHAIN_MAX_ITEMS
                && self
                    .cmd(&[
                        "/usr/bin/security",
                        "delete-generic-password",
                        "-s",
                        &service,
                    ])
                    .ok()
            {
                count += 1;
            }
            counts.push((service, count));
        }
        counts
    }

    /// install.py:1040.
    pub fn check_purge_target(&self) -> Result<()> {
        let home = py::resolve(&self.home);
        let forbidden = [
            py::resolve(Path::new("/")),
            py::resolve(&self.user_home),
            py::resolve(&self.user_home.join("Desktop")),
        ];
        if forbidden.contains(&home) || home.components().count() < 3 {
            return Err(InstallError::Install(format!(
                "refusing to delete {}",
                home.display()
            )));
        }
        if !home.join(MARKER).is_file() {
            return Err(InstallError::Install(format!(
                "refusing to delete {}: no {MARKER} marker (not a FERB home made by setup)",
                home.display()
            )));
        }
        Ok(())
    }

    fn ok(&mut self, cmd: &[&str], what: &str) -> Result<()> {
        let r = self.cmd(cmd);
        if r.ok() {
            return Ok(());
        }
        let err = py::strip(&r.stderr);
        let said = py::last_chars(
            if err.is_empty() {
                py::strip(&r.stdout)
            } else {
                err
            },
            400,
        );
        let said = if said.is_empty() {
            format!("{} exited {} and printed nothing", cmd[0], r.code)
        } else {
            said.to_string()
        };
        Err(InstallError::Install(format!("{what} failed: {said}")))
    }
}

/// `try: body finally: cleanup`: the clean-up's error wins, else the body's.
fn finally(body: Result<()>, cleanup: Result<()>) -> Result<()> {
    cleanup?;
    body
}

/// install.py:912: the helper and its pin written whole beside the ones in use, then renamed one after the other.
fn swap_helper(staged: &Path, binary: &Path, requirement: &str) -> Result<()> {
    let pin = binary.with_extension("requirement");
    let staged_pin = binary.with_file_name(".ferb-secrets.requirement.new");
    let written = chmod(staged, 0o700)
        .and_then(|_| {
            std::fs::write(&staged_pin, format!("{requirement}\n"))
                .map_err(|e| io_err(e, &staged_pin))
        })
        .and_then(|_| chmod(&staged_pin, 0o600));
    if let Err(e) = written {
        unlink_missing_ok(staged);
        unlink_missing_ok(&staged_pin);
        return Err(e);
    }
    rename(staged, binary)?;
    rename(&staged_pin, &pin)
}
