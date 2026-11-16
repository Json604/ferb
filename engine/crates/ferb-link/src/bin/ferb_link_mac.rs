//! The `ferb-link` program (FERB.app's `Contents/Helpers/ferb-link`, signed `com.ferb.link`; kb PHN-441).
//!
//!   ferb-link run --daemon SOCKET --state DIR [--shared-relay URL]... [--no-portmap]
//!                                       FERB's helper, started by FERB's daemon (keys on standard input)
//!   ferb-link phone --state FILE [--shared-relay URL]... [--name NAME]    a phone simulator on this Mac (lab tool)
//!   ferb-link relay-test URL [TOKEN]                                      what "Test" in the relay setting says
//!
//! `--shared-relay` replaces n0's public relays (for local runs against `dev-relay`); `--force-relay` (the simulator)
//! removes the direct paths so every byte crosses a relay. The lab's `serve` (the bridge with a terminal prompt and an
//! admin socket) is gone since S4: pairing's Yes, removal and the relay setting are FERB.app's, through the daemon.

use ferb_link::{
    bridge::{Bridge, BridgeConfig},
    client::{Phone, PhoneConfig, PhoneState},
    errors::LinkError,
    net::{RelaySpec, Shared},
    secrets::Keys,
};
use p256::ecdsa::{SigningKey, signature::Signer};
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::io::{AsyncBufReadExt, BufReader};

struct Args(Vec<String>);
impl Args {
    fn one(&self, name: &str) -> Option<String> {
        self.0
            .iter()
            .position(|a| a == name)
            .and_then(|i| self.0.get(i + 1))
            .cloned()
    }
    fn all(&self, name: &str) -> Vec<String> {
        self.0
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == name)
            .filter_map(|(i, _)| self.0.get(i + 1))
            .cloned()
            .collect()
    }
    fn has(&self, name: &str) -> bool {
        self.0.iter().any(|a| a == name)
    }
    fn shared(&self) -> Shared {
        let urls = self.all("--shared-relay");
        if urls.is_empty() {
            Shared::N0
        } else {
            Shared::Custom(
                urls.into_iter()
                    .map(|url| RelaySpec::new(url, None))
                    .collect(),
            )
        }
    }
}

fn show(r: Result<Value, LinkError>) {
    match r {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
        Err(e) => println!(
            "REFUSED [{}] {}{}",
            e.code,
            e.message,
            e.allow.map(|a| format!("\n  -> {a}")).unwrap_or_default()
        ),
    }
}

