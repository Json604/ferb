//! The check that a socket peer is FERB.app (kb area app: the live preview is FERB.app's alone, not every
//! program running as the owner). Port of `ferb/daemon/app_peer.py`. It grants nothing else: `auth::METHODS`'
//! `app_only` names what needs it (DMN-23, DMN-402).
//!
//! The kernel's audit token of the connected process (`peer::peer_token`) is handed to the Security framework,
//! which checks the running code's signature against a requirement: FERB.app's own designated requirement (its
//! exact code hash for an ad-hoc build, its certificate or Developer ID team otherwise), so only that app,
//! signed as it is, passes. Anything that can't be checked fails closed.
//!
//! # Which FERB.app (`Mode`)
//!
//! Python chose from `sys.prefix` (FERB.app's bundled Python, FERB's own venv, or a checkout's `.venv`). A Rust
//! daemon has no prefix, so the rule here reads the daemon executable's own resolved path (`mode_for`):
//!
//! - **Bundled**: the executable is inside an app bundle (`<X>.app/Contents/…`): the outermost such bundle is
//!   FERB.app (the daemon's agent app sits inside FERB.app, so the outermost one is the app the owner sees).
//! - **Checkout**: the executable was built by cargo in a source checkout: it is under a folder named `target`
//!   whose parent holds a `Cargo.toml`, and not under FERB's home. Only then are the notch's identifiers
//!   accepted, which an owner-run program could also claim (the developer toolbox).
//! - **Installed**: anything else, including a path that can't be resolved: the installed FERB.app
//!   (/Applications/FERB.app, then ~/Applications/FERB.app, with FERB's bundle id); none found: nobody passes.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use crate::peer::AuditToken;

/// What a daemon run from a source checkout accepts (app_peer.py:23).
pub const DEV_REQUIREMENT: &str = r#"identifier "com.ferb.FERB" or identifier "dev.ferb.notch""#;
/// FERB.app's bundle id (`ferb/paths.py:27`, ui/mac/FERBNotch/Info.plist).
pub const APP_ID: &str = "com.ferb.FERB";

/// The code-signature calls the check needs; `security::SecurityFramework` in the daemon, a fake in tests.
/// Each answer is `None`/`false` when it can't be had.
pub trait Security {
    type Requirement;

    /// The requirement this text states.
    fn requirement(&self, text: &str) -> Option<Self::Requirement>;
    /// The designated requirement of the app at `bundle`, as its signature states it.
    fn designated(&self, bundle: &Path) -> Option<Self::Requirement>;
    /// The running process with this audit token is validly signed and satisfies the requirement.
    fn satisfies(&self, token: &AuditToken, requirement: &Self::Requirement) -> bool;
}

/// Which FERB.app's signature counts; see the module's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The daemon runs from inside this FERB.app.
    Bundled(PathBuf),
    /// The daemon runs from somewhere else: the installed FERB.app.
    Installed,
    /// The daemon runs from a source checkout's build folder: the notch's identifiers.
    Checkout,
}

/// The mode of a daemon whose executable is at `executable`, with FERB's data in `ferb_home`.
pub fn mode_for(executable: &Path, ferb_home: &Path) -> Mode {
    let Ok(executable) = executable.canonicalize() else {
        return Mode::Installed;
    };
    if let Some(bundle) = enclosing_app(&executable) {
        return Mode::Bundled(bundle);
    }
    if in_checkout(&executable, ferb_home) {
        Mode::Checkout
    } else {
        Mode::Installed
    }
}

/// The mode of this process (`std::env::current_exe`); `Installed` when the system can't say where it runs
/// from.
pub fn mode_of_this_process(ferb_home: &Path) -> Mode {
    match std::env::current_exe() {
        Ok(executable) => mode_for(&executable, ferb_home),
        Err(_) => Mode::Installed,
    }
}

/// The outermost `<X>.app` that has `executable` under its `Contents` folder.
fn enclosing_app(executable: &Path) -> Option<PathBuf> {
    let mut bundle = PathBuf::new();
    let mut parts = executable.components().peekable();
    while let Some(part) = parts.next() {
        bundle.push(part);
        let is_app = bundle.extension().is_some_and(|ext| ext == "app");
        if is_app
            && parts
                .peek()
                .is_some_and(|next| next.as_os_str() == "Contents")
        {
            return Some(bundle);
        }
    }
    None
}

