#![cfg(target_os = "macos")]
//! What travels in and out is encrypted end to end (the owner, kb PHN-453: "jsut make sure info going in and out is
//! encrypted"). A relay forwards QUIC packets between the two endpoint keys and sees only ciphertext: these tests
//! put a recorder in front of a relay, the public one and the owner's, send known markers through the link (a chat
//! message, a hidden answer, a file both ways, the pairing), and look for them in every byte the relay was given.
//! A peer with another key cannot complete a session, and no path of the link leaves QUIC (or, for the relay's own
//! setup, TLS).
mod common;
use common::*;
use ferb_link::{errors, net::Shared};
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn pump(
    mut from: impl AsyncReadExt + Unpin,
    mut to: impl AsyncWriteExt + Unpin,
    all: Arc<Mutex<Vec<u8>>>,
) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match from.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                all.lock().unwrap().extend_from_slice(&buf[..n]);
                if to.write_all(&buf[..n]).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// A TCP recorder in front of a relay: every byte either side sends through it is kept.
struct Recorder {
    port: u16,
    bytes: Arc<Mutex<Vec<u8>>>,
}
impl Recorder {
    async fn start(target: u16) -> Self {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let bytes: Arc<Mutex<Vec<u8>>> = Default::default();
        let all = bytes.clone();
        tokio::spawn(async move {
            while let Ok((inside, _)) = listener.accept().await {
                let Ok(outside) = tokio::net::TcpStream::connect(("127.0.0.1", target)).await
                else {
                    continue;
                };
                let (ir, iw) = inside.into_split();
                let (or, ow) = outside.into_split();
                tokio::spawn(pump(ir, ow, all.clone()));
                tokio::spawn(pump(or, iw, all.clone()));
            }
        });
        Self { port, bytes }
    }
    fn spec(&self) -> ferb_link::net::RelaySpec {
        ferb_link::net::RelaySpec::new(format!("http://localhost:{}", self.port), None)
    }
    fn len(&self) -> usize {
        self.bytes.lock().unwrap().len()
    }
    /// Whether these bytes appear anywhere in what the relay was given.
    fn saw(&self, needle: &[u8]) -> bool {
        self.bytes
            .lock()
            .unwrap()
            .windows(needle.len())
            .any(|w| w == needle)
    }
}

const CHAT: &str = "MARKER-chat-7Q2xv9-plaintext";
const SECRET: &str = "MARKER-secret-K8pw3-hunter2";
const FILE: &str = "MARKER-file-chunk-Zr41";
const UPLOAD: &str = "MARKER-upload-chunk-Yb27";

/// A hidden answer through the sealed path (as the app gives one), with its marker as the value.
async fn hidden_answer(w: &World, p: &TestPhone) {
    let id = w.standin.cli("standin.ask", json!({"kind": "owner_secret", "capability": "secret.fill", "reason": "Password", "options": []})).await["approval_id"].as_str().unwrap().to_string();
    assert!(
        wait(10, || p.approval(&id).is_some()).await,
        "question not pushed"
    );
    let challenge = p.approval(&id).unwrap()["phone"]["challenge"]
        .as_str()
        .unwrap()
        .to_string();
    let proof = p.sealed_proof(&id, &challenge, SECRET);
    p.phone
        .call(
            "chat.answer",
            json!({"approval_id": id, "text": SECRET}),
            Some(proof),
            |_| {},
        )
        .await
        .unwrap();
    assert_eq!(
        w.standin
            .cli(
                "standin.fill_matches",
                json!({"approval_id": id, "value": SECRET})
            )
            .await["matches"],
        true,
        "the answer arrived"
    );
}

fn assert_never_seen(rec: &Recorder, what: &[(&str, &[u8])]) {
    for (name, needle) in what {
        assert!(!rec.saw(needle), "the relay saw {name} in the clear");
    }
}

