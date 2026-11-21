//! Which servers on this Mac run outside FERB's containment (ferb/act/shell/listeners.py, F5a Task 3), so the shell's
//! profile keeps them out of reach. A listening port belongs to a process FERB contains or it doesn't, by a fact FERB
//! keeps itself (HAND-650): the kernel's unique id of each command FERB started under its shell profile, and the
//! kernel's record of which process forked which. Python asked each process's own sandbox instead (`sandbox_check` on
//! FERB's run folder and workspace): another sandbox could answer the same way, and the question ran that sandbox's
//! actions as if the process had tried (journal 161). Nothing here asks another process's sandbox anything, and
//! anything FERB can't read counts as outside (fail closed).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::sockets::Socket;

/// FERB's record (HAND-650). `known`: the kernel's unique ids (never reused while the Mac is up) of the processes FERB
/// contains: each command FERB started under its profile and every process since found to descend from one, kept for
/// the daemon's life, so a process found contained stays so after the process that forked it ends. `commands`: the
/// commands themselves, while they may still run. `ticking`: the watch runs; `launches` counts commands recorded.
#[derive(Default)]
struct Record {
    known: HashSet<u64>,
    commands: HashSet<u64>,
    ticking: bool,
    launches: u64,
}

fn record() -> MutexGuard<'static, Record> {
    static RECORD: OnceLock<Mutex<Record>> = OnceLock::new();
    RECORD
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// What the kernel records about a process's descent: the pid of its parent now (launchd's once that parent ended),
/// its own unique id, and the unique id of the process that forked it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lineage {
    pub ppid: i32,
    pub uniqueid: u64,
    pub parent_uniqueid: u64,
}

/// libproc's PROC_PIDT_BSDINFOWITHUNIQID answer (xnu bsd/sys/proc_info.h; not in the public SDK headers).
#[cfg(target_os = "macos")]
#[repr(C)]
struct BsdInfoWithUniqId {
    bsd: libc::proc_bsdinfo,
    uuid: [u8; 16],
    uniqueid: u64,
    parent_uniqueid: u64,
    idversion: i32,
    orig_ppidversion: i32,
    reserved: [u64; 2],
}

/// The process's lineage; None when the kernel won't say (another user's or root's process, a zombie, no process) or
/// answers with any size but the whole record. Allocates nothing: a child calls it between fork and exec.
#[cfg(target_os = "macos")]
pub fn lineage(pid: i32) -> Option<Lineage> {
    const PROC_PIDT_BSDINFOWITHUNIQID: i32 = 18;
    if pid <= 0 {
        return None;
    }
    let size = std::mem::size_of::<BsdInfoWithUniqId>() as i32;
    let mut info = std::mem::MaybeUninit::<BsdInfoWithUniqId>::zeroed();
    // SAFETY: `info` holds `size` bytes, the size passed; the kernel writes at most that.
    let got = unsafe {
        libc::proc_pidinfo(
            pid,
            PROC_PIDT_BSDINFOWITHUNIQID,
            0,
            info.as_mut_ptr() as *mut std::ffi::c_void,
            size,
        )
    };
    if got != size {
        return None;
    }
    // SAFETY: zeroed, then the whole record written (got == size); every field is plain data.
    let info = unsafe { info.assume_init() };
    if info.bsd.pbi_pid as i32 != pid || info.uniqueid == 0 {
        return None;
    }
    Some(Lineage {
        ppid: info.bsd.pbi_ppid as i32,
        uniqueid: info.uniqueid,
        parent_uniqueid: info.parent_uniqueid,
    })
}

#[cfg(not(target_os = "macos"))]
pub fn lineage(_pid: i32) -> Option<Lineage> {
    None
}

/// This process's unique id, for a child between fork and exec (one proc_pidinfo, no allocation).
pub(crate) fn own_uniqueid() -> Option<u64> {
    // SAFETY: getpid has no preconditions.
    lineage(unsafe { libc::getpid() }).map(|l| l.uniqueid)
}