/// A source checkout's own build (`<workspace>/target/…` beside the workspace's Cargo.toml), never anything
/// under FERB's home (app_peer.py:167-174 has the same shape for `<repo>/.venv` beside pyproject.toml).
fn in_checkout(executable: &Path, ferb_home: &Path) -> bool {
    // FERB's home may not exist yet; then nothing is under it. One that can't be resolved for another reason:
    // not a checkout.
    match ferb_home.canonicalize() {
        Ok(home) if executable.starts_with(&home) => return false,
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if executable.starts_with(ferb_home) {
                return false;
            }
        }
        Err(_) => return false,
    }
    executable.ancestors().skip(1).any(|folder| {
        folder.file_name().is_some_and(|name| name == "target")
            && folder
                .parent()
                .is_some_and(|workspace| workspace.join("Cargo.toml").is_file())
    })
}

/// The installed FERB.app, for a daemon that doesn't run from one (setup records no location): the first
/// candidate with FERB's bundle id (`ferb/paths.py:30-42`).
pub fn installed_app(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|app| info(app, "CFBundleIdentifier").as_deref() == Some(APP_ID))
        .cloned()
}

/// /Applications/FERB.app, then ~/Applications/FERB.app.
pub fn default_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![PathBuf::from("/Applications/FERB.app")];
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(Path::new(&home).join("Applications").join("FERB.app"));
    }
    candidates
}

/// One text value of the bundle's Info.plist (XML or binary); `None` when it can't be read or isn't text.
fn info(app: &Path, key: &str) -> Option<String> {
    let plist = plist::Value::from_file(app.join("Contents").join("Info.plist")).ok()?;
    Some(plist.as_dictionary()?.get(key)?.as_string()?.to_owned())
}

/// What changes when the bundle is re-signed or replaced: its CodeResources and main executable (path, inode,
/// size, modification time); `None` when it can't be read (app_peer.py:177-189).
type Stamp = Vec<(PathBuf, u64, u64, i128)>;

fn signature_stamp(app: &Path) -> Option<Stamp> {
    let contents = app.join("Contents");
    // Python: `str(plist.get("CFBundleExecutable") or "")`, after the plist itself was read
    let plist = plist::Value::from_file(contents.join("Info.plist")).ok()?;
    let executable = plist
        .as_dictionary()?
        .get("CFBundleExecutable")
        .and_then(|v| v.as_string())
        .unwrap_or("");
    let mut found = Vec::new();
    for path in [
        contents.join("_CodeSignature").join("CodeResources"),
        contents.join("MacOS").join(executable),
    ] {
        let stat = std::fs::metadata(&path).ok()?;
        let modified = i128::from(stat.mtime()) * 1_000_000_000 + i128::from(stat.mtime_nsec());
        found.push((path, stat.ino(), stat.size(), modified));
    }
    Some(found)
}

/// `AppPeers::check(token)`: whether the process behind this audit token is FERB.app, by its code signature
/// against FERB.app's own designated requirement (app_peer.py:104-164). The requirement is derived again
/// whenever the bundle's signature changes (its CodeResources or executable), so an app update never locks the
/// notch out until a daemon restart. A token's verdict is kept until then (a token names one process).
pub struct AppPeers<S: Security> {
    mode: Mode,
    candidates: Vec<PathBuf>,
    security: S,
    requirement: Option<S::Requirement>,
    /// The signature the requirement was derived from; the outer `None` is "never derived".
    stamp: Option<Option<Stamp>>,
    seen: HashMap<AuditToken, bool>,
}

impl<S: Security> AppPeers<S> {
    pub fn new(mode: Mode, security: S) -> Self {
        Self::with_candidates(mode, security, default_candidates())
    }

