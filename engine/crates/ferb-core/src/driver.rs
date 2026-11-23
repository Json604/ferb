//! A session driver's names (owner 2026-11-23, HAND-707): Hand to FERB on one of the owner's sessions runs a driver,
//! not a chat turn. Its task's request id carries a mark any part can read from the id alone, so the context switch
//! (ferb-supervisor `fresh_for`) and every store reader (restart's sweep, the Tasks page) tell it apart without
//! asking the daemon's memory.

/// A session driver's request id (and so its tasks' `request_id`) starts with this.
pub const REQUEST_PREFIX: &str = "driver-";

/// A session driver's request id made from a fresh id.
pub fn request_of(id: &str) -> String {
    format!("{REQUEST_PREFIX}{id}")
}

/// The request id is a session driver's.
pub fn is_request(request_id: &str) -> bool {
    request_id.starts_with(REQUEST_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_driver_is_read_from_its_id_alone() {
        assert!(is_request(&request_of("a1")));
        assert!(!is_request("temporary-a1"));
        assert!(!is_request("0f3a"));
        assert!(!is_request(""));
    }
}
