//! T-1214 and T-1211 against the fake Mac: the phone's own record sent when it changes and again on each connection
//! (never on a timer), the Mac's request for one precise fix and its answer, the recent files, the name search and a
//! task's file.

mod common;

use std::sync::Arc;

use common::mac::FakeMac;
use common::phone::{online, Phone, OFFLINE};
use ferb_mobile::transport::TransferSink;
use ferb_mobile::{Change, DeviceReport, ErrorCode, Fix, Mobile, Place};
use serde_json::json;

fn connected(phone: &Phone, mac: &FakeMac, tier: &str) -> Mobile {
    let m = phone.open();
    m.connect(Arc::new(mac.clone())).unwrap();
    m.link_event(&online(tier));
    phone.jobs.run();
    m
}

fn report(percent: u8) -> DeviceReport {
    DeviceReport {
        name: "Moto".into(),
        model: "motorola moto g84 5G".into(),
        os: "Android 15".into(),
        time_zone: "Asia/Kolkata".into(),
        utc_offset_minutes: 330,
        location: "on".into(),
        place: Some(Place {
            city: "Pune".into(),
            region: "Maharashtra".into(),
            country: "India".into(),
            country_code: "IN".into(),
        }),
        battery_percent: Some(percent),
        charging: Some(false),
        power_saver: None,
        network: "wifi".into(),
        locale: "en-IN".into(),
        languages: vec!["en-IN".into()],
        screen_on: Some(true),
        screen_locked: Some(false),
    }
}

/// Sent when it differs from what this connection already has, never twice the same, again after a reconnect; a
/// refusal (locked, device key not yet proved) leaves it for the next sync. Unknown facts are left out of the wire.
// decision: PHN-523
#[test]
fn the_phone_s_record_goes_when_it_changes_and_on_each_connection() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = phone.open();
    // offline: kept, nothing sent
    assert!(!m.report_device(report(80)).unwrap());
    m.connect(Arc::new(mac.clone())).unwrap();
    m.link_event(&online("direct"));
    phone.jobs.run();
    let sent = mac.calls_of("phone.device_report");
    assert_eq!(sent.len(), 1, "the sync on connecting sends what waited");
    assert_eq!(
        sent[0]["battery"],
        json!({"percent": 80, "charging": false})
    );
    assert_eq!(sent[0]["place"]["city"], "Pune");
    assert!(sent[0].get("latitude").is_none() && sent[0]["battery"].get("power_saver").is_none());
    // the same again: nothing; a change: once
    assert!(!m.report_device(report(80)).unwrap());
    assert!(m.report_device(report(75)).unwrap());
    assert_eq!(mac.calls_of("phone.device_report").len(), 2);
    // a reconnection hears it again
    m.link_event(OFFLINE);
    m.link_event(&online("direct"));
    phone.jobs.run();
    assert_eq!(mac.calls_of("phone.device_report").len(), 3);
    // refused while locked: kept for the next sync
    mac.lock().device_locked = true;
    assert!(m.report_device(report(70)).is_err());
    mac.lock().device_locked = false;
    m.sync().unwrap();
    let all = mac.calls_of("phone.device_report");
    assert_eq!(all.len(), 5, "the refused try and the sync's");
    assert_eq!(all[4]["battery"]["percent"], 70);
}

/// The Mac's request for a precise fix reaches the app as a change and is kept until answered; the answer carries
/// the fix or why there is none.
// decision: GATE-614
#[test]
fn the_mac_s_request_for_a_fix_is_heard_and_answered() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = connected(&phone, &mac, "shared_relay");
    m.link_event(r#"{"type":"location_wanted","request_id":"q1"}"#);
    assert!(phone
        .heard
        .0
        .lock()
        .unwrap()
        .contains(&Change::LocationWanted {
            request_id: "q1".into()
        }));
    assert_eq!(m.location_wanted().unwrap().as_deref(), Some("q1"));
    // a few bytes of text: it crosses the public relay too
    m.answer_location(
        "q1",
        Some(Fix {
            latitude: 18.52,
            longitude: 73.85,
            accuracy_m: 12.0,
        }),
        None,
    )
    .unwrap();
    assert_eq!(m.location_wanted().unwrap(), None);
    m.answer_location(
        "q3",
        Some(Fix {
            latitude: 1.0,
            longitude: 2.0,
            accuracy_m: -1.0,
        }),
        None,
    )
    .unwrap();
    m.answer_location("q2", None, Some("You said no on the phone."))
        .unwrap();
    let calls = mac.calls_of("phone.device_report");
    let answers: Vec<_> = calls
        .iter()
        .filter(|c| c.get("request_id").is_some())
        .collect();
    assert_eq!(
        answers[0]["exact"],
        json!({"latitude": 18.52, "longitude": 73.85, "accuracy_m": 12.0})
    );
    assert_eq!(
        answers[1]["exact"],
        json!({"latitude": 1.0, "longitude": 2.0}),
        "no made-up accuracy"
    );
    assert_eq!(answers[2]["error"], "You said no on the phone.");
}

struct Quiet;
impl TransferSink for Quiet {
    fn progress(&self, _done: u64, _of: u64) {}
}

/// Recent files and a name search come back as the Mac listed them, with what was left out counted; an empty query
/// asks nothing; a task's file is fetched by its task and index.
// decision: PHN-526
// decision: PHN-527
// decision: PHN-528
#[test]
fn recent_files_search_and_a_task_s_file() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = connected(&phone, &mac, "direct");
    let r = m.recent_files(20).unwrap();
    assert_eq!(r.files.len(), 2);
    assert_eq!(
        (r.files[0].source.as_str(), r.files[0].task_name.as_deref()),
        ("made", Some("October invoice"))
    );
    assert_eq!(r.skipped, 2);
    assert_eq!(mac.calls_of("phone.recent")[0]["limit"], 20);
    let f = m.search_files(" invoice ", None).unwrap();
    assert_eq!(f.query, "invoice");
    assert!(f.files[0].dir && f.incomplete && f.skipped == 4);
    assert_eq!(mac.calls_of("phone.search")[0]["path"], "~");
    assert_eq!(
        m.search_files("  ", None).unwrap_err().code,
        ErrorCode::Refused
    );
    assert_eq!(mac.calls_of("phone.search").len(), 1);
    mac.lock()
        .blobs
        .insert("task:t1:1".into(), b"PDF-BYTES".to_vec());
    let dir = tempfile::tempdir().unwrap();
    let to = dir.path().join("receipt.pdf");
    let got = m
        .fetch_task_file("x1", "t1", 1, to.to_str().unwrap(), Arc::new(Quiet))
        .unwrap();
    assert_eq!(got.total, 9);
    assert_eq!(std::fs::read(&to).unwrap(), b"PDF-BYTES");
}