    /// With other places to look for the installed FERB.app (tests).
    pub fn with_candidates(mode: Mode, security: S, candidates: Vec<PathBuf>) -> Self {
        AppPeers {
            mode,
            candidates,
            security,
            requirement: None,
            stamp: None,
            seen: HashMap::new(),
        }
    }

    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    /// The FERB.app whose signature counts (`None`: checkout mode, or no installed FERB.app).
    fn app(&self) -> Option<PathBuf> {
        match &self.mode {
            Mode::Bundled(bundle) => Some(bundle.clone()),
            Mode::Installed => installed_app(&self.candidates),
            Mode::Checkout => None,
        }
    }

    fn derive(&mut self) {
        let app = self.app();
        let stamp = app.as_deref().and_then(signature_stamp);
        if self.stamp.as_ref() == Some(&stamp) {
            return;
        }
        // first use, or FERB.app was updated, moved or removed
        self.stamp = Some(stamp);
        self.seen.clear();
        self.requirement = match (&self.mode, app) {
            (Mode::Checkout, _) => self.security.requirement(DEV_REQUIREMENT),
            (_, Some(app)) => self.security.designated(&app),
            (_, None) => None,
        };
    }

    /// Whether the process behind `token` is FERB.app. No token, no requirement, or no valid signature: no.
    pub fn check(&mut self, token: Option<&AuditToken>) -> bool {
        let Some(token) = token else {
            return false;
        };
        self.derive();
        if let Some(verdict) = self.seen.get(token) {
            return *verdict;
        }
        let verdict = self
            .requirement
            .as_ref()
            .is_some_and(|required| self.security.satisfies(token, required));
        self.seen.insert(*token, verdict);
        verdict
    }

    /// The check as `Server::set_is_app` takes it.
    pub fn into_check(self) -> impl Fn(Option<AuditToken>) -> bool + Send + Sync + 'static
    where
        S: Send + 'static,
        S::Requirement: Send,
    {
        let peers = Mutex::new(self);
        move |token| {
            peers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .check(token.as_ref())
        }
    }
}

