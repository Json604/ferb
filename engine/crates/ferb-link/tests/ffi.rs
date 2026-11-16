//! The app's door (`ffi`): clients with explicit handles and close, several in one process, panics caught at the
//! boundary, every call bounded, and the lab's three calls still working (kb PHN-446's reasons: the lab's library had
//! no close, no catch_unwind and one global client). Plain tests: the door blocks on each client's own runtime.
use ferb_link::ffi;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// A local relay for the clients (their own runtime is the door's; this one only runs the relay).
fn relay() -> (tokio::runtime::Runtime, iroh_relay::server::Server, String) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(async {
        let mut c = iroh_relay::server::ServerConfig::default();
        c.relay = Some(iroh_relay::server::RelayConfig::new((
            std::net::Ipv4Addr::LOCALHOST,
            0,
        )));
        iroh_relay::server::Server::spawn(c).await.unwrap()
    });
    let url = format!("http://localhost:{}", server.http_addr().unwrap().port());
    (rt, server, url)
}
fn config(url: &str) -> String {
    json!({"name": "phone", "app": "test", "dev": {"shared_relays": [url], "portmapper": false, "loopback": true}})
        .to_string()
}
fn parse(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or(Value::Null)
}

// decision: PHN-446, PHN-460, PHN-514
#[test]
fn clients_have_handles_and_close_and_several_live_in_one_process() {
    let (_rt, _server, url) = relay();
    let a = parse(&ffi::client_start(&config(&url)));
    let b = parse(&ffi::client_start(&config(&url)));
    let (ha, hb) = (a["handle"].as_u64().unwrap(), b["handle"].as_u64().unwrap());
    assert_ne!(ha, hb);
    assert_ne!(a["id"], b["id"], "two clients, two identities: {a} {b}");
    // each answers on its own
    for h in [ha, hb] {
        let r = parse(&ffi::client_command(
            h,
            &json!({"cmd": "phase"}).to_string(),
        ));
        assert_eq!(r["ok"], true, "{r}");
        let pay = parse(&ffi::client_command(
            h,
            &json!({"cmd": "call_payload", "challenge": "c", "method": "m", "params": {}})
                .to_string(),
        ));
        assert_eq!(
            pay["payload"],
            json!({"challenge": "c", "method": "m", "params": {}}).to_string()
        );
    }
    // the first one starts with its state handed to the app (a `persist` event), within the wait asked for
    let t = Instant::now();
    let mut persisted = false;
    while t.elapsed() < Duration::from_secs(5) && !persisted {
        let ev = ffi::client_poll(ha, 500);
        persisted = parse(&ev)["type"] == "persist";
    }
    assert!(persisted);
    // close: the handle is gone; a later call says so, never touches freed memory; a second close says so too
    assert_eq!(parse(&ffi::client_close(ha))["ok"], true);
    for r in [
        ffi::client_command(ha, r#"{"cmd":"phase"}"#),
        ffi::client_poll(ha, 10),
        ffi::client_close(ha),
    ] {
        assert_eq!(parse(&r)["error"]["code"], "link_closed", "{r}");
    }
    // the other is untouched
    assert_eq!(
        parse(&ffi::client_command(hb, r#"{"cmd":"phase"}"#))["ok"],
        true
    );
    assert_eq!(parse(&ffi::client_close(hb))["ok"], true);
    // a handle never given
    assert_eq!(
        parse(&ffi::client_poll(987_654, 0))["error"]["code"],
        "link_closed"
    );
}

// decision: PHN-446, PHN-460, PHN-514
#[test]
fn every_call_is_bounded_and_a_panic_stays_inside() {
    let (_rt, _server, url) = relay();
    let h = parse(&ffi::client_start(&config(&url)))["handle"]
        .as_u64()
        .unwrap();
    // drain what is there, then: a poll asking for an hour comes back within its bound (here: nothing arrives)
    while !ffi::client_poll(h, 200).is_empty() {}
    let t = Instant::now();
    assert_eq!(ffi::client_poll(h, 300), "");
    assert!(t.elapsed() < Duration::from_secs(2));
    assert!(ffi::POLL_WAIT_MAX <= Duration::from_secs(60));
    // a poll waiting on one thread is ended by a close on another, well before its wait is over
    let waiter = std::thread::spawn(move || {
        let t = Instant::now();
        let r = ffi::client_poll(h, 30_000);
        (r, t.elapsed())
    });
    std::thread::sleep(Duration::from_millis(300));
    let t = Instant::now();
    assert_eq!(parse(&ffi::client_close(h))["ok"], true);
    assert!(
        t.elapsed() <= ffi::CLOSE_DEADLINE + Duration::from_secs(1),
        "close took {:?}",
        t.elapsed()
    );
    let (_, waited) = waiter.join().unwrap();
    assert!(
        waited < Duration::from_secs(10),
        "the poll outlived the close: {waited:?}"
    );
    // a command that is not JSON, an unknown command: errors, not crashes
    let h = parse(&ffi::client_start(&config(&url)))["handle"]
        .as_u64()
        .unwrap();
    assert_eq!(
        parse(&ffi::client_command(h, "not json"))["error"]["code"],
        "bad_request"
    );
    assert_eq!(
        parse(&ffi::client_command(h, r#"{"cmd":"fly"}"#))["error"]["code"],
        "bad_request"
    );
    // a panic at the boundary is an error reply with its stable code, and the process goes on
    let r = parse(&ffi::guarded_for_test(|| panic!("boom")));
    assert_eq!(r["error"]["code"], ffi::LINK_INTERNAL_ERROR);
    assert_eq!(
        parse(&ffi::client_command(h, r#"{"cmd":"phase"}"#))["ok"],
        true
    );
    ffi::client_close(h);
}

// decision: PHN-446, PHN-460, PHN-514
#[test]
fn the_labs_three_calls_still_drive_one_client() {
    let (_rt, _server, url) = relay();
    assert_eq!(
        parse(&ffi::command(r#"{"cmd":"phase"}"#))["error"]["code"],
        "bad_request",
        "before start"
    );
    let first = parse(&ffi::start(&config(&url)));
    assert_eq!(first["ok"], true, "{first}");
    let again = parse(&ffi::start(&config(&url)));
    assert_eq!(
        (again["already"].clone(), again["id"].clone()),
        (json!(true), first["id"].clone())
    );
    assert!(parse(&ffi::command(r#"{"cmd":"phase"}"#))["phase"]["phase"].is_string());
    let t = Instant::now();
    let mut seen = false;
    while t.elapsed() < Duration::from_secs(5) && !seen {
        seen = parse(&ffi::poll(300))["type"] == "persist";
    }
    assert!(seen, "the lab's poll hands out the client's events");
}
