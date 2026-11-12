//! The stub handlers of `rust/fixtures/rpc/make.py`, registered on the Rust server, and a small test client.
#![allow(dead_code)]

use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_rpc::server::{
    BoxFuture, EventStream, HandlerError, HandlerResult, NativeChannel, NativeConnection,
};
use ferb_rpc::wire::{self, Incoming, Params};
use ferb_rpc::{caller_left, Clock, RunTokens, Server, Session};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;

pub const WAIT: Duration = Duration::from_secs(5);

/// `ferb.clock.clock.FakeClock`'s monotonic side.
pub struct FakeClock(Mutex<f64>);

impl FakeClock {
    pub fn new() -> Arc<Self> {
        Arc::new(FakeClock(Mutex::new(1000.0)))
    }

    pub fn advance(&self, seconds: f64) {
        *self.0.lock().unwrap() += seconds;
    }
}

impl Clock for FakeClock {
    fn monotonic(&self) -> f64 {
        *self.0.lock().unwrap()
    }
}

pub fn object(value: Value) -> Params {
    match value {
        Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

pub fn ok(value: Value) -> HandlerResult {
    Ok(object(value))
}

/// A stream of events known up front.
pub struct Events(pub VecDeque<HandlerResult>);

impl EventStream for Events {
    fn next(&mut self) -> BoxFuture<'_, Option<HandlerResult>> {
        Box::pin(std::future::ready(self.0.pop_front()))
    }
}

pub fn events(items: Vec<HandlerResult>) -> Box<dyn EventStream> {
    Box::new(Events(items.into()))
}

/// make.py's `native`: one line in, the same line back behind "native:", and the connection ends.
pub struct EchoChannel;

impl NativeChannel for EchoChannel {
    fn run(&self, connection: NativeConnection) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            let NativeConnection {
                reader,
                buffered,
                mut writer,
            } = connection;
            let mut line = buffered;
            let mut reader = BufReader::new(reader);
            if !line.ends_with(b"\n") {
                reader.read_until(b'\n', &mut line).await.unwrap();
            }
            writer.write_all(b"native:").await.unwrap();
            writer.write_all(&line).await.unwrap();
        })
    }
}

pub const EXTENSION: &str = "jaclnphefmmgmpofnabffahdkbjjompm";

/// make.py's UNARY and STREAMS on `server`.
pub fn register_stubs(server: &Server) {
    server.register_unary("ping", |session: Session, _: Params| async move {
        ok(json!({"pid": 4242, "role": session.role.as_str(), "now": "2026-11-13T10:00:00+00:00", "stopped": false}))
    });
    server.register_unary("trace.get", |_: Session, params: Params| async move {
        ok(json!({"echo": params}))
    });
    server.register_unary("about.get", |_: Session, _: Params| async move {
        ok(json!({
            "text": "na\u{ef}ve \u{2014} \u{65e5}\u{672c} \u{1f600} \u{7f} \u{1} \"q\" \\ / \n\t\r\u{8}\u{c}",
            "float": 1.5, "big": 1e16, "small": 1e-07, "whole": 2.0, "third": 0.1, "neg": -2.5e-05,
            "large": 123456789012345.0, "edge": 1234567890123456.0, "edge2": 12345678901234567.0,
            "tiny": 0.0001, "tinier": 0.00001, "avogadro": 6.02214076e23, "int": -7,
            "u64": 18446744073709551615u64, "list": [1, "a", null, true, false], "nested": {"k": {}},
            "empty": []
        }))
    });
    server.register_unary("doctor.check", |_: Session, _: Params| async move {
        Err(HandlerError::refusal("Refused", "the check said no"))
    });
    server.register_unary("memory.list", |_: Session, _: Params| async move {
        Err(HandlerError::fault(
            "RuntimeError",
            "boom, with words that must not reach the caller",
        ))
    });
    server.register_unary("memory.show", |_: Session, params: Params| async move {
        ok(json!({"len": params.get("p").and_then(Value::as_str).unwrap_or("").chars().count()}))
    });
    server.register_unary("tasks.preview", |_: Session, _: Params| async move {
        ok(json!({"frame": 1}))
    });
    server.register_unary("tools.call", |session: Session, _: Params| async move {
        let grant = session.grant.expect("an mcp_shim session has its grant");
        ok(json!({"task": grant.task_id, "worker": grant.worker_id, "left": caller_left()}))
    });
    server.register_stream("logs.subscribe", |_: Session, params: Params| {
        let n = params.get("n").and_then(Value::as_i64).unwrap_or(2);
        events((0..n).map(|i| ok(json!({"i": i}))).collect())
    });
    server.register_stream("notifications.subscribe", |_: Session, _: Params| {
        events(vec![
            ok(json!({"first": true})),
            Err(HandlerError::refusal("Refused", "the stream said no")),
        ])
    });
    server.register_stream("ps.attach", |_: Session, _: Params| {
        events(vec![Err(HandlerError::fault("ValueError", "bad value"))])
    });
}

