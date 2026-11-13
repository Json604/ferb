//! Running apps and Launch Services on the real Mac (apps.py, browsers.py, launched.py's quit and hide): the driver's
//! `Apps`. Lookups never resolve ambiguity by order; launching is `open -g -b` (never brought forward, HAND-239);
//! opening a file or address uses NSWorkspace with `activates` off (HAND-240); reopen is the core Apple Event without
//! kAECanSwitchLayer. Each call drains its own autorelease pool.

#![allow(deprecated)] // the same Launch Services calls as apps.py (typeOfFile, type_conformsToType, fullPathForApplication)

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSApplicationActivationOptions, NSApplicationActivationPolicy, NSRunningApplication,
    NSWorkspace, NSWorkspaceOpenConfiguration,
};
use objc2_foundation::{
    NSAppleEventDescriptor, NSAppleEventSendOptions, NSArray, NSBundle, NSDictionary, NSError,
    NSFileManager, NSNumber, NSString, NSURLIsApplicationKey, NSURL,
};
use serde_json::{Map, Value};

use crate::apps::{
    self as appsm, RunningApp, WebHandlers, OPEN_WAIT_S, PROFILE_TYPES, RUNNABLE_TYPES,
};
use crate::driver::Apps;
use crate::launched::AppControl;

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

fn running_all() -> Vec<(Retained<NSRunningApplication>, Option<String>, i32)> {
    NSWorkspace::sharedWorkspace()
        .runningApplications()
        .to_vec()
        .into_iter()
        .map(|a| {
            let b = a.bundleIdentifier().map(|s| s.to_string());
            let p = a.processIdentifier();
            (a, b, p)
        })
        .collect()
}

/// apps.py:17: the Regular apps with a bundle id.
pub fn running() -> Vec<RunningApp> {
    objc2::rc::autoreleasepool(|_| {
        running_all()
            .into_iter()
            .filter(|(a, b, _)| {
                a.activationPolicy() == NSApplicationActivationPolicy::Regular
                    && b.as_deref().is_some_and(|b| !b.is_empty())
            })
            .map(|(a, b, p)| RunningApp {
                bundle_id: b.unwrap_or_default(),
                name: a.localizedName().map(|n| n.to_string()).unwrap_or_default(),
                pid: p,
                active: a.isActive(),
            })
            .collect()
    })
}

/// An Info.plist value as JSON (only what the rules read: strings, numbers, booleans, arrays, dictionaries).
fn plist(v: &AnyObject) -> Value {
    if let Some(s) = v.downcast_ref::<NSString>() {
        return Value::String(s.to_string());
    }
    if let Some(n) = v.downcast_ref::<NSNumber>() {
        return serde_json::Number::from_f64(n.doubleValue())
            .map(Value::Number)
            .unwrap_or(Value::Null);
    }
    if let Some(a) = v.downcast_ref::<NSArray>() {
        return Value::Array(a.to_vec().iter().map(|x| plist(x)).collect());
    }
    if let Some(d) = v.downcast_ref::<NSDictionary>() {
        let mut m = Map::new();
        for k in d.allKeys().to_vec() {
            if let (Some(key), Some(val)) = (k.downcast_ref::<NSString>(), d.objectForKey(&k)) {
                m.insert(key.to_string(), plist(&val));
            }
        }
        return Value::Object(m);
    }
    Value::Null
}

fn info_of_bundle(bundle_id: &str) -> Option<Value> {
    let url =
        NSWorkspace::sharedWorkspace().URLForApplicationWithBundleIdentifier(&ns(bundle_id))?;
    let bundle = NSBundle::bundleWithURL(&url)?;
    let info = bundle.infoDictionary()?;
    Some(plist(&info))
}

fn wait_slot<T: Send + 'static>(slot: &Arc<(Mutex<Option<T>>, Condvar)>, secs: f64) -> Option<T> {
    let (m, cv) = &**slot;
    let g = m.lock().unwrap_or_else(|p| p.into_inner());
    let (mut g, _) = cv
        .wait_timeout_while(g, Duration::from_secs_f64(secs), |v| v.is_none())
        .unwrap_or_else(|p| p.into_inner());
    g.take()
}

/// The real `Apps`.
pub struct NativeApps {
    pub handlers: Arc<WebHandlers>,
    started: Instant,
    shells: Mutex<std::collections::HashMap<String, bool>>,
    doc_types: Mutex<std::collections::HashMap<String, Value>>,
}

