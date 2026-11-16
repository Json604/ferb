//! Stage 3 S3 (T-1202): a phone session's entry on the local socket. Only the phone link's helper (its code signature,
//! `set_is_link`) may say role `link` or `phone`; FERB.app, a command line, a model's tools and the extension can't open a
//! phone session, and the helper can't reach FERB.app's methods. On a phone session the gate decides each call, sees the
//! call's proof, and shapes what the phone gets; the handler sees the phone beside the role.

mod common;

use std::sync::{Arc, Mutex};

use common::{new_server, ok, Raw};
use ferb_rpc::phone_gate::{self, PhoneGate, PhoneSession, Refusal};
use ferb_rpc::server::HandlerResult;
use ferb_rpc::wire::{self, Params};
use ferb_rpc::{GrantSpec, Identity, Session};
use serde_json::{json, Value};

/// A gate with one paired phone "p1"; it refuses `settings.set` without a proof and records what it saw.
#[derive(Clone, Default)]
struct StubGate {
    seen: Arc<Mutex<Vec<String>>>,
}

impl PhoneGate for StubGate {
    fn open(&self, device: &str, session: &str) -> Result<(), Refusal> {
        self.seen
            .lock()
            .unwrap()
            .push(format!("open {device} {session}"));
        if device == "p1" {
            Ok(())
        } else {
            Err(Refusal::new(
                phone_gate::UNKNOWN_PHONE,
                "This phone is not one of FERB's paired phones.",
            ))
        }
    }
    fn admit(
        &self,
        phone: &PhoneSession,
        method: &str,
        params: &mut Params,
    ) -> Result<(), Refusal> {
        self.seen.lock().unwrap().push(format!(
            "admit {method} proof={}",
            phone
                .proof
                .as_ref()
                .map_or("none".into(), |p| Value::Object(p.clone()).to_string())
        ));
        if method == "settings.set" && phone.proof.is_none() {
            return Err(Refusal::new(
                phone_gate::UNLOCK_REQUIRED,
                "Unlock the phone to do this.",
            ));
        }
        params.insert("client_id".into(), json!(format!("phone:{}", phone.device)));
        Ok(())
    }
    fn reply(&self, _phone: &PhoneSession, method: &str, result: HandlerResult) -> HandlerResult {
        result.map(|mut r| {
            r.insert("gate".into(), json!(method));
            r
        })
    }
}

fn server_with(dir: &std::path::Path, link: bool) -> (ferb_rpc::Server, StubGate) {
    let (server, _) = new_server(dir);
    server.set_is_link(move |_| link);
    let gate = StubGate::default();
    server.set_phone_gate(gate.clone());
    // what a handler sees of the session
    server.register_unary("settings.get", |s: Session, params: Params| async move {
        ok(json!({"role": s.role.as_str(), "phone": s.phone.map(|p| p.device), "params": params}))
    });
    server.register_unary("settings.set", |_: Session, _: Params| async move {
        ok(json!({"set": true}))
    });
    server.register_unary("link.devices", |s: Session, _: Params| async move {
        ok(json!({"role": s.role.as_str()}))
    });
    server.register_unary("phone.challenge", |s: Session, _: Params| async move {
        ok(json!({"proof": s.phone.and_then(|p| p.proof).map(Value::Object)}))
    });
    (server, gate)
}

/// Nobody but the helper: a command line (or FERB.app) naming "phone" or "link" gets Python's refusal of an unknown
/// role, and none of the local roles reaches the helper's calls or a phone session's own.
// decision: PHN-441
// decision: PHN-124
// decision: GATE-612
// decision: PHN-468
#[tokio::test]
async fn only_the_link_helper_opens_a_phone_session() {
    let dir = tempfile::tempdir().unwrap();
    let (server, gate) = server_with(dir.path(), false);
    server.set_is_app(|_| true); // even FERB.app itself
    server.start().await.unwrap();
    for role in ["phone", "link"] {
        let mut raw = Raw::connect(server.path()).await;
        let refused = raw
            .call(
                "auth.hello",
                json!({"role": role, "device": "p1", "session": "s"}),
            )
            .await
            .unwrap_err();
        assert_eq!(
            refused,
            (
                wire::INVALID_REQUEST,
                "Input should be 'cli', 'mcp_shim' or 'native_host'".to_owned()
            )
        );
    }
    assert!(
        gate.seen.lock().unwrap().is_empty(),
        "the gate was never asked"
    );
    let mut cli = Raw::cli(server.path()).await;
    for method in [
        "link.devices",
        "link.pair_request",
        "phone.challenge",
        "phone.unlock",
    ] {
        let (code, _) = cli.call(method, json!({})).await.unwrap_err();
        assert_eq!(code, wire::UNAUTHORIZED, "{method}");
    }
    // a model's tools
    let token = server
        .tokens()
        .issue("t", "w", GrantSpec::new(Identity::Owner, 60.0));
    let mut shim = Raw::hello(server.path(), json!({"role": "mcp_shim", "token": token})).await;
    for method in ["link.devices", "phone.challenge", "phone.list"] {
        let (code, _) = shim.call(method, json!({})).await.unwrap_err();
        assert_eq!(code, wire::UNAUTHORIZED, "{method}");
    }
    server.close().await;
}

