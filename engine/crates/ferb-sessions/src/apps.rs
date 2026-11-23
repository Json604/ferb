//! The agent and chat apps whose open windows are sessions (HAND-711): each window listed by structure alone, its app
//! and its title, never by what it shows. Data, not judgment: which apps' windows to list. The owner adds others in
//! FERB's config (`[sessions] chat_apps`, bundle ids); a terminal app's tabs are found as terminals instead.

/// (bundle id, the app's usual name). The name the owner sees is the running app's own; this one is a fallback.
pub const KNOWN: &[(&str, &str)] = &[
    ("com.todesktop.230313mzl4w4u92", "Cursor"),
    ("com.anthropic.claudefordesktop", "Claude"),
    ("com.openai.chat", "ChatGPT"),
    ("com.openai.codex", "Codex"),
    ("com.microsoft.VSCode", "Visual Studio Code"),
    (
        "com.microsoft.VSCodeInsiders",
        "Visual Studio Code - Insiders",
    ),
    ("com.exafunction.windsurf", "Windsurf"),
    ("dev.zed.Zed", "Zed"),
];

/// HAND-712: Claude Code's own record names who runs it (`entrypoint`); a session with no terminal is shown by this app.
pub const ENTRYPOINT_APPS: &[(&str, &str)] =
    &[("claude-desktop", "com.anthropic.claudefordesktop")];

/// The bundle ids whose windows are listed: the known ones and the ones the owner added, once each, sorted.
pub fn bundles(added: &[String]) -> Vec<String> {
    let mut v: Vec<String> = KNOWN.iter().map(|(b, _)| b.to_string()).collect();
    v.extend(
        added
            .iter()
            .map(|b| b.trim().to_owned())
            .filter(|b| !b.is_empty()),
    );
    v.sort();
    v.dedup();
    v
}

/// The app a Claude Code session's `entrypoint` names, if any.
pub fn of_entrypoint(entrypoint: &str) -> Option<&'static str> {
    ENTRYPOINT_APPS
        .iter()
        .find(|(e, _)| *e == entrypoint)
        .map(|(_, b)| *b)
}

/// One open window of such an app, as the Mac lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct AppWindow {
    pub bundle_id: String,
    /// The app's name as the owner sees it.
    pub app: String,
    /// The app's process (the one that owns the window).
    pub pid: i32,
    /// The window server's id.
    pub window: i64,
    pub title: String,
    /// On the desktop (Space) on screen now.
    pub on_screen: bool,
}

/// A window's session id: `app:<bundle id>:<window id>`.
pub fn session_id(w: &AppWindow) -> String {
    crate::model::Agent::AppChat.session_id(&format!("{}:{}", w.bundle_id, w.window))
}

/// One window as the window server lists it (every desktop), for [`pick`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listed {
    pub pid: i32,
    pub id: i64,
    pub title: String,
    /// An app's normal window (layer 0).
    pub normal: bool,
    pub alpha: f64,
    pub size: (f64, f64),
    /// On the desktop on screen now.
    pub on_screen: bool,
}

/// The smallest window (width, height in points) that is listed: an app's own strips and bars are smaller.
pub const MIN_SIZE: (f64, f64) = (300.0, 200.0);

/// HAND-711: the sessions' windows among `wins` (front to back): those of the running `apps` ((bundle id, name, pid),
/// already only the listed apps), normal, drawn, titled and big enough to show something.
pub fn pick(apps: &[(String, String, i32)], wins: &[Listed]) -> Vec<AppWindow> {
    wins.iter()
        .filter(|w| {
            w.normal
                && w.alpha > 0.0
                && !w.title.trim().is_empty()
                && w.size.0 >= MIN_SIZE.0
                && w.size.1 >= MIN_SIZE.1
        })
        .filter_map(|w| {
            let (bundle, name, pid) = apps.iter().find(|a| a.2 == w.pid)?;
            Some(AppWindow {
                bundle_id: bundle.clone(),
                app: name.clone(),
                pid: *pid,
                window: w.id,
                title: w.title.clone(),
                on_screen: w.on_screen,
            })
        })
        .collect()
}
