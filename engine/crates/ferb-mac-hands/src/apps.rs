//! Running apps and the one browser list (ferb/act/mac/apps.py, browsers.py): lookup rules over the running-app list
//! and the facts an app's Info.plist declares. Must not resolve ambiguity by order. The NSWorkspace, Launch Services
//! and NSRunningApplication calls are `native::apps`; everything here is pure.

use std::sync::Mutex;

use serde_json::Value;

/// One running app (apps.py:9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningApp {
    pub bundle_id: String,
    pub name: String,
    pub pid: i32,
    pub active: bool,
}

impl RunningApp {
    pub fn new(bundle_id: &str, name: &str, pid: i32, active: bool) -> Self {
        RunningApp {
            bundle_id: bundle_id.into(),
            name: name.into(),
            pid,
            active,
        }
    }
}

/// A Chromium browser FERB pairs with (ferb/act/browser/profiles.py BROWSERS): key, bundle id, label, app name,
/// user-data folder under ~/Library/Application Support.
pub const BROWSERS: [(&str, &str, &str, &str, &str); 13] = [
    (
        "chrome",
        "com.google.Chrome",
        "Google Chrome",
        "Google Chrome",
        "Google/Chrome",
    ),
    (
        "chrome-beta",
        "com.google.Chrome.beta",
        "Google Chrome Beta",
        "Google Chrome Beta",
        "Google/Chrome Beta",
    ),
    (
        "chrome-dev",
        "com.google.Chrome.dev",
        "Google Chrome Dev",
        "Google Chrome Dev",
        "Google/Chrome Dev",
    ),
    (
        "chrome-canary",
        "com.google.Chrome.canary",
        "Google Chrome Canary",
        "Google Chrome Canary",
        "Google/Chrome Canary",
    ),
    (
        "chromium",
        "org.chromium.Chromium",
        "Chromium",
        "Chromium",
        "Chromium",
    ),
    (
        "brave",
        "com.brave.Browser",
        "Brave",
        "Brave Browser",
        "BraveSoftware/Brave-Browser",
    ),
    (
        "edge",
        "com.microsoft.edgemac",
        "Microsoft Edge",
        "Microsoft Edge",
        "Microsoft Edge",
    ),
    (
        "edge-beta",
        "com.microsoft.edgemac.Beta",
        "Microsoft Edge Beta",
        "Microsoft Edge Beta",
        "Microsoft Edge Beta",
    ),
    (
        "edge-dev",
        "com.microsoft.edgemac.Dev",
        "Microsoft Edge Dev",
        "Microsoft Edge Dev",
        "Microsoft Edge Dev",
    ),
    (
        "edge-canary",
        "com.microsoft.edgemac.Canary",
        "Microsoft Edge Canary",
        "Microsoft Edge Canary",
        "Microsoft Edge Canary",
    ),
    (
        "vivaldi",
        "com.vivaldi.Vivaldi",
        "Vivaldi",
        "Vivaldi",
        "Vivaldi",
    ),
    (
        "arc",
        "company.thebrowser.Browser",
        "Arc",
        "Arc",
        "Arc/User Data",
    ),
    (
        "opera",
        "com.operasoftware.Opera",
        "Opera",
        "Opera",
        "com.operasoftware.Opera",
    ),
];

