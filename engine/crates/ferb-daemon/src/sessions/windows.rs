//! HAND-711: every open window of an agent or chat app as a session, listed by structure alone (its app and its title,
//! from the window server), never by what it shows. FERB reads none of its words: the owner watches it live (Watch)
//! and types into it ("Type here"), and FERB types his words into that window (HAND-712's window path). A window that
//! closes leaves the list at the next look.

use std::collections::HashSet;

use ferb_sessions::apps::{self, AppWindow};
use ferb_sessions::model::*;
use ferb_sessions::paths;

use super::{lock, Entry, Sessions};

impl Sessions {
    /// One look's app windows: new ones listed, changed titles followed, closed ones gone.
    pub(super) fn note_windows(&self) {
        let bundles = apps::bundles(&(self.seams.chat_apps)());
        let windows = self.seams.mac.app_windows(&bundles);
        let now = self.now();
        let mut live: HashSet<String> = HashSet::new();
        for w in windows {
            let id = apps::session_id(&w);
            live.insert(id.clone());
            let host = Host {
                bundle_id: w.bundle_id.clone(),
                name: w.app.clone(),
                pid: w.pid,
                via: Some(Via::App),
                tty: String::new(),
            };
            let (paths_, why) = paths::send_paths(Agent::AppChat, Some(&host), false);
            let title = self.redact(&ferb_sessions::clip(
                &ferb_sessions::one_line(&w.title),
                160,
            ));
            let mut st = lock(&self.state);
            let e = st
                .sessions
                .entry(id.clone())
                .or_insert_with(|| Entry::new(&id, Agent::AppChat, &now));
            e.s.pid = w.pid;
            e.s.title = title.clone();
            e.s.window = title;
            e.s.host = Some(host);
            e.s.send_paths = paths_;
            e.s.no_send = why;
            e.s.watch = true;
            e.window = w.window;
        }
        let gone: Vec<String> = lock(&self.state)
            .sessions
            .iter()
            .filter(|(id, e)| e.s.agent == Agent::AppChat && !live.contains(*id))
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            self.end(&id);
        }
    }

    /// HAND-712: the running app's window to show a session with no terminal that its own record says this app runs
    /// (Claude Code run by the Claude app): the app's first window, as a host.
    pub(super) fn app_host(&self, bundle_id: &str) -> Option<Host> {
        let w: AppWindow = self
            .seams
            .mac
            .app_windows(&[bundle_id.to_owned()])
            .into_iter()
            .next()?;
        Some(Host {
            bundle_id: w.bundle_id,
            name: w.app,
            pid: w.pid,
            via: Some(Via::App),
            tty: String::new(),
        })
    }
}