/// The check the daemon installs: this process's mode, the real Security framework.
pub fn system(ferb_home: &Path) -> AppPeers<crate::security::SecurityFramework> {
    AppPeers::new(
        mode_of_this_process(ferb_home),
        crate::security::SecurityFramework,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::fs;
    use std::rc::Rc;

    /// Requirements are their text ("designated <bundle path>#<n>" or the text given); a token satisfies the
    /// requirement the test says its process was signed to.
    #[derive(Clone, Default)]
    struct Fake {
        signed: Rc<RefCell<HashMap<AuditToken, String>>>,
        unsigned_bundles: Rc<RefCell<Vec<PathBuf>>>,
        asked: Rc<RefCell<Vec<String>>>,
    }

    impl Security for Fake {
        type Requirement = String;

        fn requirement(&self, text: &str) -> Option<String> {
            self.asked.borrow_mut().push(format!("requirement {text}"));
            Some(text.to_owned())
        }

        fn designated(&self, bundle: &Path) -> Option<String> {
            self.asked
                .borrow_mut()
                .push(format!("designated {}", bundle.display()));
            if self.unsigned_bundles.borrow().iter().any(|b| b == bundle) {
                return None;
            }
            Some(format!("designated {}", bundle.display()))
        }

        fn satisfies(&self, token: &AuditToken, requirement: &String) -> bool {
            self.asked.borrow_mut().push("satisfies".to_owned());
            self.signed.borrow().get(token) == Some(requirement)
        }
    }

    const APP: AuditToken = [1; 32];
    const OTHER: AuditToken = [2; 32];

    fn bundle(root: &Path, name: &str, id: &str) -> PathBuf {
        let app = root.join(name);
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::create_dir_all(app.join("Contents/_CodeSignature")).unwrap();
        fs::write(
            app.join("Contents/Info.plist"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
                 <key>CFBundleIdentifier</key><string>{id}</string>\
                 <key>CFBundleExecutable</key><string>FERB</string></dict></plist>\n"
            ),
        )
        .unwrap();
        fs::write(app.join("Contents/MacOS/FERB"), b"binary").unwrap();
        fs::write(app.join("Contents/_CodeSignature/CodeResources"), b"seal 1").unwrap();
        app
    }

    fn signed_as(fake: &Fake, token: AuditToken, requirement: &str) {
        fake.signed
            .borrow_mut()
            .insert(token, requirement.to_owned());
    }

    // decision: DMN-402
    #[test]
    fn bundled_only_a_process_signed_as_that_bundle_passes() {
        let dir = tempfile::tempdir().unwrap();
        let app = bundle(dir.path(), "FERB.app", APP_ID);
        let fake = Fake::default();
        signed_as(&fake, APP, &format!("designated {}", app.display()));
        signed_as(&fake, OTHER, DEV_REQUIREMENT); // claims the notch's identifier: not enough
        let mut peers = AppPeers::with_candidates(Mode::Bundled(app), fake.clone(), vec![]);
        assert!(peers.check(Some(&APP)));
        assert!(!peers.check(Some(&OTHER)));
        assert!(!peers.check(None));
        assert!(!peers.check(Some(&[9; 32])));
    }

    // decision: DMN-402
    #[test]
    fn a_verdict_is_kept_per_token_until_the_bundle_s_signature_changes() {
        let dir = tempfile::tempdir().unwrap();
        let app = bundle(dir.path(), "FERB.app", APP_ID);
        let fake = Fake::default();
        signed_as(&fake, APP, &format!("designated {}", app.display()));
        let mut peers = AppPeers::with_candidates(Mode::Bundled(app.clone()), fake.clone(), vec![]);
        assert!(peers.check(Some(&APP)) && peers.check(Some(&APP)));
        let asked = fake.asked.borrow().clone();
        assert_eq!(
            asked,
            [
                format!("designated {}", app.display()),
                "satisfies".to_owned()
            ]
        );
        // the app is updated: a new seal, and the old process no longer matches what the framework says
        fs::write(
            app.join("Contents/_CodeSignature/CodeResources"),
            b"seal 2, longer",
        )
        .unwrap();
        fake.signed.borrow_mut().clear();
        assert!(!peers.check(Some(&APP)));
        assert_eq!(fake.asked.borrow().len(), 4); // derived again, asked again
    }

    // decision: DMN-402
    #[test]
    fn installed_the_first_candidate_with_ferb_s_bundle_id_counts_and_none_means_nobody() {
        let dir = tempfile::tempdir().unwrap();
        let imposter = bundle(&dir.path().join("a"), "FERB.app", "com.example.other");
        let real = bundle(&dir.path().join("b"), "FERB.app", APP_ID);
        let missing = dir.path().join("c/FERB.app");
        let fake = Fake::default();
        signed_as(&fake, APP, &format!("designated {}", real.display()));
        signed_as(&fake, OTHER, &format!("designated {}", imposter.display()));
        let candidates = vec![missing.clone(), imposter.clone(), real.clone()];
        assert_eq!(installed_app(&candidates), Some(real.clone()));
        let mut peers = AppPeers::with_candidates(Mode::Installed, fake.clone(), candidates);
        assert!(peers.check(Some(&APP)));
        assert!(!peers.check(Some(&OTHER)));

        let mut nobody = AppPeers::with_candidates(
            Mode::Installed,
            fake.clone(),
            vec![missing.clone(), imposter],
        );
        assert!(!nobody.check(Some(&APP)) && !nobody.check(Some(&OTHER)));
        // FERB.app is installed later: the next check finds it, with no restart
        let late = bundle(&dir.path().join("c"), "FERB.app", APP_ID);
        signed_as(&fake, APP, &format!("designated {}", late.display()));
        let mut later = AppPeers::with_candidates(Mode::Installed, fake.clone(), vec![missing]);
        assert!(later.check(Some(&APP)));
    }

    // decision: DMN-402
    #[test]
    fn a_bundle_whose_signature_cannot_be_read_lets_nobody_in() {
        let dir = tempfile::tempdir().unwrap();
        let app = bundle(dir.path(), "FERB.app", APP_ID);
        let fake = Fake::default();
        fake.unsigned_bundles.borrow_mut().push(app.clone());
        signed_as(&fake, APP, &format!("designated {}", app.display()));
        signed_as(&fake, OTHER, DEV_REQUIREMENT);
        let mut peers = AppPeers::with_candidates(Mode::Bundled(app), fake, vec![]);
        assert!(!peers.check(Some(&APP)) && !peers.check(Some(&OTHER)));
    }

    // decision: DMN-402
    #[test]
    fn only_a_checkout_accepts_the_notch_s_identifiers() {
        let fake = Fake::default();
        signed_as(&fake, APP, DEV_REQUIREMENT);
        let mut peers = AppPeers::with_candidates(Mode::Checkout, fake.clone(), vec![]);
        assert!(peers.check(Some(&APP)));
        assert!(!peers.check(Some(&OTHER)));
        assert!(peers.check(Some(&APP)));
        let asked = fake.asked.borrow().clone();
        assert_eq!(
            asked
                .iter()
                .filter(|a| a.starts_with("requirement"))
                .count(),
            1
        ); // derived once
        let mut installed = AppPeers::with_candidates(Mode::Installed, fake, vec![]);
        assert!(!installed.check(Some(&APP)));
    }

    // decision: DMN-402
    #[test]
    fn the_mode_comes_from_where_the_daemon_s_executable_is() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let home = root.join("home/.ferb");
        let touch = |path: PathBuf| {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"x").unwrap();
            path
        };

        // inside FERB.app, directly or in the agent app inside it: that outer bundle
        let app = root.join("Applications/FERB.app");
        let direct = touch(app.join("Contents/MacOS/ferbd"));
        let nested =
            touch(app.join("Contents/Library/LoginItems/FERB Agent.app/Contents/MacOS/ferbd"));
        assert_eq!(mode_for(&direct, &home), Mode::Bundled(app.clone()));
        assert_eq!(mode_for(&nested, &home), Mode::Bundled(app.clone()));
        // a symlink to it resolves to the same place
        let link = root.join("ferbd-link");
        std::os::unix::fs::symlink(&direct, &link).unwrap();
        assert_eq!(mode_for(&link, &home), Mode::Bundled(app));
        // a folder that merely ends in .app, without the bundle layout, is no bundle
        let lookalike = touch(root.join("tools.app/bin/ferbd"));
        assert_eq!(mode_for(&lookalike, &home), Mode::Installed);

        // a cargo build in a checkout: the workspace's target folder
        let workspace = root.join("src/FERB/rust");
        touch(workspace.join("Cargo.toml"));
        for built in [
            "target/debug/ferbd",
            "target/release/ferbd",
            "target/debug/deps/ferbd-0a1b",
        ] {
            assert_eq!(
                mode_for(&touch(workspace.join(built)), &home),
                Mode::Checkout,
                "{built}"
            );
        }
        // a `target` folder with no Cargo.toml beside it, and any other place
        assert_eq!(
            mode_for(&touch(root.join("elsewhere/target/debug/ferbd")), &home),
            Mode::Installed
        );
        assert_eq!(
            mode_for(&touch(root.join("usr/local/bin/ferbd")), &home),
            Mode::Installed
        );
        // never anything under FERB's home, even shaped like a checkout
        touch(home.join("rust/Cargo.toml"));
        assert_eq!(
            mode_for(&touch(home.join("rust/target/release/ferbd")), &home),
            Mode::Installed
        );
        // a path that doesn't exist can't be resolved
        assert_eq!(
            mode_for(&workspace.join("target/debug/gone"), &home),
            Mode::Installed
        );
    }

    #[test]
    fn this_test_binary_runs_from_the_checkout() {
        // FERB's home for the question: a folder that does not exist (never ~/.ferb)
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            mode_of_this_process(&dir.path().join("no-home")),
            Mode::Checkout
        );
    }

    #[test]
    fn a_binary_info_plist_is_read_too() {
        let dir = tempfile::tempdir().unwrap();
        let app = bundle(dir.path(), "FERB.app", "x");
        let mut dict = plist::Dictionary::new();
        dict.insert("CFBundleIdentifier".into(), APP_ID.into());
        dict.insert("CFBundleExecutable".into(), "FERB".into());
        plist::Value::Dictionary(dict)
            .to_file_binary(app.join("Contents/Info.plist"))
            .unwrap();
        assert_eq!(installed_app(std::slice::from_ref(&app)), Some(app.clone()));
        assert!(signature_stamp(&app).is_some());
        fs::remove_file(app.join("Contents/_CodeSignature/CodeResources")).unwrap();
        assert!(signature_stamp(&app).is_none());
    }
}