pub fn with_native(server: &Server) {
    server.set_native(EchoChannel);
    server.set_extension_ids(|| HashSet::from([EXTENSION.to_owned()]));
}

pub fn new_server(folder: &Path) -> (Server, Arc<FakeClock>) {
    let clock = FakeClock::new();
    let server = Server::new(
        folder.join("run").join("s.sock"),
        Arc::new(RunTokens::new(clock.clone())),
    );
    register_stubs(&server);
    (server, clock)
}

/// A raw connection: lines out, lines in.
pub struct Raw {
    pub reader: BufReader<OwnedReadHalf>,
    pub writer: OwnedWriteHalf,
    next_id: i64,
}

impl Raw {
    pub async fn connect(path: &Path) -> Raw {
        let (reader, writer) = UnixStream::connect(path)
            .await
            .expect("connect")
            .into_split();
        Raw {
            reader: BufReader::new(reader),
            writer,
            next_id: 0,
        }
    }

    pub async fn send(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(bytes).await
    }

    /// The next line with its newline; empty at the end of input.
    pub async fn line(&mut self) -> Vec<u8> {
        let mut line = Vec::new();
        tokio::time::timeout(WAIT, self.reader.read_until(b'\n', &mut line))
            .await
            .expect("a line in time")
            .unwrap();
        line
    }

    pub async fn at_end(&mut self) -> bool {
        let mut byte = [0u8; 1];
        matches!(
            tokio::time::timeout(WAIT, self.reader.read(&mut byte)).await,
            Ok(Ok(0)) | Ok(Err(_))
        )
    }

    /// Nothing arrives for this long.
    pub async fn silent(&mut self, time: Duration) -> bool {
        tokio::time::timeout(time, self.reader.fill_buf())
            .await
            .is_err()
    }

    /// Sends one request as the Python client does and returns its id.
    pub async fn request(&mut self, method: &str, params: Value) -> i64 {
        self.next_id += 1;
        self.send(&wire::request_line(self.next_id, method, &object(params)))
            .await
            .unwrap();
        self.next_id
    }

    pub async fn incoming(&mut self) -> Incoming {
        let line = self.line().await;
        serde_json::from_slice(&line)
            .unwrap_or_else(|e| panic!("{e}: {:?}", String::from_utf8_lossy(&line)))
    }

    /// One call: its result, or its error's (code, message).
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Params, (i64, String)> {
        let id = self.request(method, params).await;
        let reply = self.incoming().await;
        assert_eq!(reply.id, Some(json!(id)));
        match (reply.result, reply.error) {
            (Some(result), None) => Ok(result),
            (None, Some(error)) => Err((error.code, error.message)),
            other => panic!("neither a result nor an error: {other:?}"),
        }
    }

    pub async fn hello(path: &Path, params: Value) -> Raw {
        let mut raw = Raw::connect(path).await;
        raw.call("auth.hello", params).await.expect("hello");
        raw
    }

    pub async fn cli(path: &Path) -> Raw {
        Raw::hello(path, json!({"role": "cli"})).await
    }
}
