//! The server's behavior that no recorded frame shows: the socket's files, a second daemon, callers that leave,
//! abandoned shim calls, the recording hooks and closing. Ported from `tests/test_server.py` where a Python
//! test exists (named at each test).

mod common;

use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{new_server, ok, with_native, Raw, WAIT};
use ferb_rpc::auth::{GrantSpec, Identity};
use ferb_rpc::server::{BoxFuture, EventStream, HandlerError, HandlerResult};
use ferb_rpc::wire::{self, Params};
use ferb_rpc::{caller_left, caller_left_flag, Server, Session, StartError};
use serde_json::json;
use tokio::io::AsyncBufReadExt;
use tokio::sync::Notify;

fn mode(path: &std::path::Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

/// Waits until `done()`; fails after `WAIT`.
async fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !done() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "never happened: {what}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Sets its flag when dropped: a cancelled call's or a closed stream's trace.
struct Dropped(Arc<AtomicBool>);

impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

// tests/test_server.py:27-30 (spike 8)
#[tokio::test]
async fn socket_and_folder_are_owner_only_and_close_removes_only_the_socket() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = new_server(dir.path());
    server.start().await.unwrap();
    assert_eq!(mode(server.path().parent().unwrap()), 0o700);
    assert_eq!(mode(server.path()), 0o600);
    server.close().await;
    assert!(!server.path().exists() && server.path().parent().unwrap().exists());
}

#[tokio::test]
async fn an_existing_wider_folder_is_narrowed_to_the_owner() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    std::fs::create_dir(&run).unwrap();
    std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (server, _) = new_server(dir.path());
    server.start().await.unwrap();
    assert_eq!(mode(&run), 0o700);
    server.close().await;
}

// tests/test_server.py:134-143 (spike 8)
#[tokio::test]
async fn stale_socket_file_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = new_server(dir.path());
    std::fs::create_dir_all(server.path().parent().unwrap()).unwrap();
    drop(std::os::unix::net::UnixListener::bind(server.path()).unwrap()); // bound, then its daemon died
    assert!(server.path().exists());
    server.start().await.unwrap();
    assert!(Raw::cli(server.path())
        .await
        .call("ping", json!({}))
        .await
        .is_ok());
    server.close().await;
}

// tests/test_server.py:146-152 (spike 8)
#[tokio::test]
async fn second_daemon_refuses_to_start_and_leaves_the_first_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (first, _) = new_server(dir.path());
    first.start().await.unwrap();
    let (second, _) = new_server(dir.path());
    let refused = second.start().await.unwrap_err();
    assert!(
        matches!(refused, StartError::AlreadyRunning(_)),
        "{refused}"
    );
    assert_eq!(
        refused.to_string(),
        format!("another daemon is answering on {}", first.path().display())
    );
    second.close().await; // a failed start never removes another daemon's socket
    assert!(Raw::cli(first.path())
        .await
        .call("ping", json!({}))
        .await
        .is_ok());
    first.close().await;
}

#[tokio::test]
async fn the_daemon_s_ping_says_pid_role_now_stopped_in_that_order() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = new_server(dir.path());
    server.register_unary(
        "ping",
        ferb_rpc::ping(|| "2026-11-13T10:00:00+05:30".to_owned(), || true),
    );
    server.start().await.unwrap();
    let mut cli = Raw::cli(server.path()).await;
    cli.request("ping", json!({})).await;
    let expected = format!(
        "{{\"jsonrpc\": \"2.0\", \"id\": 2, \"result\": {{\"pid\": {}, \"role\": \"cli\", \
         \"now\": \"2026-11-13T10:00:00+05:30\", \"stopped\": true}}}}\n",
        std::process::id()
    );
    assert_eq!(String::from_utf8(cli.line().await).unwrap(), expected);
    server.close().await;
}

