//! profiles.py's Mac half as logic against rust/fixtures/browser/profiles_mac.json (Python run with
//! Launch Services and pgrep replaced by recorders): Browser.binary's memory, the pgrep pattern,
//! the unsupported-browser scan over invented app bundles (plistlib's XML and binary plists), and
//! `_app_installed`'s path check. The real Launch Services calls are the ignored, read-only test at
//! the end.

mod common;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ferb_browser::profiles::{self, Binaries, Platform, Unsupported};
use serde_json::{json, Value};

use common::{b64d, fixture};

#[derive(Default)]
struct Fake {
    handlers: Option<Vec<PathBuf>>,
    answers: Mutex<HashMap<String, VecDeque<Option<String>>>>,
    calls: Mutex<Vec<String>>,
    pgrep: Mutex<Vec<String>>,
}

impl Platform for Fake {
    fn has_app(&self, _: &str) -> bool {
        true
    }
    fn locate(&self, bundle_id: &str) -> Result<Option<PathBuf>, String> {
        self.calls.lock().unwrap().push(bundle_id.into());
        let a = self
            .answers
            .lock()
            .unwrap()
            .get_mut(bundle_id)
            .and_then(VecDeque::pop_front);
        match a {
            Some(Some(s)) if s == "raise" => Err("no PyObjC".into()),
            Some(Some(s)) => Ok(Some(PathBuf::from(s))),
            _ => Ok(None),
        }
    }
    fn https_handlers(&self) -> Result<Vec<PathBuf>, String> {
        self.handlers
            .clone()
            .ok_or_else(|| "Launch Services unavailable".into())
    }
    fn pgrep(&self, pattern: &str) -> bool {
        self.pgrep.lock().unwrap().push(pattern.into());
        pattern.contains("Brave") || pattern.contains("Canary")
    }
}

fn shown(found: &[Unsupported]) -> Value {
    json!(found
        .iter()
        .map(|u| json!({"app": u.app, "bundle_id": u.bundle_id}))
        .collect::<Vec<_>>())
}

fn at(root: &Path, s: &str) -> String {
    s.replace("$ROOT", &root.to_string_lossy())
}