/// Records a command FERB started under its shell profile, by the unique id its process read before it ran the
/// profile (`pty`); every process it forks from then on descends from it. Starts the watch if it isn't running.
pub(crate) fn launched(uniqueid: u64) {
    let mut r = record();
    r.known.insert(uniqueid);
    r.commands.insert(uniqueid);
    r.launches += 1;
    if !r.ticking {
        r.ticking = true;
        let started = std::thread::Builder::new()
            .name("ferb-contained-watch".into())
            .spawn(watch);
        if started.is_err() {
            r.ticking = false; // no watch: descendants are still found by `contained` and each `sweep`
        }
    }
}

/// While any recorded command runs, every WATCH_S (HAND-232's second) FERB records every descendant of its
/// contained processes, so a server that detaches from its command (pg_ctl, a daemonizing server) after living under it
/// for a second stays contained after the process that forked it ends.
fn watch() {
    loop {
        let launches = record().launches;
        std::thread::sleep(std::time::Duration::from_secs_f64(crate::manager::WATCH_S));
        if sweep() {
            continue;
        }
        let mut r = record();
        if r.launches == launches {
            r.ticking = false;
            return;
        }
    }
}

/// Records every process on this Mac now whose forking parent is one FERB contains (parents before children, so a
/// whole living tree in one pass). Whether any recorded command still runs (true when the processes can't be listed,
/// so the watch keeps trying). Reads only kernel facts (`lineage`); asks no sandbox.
pub fn sweep() -> bool {
    let before = record().launches;
    let Some(pids) = crate::sockets::pids() else {
        return true;
    };
    let rows: Vec<Lineage> = pids.into_iter().filter_map(lineage).collect();
    let mut r = record();
    let complete = r.launches == before; // a command recorded after the listing isn't in it
    sweep_rows(&mut r, rows, complete)
}

/// `sweep` over one listing; `complete` when every recorded command was started before it.
fn sweep_rows(r: &mut Record, mut rows: Vec<Lineage>, complete: bool) -> bool {
    rows.sort_by_key(|l| l.uniqueid);
    for l in &rows {
        if r.known.contains(&l.parent_uniqueid) {
            r.known.insert(l.uniqueid);
        }
    }
    if complete {
        let alive: HashSet<u64> = rows.iter().map(|l| l.uniqueid).collect();
        r.commands.retain(|c| alive.contains(c)); // a command missing from a full listing has ended
    }
    !r.commands.is_empty()
}

/// Whether the process is one FERB contains: a command FERB started under its shell profile, or a descendant of one by
/// the kernel's record of who forked whom. The chain is followed through living parents, each still the process that
/// forked its child (unique ids, so a reused pid never matches); a process whose forking parent ended before FERB saw
/// that parent here or in a `sweep` counts as outside. Never asks the process's sandbox (journal 161).
pub fn contained(pid: i32) -> bool {
    contained_by(pid, lineage)
}

