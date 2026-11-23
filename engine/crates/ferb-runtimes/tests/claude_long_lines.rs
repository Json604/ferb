//! RUN-473 (agent, for the owner's "why 64mb is a limit??? what if limit hit??/ does it try another way???"): the
//! Claude Code reader never fails a run on a long line. Task 047ad9e8's attempts 2, 4 and 5 ended with
//! CLIJSONDecodeError: a PDF page's picture came back twice in one `user` line (the tool result and Claude Code's
//! `tool_use_result` echo), about 1.14M characters against the SDK's 1 MiB. A fake Claude Code here streams such lines
//! through the real `Client`.

use std::collections::BTreeMap;
use std::sync::Arc;

use ferb_runtimes::claude::transport::{without_echo, Client, Framed, Framer, Options, LINE_GUARD};
use ferb_runtimes::process::{BoxFuture, Child, ProcControl, Spawn, Spawner};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

struct Ended;

impl ProcControl for Ended {
    fn kill(&self) {}
    fn terminate(&self) {}
    fn returncode(&self) -> Option<i32> {
        None
    }
    fn wait(&self) -> BoxFuture<'_, i32> {
        Box::pin(async { 0 })
    }
}

/// What the fake Claude Code writes after it answered `initialize`.
#[derive(Clone)]
enum Out {
    Line(Value),
    /// A line of this many bytes that is no JSON FERB would read (streamed, never held).
    Huge(usize),
}

struct FakeClaude {
    out: std::sync::Mutex<Option<Vec<Out>>>,
}

impl Spawner for FakeClaude {
    fn spawn(&self, _spec: Spawn) -> std::io::Result<Child> {
        let out = self.out.lock().unwrap().take().expect("spawned once");
        let (stdin_w, stdin_r) = tokio::io::duplex(1 << 16);
        let (stdout_w, stdout_r) = tokio::io::duplex(1 << 20);
        tokio::spawn(play(stdin_r, stdout_w, out));
        Ok(Child {
            stdin: Some(Box::new(stdin_w)),
            stdout: Some(Box::new(stdout_r)),
            stderr: None,
            control: Arc::new(Ended),
        })
    }
}

async fn play(stdin: DuplexStream, mut stdout: DuplexStream, out: Vec<Out>) {
    let mut lines = BufReader::new(stdin).lines();
    let first = lines.next_line().await.unwrap().unwrap();
    let init: Value = serde_json::from_str(&first).unwrap();
    let id = init["request_id"].clone();
    let answer = json!({"type": "control_response",
        "response": {"subtype": "success", "request_id": id, "response": {}}});
    stdout
        .write_all(format!("{answer}\n").as_bytes())
        .await
        .unwrap();
    for o in out {
        match o {
            Out::Line(v) => stdout.write_all(format!("{v}\n").as_bytes()).await.unwrap(),
            Out::Huge(n) => {
                stdout
                    .write_all(b"{\"type\": \"user\", \"x\": \"")
                    .await
                    .unwrap();
                let block = vec![b'a'; 1 << 16];
                let mut left = n - 24;
                while left > 0 {
                    let k = left.min(block.len());
                    stdout.write_all(&block[..k]).await.unwrap();
                    left -= k;
                }
                stdout.write_all(b"\n").await.unwrap();
            }
        }
    }
    // the program ends: stdout closes
}

async fn run(out: Vec<Out>) -> Vec<Result<Value, String>> {
    let spawner = FakeClaude {
        out: std::sync::Mutex::new(Some(out)),
    };
    let mut environ = BTreeMap::new();
    environ.insert(
        "CLAUDE_AGENT_SDK_SKIP_VERSION_CHECK".to_string(),
        "1".to_string(),
    );
    let client = Client::connect(
        &spawner,
        "claude",
        &environ,
        &Options::default(),
        Arc::new(|| "abcdef01".to_string()),
    )
    .await
    .expect("connects");
    let mut got = Vec::new();
    while let Some(m) = client.next_raw().await {
        got.push(m.map_err(|e| format!("{e:?}")));
    }
    client.disconnect().await;
    got
}

