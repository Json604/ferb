//! The Rust RPC server with the Mac's answers in place: the peer's pid and audit token from the kernel, and
//! the FERB.app check against the real Security framework. The server's own tests (with no platform) are in
//! `ferb-rpc/tests`; the stub handlers and the test client are theirs.
#![cfg(target_os = "macos")]

#[path = "../../ferb-rpc/tests/common/mod.rs"]
mod common;

use std::sync::{Arc, Mutex};

use common::{new_server, ok, Raw};
use ferb_rpc::wire::{self, Params};
use ferb_rpc::Session;
use serde_json::json;

// decision: DMN-402
#[tokio::test]
async fn the_ferb_app_check_gets_the_peer_s_audit_token_once_per_connection() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = new_server(dir.path());
    server.set_peer_lookup(ferb_platform_macos::peer::facts);
    let asked: Arc<Mutex<Vec<Option<ferb_rpc::AuditToken>>>> = Arc::default();
    {
        let asked = asked.clone();
        server.set_is_app(move |token| {
            asked.lock().unwrap().push(token);
            true
        });
    }
    let seen: Arc<Mutex<Vec<Session>>> = Arc::default();
    {
        let seen = seen.clone();
        server.register_unary("tasks.preview", move |session: Session, _: Params| {
            seen.lock().unwrap().push(session);
            async { ok(json!({})) }
        });
    }
    server.start().await.unwrap();
    let mut cli = Raw::cli(server.path()).await;
    for _ in 0..3 {
        assert!(cli.call("tasks.preview", json!({})).await.is_ok());
    }
    assert!(cli.call("ping", json!({})).await.is_ok()); // not FERB.app-only: no check
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    // the token names this very process (audit_token_t.val[5] is the pid), as does the session's pid
    let token = asked[0].expect("the socket gave a token");
    assert_eq!(
        u32::from_ne_bytes(token[20..24].try_into().unwrap()),
        std::process::id()
    );
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen[0].pid, Some(std::process::id() as i32));
    assert_eq!((seen[0].token, seen[0].app), (Some(token), Some(true)));
    server.close().await;
}

// decision: DMN-402
#[tokio::test]
async fn the_real_check_refuses_this_test_process() {
    // The real Security framework, in the mode this test binary runs in (a checkout): this process is not
    // signed as the notch, so it is not FERB.app. FERB's home is a folder that does not exist.
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = new_server(dir.path());
    ferb_platform_macos::rpc::attach(&server, &dir.path().join("no-home"));
    server.start().await.unwrap();
    let mut cli = Raw::cli(server.path()).await;
    let refused = cli.call("tasks.preview", json!({})).await.unwrap_err();
    assert_eq!(
        refused,
        (
            wire::APP_ONLY_REFUSED,
            "tasks.preview is for FERB.app only".to_owned()
        )
    );
    server.close().await;
}