#[tokio::main]
pub async fn main() {
    let args = Args(std::env::args().skip(1).collect());
    let out = match args.0.first().map(String::as_str) {
        Some("run") => run(&args).await,
        Some("phone") => phone(&args).await,
        Some("relay-test") => relay_test(&args).await,
        _ => {
            eprintln!(
                "usage: ferb-link run --daemon SOCKET --state DIR | phone --state FILE | relay-test URL [TOKEN]"
            );
            std::process::exit(2);
        }
    };
    if let Err(e) = out {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

async fn relay_test(args: &Args) -> Result<(), LinkError> {
    let spec = RelaySpec::from_text(
        args.0.get(1).map(String::as_str).unwrap_or_default(),
        args.0.get(2).map(String::as_str),
    )?;
    let mut key = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut key);
    let r = ferb_link::relaytest::test(&spec, iroh::SecretKey::from_bytes(&key)).await;
    show(r.map(|ok| serde_json::to_value(ok).unwrap()));
    Ok(())
}

/// FERB's helper (kb PHN-441): started by FERB's daemon with its socket and this program's own state folder; the keys
/// arrive as the first line of standard input (kb PHN-143), never in argv or the environment. It runs until standard
/// input closes (the daemon stopped, or crashed: either way this program goes with it). It holds no permission and
/// reaches FERB only through the daemon's socket, as the role the daemon gives it by its code signature (kb PHN-468).
/// What it says goes to standard error, which the daemon writes to its log redacted; never a key or token.
async fn run(args: &Args) -> Result<(), LinkError> {
    let state = PathBuf::from(
        args.one("--state")
            .ok_or_else(|| LinkError::new("bad_request", "ferb-link run needs --state DIR"))?,
    );
    let daemon = PathBuf::from(
        args.one("--daemon")
            .ok_or_else(|| LinkError::new("bad_request", "ferb-link run needs --daemon SOCKET"))?,
    );
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut first = tokio::time::timeout(std::time::Duration::from_secs(30), lines.next_line())
        .await
        .map_err(|_| LinkError::new("ferb_error", "FERB handed over no keys in 30 s"))?
        .map_err(|e| LinkError::new("ferb_error", format!("reading the keys: {e}")))?
        .ok_or_else(|| LinkError::new("ferb_error", "FERB handed over no keys"))?;
    let given = ferb_link::secrets::parse_given(&first);
    // SAFETY: zeroes are valid UTF-8; the line held the keys and its bytes should not linger
    unsafe { first.as_bytes_mut().fill(0) };
    let bridge = Bridge::start(BridgeConfig {
        state_dir: state,
        daemon_socket: daemon,
        shared: args.shared(),
        roots: None,
        force_relay: false,
        keys: Keys::Given(given?),
        accept_state: false,
        portmapper: !args.has("--no-portmap"),
        loopback_only: false,
    })
    .await?;
    eprintln!(
        "ferb-link {} (link {}, level {}): serving phones as {}",
        ferb_link::build(),
        ferb_link::VERSION,
        ferb_link::LEVEL,
        bridge.id()
    );
    let mut events = bridge.admin_events();
    let names = bridge.clone();
    tokio::spawn(async move {
        while let Ok(e) = events.recv().await {
            if let Some(line) = describe(&names, &e) {
                eprintln!("ferb-link: {line}");
            }
        }
    });
    // standard input stays open while the daemon runs; its end is the signal to stop
    while let Ok(Some(_)) = lines.next_line().await {}
    eprintln!("ferb-link: FERB's daemon closed the pipe; stopping");
    bridge.close().await;
    Ok(())
}

/// One plain line for what just happened, for FERB's log. None: not worth a line. Never a code, a key or a token.
fn describe(bridge: &Bridge, e: &Value) -> Option<String> {
    let name = |id: &Value| {
        bridge
            .devices()
            .iter()
            .find(|d| Some(d.id.as_str()) == id.as_str())
            .map(|d| d.name.clone())
            .unwrap_or_else(|| "a phone".into())
    };
    let tier = |t: &Value| match t.as_str() {
        Some("direct") => "a direct connection",
        Some("own_relay") => "the owner's relay",
        _ => "the shared relay",
    };
    Some(match e["event"].as_str()? {
        "pair_request" => "a phone proved the pairing code; FERB asks the owner".into(),
        "pair_ended" if e["paired"] == true => "a phone was paired".into(),
        "pair_ended" => "a pairing was not completed".into(),
        "pair_wrong_secret" => format!(
            "someone tried to pair with a wrong code{}",
            if e["window_closed"] == true {
                "; the code is cancelled"
            } else {
                ""
            }
        ),
        "phone_online" if e["moved"] == true => return None,
        "phone_online" => format!("{} connected through {}", name(&e["id"]), tier(&e["tier"])),
        "phone_tier" => format!("{} is now on {}", name(&e["id"]), tier(&e["tier"])),
        "phone_offline" => format!("{} disconnected", name(&e["id"])),
        "device_revoked" => "a removed phone was cut off".into(),
        "relay_rate_limited" => "the shared relay asked FERB to slow down".into(),
        "relay" => format!(
            "relay: {}",
            e["relay"]["state"]["state"].as_str().unwrap_or("shared")
        ),
        _ => return None,
    })
}

/// The simulator keeps its state (and a software stand-in for the phone's lock-bound key) in one 0600 file.
/// A real phone keeps the state encrypted and the key in its hardware keystore.
async fn phone(args: &Args) -> Result<(), LinkError> {
    use base64::Engine;
    use std::os::unix::fs::OpenOptionsExt;
    let b64 = base64::engine::general_purpose::STANDARD;
    let path = PathBuf::from(
        args.one("--state")
            .unwrap_or_else(|| "state/phone.json".into()),
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let saved: Value = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(json!({}));
    let unlock = saved["unlock_key"]
        .as_str()
        .and_then(|k| b64.decode(k).ok())
        .and_then(|der| SigningKey::from_pkcs8_der(&der).ok())
        .unwrap_or_else(|| SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng));
    let unlock_der = b64.encode(unlock.to_pkcs8_der().unwrap().as_bytes());
    let unlock_pub = (!args.has("--no-lock")).then(|| {
        b64.encode(
            unlock
                .verifying_key()
                .to_public_key_der()
                .unwrap()
                .as_bytes(),
        )
    });
    let state: Option<PhoneState> = serde_json::from_value(saved["link"].clone()).ok();
    let (phone, mut events) = Phone::start(PhoneConfig {
        state,
        name: args.one("--name").unwrap_or_else(|| "Simulator".into()),
        app: "simulator 0.1".into(),
        unlock_pub,
        device_pub: None,
        strong_pub: None,
        device_sign: None,
        shared: args.shared(),
        force_relay: args.has("--force-relay"),
        ..Default::default()
    })
    .await?;
    let approvals: Arc<Mutex<Vec<Value>>> = Default::default();
    {
        let (approvals, path, unlock_der) = (approvals.clone(), path.clone(), unlock_der.clone());
        tokio::spawn(async move {
            while let Some(e) = events.recv().await {
                match e["type"].as_str() {
                    Some("persist") => {
                        let body = serde_json::to_vec_pretty(
                            &json!({"link": e["state"], "unlock_key": unlock_der}),
                        )
                        .unwrap();
                        if let Ok(mut f) = std::fs::OpenOptions::new()
                            .write(true)
                            .create(true)
                            .truncate(true)
                            .mode(0o600)
                            .open(&path)
                        {
                            let _ = std::io::Write::write_all(&mut f, &body);
                        }
                    }
                    Some("state") => println!("[link] {}", e),
                    Some("status") => {
                        let st = &e["status"];
                        *approvals.lock().unwrap() =
                            st["approvals"].as_array().cloned().unwrap_or_default();
                        println!(
                            "[status] running: {}  questions: {}  answers: {}",
                            st["running"].as_array().map(|a| a.len()).unwrap_or(0),
                            st["approvals"].as_array().map(|a| a.len()).unwrap_or(0),
                            st["results"].as_array().map(|a| a.len()).unwrap_or(0)
                        );
                        for a in st["approvals"].as_array().into_iter().flatten() {
                            println!(
                                "  QUESTION {}: {}  {}  [{}]",
                                a["approval_id"],
                                a["reason"],
                                a["exact"],
                                if a["phone"]["can_answer"] == json!(false) {
                                    "cannot be answered from a phone"
                                } else if a["phone"]["needs_unlock"] == json!(true) {
                                    "needs unlock"
                                } else {
                                    "plain"
                                }
                            );
                        }
                    }
                    _ => println!("[event] {e}"),
                }
            }
        });
    }
    println!("phone simulator, link id {}", phone.id());
    println!(
        "commands: pair URL | unlock | network [KIND] | send TEXT | get PATH | status | yes ID | no ID | answer ID TEXT | stop TASK | stopall | preview TASK [live] | file TASK INDEX | put PATH | link | call METHOD JSON | bg | fg | retry | forget | quit"
    );
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let (cmd, rest) = line.trim().split_once(' ').unwrap_or((line.trim(), ""));
        let w: Vec<&str> = rest.split_whitespace().collect();
        let answer = |method: &str, params: Value| {
            let ap = approvals
                .lock()
                .unwrap()
                .iter()
                .find(|a| a["approval_id"] == params["approval_id"])
                .cloned();
            let proof = ap
                .filter(|a| a["phone"]["needs_unlock"] == json!(true))
                .map(|a| {
                    println!("(simulated unlock: the software key signs this answer)");
                    let payload = Phone::answer_payload(
                        params["approval_id"].as_str().unwrap_or_default(),
                        a["phone"]["challenge"].as_str().unwrap_or_default(),
                        method,
                        &params,
                    );
                    let sig: p256::ecdsa::Signature = unlock.sign(payload.as_bytes());
                    json!({"payload": payload, "sig": b64.encode(sig.to_der().as_bytes())})
                });
            (method.to_string(), params, proof)
        };
        match (cmd, w.as_slice()) {
            ("", _) => {}
            ("pair", [url]) => show(phone.pair(url).map(|_| json!({"ok": "pairing started"}))),
            ("send", _) => show(phone.call("chat.send", json!({"text": rest}), None, |ev| println!("  [{}] {}", ev["type"].as_str().unwrap_or("event"), ev["text"].as_str().unwrap_or(&ev.to_string()))).await),
            ("status", _) => show(phone.call("chat.status", json!({}), None, |_| {}).await),
            ("yes", [id]) | ("no", [id]) => {
                let (m, p, proof) = answer("chat.approve", json!({"approval_id": id, "granted": cmd == "yes"}));
                show(phone.call(&m, p, proof, |_| {}).await)
            }
            ("answer", [id, ..]) => {
                let (m, p, proof) = answer("chat.answer", json!({"approval_id": id, "text": rest.split_once(' ').map(|x| x.1).unwrap_or("")}));
                show(phone.call(&m, p, proof, |_| {}).await)
            }
            ("stop", [task]) => show(phone.call("task.stop", json!({"task_ids": [task]}), None, |_| {}).await),
            ("stopall", _) => show(phone.call("stop.engage", json!({}), None, |_| {}).await),
            ("preview", [task, more @ ..]) => show(phone.get(json!({"what": "preview", "task_id": task, "live": more.first() == Some(&"live")}), |_, _| {}).await.map(|(m, b)| json!({"reply": m, "bytes": b.len()}))),
            ("file", [task, index]) => show(phone.get(json!({"what": "file", "task_id": task, "index": index.parse::<u64>().unwrap_or(0)}), |_, _| {}).await.map(|(m, b)| json!({"reply": m, "bytes": b.len()}))),
            ("put", [p]) => match std::fs::read(p) {
                Ok(bytes) => show(phone.put(p.rsplit('/').next().unwrap_or("file"), &bytes, |_, _| {}).await),
                Err(e) => println!("{e}"),
            },
            ("link", _) => show(phone.call("link.status", json!({}), None, |_| {}).await),
            // as the app does when Android says the phone is on another network: begins the start-over schedule
            ("network", _) => {
                phone.network_changed_to(w.first().copied()).await;
                println!("told the link the network changed; start-overs so far: {} (not through: {}); the last: {}", phone.fresh_starts(), phone.failed_starts(), phone.last_start());
            }
            // FERB opens for a phone only after its unlock: the simulator's software key signs it
            ("unlock", _) => match phone.challenge().await {
                Ok(ch) => {
                    let payload = Phone::call_payload(&ch, "link.unlock", &json!({}));
                    let sig: p256::ecdsa::Signature = unlock.sign(payload.as_bytes());
                    show(phone.call("link.unlock", json!({}), Some(json!({"payload": payload, "sig": b64.encode(sig.to_der().as_bytes())})), |_| {}).await)
                }
                Err(e) => show(Err(e)),
            },
            ("get", [path]) => {
                let t = std::time::Instant::now();
                show(phone.get_with(json!({"what": "path", "path": path}), |_| {}, |_, _| {}).await.map(|m| json!({"reply": m, "seconds": t.elapsed().as_secs_f64()})))
            }
            ("call", [method, ..]) => show(phone.call(method, serde_json::from_str(rest.split_once(' ').map(|x| x.1).unwrap_or("{}")).unwrap_or(json!({})), None, |ev| println!("  {ev}")).await),
            ("bg", _) => phone.set_active(false),
            ("fg", _) => phone.set_active(true),
            ("retry", _) => phone.retry_now(),
            ("forget", _) => phone.forget(),
            ("quit", _) => break,
            _ => println!("?"),
        }
    }
    phone.close().await;
    Ok(())
}