const OTHER_BROWSERS: [&str; 12] = [
    "com.apple.Safari",
    "com.apple.SafariTechnologyPreview",
    "org.mozilla.firefox",
    "org.mozilla.firefoxdeveloperedition",
    "org.mozilla.nightly",
    "com.kagi.kagimacOS",
    "com.duckduckgo.macos.browser",
    "io.gitlab.librewolf-community",
    "ai.perplexity.comet",
    "ru.yandex.desktop.yandex-browser",
    "one.ablaze.floorp",
    "net.mullvad.mullvadbrowser",
];
const BROWSER_VENDORS: [&str; 8] = [
    "com.operasoftware",
    "com.vivaldi",
    "company.thebrowser",
    "org.chromium",
    "org.torproject",
    "app.zen-browser",
    "net.waterfox",
    "com.sigmaos",
];
const SUPPORT: &str = "~/Library/Application Support/";
const BROWSER_DATA: [(&str, &[&str]); 17] = [
    (
        "com.apple.Safari",
        &["~/Library/Safari", "~/Library/Containers/com.apple.Safari"],
    ),
    (
        "com.apple.SafariTechnologyPreview",
        &[
            "~/Library/SafariTechnologyPreview",
            "~/Library/Containers/com.apple.SafariTechnologyPreview",
        ],
    ),
    ("com.brave.Browser", &["BraveSoftware"]),
    ("company.thebrowser", &["Arc", "Dia"]),
    ("com.operasoftware", &["com.operasoftware.OperaGX"]),
    ("org.mozilla.firefox", &["Firefox"]),
    ("com.kagi.kagimacOS", &["Orion"]),
    (
        "com.duckduckgo.macos.browser",
        &["~/Library/Containers/com.duckduckgo.macos.browser"],
    ),
    ("io.gitlab.librewolf-community", &["librewolf"]),
    ("ai.perplexity.comet", &["Comet"]),
    ("ru.yandex.desktop.yandex-browser", &["Yandex"]),
    ("one.ablaze.floorp", &["Floorp"]),
    ("net.mullvad.mullvadbrowser", &["MullvadBrowser"]),
    ("org.torproject", &["TorBrowser-Data"]),
    ("app.zen-browser", &["zen"]),
    ("net.waterfox", &["Waterfox"]),
    ("org.chromium", &["Thorium"]),
];
const EXTRA_NAMES: [&str; 20] = [
    "safari",
    "safari technology preview",
    "firefox",
    "firefox developer edition",
    "firefox nightly",
    "orion",
    "duckduckgo",
    "librewolf",
    "comet",
    "yandex",
    "floorp",
    "mullvad browser",
    "tor browser",
    "zen",
    "zen browser",
    "waterfox",
    "sigmaos",
    "opera gx",
    "dia",
    "thorium",
];

fn invisible(c: char) -> bool {
    matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
}

/// browsers.py:118: invisible marks removed, outer spaces trimmed, casefolded.
pub fn clean(name: &str) -> String {
    let kept: String = name.chars().filter(|c| !invisible(*c)).collect();
    ferb_sanitize::parity::casefold(ferb_py::value::strip(&kept))
}

/// The name with invisible marks removed only (browsers.INVISIBLE.sub).
pub fn without_marks(name: &str) -> String {
    name.chars().filter(|c| !invisible(*c)).collect()
}

fn families() -> Vec<String> {
    BROWSERS
        .iter()
        .map(|b| b.1)
        .chain(OTHER_BROWSERS.iter().copied())
        .chain(BROWSER_VENDORS.iter().copied())
        .map(ferb_sanitize::parity::casefold)
        .collect()
}

/// browsers.py:132.
pub fn pair_key(bundle_id: &str) -> String {
    let b = clean(bundle_id);
    if b == "com.apple.safari" {
        return "safari".into();
    }
    BROWSERS
        .iter()
        .find(|v| ferb_sanitize::parity::casefold(v.1) == b)
        .map(|v| v.0.to_string())
        .unwrap_or_else(|| bundle_id.to_string())
}

/// browsers.py:141: a bundle id of a browser FERB knows (no I/O).
pub fn known(bundle_id: &str) -> bool {
    let b = clean(bundle_id);
    families()
        .iter()
        .any(|f| b == *f || b.starts_with(&format!("{f}.")))
}

/// browsers.py:84.
pub fn data_folders() -> Vec<String> {
    let mut folders: Vec<String> = vec![];
    for (_, found) in BROWSER_DATA {
        for f in found {
            folders.push(if f.starts_with('~') {
                f.to_string()
            } else {
                format!("{SUPPORT}{f}")
            });
        }
    }
    for b in BROWSERS {
        folders.push(format!("{SUPPORT}{}", b.4));
    }
    let mut unique: Vec<String> = vec![];
    for f in folders {
        if !unique.contains(&f) {
            unique.push(f);
        }
    }
    unique
        .iter()
        .filter(|f| !unique.iter().any(|o| f.starts_with(&format!("{o}/"))))
        .cloned()
        .collect()
}

