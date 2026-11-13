//! How FERB's extension reaches each browser (decision: HAND-247): a port of
//! `ferb/act/browser/autoinstall.py`: the Chrome Web Store install requests FERB writes (and
//! withdraws), each profile's FERB copy as its own files say (HAND-412, HAND-416), what setup tells
//! the owner, and the Web Store upload's package.
//!
//! `package`'s zip is not byte-for-byte Python's: Python stamps `manifest.json`'s entry with the
//! time it runs and copies each other file's own time and mode, so its bytes change from run to run;
//! the same names, order and contents are what is checked.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::profiles::{self, BrowserProfile, Platform};

pub const WEB_STORE_UPDATE_URL: &str = "https://clients2.google.com/service/update2/crx";
/// FERB on the Chrome Web Store; `[browser] store_extension_id` overrides.
pub const STORE_EXTENSION_ID: &str = "plbfkifcmgkgnlfgikdeaaglgncdhkma";
/// In FERB's home: the request files FERB wrote.
pub const REQUEST_RECORD: &str = "extension-requests.json";

const EDGE: &str = "Microsoft documents this folder with its own store's address; the Chrome Web Store's only for \
Windows. Edge adds FERB from the Chrome Web Store page once you allow extensions from other stores";

/// `INSTALL`: (browser, way, note): "request", "unconfirmed" or "store".
pub const INSTALL: [(&str, &str, &str); 13] = [
    ("chrome", "request", ""),
    ("chrome-beta", "request", ""),
    ("chrome-dev", "request", ""),
    ("chrome-canary", "request", ""),
    ("chromium", "request", ""),
    (
        "brave",
        "unconfirmed",
        "Brave doesn't document this folder; it loads extensions as Chromium does",
    ),
    ("edge", "unconfirmed", EDGE),
    ("edge-beta", "unconfirmed", EDGE),
    ("edge-dev", "unconfirmed", EDGE),
    ("edge-canary", "unconfirmed", EDGE),
    (
        "vivaldi",
        "store",
        "Vivaldi adds extensions straight from the Chrome Web Store",
    ),
    (
        "arc",
        "store",
        "Arc adds extensions straight from the Chrome Web Store",
    ),
    (
        "opera",
        "store",
        "Opera adds Chrome Web Store extensions once its “Install Chrome Extensions” add-on is on",
    ),
];

fn install(browser: &str) -> Option<(&'static str, &'static str)> {
    INSTALL
        .iter()
        .find(|(b, ..)| *b == browser)
        .map(|(_, w, n)| (*w, *n))
}

fn documented(browser: &str) -> bool {
    install(browser).is_some_and(|(w, _)| w == "request")
}

fn unconfirmed(browser: &str) -> Option<&'static str> {
    install(browser)
        .filter(|(w, _)| *w == "unconfirmed")
        .map(|(_, n)| n)
}

/// `ALSO_READS`: browsers that look for native hosts in Google Chrome's folder.
pub const ALSO_READS: [(&str, &str); 2] = [("brave", "chrome"), ("opera", "chrome")];

/// `valid_id`: 32 letters a-p.
pub fn valid_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

/// `accepted_extension_ids`: the owner's configured ids that are extension ids, the pinned unpacked
/// id, then the Web Store id when valid.
pub fn accepted_extension_ids(configured: Option<&Value>, store_id: &str) -> Vec<String> {
    let mut ids: Vec<String> = configured
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(ferb_py::value::py_str)
        .filter(|i| valid_id(i))
        .collect();
    ids.push(crate::protocol::FERB_EXTENSION_ID.into());
    if valid_id(store_id) {
        ids.push(store_id.into());
    }
    let mut out: Vec<String> = vec![];
    for i in ids {
        if !out.contains(&i) {
            out.push(i);
        }
    }
    out
}

/// `store_extension_id`: the configured Web Store id (`None`: not configured), stripped.
pub fn store_extension_id(configured: Option<&Value>) -> String {
    match configured {
        None => STORE_EXTENSION_ID.into(),
        Some(v) if !ferb_py::value::truthy(v) => String::new(),
        Some(v) => ferb_py::value::strip(&ferb_py::value::py_str(v)).to_string(),
    }
}

