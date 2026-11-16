//! T-1214: a paired phone's own record (`phone.device_report`) through a real ferb-rpc server as the link's helper
//! reaches the daemon: who may report, what is kept (the city, never coordinates), how long, and the one precise fix
//! the Mac asks of a connected phone after the owner's Yes.

mod phone_support;

use ferb_rpc::phone_gate as code;
use phone_support::*;
use serde_json::json;

const P1: &str = "endpoint-one";
const P2: &str = "endpoint-two";

fn report() -> serde_json::Value {
    json!({"name": "Moto", "model": "motorola moto g84 5G", "os": "Android 15", "time_zone": "Asia/Kolkata",
           "utc_offset_minutes": 330, "location": "on",
           "place": {"city": "Pune", "region": "Maharashtra", "country": "India", "country_code": "IN",
                     "latitude": 18.52, "longitude": 73.85},
           "battery": {"percent": 81, "charging": true, "power_saver": false}, "network": "wifi",
           "locale": "en-IN", "languages": ["en-IN", "hi-IN"], "screen": {"on": true, "locked": false}})
}

/// Who may report: the phone inside its unlock window, or (its app locked, Stay connected) a connection that proved
/// the phone's device key; anyone else on a locked connection is refused. What is kept is the city, never a
/// coordinate; a report with an unknown state is refused; a zone this Mac doesn't know is dropped.
// decision: PHN-523
// decision: SENSE-420
#[tokio::test]
async fn a_phone_reports_its_own_record_and_the_mac_keeps_only_the_city() {
    let w = world(None).await;
    let k = Keys::new(1);
    pair(&w, P1, &k, true, true).await;
    let mut c = Conn::phone(&w, P1, "s1").await;
    // locked, not proved: refused
    let e = c
        .call("phone.device_report", report(), None)
        .await
        .unwrap_err();
    assert_eq!(e.0, code::LOCKED, "{e:?}");
    assert!(w.phones.device_views()[0].record.is_none());
    // proved its device key: taken while locked
    assert_eq!(c.present(&k).await["present"], true);
    c.ok("phone.device_report", report()).await;
    let v = &w.phones.device_views()[0];
    assert!(v.connected);
    let r = v.record.as_ref().unwrap();
    assert_eq!(r.time_zone, "Asia/Kolkata");
    assert_eq!(r.place.as_ref().unwrap().text(), "Pune, Maharashtra, India");
    assert_eq!((r.battery_percent, r.charging), (Some(81), Some(true)));
    assert_eq!(r.languages, vec!["en-IN", "hi-IN"]);
    assert!(!format!("{r:?}").contains("18.52"), "never a coordinate");
    // location off: no place, whatever is sent
    let mut off = report();
    off["location"] = json!("off");
    c.ok("phone.device_report", off).await;
    assert!(w.phones.device_views()[0]
        .record
        .as_ref()
        .unwrap()
        .place
        .is_none());
    // an unknown zone is dropped (the offset stays), an unknown state refused
    let mut odd = report();
    odd["time_zone"] = json!("Mars/Olympus");
    c.ok("phone.device_report", odd).await;
    let r = w.phones.device_views()[0].record.clone().unwrap();
    assert_eq!(
        (r.time_zone.as_str(), r.utc_offset_minutes),
        ("", Some(330))
    );
    let mut bad = report();
    bad["location"] = json!("always");
    assert_eq!(
        c.call("phone.device_report", bad, None)
            .await
            .unwrap_err()
            .0,
        code::BAD_REQUEST
    );
    // another session, unlocked by the lock key, may report too
    let mut u = Conn::phone(&w, P1, "s2").await;
    u.unlock(&k).await;
    u.ok("phone.device_report", report()).await;
}

