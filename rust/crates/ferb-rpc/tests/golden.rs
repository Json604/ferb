//! ENG-3: the Rust server answers every recorded situation with the bytes the Python server wrote
//! (`rust/fixtures/rpc/frames.json`), and its tables are Python's (`table.json`). Both files are written by
//! `rust/fixtures/rpc/make.py`, which plays the same scenarios against `ferb.daemon.server.Server`.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use common::{new_server, with_native, Raw};
use ferb_rpc::auth::{GrantSpec, Identity, Role, METHODS};
use ferb_rpc::{peer, server, wire, RunTokens, Server};
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/rpc")
        .join(name);
    serde_json::from_slice(
        &std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .unwrap()
}

fn names(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect()
}

// decision: DMN-23
// decision: DMN-33
#[test]
fn the_method_table_is_python_s() {
    let table = fixture("table.json");
    for role in Role::ALL {
        let ours: Vec<String> = METHODS
            .iter()
            .filter(|m| m.roles.contains(&role))
            .map(|m| m.name.to_owned())
            .collect();
        assert_eq!(
            ours,
            names(&table["roles"][role.as_str()]),
            "{}",
            role.as_str()
        );
    }
    assert_eq!(table["roles"].as_object().unwrap().len(), Role::ALL.len());
    let app_only: Vec<String> = METHODS
        .iter()
        .filter(|m| m.app_only)
        .map(|m| m.name.to_owned())
        .collect();
    assert_eq!(app_only, names(&table["app_only"]));
    // the numbers the port was told to expect
    let count = |role| METHODS.iter().filter(|m| m.roles.contains(&role)).count();
    assert_eq!(
        (
            count(Role::Cli),
            count(Role::McpShim),
            count(Role::NativeHost),
            app_only.len()
        ),
        (106, 3, 0, 20)
    );
}

// decision: DMN-510
// decision: DMN-403
#[test]
fn the_constants_are_python_s() {
    let table = fixture("table.json");
    assert_eq!(table["max_message"], wire::MAX_MESSAGE);
    let codes: BTreeMap<&str, i64> = BTreeMap::from([
        ("PARSE_ERROR", wire::PARSE_ERROR),
        ("INVALID_REQUEST", wire::INVALID_REQUEST),
        ("METHOD_NOT_FOUND", wire::METHOD_NOT_FOUND),
        ("INTERNAL", wire::INTERNAL),
        ("UNAUTHORIZED", wire::UNAUTHORIZED),
        ("TOO_LARGE", wire::TOO_LARGE),
        ("UNSUPPORTED", wire::UNSUPPORTED),
        ("FAILED", wire::FAILED),
        ("APP_ONLY_REFUSED", wire::APP_ONLY_REFUSED),
    ]);
    assert_eq!(serde_json::to_value(&codes).unwrap(), table["codes"]);
    assert_eq!(table["client_wait_s"], server::CLIENT_WAIT_S);
    let waits: BTreeMap<&str, f64> = server::LONG_WAITS.iter().copied().collect();
    assert_eq!(waits.len(), server::LONG_WAITS.len());
    assert_eq!(serde_json::to_value(&waits).unwrap(), table["long_waits"]);
    assert_eq!(server::CANCEL_WHEN_LEFT, names(&table["cancel_when_left"]));
    assert_eq!(
        server::ANSWERED_WHILE_STARTING,
        names(&table["answered_while_starting"])
    );
    assert_eq!(
        table["journal_kinds"]["INTERNAL_ERROR"],
        server::INTERNAL_ERROR
    );
    assert_eq!(table["journal_kinds"]["SLOW_CALL"], server::SLOW_CALL);
    assert_eq!(
        table["dev_requirement"],
        ferb_rpc::app_peer::DEV_REQUIREMENT
    );
    assert_eq!(table["app_id"], ferb_rpc::app_peer::APP_ID);
}

