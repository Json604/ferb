//! FERB's protection lists (`ferb-policy`'s `Protection`, ferb/policy/protection.py) behind this crate's
//! [`Protection`] seam: what the sandbox profile, the local-port checks and `shell.rules` read from them. Nothing here
//! decides a rule; every value is the policy crate's.
//!
//! Here, not in `ferb-policy`: this crate owns the seam and may see the policy crate (the policy crate sees no shell),
//! so the one implementation sits beside the trait (feature `policy`, on by default).

use std::path::{Path, PathBuf};

use ferb_policy::ppath::PosixFiles;
use ferb_policy::protection as policy;
use serde_json::Value;

use crate::protection::{Entry, LocalPort, Protection};

/// The stateless file calls the lists use: each goes straight to the system.
static POSIX_FILES: PosixFiles = PosixFiles;

/// The policy part's protection lists as the shell reads them.
pub struct PolicyProtection {
    p: policy::Protection<'static>,
    home: PathBuf,
    ferb_home: PathBuf,
    workspace: PathBuf,
    uploads: PathBuf,
    reads: Vec<Entry>,
    writes: Vec<Entry>,
    ports: Vec<LocalPort>,
}

// SAFETY: `policy::Protection<'static>` holds owned data and a `&'static dyn Files`; [`PolicyProtection::load`] is
// the only constructor and always gives it this module's stateless `PosixFiles` (each call goes straight to libc), so
// sharing it between threads is safe. Nothing in it has interior mutability.
unsafe impl Send for PolicyProtection {}
// SAFETY: as above: read-only data and a stateless file-call table.
unsafe impl Sync for PolicyProtection {}

fn entries(list: &[policy::Entry]) -> Vec<Entry> {
    list.iter()
        .map(|e| Entry {
            id: e.id.clone(),
            path: PathBuf::from(&e.path),
            typed: PathBuf::from(&e.typed),
        })
        .collect()
}

impl PolicyProtection {
    /// `protection.load(lists, home, ferb_home, app_bundle)`: the daemon's lists after the workspace check, on the
    /// real file system.
    pub fn load(
        lists: &policy::Lists<'_>,
        home: &str,
        ferb_home: &str,
        app_bundle: Option<&str>,
    ) -> Result<PolicyProtection, policy::BadProtection> {
        policy::load(lists, home, ferb_home, app_bundle, &POSIX_FILES).map(PolicyProtection::of)
    }

    /// `Protection(home, ferb_home, lists, app_bundle)` without `load`'s workspace check (tests, a fresh home).
    pub fn new(
        lists: &policy::Lists<'_>,
        home: &str,
        ferb_home: &str,
        app_bundle: Option<&str>,
    ) -> Result<PolicyProtection, policy::BadProtection> {
        policy::Protection::new(home, ferb_home, lists, app_bundle, &POSIX_FILES)
            .map(PolicyProtection::of)
    }

    fn of(p: policy::Protection<'static>) -> PolicyProtection {
        PolicyProtection {
            home: PathBuf::from(&p.home),
            ferb_home: PathBuf::from(&p.ferb_home),
            workspace: PathBuf::from(&p.workspace),
            uploads: PathBuf::from(&p.uploads),
            reads: entries(&p.reads),
            writes: entries(&p.writes),
            ports: p
                .local_ports
                .iter()
                .map(|l| LocalPort {
                    port: l.port,
                    executable: PathBuf::from(&l.executable),
                })
                .collect(),
            p,
        }
    }

    /// The policy part's lists themselves.
    pub fn policy(&self) -> &policy::Protection<'static> {
        &self.p
    }
}

impl Protection for PolicyProtection {
    fn home(&self) -> &Path {
        &self.home
    }
    fn ferb_home(&self) -> &Path {
        &self.ferb_home
    }
    fn workspace(&self) -> &Path {
        &self.workspace
    }
    fn uploads(&self) -> &Path {
        &self.uploads
    }
    fn reads(&self) -> &[Entry] {
        &self.reads
    }
    fn writes(&self) -> &[Entry] {
        &self.writes
    }
    fn app_entries(&self) -> Vec<PathBuf> {
        self.p
            .app_entries()
            .into_iter()
            .map(PathBuf::from)
            .collect()
    }
    fn service_files(&self) -> Vec<PathBuf> {
        self.p.service_files.iter().map(PathBuf::from).collect()
    }
    fn guarded_parents(&self) -> Vec<PathBuf> {
        self.p
            .guarded_parents()
            .into_iter()
            .map(PathBuf::from)
            .collect()
    }
    fn forms_now(&self, path: &Path) -> Vec<PathBuf> {
        self.p
            .forms_now(&path.to_string_lossy())
            .into_iter()
            .map(PathBuf::from)
            .collect()
    }
    fn app_data(&self, path: &Path) -> bool {
        self.p.app_data(&path.to_string_lossy())
    }
    fn local_ports(&self) -> &[LocalPort] {
        &self.ports
    }
    fn rules(&self) -> Vec<Value> {
        self.p.rules()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_shell_reads_the_policy_lists_as_they_are() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("user");
        let ferb = tmp.path().join("ferb");
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(ferb.join("workspace")).unwrap();
        let (home, ferb) = (home.to_str().unwrap(), ferb.to_str().unwrap());
        let never_read = json!(["~/.ssh"]);
        let never_write = json!(["~/.zshrc"]);
        let terminals = json!(policy::DEFAULT_TERMINAL_APPS);
        let ports = json!([]);
        let lists = policy::Lists {
            never_read: &never_read,
            never_write: &never_write,
            terminal_apps: &terminals,
            local_ports: &ports,
        };
        let p = PolicyProtection::new(&lists, home, ferb, None).unwrap();
        fn shared<T: Send + Sync>(_: &T) {}
        shared(&p);
        let seam: &dyn Protection = &p;
        assert_eq!(seam.home(), Path::new(home));
        assert_eq!(seam.ferb_home(), Path::new(&p.policy().ferb_home));
        assert_eq!(seam.reads().len(), p.policy().reads.len());
        assert_eq!(
            seam.entry("read:~/.ssh").map(|e| e.id.as_str()),
            Some("read:~/.ssh")
        );
        assert_eq!(seam.rules(), p.policy().rules());
        assert_eq!(
            seam.guarded_parents(),
            p.policy()
                .guarded_parents()
                .into_iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>()
        );
    }
}