fn known_names() -> Vec<String> {
    BROWSERS
        .iter()
        .flat_map(|b| [b.2, b.3])
        .chain(EXTRA_NAMES.iter().copied())
        .map(ferb_sanitize::parity::casefold)
        .collect()
}

/// The Launch Services list of apps that open web links (browsers.py:150-212), enumerated by `native::apps` and
/// kept HANDLERS_TTL_S.
#[derive(Default)]
pub struct WebHandlers {
    last: Mutex<Option<(f64, Vec<String>, Vec<String>)>>,
}

pub const HANDLERS_TTL_S: f64 = 60.0;

impl WebHandlers {
    /// The list as last enumerated, whatever its age.
    pub fn cached(&self) -> (Vec<String>, Vec<String>) {
        self.last
            .lock()
            .expect("lock")
            .as_ref()
            .map(|l| (l.1.clone(), l.2.clone()))
            .unwrap_or_default()
    }
    pub fn fresh(&self, now: f64) -> Option<(Vec<String>, Vec<String>)> {
        self.last
            .lock()
            .expect("lock")
            .as_ref()
            .filter(|l| now - l.0 < HANDLERS_TTL_S)
            .map(|l| (l.1.clone(), l.2.clone()))
    }
    /// Blocking: the list, enumerated at most once per TTL; when enumeration fails, the last list.
    pub fn installed(
        &self,
        now: f64,
        enumerate: impl FnOnce() -> Option<(Vec<String>, Vec<String>)>,
    ) -> (Vec<String>, Vec<String>) {
        if let Some(f) = self.fresh(now) {
            return f;
        }
        match enumerate() {
            Some((ids, names)) => {
                *self.last.lock().expect("lock") = Some((now, ids.clone(), names.clone()));
                (ids, names)
            }
            None => self.cached(),
        }
    }
    pub fn set(&self, now: f64, ids: Vec<String>, names: Vec<String>) {
        *self.last.lock().expect("lock") = Some((now, ids, names));
    }
    /// browsers.py:202: from a list enumerated within the TTL, never enumerating.
    pub fn known_opens_web_links(&self, bundle_id: &str, now: f64) -> bool {
        self.fresh(now)
            .is_some_and(|(ids, _)| ids.contains(&clean(bundle_id)))
    }
    /// browsers.py:208.
    pub fn is_browser(&self, bundle_id: &str) -> bool {
        known(bundle_id) || self.cached().0.contains(&clean(bundle_id))
    }
}

fn bundle_like(text: &str) -> bool {
    static R: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"^[a-z0-9-]+(?:\.[a-z0-9_-]+){2,}$").expect("fixed"))
        .is_match(text)
}

/// browsers.py:222: whether a Mac tool's `app` argument names a web browser. `running` gives the bundle ids of
/// running apps by pid or cleaned name.
pub fn is_browser_app(
    handlers: &WebHandlers,
    app_id: &str,
    running: &dyn Fn(Option<i32>, Option<&str>) -> Vec<String>,
) -> bool {
    let q = clean(app_id);
    if q.is_empty() {
        return false;
    }
    if let Some(rest) = q.strip_prefix("pid:") {
        let Ok(pid) = py_int(rest) else { return false };
        return running(Some(pid), None)
            .iter()
            .any(|b| handlers.is_browser(b));
    }
    if bundle_like(&q) {
        return handlers.is_browser(&q);
    }
    if known_names().contains(&q) || handlers.cached().1.contains(&q) {
        return true;
    }
    running(None, Some(&q))
        .iter()
        .any(|b| handlers.is_browser(b))
}

