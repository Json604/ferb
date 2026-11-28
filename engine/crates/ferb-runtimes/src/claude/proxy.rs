//! The tool-list proxy for FERB's Claude runs (ferb/runtimes/proxy.py): bound to 127.0.0.1 and used only as the
//! ANTHROPIC_BASE_URL of runs FERB starts. It forwards only for a live run (the `/run/<id>/<secret>` prefix FERB
//! issued, compared in constant time; anything else gets 403, so a command in FERB's shell can't use it to reach
//! the API host), records ONLY the tool names of each request and the token counts of each answer (RUN-477; never
//! headers, prompts or reply text; nothing logged or written), and forwards everything else unchanged, except one
//! thing (RUN-470): for a run that asked for it
//! ([`ToolProxy::compact`], a worker's tool loop), the request's earlier screens and large reads are replaced by the
//! run's compaction plan before it goes on (crate::compact), the same bytes on every request until the next
//! compaction.
//!
//! ORCH-719 step 1: each model step's answer is read as it streams back for its usage only ([`super::tally`]), so a
//! run's tokens are counted per request and even when Claude Code never reports them (RUN-477). When Anthropic
//! refuses an edited request for its edited history (the history-editing check), the run's own unedited request goes
//! instead and the run is never edited again (RUN-478).
//!
//! T-1338 (the front door's step 1): beside the usage, each model step leaves a record of names, flags and times only
//! ([`super::steps`]): the tools its answer called, the name and error flag of each tool result its request carried,
//! the stop reason, a status that was not a success, and the times to the answer's first byte and end on the relay's
//! clock. A step whose request or answer was compressed, whose request is not JSON, or whose JSON answer was too
//! large to read leaves none.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use super::steps::{self, Step, Steps};
use super::tally::{history_refused, Tally, UsageReader};
use super::ToolProxy;
use crate::compact::{Clock, Compaction, Compactor, Policy};

/// The relay's stopwatch: seconds on a monotonic clock (crate::monotonic_now; a test's own).
pub type Monotonic = crate::codex::Clock;

pub const UPSTREAM: &str = "https://api.anthropic.com";

/// A request for the model's next step (a query string allowed; `count_tokens` is not a step, so it neither counts nor
/// changes).
fn model_step(path: &str) -> bool {
    path.split('?').next().unwrap_or_default() == "v1/messages"
}

const HOP: [&str; 6] = [
    "host",
    "content-length",
    "connection",
    "accept-encoding",
    "transfer-encoding",
    "content-encoding",
];

/// proxy.py `tool_names`: names from a request body's `tools` list; the body itself is discarded by the caller.
pub fn tool_names(body: &[u8]) -> BTreeSet<String> {
    let Some(Value::Object(data)) = std::str::from_utf8(body)
        .ok()
        .and_then(|t| ferb_py::json::loads(t).ok())
    else {
        return BTreeSet::new();
    };
    let Some(Value::Array(tools)) = data.get("tools") else {
        return BTreeSet::new();
    };
    tools
        .iter()
        .filter_map(Value::as_object)
        .map(|t| {
            [t.get("name"), t.get("type")]
                .into_iter()
                .flatten()
                .find(|v| ferb_py::value::truthy(v))
                .map(ferb_py::value::py_str)
                .unwrap_or_else(|| "?".into())
        })
        .collect()
}

#[derive(Default)]
struct Book {
    seen: HashMap<String, BTreeSet<String>>,
    live: HashMap<String, String>,
    /// RUN-470: the runs whose requests are compacted, and the compactions not yet reported.
    compactors: HashMap<String, Compactor>,
    made: HashMap<String, Vec<Compaction>>,
    /// ORCH-719: each run's model requests' usage since it was last taken.
    usage: HashMap<String, Tally>,
    /// RUN-478: the runs whose edited request Anthropic refused, refusals not yet reported.
    refused: HashMap<String, usize>,
    /// T-1338: each run's model steps since they were last taken.
    steps: HashMap<String, Steps>,
    /// ORCH-760: each run's newest model request's whole prompt, in tokens.
    context: HashMap<String, i64>,
}

pub struct ToolListProxy {
    upstream: String,
    clock: Clock,
    port: u16,
    book: Arc<Mutex<Book>>,
    server: JoinHandle<()>,
    secret: Arc<dyn Fn() -> String + Send + Sync>,
}

impl ToolListProxy {
    /// Binds 127.0.0.1 on a free port and serves until `close` (or drop).
    pub async fn start(
        upstream: &str,
        secret: Arc<dyn Fn() -> String + Send + Sync>,
    ) -> std::io::Result<ToolListProxy> {
        Self::start_with_clock(upstream, secret, crate::compact::local_clock()).await
    }

