pub mod ledger;

// decision: APP-926
/// APP-926 (owner, 2026-11-16, and the owner's "ya change it" of 2026-11-17 for the older strings): the one named
/// difference in these Python recordings. Owner-facing words don't speak of AI, and Settings' "AI" tab is "Thinking":
/// each pair is a phrase Python said and the words the Rust engine says instead. Nothing else in a recording changes.
#[allow(dead_code)]
pub const APP_926: &[(&str, &str)] = &[
    ("Settings → AI", "Settings → Thinking"),
    ("Settings \\u2192 AI", "Settings \\u2192 Thinking"),
    ("unknown AI service ", "unknown service "),
    ("the AI service (", "the service FERB thinks with ("),
    ("the AI service I use", "the service I think with"),
    ("The AI service I use", "The service I think with"),
    ("the AI runtime", "the service FERB thinks with"),
    (
        "the AI's answer didn't fit",
        "the answer that came back didn't fit",
    ),
    (
        "; FERB's AI never sees it.",
        "; it never reaches the service FERB thinks with.",
    ),
    (
        ". FERB's AI never sees it.",
        ". It never reaches the service FERB thinks with.",
    ),
    (
        "(no AI sees it)",
        "(it never reaches the service FERB thinks with)",
    ),
    (
        "nothing came back from the AI service for",
        "nothing came back from the service I think with for",
    ),
    (
        "Pick another AI to finish it",
        "Pick another service to finish it",
    ),
    (
        "No other AI service is signed in for FERB",
        "No other service is signed in for FERB",
    ),
    ("Switch AI?", "Switch service?"),
    ("What the AI was given", "What FERB sent for thinking"),
    ("shown to the AI", "read by FERB"),
    (
        "sign in to its AI service if it says signed out",
        "sign in again in Settings → Thinking if it says signed out",
    ),
];

/// [`APP_926`] applied to every string of a recording.
#[allow(dead_code)]
pub fn app_926(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::String(s) => Value::String(
            APP_926
                .iter()
                .fold(s, |s, (python, rust)| s.replace(python, rust)),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(app_926).collect()),
        Value::Object(m) => Value::Object(m.into_iter().map(|(k, v)| (k, app_926(v))).collect()),
        other => other,
    }
}
