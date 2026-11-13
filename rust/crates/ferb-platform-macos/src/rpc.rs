//! The Mac's parts of FERB's local RPC server, put in place: who is on a connection (the kernel's pid and audit
//! token) and whether that process is FERB.app (the Security framework).

use std::path::Path;

use ferb_rpc::app_peer::{mode_of_this_process, AppPeers};
use ferb_rpc::Server;

use crate::security::SecurityFramework;

/// The check the daemon installs: this process's mode, the real Security framework.
pub fn system(ferb_home: &Path) -> AppPeers<SecurityFramework> {
    AppPeers::new(mode_of_this_process(ferb_home), SecurityFramework)
}

/// Gives `server` the Mac's peer lookup and FERB.app check, for a daemon whose data is in `ferb_home`.
pub fn attach(server: &Server, ferb_home: &Path) {
    server.set_peer_lookup(crate::peer::facts);
    server.set_is_app(system(ferb_home).into_check());
}