/// `native_host_dirs`.
pub fn native_host_dirs(support: &Path, browsers: &[&str]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = vec![];
    for browser in browsers {
        let also = ALSO_READS
            .iter()
            .find(|(b, _)| b == browser)
            .map(|(_, a)| *a);
        for b in std::iter::once(*browser).chain(also) {
            if let Some(d) = profiles::data_dir(b, support) {
                let d = d.join("NativeMessagingHosts");
                if !out.contains(&d) {
                    out.push(d);
                }
            }
        }
    }
    out
}

/// `request_file`; `Err` as Python's `ValueError`.
pub fn request_file(browser: &str, support: &Path, store_id: &str) -> Result<PathBuf, String> {
    if !valid_id(store_id) {
        return Err(format!(
            "{} isn't a Chrome extension id",
            ferb_identity::py::repr(store_id)
        ));
    }
    let data = profiles::data_dir(browser, support).ok_or_else(|| browser.to_string())?;
    Ok(data
        .join("External Extensions")
        .join(format!("{store_id}.json")))
}

fn atomic_write(
    dir: &Path,
    prefix: &str,
    name: &Path,
    body: &str,
    mode: Option<u32>,
) -> std::io::Result<()> {
    let tmp = dir.join(format!("{prefix}{}.json", std::process::id()));
    let r = (|| {
        std::fs::write(&tmp, body)?;
        #[cfg(unix)]
        if let Some(m) = mode {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(m))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        std::fs::rename(&tmp, name)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

/// `json.dumps(v, indent=2)`.
fn indented(v: &Value) -> String {
    ferb_py::json::dumps_with(
        v,
        ferb_py::json::Style {
            ascii: true,
            sort_keys: false,
            indent: Some(2),
        },
    )
}

/// `write_request`: the install request, inside an existing user-data folder (never one made).
pub fn write_request(browser: &str, support: &Path, store_id: &str) -> std::io::Result<PathBuf> {
    let path = request_file(browser, support, store_id).map_err(std::io::Error::other)?;
    let body = indented(&json!({"external_update_url": WEB_STORE_UPDATE_URL})) + "\n";
    let parent = path.parent().expect("a folder");
    match std::fs::create_dir(parent) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
        _ => {}
    }
    if !path.is_file() || std::fs::read_to_string(&path).ok().as_deref() != Some(body.as_str()) {
        atomic_write(parent, ".ferb-", &path, &body, Some(0o644))?;
    }
    Ok(path)
}

fn recorded(home: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(home.join(REQUEST_RECORD)) else {
        return vec![];
    };
    match ferb_py::json::loads(&text) {
        Ok(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(PathBuf::from)
            .collect(),
        _ => vec![],
    }
}

fn save_record(home: &Path, paths: &[PathBuf]) -> std::io::Result<()> {
    let record = home.join(REQUEST_RECORD);
    let mut unique: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    unique.sort();
    unique.dedup();
    if unique.is_empty() {
        return match std::fs::remove_file(&record) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    atomic_write(
        home,
        ".extension-requests.",
        &record,
        &(indented(&json!(unique)) + "\n"),
        Some(0o600),
    )
}

/// `_is_request`: only `.../External Extensions/<extension id>.json` is ever removed.
fn is_request(path: &Path) -> bool {
    path.is_absolute()
        && path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|n| n == "External Extensions")
        && path.extension().is_some_and(|e| e == "json")
        && path
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(valid_id)
}

/// `withdraw`: the recorded request files go, except `keep`'s; one that can't stays recorded.
pub fn withdraw(home: &Path, keep: &str) -> std::io::Result<Vec<PathBuf>> {
    let (mut removed, mut kept) = (vec![], vec![]);
    for path in recorded(home) {
        if !is_request(&path) {
            continue;
        }
        if path.file_stem().and_then(|s| s.to_str()) == Some(keep) {
            kept.push(path);
            continue;
        }
        if path.is_file() {
            match std::fs::remove_file(&path) {
                Ok(()) => removed.push(path),
                Err(_) => kept.push(path),
            }
        }
    }
    if home.join(REQUEST_RECORD).exists() {
        save_record(home, &kept)?;
    }
    Ok(removed)
}

/// `remove_requests` (uninstall): every recorded request, plus the current id's in every browser.
pub fn remove_requests(
    home: &Path,
    support: &Path,
    store_id: &str,
) -> std::io::Result<Vec<PathBuf>> {
    let mut removed = withdraw(home, "")?;
    if valid_id(store_id) {
        for b in profiles::BROWSERS {
            let path = request_file(b.key, support, store_id).map_err(std::io::Error::other)?;
            if path.is_file() {
                std::fs::remove_file(&path)?;
                removed.push(path);
            }
        }
    }
    Ok(removed)
}

/// `ProfileStatus`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileStatus {
    pub browser: String,
    pub folder: String,
    pub name: String,
    pub extension_id: String,
    pub enabled: bool,
    pub removed: bool,
    pub paired: Option<bool>,
    pub version: String,
    pub disabled_for: Vec<i64>,
    pub email: String,
}

/// `BrowserState`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserState {
    pub browser: String,
    pub request: Option<PathBuf>,
    pub profiles: Vec<ProfileStatus>,
    pub error: String,
}

/// `_removed` (HAND-416).
fn removed(sections: &[Map<String, Value>], store_id: &str) -> bool {
    sections.iter().any(|section| {
        section
            .get("external_uninstalls")
            .and_then(Value::as_array)
            .is_some_and(|l| l.iter().any(|x| x.as_str() == Some(store_id)))
            || section
                .get("settings")
                .and_then(Value::as_object)
                .and_then(|s| s.get(store_id))
                .and_then(Value::as_object)
                .is_some_and(|e| {
                    e.get("state").is_some_and(|s| {
                        ferb_py::value::py_eq(s, &json!(profiles::EXTERNAL_EXTENSION_UNINSTALLED))
                    })
                })
    })
}

/// `owner_removed`.
pub fn owner_removed(p: &BrowserProfile, data_dir: &Path, store_id: &str) -> bool {
    valid_id(store_id)
        && removed(
            &profiles::extension_prefs(&data_dir.join(&p.folder)),
            store_id,
        )
}

/// `profile_status`.
pub fn profile_status(
    p: &BrowserProfile,
    ids: &[&str],
    data_dir: &Path,
    paired: Option<&[(String, String)]>,
    store_id: &str,
) -> ProfileStatus {
    let folder = data_dir.join(&p.folder);
    let found = profiles::extension_entry(&folder, ids);
    ProfileStatus {
        browser: p.browser.clone(),
        folder: p.folder.clone(),
        name: p.display_name.clone(),
        extension_id: found.as_ref().map(|f| f.0.clone()).unwrap_or_default(),
        enabled: found
            .as_ref()
            .is_some_and(|f| profiles::extension_enabled(&f.1)),
        removed: owner_removed(p, data_dir, store_id),
        paired: paired.map(|ps| ps.iter().any(|(b, f)| *b == p.browser && *f == p.folder)),
        version: found
            .as_ref()
            .map(|f| profiles::extension_version(&f.1, &folder, &f.0))
            .unwrap_or_default(),
        disabled_for: found
            .as_ref()
            .map(|f| profiles::disable_reasons(&f.1))
            .unwrap_or_default(),
        email: p.email.clone(),
    }
}

/// `prepare`: for every installed browser, the Web Store request where a source supports it, and
/// each profile's state; with `home`, each request written is recorded there.
pub fn prepare(
    platform: &dyn Platform,
    support: &Path,
    store_id: &str,
    ids: &[&str],
    paired: Option<&[(String, String)]>,
    home: Option<&Path>,
) -> std::io::Result<Vec<BrowserState>> {
    let mut out = vec![];
    for browser in profiles::installed(platform, support) {
        let (mut request, mut error) = (None, String::new());
        if valid_id(store_id) && (documented(browser) || unconfirmed(browser).is_some()) {
            match write_request(browser, support, store_id) {
                Ok(p) => request = Some(p),
                Err(e) => error = crate::upload::unix::strerror(&e),
            }
        }
        let data = profiles::data_dir(browser, support).expect("a known browser");
        out.push(BrowserState {
            browser: browser.into(),
            request,
            profiles: profiles::discover(browser, &data)
                .iter()
                .map(|p| profile_status(p, ids, &data, paired, store_id))
                .collect(),
            error,
        });
    }
    let written: Vec<PathBuf> = out.iter().filter_map(|s| s.request.clone()).collect();
    if let Some(home) = home.filter(|_| !written.is_empty()) {
        let mut all = recorded(home);
        all.extend(written);
        save_record(home, &all)?;
    }
    Ok(out)
}

/// `read_states`: each installed browser's state, read only.
pub fn read_states(
    platform: &dyn Platform,
    support: &Path,
    store_id: &str,
    ids: &[&str],
    paired: Option<&[(String, String)]>,
    data_dirs: Option<&[(String, PathBuf)]>,
) -> Vec<BrowserState> {
    let dirs: Vec<(String, PathBuf)> = match data_dirs {
        Some(d) => d
            .iter()
            .filter(|(b, _)| profiles::app_installed(platform, b))
            .cloned()
            .collect(),
        None => profiles::installed(platform, support)
            .into_iter()
            .map(|b| {
                (
                    b.to_string(),
                    profiles::data_dir(b, support).expect("known"),
                )
            })
            .collect(),
    };
    dirs.iter()
        .map(|(browser, data)| {
            let request = valid_id(store_id)
                .then(|| {
                    data.join("External Extensions")
                        .join(format!("{store_id}.json"))
                })
                .filter(|r| r.is_file());
            BrowserState {
                browser: browser.clone(),
                request,
                profiles: profiles::discover(browser, data)
                    .iter()
                    .map(|p| profile_status(p, ids, data, paired, store_id))
                    .collect(),
                error: String::new(),
            }
        })
        .collect()
}

/// `unsupported_line` (HAND-415).
pub fn unsupported_line(app: &str) -> String {
    format!("{app} isn't supported by FERB yet")
}

fn profile_line(p: &ProfileStatus, store_id: &str) -> String {
    let state = if p.extension_id.is_empty() {
        if p.removed {
            format!(
                "removed earlier, so the browser won't add it again: add it from https://chromewebstore.google.com/detail/{store_id}"
            )
        } else {
            "not installed".into()
        }
    } else {
        let mut s = format!(
            "installed {}",
            if p.extension_id == store_id {
                "from the Web Store"
            } else {
                "unpacked"
            }
        );
        s.push_str(if p.enabled {
            ", enabled"
        } else {
            ", waiting for you to turn it on"
        });
        if p.enabled {
            s.push_str(match p.paired {
                None => ", pairing not checked (FERB isn't running)",
                Some(true) => ", paired",
                Some(false) => ", not paired",
            });
        }
        s
    };
    let place = if p.email.is_empty() {
        p.folder.clone()
    } else {
        format!("{}, {}", p.email, p.folder)
    };
    format!("    “{}” ({place}): {state}", p.name)
}

/// `describe`: what setup tells the owner.
pub fn describe(
    states: &[BrowserState],
    store_id: &str,
    extension_dir: &Path,
    withdrawn: &[PathBuf],
    unsupported: &[String],
) -> Vec<String> {
    let valid = valid_id(store_id);
    let removed: Vec<String> = withdrawn
        .iter()
        .map(|p| format!("  Removed FERB's earlier install request {}", p.display()))
        .collect();
    let others: Vec<String> = unsupported
        .iter()
        .map(|a| format!("  {}.", unsupported_line(a)))
        .collect();
    let label =
        |b: &str| profiles::browser(b).map_or_else(|| b.to_string(), |x| x.label.to_string());
    if states.is_empty() {
        let mut out = vec!["Browser extension: no Chromium-family browser has been started on this Mac yet; FERB sets it up \
                            the next time it updates after you start one."
            .to_string()];
        out.extend(others);
        out.extend(removed);
        return out;
    }
    let asked: Vec<String> = states
        .iter()
        .filter(|s| s.request.is_some())
        .map(|s| label(&s.browser))
        .collect();
    let mut lines = vec![if valid && !asked.is_empty() {
        format!(
            "Browser extension: FERB asked {} to install it from the Chrome Web Store. Each installs it at its next \
             start, for all its profiles, and asks you to turn it on in each profile. FERB never restarts a browser or \
             answers its prompts.",
            asked.join(", ")
        )
    } else if valid {
        "Browser extension: no browser here took FERB's Chrome Web Store install request; add FERB from its Chrome Web \
         Store page in each (below)."
            .to_string()
    } else if !store_id.is_empty() {
        format!(
            "Browser extension: [browser] store_extension_id {} in config.toml isn't a Chrome extension id (32 letters \
             a-p), so no browser was asked to install FERB.",
            ferb_identity::py::repr(store_id)
        )
    } else {
        "Browser extension: the Chrome Web Store listing is pending, so browsers can't be asked to install FERB yet."
            .to_string()
    }];
    let page = format!("https://chromewebstore.google.com/detail/{store_id}");
    let mut reopen = vec![];
    for s in states {
        let l = label(&s.browser);
        let note = if !valid {
            String::new()
        } else if !s.error.is_empty() {
            format!(
                ": couldn't write the install request ({}); add FERB from {page}",
                s.error
            )
        } else if s.request.is_none() {
            let how = install(&s.browser).map_or("", |(_, n)| n);
            format!(
                ": add FERB from {page}{}",
                if how.is_empty() {
                    String::new()
                } else {
                    format!(" ({how})")
                }
            )
        } else if let Some(n) = unconfirmed(&s.browser) {
            format!(": install requested (unconfirmed: {n}; if FERB isn't offered after you reopen it, add it from {page})")
        } else {
            ": install requested".into()
        };
        lines.push(format!("  {l}{note}"));
        if s.profiles.is_empty() {
            lines.push("    no profiles yet".into());
        } else {
            lines.extend(s.profiles.iter().map(|p| profile_line(p, store_id)));
        }
        if s.request.is_some()
            && s.profiles
                .iter()
                .any(|p| p.extension_id.is_empty() && !p.removed)
        {
            reopen.push(l);
        }
    }
    if !reopen.is_empty() {
        lines.push(format!(
            "  Reopen {} to pick up the extension and FERB's native host.",
            reopen.join(", ")
        ));
    }
    if !valid {
        lines.push(format!(
            "  Load unpacked, once per profile: open the browser's extensions page, turn on Developer mode, click “Load \
             unpacked” and choose {}",
            extension_dir.display()
        ));
    }
    if states
        .iter()
        .any(|s| s.profiles.iter().any(|p| p.paired != Some(true)))
    {
        lines.push(
            "  To pair a profile with FERB turned on: Pair all or Pair in FERB's Settings → Browsers, or `ferb browsers \
             pair --browser <browser> \"<profile>\"`."
                .into(),
        );
    }
    lines.extend(others);
    lines.extend(removed);
    lines
}

/// `package`: the Web Store upload: every bundle file with manifest.json at the zip's root, minus
/// the development `key` and Markdown; the zip and what the listing still needs. `Err` as Python's
/// `ValueError` (or the I/O error).
pub fn package(source: &Path, out_dir: &Path) -> Result<(PathBuf, Vec<String>), String> {
    let text = std::fs::read_to_string(source.join("manifest.json")).map_err(|e| e.to_string())?;
    let mut manifest = match ferb_py::json::loads(&text) {
        Ok(Value::Object(m)) => m,
        _ => return Err("manifest.json isn't a JSON object".into()),
    };
    let missing: Vec<&str> = ["name", "version", "description"]
        .into_iter()
        .filter(|k| !manifest.get(*k).is_some_and(ferb_py::value::truthy))
        .collect();
    if !missing.is_empty() {
        return Err(format!("manifest.json lacks {}", missing.join(", ")));
    }
    manifest.shift_remove("key");
    let notes = if manifest.get("icons").is_some_and(ferb_py::value::truthy) {
        vec![]
    } else {
        vec!["manifest.json has no icons; the Chrome Web Store asks for them (at least a 128x128 PNG)".to_string()]
    };
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let version = ferb_py::value::py_str(&manifest["version"]);
    let target = out_dir.join(format!("ferb-extension-{version}.zip"));
    let mut files = vec![];
    walk(source, source, &mut files).map_err(|e| e.to_string())?;
    files.sort_by(|a, b| a.split('/').cmp(b.split('/')));
    let tmp = out_dir.join(format!(".ferb-extension-{}.zip", std::process::id()));
    let r = (|| -> Result<(), String> {
        let f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut z = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for name in &files {
            z.start_file(name.as_str(), opts)
                .map_err(|e| e.to_string())?;
            let body = if name == "manifest.json" {
                (indented(&Value::Object(manifest.clone())) + "\n").into_bytes()
            } else {
                std::fs::read(source.join(name)).map_err(|e| e.to_string())?
            };
            std::io::Write::write_all(&mut z, &body).map_err(|e| e.to_string())?;
        }
        z.finish().map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &target).map_err(|e| e.to_string())
    })();
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok((target, notes))
}

/// Every regular file under `dir` (no link, no `.md`, nothing under a dot-named part), as posix
/// paths relative to `root` (`sorted` on the Paths: by parts, as Python compares them).
fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            walk(root, &path, out)?;
        } else if kind.is_file() && path.extension().is_none_or(|e| e != "md") {
            out.push(
                path.strip_prefix(root)
                    .expect("under root")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    Ok(())
}