// decisions: HAND-247, HAND-414, HAND-415
#[test]
fn the_mac_half_of_profiles_matches_python() {
    let f = fixture("profiles_mac.json");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let apps = ["Applications/Comet.app", "Applications/NoPlist.app"];
    for (rel, data) in f["apps"].as_object().unwrap() {
        let p = root.join(rel).join("Contents");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("Info.plist"), b64d(data.as_str().unwrap())).unwrap();
    }
    for rel in apps {
        std::fs::create_dir_all(root.join(rel).join("Contents")).unwrap();
    }
    let mut compared = 0;

    // the scan
    for run in f["unsupported"].as_array().unwrap() {
        let name = run["name"].as_str().unwrap();
        let fake = Fake {
            handlers: run["handlers"]
                .as_array()
                .map(|a| a.iter().map(|r| root.join(r.as_str().unwrap())).collect()),
            ..Fake::default()
        };
        let got = shown(&profiles::unsupported(&fake));
        if name.starts_with("type_error") {
            // Fixed against Python: a truthy number or boolean where Info.plist should hold a list raised
            // TypeError out of the whole scan; Rust reads it as no entries, so every other browser is still named.
            assert_eq!(run["out"]["error"], "TypeError", "{name}");
            let mut want = f["unsupported"][0]["out"]["ok"].as_array().unwrap().clone();
            if name == "type_error" {
                want.push(json!({"app": "Weird", "bundle_id": "com.example.weird"}));
                want.sort_by_key(|u| u["app"].as_str().unwrap().to_lowercase());
            }
            assert_eq!(got, json!(want), "{name}");
        } else {
            assert_eq!(got, run["out"]["ok"], "{name}");
        }
        compared += 1;
    }
    for (rel, want) in f["family"].as_object().unwrap() {
        let got = profiles::chromium_family(&profiles::read_info(&root.join(rel)));
        if want.get("error").is_some() {
            // the fix above: Weird declares the type in CFBundleDocumentTypes; Weird2's only list is `true`
            assert_eq!(got, rel.ends_with("Weird.app"), "{rel}");
        } else {
            assert_eq!(json!(got), *want, "{rel}");
        }
        compared += 1;
    }

    // Browser.binary's memory
    std::fs::create_dir_all(root.join("bin/edge-dir")).unwrap();
    let exe = root.join("bin/Google Chrome");
    std::fs::write(&exe, "#!/bin/sh\n").unwrap();
    let fake = Fake::default();
    for (bid, list) in f["binary"]["answers"].as_object().unwrap() {
        fake.answers.lock().unwrap().insert(
            bid.clone(),
            list.as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().map(|s| at(&root, s)))
                .collect(),
        );
    }
    let binaries = Binaries::default();
    for step in f["binary"]["steps"].as_array().unwrap() {
        if step["note"] == "gone: asked again" {
            std::fs::remove_file(&exe).unwrap();
        }
        let b = profiles::browser(step["browser"].as_str().unwrap()).unwrap();
        let start = fake.calls.lock().unwrap().len();
        let got = binaries.binary(&fake, b);
        assert_eq!(got, at(&root, step["binary"].as_str().unwrap()), "{step}");
        assert_eq!(
            json!(fake.calls.lock().unwrap()[start..]),
            step["calls"],
            "{step}"
        );
        compared += 1;
    }

    // running: pgrep's pattern
    let fake = Fake::default();
    let binaries = Binaries::default();
    for r in f["running"].as_array().unwrap() {
        let start = fake.pgrep.lock().unwrap().len();
        let got = profiles::running(&fake, &binaries, r["browser"].as_str().unwrap());
        let argv: Vec<Value> = fake.pgrep.lock().unwrap()[start..]
            .iter()
            .map(|p| json!(["pgrep", "-f", p]))
            .collect();
        assert_eq!(json!(argv), r["argv"], "{r}");
        assert_eq!(json!(got), r["running"], "{r}");
        compared += 1;
    }

    // _app_installed's path check
    std::fs::create_dir_all(root.join("home/.Trash/Old.app")).unwrap();
    std::fs::write(root.join("file.app"), "x").unwrap();
    for (rel, want) in f["app_path_ok"].as_object().unwrap() {
        assert_eq!(
            json!(profiles::app_path_ok(&root.join(rel))),
            *want,
            "{rel}"
        );
        compared += 1;
    }
    assert!(compared > 40, "{compared}");
}

/// Read only: asks the Mac's Launch Services, pgrep and the volume's capacity; opens and changes
/// nothing. Run with `cargo test -p ferb-browser --test profiles_mac -- --ignored`.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "asks this Mac's Launch Services (read only)"]
fn the_macs_launch_services_answer() {
    use ferb_browser::macos::{self, MacPlatform};
    let mac = MacPlatform;
    // Safari is part of macOS
    assert!(mac.has_app("com.apple.Safari"));
    let exe = mac.locate("com.apple.Safari").unwrap().unwrap();
    assert!(exe.is_file(), "{exe:?}");
    assert!(!mac.has_app("com.example.not-installed"));
    assert_eq!(mac.locate("com.example.not-installed"), Ok(None));
    let handlers = mac.https_handlers().unwrap();
    assert!(
        handlers
            .iter()
            .any(|p| p.to_string_lossy().contains("Safari")),
        "{handlers:?}"
    );
    // a scan never names a browser FERB knows, nor Safari (it isn't Chromium-built)
    for u in profiles::unsupported(&mac) {
        assert!(profiles::BROWSERS
            .iter()
            .all(|b| !b.bundle_id.eq_ignore_ascii_case(&u.bundle_id)));
        assert_ne!(u.bundle_id, "com.apple.Safari");
    }
    assert!(!mac.pgrep("^/nonexistent/ferb-test-binary( |$)"));
    let free = macos::important_usage(&std::env::temp_dir()).unwrap();
    assert!(free > 0);
}
