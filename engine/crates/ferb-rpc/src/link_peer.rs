//! The check that a socket peer is the phone link's own helper (stage 3, PHN-441): the only caller that may open phone
//! sessions (`Role::Phone`) and make the link's own calls (`Role::Link`). As strong as FERB.app's check
//! ([`crate::app_peer`]): the kernel's audit token of the connected process is handed to the Security framework, which
//! checks the running code's signature against the helper's own designated requirement, read from the helper's
//! signature on disk inside FERB.app ([`HELPER`]); so only that program, signed as it is, passes. Anything that can't be
//! checked fails closed. Its own requirement is not FERB.app's: FERB.app is refused the phone role and the helper is
//! refused FERB.app's methods.
//!
//! Which FERB.app: the same [`Mode`] as the FERB.app check. A daemon run from a source checkout accepts
//! [`DEV_REQUIREMENT`] (the helper's identifier), which an owner-run program could also claim, as the notch's
//! identifiers are there.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use crate::app_peer::{default_candidates, installed_app, AuditToken, Mode, Security};

/// Where the helper sits inside FERB.app (S4 puts it there, beside `ferb-secrets`).
pub const HELPER: &str = "Contents/Helpers/ferb-link";
/// The helper's signing identifier (S4 signs it so).
pub const HELPER_ID: &str = "com.ferb.link";
/// What a daemon run from a source checkout accepts.
pub const DEV_REQUIREMENT: &str = r#"identifier "com.ferb.link""#;

/// What changes when the helper is re-signed or replaced: its path, inode, size and modification time.
type Stamp = (PathBuf, u64, u64, i128);

fn stamp(path: &Path) -> Option<Stamp> {
    let stat = std::fs::metadata(path).ok()?;
    let modified = i128::from(stat.mtime()) * 1_000_000_000 + i128::from(stat.mtime_nsec());
    Some((path.to_path_buf(), stat.ino(), stat.size(), modified))
}

/// `LinkPeers::check(token)`: whether the process behind this audit token is the phone link's helper. The requirement
/// is derived again whenever the helper on disk changes (an update), and a token's verdict is kept until then.
pub struct LinkPeers<S: Security> {
    mode: Mode,
    candidates: Vec<PathBuf>,
    security: S,
    requirement: Option<S::Requirement>,
    stamp: Option<Option<Stamp>>,
    seen: HashMap<AuditToken, bool>,
}

impl<S: Security> LinkPeers<S> {
    pub fn new(mode: Mode, security: S) -> Self {
        Self::with_candidates(mode, security, default_candidates())
    }

    /// With other places to look for the installed FERB.app (tests).
    pub fn with_candidates(mode: Mode, security: S, candidates: Vec<PathBuf>) -> Self {
        LinkPeers {
            mode,
            candidates,
            security,
            requirement: None,
            stamp: None,
            seen: HashMap::new(),
        }
    }

    /// The helper whose signature counts (`None`: checkout mode, or no FERB.app).
    fn helper(&self) -> Option<PathBuf> {
        let app = match &self.mode {
            Mode::Bundled(bundle) => Some(bundle.clone()),
            Mode::Installed => installed_app(&self.candidates),
            Mode::Checkout => None,
        };
        app.map(|app| app.join(HELPER))
    }

    fn derive(&mut self) {
        let helper = self.helper();
        let now = helper.as_deref().and_then(stamp);
        if self.stamp.as_ref() == Some(&now) {
            return;
        }
        self.stamp = Some(now.clone());
        self.seen.clear();
        self.requirement = match (&self.mode, helper) {
            (Mode::Checkout, _) => self.security.requirement(DEV_REQUIREMENT),
            // a helper that isn't there has no requirement: nobody passes
            (_, Some(helper)) if now.is_some() => self.security.designated(&helper),
            _ => None,
        };
    }

    /// Whether the process behind `token` is the helper. No token, no requirement, or no valid signature: no.
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

    /// The check as `Server::set_is_link` takes it.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A requirement is "designated <path>" or its text; a token satisfies what its process was signed to.
    #[derive(Clone, Default)]
    struct Fake {
        signed: Rc<RefCell<HashMap<AuditToken, String>>>,
    }

    impl Security for Fake {
        type Requirement = String;
        fn requirement(&self, text: &str) -> Option<String> {
            Some(text.to_owned())
        }
        fn designated(&self, bundle: &Path) -> Option<String> {
            Some(format!("designated {}", bundle.display()))
        }
        fn satisfies(&self, token: &AuditToken, requirement: &String) -> bool {
            self.signed.borrow().get(token) == Some(requirement)
        }
    }

    fn token(n: u8) -> AuditToken {
        [n; 32]
    }

    /// Only the helper inside this FERB.app, as signed, passes; FERB.app itself and any other program do not.
    // decision: PHN-441
    // decision: PHN-124
    #[test]
    fn only_the_helper_inside_ferb_app_passes() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("FERB.app");
        std::fs::create_dir_all(app.join("Contents/Helpers")).unwrap();
        let fake = Fake::default();
        let mut peers =
            LinkPeers::with_candidates(Mode::Bundled(app.clone()), fake.clone(), vec![]);
        // no helper on disk: nobody
        fake.signed.borrow_mut().insert(
            token(1),
            format!("designated {}", app.join(HELPER).display()),
        );
        assert!(!peers.check(Some(&token(1))));
        std::fs::write(app.join(HELPER), b"helper").unwrap();
        // the helper appeared (an update): derived again
        assert!(peers.check(Some(&token(1))));
        fake.signed
            .borrow_mut()
            .insert(token(2), format!("designated {}", app.display()));
        assert!(!peers.check(Some(&token(2))), "FERB.app is not the helper");
        assert!(!peers.check(Some(&token(3))));
        assert!(!peers.check(None));
    }

    #[test]
    fn a_checkout_accepts_the_helper_s_identifier_and_no_ferb_app_means_nobody() {
        let fake = Fake::default();
        fake.signed
            .borrow_mut()
            .insert(token(1), DEV_REQUIREMENT.to_owned());
        let mut dev = LinkPeers::with_candidates(Mode::Checkout, fake.clone(), vec![]);
        assert!(dev.check(Some(&token(1))));
        let mut installed = LinkPeers::with_candidates(Mode::Installed, fake, vec![]);
        assert!(!installed.check(Some(&token(1))));
    }
}
