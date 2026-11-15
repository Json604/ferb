//! System reads: the general pasteboard (change count, types and access state never make macOS ask; contents only
//! when allowed: SENSE-404), the login session and display sleep (presence), Input Monitoring's state (no prompt),
//! and the programs the software and tab watchers run (`osascript` with Automation already granted, `xattr`,
//! `codesign`, `spctl`), each with its output captured and nothing shown.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use core_foundation_sys::base::CFTypeRef;
use core_foundation_sys::dictionary::CFDictionaryGetValue;
use objc2::runtime::{AnyObject, Sel};
use objc2::{msg_send, sel};
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use serde_json::Value;

use super::ffi::*;
use super::{cfstr, pool, scalar, Owned};
use crate::software::Answer;
use crate::watchers::clipboard::Pasteboard;
use crate::watchers::presence::Session;
use crate::watchers::software::SoftwareReads;

/// The general pasteboard.
pub struct GeneralPasteboard;

fn board() -> objc2::rc::Retained<NSPasteboard> {
    NSPasteboard::generalPasteboard()
}

impl Pasteboard for GeneralPasteboard {
    fn change_count(&self) -> i64 {
        pool(|| board().changeCount() as i64)
    }

    /// clipboard.py:20: `accessBehavior` exists from macOS 15.4; before, None (unrestricted).
    fn access_behavior(&self) -> Option<i64> {
        pool(|| {
            let b = board();
            let obj: &AnyObject = &b;
            let selector: Sel = sel!(accessBehavior);
            // SAFETY: respondsToSelector: is answered by every NSObject.
            let responds: bool = unsafe { msg_send![obj, respondsToSelector: selector] };
            if !responds {
                return None;
            }
            // SAFETY: the selector exists (checked) and returns an NSInteger.
            let v: isize = unsafe { msg_send![obj, accessBehavior] };
            Some(v as i64)
        })
    }

    fn types(&self) -> Vec<String> {
        pool(|| {
            board()
                .types()
                .map(|a| a.iter().map(|t| t.to_string()).collect())
                .unwrap_or_default()
        })
    }

    /// Reads the contents: the watcher calls it only when access is `allow` or unrestricted.
    fn read_string(&self) -> Option<String> {
        // SAFETY: NSPasteboardTypeString is a constant AppKit defines.
        pool(|| {
            board()
                .stringForType(unsafe { NSPasteboardTypeString })
                .map(|s| s.to_string())
        })
    }
}

/// clipboard.py:34 `request_access`: setup only, with the owner there: one read so macOS shows its "paste from
/// other apps" alert now. Nothing read is kept. Returns the access state afterwards. Never called by a watcher.
pub fn request_access() -> &'static str {
    let b = GeneralPasteboard;
    if matches!(
        crate::watchers::clipboard::access(b.access_behavior()),
        "default" | "ask"
    ) {
        drop(b.read_string());
    }
    crate::watchers::clipboard::access(b.access_behavior())
}

/// presence.py:34 `session`: the login session as CoreGraphics reports it (read-only); empty when unreadable.
pub fn session() -> Session {
    pool(|| {
        // SAFETY: a read-only query; the dictionary is owned (Copy rule).
        let Some(d) = Owned::new(unsafe { CGSessionCopyCurrentDictionary() } as CFTypeRef) else {
            return Session::default();
        };
        let get = |key: &str| {
            let k = cfstr(key);
            // SAFETY: `d` is a CFDictionary; the value (if any) is checked by `scalar`.
            unsafe { scalar(CFDictionaryGetValue(d.0 as _, k.0)) }
        };
        let flag = |v: Option<Value>, default: bool| match v {
            Some(Value::Bool(b)) => b,
            Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
            Some(_) => true,
            None => default,
        };
        Session {
            id: match get("CGSSessionUniqueSessionUUID") {
                Some(Value::String(s)) if !s.is_empty() => Some(s),
                _ => None,
            },
            locked: Some(flag(get("CGSSessionScreenIsLocked"), false)),
            on_console: Some(flag(get("kCGSSessionOnConsoleKey"), true)),
        }
    })
}

/// presence.py:50 `displays_asleep`: the main display is asleep (read-only).
pub fn displays_asleep() -> Option<bool> {
    // SAFETY: read-only queries with no preconditions.
    Some(unsafe { CGDisplayIsAsleep(CGMainDisplayID()) } != 0)
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDCheckAccess(request: u32) -> u32;
}

/// device/permissions.py `input_monitoring`: Input Monitoring granted (`IOHIDCheckAccess`, which never prompts).
pub fn input_monitoring_granted() -> bool {
    // SAFETY: a read-only query; 1 is kIOHIDRequestTypeListenEvent and 0 kIOHIDAccessTypeGranted.
    unsafe { IOHIDCheckAccess(1) == 0 }
}

