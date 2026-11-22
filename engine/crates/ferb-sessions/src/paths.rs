//! Which send paths a session has, in the order FERB tries them (HAND-656): the agent's own interface, a host's
//! background adapter, then the generic foreground path for any other app. A host without an adapter is never
//! "unsupported": it gets the foreground path. The phone never chooses a mechanism.

use crate::model::{Agent, Host, SendPath, Via};

/// Terminal's bundle id: the one host with a background scripting adapter here (`do script` in the session's tab).
pub const TERMINAL: &str = "com.apple.Terminal";

/// The owner's words when nothing can deliver.
pub const NO_WINDOW: &str = "No app shows this session, so FERB can't type into it.";

/// The paths for a session, and why there are none.
pub fn send_paths(agent: Agent, host: Option<&Host>, agent_api: bool) -> (Vec<SendPath>, String) {
    if agent == Agent::Codex && agent_api {
        return (vec![SendPath::AgentApi], String::new());
    }
    let Some(h) = host else {
        return (Vec::new(), NO_WINDOW.to_owned());
    };
    match h.via {
        Some(Via::Tmux) => (vec![SendPath::Tmux], String::new()),
        Some(Via::Direct) if h.bundle_id == TERMINAL => (
            vec![SendPath::TerminalScript, SendPath::Foreground],
            String::new(),
        ),
        Some(Via::Direct) | Some(Via::App) if h.pid > 0 => {
            (vec![SendPath::Foreground], String::new())
        }
        _ => (Vec::new(), NO_WINDOW.to_owned()),
    }
}
