//! The tool-list proxy for FERB's Claude runs (ferb/runtimes/proxy.py): bound to 127.0.0.1 and used only as the
//! ANTHROPIC_BASE_URL of runs FERB starts. It forwards only for a live run (the `/run/<id>/<secret>` prefix FERB
//! issued, compared in constant time; anything else gets 403, so a command in FERB's shell can't use it to reach
//! the API host), records ONLY the tool names of each request (never headers, prompts or replies; nothing logged
//! or written), and forwards everything else unchanged.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use super::ToolProxy;

pub const UPSTREAM: &str = "https://api.anthropic.com";
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
}

pub struct ToolListProxy {
    upstream: String,
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
                let (b, u, c) = (b.clone(), u.clone(), client.clone());
                tokio::spawn(async move {
                    let _ = serve(stream, b, u, c).await;
                });
            }
        });
        Ok(ToolListProxy {
            upstream: upstream_url,
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

async fn serve(
    stream: TcpStream,
    book: Arc<Mutex<Book>>,
    upstream: String,
    client: reqwest::Client,
) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    loop {
        // the request line and headers
        let mut line = String::new();
        if read.read_line(&mut line).await? == 0 {
            return Ok(());
        }
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
        let mut req = client.request(
            if method == "GET" {
                reqwest::Method::GET
            } else {
                reqwest::Method::POST
            },
            format!("{upstream}/{upstream_path}"),
        );
        for (k, v) in &headers {
            if !HOP.contains(&k.to_lowercase().as_str()) {
                req = req.header(k, v);
            }
        }
        let resp = match req.body(body).send().await {
            Ok(r) => r,
            Err(_) => {
                write
                    .write_all(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
                    .await?;
                return Ok(());
            }
        };
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
        let mut resp = resp;
        while let Ok(Some(chunk)) = resp.chunk().await {
            write
                .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                .await?;
            write.write_all(&chunk).await?;
            write.write_all(b"\r\n").await?;
            write.flush().await?;
        }
        write.write_all(b"0\r\n\r\n").await?;
        write.flush().await?;
    }
}
