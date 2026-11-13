//! ferb/orchestrator/availability.py: installed capability facts for planning, without routing or permission
//! decisions.

use std::collections::BTreeSet;

/// availability.py:8 `Group`.
pub struct Group {
    pub key: &'static str,
    pub name: &'static str,
    pub scopes: &'static [&'static str],
    pub text: &'static str,
}

pub const GROUPS: &[Group] = &[
    Group { key: "files", name: "files", scopes: &["fs:read", "fs:write"],
        text: "Files: list, read, write, edit and move to Trash. Relative paths use FERB's workspace; absolute paths use the supplied location and operating-system access." },
    Group { key: "owner_browsers", name: "the owner's browsers", scopes: &["browser:read", "browser:act"],
        text: "The owner's browsers, connection observations: {profiles} — open pages in their signed-in sessions, read them, click/type/select, or take over a tab the owner already has open. For work that needs the owner's accounts or a profile they name (prompts decision 5); login codes and passwords come through FERB's hidden input." },
    Group { key: "ferb_browser", name: "FERB's own browser", scopes: &["identity:browse", "secrets:fill", "mailbox:send"],
        text: "FERB's own browser ({scopes}): the default for web work, FERB's own accounts, passwords from the Keychain and codes from FERB's inbox (the worker never sees them), and sending email from FERB's own address (mailbox:send)." },
    Group { key: "time", name: "reminders and timers", scopes: &["time"],
        text: "Time (either identity): reminders and timers for the owner, and pausing a task until a time (time)." },
    Group { key: "mac", name: "the owner's Mac apps", scopes: &["mac:read", "mac:act", "applescript"],
        text: "The owner's Mac apps: through Accessibility{and_applescript} ({scopes}), from the background: the owner stays in their own app. A goal is what to read or do in it (\"Read the top chat in WhatsApp\")." },
    Group { key: "screen", name: "screenshots of app windows", scopes: &["screen"],
        text: "A screenshot of one app window, with screenshot-bound pointer input (screen, mac:act)." },
    Group { key: "hearing", name: "hearing app sound", scopes: &["audio"],
        text: "Hearing (audio): what one app or a task's tab is playing, as a transcript recognized on this Mac only; recordings can include the sound. The media must be playing and unmuted." },
    Group { key: "shell", name: "shell commands", scopes: &["shell"], text: "Shell commands on the owner's Mac (shell)." },
    Group { key: "trail", name: "the owner's activity record", scopes: &["trail:read"],
        text: "FERB's record of the owner's recent activity (trail:read)." },
];

/// availability.py:47 `Availability`.
#[derive(Debug, Clone, PartialEq)]
pub struct Availability {
    pub scopes: BTreeSet<String>,
    pub text: String,
    pub missing: Vec<String>,
}

impl Availability {
    pub fn unavailable_line(&self) -> String {
        if self.missing.is_empty() {
            String::new()
        } else {
            format!("Unavailable: {}.", self.missing.join("; "))
        }
    }
}

/// availability.py:57 `build`. `problems`: group key -> why it can't be used now, in the dict's order (only
/// membership is read).
pub fn build(
    registered: &BTreeSet<String>,
    problems: &[(String, String)],
    profiles: &str,
) -> Availability {
    let mut lines: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for g in GROUPS {
        if let Some((_, why)) = problems.iter().find(|(k, _)| k == g.key) {
            missing.push(format!("{} ({why})", g.name));
        }
        let present: BTreeSet<&str> = g
            .scopes
            .iter()
            .copied()
            .filter(|s| registered.contains(*s))
            .collect();
        if present.is_empty() {
            continue;
        }
        let profiles = if profiles.is_empty() {
            "the paired profiles"
        } else {
            profiles
        };
        let scopes = present.iter().copied().collect::<Vec<_>>().join(", ");
        let applescript = if present.contains("applescript") {
            " and AppleScript"
        } else {
            ""
        };
        let line = crate::py::format_named(
            g.text,
            &[
                ("profiles", profiles),
                ("scopes", &scopes),
                ("and_applescript", applescript),
            ],
        );
        lines.push(format!("- {line}"));
    }
    let mut text = format!("Installed worker capabilities:\n{}", lines.join("\n"));
    if !missing.is_empty() {
        text.push_str(&format!(
            "\nConnection/permission observations: {}.",
            missing.join("; ")
        ));
    }
    Availability {
        scopes: registered.clone(),
        text,
        missing,
    }
}
