#![cfg(target_os = "macos")]
//! How fast a paired phone reaches the Mac when the app is opened (kb PHN-740..742): every place the Mac may be is
//! dialed at once, and the dial does not wait for the phone's own relay. On 127.0.0.1 with local relays.
mod common;
use common::*;
use ferb_link::net::Shared;
use std::time::{Duration, Instant};

/// A relay address that takes the connection and never answers: the phone's own relay is not ready.
async fn silent_relay() -> (tokio::task::JoinHandle<()>, String) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let h = tokio::spawn(async move {
        let mut held = vec![];
        while let Ok((s, _)) = l.accept().await {
            held.push(s);
        }
    });
    (h, format!("http://localhost:{port}"))
}

/// A way to a relay that opens only at `opens` (a slow network to the relay): connections made before wait until then.
async fn slow_way_to(
    relay: &Relay,
    opens: tokio::time::Instant,
) -> (tokio::task::JoinHandle<()>, String) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let to = relay.port;
    let h = tokio::spawn(async move {
        while let Ok((mut a, _)) = l.accept().await {
            tokio::spawn(async move {
                tokio::time::sleep_until(opens).await;
                if let Ok(mut b) = tokio::net::TcpStream::connect(("127.0.0.1", to)).await {
                    let _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
                }
            });
        }
    });
    (h, format!("http://localhost:{port}"))
}

fn port_of(url: &str) -> String {
    url.trim_end_matches('/')
        .rsplit(':')
        .next()
        .unwrap()
        .to_string()
}

/// The time from the app's start to the phone being online, and how many dials it took.
async fn open(
    state: &ferb_link::client::PhoneState,
    shared: &Shared,
    within: u64,
) -> (TestPhone, Duration, usize) {
    let t = Instant::now();
    let p = TestPhone::start("phone", shared, false, Some(state.clone()), true).await;
    let online = p.wait_phase("online", within).await;
    let took = t.elapsed();
    assert!(online, "not online after {took:?}: {}", p.phone.phase());
    let dials = p.phone.dial_times().len();
    (p, took, dials)
}

// decision: PHN-740
/// The Mac moved to another relay while the app was closed (its home relay changes by itself with the network's
/// latencies). The phone remembers the old one: one dead candidate and one live one must not cost a dial timeout.
#[tokio::test(flavor = "multi_thread")]
async fn a_dead_candidate_beside_a_live_one_costs_nothing() {
    let mut old = Relay::start(None, None).await; // where the Mac was when the phone last saw it
    let now = Relay::start(None, None).await; // where the Mac is now
    let w = World::start(Shared::Custom(vec![now.spec()]), None, false).await;
    let p = w.paired_phone("phone").await;
    let mut state = p.saved().unwrap();
    p.phone.close().await;
    drop(p);
    let both = Shared::Custom(vec![old.spec(), now.spec()]);

    // the old relay is up, the Mac is just not on it
    state.mac.as_mut().unwrap().relays = vec![old.url()];
    let (p, took, dials) = open(&state, &both, 40).await;
    println!(
        "MEASURE the Mac moved to another relay (the old one up): online after {took:?}, {dials} dials"
    );
    assert!(
        took < Duration::from_secs(2) && dials == 1,
        "{took:?}, {dials} dials"
    );
    // the welcome says where the Mac is now: that is remembered for next time
    assert!(
        wait(5, || p
            .saved()
            .unwrap()
            .mac
            .unwrap()
            .relays
            .iter()
            .any(|r| port_of(r) == now.port.to_string()))
        .await,
        "{:?}",
        p.saved().unwrap().mac
    );
    p.phone.close().await;
    drop(p);

    // the old relay is gone altogether
    old.stop().await;
    let (p, took, dials) = open(&state, &both, 40).await;
    println!(
        "MEASURE the Mac moved to another relay (the old one down): online after {took:?}, {dials} dials"
    );
    assert!(
        took < Duration::from_secs(2) && dials == 1,
        "{took:?}, {dials} dials"
    );
    p.phone.close().await;
}

// decision: PHN-741
/// The phone's own relay is not ready when the app opens (on the owner's phone its home relay took 4 s: iroh's first
/// network report decides it). The Mac's relay is reachable all the same, and the dial goes there at once.
#[tokio::test(flavor = "multi_thread")]
async fn the_dial_does_not_wait_for_the_phones_own_relay() {
    let relay = Relay::start(None, None).await;
    let w = World::start(Shared::Custom(vec![relay.spec()]), None, false).await;
    let p = w.paired_phone("phone").await;
    let state = p.saved().unwrap();
    p.phone.close().await;
    drop(p);
    // the phone's relays: one that never answers, so it has no home relay for as long as the test runs
    let (_silent, silent) = silent_relay().await;
    let phones = Shared::Custom(vec![ferb_link::net::RelaySpec::new(silent, None)]);
    let (p, took, dials) = open(&state, &phones, 40).await;
    println!("MEASURE the phone's own relay not ready: online after {took:?}, {dials} dials");
    assert!(
        ferb_link::net::home_relays(&p.phone.endpoint()).is_empty(),
        "the phone has no home relay in this test"
    );
    assert!(
        took < Duration::from_secs(2) && dials == 1,
        "{took:?}, {dials} dials"
    );
    assert!(p.call("ping", serde_json::json!({})).await.is_ok());
    p.phone.close().await;
}

