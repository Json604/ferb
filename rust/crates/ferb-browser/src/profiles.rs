//! Browser profiles: a port of `ferb/act/browser/profiles.py`'s logic (the browsers FERB knows,
//! profile discovery from Local State, how a profile is named and matched, the FERB copy's state in
//! a profile's settings) and `resolver.py` (the profile a task's words name; decisions: HAND-2,
//! HAND-407, HAND-412, HAND-414, HAND-416, HAND-424).
//!
//! Seams: [`Platform`] (Launch Services: whether a browser's app is installed, where its executable
//! is, which apps open https links; whether a program runs). The Mac's implementation is
//! [`crate::mac_profiles::MacPlatform`]. Files are read with `std::fs` under the folders given.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// A browser FERB knows (`Browser`): the name the owner knows, its user-data folder under
/// ~/Library/Application Support, its bundle id, its app's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Browser {
    pub key: &'static str,
    pub label: &'static str,
    pub data: &'static str,
    pub bundle_id: &'static str,
    pub app: &'static str,
}

/// `BROWSERS`, in its order.
pub const BROWSERS: [Browser; 13] = [
    Browser {
        key: "chrome",
        label: "Google Chrome",
        data: "Google/Chrome",
        bundle_id: "com.google.Chrome",
        app: "Google Chrome",
    },
    Browser {
        key: "chrome-beta",
        label: "Google Chrome Beta",
        data: "Google/Chrome Beta",
        bundle_id: "com.google.Chrome.beta",
        app: "Google Chrome Beta",
    },
    Browser {
        key: "chrome-dev",
        label: "Google Chrome Dev",
        data: "Google/Chrome Dev",
        bundle_id: "com.google.Chrome.dev",
        app: "Google Chrome Dev",
    },
    Browser {
        key: "chrome-canary",
        label: "Google Chrome Canary",
        data: "Google/Chrome Canary",
        bundle_id: "com.google.Chrome.canary",
        app: "Google Chrome Canary",
    },
    Browser {
        key: "chromium",
        label: "Chromium",
        data: "Chromium",
        bundle_id: "org.chromium.Chromium",
        app: "Chromium",
    },
    Browser {
        key: "brave",
        label: "Brave",
        data: "BraveSoftware/Brave-Browser",
        bundle_id: "com.brave.Browser",
        app: "Brave Browser",
    },
    Browser {
        key: "edge",
        label: "Microsoft Edge",
        data: "Microsoft Edge",
        bundle_id: "com.microsoft.edgemac",
        app: "Microsoft Edge",
    },
    Browser {
        key: "edge-beta",
        label: "Microsoft Edge Beta",
        data: "Microsoft Edge Beta",
        bundle_id: "com.microsoft.edgemac.Beta",
        app: "Microsoft Edge Beta",
    },
    Browser {
        key: "edge-dev",
        label: "Microsoft Edge Dev",
        data: "Microsoft Edge Dev",
        bundle_id: "com.microsoft.edgemac.Dev",
        app: "Microsoft Edge Dev",
    },
    Browser {
        key: "edge-canary",
        label: "Microsoft Edge Canary",
        data: "Microsoft Edge Canary",
        bundle_id: "com.microsoft.edgemac.Canary",
        app: "Microsoft Edge Canary",
    },
    Browser {
        key: "vivaldi",
        label: "Vivaldi",
        data: "Vivaldi",
        bundle_id: "com.vivaldi.Vivaldi",
        app: "Vivaldi",
    },
    Browser {
        key: "arc",
        label: "Arc",
        data: "Arc/User Data",
        bundle_id: "company.thebrowser.Browser",
        app: "Arc",
    },
    Browser {
        key: "opera",
        label: "Opera",
        data: "com.operasoftware.Opera",
        bundle_id: "com.operasoftware.Opera",
        app: "Opera",
    },
];

/// `BROWSERS.get(key)`.
pub fn browser(key: &str) -> Option<&'static Browser> {
    BROWSERS.iter().find(|b| b.key == key)
}

/// A browser profile (`BrowserProfile`); `email` "" when it isn't signed in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BrowserProfile {
    pub browser: String,
    pub folder: String,
    pub display_name: String,
    pub email: String,
}

impl BrowserProfile {
    pub fn new(browser: &str, folder: &str, display_name: &str, email: &str) -> BrowserProfile {
        BrowserProfile {
            browser: browser.into(),
            folder: folder.into(),
            display_name: display_name.into(),
            email: email.into(),
        }
    }
}

