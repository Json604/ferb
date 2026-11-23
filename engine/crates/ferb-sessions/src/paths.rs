//! Which send paths a session has, in the order FERB tries them (HAND-656, HAND-712): the agent's own interface, a
//! host's exact background adapter (tmux, Terminal's scripting), the proven foreground path into an agent's own terminal,
//! else the session's window brought up and typed into while the owner watches it (HAND-710: "when not available just
//! watch live and type"). A host without an adapter is never "unsupported". The phone never chooses a mechanism.

use crate::model::{Agent, Host, SendPath, Via};

/// Terminal's bundle id: the one host with a background scripting adapter here (`do script` in the session's tab).
pub const TERMINAL: &str = "com.apple.Terminal";

/// The owner's words when nothing can deliver.
pub const NO_WINDOW: &str = "No app on your Mac shows this session, so FERB can't type into it.";

/// The paths for a session, and why there are none.
pub fn send_paths(agent: Agent, host: Option<&Host>, agent_api: bool) -> (Vec<SendPath>, String) {
    if agent == Agent::Codex && agent_api {
        return (vec![SendPath::AgentApi], String::new());
    }
    let Some(h) = host else {
        return (Vec::new(), NO_WINDOW.to_owned());
    };
    let agent_terminal = matches!(agent, Agent::ClaudeCode | Agent::Codex);
    match h.via {
        Some(Via::Tmux) => (vec![SendPath::Tmux], String::new()),
        Some(Via::Direct) if h.bundle_id == TERMINAL && !h.tty.is_empty() => {
            (vec![SendPath::TerminalScript], String::new())
        }
        // an agent in an app's terminal: typed there once code proves the terminal is its own (HAND-682/683)
        Some(Via::Direct) if agent_terminal && !h.tty.is_empty() && h.pid > 0 => {
            (vec![SendPath::Foreground], String::new())
        }
        // anything else an app shows: its window, watched and typed into (HAND-710)
        Some(_) if h.pid > 0 && !h.bundle_id.is_empty() => (vec![SendPath::Window], String::new()),
        _ => (Vec::new(), NO_WINDOW.to_owned()),
    }
}
