//! The chat and task methods through the daemon's real wire: `chat_rpc::register` on a real `ferb_rpc::Server`
//! listening on a socket in a temporary folder, each recorded request line (rust/fixtures/daemon/rpc_transcripts.json)
//! written as the Python client wrote it and each reply compared byte for byte with the Python daemon's.
//!
//! The lines replayed are those a fresh store answers the same: the role table's refusals, the refusals of bad
//! parameters, the empty answers (no task, no question, no conversation) and `chat.attach`. Lines that need the state
//! a recorded chat turn left (a task's row, an open question) are replayed in-process by tests/replay.rs.

mod common;
mod support_control;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;

use common::{read, s};
use ferb_gateway::questions::{Approval, Approvals, ResolveError};
use ferb_orchestrator::ports::BoxFuture;
use ferb_rpc::auth::RunGrant;
use ferb_supervisor::flow::chat_rpc::{ChatDeps, ChatGateway, ChatRpc};
use serde_json::{Map, Value};

/// A gateway with no question and no tool call (a fresh daemon's).
struct Fresh(std::sync::Mutex<Approvals>);

impl ChatGateway for Fresh {
    fn pending(&self) -> Vec<Approval> {
        self.0
            .lock()
            .unwrap()
            .pending()
            .into_iter()
            .cloned()
            .collect()
    }
    fn approval(&self, id: &str) -> Option<Approval> {
        self.0.lock().unwrap().get(id).cloned()
    }
    fn resolve(
        &self,
        id: &str,
        granted: bool,
        choice: Option<usize>,
        text: Option<&str>,
        _hidden: &[String],
    ) -> Result<Approval, ResolveError> {
        self.0
            .lock()
            .unwrap()
            .resolve(id, granted, choice, text)
            .cloned()
    }
    fn tools_for(&self, _grant: &RunGrant) -> Vec<Value> {
        Vec::new()
    }
    fn call<'a>(
        &'a self,
        _grant: &'a RunGrant,
        _name: &'a str,
        _arguments: &'a Map<String, Value>,
    ) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async { Err("RuntimeError".to_string()) })
    }
    fn stopped(&self) -> bool {
        false
    }
}

struct RpcClock;

impl ferb_rpc::Clock for RpcClock {
    fn monotonic(&self) -> f64 {
        1000.0
    }
}

/// The recorded (request, reply) lines a fresh store answers, by scenario and the request's id on its connection.
const LINES: [(&str, &str, u64); 20] = [
    ("protocol", "cli", 5),
    ("protocol", "cli", 6),
    ("settings_setup_about", "app", 32),
    ("settings_setup_about", "app", 33),
    ("settings_setup_about", "app", 57),
    ("settings_setup_about", "app", 58),
    ("settings_setup_about", "app", 60),
    ("tasks_and_records", "app#3", 4),
    ("tasks_and_records", "app#3", 11),
    ("tasks_and_records", "app#3", 12),
    ("tasks_and_records", "app#3", 13),
    ("tasks_and_records", "app#3", 14),
    ("tasks_and_records", "app#3", 15),
    ("tasks_and_records", "app#3", 16),
    ("questions", "app", 8),
    ("questions", "app", 9),
    ("questions", "app", 10),
    ("questions", "app", 11),
    ("questions", "app", 12),
    ("questions", "app", 6),
];

/// (scenario, connection, id) → (request line, reply line).
fn recorded(scenario: &str, conn: &str, id: u64) -> (String, String) {
    let all = read("rust/fixtures/daemon/rpc_transcripts.json");
    let sc = all["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == scenario)
        .unwrap();
    for st in sc["steps"].as_array().unwrap() {
        if st["conn"] != conn {
            continue;
        }
        let lines = st["lines"].as_array().cloned().unwrap_or_default();
        let sent = lines
            .iter()
            .filter_map(|l| l.get("send").and_then(Value::as_str))
            .find(|l| serde_json::from_str::<Value>(l).is_ok_and(|v| v["id"] == id));
        if let Some(sent) = sent {
            let reply = lines
                .iter()
                .filter_map(|l| l.get("recv").and_then(Value::as_str))
                .find(|l| serde_json::from_str::<Value>(l).is_ok_and(|v| v["id"] == id))
                .unwrap();
            return (sent.to_string(), reply.to_string());
        }
    }
    panic!("no recorded line {scenario} {conn} {id}")
}

#[test]
fn the_chat_methods_on_the_real_server() {
    let world = support_control::world("plain_reply");
    let sup = world.sup.clone();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("s.sock");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let server = rt.block_on(async {
        let server = ferb_rpc::Server::new(
            socket.clone(),
            Arc::new(ferb_rpc::RunTokens::new(Arc::new(RpcClock))),
        );
        let rpc = ChatRpc::new(ChatDeps {
            supervisor: sup,
            gateway: Arc::new(Fresh(std::sync::Mutex::new(Approvals::new(|| {
                "ap-a00000000001".to_string()
            })))),
            persona: None,
            prepare_persona: Arc::new(|| {}),
            notices: Some(Arc::new(Vec::new)),
            conversations: Arc::new(Vec::new),
            sensing_paused: Arc::new(|| false),
            shutting_down: Arc::new(|| false),
            question_answered: Arc::new(|_ap| String::new()),
            ask_input_monitoring: Arc::new(|| {}),
            click_through: None,
        });
        ferb_supervisor::flow::chat_rpc::register(&server, rpc);
        server.set_is_app(|_| true);
        server.start().await.expect("the server");
        server
    });
    let mut differ = Vec::new();
    let mut conns: std::collections::HashMap<String, (BufReader<UnixStream>, UnixStream)> =
        std::collections::HashMap::new();
    for (scenario, conn, id) in LINES {
        let (sent, want) = recorded(scenario, conn, id);
        let (reader, writer) = conns.entry(format!("{scenario}/{conn}")).or_insert_with(|| {
            let st = UnixStream::connect(&socket).unwrap();
            let mut w = st.try_clone().unwrap();
            let mut r = BufReader::new(st);
            w.write_all(
                b"{\"jsonrpc\": \"2.0\", \"id\": 0, \"method\": \"auth.hello\", \"params\": {\"role\": \"cli\"}}\n",
            )
            .unwrap();
            let mut hello = String::new();
            r.read_line(&mut hello).unwrap();
            (r, w)
        });
        writer.write_all(format!("{sent}\n").as_bytes()).unwrap();
        let mut got = String::new();
        reader.read_line(&mut got).unwrap();
        let got = got.trim_end_matches('\n');
        // ENG-6: an unknown conversation id was an internal error (ValueError) in Python; refused in plain words here
        let want = if (scenario, id) == ("settings_setup_about", 58) {
            format!(
                "{{\"jsonrpc\": \"2.0\", \"id\": 58, \"error\": {{\"code\": -32004, \"message\": \"{}\"}}}}",
                ferb_supervisor::flow::chat_rpc::NO_CONVERSATION
            )
        } else {
            want
        };
        if got != want {
            differ.push(format!(
                "{scenario} {conn} {id}: {}\n    python: {want}\n    rust:   {got}",
                s(&serde_json::from_str::<Value>(&sent).unwrap()["method"])
            ));
        }
    }
    drop(conns);
    rt.block_on(server.close());
    eprintln!("{} lines through the real server", LINES.len());
    assert!(differ.is_empty(), "{}", differ.join("\n"));
}
