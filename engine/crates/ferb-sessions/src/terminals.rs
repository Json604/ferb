//! Every terminal on the Mac as a session (HAND-691, HAND-692), and what is new in a terminal's text each time FERB
//! reads it (HAND-693).
//!
//! A terminal is a terminal device with the owner's processes on it: a tab or window of any terminal app, an editor's
//! terminal, a tmux pane. Everything here is the process table's structure (a process's terminal, its parent, its
//! group, the terminal's foreground group), never a list of programs or apps: a terminal running `agy`, `python` or a
//! program nobody has heard of is found the same way (AGENTS.md rule 4).

use std::collections::BTreeMap;

use crate::host::{self, Table};

/// One terminal as the process table shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Terminal {
    /// Its device ("/dev/ttys004").
    pub tty: String,
    /// The first process on it (its parent is on no terminal or another one: `login`, an editor's shell): the
    /// terminal lives as long as it does, so it names the session.
    pub leader: i32,
    /// The program in front: the leader of the terminal's foreground process group (what typed text reaches).
    pub front: i32,
}

/// The terminals in `t`, by device. FERB's own (any process on it descends from `own_pid`) are left out, and so is a
/// terminal whose program in front is a tmux client: its panes are terminals of their own.
pub fn find(t: &Table, own_pid: i32) -> Vec<Terminal> {
    let mut by_tty: BTreeMap<&str, Vec<i32>> = BTreeMap::new();
    for p in t.values() {
        if !p.tty.is_empty() {
            by_tty.entry(p.tty.as_str()).or_default().push(p.pid);
        }
    }
    let mut out = Vec::new();
    for (tty, mut pids) in by_tty {
        pids.sort_unstable();
        if pids
            .iter()
            .any(|p| host::ancestors(t, *p).contains(&own_pid))
        {
            continue;
        }
        let on_it = |pid: i32| t.get(&pid).is_some_and(|p| p.tty == tty);
        let Some(leader) = pids
            .iter()
            .copied()
            .find(|p| t.get(p).is_some_and(|x| !on_it(x.ppid)))
        else {
            continue;
        };
        let group = pids
            .iter()
            .filter_map(|p| t.get(p))
            .map(|p| p.tpgid)
            .find(|g| *g > 0);
        let front = group
            .and_then(|g| {
                pids.iter().copied().find(|p| *p == g).or_else(|| {
                    pids.iter()
                        .copied()
                        .find(|p| t.get(p).is_some_and(|x| x.pgid == g))
                })
            })
            .unwrap_or(leader);
        if t.get(&front).is_some_and(host::is_tmux) {
            continue;
        }
        out.push(Terminal {
            tty: tty.to_owned(),
            leader,
            front,
        });
    }
    out
}

/// A program's name as it was started: argv[0]'s file name, a login shell's leading "-" dropped ("-zsh" is "zsh");
/// the executable's file name when argv[0] is unknown.
pub fn program_name(argv0: &str, exe: &str) -> String {
    let a = argv0
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim_start_matches('-');
    if a.is_empty() {
        crate::proof::program_name(exe)
    } else {
        a.to_owned()
    }
}

/// The window of an app that is a terminal's, by the app's own words (HAND-692): among the app's windows (id, title),
/// the one title naming both the program and the folder, else the one naming the folder, else the one naming the
/// program, else the app's only window. None when it can't tell.
pub fn window_of<'a>(
    windows: &'a [(i64, String)],
    program: &str,
    folder: &str,
) -> Option<&'a (i64, String)> {
    use crate::proof::has_token;
    let one = |f: &dyn Fn(&str) -> bool| {
        let mut hits = windows.iter().filter(|(_, t)| f(t));
        let first = hits.next()?;
        hits.next().is_none().then_some(first)
    };
    let (p, f) = (program.trim(), folder.trim());
    (!p.is_empty() && !f.is_empty())
        .then(|| one(&|t| has_token(t, p) && has_token(t, f)))
        .flatten()
        .or_else(|| (!f.is_empty()).then(|| one(&|t| has_token(t, f))).flatten())
        .or_else(|| (!p.is_empty()).then(|| one(&|t| has_token(t, p))).flatten())
        .or_else(|| (windows.len() == 1).then(|| &windows[0]))
}

/// The most lines the first read of a terminal shows.
pub const FIRST_LINES: usize = 80;
/// The longest piece of a terminal's text one item holds (its end is kept).
pub const MAX_ITEM: usize = 8_000;

/// What FERB has shown of one terminal's text, so each new read gives only what is new (HAND-693).
///
/// A terminal's last line is open: the cursor is on it (a prompt being typed at, a progress line). Every other line
/// is settled. A read whose settled lines continue the last read's (the same lines, maybe scrolled up) gives the new
/// settled lines. A read that doesn't (a full-screen program redrew it, the screen was cleared) gives the whole screen
/// once it holds still for one read, so a program that keeps redrawing (a spinner) shows when it settles, not every
/// frame.
#[derive(Clone, Debug, Default)]
pub struct Screen {
    /// The text the shown lines line up with.
    base: Vec<String>,
    read_once: bool,
    /// The previous read, to tell a screen that holds still.
    last: Vec<String>,
    /// The last whole screen shown.
    shown: Vec<String>,
}

fn lines(text: &str) -> Vec<String> {
    let mut v: Vec<String> = text
        .replace("\r\n", "\n")
        .split(['\n', '\r'])
        .map(|l| l.trim_end().to_owned())
        .collect();
    while v.last().is_some_and(|l| l.is_empty()) {
        v.pop();
    }
    v
}

fn piece(ls: &[String]) -> Option<String> {
    let text = ls.join("\n");
    if text.trim().is_empty() {
        return None;
    }
    let n = text.chars().count();
    Some(if n > MAX_ITEM {
        let tail: String = text.chars().skip(n - (MAX_ITEM - 2)).collect();
        format!("…\n{tail}")
    } else {
        text
    })
}

impl Screen {
    /// The terminal's text now: what is new since the last read, None when nothing is to show yet.
    pub fn read(&mut self, text: &str) -> Option<String> {
        let cur = lines(text);
        let was = std::mem::replace(&mut self.last, cur.clone());
        if !self.read_once {
            self.read_once = true;
            self.base = cur.clone();
            self.shown = cur.clone();
            let from = cur.len().saturating_sub(FIRST_LINES);
            return piece(&cur[from..]);
        }
        let settled = self.base.len().saturating_sub(1);
        let need = settled.min(2);
        let lined_up = (0..=settled).find(|d| {
            let k = settled - d;
            k >= need && k <= cur.len() && self.base[*d..settled] == cur[..k]
        });
        if let Some(d) = lined_up {
            let k = settled - d;
            let open = cur.len().saturating_sub(1);
            self.base = cur.clone();
            return if open > k { piece(&cur[k..open]) } else { None };
        }
        // redrawn: shown once it holds still, and only when it differs from the last screen shown
        if cur == was && cur != self.shown {
            self.base = cur.clone();
            self.shown = cur.clone();
            return piece(&cur);
        }
        None
    }
}

/// Whether the owner's words show in a terminal's text (the proof a send arrived, HAND-693): one line each, the words
/// somewhere in the text.
pub fn shows(text: &str, words: &str) -> bool {
    let w = crate::one_line(words);
    !w.is_empty() && crate::one_line(text).contains(&w)
}