/// `shown` (HAND-424): `Google Chrome “Work” (work@example.com)`; `label` replaces the browser's
/// name ("" leaves it out).
pub fn shown(p: &BrowserProfile, label: Option<&str>) -> String {
    let b = label.map_or_else(
        || browser(&p.browser).map_or_else(|| p.browser.clone(), |b| b.label.to_string()),
        String::from,
    );
    let name = format!(
        "“{}”",
        if p.display_name.is_empty() {
            &p.folder
        } else {
            &p.display_name
        }
    );
    let mut out = if b.is_empty() {
        name
    } else {
        format!("{b} {name}")
    };
    if !p.email.is_empty() {
        out.push_str(&format!(" ({})", p.email));
    }
    out
}

/// `named`: the discovered profile in that folder, else one named by its folder with no account.
pub fn named(found: &[BrowserProfile], browser: &str, folder: &str) -> BrowserProfile {
    found
        .iter()
        .find(|p| p.browser == browser && p.folder == folder)
        .cloned()
        .unwrap_or_else(|| BrowserProfile::new(browser, folder, folder, ""))
}

/// Python's `str.capitalize()`.
pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => {
            let first: String = if f == 'ǅ' || f == 'ǈ' || f == 'ǋ' || f == 'ǲ' {
                f.to_string()
            } else {
                titlecase(f)
            };
            first + &c.as_str().to_lowercase()
        }
    }
}

/// A character's titlecase as Python's `capitalize` writes it (its uppercase for every character
/// FERB's browser keys hold).
fn titlecase(c: char) -> String {
    c.to_uppercase().collect()
}

/// What the Mac tells about browsers (Launch Services, processes): the platform's part. The
/// methods after `has_app` default to "can't be asked" (Python's PyObjC-missing path).
pub trait Platform: Send + Sync {
    /// `_app_installed`: Launch Services knows an app with this bundle id, not in a Trash.
    fn has_app(&self, bundle_id: &str) -> bool;
    /// `_launch_services` (`locate`): the executable of the app Launch Services knows for this
    /// bundle id; `Ok(None)` when it knows none, `Err` when it can't be asked.
    fn locate(&self, _bundle_id: &str) -> std::result::Result<Option<PathBuf>, String> {
        Err("Launch Services isn't available".into())
    }
    /// `_https_handlers` (`handlers`): every app bundle Launch Services lists as opening https
    /// links; `Err` when it can't be asked.
    fn https_handlers(&self) -> std::result::Result<Vec<PathBuf>, String> {
        Err("Launch Services isn't available".into())
    }
    /// `pgrep -f <pattern>` exits 0: a process's full command line matches the extended regex.
    fn pgrep(&self, _pattern: &str) -> bool {
        false
    }
}

/// `_app_installed`'s check of the bundle Launch Services named: a folder, in no Trash.
pub fn app_path_ok(path: &Path) -> bool {
    path.is_dir() && !path.components().any(|c| c.as_os_str() == ".Trash")
}

/// `Browser.binary`'s memory (`_located`): each browser's executable, found by bundle id through
/// Launch Services (HAND-247), remembered while it is there.
#[derive(Default)]
pub struct Binaries {
    located: std::sync::Mutex<std::collections::HashMap<String, PathBuf>>,
}

impl Binaries {
    /// `Browser.binary`: the executable Launch Services knows (remembered while it is a file);
    /// `/Applications/<app>.app/Contents/MacOS/<app>` when it knows none or can't be asked.
    pub fn binary(&self, platform: &dyn Platform, b: &Browser) -> String {
        let mut located = self
            .located
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(known) = located.get(b.bundle_id) {
            if known.is_file() {
                return known.to_string_lossy().into_owned();
            }
        }
        located.remove(b.bundle_id);
        match platform.locate(b.bundle_id) {
            Ok(Some(found)) => {
                let out = found.to_string_lossy().into_owned();
                located.insert(b.bundle_id.to_string(), found);
                out
            }
            _ => format!("/Applications/{0}.app/Contents/MacOS/{0}", b.app),
        }
    }
}