/// A tool result as Claude Code 2.1.310 writes it: in the message's content and again as `tool_use_result`.
fn tool_result(text: &str) -> Value {
    json!({"type": "user", "uuid": "u-1", "session_id": "s",
        "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_1", "content": [{"type": "text", "text": text}]}]},
        "tool_use_result": [{"type": "text", "text": text}]})
}

fn result() -> Value {
    json!({"type": "result", "subtype": "success", "duration_ms": 1, "duration_api_ms": 1, "is_error": false,
        "num_turns": 1, "session_id": "s", "result": "done"})
}

// decision: RUN-473
#[tokio::test]
async fn a_three_mib_line_is_read_and_the_run_goes_on() {
    let big = "x".repeat(3 << 20);
    let got = run(vec![Out::Line(tool_result(&big)), Out::Line(result())]).await;
    assert_eq!(
        got.len(),
        2,
        "{:?}",
        got.iter().map(|g| g.is_ok()).collect::<Vec<_>>()
    );
    let user = got[0].as_ref().expect("the long line is a message");
    assert_eq!(
        user["message"]["content"][0]["content"][0]["text"]
            .as_str()
            .map(str::len),
        Some(3 << 20),
        "the tool result as the model got it"
    );
    assert_eq!(got[1].as_ref().unwrap()["type"], "result");
}

// decision: RUN-473
#[tokio::test]
async fn claude_codes_tool_use_result_echo_is_dropped() {
    let got = run(vec![Out::Line(tool_result("seen")), Out::Line(result())]).await;
    let user = got[0].as_ref().unwrap();
    assert!(user.get("tool_use_result").is_none(), "{user}");
    assert_eq!(
        user["message"]["content"][0]["tool_use_id"], "toolu_1",
        "the content stays"
    );
    // only a `user` message loses it
    let other = json!({"type": "assistant", "tool_use_result": 1});
    assert_eq!(without_echo(other.clone()), other);
}

/// A line past the memory guard is skipped (never held), and the messages after it arrive: no failure.
// decision: RUN-473
#[tokio::test]
async fn a_line_over_the_guard_is_skipped_and_the_run_goes_on() {
    let got = run(vec![
        Out::Line(tool_result("before")),
        Out::Huge(LINE_GUARD + 1000),
        Out::Line(tool_result("after")),
        Out::Line(result()),
    ])
    .await;
    let kinds: Vec<String> = got
        .iter()
        .map(|g| match g {
            Ok(v) => v["type"].as_str().unwrap_or("?").to_string(),
            Err(e) => format!("error {e}"),
        })
        .collect();
    assert_eq!(kinds, ["user", "user", "result"]);
    assert_eq!(
        got[1].as_ref().unwrap()["message"]["content"][0]["content"][0]["text"],
        "after"
    );
}

/// The framer itself, with a small guard: lines of any length up to it, split across chunks however they come; a
/// longer one reported once with its size, and the next line whole.
// decision: RUN-473
#[test]
fn the_framer_reads_any_split_and_skips_only_past_the_guard() {
    let mut f = Framer::new(10);
    let mut out = Vec::new();
    for chunk in [
        &b"abc"[..],
        b"defghij\nshort\n12345",
        b"678901234",
        b"5\nok",
        b"\n",
    ] {
        out.extend(f.push(chunk));
    }
    assert_eq!(
        out,
        vec![
            Framed::Line(b"abcdefghij".to_vec()),
            Framed::Line(b"short".to_vec()),
            Framed::Skipped(15),
            Framed::Line(b"ok".to_vec()),
        ]
    );
    let mut f = Framer::new(10);
    f.push(b"tail");
    assert_eq!(f.tail(), Some(b"tail".to_vec()));
}
