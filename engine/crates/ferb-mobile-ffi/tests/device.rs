//! T-1214, T-1211 through the bindings: the phone's record, the Mac's request for a fix, recent files and the name
//! search; and a hidden answer handed over one UTF-16 unit at a time (`SecretInput`), overwritten when it goes.

mod common;

use std::sync::Arc;

use common::{FakeMac, ForeignMac, Heard, Signs, config, online, open};
use ferb_mobile_ffi::records::{Change, DevicePlace, DeviceReport, LocationFix};
use ferb_mobile_ffi::{FerbError, Mobile, SecretInput};
use serde_json::json;

fn connected(mac: &FakeMac, dir: &std::path::Path) -> (Arc<Mobile>, Arc<Heard>) {
    let heard = Arc::new(Heard::default());
    let m = open(dir, config("phone-1"), heard.clone()).unwrap();
    m.connect_transport(ForeignMac::new(mac.clone())).unwrap();
    m.link_event(online("direct")).unwrap();
    m.sync().unwrap();
    (m, heard)
}

fn report() -> DeviceReport {
    DeviceReport {
        name: "Moto".into(),
        model: "moto g84".into(),
        os: "Android 15".into(),
        time_zone: "Asia/Kolkata".into(),
        utc_offset_minutes: 330,
        location: "on".into(),
        place: Some(DevicePlace {
            city: "Pune".into(),
            region: "Maharashtra".into(),
            country: "India".into(),
            country_code: "IN".into(),
        }),
        battery_percent: Some(55),
        charging: Some(true),
        power_saver: Some(false),
        network: "mobile".into(),
        locale: "en-IN".into(),
        languages: vec!["en-IN".into()],
        screen_on: None,
        screen_locked: None,
    }
}

// decision: PHN-523
// decision: GATE-614
// decision: PHN-526
// decision: PHN-527
#[test]
fn the_record_the_fix_and_finding_files_through_the_bindings() {
    let mac = FakeMac::new("C1");
    let dir = tempfile::tempdir().unwrap();
    let (m, heard) = connected(&mac, dir.path());
    assert!(m.report_device(report()).unwrap());
    assert!(!m.report_device(report()).unwrap(), "the same twice: once");
    let sent = mac.calls_of("phone.device_report");
    assert_eq!(
        sent[0]["battery"],
        json!({"percent": 55, "charging": true, "power_saver": false})
    );
    m.link_event(r#"{"type":"location_wanted","request_id":"q9"}"#.into())
        .unwrap();
    assert!(heard.0.lock().unwrap().iter().any(|c| matches!(
        c,
        Change::LocationWanted { request_id } if request_id == "q9"
    )));
    assert_eq!(m.location_wanted().unwrap().as_deref(), Some("q9"));
    m.answer_location(
        "q9".into(),
        Some(LocationFix {
            latitude: 1.5,
            longitude: 2.5,
            accuracy_m: 3.0,
        }),
        None,
    )
    .unwrap();
    assert_eq!(m.recent_files(10).unwrap().files.len(), 2);
    assert!(m.search_files("inv".into(), None, None).unwrap().incomplete);
    m.shutdown();
    assert!(matches!(
        m.report_device(report()),
        Err(FerbError::Closed { .. })
    ));
}

/// A hidden answer from a `SecretInput`: pushed unit by unit (a character outside the basic plane as two), sent on the
/// sealed path signed over its hash only, and the input is empty after, whether the call went or failed.
// decision: PHN-529
// decision: PHN-110
#[test]
fn a_hidden_answer_from_units_leaves_the_input_empty_whatever_happens() {
    let mac = FakeMac::new("C1");
    let dir = tempfile::tempdir().unwrap();
    let (m, _heard) = connected(&mac, dir.path());
    let secret = SecretInput::new();
    for u in "pa55-😀".encode_utf16() {
        secret.push(u);
    }
    assert_eq!(secret.len(), 7);
    let signs = Arc::new(Signs::default());
    m.answer_hidden("q1".into(), secret.clone(), signs.clone())
        .unwrap();
    assert!(secret.is_empty());
    let answer = mac.calls_of("chat.answer");
    assert_eq!(answer[0]["text"], "pa55-😀");
    assert!(
        !signs.0.lock().unwrap()[0].contains("pa55"),
        "the proof is over the hash only"
    );
    // a broken unit: refused, and the input still emptied
    let broken = SecretInput::new();
    broken.push(0xD800);
    assert!(matches!(
        m.answer_hidden("q1".into(), broken.clone(), signs.clone()),
        Err(FerbError::BadArgument { .. })
    ));
    assert!(broken.is_empty());
    // closed: refused, emptied
    let late = SecretInput::new();
    for u in "1234".encode_utf16() {
        late.push(u);
    }
    m.shutdown();
    assert!(m.answer_hidden("q1".into(), late.clone(), signs).is_err());
    assert!(late.is_empty());
    // growing past its first buffer keeps every unit; clear empties
    let long = SecretInput::new();
    for _ in 0..600 {
        long.push(u16::from(b'x'));
    }
    assert_eq!(long.len(), 600);
    long.clear();
    assert!(long.is_empty());
}