// decision: PHN-453, PHN-463, PHN-118, PHN-119, PHN-202
#[tokio::test(flavor = "multi_thread")]
async fn the_owners_relay_forwards_only_ciphertext() {
    let public = Relay::start(None, None).await;
    let relay = Relay::start(None, None).await;
    let rec = Recorder::start(relay.port).await;
    // every byte through the owner's relay, and through the recorder in front of it
    let w = World::start(Shared::Custom(vec![public.spec()]), Some(rec.spec()), true).await;
    let mut admin = w.bridge.admin_events();
    let qr = w.bridge.pair_start().unwrap();
    let p = w.phone("phone").await;
    p.phone.pair(qr["code"].as_str().unwrap()).unwrap();
    let invite = ferb_link::pairing::Invite::parse(qr["code"].as_str().unwrap()).unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        while admin.recv().await.unwrap()["event"] != "pair_request" {}
    })
    .await
    .unwrap();
    w.decide(true).unwrap();
    assert!(p.wait_phase("online", 20).await, "{}", p.phone.phase());
    assert_eq!(p.tier(), "own_relay");
    p.unlock().await.unwrap();

    // a chat message and FERB's reply
    let mut said = vec![];
    p.phone
        .call("chat.send", json!({"text": CHAT}), None, |e| said.push(e))
        .await
        .unwrap();
    assert!(
        said.iter()
            .any(|e| e["text"].as_str().is_some_and(|t| t.contains(CHAT))),
        "the reply came back: {said:?}"
    );
    // a hidden answer
    hidden_answer(&w, &p).await;
    // a file from the Mac and one to it, each larger than a chunk, the marker throughout
    let file = w.dir.join("marked.txt");
    std::fs::write(&file, FILE.repeat(20_000)).unwrap();
    let (meta, got) = p
        .phone
        .get(json!({"what": "path", "path": file}), |_, _| {})
        .await
        .unwrap();
    assert_eq!(
        (got.len(), meta["tier"].as_str()),
        (FILE.len() * 20_000, Some("own_relay"))
    );
    let up = p
        .phone
        .put("up.txt", UPLOAD.repeat(20_000).as_bytes(), |_, _| {})
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(up["saved"].as_str().unwrap()).unwrap().len(),
        UPLOAD.len() * 20_000
    );

    // the relay carried all of it, and none of it can be read there
    assert!(
        rec.len() > 2 * FILE.len() * 20_000,
        "the recorder did not carry the session ({} bytes)",
        rec.len()
    );
    // (the recorder does see what is in the clear: the relay's own HTTP upgrade, plain on this test's localhost)
    assert!(
        rec.saw(b"HTTP/1.1"),
        "the recorder recorded nothing readable at all"
    );
    let secret_b64 = {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(invite.secret)
    };
    assert_never_seen(
        &rec,
        &[
            ("the chat message", CHAT.as_bytes()),
            ("the hidden answer", SECRET.as_bytes()),
            ("a file chunk from the Mac", FILE.as_bytes()),
            ("a file chunk to the Mac", UPLOAD.as_bytes()),
            ("the pairing code's one-time secret", &invite.secret),
            (
                "the pairing code's one-time secret (as text)",
                secret_b64.as_bytes(),
            ),
            ("the phone's name", b"\"name\":\"phone\""),
        ],
    );
    println!(
        "MEASURE bytes the owner's relay forwarded, none readable: {}",
        rec.len()
    );
}

// decision: PHN-453, PHN-463, PHN-118, PHN-119, PHN-301
#[tokio::test(flavor = "multi_thread")]
async fn the_public_relay_forwards_only_ciphertext() {
    let relay = Relay::start(None, None).await;
    let rec = Recorder::start(relay.port).await;
    let w = World::start(Shared::Custom(vec![rec.spec()]), None, true).await;
    let p = w.paired_phone("phone").await;
    assert_eq!(p.tier(), "shared_relay");
    let mut said = vec![];
    p.phone
        .call("chat.send", json!({"text": CHAT}), None, |e| said.push(e))
        .await
        .unwrap();
    assert!(
        said.iter()
            .any(|e| e["text"].as_str().is_some_and(|t| t.contains(CHAT)))
    );
    hidden_answer(&w, &p).await;
    assert!(
        rec.len() > 10_000,
        "the recorder did not carry the session ({} bytes)",
        rec.len()
    );
    assert!(
        rec.saw(b"HTTP/1.1"),
        "the recorder recorded nothing readable at all"
    );
    assert_never_seen(
        &rec,
        &[
            ("the chat message", CHAT.as_bytes()),
            ("the hidden answer", SECRET.as_bytes()),
            ("the phone's name", b"\"name\":\"phone\""),
        ],
    );
}

