//! Whether the terminal FERB would type into is provably the session's (HAND-660, HAND-682), from what Accessibility
//! shows of the focused element and its window: structure, never meaning (AGENTS.md rule 4).
//!
//! Two proofs, either one enough, and both only in a terminal (code's structural test, ferb-policy's `surface`):
//! - the session's title in the focused element's words or its window's (an agent's terminal title: Terminal, iTerm);
//! - the agent's program as the Mac names it (its executable's file name, "2.1.330" for Claude Code's own installer)
//!   in the focused terminal's own words, with the session's folder in its window's words. This is how an editor's
//!   integrated terminal shows it (VS Code: "Terminal 1, 2.1.330" in a window titled "… — FERB"). The daemon offers
//!   it only when no other running session in that app shares the folder and the program, so it can't be another's.

/// What proves a terminal is the session's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Proof {
    /// The session's title (empty: none yet).
    pub title: String,
    /// The agent's program as the Mac names it (empty: not offered).
    pub program: String,
    /// The session's folder's name (empty: not offered).
    pub folder: String,
}

/// The focused element's words, as the Mac read them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Focus {
    /// The focused element's own words and its containers' below the window (titles, descriptions).
    pub own: String,
    /// Every word of the chain, the window's title among them.
    pub around: String,
    /// The chain is a terminal by structure (a terminal app, or xterm.js's DOM classes in an editor).
    pub terminal: bool,
    /// The whole app is a terminal (its window's title is the terminal's own).
    pub terminal_app: bool,
}

/// Why a focus isn't the session's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unproven {
    /// The cursor isn't in a terminal at all (an editor's text, a search field).
    NotATerminal,
    /// A terminal, but nothing shows it is this session's.
    NotTheSession,
}

/// Whether `focus` is the session's terminal.
pub fn proven(p: &Proof, focus: &Focus) -> Result<(), Unproven> {
    if !focus.terminal {
        return Err(Unproven::NotATerminal);
    }
    let title = p.title.trim().to_lowercase();
    if !title.is_empty() && focus.around.to_lowercase().contains(&title) {
        return Ok(());
    }
    let own = if focus.terminal_app {
        &focus.around
    } else {
        &focus.own
    };
    if !p.program.is_empty()
        && !p.folder.is_empty()
        && has_token(own, &p.program)
        && has_token(&focus.around, &p.folder)
    {
        return Ok(());
    }
    Err(Unproven::NotTheSession)
}

/// `token` in `text`, ignoring case, not glued to a letter or digit on either side.
pub fn has_token(text: &str, token: &str) -> bool {
    let (text, token) = (text.to_lowercase(), token.trim().to_lowercase());
    if token.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(i) = text[from..].find(&token) {
        let start = from + i;
        let end = start + token.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        if !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric) {
            return true;
        }
        from = start + text[start..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// An executable's file name ("" for none).
pub fn program_name(exe: &str) -> String {
    exe.rsplit('/').next().unwrap_or("").to_owned()
}

/// The text inputs in a window that name the program in their own words (xterm.js's input in an editor, labelled
/// "Terminal 1, 2.1.330"): where FERB may put the cursor when it isn't in the session's terminal. `inputs`: each
/// editable text element's label. Only one may match; else none (FERB can't tell which).
pub fn terminal_input(inputs: &[&str], program: &str) -> Option<usize> {
    let mut hits = inputs
        .iter()
        .enumerate()
        .filter(|(_, l)| has_token(l, program));
    let first = hits.next()?.0;
    hits.next().is_none().then_some(first)
}