// decision: DMN-403
#[tokio::test]
async fn a_preview_call_ends_when_its_caller_leaves_and_another_call_runs_to_its_end() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = new_server(dir.path());
    let (cancelled, finished) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let started = Arc::new(AtomicUsize::new(0));
    server.set_is_app(|_| true);
    {
        let (cancelled, started) = (cancelled.clone(), started.clone());
        server.register_unary("tasks.preview", move |_: Session, _: Params| {
            let (guard, started) = (Dropped(cancelled.clone()), started.clone());
            async move {
                let _guard = guard;
                started.fetch_add(1, Ordering::SeqCst);
                std::future::pending::<HandlerResult>().await
            }
        });
    }
    {
        let (finished, started) = (finished.clone(), started.clone());
        server.register_unary("trace.get", move |_: Session, _: Params| {
            let (finished, started) = (finished.clone(), started.clone());
            async move {
                started.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(150)).await;
                finished.store(true, Ordering::SeqCst);
                ok(json!({}))
            }
        });
    }
    server.start().await.unwrap();

    let mut preview = Raw::cli(server.path()).await;
    preview
        .request("tasks.preview", json!({"task_id": "t"}))
        .await;
    until("the preview started", || {
        started.load(Ordering::SeqCst) == 1
    })
    .await;
    assert!(!cancelled.load(Ordering::SeqCst));
    drop(preview);
    until("the abandoned preview was cancelled", || {
        cancelled.load(Ordering::SeqCst)
    })
    .await;

    let mut other = Raw::cli(server.path()).await;
    other.request("trace.get", json!({})).await;
    until("the other call started", || {
        started.load(Ordering::SeqCst) == 2
    })
    .await;
    drop(other);
    until(
        "a call that is not a preview finishes without its caller",
        || finished.load(Ordering::SeqCst),
    )
    .await;
    server.close().await;
}

// decision: DMN-403
#[tokio::test]
async fn a_preview_caller_that_sends_its_next_request_early_is_still_there() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = new_server(dir.path());
    server.set_is_app(|_| true);
    let release = Arc::new(Notify::new());
    {
        let release = release.clone();
        server.register_unary("tasks.preview", move |_: Session, _: Params| {
            let release = release.clone();
            async move {
                release.notified().await;
                ok(json!({"frame": 2}))
            }
        });
    }
    server.start().await.unwrap();
    let mut cli = Raw::cli(server.path()).await;
    cli.request("tasks.preview", json!({})).await;
    cli.request("ping", json!({})).await; // sent while the preview runs: kept for after it
    assert!(cli.silent(Duration::from_millis(100)).await);
    release.notify_one();
    assert_eq!(
        cli.incoming().await.result,
        Some(common::object(json!({"frame": 2})))
    );
    assert_eq!(cli.incoming().await.id, Some(json!(3)));
    server.close().await;
}

/// An MCP shim session on `server` for task "task-9".
async fn shim(server: &Server) -> Raw {
    let token = server
        .tokens()
        .issue("task-9", "w-1", GrantSpec::new(Identity::Owner, 60.0));
    Raw::hello(server.path(), json!({"role": "mcp_shim", "token": token})).await
}

type Seen = Arc<Mutex<Vec<(bool, bool)>>>;

/// `tools.call` that reports what it saw when released: (caller_left() at the end, the flag carried into a
/// spawned task).
fn slow_tool(server: &Server) -> (Arc<AtomicUsize>, Arc<Notify>, Seen) {
    let (running, release, seen) = (
        Arc::new(AtomicUsize::new(0)),
        Arc::new(Notify::new()),
        Seen::default(),
    );
    let (running2, release2, seen2) = (running.clone(), release.clone(), seen.clone());
    server.register_unary("tools.call", move |_: Session, _: Params| {
        let (running, release, seen) = (running2.clone(), release2.clone(), seen2.clone());
        async move {
            assert!(!caller_left());
            let flag = caller_left_flag().expect("inside a shim's call");
            running.fetch_add(1, Ordering::SeqCst);
            release.notified().await;
            let carried = tokio::spawn(async move { (caller_left(), flag.is_set()) })
                .await
                .unwrap();
            assert!(
                !carried.0,
                "a spawned task does not inherit the flag; it carries it"
            );
            seen.lock().unwrap().push((caller_left(), carried.1));
            ok(json!({"done": true}))
        }
    });
    (running, release, seen)
}

