//! The app that shows an agent, from the process table alone (HAND-654): the agent's ancestors up to the first one whose
//! program is inside an `.app` bundle; the app's main process is the highest ancestor in that same bundle (an IDE's
//! terminal runs under a helper inside the IDE's bundle). Through tmux, the pane's server has no app above it: its
//! clients do, so the tmux client's own chain is followed. No list of hosts: any app found this way is a host.

use std::collections::HashMap;

use crate::model::Via;

/// One process as the Mac lists it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    /// Its process group.
    pub pgid: i32,
    /// The foreground process group of its terminal (-1 or 0 when it has none).
    pub tpgid: i32,
    /// Its terminal ("/dev/ttys004"), "" when none.
    pub tty: String,
    /// The program's full path ("" when unreadable).
    pub exe: String,
}

/// A process table by pid.
pub type Table = HashMap<i32, Proc>;

/// The outermost `.app` bundle a program lives in ("/Applications/Visual Studio Code.app"), if any.
pub fn bundle_of(exe: &str) -> Option<String> {
    let mut at = 0usize;
    for part in exe.split('/') {
        at += part.len();
        if part.len() > 4 && part.ends_with(".app") {
            return Some(exe[..at].to_owned());
        }
        at += 1; // the '/'
    }
    None
}

/// The program's file name.
pub fn program(exe: &str) -> &str {
    exe.rsplit('/').next().unwrap_or(exe)
}

/// What the process tree says about an agent's host (before the Mac fills in the bundle id and name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The bundle's folder.
    pub bundle: String,
    /// The app's main process.
    pub pid: i32,
    pub via: Via,
    /// The terminal the input goes to (the tmux client's for `Tmux`).
    pub tty: String,
    /// For tmux: its server's pid.
    pub tmux_server: i32,
}

/// The chain of ancestors of `pid`, itself first, stopping at pid 1 or a loop.
pub fn ancestors(t: &Table, pid: i32) -> Vec<i32> {
    let mut out = vec![];
    let mut cur = pid;
    while cur > 1 && !out.contains(&cur) && out.len() < 64 {
        out.push(cur);
        match t.get(&cur) {
            Some(p) => cur = p.ppid,
            None => break,
        }
    }
    out
}

/// The app above `pid` (by its own chain): (bundle, main pid).
fn app_above(t: &Table, pid: i32) -> Option<(String, i32)> {
    let chain = ancestors(t, pid);
    let mut found: Option<(String, i32)> = None;
    for p in chain {
        let exe = t.get(&p).map(|x| x.exe.as_str()).unwrap_or("");
        match (&found, bundle_of(exe)) {
            (None, Some(b)) => found = Some((b, p)),
            (Some((b, _)), Some(b2)) if *b == b2 => found = Some((b2, p)),
            (Some(_), _) => break,
            (None, None) => {}
        }
    }
    found
}

/// Whether a process is a tmux server or client (by its program's name: tmux is a multiplexer FERB has an adapter
/// for, not a host).
pub fn is_tmux(p: &Proc) -> bool {
    program(&p.exe) == "tmux" || p.exe.ends_with("/tmux")
}

/// The host of the agent `pid`. `tmux_clients(server_pid)` lists the tty and pid of each client of a tmux server
/// (the Mac asks tmux itself); the first client with an app above it is the one that shows the pane.
pub fn resolve(
    t: &Table,
    pid: i32,
    tmux_clients: &dyn Fn(i32) -> Vec<(i32, String)>,
) -> Option<Found> {
    let me = t.get(&pid)?;
    let chain = ancestors(t, pid);
    if let Some(server) = chain.iter().skip(1).find(|p| t.get(p).is_some_and(is_tmux)) {
        for (cpid, ctty) in tmux_clients(*server) {
            if let Some((bundle, app)) = app_above(t, cpid) {
                return Some(Found {
                    bundle,
                    pid: app,
                    via: Via::Tmux,
                    tty: ctty,
                    tmux_server: *server,
                });
            }
        }
        // a detached tmux: no window shows it, input goes through tmux alone
        return Some(Found {
            bundle: String::new(),
            pid: 0,
            via: Via::Tmux,
            tty: me.tty.clone(),
            tmux_server: *server,
        });
    }
    let (bundle, app) = app_above(t, pid)?;
    Some(Found {
        bundle,
        pid: app,
        via: Via::Direct,
        tty: me.tty.clone(),
        tmux_server: 0,
    })
}

/// HAND-658: the agent is its terminal's foreground process group, so typed text reaches it and not a shell.
pub fn in_front(t: &Table, pid: i32) -> bool {
    t.get(&pid)
        .is_some_and(|p| !p.tty.is_empty() && p.tpgid > 0 && p.pgid == p.tpgid)
}