/// How long: while connected, and twelve hours (the longest one unlock lasts, PHN-141) after the phone's last report
/// or disconnect; a removed phone's record goes at once. Memory only: nothing of it is written to the store.
// decision: PHN-523
#[tokio::test]
async fn a_record_is_kept_while_connected_and_twelve_hours_after() {
    let w = world(None).await;
    let (k1, k2) = (Keys::new(1), Keys::new(2));
    pair(&w, P1, &k1, true, true).await;
    pair(&w, P2, &k2, true, true).await;
    let mut c = Conn::phone(&w, P1, "s").await;
    c.unlock(&k1).await;
    c.ok("phone.device_report", report()).await;
    w.clock.advance(13.0 * 3600.0);
    assert!(
        w.phones.device_views()[0].record.is_some(),
        "connected: kept"
    );
    c.ok("phone.session_end", json!({})).await;
    let v = &w.phones.device_views()[0];
    assert!(!v.connected && v.record.is_some());
    assert!(!v.last_seen.is_empty());
    // (the report is 13 h old but the phone was connected until now: the keep counts from the disconnect)
    w.clock.advance(11.9 * 3600.0);
    assert!(w.phones.device_views()[0].record.is_some());
    w.clock.advance(0.2 * 3600.0);
    assert!(w.phones.device_views()[0].record.is_none());
    // a fresh report, then not connected: kept for 12 h, gone after
    let mut c = Conn::phone(&w, P1, "s2").await;
    c.present(&k1).await;
    c.ok("phone.device_report", report()).await;
    c.ok("phone.session_end", json!({})).await;
    w.clock.advance(11.9 * 3600.0);
    assert!(w.phones.device_views()[0].record.is_some());
    w.clock.advance(0.2 * 3600.0);
    assert!(w.phones.device_views()[0].record.is_none());
    // removal: at once
    let mut c2 = Conn::phone(&w, P2, "t").await;
    c2.present(&k2).await;
    c2.ok("phone.device_report", report()).await;
    assert!(w.phones.device_views()[1].record.is_some());
    let mut id = ferb_rpc::Params::new();
    id.insert("id".into(), json!(P2));
    w.phones.remove(&app_session(), &id).unwrap();
    assert_eq!(w.phones.device_views().len(), 1);
    // nothing of the record reached the store's tables
    let db = w.db.lock().unwrap();
    let dump: String = db
        .conn()
        .query_row(
            "SELECT group_concat(detail_json, ' ') FROM phone_audit",
            [],
            |r| r.get::<_, Option<String>>(0),
        )
        .unwrap()
        .unwrap_or_default();
    assert!(!dump.contains("Pune") && !dump.contains("81"), "{dump}");
}

/// The one precise fix (GATE-614): asked only of a connected phone, through the helper (`link.events
/// location_wanted` naming the phone), answered only by the phone that was asked, audited without the coordinates.
// decision: GATE-614
#[tokio::test]
async fn a_precise_fix_is_asked_of_a_connected_phone_and_only_it_answers() {
    let w = world(None).await;
    let (k1, k2) = (Keys::new(1), Keys::new(2));
    pair(&w, P1, &k1, true, true).await;
    pair(&w, P2, &k2, true, true).await;
    let why = w.phones.want_fix(P1, "q0").unwrap_err();
    assert!(why.contains("isn't connected"), "{why}");
    let mut events = Conn::link(&w).await;
    events.start("link.events", json!({})).await;
    let mut c1 = Conn::phone(&w, P1, "s").await;
    c1.present(&k1).await;
    let mut c2 = Conn::phone(&w, P2, "t").await;
    c2.present(&k2).await;
    let rx = w.phones.want_fix(P1, "q1").unwrap();
    let mut seen = None;
    for _ in 0..5 {
        let ev = events.next_line().await;
        if ev["event"] == "location_wanted" {
            seen = Some(ev);
            break;
        }
    }
    let ev = seen.expect("the helper hears the request");
    assert_eq!(
        (ev["id"].as_str(), ev["request_id"].as_str()),
        (Some(P1), Some("q1"))
    );
    // another phone can't answer it
    let other = c2
        .ok(
            "phone.device_report",
            json!({"request_id": "q1", "exact": {"latitude": 1.0, "longitude": 2.0, "accuracy_m": 3.0}}),
        )
        .await;
    assert_eq!(other["delivered"], false);
    // out of range is refused; the asked phone's answer arrives
    assert_eq!(
        c1.call(
            "phone.device_report",
            json!({"request_id": "q1", "exact": {"latitude": 91.0, "longitude": 2.0, "accuracy_m": 3.0}}),
            None
        )
        .await
        .unwrap_err()
        .0,
        code::BAD_REQUEST
    );
    let got = c1
        .ok(
            "phone.device_report",
            json!({"request_id": "q1", "exact": {"latitude": 18.5204, "longitude": 73.8567, "accuracy_m": 9.0}}),
        )
        .await;
    assert_eq!(got["delivered"], true);
    let fix = rx.await.unwrap().unwrap();
    assert_eq!((fix.latitude, fix.longitude), (18.5204, 73.8567));
    assert_eq!(fix.accuracy_m, Some(9.0));
    // a fix without an accuracy (Android's hasAccuracy false): taken, said as none
    let rx = w.phones.want_fix(P1, "q1b").unwrap();
    c1.ok(
        "phone.device_report",
        json!({"request_id": "q1b", "exact": {"latitude": 18.5, "longitude": 73.8}}),
    )
    .await;
    assert_eq!(rx.await.unwrap().unwrap().accuracy_m, None);
    // the phone's No: delivered as why
    let rx = w.phones.want_fix(P1, "q2").unwrap();
    c1.ok(
        "phone.device_report",
        json!({"request_id": "q2", "error": "The owner said no on the phone."}),
    )
    .await;
    assert_eq!(
        rx.await.unwrap().unwrap_err(),
        "The owner said no on the phone."
    );
    let audit = audit_text(&w);
    assert!(audit.contains("phone.device_report"), "{audit}");
    assert!(!audit.contains("18.52"), "{audit}");
}