/// Python's `re.escape`: a backslash before each of `()[]{}?*+-|^$\.&~#`, space, tab, CR, LF, VT and FF.
pub fn re_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "()[]{}?*+-|^$\\.&~# \t\n\r\x0b\x0c".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `running`'s pattern: the browser's own binary as the start of a command line (`pgrep -x`
/// would match only a 16-character process name).
pub fn running_pattern(binary: &str) -> String {
    format!("^{}( |$)", re_escape(binary))
}

/// `running`: whether that browser's own binary runs.
pub fn running(platform: &dyn Platform, binaries: &Binaries, key: &str) -> bool {
    browser(key).is_some_and(|b| platform.pgrep(&running_pattern(&binaries.binary(platform, b))))
}

/// `CHROMIUM_EXTENSION_TYPE`: Chromium's own file type for an extension package, which every
/// Chromium-built browser's Info.plist declares (Electron apps don't).
pub const CHROMIUM_EXTENSION_TYPE: &str = "org.chromium.extension";

/// `_info`: an app's Info.plist (XML or binary), `{}` when it can't be read or isn't a dictionary.
pub fn read_info(app: &Path) -> plist::Dictionary {
    match plist::Value::from_file(app.join("Contents").join("Info.plist")) {
        Ok(plist::Value::Dictionary(d)) => d,
        _ => plist::Dictionary::new(),
    }
}

/// A plist scalar as Python holds it (for `str()`, truthiness and `==`); `None` for an array, a
/// dictionary, data or a date.
fn scalar(v: &plist::Value) -> Option<Value> {
    Some(match v {
        plist::Value::String(s) => Value::from(s.as_str()),
        plist::Value::Boolean(b) => Value::from(*b),
        plist::Value::Integer(i) => match i.as_signed() {
            Some(n) => Value::from(n),
            None => Value::from(i.as_unsigned()?),
        },
        plist::Value::Real(f) => Value::from(*f),
        _ => return None,
    })
}

fn plist_truthy(v: &plist::Value) -> bool {
    match v {
        plist::Value::Array(a) => !a.is_empty(),
        plist::Value::Dictionary(d) => !d.is_empty(),
        plist::Value::Data(d) => !d.is_empty(),
        plist::Value::Date(_) => true,
        other => scalar(other).is_some_and(|s| ferb_py::value::truthy(&s)),
    }
}

/// What Python's `for x in (value or [])` walks: an array's items, a dictionary's keys, a string's
/// characters, data's bytes. A truthy number, boolean or date isn't iterable (Python's TypeError,
/// which broke the whole scan): fixed against Python, it walks nothing.
fn iterate(v: Option<&plist::Value>) -> Vec<plist::Value> {
    let Some(v) = v.filter(|v| plist_truthy(v)) else {
        return vec![];
    };
    match v {
        plist::Value::Array(a) => a.clone(),
        plist::Value::Dictionary(d) => d.keys().map(|k| plist::Value::from(k.as_str())).collect(),
        plist::Value::String(s) => s
            .chars()
            .map(|c| plist::Value::from(c.to_string()))
            .collect(),
        plist::Value::Data(d) => d
            .iter()
            .map(|b| plist::Value::from(u64::from(*b)))
            .collect(),
        _ => vec![],
    }
}

/// `chromium_family`: a structural fact from an app's Info.plist: it declares Chromium's extension
/// file type (exported or imported), or opens it.
pub fn chromium_family(info: &plist::Dictionary) -> bool {
    let is_type = |v: &plist::Value| v.as_string() == Some(CHROMIUM_EXTENSION_TYPE);
    for key in ["UTExportedTypeDeclarations", "UTImportedTypeDeclarations"] {
        for t in iterate(info.get(key)) {
            if let plist::Value::Dictionary(t) = t {
                if t.get("UTTypeIdentifier").is_some_and(is_type) {
                    return true;
                }
            }
        }
    }
    for d in iterate(info.get("CFBundleDocumentTypes")) {
        if let plist::Value::Dictionary(d) = d {
            if iterate(d.get("LSItemContentTypes")).iter().any(is_type) {
                return true;
            }
        }
    }
    false
}

/// `str(value or "")` for an Info.plist value; `None` when Python would write the `repr` of an
/// array, dictionary, data or date (such an app is skipped: not reproduced).
fn plist_text(v: Option<&plist::Value>) -> Option<String> {
    match v.filter(|v| plist_truthy(v)) {
        None => Some(String::new()),
        Some(v) => scalar(v).map(|s| ferb_py::value::py_str(&s)),
    }
}

/// `Path.stem`.
fn stem(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => name[..i].to_string(),
        _ => name,
    }
}

/// An installed Chromium-family browser FERB doesn't know (HAND-415).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsupported {
    pub app: String,
    pub bundle_id: String,
}