// decision: PHN-742
/// A phone that is connected when the Mac's home relay moves is told, and remembers it for its next start.
#[tokio::test(flavor = "multi_thread")]
async fn a_connected_phone_learns_where_the_mac_moved() {
    let mut a = Relay::start(None, None).await;
    let mut b = Relay::start(None, None).await;
    let w = World::start(Shared::Custom(vec![a.spec(), b.spec()]), None, false).await;
    let p = w.paired_phone("phone").await;
    let home = ferb_link::net::home_relays(&w.bridge.endpoint());
    let (gone, stays) = if home
        .iter()
        .any(|u| port_of(u.as_str()) == a.port.to_string())
    {
        (&mut a, b.port)
    } else {
        (&mut b, a.port)
    };
    gone.stop().await;
    let t = Instant::now();
    let moved = wait(90, || {
        ferb_link::net::home_relays(&w.bridge.endpoint())
            .iter()
            .any(|u| port_of(u.as_str()) == stays.to_string())
    })
    .await;
    println!("MEASURE the Mac's home relay moved after {:?}", t.elapsed());
    assert!(moved, "the Mac never moved");
    assert!(
        wait(10, || p
            .saved()
            .unwrap()
            .mac
            .unwrap()
            .relays
            .iter()
            .any(|r| port_of(r) == stays.to_string()))
        .await,
        "the phone was not told: {:?}",
        p.saved().unwrap().mac
    );
    assert_eq!(p.phase(), "online");
    p.phone.close().await;
}

/// The phone app's process ended without a word (Android ends a closed app's process): its connection stays open on
/// the Mac until QUIC gives up on it, with a direct path to sockets that are gone. A new process of the same phone
/// must not wait for that.
// decision: PHN-740, PHN-741
#[tokio::test(flavor = "multi_thread")]
async fn a_phone_whose_last_process_died_reaches_the_mac_at_once() {
    use std::io::BufRead;
    let relay = Relay::start(None, None).await;
    let w = World::start(Shared::Custom(vec![relay.spec()]), None, false).await;
    let p = w.paired_phone("phone").await;
    let state = p.saved().unwrap();
    p.phone.close().await;
    drop(p);
    let mut took = vec![];
    for _ in 0..3 {
        // the last process: online and direct, then killed
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["child_phone", "--ignored", "--exact", "--nocapture"])
            .env(
                "FERB_DIAL_CHILD",
                serde_json::json!({"state": state, "relay": relay.url()}).to_string(),
            )
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let out = std::io::BufReader::new(child.stdout.take().unwrap());
        let line = out
            .lines()
            .map_while(Result::ok)
            .find(|l| l.starts_with("CHILD "))
            .unwrap_or_default();
        assert!(line.starts_with("CHILD direct"), "{line}");
        child.kill().unwrap();
        let _ = child.wait();
        let (p, t, dials) = open(&state, &w.shared, 40).await;
        took.push(t.as_secs_f64() * 1000.0);
        println!("MEASURE the last process was killed: online after {t:?}, {dials} dials");
        p.phone.close().await;
        drop(p);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(
        took.iter().all(|t| *t < 2000.0),
        "online after (ms): {took:?}"
    );
}

/// The process `a_phone_whose_last_process_died_reaches_the_mac_at_once` kills (not a test of its own).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "started by a_phone_whose_last_process_died_reaches_the_mac_at_once"]
async fn child_phone() {
    let Ok(arg) = std::env::var("FERB_DIAL_CHILD") else {
        return;
    };
    let arg: serde_json::Value = serde_json::from_str(&arg).unwrap();
    let state = serde_json::from_value(arg["state"].clone()).unwrap();
    let shared = Shared::Custom(vec![ferb_link::net::RelaySpec::new(
        arg["relay"].as_str().unwrap(),
        None,
    )]);
    let p = TestPhone::start("phone", &shared, false, Some(state), true).await;
    let online = p.wait_phase("online", 30).await && wait(15, || p.tier() == "direct").await;
    println!("CHILD {}", if online { p.tier() } else { p.phase() });
    tokio::time::sleep(Duration::from_secs(120)).await;
}

// decision: PHN-741
/// The phone's way to the Mac's relay opens only after 4 s (`DIAL_SLOW_S`): the dial, begun at once, is answered the
/// moment the relay is up (iroh holds the first packet while it connects, and QUIC sends it again). Measured with
/// 2 to 9 s: online within 0.25 s of the relay opening every time; the code before PHN-741 took 5.0 s at 4 s.
#[tokio::test(flavor = "multi_thread")]
async fn a_relay_that_comes_up_late_is_used_the_moment_it_is_up() {
    let relay = Relay::start(None, None).await;
    let w = World::start(Shared::Custom(vec![relay.spec()]), None, false).await;
    let p = w.paired_phone("phone").await;
    let mut state = p.saved().unwrap();
    p.phone.close().await;
    drop(p);
    let after: u64 = std::env::var("DIAL_SLOW_S")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let opens = tokio::time::Instant::now() + Duration::from_secs(after);
    let (_way, slow) = slow_way_to(&relay, opens).await;
    state.mac.as_mut().unwrap().relays = vec![slow.clone()];
    let phones = Shared::Custom(vec![ferb_link::net::RelaySpec::new(slow, None)]);
    let (p, took, dials) = open(&state, &phones, 40).await;
    println!(
        "MEASURE the way to the relay opened after {after} s: online after {took:?}, {dials} dials"
    );
    assert!(
        took < Duration::from_millis(after * 1000 + 1500) && dials == 1,
        "{took:?}, {dials} dials"
    );
    p.phone.close().await;
}
