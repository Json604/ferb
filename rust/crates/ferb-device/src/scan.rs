//! The device scan (ferb/device/scan.py): installed apps from their Info.plist only, whether each declares
//! scripting, and the browsers among them. App bundles only, never app data folders.
//!
//! Which bundle ids are browsers, and the name FERB's browser connection knows each by, are the browser part's
//! tables (ferb/act/mac/browsers.py, ferb/act/browser/profiles.py): they come in through [`BrowserCatalog`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::plistlib;
use crate::profile::{AppInfo, BrowserInfo, DeviceProfile};

/// scan.py APP_ROOTS: /Applications, its Utilities, the system's, and the account's own.
pub fn app_roots(user_home: &Path) -> Vec<PathBuf> {
    vec![
        "/Applications".into(),
        "/Applications/Utilities".into(),
        "/System/Applications".into(),
        "/System/Applications/Utilities".into(),
        user_home.join("Applications"),
    ]
}

/// The browser part's facts about a bundle id.
pub trait BrowserCatalog {
    /// browsers.known: a browser FERB knows (or one of its channels, a web app's shim, a browser maker's app).
    fn known(&self, bundle_id: &str) -> bool;
    /// browsers.pair_key: the name FERB's browser connection knows it by, else the bundle id itself.
    fn pair_key(&self, bundle_id: &str) -> String;
    /// A Chromium-family browser FERB pairs with (profiles.BROWSERS).
    fn paired(&self, bundle_id: &str) -> bool;
}

/// scan.py:377: one app from its Info.plist; None without a bundle id. An Info.plist that isn't a dictionary is
/// skipped (Python raised AttributeError there and the whole scan failed; ENG-6).
pub fn read_app(bundle: &Path) -> Option<AppInfo> {
    let info = plistlib::load(&bundle.join("Contents/Info.plist"))?;
    let info = info.as_dictionary()?;
    let bundle_id = info.get("CFBundleIdentifier")?.as_string()?.to_string();
    if bundle_id.is_empty() {
        return None;
    }
    let has_sdef = std::fs::read_dir(bundle.join("Contents/Resources"))
        .map(|rd| {
            rd.flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".sdef"))
        })
        .unwrap_or(false);
    let scriptable = info
        .get("NSAppleScriptEnabled")
        .is_some_and(plistlib::truthy)
        || info.contains_key("OSAScriptingDefinition")
        || has_sdef;
    let name = ["CFBundleDisplayName", "CFBundleName"]
        .iter()
        .filter_map(|k| info.get(k))
        .find(|v| plistlib::truthy(v))
        .map(plistlib::py_str)
        .unwrap_or_else(|| {
            bundle
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
    let version = info
        .get("CFBundleShortVersionString")
        .map(plistlib::py_str)
        .unwrap_or_default();
    Some(AppInfo::new(
        &bundle_id,
        &name,
        &version,
        &bundle.to_string_lossy(),
        scriptable,
    ))
}

/// scan.py:397: every app under `roots`, the first root's copy winning, sorted by bundle id.
pub fn scan_apps(roots: &[PathBuf]) -> Vec<AppInfo> {
    let mut seen: BTreeMap<String, AppInfo> = BTreeMap::new();
    for root in roots {
        let Ok(rd) = std::fs::read_dir(root) else {
            continue;
        };
        let mut bundles: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".app"))
            .map(|e| e.path())
            .collect();
        bundles.sort();
        for bundle in bundles {
            if let Some(app) = read_app(&bundle) {
                seen.entry(app.bundle_id.clone()).or_insert(app);
            }
        }
    }
    seen.into_values().collect()
}

/// scan.py:411: the app's Info.plist registers it for http or https links.
pub fn registers_web(bundle: &Path) -> bool {
    let Some(info) = plistlib::load(&bundle.join("Contents/Info.plist")) else {
        return false;
    };
    let Some(types) = info
        .as_dictionary()
        .and_then(|d| d.get("CFBundleURLTypes"))
        .and_then(|t| t.as_array())
    else {
        return false;
    };
    types.iter().any(|t| {
        t.as_dictionary()
            .and_then(|d| d.get("CFBundleURLSchemes"))
            .and_then(|s| s.as_array())
            .is_some_and(|s| {
                s.iter().any(|x| {
                    matches!(
                        plistlib::py_str(x).to_lowercase().as_str(),
                        "http" | "https"
                    )
                })
            })
    })
}

/// scan.py:424.
pub fn browsers_from(apps: &[AppInfo], catalog: &dyn BrowserCatalog) -> Vec<BrowserInfo> {
    let mut out = Vec::new();
    for app in apps {
        if !(catalog.known(&app.bundle_id) || registers_web(Path::new(&app.path))) {
            continue;
        }
        let key = catalog.pair_key(&app.bundle_id);
        let name = if key != app.bundle_id {
            key
        } else {
            app.name.to_lowercase()
        };
        let mut drivers = BTreeMap::new();
        drivers.insert(
            format!("ax:{name}"),
            "Accessibility read fallback".to_string(),
        );
        if catalog.paired(&app.bundle_id) {
            drivers.insert(
                format!("ext:{name}"),
                "FERB extension, per paired profile (`ferb browsers`)".to_string(),
            );
        }
        if name == "safari" {
            drivers = BTreeMap::new();
            drivers.insert(
                "applescript_read".into(),
                "watch-only (Block 1), when Automation is granted".into(),
            );
        }
        out.push(BrowserInfo {
            name,
            bundle_id: app.bundle_id.clone(),
            path: app.path.clone(),
            drivers,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// scan.py:443. `os_version` is `platform.mac_ver()[0]` (or "unknown") and `scanned_at` the local time in seconds,
/// both from the caller.
pub fn scan(
    device_id: &str,
    roots: &[PathBuf],
    catalog: &dyn BrowserCatalog,
    os_version: &str,
    scanned_at: &str,
) -> DeviceProfile {
    let apps = scan_apps(roots);
    let browsers = browsers_from(&apps, catalog);
    DeviceProfile {
        device_id: device_id.into(),
        os: "macos".into(),
        os_version: if os_version.is_empty() {
            "unknown".into()
        } else {
            os_version.into()
        },
        scanned_at: scanned_at.into(),
        apps,
        browsers,
    }
}

/// `datetime.now().astimezone().isoformat(timespec="seconds")`.
pub fn now_local() -> String {
    chrono::Local::now()
        .format("%Y-%m-%dT%H:%M:%S%:z")
        .to_string()
}