/// `unsupported` (HAND-415): each installed app macOS lists as opening https links that is a
/// Chromium-family browser [`BROWSERS`] doesn't know, by name. Read only; no list: none.
pub fn unsupported(platform: &dyn Platform) -> Vec<Unsupported> {
    let fold = ferb_sanitize::parity::casefold;
    let known: Vec<String> = BROWSERS.iter().map(|b| fold(b.bundle_id)).collect();
    let Ok(apps) = platform.https_handlers() else {
        return vec![];
    };
    let mut out: Vec<(String, Unsupported)> = vec![];
    for app in apps {
        if app.components().any(|c| c.as_os_str() == ".Trash") {
            continue;
        }
        let info = read_info(&app);
        let Some(ident) = plist_text(info.get("CFBundleIdentifier")) else {
            continue;
        };
        let key = fold(&ident);
        if ident.is_empty()
            || known.contains(&key)
            || out.iter().any(|(k, _)| *k == key)
            || !chromium_family(&info)
        {
            continue;
        }
        let shown = [info.get("CFBundleDisplayName"), info.get("CFBundleName")]
            .into_iter()
            .find(|v| v.is_some_and(plist_truthy))
            .flatten();
        let name = match shown {
            Some(v) => match plist_text(Some(v)) {
                Some(n) => n,
                None => continue,
            },
            None => stem(&app),
        };
        out.push((
            key,
            Unsupported {
                app: name,
                bundle_id: ident,
            },
        ));
    }
    let mut found: Vec<Unsupported> = out.into_iter().map(|(_, u)| u).collect();
    found.sort_by_key(|u| fold(&u.app));
    found
}

/// `app_installed`: the browser's app is installed on this Mac (HAND-414).
pub fn app_installed(platform: &dyn Platform, key: &str) -> bool {
    browser(key).is_some_and(|b| platform.has_app(b.bundle_id))
}

/// `data_dir`.
pub fn data_dir(key: &str, support: &Path) -> Option<PathBuf> {
    browser(key).map(|b| support.join(b.data))
}

/// `installed`: the browsers whose app is installed and that made their user-data folder.
pub fn installed(platform: &dyn Platform, support: &Path) -> Vec<&'static str> {
    BROWSERS
        .iter()
        .filter(|b| {
            support.join(b.data).join("Local State").is_file() && platform.has_app(b.bundle_id)
        })
        .map(|b| b.key)
        .collect()
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read(path).ok()?;
    ferb_py::json::loads(&String::from_utf8_lossy(&text)).ok()
}