    /// `start` with the clock that names the time in a compaction's lines.
    pub async fn start_with_clock(
        upstream: &str,
        secret: Arc<dyn Fn() -> String + Send + Sync>,
        clock: Clock,
    ) -> std::io::Result<ToolListProxy> {
        Self::start_with_clocks(upstream, secret, clock, Arc::new(crate::monotonic_now)).await
    }

    /// `start_with_clock` with the stopwatch that times each model step too.
    pub async fn start_with_clocks(
        upstream: &str,
        secret: Arc<dyn Fn() -> String + Send + Sync>,
        clock: Clock,
        monotonic: Monotonic,
    ) -> std::io::Result<ToolListProxy> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let book = Arc::new(Mutex::new(Book::default()));
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .no_gzip()
            .build()
            .map_err(std::io::Error::other)?;
        let upstream_url = upstream.trim_end_matches('/').to_string();
        let (b, u) = (book.clone(), upstream_url.clone());
        let server = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let (b, u, c, m) = (b.clone(), u.clone(), client.clone(), monotonic.clone());
                tokio::spawn(async move {
                    let _ = serve(stream, b, u, c, m).await;
                });
            }
        });
        Ok(ToolListProxy {
            upstream: upstream_url,
            clock,
            port,
            book,
            server,
            secret,
        })
    }

    pub fn upstream(&self) -> &str {
        &self.upstream
    }

    pub fn close(&self) {
        self.server.abort();
    }
}

impl Drop for ToolListProxy {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl ToolProxy for ToolListProxy {
    fn base_url(&self, run_id: &str) -> String {
        let mut book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        let secret = book
            .live
            .entry(run_id.to_string())
            .or_insert_with(|| (self.secret)())
            .clone();
        format!("http://127.0.0.1:{}/run/{run_id}/{secret}", self.port)
    }

    fn seen(&self, run_id: &str) -> BTreeSet<String> {
        let book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        book.seen.get(run_id).cloned().unwrap_or_default()
    }

    fn forget(&self, run_id: &str) {
        let mut book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        book.seen.remove(run_id);
        book.live.remove(run_id);
        book.compactors.remove(run_id);
        book.made.remove(run_id);
        book.usage.remove(run_id);
        book.refused.remove(run_id);
        book.steps.remove(run_id);
        book.context.remove(run_id);
    }

    fn compact(&self, run_id: &str, policy: Policy) {
        let mut book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        book.compactors
            .entry(run_id.to_string())
            .or_insert_with(|| Compactor::new(policy, self.clock.clone()));
    }

    fn compactions(&self, run_id: &str) -> Vec<Compaction> {
        let mut book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        book.made.remove(run_id).unwrap_or_default()
    }

    fn take_usage(&self, run_id: &str) -> Option<Tally> {
        let mut book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        book.usage.remove(run_id).filter(|t| !t.is_empty())
    }

    fn history_refusals(&self, run_id: &str) -> usize {
        let mut book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        book.refused.remove(run_id).unwrap_or(0)
    }

    fn take_steps(&self, run_id: &str) -> Option<Steps> {
        let mut book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        book.steps.remove(run_id).filter(|s| !s.is_empty())
    }

    fn context_tokens(&self, run_id: &str) -> Option<i64> {
        let book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        book.context.get(run_id).copied()
    }
}

/// One model step being timed: when its request arrived and what it carried back.
struct Timing {
    monotonic: Monotonic,
    arrived: f64,
    results: Vec<steps::ToolResult>,
    first_byte_ms: Option<i64>,
}

impl Timing {
    fn ms(&self) -> i64 {
        let ms = ((self.monotonic)() - self.arrived) * 1000.0;
        if ms.is_finite() && ms > 0.0 {
            ms.round() as i64
        } else {
            0
        }
    }

    fn first_byte(&mut self) {
        if self.first_byte_ms.is_none() {
            self.first_byte_ms = Some(self.ms());
        }
    }

