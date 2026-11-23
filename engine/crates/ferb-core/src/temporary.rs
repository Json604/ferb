//! A temporary chat's names (owner 2026-11-23, ORCH-699): the conversation and every message of it carry a mark any
//! part can read from the id alone, so a store reader (the conversation index, the Tasks page, the start-up sweep,
//! learning) leaves a temporary chat out without asking the daemon's memory.
//!
//! The conversation is `temporary:<id>` (a reserved session, like `schedule:` and `preview:`); each message's request
//! id is `temporary-<id>`, and its tasks keep that request id.

/// A temporary chat's conversation (`chat.send`'s session) starts with this.
pub const SESSION_PREFIX: &str = "temporary:";
/// A temporary chat's message (its request id, and so its tasks' `request_id`) starts with this.
pub const REQUEST_PREFIX: &str = "temporary-";

/// The conversation of the temporary chat `id`.
pub fn session_of(id: &str) -> String {
    format!("{SESSION_PREFIX}{id}")
}

/// A temporary chat's request id made from a fresh id.
pub fn request_of(id: &str) -> String {
    format!("{REQUEST_PREFIX}{id}")
}

/// The conversation is a temporary chat's.
pub fn is_session(session: &str) -> bool {
    session.starts_with(SESSION_PREFIX)
}

/// The message (request id) is a temporary chat's.
pub fn is_request(request_id: &str) -> bool {
    request_id.starts_with(REQUEST_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_read_from_the_id_alone() {
        assert!(is_session(&session_of("a1")));
        assert!(is_request(&request_of("b2")));
        assert!(!is_session("owner"));
        assert!(!is_session("schedule:x"));
        assert!(!is_request("0f3a"));
        assert!(!is_request(""));
    }
}