/// The helper as role `link` reaches only the link's own calls: never FERB.app's (`app_only` or not).
// decision: PHN-441
// decision: PHN-106
// decision: PHN-468
#[tokio::test]
async fn the_helper_reaches_only_the_link_s_own_calls() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = server_with(dir.path(), true);
    server.start().await.unwrap();
    let mut link = Raw::hello(server.path(), json!({"role": "link"})).await;
    assert_eq!(
        link.call("link.devices", json!({})).await.unwrap()["role"],
        "link"
    );
    for method in [
        "chat.send",
        "chat.approve",
        "settings.get",
        "phone.list",
        "phone.remove",
        "data.tables",
        "phone.challenge",
    ] {
        let (code, why) = link.call(method, json!({})).await.unwrap_err();
        assert_eq!(code, wire::UNAUTHORIZED, "{method}");
        assert_eq!(why, format!("{method} not allowed for link"));
    }
    server.close().await;
}

/// A phone session: only a paired phone opens one; the gate sees each call's proof and may refuse with its own code
/// or adjust the parameters; the handler sees the phone; a `never` method is refused with its reason.
// decision: PHN-112
// decision: PHN-124
// decision: PHN-468
#[tokio::test]
async fn a_phone_session_is_decided_by_the_gate() {
    let dir = tempfile::tempdir().unwrap();
    let (server, gate) = server_with(dir.path(), true);
    server.start().await.unwrap();
    let mut stranger = Raw::connect(server.path()).await;
    assert_eq!(
        stranger
            .call(
                "auth.hello",
                json!({"role": "phone", "device": "p2", "session": "s"})
            )
            .await
            .unwrap_err()
            .0,
        phone_gate::UNKNOWN_PHONE
    );
    let mut nameless = Raw::connect(server.path()).await;
    assert_eq!(
        nameless
            .call("auth.hello", json!({"role": "phone"}))
            .await
            .unwrap_err()
            .0,
        wire::INVALID_REQUEST
    );
    let mut phone = Raw::hello(
        server.path(),
        json!({"role": "phone", "device": "p1", "session": "s1"}),
    )
    .await;
    let got = phone.call("settings.get", json!({"k": 1})).await.unwrap();
    assert_eq!(got["role"], "phone");
    assert_eq!(got["phone"], "p1");
    assert_eq!(
        got["params"]["client_id"], "phone:p1",
        "the gate's adjustment reached the handler"
    );
    assert_eq!(
        got["gate"], "settings.get",
        "the gate's reply shaped the result"
    );
    let (code, _) = phone.call("settings.set", json!({})).await.unwrap_err();
    assert_eq!(code, phone_gate::UNLOCK_REQUIRED);
    // the proof rides beside params and reaches the gate and the handler untouched
    phone
        .send(b"{\"jsonrpc\": \"2.0\", \"id\": 50, \"method\": \"phone.challenge\", \"params\": {}, \"proof\": {\"payload\": \"x\", \"sig\": \"y\"}}\n")
        .await
        .unwrap();
    let reply: Value = serde_json::from_slice(&phone.line().await).unwrap();
    assert_eq!(
        reply["result"]["proof"],
        json!({"payload": "x", "sig": "y"})
    );
    // the next call has none: a proof is for its own call only
    let next = phone.call("phone.challenge", json!({})).await.unwrap();
    assert_eq!(next["proof"], Value::Null);
    let (code, why) = phone
        .call("notifications.subscribe", json!({}))
        .await
        .unwrap_err();
    assert_eq!(code, wire::UNAUTHORIZED);
    assert!(
        why.starts_with("notifications.subscribe cannot be done from a phone ("),
        "{why}"
    );
    let (code, _) = phone.call("link.devices", json!({})).await.unwrap_err();
    assert_eq!(code, wire::UNAUTHORIZED);
    let seen = gate.seen.lock().unwrap().clone();
    assert!(seen.contains(&"open p1 s1".to_owned()), "{seen:?}");
    assert!(
        seen.contains(&"admit phone.challenge proof={\"payload\":\"x\",\"sig\":\"y\"}".to_owned()),
        "{seen:?}"
    );
    assert!(
        !seen.iter().any(|s| s.contains("notifications.subscribe")),
        "never reaches the gate"
    );
    server.close().await;
}