/// `contained` over a lineage reader (the unit tests script it).
fn contained_by(pid: i32, read: impl Fn(i32) -> Option<Lineage>) -> bool {
    let Some(mut at) = read(pid) else {
        return false;
    };
    let mut chain = Vec::new();
    let ours = loop {
        {
            let r = record();
            if r.known.contains(&at.uniqueid) || r.known.contains(&at.parent_uniqueid) {
                break true;
            }
        }
        chain.push(at.uniqueid);
        if at.ppid <= 1 {
            break false;
        }
        match read(at.ppid) {
            // a parent's id is always smaller than its child's, so the walk ends
            Some(parent)
                if parent.uniqueid == at.parent_uniqueid && parent.uniqueid < at.uniqueid =>
            {
                at = parent
            }
            _ => break false,
        }
    };
    if ours {
        record().known.extend(chain);
    }
    ours
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

/// `local_listeners` on this Mac's sockets now (every user's: `sockets::complete`), after a `sweep`.
pub fn local_listeners_now() -> Option<(BTreeSet<u32>, BTreeSet<u32>)> {
    sweep();
    let every = crate::sockets::complete();
    local_listeners(every.as_deref(), contained)
}

/// Ports of this Mac's servers that run outside FERB's containment; None when unreadable (listeners.py:75).
pub fn outside_ports() -> Option<BTreeSet<u32>> {
    local_listeners_now().map(|(outside, _)| outside)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids far above any real one (the kernel counts up from 1 at boot).
    const TOP: u64 = u64::MAX - 1_000;

    fn table(rows: &[(i32, Lineage)]) -> impl Fn(i32) -> Option<Lineage> + '_ {
        move |pid| rows.iter().find(|(p, _)| *p == pid).map(|(_, l)| *l)
    }

    fn row(ppid: i32, uniqueid: u64, parent_uniqueid: u64) -> Lineage {
        Lineage {
            ppid,
            uniqueid,
            parent_uniqueid,
        }
    }

    // decision: HAND-650
    #[test]
    fn descent_is_followed_by_unique_ids_and_a_reused_pid_never_matches() {
        record().known.insert(TOP);
        let live = [
            (700, row(1, TOP, 9)),             // FERB's command (its parent: FERB)
            (701, row(700, TOP + 1, TOP)),     // its child
            (702, row(701, TOP + 2, TOP + 1)), // a grandchild through a living parent
            // pid 703 now belongs to another process than the one that forked 704
            (703, row(1, 50, 1)),
            (704, row(703, TOP + 5, TOP + 4)),
            (705, row(1, 51, 1)), // never FERB's
        ];
        let read = table(&live);
        assert!(contained_by(700, &read));
        assert!(contained_by(702, &read));
        assert!(
            !contained_by(704, &read),
            "a reused parent pid is not the parent"
        );
        assert!(!contained_by(705, &read));
        assert!(
            !contained_by(999, &read),
            "an unreadable process is outside"
        );
        // the grandchild was found ours while its parent lived; once that parent ends it stays ours
        let later = [(702, row(1, TOP + 2, TOP + 1))];
        assert!(contained_by(702, table(&later)));
        // a command's own child outlives it: the kernel still names the command as the process that forked it
        let backgrounded = [(708, row(1, TOP + 9, TOP))];
        assert!(contained_by(708, table(&backgrounded)));
        // an orphan whose forking parent FERB never saw is outside (fail closed)
        let orphan = [(706, row(1, TOP + 7, TOP + 6))];
        assert!(!contained_by(706, table(&orphan)));
    }

    // decision: HAND-650
    #[test]
    fn a_sweep_records_a_living_tree_in_one_pass_and_says_whether_a_command_runs() {
        let mut r = Record::default();
        r.known.insert(TOP - 100);
        r.commands.insert(TOP - 100);
        // listed in pid order, not by age: the grandchild comes first
        let rows = vec![
            row(1, TOP - 97, TOP - 98),  // a server whose parent (a pg_ctl) is alive
            row(1, TOP - 98, TOP - 100), // that pg_ctl, forked by FERB's command
            row(1, TOP - 100, 9),        // the command
            row(1, TOP - 90, 77),        // another program's
        ];
        assert!(sweep_rows(&mut r, rows, true));
        assert!(r.known.contains(&(TOP - 97)) && r.known.contains(&(TOP - 98)));
        assert!(!r.known.contains(&(TOP - 90)));
        // the command and the pg_ctl ended: the server is still FERB's, and no command runs
        assert!(!sweep_rows(&mut r, vec![row(1, TOP - 97, TOP - 98)], true));
        assert!(r.known.contains(&(TOP - 97)));
        // a command recorded after the listing is kept until a listing that could show it
        r.commands.insert(TOP - 50);
        assert!(sweep_rows(&mut r, vec![], false));
    }
}