impl NativeApps {
    pub fn new(handlers: Arc<WebHandlers>) -> Self {
        NativeApps {
            handlers,
            started: Instant::now(),
            shells: Default::default(),
            doc_types: Default::default(),
        }
    }

    fn now(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// browsers.py:150: every app Launch Services lists as opening http or https links (bundle ids, names).
    pub fn enumerate_web_handlers() -> Option<(Vec<String>, Vec<String>)> {
        objc2::rc::autoreleasepool(|_| {
            let ws = NSWorkspace::sharedWorkspace();
            let (mut ids, mut names) = (vec![], vec![]);
            for link in ["http://example.com", "https://example.com"] {
                let url = NSURL::URLWithString(&ns(link))?;
                for app in ws.URLsForApplicationsToOpenURL(&url).to_vec() {
                    let Some(ident) =
                        NSBundle::bundleWithURL(&app).and_then(|b| b.bundleIdentifier())
                    else {
                        continue;
                    };
                    let id = appsm::clean(&ident.to_string());
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                    if let Some(path) = app.path() {
                        let name = NSFileManager::defaultManager()
                            .displayNameAtPath(&path)
                            .to_string();
                        let name = appsm::clean(name.strip_suffix(".app").unwrap_or(&name));
                        if !name.is_empty() && !names.contains(&name) {
                            names.push(name);
                        }
                    }
                }
            }
            Some((ids, names))
        })
    }

    fn conforms(ws: &NSWorkspace, a: &str, b: &str) -> bool {
        #[allow(deprecated)]
        ws.type_conformsToType(&ns(a), &ns(b))
    }
}

impl Apps for NativeApps {
    fn find(&self, q: &str) -> Option<RunningApp> {
        appsm::find(&running(), q)
    }
    fn candidates(&self, q: &str) -> Option<Vec<RunningApp>> {
        Some(appsm::candidates(&running(), q))
    }
    fn running(&self) -> Vec<RunningApp> {
        running()
    }
    fn pids_of(&self, bundle_ids: &[&str]) -> Vec<i32> {
        objc2::rc::autoreleasepool(|_| {
            running_all()
                .into_iter()
                .filter(|(_, b, _)| {
                    b.as_deref().is_some_and(|b| {
                        bundle_ids.contains(&ferb_sanitize::parity::casefold(b).as_str())
                    })
                })
                .map(|(_, _, p)| p)
                .collect()
        })
    }
    /// apps.py:116: Accessibility's focused application first (NSWorkspace's flag goes stale in a daemon).
    fn frontmost(&self) -> Option<RunningApp> {
        use crate::ax::AxSource;
        let src = super::ax::NativeAx::default();
        let (err, focused) = src.raw_attr(&src.system(), "AXFocusedApplication");
        if let (0, Some(f)) = (err, focused.as_el()) {
            if let Some(pid) = src.pid_of(&f) {
                let found = objc2::rc::autoreleasepool(|_| {
                    NSRunningApplication::runningApplicationWithProcessIdentifier(pid).map(|a| {
                        RunningApp {
                            bundle_id: a
                                .bundleIdentifier()
                                .map(|s| s.to_string())
                                .unwrap_or_default(),
                            name: a.localizedName().map(|s| s.to_string()).unwrap_or_default(),
                            pid,
                            active: true,
                        }
                    })
                });
                if let Some(a) = found {
                    return Some(a);
                }
            }
        }
        running().into_iter().find(|a| a.active)
    }
    /// apps.py:142: the explicit foreground route (only after the owner's Yes): one resolved process.
    fn activate(&self, bundle_id: &str, pid: Option<i32>) -> bool {
        let q = match pid {
            Some(p) => format!("pid:{p}"),
            None => bundle_id.to_string(),
        };
        let matches = appsm::candidates(&running(), &q);
        if matches.len() != 1 || matches[0].bundle_id != bundle_id {
            return false;
        }
        objc2::rc::autoreleasepool(|_| {
            NSRunningApplication::runningApplicationWithProcessIdentifier(matches[0].pid)
                .is_some_and(|a| {
                    a.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps)
                })
        })
    }
    /// apps.py:164 (HAND-239): `open -g -b`: launched in the background, never unhidden.
    fn launch(&self, bundle_id: &str) -> bool {
        if !appsm::candidates(&running(), bundle_id).is_empty() {
            return true;
        }
        if objc2::rc::autoreleasepool(|_| {
            NSWorkspace::sharedWorkspace()
                .URLForApplicationWithBundleIdentifier(&ns(bundle_id))
                .is_none()
        }) {
            return false;
        }
        std::process::Command::new("/usr/bin/open")
            .args(["-g", "-b", bundle_id])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    /// apps.py:225 (HAND-240): a file or web address opened in the app without activating it.
    fn open_in(&self, bundle_id: &str, target: &str) -> Option<String> {
        let slot: Arc<(Mutex<Option<Option<String>>>, Condvar)> =
            Arc::new((Mutex::new(None), Condvar::new()));
        let started = objc2::rc::autoreleasepool(|_| -> Result<(), String> {
            let ws = NSWorkspace::sharedWorkspace();
            let app = ws
                .URLForApplicationWithBundleIdentifier(&ns(bundle_id))
                .ok_or_else(|| format!("no installed app {bundle_id}"))?;
            let url = if target.starts_with('/') {
                Some(NSURL::fileURLWithPath(&ns(target)))
            } else {
                NSURL::URLWithString(&ns(target))
            };
            let url = url.ok_or_else(|| {
                format!(
                    "{} isn't an address macOS can open",
                    crate::driver::py_repr(target)
                )
            })?;
            let config = NSWorkspaceOpenConfiguration::configuration();
            config.setActivates(false);
            let s2 = slot.clone();
            let done = RcBlock::new(
                move |_app: *mut NSRunningApplication, error: *mut NSError| {
                    // SAFETY: a valid NSError or null, for the call's duration.
                    let why = unsafe { Retained::retain(error) }
                        .map(|e| e.localizedDescription().to_string());
                    let (m, cv) = &*s2;
                    *m.lock().unwrap_or_else(|p| p.into_inner()) = Some(why);
                    cv.notify_all();
                },
            );
            ws.openURLs_withApplicationAtURL_configuration_completionHandler(
                &NSArray::from_retained_slice(&[url]),
                &app,
                &config,
                Some(&done),
            );
            Ok(())
        });
        if let Err(why) = started {
            return Some(why);
        }
        match wait_slot(&slot, OPEN_WAIT_S) {
            Some(why) => why,
            None => Some(format!("macOS didn't confirm the open within {:.0} s; check with mac_snapshot before trying again", OPEN_WAIT_S)),
        }
    }
    /// apps.py:268: the Dock's reopen event, without kAECanSwitchLayer, never interacting, no reply awaited.
    fn reopen(&self, pid: i32) -> Option<String> {
        objc2::rc::autoreleasepool(|_| {
            let code = |s: &[u8; 4]| u32::from_be_bytes(*s);
            let target = NSAppleEventDescriptor::descriptorWithProcessIdentifier(pid);
            let event = NSAppleEventDescriptor::appleEventWithEventClass_eventID_targetDescriptor_returnID_transactionID(code(b"aevt"), code(b"rapp"), Some(&target), -1, 0);
            let options = NSAppleEventSendOptions(0x1 | 0x10 | 0x1000);
            event
                .sendEventWithOptions_timeout_error(options, 5.0)
                .err()
                .map(|e| e.localizedDescription().to_string())
        })
    }
    /// apps.py:177: Launch Services counts the path an application; unreadable counts as one.
    fn is_application(&self, path: &str) -> Option<bool> {
        objc2::rc::autoreleasepool(|_| {
            let url = NSURL::fileURLWithPath(&ns(path));
            let mut value: Option<Retained<AnyObject>> = None;
            // SAFETY: the documented key; `value` receives an NSNumber or nil.
            let ok =
                unsafe { url.getResourceValue_forKey_error(&mut value, NSURLIsApplicationKey) };
            Some(match ok {
                Ok(()) => value
                    .and_then(|v| v.downcast::<NSNumber>().ok())
                    .map(|n| n.boolValue())
                    .unwrap_or(false),
                Err(_) => true,
            })
        })
    }
    fn opens_web_links(&self, bundle_id: &str) -> bool {
        let (ids, _) = self
            .handlers
            .installed(self.now(), Self::enumerate_web_handlers);
        ids.contains(&appsm::clean(bundle_id))
    }
    fn known_opens_web_links(&self, bundle_id: &str) -> bool {
        self.handlers.known_opens_web_links(bundle_id, self.now())
    }
    /// apps.py:83: the Shell role for executables or shell scripts, cached per bundle id.
    fn declares_shell(&self, bundle_id: &str) -> bool {
        let key = ferb_sanitize::parity::casefold(ferb_py::value::strip(bundle_id));
        if let Some(v) = self.shells.lock().expect("lock").get(&key) {
            return *v;
        }
        let v =
            objc2::rc::autoreleasepool(|_| appsm::shell_role(info_of_bundle(bundle_id).as_ref()));
        self.shells.lock().expect("lock").insert(key, v);
        v
    }
    /// apps.py:197 (HAND-241): the file's type can start code when opened; unknown or unreadable counts.
    fn runs_when_opened(&self, path: &str) -> bool {
        objc2::rc::autoreleasepool(|_| {
            let ws = NSWorkspace::sharedWorkspace();
            if ws.isFilePackageAtPath(&ns(path)) {
                return true;
            }
            let Ok(kind) = ws.typeOfFile_error(&ns(path)) else {
                return true;
            };
            let kind = kind.to_string();
            if kind.is_empty() || kind.starts_with("dyn.") {
                return true;
            }
            RUNNABLE_TYPES.iter().any(|t| Self::conforms(&ws, &kind, t))
        })
    }
    /// apps.py:203: the app declares itself the file's Editor (never for a configuration profile).
    fn edits(&self, bundle_id: &str, path: &str) -> bool {
        objc2::rc::autoreleasepool(|_| {
            let ws = NSWorkspace::sharedWorkspace();
            let kind = ws
                .typeOfFile_error(&ns(path))
                .map(|k| k.to_string())
                .unwrap_or_default();
            if kind.is_empty() || PROFILE_TYPES.iter().any(|t| Self::conforms(&ws, &kind, t)) {
                return false;
            }
            let key = ferb_sanitize::parity::casefold(ferb_py::value::strip(bundle_id));
            let types = {
                let mut cache = self.doc_types.lock().expect("lock");
                cache
                    .entry(key)
                    .or_insert_with(|| {
                        info_of_bundle(bundle_id)
                            .and_then(|i| i.get("CFBundleDocumentTypes").cloned())
                            .unwrap_or(Value::Array(vec![]))
                    })
                    .clone()
            };
            let ext = std::path::Path::new(path.trim_end_matches('/'))
                .extension()
                .map(|e| ferb_sanitize::parity::casefold(&e.to_string_lossy()))
                .unwrap_or_default();
            let info = serde_json::json!({"CFBundleDocumentTypes": types});
            appsm::editor_role(Some(&info), &kind, &ext, &|a, b| Self::conforms(&ws, a, b))
        })
    }
    /// apps.py:103: the app Launch Services would open the file with.
    fn default_app_for(&self, path: &str) -> Option<String> {
        objc2::rc::autoreleasepool(|_| {
            let url = NSWorkspace::sharedWorkspace()
                .URLForApplicationToOpenURL(&NSURL::fileURLWithPath(&ns(path)))?;
            NSBundle::bundleWithURL(&url)
                .and_then(|b| b.bundleIdentifier())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
        })
    }
}

/// launched.py's quit and hide of exactly one process (as ⌘Q and ⌘H do), and AXEdited.
pub struct NativeControl;

impl AppControl for NativeControl {
    fn running(&self) -> Vec<RunningApp> {
        running()
    }
    fn unsaved(&self, pid: i32) -> Option<bool> {
        super::ax::unsaved(pid)
    }
    fn quit(&self, pid: i32, bundle_id: &str) -> bool {
        objc2::rc::autoreleasepool(|_| {
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
                .filter(|a| {
                    a.bundleIdentifier().map(|b| b.to_string()).as_deref() == Some(bundle_id)
                })
                .is_some_and(|a| a.terminate())
        })
    }
    fn hide(&self, pid: i32, bundle_id: &str) -> bool {
        objc2::rc::autoreleasepool(|_| {
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
                .filter(|a| {
                    a.bundleIdentifier().map(|b| b.to_string()).as_deref() == Some(bundle_id)
                })
                .is_some_and(|a| a.hide())
        })
    }
}

/// apps.py:153: the installed app macOS finds by this name (for an app that isn't running).
pub fn installed_bundle(name: &str) -> Option<String> {
    if ferb_py::value::strip(name).is_empty() || name.starts_with("pid:") {
        return None;
    }
    objc2::rc::autoreleasepool(|_| {
        let path = NSWorkspace::sharedWorkspace()
            .fullPathForApplication(&ns(ferb_py::value::strip(&appsm::without_marks(name))))?;
        NSBundle::bundleWithPath(&path)
            .and_then(|b| b.bundleIdentifier())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    })
}
