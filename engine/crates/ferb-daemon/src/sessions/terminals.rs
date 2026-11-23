//! Every terminal on the Mac as a session (kb HAND-691..695): found by code from the process table on each look, watched
//! live and typed into through the one send path (HAND-710, HAND-711: FERB reads none of its text), and new ones opened
//! for the owner.
//!
//! - Found (HAND-692): one session per terminal device with the owner's processes on it, in any app the process tree
//!   leads to (HAND-654), whatever program runs there. A Claude Code or Codex session claims its terminal: the agent's
//!   row is the one shown, with the terminal's program and window added, never a second row.
//! - Seen (HAND-711): never read; `watch` on the session: its window is watched live (Watch) and typed into.
//! - Sent to (HAND-694): whatever leads the terminal gets the words, but only while it is still the program the owner
//!   was shown; GATE-622's question and HAND-682/683's proof as for every session.
//! - Opened (HAND-695): a new window of his terminal app in a folder, typing nothing unless he gave a command.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use ferb_sessions::codes;
use ferb_sessions::host::{self, Table};
use ferb_sessions::model::*;
use ferb_sessions::{paths, terminals};
use serde_json::json;

use super::{folder_name, lock, no, Caller, Entry, Refused, Sessions, Term};

/// A terminal is working while its device had input or output within this many seconds (two looks).
pub const WORKING_S: f64 = 4.0;
/// How long a new terminal may take to appear before FERB says it couldn't find it (the call answers within the
/// link's 10 s).
pub const START_WAIT_S: f64 = 6.0;

/// HAND-695: the apps FERB can open a new terminal window in, in the background: (bundle id, name). The owner's most
/// recently used one of these is chosen; Terminal when he uses none of them. Others: T-1287.
pub const OPENERS: &[(&str, &str)] = &[(paths::TERMINAL, "Terminal")];