/// `discover`: a browser's profiles from its own Local State (`profile.info_cache`), by folder.
pub fn discover(key: &str, data_dir: &Path) -> Vec<BrowserProfile> {
    let Some(state) = read_json(&data_dir.join("Local State")) else {
        return vec![];
    };
    let cache = state
        .get("profile")
        .and_then(|p| p.get("info_cache"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut out: Vec<BrowserProfile> = cache
        .iter()
        .map(|(folder, info)| {
            let s = |k: &str| info.get(k).map(ferb_py::value::py_str).unwrap_or_default();
            BrowserProfile::new(key, folder, &s("name"), &s("user_name").to_lowercase())
        })
        .collect();
    out.sort_by(|a, b| a.folder.cmp(&b.folder));
    out
}

/// `last_used`: the profile folder whose window was active last, as Local State records it.
pub fn last_used(data_dir: &Path) -> Option<String> {
    let state = read_json(&data_dir.join("Local State"))?;
    let profile = state.get("profile")?;
    let folder = match profile.get("last_used") {
        Some(f) => Some(f.clone()),
        None => profile
            .get("info_cache")
            .and_then(Value::as_object)
            .filter(|c| c.contains_key("Default"))
            .map(|_| Value::from("Default")),
    };
    folder.and_then(|f| f.as_str().filter(|s| !s.is_empty()).map(String::from))
}

/// `with_paired` (HAND-407): the discovered profiles, plus each paired one of a browser discovery
/// found none in (named by its folder); a browser whose app isn't installed adds nothing.
pub fn with_paired(
    platform: &dyn Platform,
    discovered: &[BrowserProfile],
    paired: &[(String, String)],
) -> Vec<BrowserProfile> {
    let read: Vec<&str> = discovered.iter().map(|p| p.browser.as_str()).collect();
    let mut out = discovered.to_vec();
    let mut seen: Vec<&(String, String)> = vec![];
    for pair in paired {
        if seen.contains(&pair) {
            continue;
        }
        seen.push(pair);
        let (b, f) = pair;
        if browser(b).is_some() && !read.contains(&b.as_str()) && app_installed(platform, b) {
            out.push(BrowserProfile::new(b, f, f, ""));
        }
    }
    out
}

/// `ferb.act.mac.browsers.clean`: invisible marks removed, outer spaces trimmed, casefolded.
pub fn clean(name: &str) -> String {
    let kept: String = name
        .chars()
        .filter(|c| {
            !matches!(*c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{2069}' | '\u{feff}')
        })
        .collect();
    ferb_sanitize::parity::casefold(ferb_py::value::strip(&kept))
}

/// `_norm`: a name as compared.
fn norm(text: &str) -> String {
    let c = clean(text);
    ferb_py::value::squeeze(c.trim_matches(|ch| matches!(ch, '“' | '”' | '"' | '\'' | ' ')))
}

fn browser_names(key: &str) -> Vec<String> {
    let mut out = vec![norm(key)];
    if let Some(b) = browser(key) {
        for n in [norm(b.label), norm(b.app)] {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out
}

/// `Resolution`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    /// "one", "ambiguous" or "none".
    pub kind: &'static str,
    pub step: String,
    pub candidates: Vec<BrowserProfile>,
}

fn py_repr(s: &str) -> String {
    ferb_identity::py::repr(s)
}

impl Resolution {
    pub fn message(&self, spoken: &str, available: &[BrowserProfile]) -> String {
        match self.kind {
            "one" => {
                let p = &self.candidates[0];
                format!("{}/{}", p.browser, p.folder)
            }
            "ambiguous" => format!(
                "'{spoken}' matches several profiles: {}",
                self.candidates
                    .iter()
                    .map(|p| format!(
                        "{}/{} (name={}, account={})",
                        p.browser,
                        p.folder,
                        py_repr(&p.display_name),
                        if p.email.is_empty() {
                            "unknown"
                        } else {
                            &p.email
                        }
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            _ => {
                let listed: Vec<String> = available
                    .iter()
                    .map(|p| {
                        let mut s = format!("{}/{}", p.browser, p.display_name);
                        if !p.email.is_empty() {
                            s.push_str(&format!(" ({})", p.email));
                        }
                        s
                    })
                    .collect();
                format!(
                    "no profile called '{spoken}'; found: {}",
                    if listed.is_empty() {
                        "none".to_string()
                    } else {
                        listed.join(", ")
                    }
                )
            }
        }
    }
}

/// `resolve`: a profile named by its folder or display name (alone or after its browser's name and
/// one of "/", ":", a space or quotes), its account, a saved alias, or its browser alone; never
/// fuzzily. A display name never hides another profile's account or alias.
pub fn resolve(
    spoken: &str,
    profiles: &[BrowserProfile],
    aliases: &[(String, (String, String))],
    only: Option<&str>,
) -> Resolution {
    let pool: Vec<&BrowserProfile> = profiles
        .iter()
        .filter(|p| only.is_none_or(|b| p.browser == b))
        .collect();
    let s = norm(spoken);
    let sc: Vec<char> = s.chars().collect();
    let mut named = vec![];
    for p in &pool {
        for b in browser_names(&p.browser) {
            let bl = b.chars().count();
            if s.starts_with(&b) && sc.len() > bl && "/: “\"".contains(sc[bl]) {
                let rest = norm(&sc[bl + 1..].iter().collect::<String>());
                if rest == norm(&p.folder) || rest == norm(&p.display_name) {
                    named.push((*p).clone());
                }
            }
        }
    }
    let alias = aliases
        .iter()
        .find(|(k, _)| *k == s)
        .map(|(_, v)| v.clone());
    let steps: Vec<(&str, Vec<BrowserProfile>)> = vec![
        ("profile_id", named),
        (
            "display_name",
            pool.iter()
                .filter(|p| norm(&p.display_name) == s)
                .map(|p| (*p).clone())
                .collect(),
        ),
        (
            "email",
            pool.iter()
                .filter(|p| {
                    !p.email.is_empty()
                        && (p.email == s || p.email.split('@').next() == Some(s.as_str()))
                })
                .map(|p| (*p).clone())
                .collect(),
        ),
        (
            "alias",
            pool.iter()
                .filter(|p| alias.as_ref() == Some(&(p.browser.clone(), p.folder.clone())))
                .map(|p| (*p).clone())
                .collect(),
        ),
        (
            "browser",
            pool.iter()
                .filter(|p| browser_names(&p.browser).contains(&s))
                .map(|p| (*p).clone())
                .collect(),
        ),
    ];
    let mut hits: Vec<BrowserProfile> = vec![];
    for (_, c) in &steps {
        for p in c {
            if !hits.contains(p) {
                hits.push(p.clone());
            }
        }
    }
    let step = steps
        .iter()
        .filter(|(_, c)| !c.is_empty())
        .map(|(s, _)| *s)
        .collect::<Vec<_>>()
        .join(",");
    let kind = match hits.len() {
        1 => "one",
        0 => "none",
        _ => "ambiguous",
    };
    Resolution {
        kind,
        step,
        candidates: hits,
    }
}

/// `open_known_profile`'s command: what is run to load a profile with no window (or its window), or
/// to hand it a URL in the foreground. `Err` as Python's `ValueError`.
#[allow(clippy::too_many_arguments)] // open_known_profile's keywords
pub fn open_command(
    key: &str,
    folder: &str,
    url: Option<&str>,
    known: &[BrowserProfile],
    binary: &str,
    background: bool,
    running: bool,
    window: bool,
) -> Result<Vec<String>, String> {
    if !known.iter().any(|p| p.browser == key && p.folder == folder) {
        return Err(format!(
            "{key}/{folder} isn't a discovered profile; refusing to create one"
        ));
    }
    let b = browser(key).ok_or_else(|| key.to_string())?;
    let dir = format!("--profile-directory={folder}");
    if window {
        return Ok(vec![binary.into(), dir]);
    }
    if background && url.is_some() {
        return Err(
            "needs_foreground: URL launch requires an explicit foreground setup step".into(),
        );
    }
    let last = url.unwrap_or("--no-startup-window").to_string();
    Ok(if background && url.is_none() && running {
        vec![binary.into(), dir, "--no-startup-window".into()]
    } else if background {
        vec![
            "open".into(),
            "-g".into(),
            "-b".into(),
            b.bundle_id.into(),
            "--args".into(),
            dir,
            last,
        ]
    } else {
        vec![binary.into(), dir, last]
    })
}

/// `extension_prefs`: the `extensions` sections of a profile's two settings files, read only.
pub fn extension_prefs(folder: &Path) -> Vec<Map<String, Value>> {
    ["Secure Preferences", "Preferences"]
        .iter()
        .filter_map(|name| read_json(&folder.join(name)))
        .filter_map(|doc| doc.get("extensions").and_then(Value::as_object).cloned())
        .collect()
}

/// `EXTERNAL_EXTENSION_UNINSTALLED`.
pub const EXTERNAL_EXTENSION_UNINSTALLED: i64 = 2;

fn plain_int(v: Option<&Value>) -> Option<i64> {
    v.filter(|v| !v.is_boolean()).and_then(Value::as_i64)
}

/// `installed_entry` (HAND-416).
pub fn installed_entry(entry: &Map<String, Value>) -> bool {
    plain_int(entry.get("location")).is_some()
        && entry
            .get("path")
            .and_then(Value::as_str)
            .is_some_and(|p| !p.is_empty())
        && !entry
            .get("state")
            .is_some_and(|s| ferb_py::value::py_eq(s, &Value::from(EXTERNAL_EXTENSION_UNINSTALLED)))
}

/// `extension_enabled`.
pub fn extension_enabled(entry: &Map<String, Value>) -> bool {
    !entry
        .get("state")
        .is_some_and(|s| ferb_py::value::py_eq(s, &Value::from(0)))
        && !entry
            .get("disable_reasons")
            .is_some_and(ferb_py::value::truthy)
}

/// `disable_reasons` (HAND-412): Chromium's reason codes; an old `state` 0 with none is 1.
pub fn disable_reasons(entry: &Map<String, Value>) -> Vec<i64> {
    let mut codes: Vec<i64> = match entry.get("disable_reasons") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|r| plain_int(Some(r)))
            .filter(|r| *r > 0)
            .collect(),
        Some(v) => match plain_int(Some(v)).filter(|r| *r > 0) {
            Some(raw) => (0..63)
                .filter(|bit| raw >> bit & 1 == 1)
                .map(|bit| 1i64 << bit)
                .collect(),
            None => vec![],
        },
        None => vec![],
    };
    codes.sort_unstable();
    codes.dedup();
    if codes.is_empty()
        && entry
            .get("state")
            .is_some_and(|s| ferb_py::value::py_eq(s, &Value::from(0)))
    {
        return vec![1];
    }
    codes
}

/// `_version_like`.
pub fn version_like(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    !parts.is_empty()
        && parts.len() <= 4
        && parts
            .iter()
            .all(|x| !x.is_empty() && x.chars().all(crate::lax::is_digit) && x.chars().count() <= 9)
}

/// `extension_version` (HAND-412): the copy's version as the profile's own files say it.
pub fn extension_version(entry: &Map<String, Value>, folder: &Path, extension_id: &str) -> String {
    if let Some(v) = entry
        .get("manifest")
        .and_then(Value::as_object)
        .and_then(|m| m.get("version"))
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
    {
        return ferb_py::value::prefix(v, 32).to_string();
    }
    if let Some(path) = entry.get("path").and_then(Value::as_str) {
        if path.starts_with(&format!("{extension_id}/")) {
            let last = path.rsplit('/').next().unwrap_or("");
            let version = last.rsplit_once('_').map_or(last, |(v, _)| v);
            if version_like(version) {
                return version.to_string();
            }
        }
    }
    let Ok(dir) = std::fs::read_dir(folder.join("Extensions").join(extension_id)) else {
        return String::new();
    };
    let found: Vec<String> = dir
        .flatten()
        .filter(|d| d.path().is_dir())
        .map(|d| {
            let n = d.file_name().to_string_lossy().into_owned();
            n.rsplit_once('_').map_or(n.clone(), |(v, _)| v.to_string())
        })
        .filter(|v| version_like(v))
        .collect();
    let key = |v: &String| -> Vec<u128> {
        v.split('.')
            .map(|x| {
                x.chars()
                    .filter_map(crate::lax::decimal_value)
                    .fold(0u128, |a, d| a * 10 + u128::from(d))
            })
            .collect()
    };
    // `max` keeps the first of equal keys
    let mut best: Option<&String> = None;
    for v in &found {
        if best.is_none_or(|b| key(v) > key(b)) {
            best = Some(v);
        }
    }
    best.cloned().unwrap_or_default()
}

/// `extension_entry`: the FERB copy in this profile, its settings from both files merged: the first
/// of `ids` that is on, else the first installed.
pub fn extension_entry(folder: &Path, ids: &[&str]) -> Option<(String, Map<String, Value>)> {
    let sections: Vec<Option<Map<String, Value>>> = extension_prefs(folder)
        .into_iter()
        .map(|s| s.get("settings").and_then(Value::as_object).cloned())
        .collect();
    let mut installed = vec![];
    for i in ids {
        let found: Vec<&Map<String, Value>> = sections
            .iter()
            .flatten()
            .filter_map(|s| s.get(*i).and_then(Value::as_object))
            .collect();
        let mut merged = Map::new();
        for entry in &found {
            for (k, v) in *entry {
                merged.insert(k.clone(), v.clone());
            }
        }
        if !found.is_empty() && installed_entry(&merged) {
            installed.push(((*i).to_string(), merged));
        }
    }
    installed
        .iter()
        .find(|e| extension_enabled(&e.1))
        .or(installed.first())
        .cloned()
}

/// `extension_state`: "on", "off" or "missing".
pub fn extension_state(folder: &Path, ids: &[&str]) -> &'static str {
    match extension_entry(folder, ids) {
        None => "missing",
        Some((_, e)) if extension_enabled(&e) => "on",
        Some(_) => "off",
    }
}

/// The resolver's choice (`Choice`): the profile and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub browser: String,
    pub folder: String,
    pub reason: String,
}

/// A paired profile, as the resolver reads FERB's pairing records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paired {
    pub browser: String,
    pub folder: String,
    pub owner: bool,
}

/// `resolver.Resolver` (HAND-2, HAND-407): an explicit profile selection matched to the paired
/// candidates; ambiguity is the agent's (then the owner's) to settle, never code's (`NeedsOwner`,
/// a refusal).
pub struct Resolver<'a> {
    pub platform: &'a dyn Platform,
    /// Discovered profiles.
    pub profiles: Vec<BrowserProfile>,
    pub bindings: Vec<Paired>,
    /// FERB's own profile, never for an owner task.
    pub identity_profile: Option<(String, String)>,
    pub aliases: Vec<(String, (String, String))>,
    /// The owner's pick for browsing while FERB has no profile of its own (`browsing.owner_profile`).
    pub owner_default: Option<(String, String)>,
    /// Why discovery can't see the owner's profiles now (Full Disk Access off).
    pub unseen: Option<String>,
}

impl Resolver<'_> {
    /// `known`.
    pub fn known(&self) -> Vec<BrowserProfile> {
        let paired: Vec<(String, String)> = self
            .bindings
            .iter()
            .map(|b| (b.browser.clone(), b.folder.clone()))
            .collect();
        with_paired(self.platform, &self.profiles, &paired)
    }

    fn owner_paired(&self) -> Vec<(String, String)> {
        self.bindings
            .iter()
            .filter(|b| {
                b.owner && Some((b.browser.clone(), b.folder.clone())) != self.identity_profile
            })
            .map(|b| (b.browser.clone(), b.folder.clone()))
            .collect()
    }

    fn name(&self, browser: &str, folder: &str) -> String {
        let known = self.known();
        let p = known
            .iter()
            .find(|p| p.browser == browser && p.folder == folder);
        let mut s = format!(
            "{browser}/{}",
            p.map_or(folder, |p| p.display_name.as_str())
        );
        if let Some(p) = p.filter(|p| !p.email.is_empty()) {
            s.push_str(&format!(" ({})", p.email));
        }
        s
    }

    /// `choose`: `Err` is the refusal's words (`NeedsOwner`).
    pub fn choose(&self, named: Option<&str>) -> Result<Choice, String> {
        let paired = self.owner_paired();
        let ferb = self.identity_profile.clone();
        if let Some(named) = named.filter(|n| !n.is_empty()) {
            let pool: Vec<BrowserProfile> = self
                .known()
                .into_iter()
                .filter(|p| Some((p.browser.clone(), p.folder.clone())) != ferb)
                .collect();
            let r = resolve(named, &pool, &self.aliases, None);
            if r.kind != "one" {
                let why = (r.kind == "none" && self.profiles.is_empty())
                    .then(|| self.unseen.clone())
                    .flatten();
                return Err(
                    r.message(named, &pool) + &why.map(|w| format!(" ({w})")).unwrap_or_default()
                );
            }
            let p = &r.candidates[0];
            if !paired.contains(&(p.browser.clone(), p.folder.clone())) {
                let account = if p.email.is_empty() {
                    String::new()
                } else {
                    format!(", signed in as {}", p.email)
                };
                return Err(format!(
                    "{} profile '{}' ({}/{}{account}) isn't paired with FERB yet: pair it with browser_pair profile={}/{} \
                     (FERB asks the owner once)",
                    p.browser, p.display_name, p.browser, p.folder, p.browser, p.folder
                ));
            }
            return Ok(Choice {
                browser: p.browser.clone(),
                folder: p.folder.clone(),
                reason: format!("named: {named}"),
            });
        }
        let chosen = if ferb.is_none() {
            self.owner_default.clone()
        } else {
            None
        };
        if let Some(c) = chosen.as_ref().filter(|c| paired.contains(c)) {
            return Ok(Choice {
                browser: c.0.clone(),
                folder: c.1.clone(),
                reason: "the owner's chosen profile for browsing (FERB has no profile of its own)"
                    .into(),
            });
        }
        if let Some(c) = &chosen {
            if let Some(p) = self
                .known()
                .into_iter()
                .find(|p| p.browser == c.0 && p.folder == c.1)
            {
                let account = if p.email.is_empty() {
                    String::new()
                } else {
                    format!(", signed in as {}", p.email)
                };
                return Err(format!(
                    "the owner picked {} profile '{}' ({}/{}{account}) for FERB's web work and it isn't paired with FERB \
                     yet: pair it with browser_pair profile={}/{} (FERB asks the owner once)",
                    p.browser, p.display_name, p.browser, p.folder, p.browser, p.folder
                ));
            }
        }
        if paired.is_empty() {
            return Err("no browser profile is paired with FERB yet: pick the owner's profile from browser_profiles and \
                        pair it with browser_pair (FERB asks the owner once)"
                .into());
        }
        let mut sorted = paired.clone();
        sorted.sort();
        Err(format!(
            "select a profile from the task context; available: {}. Use ferb_browser for ordinary browsing in FERB's own \
             profile.",
            sorted.iter().map(|(b, f)| self.name(b, f)).collect::<Vec<_>>().join(", ")
        ))
    }
}
