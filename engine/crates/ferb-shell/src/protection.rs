//! The seam to FERB's protection lists (ferb/policy/protection.py, ported in the policy part): what the sandbox
//! profile, the local-port checks and `shell.rules` read from them. Nothing here decides a rule; the lists are paths
//! and ids, built and resolved by the policy crate.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// protection.py:89-98.
pub const FERB_STATE: &str = "ferb_state";
pub const NETWORK: &str = "network";
pub const ADMIN: &str = "admin";
pub const TRUSTD: &str = "trustd";
pub const FERB_APP: &str = "ferb_app";
pub const FERB_SERVICE: &str = "ferb_service";
/// The rules no approved escape lifts (rule 6).
pub const NEVER_LIFTED: [&str; 4] = [FERB_STATE, NETWORK, FERB_APP, FERB_SERVICE];

/// One never-read or never-write entry (protection.py:109-116): the parts the shell profile reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// `read:~/.ssh`, `write:~/.zshrc`.
    pub id: String,
    /// Resolved when FERB started.
    pub path: PathBuf,
    /// As written, absolute, `..` folded.
    pub typed: PathBuf,
}

/// An owner-approved TCP port and the resolved executable its listener must run (protection.py:119-123).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalPort {
    pub port: u16,
    pub executable: PathBuf,
}

/// What the shell reads from the protection lists. The policy crate implements it over its `Protection`.
pub trait Protection: Send + Sync {
    /// The owner's home folder.
    fn home(&self) -> &Path;
    /// FERB's own home, resolved.
    fn ferb_home(&self) -> &Path;
    /// The literal workspace folder in FERB's home.
    fn workspace(&self) -> &Path;
    /// The workspace's upload staging.
    fn uploads(&self) -> &Path;
    fn reads(&self) -> &[Entry];
    fn writes(&self) -> &[Entry];
    /// FERB.app, resolved and as typed (protection.py:491).
    fn app_entries(&self) -> Vec<PathBuf>;
    /// FERB's launchd definitions (protection.py:366).
    fn service_files(&self) -> Vec<PathBuf>;
    /// Every folder above a listed entry, the upload staging, FERB.app and the service files (protection.py:495).
    fn guarded_parents(&self) -> Vec<PathBuf>;
    /// A listed path as written and as written resolved now (protection.py:397); never looks into another app's
    /// container.
    fn forms_now(&self, path: &Path) -> Vec<PathBuf>;
    /// Whether the path is another app's container or inside one (protection.py:386).
    fn app_data(&self, path: &Path) -> bool;
    fn local_ports(&self) -> &[LocalPort];
    /// `rules()` as shell.rules shows them (protection.py:506).
    fn rules(&self) -> Vec<Value>;

    /// protection.py:479.
    fn entry(&self, rule_id: &str) -> Option<&Entry> {
        self.reads()
            .iter()
            .chain(self.writes())
            .find(|e| e.id == rule_id)
    }
}