impl Sessions {
    /// HAND-692: one look's terminals. `table` is this look's process table.
    pub(super) fn note_terminals(&self, table: &Table) {
        let mac = self.seams.mac.clone();
        let found = terminals::find(table, self.seams.own_pid);
        // an agent's session claims its terminal
        let claimed: HashMap<String, String> = {
            let st = lock(&self.state);
            st.sessions
                .values()
                .filter(|e| e.term.is_none() && e.s.pid > 0 && e.s.agent != Agent::AppChat)
                .filter_map(|e| {
                    let tty = table.get(&e.s.pid).map(|p| p.tty.clone())?;
                    (!tty.is_empty()).then(|| (tty, e.s.id.clone()))
                })
                .collect()
        };
        let windows = if found.is_empty() {
            Vec::new()
        } else {
            mac.window_titles()
        };
        let windows_of = |pid: i32| -> Vec<(i64, String)> {
            windows
                .iter()
                .filter(|(p, _, _)| *p == pid)
                .map(|(_, id, t)| (*id, t.clone()))
                .collect()
        };
        let mono = (self.seams.mono)();
        let mut live: HashSet<String> = HashSet::new();
        for t in &found {
            let front = table.get(&t.front).cloned().unwrap_or_default();
            let program = mac.program_of(t.front, &front.exe);
            let mut cwd = mac.cwd_of(t.front);
            if cwd.is_empty() {
                cwd = mac.cwd_of(t.leader);
            }
            if let Some(agent_id) = claimed.get(&t.tty) {
                // the agent's row, with what its terminal shows
                let host_pid = lock(&self.state)
                    .sessions
                    .get(agent_id)
                    .and_then(|e| e.s.host.as_ref().map(|h| h.pid))
                    .unwrap_or(0);
                let words =
                    terminals::window_of(&windows_of(host_pid), &program, &folder_name(&cwd))
                        .map(|(_, w)| self.redact(w))
                        .unwrap_or_default();
                if let Some(e) = lock(&self.state).sessions.get_mut(agent_id) {
                    e.s.program = program;
                    e.s.window = words;
                }
                continue;
            }
            let id = Agent::Terminal.session_id(&t.leader.to_string());
            live.insert(id.clone());
            let known_host = {
                let st = lock(&self.state);
                st.sessions.get(&id).and_then(|e| {
                    let term = e.term.as_ref()?;
                    (term.tty == t.tty).then(|| e.s.host.clone()).flatten()
                })
            };
            let host = known_host.or_else(|| {
                host::resolve(table, t.leader, &|server| mac.tmux_clients(server)).map(|f| {
                    let (bundle_id, name) = if f.bundle.is_empty() {
                        (String::new(), String::new())
                    } else {
                        mac.app(&f.bundle)
                    };
                    // the app's own process, which owns its windows (HAND-654, PHN-797)
                    let pid = if bundle_id.is_empty() {
                        f.pid
                    } else {
                        mac.app_pid(&bundle_id, f.pid)
                    };
                    Host {
                        bundle_id,
                        name,
                        pid,
                        via: Some(f.via),
                        tty: f.tty,
                    }
                })
            });
            // no app shows it and no tmux reaches it: not a terminal the owner has open
            let Some(host) = host else { continue };
            let window =
                terminals::window_of(&windows_of(host.pid), &program, &folder_name(&cwd)).cloned();
            let activity = mac.tty_activity(&t.tty);
            let (paths_, why) = paths::send_paths(Agent::Terminal, Some(&host), false);
            let now = self.now();
            let mut st = lock(&self.state);
            let e = st.sessions.entry(id.clone()).or_insert_with(|| {
                let mut e = Entry::new(&id, Agent::Terminal, &now);
                e.term = Some(Term {
                    leader: t.leader,
                    tty: t.tty.clone(),
                    activity: activity.as_ref().map_or(0.0, |a| a.0),
                    changed_at: f64::NEG_INFINITY,
                    ..Term::default()
                });
                if let Some((_, iso)) = &activity {
                    e.s.last_at = iso.clone();
                }
                e
            });
            let term = e.term.get_or_insert_with(Term::default);
            if let Some((at, iso)) = &activity {
                if *at > term.activity {
                    term.activity = *at;
                    term.changed_at = mono;
                    e.s.last_at = iso.clone();
                }
            }
            term.tty = t.tty.clone();
            term.window = window.as_ref().map_or(0, |w| w.0);
            e.s.status = if mono - term.changed_at < WORKING_S {
                Status::Working
            } else {
                Status::Idle
            };
            e.s.pid = t.front;
            e.s.cwd = cwd;
            e.s.program = program;
            e.s.window = window.map(|w| self.redact(&w.1)).unwrap_or_default();
            e.s.host = Some(host);
            e.s.send_paths = paths_;
            e.s.no_send = why;
            e.s.watch = true;
            e.exe = front.exe;
        }
        // terminals that closed, or that an agent's session claimed since
        let gone: Vec<String> = lock(&self.state)
            .sessions
            .iter()
            .filter(|(id, e)| e.term.is_some() && !live.contains(*id))
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            self.end(&id);
        }
    }

    /// HAND-694: right before typing into a terminal, the program in front must still be the one the owner was shown.
    pub(super) fn term_unchanged(&self, s: &Session) -> Result<(), Refused> {
        let table = self.seams.mac.processes();
        let leader = lock(&self.state)
            .sessions
            .get(&s.id)
            .and_then(|e| e.term.as_ref().map(|t| t.leader))
            .unwrap_or(0);
        let now = terminals::find(&table, self.seams.own_pid)
            .into_iter()
            .find(|t| t.leader == leader)
            .ok_or_else(super::gone)?;
        let exe = table
            .get(&now.front)
            .map(|p| p.exe.clone())
            .unwrap_or_default();
        let program = self.seams.mac.program_of(now.front, &exe);
        if program != s.program {
            return Err(no(
                codes::NOT_IN_FRONT,
                format!(
                    "{} is in front in this terminal now, not {}, so FERB typed nothing. Look again and send if it's right.",
                    if program.is_empty() { "Something else".to_owned() } else { program },
                    if s.program.is_empty() { "what you saw" } else { &s.program }
                ),
            ));
        }
        Ok(())
    }

    /// HAND-695: the app a new terminal opens in: the owner's most recently used terminal app FERB can open a window
    /// in, else Terminal.
    pub fn start_host(&self) -> (String, String) {
        self.list()
            .into_iter()
            .filter_map(|s| s.host)
            .find_map(|h| {
                OPENERS
                    .iter()
                    .find(|(b, _)| *b == h.bundle_id)
                    .map(|(b, n)| (b.to_string(), n.to_string()))
            })
            .unwrap_or_else(|| (OPENERS[0].0.to_string(), OPENERS[0].1.to_string()))
    }

    /// HAND-695: the question before a new terminal types the owner's command (GATE-585: the exact command, asked
    /// first). None when there is no command: opening a window types nothing.
    pub fn start_question(
        &self,
        folder: &str,
        command: &str,
    ) -> Result<Option<super::NeedsYes>, Refused> {
        let f = start_folder(folder)?;
        let command = command.trim();
        if command.is_empty() {
            return Ok(None);
        }
        let (_, app) = self.start_host();
        Ok(Some(super::NeedsYes {
            question: format!(
                "Open a new {app} window in {} and type this?",
                folder_name(&f)
            ),
            detail: command.to_owned(),
            covers: "FERB opens it in the background and types only this command, then Return. You can type more into it from here."
                .to_owned(),
        }))
    }

    /// HAND-695: opens a new terminal window in `folder` for the owner and, when he gave one, types his command into
    /// it. Answers the new session's id ("" when it hadn't appeared yet and nothing was to be typed) and the app.
    pub async fn start_terminal(
        &self,
        folder: &str,
        command: &str,
        by: &Caller,
    ) -> Result<(String, String), Refused> {
        if !self.enabled() {
            return Err(super::off());
        }
        let f = start_folder(folder)?;
        let command = command.trim().to_owned();
        let (bundle, app) = self.start_host();
        let before: HashSet<String> = self.list().into_iter().map(|s| s.id).collect();
        let (mac, b, fp) = (self.seams.mac.clone(), bundle.clone(), f.clone());
        tokio::task::spawn_blocking(move || mac.terminal_open_at(&b, &fp))
            .await
            .map_err(|e| no(codes::SEND_FAILED, e.to_string()))?
            .map_err(|e| no(codes::SEND_FAILED, e))?;
        (self.seams.journal)(
            "sessions.started",
            json!({"host": app, "folder": f, "by": by.key(), "command": self.redact(&command)}),
        );
        let deadline = (self.seams.mono)() + START_WAIT_S;
        let found = loop {
            let me = self.clone();
            let _ = tokio::task::spawn_blocking(move || me.look()).await;
            let hit = self.list().into_iter().find(|s| {
                !before.contains(&s.id)
                    && s.agent == Agent::Terminal
                    && same_folder(&s.cwd, &f)
                    && s.host.as_ref().is_some_and(|h| h.bundle_id == bundle)
            });
            if hit.is_some() || (self.seams.mono)() >= deadline {
                break hit;
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        };
        let Some(s) = found else {
            if command.is_empty() {
                return Ok((String::new(), app));
            }
            return Err(no(
                codes::SEND_FAILED,
                format!("A new {app} window opened, but FERB couldn't find it to type your command, so nothing was typed."),
            ));
        };
        if let Some(e) = lock(&self.state).sessions.get_mut(&s.id) {
            e.s.source = Source::StartedByFerb;
        }
        self.take_control(&s.id, by)?;
        if !command.is_empty() {
            // his Yes to the exact command (start_question) covers this one typing
            let path = *s
                .send_paths
                .first()
                .ok_or_else(|| no(codes::NO_SEND_PATH, s.no_send.clone()))?;
            self.deliver(&s, path, &command, "").await?;
            (self.seams.journal)(
                "sessions.sent",
                json!({"session": s.id, "by": by.key(), "path": path.as_str(), "text": self.redact(&command)}),
            );
        }
        Ok((s.id, app))
    }
}

/// A folder a new terminal may open in: an existing folder on this Mac, by its full path.
fn start_folder(folder: &str) -> Result<String, Refused> {
    let f = Path::new(folder.trim());
    if !f.is_absolute() || !f.is_dir() {
        return Err(no(codes::SEND_FAILED, "That folder isn't on this Mac."));
    }
    Ok(f.to_string_lossy().trim_end_matches('/').to_owned())
}

/// The same folder, through links ("/tmp" is "/private/tmp").
fn same_folder(a: &str, b: &str) -> bool {
    let real = |p: &str| std::fs::canonicalize(p).ok();
    a == b || (real(a).is_some() && real(a) == real(b))
}