/// Python's `int(text)` for a pid: surrounding whitespace and an underscore-free sign allowed.
pub fn py_int(text: &str) -> Result<i32, ()> {
    let t = ferb_py::value::strip(text);
    let (neg, t) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    if t.is_empty() || t.starts_with('_') || t.ends_with('_') || t.contains("__") {
        return Err(());
    }
    let digits: String = t.chars().filter(|c| *c != '_').collect();
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(());
    }
    let n: i64 = digits.parse().map_err(|_| ())?;
    i32::try_from(if neg { -n } else { n }).map_err(|_| ())
}

/// apps.py:36: every matching process (exact lookup, never a first match).
pub fn candidates(running: &[RunningApp], name_or_bundle: &str) -> Vec<RunningApp> {
    if let Some(rest) = name_or_bundle.strip_prefix("pid:") {
        let Ok(pid) = py_int(rest) else { return vec![] };
        return running.iter().filter(|a| a.pid == pid).cloned().collect();
    }
    let q = clean(name_or_bundle);
    running
        .iter()
        .filter(|a| ferb_sanitize::parity::casefold(&a.bundle_id) == q || clean(&a.name) == q)
        .cloned()
        .collect()
}

pub fn find(running: &[RunningApp], name_or_bundle: &str) -> Option<RunningApp> {
    let m = candidates(running, name_or_bundle);
    if m.len() == 1 {
        m.into_iter().next()
    } else {
        None
    }
}

const SHELL_TYPES: [&str; 3] = [
    "public.unix-executable",
    "com.apple.terminal.shell-script",
    "public.shell-script",
];

fn doc_types(info: Option<&Value>) -> Vec<Value> {
    info.and_then(|i| i.get("CFBundleDocumentTypes"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| {
                    x.as_str()
                        .map(String::from)
                        .unwrap_or_else(|| x.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// apps.py:69: an Info.plist with the Shell role for executables or shell scripts.
pub fn shell_role(info: Option<&Value>) -> bool {
    doc_types(info).iter().any(|t| {
        t.get("CFBundleTypeRole").and_then(Value::as_str) == Some("Shell")
            && str_list(t.get("LSItemContentTypes"))
                .iter()
                .any(|u| SHELL_TYPES.contains(&u.as_str()))
    })
}

/// apps.py:186: an Info.plist declaring the app the Editor of a file of type `kind` or with `extension`.
pub fn editor_role(
    info: Option<&Value>,
    kind: &str,
    extension: &str,
    conforms: &dyn Fn(&str, &str) -> bool,
) -> bool {
    for t in doc_types(info) {
        if t.get("CFBundleTypeRole").and_then(Value::as_str) != Some("Editor") {
            continue;
        }
        if !kind.is_empty()
            && str_list(t.get("LSItemContentTypes"))
                .iter()
                .any(|u| conforms(kind, u))
        {
            return true;
        }
        if !extension.is_empty()
            && str_list(t.get("CFBundleTypeExtensions"))
                .iter()
                .any(|e| ferb_sanitize::parity::casefold(e) == extension)
        {
            return true;
        }
    }
    false
}

/// Launch Services types whose files start code when opened (apps.py:166).
pub const RUNNABLE_TYPES: [&str; 10] = [
    "public.executable",
    "public.script",
    "com.apple.installer-package-archive",
    "com.apple.installer-package",
    "com.apple.installer-meta-package",
    "com.apple.mobileconfig",
    "com.apple.configprofile",
    "com.apple.shortcut",
    "com.apple.shortcuts.workflow-file",
    "com.apple.automator-workflow",
];
pub const PROFILE_TYPES: [&str; 2] = ["com.apple.mobileconfig", "com.apple.configprofile"];
pub const OPEN_WAIT_S: f64 = 15.0;

/// apps.py:151 (HAND-239): `/usr/bin/open`'s arguments that launch an installed app in the background: `-g` never
/// brings it forward, and no `-j` (a hidden app's windows are off screen, so captures and pointer input can't reach
/// them).
pub fn launch_args(bundle_id: &str) -> Vec<String> {
    vec!["-g".into(), "-b".into(), bundle_id.into()]
}

/// apps.py:257 (HAND-240): a file or address is handed over with NSWorkspace's open configuration `activates` off.
pub const OPEN_ACTIVATES: bool = false;