#[tokio::test]
async fn a_shim_that_leaves_mid_call_lets_the_call_finish_and_the_call_can_tell() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = new_server(dir.path());
    let (running, release, seen) = slow_tool(&server);
    server.start().await.unwrap();
    let mut caller = shim(&server).await;
    caller.request("tools.call", json!({"call_id": "c1"})).await;
    until("the call runs", || running.load(Ordering::SeqCst) == 1).await;
    drop(caller); // the runtime gave up the call
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(seen.lock().unwrap().is_empty(), "the call is not cancelled");
    release.notify_one();
    until("the call finished", || !seen.lock().unwrap().is_empty()).await;
    assert_eq!(*seen.lock().unwrap(), [(true, true)]);
    assert!(
        !server.abandon("task-9", "c1"),
        "a call whose shim left is no longer named"
    );
    assert!(!caller_left(), "outside a shim's call nobody has left");
    server.close().await;
}

#[tokio::test]
async fn an_abandoned_shim_call_closes_its_connection_and_only_that_task_s_oldest_call() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = new_server(dir.path());
    let (running, release, seen) = slow_tool(&server);
    server.start().await.unwrap();
    let (mut first, mut second) = (shim(&server).await, shim(&server).await);
    first
        .request("tools.call", json!({"call_id": "same"}))
        .await;
    until("the first call runs", || {
        running.load(Ordering::SeqCst) == 1
    })
    .await;
    second
        .request("tools.call", json!({"call_id": "same"}))
        .await; // a runtime may repeat a call id
    until("the second call runs", || {
        running.load(Ordering::SeqCst) == 2
    })
    .await;

    assert!(!server.abandon("another-task", "same"));
    assert!(!server.abandon("task-9", "other-call"));
    assert!(server.abandon("task-9", "same"));
    assert!(
        first.at_end().await,
        "the abandoned call's shim reads the end at once, before the call returns"
    );
    assert!(
        second.silent(Duration::from_millis(50)).await,
        "the newer call is untouched"
    );
    assert!(
        seen.lock().unwrap().is_empty(),
        "the abandoned call is not cancelled"
    );

    // both calls wait on the same Notify; one permit each
    release.notify_waiters();
    until("both calls finished", || seen.lock().unwrap().len() == 2).await;
    let mut verdicts = seen.lock().unwrap().clone();
    verdicts.sort();
    assert_eq!(verdicts, [(false, false), (true, true)]);
    assert_eq!(
        second.incoming().await.result,
        Some(common::object(json!({"done": true})))
    );
    assert!(
        !server.abandon("task-9", "same"),
        "nothing is running under that name any more"
    );
    server.close().await;
}

/// A stream that sends one event and then waits for ever.
struct Endless(Option<HandlerResult>, #[allow(dead_code)] Dropped);

impl EventStream for Endless {
    fn next(&mut self) -> BoxFuture<'_, Option<HandlerResult>> {
        match self.0.take() {
            Some(first) => Box::pin(std::future::ready(Some(first))),
            None => Box::pin(std::future::pending()),
        }
    }
}

fn endless(server: &Server) -> Arc<AtomicBool> {
    let closed = Arc::new(AtomicBool::new(false));
    let flag = closed.clone();
    server.register_stream(
        "logs.subscribe",
        move |_: Session, _: Params| -> Box<dyn EventStream> {
            Box::new(Endless(Some(ok(json!({"line": 1}))), Dropped(flag.clone())))
        },
    );
    closed
}

