//! The stable refusal codes of the `sessions.*` calls, with the names the phones and FERB.app use (kb PHN-797). The
//! words that go with each are the daemon's, in the owner's language.

/// The session ended, or FERB doesn't know it.
pub const SESSION_GONE: i64 = -32040;
/// The agent isn't its terminal's foreground program: typed text would reach a shell or another program (HAND-658).
pub const NOT_IN_FRONT: i64 = -32041;
/// Nothing can deliver to this session (no app shows it).
pub const NO_SEND_PATH: i64 = -32042;
/// The agent is asking something: answer that first (typed text would answer its dialog).
pub const CARD_OPEN: i64 = -32043;
/// The owner turned sessions off (or never turned them on).
pub const SESSIONS_OFF: i64 = -32044;
/// The window or terminal FERB would type into isn't provably the session's.
pub const NOT_THE_SESSION: i64 = -32045;
/// The card was answered or withdrawn already.
pub const CARD_GONE: i64 = -32046;
/// FERB's own task may not answer the agent's permission prompts: they are the owner's.
pub const OWNERS_ONLY: i64 = -32047;
/// The input couldn't be sent (the app or the agent's interface refused it).
pub const SEND_FAILED: i64 = -32048;

/// The codes by name.
pub const CODES: &[(&str, i64)] = &[
    ("session_gone", SESSION_GONE),
    ("not_in_front", NOT_IN_FRONT),
    ("no_send_path", NO_SEND_PATH),
    ("card_open", CARD_OPEN),
    ("sessions_off", SESSIONS_OFF),
    ("not_the_session", NOT_THE_SESSION),
    ("card_gone", CARD_GONE),
    ("owners_only", OWNERS_ONLY),
    ("send_failed", SEND_FAILED),
];

/// A code's name ("" when it isn't one of these).
pub fn name(code: i64) -> &'static str {
    CODES
        .iter()
        .find(|(_, c)| *c == code)
        .map(|(n, _)| *n)
        .unwrap_or("")
}
