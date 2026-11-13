//! Which servers on this Mac run outside FERB's containment (ferb/act/shell/listeners.py, F5a Task 3), so the shell's
//! profile keeps them out of reach. A listening port belongs to a process inside FERB's sandbox or it doesn't, by
//! fact: `sandbox_check` asks the kernel whether a process's sandbox denies reading FERB's run folder and allows its
//! workspace, which only FERB's profile does. Anything the kernel can't answer for counts as outside (fail closed).

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use crate::sockets::Socket;

/// Whether the process runs under FERB's shell profile (or mac.applescript's): sandboxed, denied FERB's run folder,
/// allowed its workspace (listeners.py:44).
#[cfg(target_os = "macos")]
pub fn contained(pid: i32, ferb_home: &Path, workspace: &Path) -> bool {
    use std::ffi::{c_char, CString};
    use std::os::unix::ffi::OsStrExt;

    const SANDBOX_FILTER_PATH: i32 = 1;
    const SANDBOX_CHECK_NO_REPORT: i32 = 0x4000_0000;
    extern "C" {
        // libsystem_sandbox (in libSystem): variadic, the filter's argument follows `type`.
        fn sandbox_check(pid: libc::pid_t, operation: *const c_char, type_: i32, ...) -> i32;
    }
    let real = |p: &Path| CString::new(crate::pyos::realpath(p).as_os_str().as_bytes()).ok();
    let (Some(run), Some(ws)) = (real(&ferb_home.join("run")), real(workspace)) else {
        return false;
    };
    let op = c"file-read-data";
    let flags = SANDBOX_FILTER_PATH | SANDBOX_CHECK_NO_REPORT;
    // SAFETY: every pointer is a NUL-terminated string that outlives the call; a null operation asks only whether the
    // process is sandboxed at all.
    unsafe {
        sandbox_check(pid, std::ptr::null(), 0) == 1
            && sandbox_check(pid, op.as_ptr(), flags, run.as_ptr()) == 1
            && sandbox_check(pid, op.as_ptr(), flags, ws.as_ptr()) == 0
    }
}

#[cfg(not(target_os = "macos"))]
pub fn contained(_pid: i32, _ferb_home: &Path, _workspace: &Path) -> bool {
    false
}

/// (outside, inside): ports of this Mac's servers that run outside FERB's containment, and ports whose every listener
/// runs inside it, from one complete socket reading (listeners.py:58). None when the listing couldn't be read.
pub fn local_listeners(
    every: Option<&[Socket]>,
    mut contained: impl FnMut(i32) -> bool,
) -> Option<(BTreeSet<u32>, BTreeSet<u32>)> {
    let every = every?;
    let found: Vec<(u32, Option<i32>)> = every
        .iter()
        .filter(|s| {
            s.local.1 != 0
                && ((s.proto == "tcp" && s.state == "LISTEN")
                    || (s.proto == "udp" && s.remote.is_none()))
        })
        .map(|s| (s.local.1, s.pid))
        .collect();
    let mut inside: HashMap<i32, bool> = HashMap::new();
    for (_, pid) in &found {
        if let Some(pid) = pid {
            if !inside.contains_key(pid) {
                inside.insert(*pid, contained(*pid));
            }
        }
    }
    let outside: BTreeSet<u32> = found
        .iter()
        .filter(|(_, pid)| pid.is_none_or(|p| !inside[&p]))
        .map(|(port, _)| *port)
        .collect();
    let all: BTreeSet<u32> = found.iter().map(|(port, _)| *port).collect();
    Some((outside.clone(), all.difference(&outside).copied().collect()))
}

/// `local_listeners` on this Mac's sockets now (every user's: `sockets::complete`).
pub fn local_listeners_now(
    ferb_home: &Path,
    workspace: &Path,
) -> Option<(BTreeSet<u32>, BTreeSet<u32>)> {
    let every = crate::sockets::complete();
    local_listeners(every.as_deref(), |pid| contained(pid, ferb_home, workspace))
}

/// Ports of this Mac's servers that run outside FERB's containment; None when unreadable (listeners.py:75).
pub fn outside_ports(ferb_home: &Path, workspace: &Path) -> Option<BTreeSet<u32>> {
    local_listeners_now(ferb_home, workspace).map(|(outside, _)| outside)
}