/// Run a program with no input, capturing its output, killed after `limit` (None: no answer in time).
pub fn run(args: &[String], limit: Duration) -> std::io::Result<Option<Answer>> {
    let (program, rest) = args
        .split_first()
        .ok_or_else(|| std::io::Error::other("no program"))?;
    let mut child = Command::new(program)
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut out = child.stdout.take().expect("piped");
    let mut err = child.stderr.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut o = Vec::new();
        let _ = out.read_to_end(&mut o);
        o
    });
    let ereader = std::thread::spawn(move || {
        let mut e = Vec::new();
        let _ = err.read_to_end(&mut e);
        e
    });
    let started = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break Some(s);
        }
        if started.elapsed() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let (o, e) = (
        reader.join().unwrap_or_default(),
        ereader.join().unwrap_or_default(),
    );
    Ok(status.map(|s| (s.code().unwrap_or(-1), o, e)))
}

/// `osascript -e script` (browser_tabs.py: 5 s): its standard output when it succeeded.
pub fn osascript(script: &str) -> Result<Option<String>, String> {
    let args = vec![
        "osascript".to_string(),
        "-e".to_string(),
        script.to_string(),
    ];
    match run(&args, Duration::from_secs(5)) {
        Ok(Some((0, out, _))) => Ok(Some(String::from_utf8_lossy(&out).into_owned())),
        Ok(Some(_)) => Ok(None),
        Ok(None) => Err("TimeoutExpired".into()),
        Err(e) => Err(crate::software::os_error_name(&e).into()),
    }
}

/// browser_tabs.py:45 `applescript_front_tab` (Automation must already be granted; never asked for here).
pub fn front_tab(bundle: &str) -> Result<Option<(String, String)>, String> {
    let Some(script) = crate::watchers::browser_tabs::front_tab_script(bundle) else {
        return Ok(None);
    };
    Ok(osascript(&script)?.and_then(|out| crate::watchers::browser_tabs::parse_front_tab(&out)))
}

/// browser_tabs.py:123 `applescript_windows`: every window of the browser; None when it didn't answer.
pub fn browser_windows(
    bundle: &str,
) -> Result<Option<Vec<crate::watchers::browser_tabs::ScriptedWindow>>, String> {
    let Some(script) = crate::watchers::browser_tabs::windows_script(bundle) else {
        return Ok(None);
    };
    Ok(osascript(&script)?.map(|out| crate::watchers::browser_tabs::parse_windows(&out)))
}

/// software.py:110 `read_xattr`: `/usr/bin/xattr -px` keeps binary attributes as hex.
pub fn read_xattr(path: &Path, name: &str) -> std::io::Result<Vec<u8>> {
    let args = vec![
        "/usr/bin/xattr".to_string(),
        "-px".into(),
        name.into(),
        crate::paths::text(path),
    ];
    match run(&args, Duration::from_secs(30))? {
        Some((0, out, _)) => {
            let text: String = String::from_utf8_lossy(&out).split_whitespace().collect();
            hex::decode(text).map_err(|_| std::io::Error::other("extended attribute unavailable"))
        }
        _ => Err(std::io::Error::other("extended attribute unavailable")),
    }
}

/// The software watcher's reads on this Mac (home: the owner's).
pub struct MacSoftwareReads {
    pub home: std::path::PathBuf,
}

impl SoftwareReads for MacSoftwareReads {
    fn stamp(&self) -> Vec<String> {
        crate::software::change_stamp(
            &crate::software::tcc_databases(&self.home),
            &crate::software::job_folders(&self.home),
        )
    }

    fn security_facts(&self, bundle: &str, path: &str) -> Value {
        let run_it = |args: &[String]| -> std::io::Result<Answer> {
            run(args, Duration::from_secs(60))?.ok_or_else(|| std::io::Error::other("timed out"))
        };
        let privacy = |b: &str, p: &Path| self.privacy(b, &crate::paths::text(p));
        let jobs = |p: &Path| self.jobs(&crate::paths::text(p));
        crate::software::app_security_facts(
            Path::new(path),
            bundle,
            &crate::software::Readers {
                run: &run_it,
                xattr: &read_xattr,
                privacy: &privacy,
                jobs: &jobs,
            },
        )
    }

    fn privacy(&self, bundle: &str, path: &str) -> Value {
        #[cfg(feature = "store")]
        {
            crate::software::privacy_both(bundle, Path::new(path), &self.home)
        }
        #[cfg(not(feature = "store"))]
        {
            let _ = (bundle, path);
            Value::Null
        }
    }

    fn jobs(&self, path: &str) -> Value {
        Value::Array(crate::software::associated_jobs(
            Path::new(path),
            &crate::software::job_folders(&self.home),
        ))
    }

    fn login_items(&self, bundle: &str, path: &str) -> Value {
        crate::software::login_items(Path::new(path), bundle, None)
    }
}