#[tokio::test]
async fn a_stream_is_closed_when_its_client_leaves() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = new_server(dir.path());
    let closed = endless(&server);
    server.start().await.unwrap();
    let mut cli = Raw::cli(server.path()).await;
    cli.request("logs.subscribe", json!({})).await;
    assert_eq!(
        cli.incoming().await.method.as_deref(),
        Some("logs.subscribe.event")
    );
    assert!(!closed.load(Ordering::SeqCst));
    drop(cli);
    until("the stream was closed", || closed.load(Ordering::SeqCst)).await;
    server.close().await;
}

#[tokio::test]
async fn a_client_that_sends_during_its_stream_ends_it_without_a_last_frame() {
    // server.py:420-436: `reader.read(1)` returns for anything the client sends, not only for its leaving; the
    // stream is cancelled, nothing more is written for it and the byte read is gone from the input.
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = new_server(dir.path());
    let closed = endless(&server);
    server.start().await.unwrap();
    let mut cli = Raw::cli(server.path()).await;
    cli.request("logs.subscribe", json!({})).await;
    cli.incoming().await;
    cli.send(b" ").await.unwrap(); // one byte, eaten by the watch
    until("the stream was closed", || closed.load(Ordering::SeqCst)).await;
    assert!(
        cli.call("ping", json!({})).await.is_ok(),
        "the connection keeps serving"
    );
    server.close().await;
}

#[tokio::test]
async fn closing_ends_open_streams_and_waits_for_them() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = new_server(dir.path());
    let closed = endless(&server);
    server.start().await.unwrap();
    let mut cli = Raw::cli(server.path()).await;
    cli.request("logs.subscribe", json!({})).await;
    cli.incoming().await;
    tokio::time::timeout(WAIT, server.close())
        .await
        .expect("close returns");
    assert!(
        closed.load(Ordering::SeqCst),
        "the stream was closed before close returned"
    );
    assert!(cli.at_end().await);
    assert!(
        tokio::net::UnixStream::connect(server.path())
            .await
            .is_err(),
        "nobody is listening"
    );
}

#[tokio::test]
async fn a_fault_is_told_to_the_daemon_and_a_broken_hook_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = new_server(dir.path());
    let told: Arc<Mutex<Vec<(String, HandlerError)>>> = Arc::default();
    {
        let told = told.clone();
        server.on_internal(move |method, error| {
            told.lock()
                .unwrap()
                .push((method.to_owned(), error.clone()));
            panic!("a hook that fails (expected in this test)");
        });
    }
    server.register_unary("ps.list", |_: Session, params: Params| async move {
        assert!(
            !params.is_empty(),
            "a handler that panics (expected in this test)"
        );
        ok(json!({}))
    });
    server.start().await.unwrap();
    let mut cli = Raw::cli(server.path()).await;
    assert_eq!(
        cli.call("memory.list", json!({})).await,
        Err((wire::INTERNAL, "RuntimeError".to_owned()))
    );
    assert_eq!(
        cli.call("doctor.check", json!({})).await,
        Err((wire::FAILED, "the check said no".to_owned()))
    );
    assert_eq!(
        cli.call("ps.list", json!({})).await,
        Err((wire::INTERNAL, "Panic".to_owned()))
    );
    // a stream's fault is told too; its refusal is not (server.py:430-431)
    cli.request("ps.attach", json!({})).await;
    assert_eq!(
        cli.incoming().await.error.unwrap().message,
        "ValueError: bad value"
    );
    cli.request("notifications.subscribe", json!({})).await;
    cli.incoming().await;
    assert_eq!(
        cli.incoming().await.error.unwrap().message,
        "Refused: the stream said no"
    );
    assert!(cli.call("ping", json!({})).await.is_ok());
    let told = told.lock().unwrap().clone();
    let methods: Vec<&str> = told.iter().map(|(method, _)| method.as_str()).collect();
    assert_eq!(methods, ["memory.list", "ps.list", "ps.attach"]);
    assert_eq!(
        told[0].1.message,
        "boom, with words that must not reach the caller"
    );
    server.close().await;
}

