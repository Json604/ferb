//! A fresh process's reading of Screen Recording and Input Monitoring (ferb/device/grant_probe.py, APP-662): macOS
//! caches both per process, so the daemon starts a new one to read what TCC says now. It prompts for nothing.

/// kIOHIDRequestTypeListenEvent.
const LISTEN_EVENT: u32 = 1;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDCheckAccess(request: u32) -> u32;
}

/// grant_probe.py `_HID`.
pub fn hid_words(code: u32) -> &'static str {
    match code {
        0 => "granted",
        1 => "denied",
        2 => "not_determined",
        _ => "unknown",
    }
}

/// The probe's one line: `{"screen_recording": ..., "input_monitoring": ...}` as `json.dumps` writes it.
pub fn line(screen: bool, hid: u32) -> String {
    format!(
        "{{\"screen_recording\": \"{}\", \"input_monitoring\": \"{}\"}}",
        if screen { "granted" } else { "denied" },
        hid_words(hid)
    )
}

/// What TCC says now (reads only).
pub fn read() -> String {
    // SAFETY: both calls take plain values, return plain values and only read this process's grants.
    let (screen, hid) = unsafe {
        (
            CGPreflightScreenCaptureAccess(),
            IOHIDCheckAccess(LISTEN_EVENT),
        )
    };
    line(screen, hid)
}

/// grant_probe.py `main`: (the line to print, the exit status). It takes no arguments.
pub fn main(args: &[String]) -> (String, i32) {
    if !args.is_empty() {
        return ("{\"error\": \"takes no arguments\"}".into(), 64);
    }
    (read(), 0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_line_is_pythons() {
        assert_eq!(
            super::line(true, 2),
            r#"{"screen_recording": "granted", "input_monitoring": "not_determined"}"#
        );
        assert_eq!(
            super::line(false, 9),
            r#"{"screen_recording": "denied", "input_monitoring": "unknown"}"#
        );
        assert_eq!(super::main(&["x".into()]).1, 64);
    }
}