async fn play(scenario: &Value) {
    let name = scenario["name"].as_str().unwrap();
    let options = &scenario["server"];
    let flag = |key: &str| options[key].as_bool().unwrap_or(false);
    let dir = tempfile::tempdir().unwrap();
    let (mut server, clock) = new_server(dir.path());
    if flag("owner_uid_other") {
        let tokens = RunTokens::new(clock.clone());
        server = Server::with_owner_uid(
            server.path().to_owned(),
            tokens.into(),
            peer::current_uid() + 1,
        );
        common::register_stubs(&server);
    }
    if flag("native") {
        with_native(&server);
    }
    if flag("is_app") {
        server.set_is_app(|_| true);
    }
    if flag("starting") {
        server.set_starting();
    }
    let token = match options.get("token") {
        Some(t) => {
            let spec = GrantSpec::new(Identity::Owner, t["ttl_s"].as_f64().unwrap());
            server.tokens().issue(
                t["task_id"].as_str().unwrap(),
                t["worker_id"].as_str().unwrap(),
                spec,
            )
        }
        None => String::new(),
    };
    server.start().await.unwrap();
    let mut client = Raw::connect(server.path()).await;
    for (at, step) in scenario["steps"].as_array().unwrap().iter().enumerate() {
        let here = format!("{name:?}, step {at}");
        if let Some(text) = step.get("send") {
            client
                .send(text.as_str().unwrap().replace("$TOKEN", &token).as_bytes())
                .await
                .expect(&here);
        } else if let Some(hex) = step.get("send_hex") {
            let hex = hex.as_str().unwrap();
            let bytes: Vec<u8> = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect();
            client.send(&bytes).await.expect(&here);
        } else if let Some(padded) = step.get("send_padded") {
            let (prefix, suffix) = (
                padded["prefix"].as_str().unwrap(),
                padded["suffix"].as_str().unwrap(),
            );
            let fill =
                padded["content_len"].as_u64().unwrap() as usize - prefix.len() - suffix.len();
            // the server may stop reading a message over the cap before all of it is written
            let _ = client
                .send(format!("{prefix}{}{suffix}\n", "a".repeat(fill)).as_bytes())
                .await;
        } else if let Some(expected) = step.get("recv") {
            let line = client.line().await;
            assert_eq!(
                String::from_utf8_lossy(&line),
                expected.as_str().unwrap(),
                "{here}"
            );
        } else if step.get("eof").is_some() {
            assert!(
                client.at_end().await,
                "{here}: the connection should have ended"
            );
        } else if let Some(ms) = step.get("silent_ms") {
            assert!(
                client
                    .silent(Duration::from_millis(ms.as_u64().unwrap()))
                    .await,
                "{here}: an answer came"
            );
        } else {
            match step["op"].as_str().unwrap() {
                "revoke_all" => drop(server.tokens().revoke_all()),
                "revoke_tree" => drop(server.tokens().revoke_tree(step["tree"].as_str().unwrap())),
                "advance" => clock.advance(step["seconds"].as_f64().unwrap()),
                "set_ready" => server.set_ready(),
                "shut_wr" => {
                    use tokio::io::AsyncWriteExt;
                    client.writer.shutdown().await.expect(&here);
                }
                "close" => server.close().await,
                other => panic!("{here}: unknown op {other}"),
            }
        }
    }
    drop(client);
    server.close().await;
    assert!(
        !server.path().exists(),
        "{name:?}: close removes the socket"
    );
}

// decision: DMN-23
// decision: DMN-402
// decision: DMN-510
// decision: HAND-406
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_recorded_scenario_gets_python_s_bytes() {
    let frames = fixture("frames.json");
    assert_eq!(frames["stub_extension_id"], common::EXTENSION);
    let scenarios = frames["scenarios"].as_array().unwrap();
    assert!(scenarios.len() >= 40, "the fixture looks cut short");
    for scenario in scenarios {
        play(scenario).await;
    }
}