async fn timed(raw: &mut Raw, method: &str, seconds: f64) {
    raw.request(method, json!({"s": seconds})).await;
    let mut line = Vec::new();
    raw.reader.read_until(b'\n', &mut line).await.unwrap(); // no real-time limit: the test's clock is paused
    assert!(line.ends_with(b"\n"));
}

#[tokio::test(start_paused = true)]
async fn an_owner_side_call_as_long_as_its_caller_waits_is_told_as_slow() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = new_server(dir.path());
    let slow: Arc<Mutex<Vec<(String, f64)>>> = Arc::default();
    {
        let slow = slow.clone();
        server.on_slow(move |method, seconds| {
            slow.lock().unwrap().push((method.to_owned(), seconds))
        });
    }
    for method in ["trace.get", "reports.build", "tools.call"] {
        server.register_unary(method, |_: Session, params: Params| async move {
            tokio::time::sleep(Duration::from_secs_f64(params["s"].as_f64().unwrap())).await;
            ok(json!({}))
        });
    }
    server.start().await.unwrap();
    let mut cli = Raw::cli(server.path()).await;
    timed(&mut cli, "trace.get", 9.0).await; // under the 10 s a caller waits
    timed(&mut cli, "trace.get", 10.0).await;
    timed(&mut cli, "reports.build", 100.0).await; // the notch waits 120 s for this one
    timed(&mut cli, "reports.build", 120.0).await;
    let mut tool = shim(&server).await;
    timed(&mut tool, "tools.call", 500.0).await; // a shim waits however long its call runs
    let slow = slow.lock().unwrap().clone();
    let methods: Vec<&str> = slow.iter().map(|(method, _)| method.as_str()).collect();
    assert_eq!(methods, ["trace.get", "reports.build"]);
    assert!(
        (10.0..11.0).contains(&slow[0].1) && (120.0..121.0).contains(&slow[1].1),
        "{slow:?}"
    );
}

// decision: HAND-406
#[tokio::test]
async fn a_refused_native_host_hello_is_told_with_its_reason_and_origin() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = new_server(dir.path());
    let refused: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    {
        let refused = refused.clone();
        server.on_native_refused(move |why, origin| {
            refused
                .lock()
                .unwrap()
                .push((why.to_owned(), origin.to_owned()))
        });
    }
    server.start().await.unwrap();
    let path = server.path().to_owned();
    let hello = async |params: serde_json::Value| {
        Raw::connect(&path).await.call("auth.hello", params).await
    };
    let long = format!("chrome-extension://{}", "\u{e9}".repeat(200));
    assert_eq!(
        hello(json!({"role": "native_host", "origin": long}))
            .await
            .unwrap_err()
            .0,
        wire::UNSUPPORTED
    );
    with_native(&server);
    assert!(
        hello(json!({"role": "native_host", "origin": "https://x/"}))
            .await
            .is_err()
    );
    assert!(hello(json!({"role": "native_host"})).await.is_err());
    // not told: a hello that is not a valid hello, another role's refusal, and an accepted host
    assert!(hello(json!({"role": "native_host", "origin": 5}))
        .await
        .is_err());
    assert!(
        hello(json!({"role": "mcp_shim", "token": "made-up", "origin": "x"}))
            .await
            .is_err()
    );
    let origin = format!("chrome-extension://{}/", common::EXTENSION);
    assert!(hello(json!({"role": "native_host", "origin": origin}))
        .await
        .is_ok());
    let refused = refused.lock().unwrap().clone();
    assert_eq!(refused.len(), 3, "{refused:?}");
    assert_eq!(refused[0].0, "FERB's browser bridge isn't running");
    assert_eq!(refused[0].1.chars().count(), 120);
    assert_eq!(
        refused[1],
        (
            "unknown extension origin".to_owned(),
            "https://x/".to_owned()
        )
    );
    assert_eq!(
        refused[2],
        ("unknown extension origin".to_owned(), String::new())
    );
    server.close().await;
}
