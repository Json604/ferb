//! Which of the host app's windows shows a session (HAND-682, HAND-683, PHN-797): the one place both the send path
//! and the Watch view find it. Structure only: the window server's ids, titles and whether a window is on screen.
//!
//! In order: an app chat's own window (HAND-672); the window of the terminal the owner chose (HAND-683); a window
//! whose title holds the session's title; one whose title names the session's folder as a whole word (the send
//! proof's folder test, VS Code's "… — FERB"); the app's only window; else the app's front window. Among several
//! matches the one on this Space comes first, front to back. An app keeps windows of its own that show nothing (an
//! editor's off-screen strips, a transparent bar): only titled windows count, or, when the Mac gives no titles, the
//! windows big enough to show a terminal.

use crate::proof::{has_token, Proof};

/// One of the host app's normal windows, front to back as the window server lists them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Win {
    /// The window server's id.
    pub id: i64,
    pub title: String,
    /// Its size (width, height) in points.
    pub size: (f64, f64),
    /// On the Space the owner is on now (the window server's on-screen list).
    pub on_screen: bool,
}

/// The smallest window (each side, in points) that counts when the Mac gives no titles.
pub const MIN_SIDE: f64 = 120.0;

/// The window that shows the session, and how it was found.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub id: i64,
    pub title: String,
    pub on_screen: bool,
}

/// The window of the terminal the owner chose ("w<window>:<n>:<its words>", HAND-683), None for "front" or none.
pub fn chosen_window(chosen: &str) -> Option<i64> {
    chosen
        .split(':')
        .next()
        .and_then(|w| w.strip_prefix('w'))
        .and_then(|x| x.parse::<i64>().ok())
}

/// The windows whose title names the folder as a whole word (the send proof's folder test, HAND-682).
pub fn naming_folder<'a>(wins: &'a [Win], folder: &str) -> Vec<&'a Win> {
    wins.iter()
        .filter(|w| !folder.trim().is_empty() && has_token(&w.title, folder))
        .collect()
}

/// The windows that can show something: titled ones, or, when none has a title, the big enough ones.
fn showing(wins: &[Win]) -> Vec<&Win> {
    let titled: Vec<&Win> = wins.iter().filter(|w| !w.title.trim().is_empty()).collect();
    if !titled.is_empty() {
        return titled;
    }
    wins.iter()
        .filter(|w| w.size.0 >= MIN_SIDE && w.size.1 >= MIN_SIDE)
        .collect()
}

/// The front-most of these, the ones on this Space first.
fn front<'a>(of: &[&'a Win]) -> Option<&'a Win> {
    of.iter()
        .find(|w| w.on_screen)
        .or_else(|| of.first())
        .copied()
}

/// The session's window among the host app's (`wins`, front to back). `known`: an app chat's own window (0: none);
/// `chosen`: the terminal the owner chose for it (HAND-683, "" when none); `proof`'s title and folder name it.
pub fn session_window(wins: &[Win], proof: &Proof, chosen: &str, known: i64) -> Option<Found> {
    let found = |w: &Win| Found {
        id: w.id,
        title: w.title.clone(),
        on_screen: w.on_screen,
    };
    for id in [Some(known).filter(|k| *k > 0), chosen_window(chosen)]
        .into_iter()
        .flatten()
    {
        if let Some(w) = wins.iter().find(|w| w.id == id) {
            return Some(found(w));
        }
    }
    let shown = showing(wins);
    let title = proof.title.trim().to_lowercase();
    let by_title: Vec<&Win> = shown
        .iter()
        .copied()
        .filter(|w| !title.is_empty() && w.title.to_lowercase().contains(&title))
        .collect();
    if let Some(w) = front(&by_title) {
        return Some(found(w));
    }
    let by_folder: Vec<&Win> = shown
        .iter()
        .copied()
        .filter(|w| !proof.folder.trim().is_empty() && has_token(&w.title, &proof.folder))
        .collect();
    if let Some(w) = front(&by_folder) {
        return Some(found(w));
    }
    front(&shown).map(found)
}

/// Why the session's window can't be watched now, in the owner's words. `app`: the host app's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unseen {
    /// The app shows no window at all.
    NoWindow,
    /// Its window is minimized in the Dock.
    Minimized,
    /// Its window is on another Space (or another desktop's full screen).
    OtherSpace,
}

impl Unseen {
    /// What the Watch view says.
    pub fn words(self, app: &str, title: &str) -> String {
        let app = if app.trim().is_empty() {
            "its app"
        } else {
            app.trim()
        };
        let named = if title.trim().is_empty() {
            format!("Its window in {app}")
        } else {
            format!("Its window in {app} (“{}”)", crate::clip(title, 60))
        };
        match self {
            Unseen::NoWindow => format!("{app} has no window open for this session."),
            Unseen::Minimized => format!(
                "{named} is minimized in the Dock. Open it again and Watch shows it; FERB doesn't bring windows back on its own."
            ),
            Unseen::OtherSpace => format!(
                "{named} is on another desktop (Space). Watch shows it once that desktop is the one on screen; FERB doesn't switch desktops on its own."
            ),
        }
    }
}