    /// The step, ended now, kept for the run.
    fn record(self, book: &Mutex<Book>, run_id: &str, status: u16, answer: steps::Answer) {
        let done_ms = self.ms();
        let step = Step {
            called: answer.called,
            results: self.results,
            stop: answer.stop,
            status,
            first_byte_ms: self.first_byte_ms,
            done_ms,
        };
        let mut b = book.lock().unwrap_or_else(|e| e.into_inner());
        b.steps.entry(run_id.to_string()).or_default().push(step);
    }
}

/// `secrets.token_urlsafe(24)`.
pub fn token_urlsafe() -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut bytes = [0u8; 24];
    let _ = getrandom::fill(&mut bytes);
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]);
        for i in 0..4 {
            out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// RUN-470: the run's plan applied to one request body; `None` keeps the body's own bytes (no plan yet, a body that
/// isn't JSON, or one the client compressed).
fn compacted(book: &Mutex<Book>, run_id: &str, body: &[u8]) -> Option<Vec<u8>> {
    let mut book = book.lock().unwrap_or_else(|e| e.into_inner());
    let Book {
        compactors, made, ..
    } = &mut *book;
    let c = compactors.get_mut(run_id)?;
    let mut value: Value = serde_json::from_slice(body).ok()?;
    let results = crate::api::anthropic::results(&value);
    crate::api::anthropic::rewrite(&mut value, c.plan());
    let tokens = crate::compact::estimate(&value);
    if let Some(m) = c.step(&results, tokens) {
        crate::api::anthropic::rewrite(&mut value, c.plan());
        made.entry(run_id.to_string()).or_default().push(m);
    }
    if c.plan().is_empty() {
        return None;
    }
    serde_json::to_vec(&value).ok()
}

async fn serve(
    stream: TcpStream,
    book: Arc<Mutex<Book>>,
    upstream: String,
    client: reqwest::Client,
    monotonic: Monotonic,
) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    loop {
        // the request line and headers
        let mut line = String::new();
        if read.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        let arrived = monotonic();
        let mut parts = line.trim_end().splitn(3, ' ');
        let (method, path) = (
            parts.next().unwrap_or_default().to_string(),
            parts.next().unwrap_or_default().to_string(),
        );
        let mut headers: Vec<(String, String)> = Vec::new();
        loop {
            let mut h = String::new();
            if read.read_line(&mut h).await? == 0 {
                return Ok(());
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                headers.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
        let trimmed = path.trim_start_matches('/');
        let (prefix, rest) = trimmed.split_once('/').unwrap_or((trimmed, ""));
        let (run_id, rest) = if prefix == "run" {
            rest.split_once('/').unwrap_or((rest, ""))
        } else {
            ("", "")
        };
        let (secret, upstream_path) = rest.split_once('/').unwrap_or((rest, ""));
        let allowed = {
            let b = book.lock().unwrap_or_else(|e| e.into_inner());
            b.live
                .get(run_id)
                .is_some_and(|expected| constant_eq(expected.as_bytes(), secret.as_bytes()))
        };
        if !(method == "GET" || method == "POST") || !allowed {
            let code = if allowed { 501 } else { 403 };
            let reason = if allowed {
                "Unsupported method"
            } else {
                "Forbidden"
            };
            write
                .write_all(
                    format!("HTTP/1.1 {code} {reason}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
                        .as_bytes(),
                )
                .await?;
            return Ok(());
        }
        let length: usize = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.parse().ok())
            .unwrap_or(0);
        let mut body = vec![0u8; length];
        read.read_exact(&mut body).await?;
        if !body.is_empty() && upstream_path.starts_with("v1/messages") {
            let names = tool_names(&body); // the only thing kept
            book.lock()
                .unwrap_or_else(|e| e.into_inner())
                .seen
                .entry(run_id.to_string())
                .or_default()
                .extend(names);
        }
        let encoding = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-encoding"))
            .map(|(_, v)| v.clone());
        let step = !body.is_empty() && encoding.is_none() && model_step(upstream_path);
        // T-1338: the step's record, from the request as Claude Code sent it (None: not a JSON request)
        let mut timing = step
            .then(|| steps::results(&body))
            .flatten()
            .map(|results| Timing {
                monotonic: monotonic.clone(),
                arrived,
                results,
                first_byte_ms: None,
            });
        // RUN-478: the run's own request, kept while an edited one is out
        let mut unedited: Option<Vec<u8>> = None;
        if step {
            if let Some(new) = compacted(&book, run_id, &body) {
                unedited = Some(std::mem::replace(&mut body, new));
            }
        }
        let out = Outgoing {
            client: &client,
            method: &method,
            url: format!("{upstream}/{upstream_path}"),
            headers: &headers,
            encoding: encoding.as_deref(),
        };
        let Ok(mut resp) = out.send(body).await else {
            if let Some(t) = timing.take() {
                t.record(&book, run_id, 502, steps::Answer::default());
            }
            write
                .write_all(
                    b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
                )
                .await?;
            return Ok(());
        };
        if step && unedited.is_some() && resp.status().as_u16() == 400 {
            // RUN-478: read the refusal whole (it is small) to see whether it is Anthropic's history-editing check
            let head = resp.headers().clone();
            let Ok(answer) = resp.bytes().await else {
                write
                    .write_all(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
                    .await?;
                return Ok(());
            };
            if !history_refused(400, &answer) {
                if let Some(mut t) = timing.take() {
                    t.first_byte();
                    t.record(&book, run_id, 400, steps::Answer::default());
                }
                relay_whole(&mut write, 400, &head, &answer).await?;
                continue;
            }
            {
                let mut b = book.lock().unwrap_or_else(|e| e.into_inner());
                b.compactors.remove(run_id); // never edited again
                *b.refused.entry(run_id.to_string()).or_default() += 1;
            }
            let Ok(again) = out.send(unedited.take().unwrap_or_default()).await else {
                if let Some(t) = timing.take() {
                    t.record(&book, run_id, 502, steps::Answer::default());
                }
                write
                    .write_all(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
                    .await?;
                return Ok(());
            };
            resp = again;
        }
        let status = resp.status();
        let mut head = format!(
            "HTTP/1.1 {} {}\r\n",
            status.as_u16(),
            status.canonical_reason().unwrap_or("")
        );
        for (k, v) in resp.headers() {
            if !HOP.contains(&k.as_str().to_lowercase().as_str()) {
                head.push_str(&format!("{}: {}\r\n", k, v.to_str().unwrap_or_default()));
            }
        }
        head.push_str("transfer-encoding: chunked\r\n\r\n");
        write.write_all(head.as_bytes()).await?;
        // ORCH-719: a model step's answer is read for its usage as it passes (an answer the upstream compressed is not)
        let compressed = resp.headers().contains_key("content-encoding");
        let sse = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.trim_start().starts_with("text/event-stream"));
        let mut reader =
            (step && status.is_success() && !compressed).then(|| UsageReader::new(sse));
        if compressed {
            timing = None; // T-1338: an answer not read gets no record rather than a wrong one
        }
        let mut resp = resp;
        while let Ok(Some(chunk)) = resp.chunk().await {
            if let Some(t) = timing.as_mut() {
                t.first_byte();
            }
            if let Some(r) = reader.as_mut() {
                r.feed(&chunk);
            }
            write
                .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                .await?;
            write.write_all(&chunk).await?;
            write.write_all(b"\r\n").await?;
            write.flush().await?;
        }
        let (tally, answer) = match reader {
            Some(r) => r.finish_step(),
            // a refusal or failure: its body is not read, the step is still kept with its status
            None => (None, Some(steps::Answer::default())),
        };
        if let Some(t) = tally {
            let mut b = book.lock().unwrap_or_else(|e| e.into_inner());
            // ORCH-760: one request's whole prompt is the conversation's size as of that step
            b.context
                .insert(run_id.to_string(), t.input + t.cache_read + t.cache_write);
            b.usage.entry(run_id.to_string()).or_default().add(&t);
        }
        if let (Some(t), Some(answer)) = (timing, answer) {
            t.record(&book, run_id, status.as_u16(), answer);
        }
        write.write_all(b"0\r\n\r\n").await?;
        write.flush().await?;
    }
}

/// One request as the relay sends it on: hop-by-hop headers dropped, a compressed body with its encoding.
struct Outgoing<'a> {
    client: &'a reqwest::Client,
    method: &'a str,
    url: String,
    headers: &'a [(String, String)],
    encoding: Option<&'a str>,
}

impl Outgoing<'_> {
    async fn send(&self, body: Vec<u8>) -> reqwest::Result<reqwest::Response> {
        let mut req = self.client.request(
            if self.method == "GET" {
                reqwest::Method::GET
            } else {
                reqwest::Method::POST
            },
            &self.url,
        );
        for (k, v) in self.headers {
            if !HOP.contains(&k.to_lowercase().as_str()) {
                req = req.header(k, v);
            }
        }
        if let Some(e) = self.encoding {
            // a body the client compressed goes on as it came, so its encoding goes with it
            req = req.header("content-encoding", e);
        }
        req.body(body).send().await
    }
}

/// An answer the relay read whole, written back as it came (chunked, like every answer it streams).
async fn relay_whole(
    write: &mut tokio::net::tcp::OwnedWriteHalf,
    status: u16,
    headers: &reqwest::header::HeaderMap,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = reqwest::StatusCode::from_u16(status)
        .ok()
        .and_then(|s| s.canonical_reason())
        .unwrap_or("");
    let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
    for (k, v) in headers {
        if !HOP.contains(&k.as_str().to_lowercase().as_str()) {
            head.push_str(&format!("{}: {}\r\n", k, v.to_str().unwrap_or_default()));
        }
    }
    head.push_str("transfer-encoding: chunked\r\n\r\n");
    write.write_all(head.as_bytes()).await?;
    if !body.is_empty() {
        write
            .write_all(format!("{:x}\r\n", body.len()).as_bytes())
            .await?;
        write.write_all(body).await?;
        write.write_all(b"\r\n").await?;
    }
    write.write_all(b"0\r\n\r\n").await?;
    write.flush().await
}