// decision: PHN-453, PHN-119, PHN-102
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_with_another_key_cannot_complete_a_session() {
    use ferb_link::net::{ALPN, NetConfig, bind, dial_addr};
    let relay = Relay::start(None, None).await;
    let w = World::start(Shared::Custom(vec![relay.spec()]), None, false).await;
    let p = w.paired_phone("phone").await;
    let mac_id: iroh::EndpointId = w.bridge.id().parse().unwrap();

    // 1. an impostor of the Mac: it answers at an address, but the handshake proves the key, not the address
    let impostor = bind(&NetConfig {
        accept: true,
        portmapper: false,
        loopback_only: true,
        ..NetConfig::new(
            iroh::SecretKey::from_bytes(&[11; 32]),
            None,
            Shared::Custom(vec![]),
        )
    })
    .await
    .unwrap();
    let at = *impostor
        .bound_sockets()
        .iter()
        .find(|a| a.is_ipv4())
        .unwrap();
    let at = std::net::SocketAddr::from(([127, 0, 0, 1], at.port()));
    tokio::spawn({
        let impostor = impostor.clone();
        async move {
            while let Some(i) = impostor.accept().await {
                let _ = i.await;
            }
        }
    });
    let client = bind(&NetConfig {
        portmapper: false,
        loopback_only: true,
        ..NetConfig::new(
            iroh::SecretKey::from_bytes(&[12; 32]),
            None,
            Shared::Custom(vec![]),
        )
    })
    .await
    .unwrap();
    let to_mac_at_impostor = iroh::EndpointAddr::new(mac_id).with_ip_addr(at);
    let r = tokio::time::timeout(
        Duration::from_secs(10),
        client.connect(to_mac_at_impostor, ALPN),
    )
    .await;
    assert!(
        !matches!(r, Ok(Ok(_))),
        "a peer without the Mac's key completed a session as the Mac"
    );

    // 2. a stranger at the Mac (its own key, the paired phone's name in its hello): turned away, no welcome
    let stranger = bind(&NetConfig {
        force_relay: true,
        portmapper: false,
        loopback_only: true,
        ..NetConfig::new(
            iroh::SecretKey::from_bytes(&[13; 32]),
            None,
            w.shared.clone(),
        )
    })
    .await
    .unwrap();
    ferb_link::net::wait_home(&stranger, Duration::from_secs(10)).await;
    let conn = stranger
        .connect(dial_addr(mac_id, &[relay.url().parse().unwrap()]), ALPN)
        .await
        .unwrap();
    let opened = async {
        let (mut tx, mut rx) = conn.open_bi().await.ok()?;
        ferb_link::wire::write_json(
            &mut tx,
            ferb_link::wire::K_HELLO,
            &json!({"v": 2, "min_v": 2, "name": "phone"}),
        )
        .await
        .ok()?;
        ferb_link::wire::read_frame(&mut rx, 65536, Duration::from_secs(5), false)
            .await
            .ok()
            .flatten()
    };
    let frame = tokio::time::timeout(Duration::from_secs(10), opened)
        .await
        .ok()
        .flatten();
    assert!(
        !frame
            .as_ref()
            .is_some_and(|(k, _)| *k == ferb_link::wire::K_WELCOME),
        "a stranger was welcomed"
    );
    let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
    assert!(
        matches!(conn.close_reason(), Some(iroh::endpoint::ConnectionError::ApplicationClosed(c)) if c.reason.as_ref() == errors::NOT_PAIRED.as_bytes()),
        "{:?}",
        conn.close_reason()
    );
    // the real phone was never disturbed
    assert!(p.call("ping", json!({})).await.is_ok());
}

// decision: PHN-453, PHN-204, PHN-118
#[test]
fn no_path_of_the_link_leaves_quic_or_tls() {
    use ferb_link::net::parse_relay_url;
    // a relay is reached over https only; plain http only to this very Mac (a relay in dev mode, the tests)
    for bad in [
        "http://relay.example.com",
        "http://192.0.2.7:7443",
        "ws://relay.example.com",
        "relay.example.com",
        "ftp://x",
    ] {
        assert_eq!(
            parse_relay_url(bad).unwrap_err().code,
            errors::OWN_RELAY_URL,
            "{bad}"
        );
    }
    for good in [
        "https://relay.example.com",
        "http://localhost:3340",
        "http://127.0.0.1:3340",
    ] {
        assert!(parse_relay_url(good).is_ok(), "{good}");
    }
    // the source: one endpoint builder, iroh's Minimal preset (no address lookup: nothing published, kb PHN-204), and
    // no socket of its own beside it except the relay Test's (TCP to the relay, then TLS; plain only to localhost)
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut builders = 0;
    for f in std::fs::read_dir(&src)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
    {
        let text = std::fs::read_to_string(&f).unwrap();
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        builders += text.matches("Endpoint::builder(").count();
        assert!(
            !text.contains("presets::N0")
                && !text.contains("address_lookup")
                && !text.contains("UdpSocket"),
            "{name}"
        );
        if name != "relaytest.rs" {
            assert!(
                !text.contains("TcpStream") && !text.contains("TcpListener"),
                "{name} opens a TCP socket of its own"
            );
        }
        if name == "net.rs" {
            assert!(
                text.contains("Endpoint::builder(presets::Minimal)"),
                "net.rs builds its endpoint from the Minimal preset"
            );
        }
    }
    assert_eq!(
        builders, 1,
        "one endpoint builder (net::bind) for both sides"
    );
}
