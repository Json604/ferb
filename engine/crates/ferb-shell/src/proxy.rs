//! FERB's egress proxy for its shell (ferb/act/shell/proxy.py; S2, D28): the only way off this Mac for a contained
//! command. Per process it records each destination's host, port, byte counts and connections, never bodies, headers,
//! paths or query strings, and nothing is logged.
//!
//! - Each shell process has its own credential (its process id and a random token kept in memory only); the token is
//!   a secret value the redactor scrubs, and it never appears in an error, a `Debug` or a log.
//! - `allow(proc, host, port)` is asked before any name is resolved (proxy.py:277): a refused name never reaches DNS.
//! - This Mac's own addresses are refused, except explicit local destinations checked by `LocalAccess` before dialling
//!   and against the exact connected peer before a byte is forwarded (local.rs).
//! - CONNECT tunnels any TCP port; a TLS ClientHello's SNI is checked like the CONNECT host when `inspect_sni` says so.
//!
//! The HTTP parsing is Python 3.12's `http.server.BaseHTTPRequestHandler` (request line, headers through the email
//! parser, `send_error`'s page), so every reply a client gets is the same bytes, except that no `Server:` header is
//! sent (Python's names its own HTTP library). Threads, as in Python: one per connection, an accept loop and the
//! reaper that closes connections that don't authenticate in time.

use std::collections::{HashMap, HashSet};
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::local::{LocalAccess, LocalPeer, LocalRefused};
use crate::pynet::{ip_address, urlsplit, Ip};
use crate::pyos::{int_hex_bytes, int_str, repr_latin1, split_ws, strip};

pub const ENDED: &str = "this command has ended; its proxy credential no longer opens connections";
pub const OWN: &str =
    "this Mac's own address: a command reaches this Mac's servers directly, and only ones started in FERB's shell";
/// Not forwarded: they describe this hop, or carry FERB's credential (RFC 9110 7.6.1).
pub const HOP: [&str; 9] = [
    "proxy-authorization",
    "proxy-connection",
    "connection",
    "keep-alive",
    "te",
    "trailer",
    "upgrade",
    "expect",
    "proxy-authenticate",
];
pub const CHUNK: usize = 65536;
/// S4: FERB's reasons the shell tool matches by their exact words.
pub const DEST_REFUSED_NO: &str = "you said no to this place in this task";
pub const DEST_SNI_NONE: &str = "a TLS connection whose server name FERB couldn't read";
/// The most of a client's ClientHello FERB reassembles.
pub const SNI_MAX: usize = 16 * 1024;
/// Seconds from accept until a connection must authenticate (an absolute deadline).
pub const UNAUTH_TIMEOUT: f64 = 10.0;
/// Connections not yet authenticated held at once; past it the oldest is closed.
pub const MAX_UNAUTH: usize = 1024;
/// A response head larger than this is malformed: 502.
pub const MAX_HEAD: usize = 256 * 1024;
/// A complete handshake record that isn't a ClientHello.
pub const NOT_HELLO: &str = "\x00not-a-client-hello";

/// Why the proxy won't or can't connect to a destination (FERB's words; recorded under `refused`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Unreachable(pub String);

pub type Allow = Arc<dyn Fn(&str, &str, u16) -> (bool, String) + Send + Sync>;
pub type Resolve = Arc<dyn Fn(&str, u16) -> Result<Vec<SocketAddr>, Unreachable> + Send + Sync>;
pub type Report = Arc<dyn Fn(&str, Map<String, Value>) + Send + Sync>;
type HostCheck = Arc<dyn Fn(&str) -> bool + Send + Sync>;
type SniCheck = Arc<dyn Fn(&str, &str, u16) -> bool + Send + Sync>;
type OnDestination = Arc<dyn Fn(&str, &str, u16) + Send + Sync>;

/// The redactor's registry of FERB's own secret values (GATE-573): the proxy's tokens go in and come out.
pub trait Secrets: Send + Sync {
    fn add_secret_value(&self, value: &str);
    fn forget_secret_value(&self, value: &str);
}

impl Secrets for ferb_sanitize::Redactor {
    fn add_secret_value(&self, value: &str) {
        ferb_sanitize::Redactor::add_secret_value(self, value)
    }
    fn forget_secret_value(&self, value: &str) {
        ferb_sanitize::Redactor::forget_secret_value(self, value)
    }
}

// ---------- address decisions ----------

/// One of this Mac's own addresses: loopback, unspecified, or bound to an interface (proxy.py:89).
pub fn this_mac(ip: &str) -> bool {
    let Some(addr) = ip_address(ip.split('%').next().unwrap_or("")) else {
        return false;
    };
    let addr = addr.unmapped();
    if addr.is_loopback() || addr.is_unspecified() {
        return true;
    }
    // only an own address can be bound (the scoped text as given, as Python binds `ip`)
    let bind: Option<SocketAddr> = match addr {
        Ip::V4(o) => Some(SocketAddr::from((o, 0))),
        Ip::V6(_) => format!("[{ip}]:0").parse().ok().or_else(|| {
            let h = match addr {
                Ip::V6(h) => h,
                Ip::V4(_) => unreachable!(),
            };
            Some(SocketAddr::from((std::net::Ipv6Addr::from(h), 0)))
        }),
    };
    bind.is_some_and(|a| std::net::TcpListener::bind(a).is_ok())
}

/// Whether the host is an IP literal (a scope after "%" ignored).
pub fn literal(host: &str) -> bool {
    ip_address(host.split('%').next().unwrap_or("")).is_some()
}

/// Only explicit local destinations: no arbitrary DNS name gets resolved before the destination gate.
pub fn local_host(host: &str) -> bool {
    ferb_py::text::lower(host.trim_end_matches('.')) == "localhost"
        || (literal(host) && this_mac(host))
}

/// Where an explicit local destination is dialled: localhost never causes DNS; a literal exactly as supplied.
pub fn local_addresses(host: &str, port: u16) -> Vec<(bool, String, u16)> {
    if ferb_py::text::lower(host.trim_end_matches('.')) == "localhost" {
        return vec![
            (true, "::1".into(), port),
            (false, "127.0.0.1".into(), port),
        ];
    }
    vec![(host.contains(':'), host.to_string(), port)]
}

fn sockaddr(v6: bool, host: &str, port: u16) -> Option<SocketAddr> {
    let bare = host.split('%').next().unwrap_or(host);
    match ip_address(bare)? {
        Ip::V4(o) if !v6 => Some(SocketAddr::from((o, port))),
        Ip::V6(h) if v6 => {
            let scope = host
                .split_once('%')
                .and_then(|(_, s)| s.parse::<u32>().ok().or_else(|| if_index(s)))
                .unwrap_or(0);
            Some(SocketAddr::V6(std::net::SocketAddrV6::new(
                h.into(),
                port,
                0,
                scope,
            )))
        }
        _ => None,
    }
}

fn if_index(name: &str) -> Option<u32> {
    let c = std::ffi::CString::new(name).ok()?;
    // SAFETY: a NUL-terminated interface name; the call only reads it.
    let i = unsafe { libc::if_nametoindex(c.as_ptr()) };
    (i != 0).then_some(i)
}

/// Where the proxy may connect for host:port: resolved here, never one of this Mac's own addresses (proxy.py:109).
pub fn addresses(host: &str, port: u16) -> Result<Vec<SocketAddr>, Unreachable> {
    use std::net::ToSocketAddrs;
    let found: Vec<SocketAddr> = match (host, port).to_socket_addrs() {
        Ok(it) => crate::pyos::dedupe(it),
        Err(_) => return Err(Unreachable("couldn't resolve the name (gaierror)".into())),
    };
    if found.iter().any(|a| this_mac(&a.ip().to_string())) {
        return Err(Unreachable(OWN.into()));
    }
    Ok(found)
}

// ---------- TLS ClientHello ----------

fn be(b: &[u8]) -> usize {
    b.iter().fold(0usize, |n, &x| n << 8 | x as usize)
}

/// The SNI host in a complete ClientHello body (after its 4-byte handshake header), lowercased; "" when none.
/// Slices clamp as Python's do (a short slice reads as a smaller number); an index past the end is "".
pub fn sni_of(body: &[u8]) -> String {
    let sl =
        |a: usize, b: usize| &body[a.min(body.len())..b.min(body.len()).max(a.min(body.len()))];
    let run = || -> Option<String> {
        let mut p = 2 + 32;
        p += 1 + *body.get(p)? as usize;
        p += 2 + be(sl(p, p + 2));
        p += 1 + *body.get(p)? as usize;
        if p + 2 > body.len() {
            return Some(String::new());
        }
        let end = p + 2 + be(sl(p, p + 2));
        p += 2;
        while p + 4 <= end {
            let etype = be(sl(p, p + 2));
            let elen = be(sl(p + 2, p + 4));
            p += 4;
            if etype == 0 {
                let nlen = be(sl(p + 3, p + 5));
                let name = sl(p + 5, p + 5 + nlen);
                if !name.is_ascii() {
                    return None;
                }
                return Some(String::from_utf8_lossy(name).to_ascii_lowercase());
            }
            p += elen;
        }
        Some(String::new())
    };
    run().unwrap_or_default()
}

/// Reassembles a ClientHello across TLS handshake records incrementally (proxy.py:171): `feed` returns None while
/// more bytes are needed, NOT_HELLO once a complete record shows it isn't one, "" for a hello without an SNI, else
/// the lowercased SNI host.
#[derive(Debug, Default)]
pub struct HelloReader {
    buf: Vec<u8>,
    pos: usize,
    hs: Vec<u8>,
    pub read: usize,
}

impl HelloReader {
    pub fn new() -> HelloReader {
        HelloReader::default()
    }

    pub fn feed(&mut self, data: &[u8]) -> Option<String> {
        self.buf.extend_from_slice(data);
        self.read += data.len();
        while self.pos + 5 <= self.buf.len() {
            if self.buf[self.pos] != 0x16 {
                return Some(NOT_HELLO.to_string());
            }
            let rec_len = be(&self.buf[self.pos + 3..self.pos + 5]);
            if self.pos + 5 + rec_len > self.buf.len() {
                return None;
            }
            let rec = self.buf[self.pos + 5..self.pos + 5 + rec_len].to_vec();
            self.hs.extend_from_slice(&rec);
            self.pos += 5 + rec_len;
            if !self.hs.is_empty() && self.hs[0] != 0x01 {
                return Some(NOT_HELLO.to_string());
            }
            if self.hs.len() >= 4 {
                let n = be(&self.hs[1..4]);
                if self.hs.len() >= 4 + n {
                    return Some(sni_of(&self.hs[4..4 + n]));
                }
            }
        }
        None
    }
}

// ---------- credentials ----------

/// `base64.b64decode(text, validate=True)`: ASCII, the standard alphabet, padding only at the end and complete.
pub fn b64decode_strict(text: &str) -> Option<Vec<u8>> {
    if !text.is_ascii() {
        return None;
    }
    let b = text.as_bytes();
    let pads = b.iter().rev().take_while(|&&c| c == b'=').count();
    if pads > 2
        || b[..b.len() - pads]
            .iter()
            .any(|&c| !(c.is_ascii_alphanumeric() || c == b'+' || c == b'/'))
    {
        return None;
    }
    if !b.len().is_multiple_of(4) {
        return None;
    }
    let val = |c: u8| -> u32 {
        match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            _ => 0,
        }
    };
    let mut out = Vec::new();
    for quad in b.chunks(4) {
        let data: Vec<u32> = quad
            .iter()
            .filter(|&&c| c != b'=')
            .map(|&c| val(c))
            .collect();
        let n = data.iter().fold(0u32, |acc, v| acc << 6 | v) << (6 * (4 - data.len()));
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..data.len().saturating_sub(1)]);
    }
    Some(out)
}

fn b64encode(data: &[u8], alphabet: &[u8; 64], pad: bool) -> String {
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        let chars = chunk.len() + 1;
        for i in 0..4 {
            if i < chars {
                out.push(alphabet[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else if pad {
                out.push('=');
            }
        }
    }
    out
}

const STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URLSAFE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// `base64.b64encode(data).decode()`.
pub fn b64encode_std(data: &[u8]) -> String {
    b64encode(data, STD, true)
}

/// `secrets.token_urlsafe(n)`.
pub fn token_urlsafe(n: usize) -> String {
    b64encode(&crate::random_bytes(n), URLSAFE, false)
}

/// The process a `Proxy-Authorization` value names, when its token is the one FERB issued (proxy.py:249).
pub fn who(value: &str, tokens: &HashMap<String, String>) -> Option<String> {
    let (scheme, raw) = value.split_once(' ').unwrap_or((value, ""));
    if ferb_py::text::lower(scheme) != "basic" {
        return None;
    }
    let decoded = b64decode_strict(strip(raw))?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, token) = text.split_once(':').unwrap_or((&text, ""));
    let expected = tokens.get(user)?;
    if !token.is_ascii() || !constant_eq(expected.as_bytes(), token.as_bytes()) {
        return None;
    }
    Some(user.to_string())
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() != b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).unwrap_or(&0) ^ b.get(i).unwrap_or(&0);
    }
    diff == 0
}

// ---------- facts ----------

#[derive(Debug, Default, Clone)]
struct Counts {
    bytes_out: i64,
    bytes_in: i64,
    connections: i64,
}

#[derive(Debug, Default)]
struct Facts {
    destinations: std::collections::BTreeMap<(String, u16), Counts>,
    refused: std::collections::BTreeMap<(String, u16, String), i64>,
    local: std::collections::BTreeMap<(String, u16, String, String), i64>,
}

/// An insertion-ordered map (Python's dict) on a small key set.
struct Ordered<K, V>(Vec<(K, V)>);

impl<K: PartialEq, V> Ordered<K, V> {
    fn entry(&mut self, k: K, default: impl FnOnce() -> V) -> &mut V {
        let i = match self.0.iter().position(|(key, _)| *key == k) {
            Some(i) => i,
            None => {
                self.0.push((k, default()));
                self.0.len() - 1
            }
        };
        &mut self.0[i].1
    }
}

fn facts_json(f: Option<&Facts>, excluded: &dyn Fn(&str) -> bool) -> Map<String, Value> {
    let empty = Facts::default();
    let f = f.unwrap_or(&empty);
    let shown = |h: &str| {
        if excluded(h) {
            "excluded".to_string()
        } else {
            h.to_string()
        }
    };
    let mut dests: Ordered<(String, u16), Counts> = Ordered(Vec::new());
    for ((host, port), v) in &f.destinations {
        let c = dests.entry((shown(host), *port), Counts::default);
        c.bytes_out += v.bytes_out;
        c.bytes_in += v.bytes_in;
        c.connections += v.connections;
    }
    let mut refused: Ordered<(String, u16, String), i64> = Ordered(Vec::new());
    for ((host, port, why), count) in &f.refused {
        *refused.entry((shown(host), *port, why.clone()), || 0) += count;
    }
    let mut local: Ordered<(String, u16), Vec<Value>> = Ordered(Vec::new());
    for ((host, port, address, executable), count) in &f.local {
        let s = shown(host);
        let address = if s == "excluded" {
            "excluded".to_string()
        } else {
            address.clone()
        };
        local
            .entry((s, *port), Vec::new)
            .push(json!({"address": address, "executable": executable, "connections": count}));
    }
    let mut out = Map::new();
    let destinations: Vec<Value> = dests
        .0
        .iter()
        .map(|((h, p), v)| {
            let mut m = Map::new();
            m.insert("host".into(), json!(h));
            m.insert("port".into(), json!(p));
            m.insert("bytes_out".into(), json!(v.bytes_out));
            m.insert("bytes_in".into(), json!(v.bytes_in));
            m.insert("connections".into(), json!(v.connections));
            if let Some((_, conns)) = local.0.iter().find(|(k, _)| k.0 == *h && k.1 == *p) {
                m.insert("local_connections".into(), Value::Array(conns.clone()));
            }
            Value::Object(m)
        })
        .collect();
    out.insert("destinations".into(), Value::Array(destinations));
    out.insert(
        "refused".into(),
        Value::Array(
            refused
                .0
                .iter()
                .map(|((h, p, w), c)| json!({"host": h, "port": p, "why": w, "count": c}))
                .collect(),
        ),
    );
    out
}

// ---------- the proxy ----------

/// What the daemon gives the proxy (proxy.py:210's arguments). `allow` is the gateway's destination gate.
pub struct ProxyConfig {
    pub allow: Allow,
    pub resolve: Resolve,
    pub redactor: Option<Arc<dyn Secrets>>,
    /// The owner's exclusion list: such a host is never shown.
    pub excluded: HostCheck,
    /// A process is fully done: its final record.
    pub on_report: Option<Report>,
    /// S4: whether to hold a CONNECT's ClientHello to check its SNI (the daemon: always).
    pub inspect_sni: SniCheck,
    pub local_access: Option<Arc<LocalAccess>>,
    /// Standing local permission (a final No still wins); defaults to `allow`.
    pub allow_local: Option<Allow>,
    /// A process's first connection to a host and port, as it opens.
    pub on_destination: Option<OnDestination>,
    pub unauth_timeout: Duration,
    pub max_unauth: usize,
}

impl ProxyConfig {
    pub fn new(allow: Allow) -> ProxyConfig {
        ProxyConfig {
            allow,
            resolve: Arc::new(addresses),
            redactor: None,
            excluded: Arc::new(|_| false),
            on_report: None,
            inspect_sni: Arc::new(|_, _, _| false),
            local_access: None,
            allow_local: None,
            on_destination: None,
            unauth_timeout: Duration::from_secs_f64(UNAUTH_TIMEOUT),
            max_unauth: MAX_UNAUTH,
        }
    }
}

#[derive(Default)]
struct State {
    tokens: HashMap<String, String>,
    facts: HashMap<String, Facts>,
    active: HashMap<String, i64>,
    ending: HashSet<String>,
    pending: HashMap<u64, (Instant, TcpStream)>,
    next_conn: u64,
    shut: bool,
}

struct Inner {
    cfg: ProxyConfig,
    on_report: Mutex<Option<Report>>,
    state: Mutex<State>,
    wake: Condvar,
    listener: TcpListener,
    port: u16,
    thread: Mutex<Option<JoinHandle<()>>>,
    restarts: AtomicU64,
    last_error: Mutex<Option<String>>,
    shut: AtomicBool,
}

pub struct EgressProxy {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for EgressProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EgressProxy")
            .field("port", &self.inner.port)
            .finish_non_exhaustive()
    }
}

fn hang_up(s: &TcpStream) {
    let _ = s.shutdown(Shutdown::Both);
}

fn py_error_name(e: &std::io::Error) -> &'static str {
    match e.kind() {
        ErrorKind::BrokenPipe => "BrokenPipeError",
        ErrorKind::ConnectionReset => "ConnectionResetError",
        ErrorKind::ConnectionAborted => "ConnectionAbortedError",
        ErrorKind::TimedOut => "TimeoutError",
        _ => "OSError",
    }
}

impl EgressProxy {
    /// Binds 127.0.0.1 on a free port and starts serving.
    pub fn new(cfg: ProxyConfig) -> std::io::Result<EgressProxy> {
        let listener = bind_listener()?;
        let port = listener.local_addr()?.port();
        let mut cfg = cfg;
        let on_report = Mutex::new(cfg.on_report.take());
        let inner = Arc::new(Inner {
            cfg,
            on_report,
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            listener,
            port,
            thread: Mutex::new(None),
            restarts: AtomicU64::new(0),
            last_error: Mutex::new(None),
            shut: AtomicBool::new(false),
        });
        let reaper = inner.clone();
        std::thread::Builder::new()
            .name("ferb-proxy-reaper".into())
            .spawn(move || reaper.reap())?;
        let proxy = EgressProxy { inner };
        proxy.ensure_serving();
        Ok(proxy)
    }

    pub fn port(&self) -> u16 {
        self.inner.port
    }

    /// Where a finished process's final record goes (the shell manager wires itself in after both exist).
    pub fn set_on_report(&self, f: Report) {
        *self
            .inner
            .on_report
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(f);
    }

    /// Start the serving thread, or restart it if it died; true when it restarted a dead one. Never after close().
    pub fn ensure_serving(&self) -> bool {
        let mut t = self.inner.thread.lock().unwrap_or_else(|e| e.into_inner());
        if self.inner.shut.load(Ordering::SeqCst) || t.as_ref().is_some_and(|h| !h.is_finished()) {
            return false;
        }
        let restarted = t.is_some();
        if restarted {
            self.inner.restarts.fetch_add(1, Ordering::SeqCst);
        }
        let inner = self.inner.clone();
        *t = std::thread::Builder::new()
            .name("ferb-proxy".into())
            .spawn(move || inner.serve())
            .ok();
        restarted
    }

    /// `ferb doctor`'s liveness check: an unauthenticated request must get 407.
    pub fn probe(&self) -> Map<String, Value> {
        let restarted = self.ensure_serving();
        let mut alive = false;
        if !self.inner.shut.load(Ordering::SeqCst) {
            alive = (|| -> std::io::Result<bool> {
                let mut c = TcpStream::connect(("127.0.0.1", self.inner.port))?;
                c.set_read_timeout(Some(Duration::from_secs(5)))?;
                c.write_all(b"GET http://127.0.0.1/ HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept-Encoding: identity\r\n\r\n")?;
                let mut got = Vec::new();
                let mut one = [0u8; 1];
                while !got.ends_with(b"\r\n") && got.len() < 256 {
                    if c.read(&mut one)? == 0 {
                        break;
                    }
                    got.push(one[0]);
                }
                Ok(got.split(|&b| b == b' ').nth(1) == Some(b"407"))
            })()
            .unwrap_or(false);
        }
        let mut m = Map::new();
        m.insert("alive".into(), json!(alive));
        m.insert("restarted".into(), json!(restarted));
        m.insert(
            "restarts".into(),
            json!(self.inner.restarts.load(Ordering::SeqCst)),
        );
        m.insert(
            "last_error".into(),
            json!(*self
                .inner
                .last_error
                .lock()
                .unwrap_or_else(|e| e.into_inner())),
        );
        m
    }

    /// This process's token (made on first use, until `forget`).
    pub fn credentials(&self, proc_id: &str) -> String {
        let (token, new) = {
            let mut st = self.inner.lock();
            match st.tokens.get(proc_id) {
                Some(t) => (t.clone(), false),
                None => {
                    let t = token_urlsafe(24);
                    st.tokens.insert(proc_id.to_string(), t.clone());
                    st.facts.entry(proc_id.to_string()).or_default();
                    (t, true)
                }
            }
        };
        if new {
            if let Some(r) = &self.inner.cfg.redactor {
                r.add_secret_value(&token);
            }
        }
        token
    }

    /// The process's proxy variables: every common spelling (proxy.py:578).
    pub fn env(&self, proc_id: &str) -> Vec<(String, String)> {
        let url = format!(
            "http://{proc_id}:{}@127.0.0.1:{}",
            self.credentials(proc_id),
            self.inner.port
        );
        let local = if self.inner.cfg.local_access.is_some() {
            ""
        } else {
            "localhost,127.0.0.1,::1"
        };
        let mut out: Vec<(String, String)> = [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ]
        .iter()
        .map(|k| (k.to_string(), url.clone()))
        .collect();
        out.push(("NO_PROXY".into(), local.into()));
        out.push(("no_proxy".into(), local.into()));
        out.push(("NODE_USE_ENV_PROXY".into(), "1".into()));
        out
    }

    /// What the process reached and what FERB refused it, so far.
    pub fn facts(&self, proc_id: &str) -> Map<String, Value> {
        let st = self.inner.lock();
        facts_json(st.facts.get(proc_id), &*self.inner.cfg.excluded)
    }

    /// The process is done with: its credential stops working at once; its record is reported once its last
    /// connection closes (proxy.py:619).
    pub fn forget(&self, proc_id: &str) {
        let (token, report) = {
            let mut st = self.inner.lock();
            let token = st.tokens.remove(proc_id);
            let report = if !st.facts.contains_key(proc_id) {
                None
            } else if st.active.get(proc_id).copied().unwrap_or(0) > 0 {
                st.ending.insert(proc_id.to_string());
                None
            } else {
                Some(self.inner.take(&mut st, proc_id))
            };
            (token, report)
        };
        if let (Some(t), Some(r)) = (token, &self.inner.cfg.redactor) {
            r.forget_secret_value(&t);
        }
        if let Some(rep) = report {
            self.inner.report(proc_id, rep);
        }
    }

    /// Whether a final record for this process is still to come.
    pub fn pending(&self, proc_id: &str) -> bool {
        self.inner.lock().facts.contains_key(proc_id)
    }

    /// Stop serving, for good. Every record still held is reported first, marked `still_open` when a connection is
    /// still open (proxy.py:804).
    pub fn close(&self) {
        let (reports, tokens) = {
            let mut st = self.inner.lock();
            if st.shut {
                return;
            }
            st.shut = true;
            self.inner.shut.store(true, Ordering::SeqCst);
            self.inner.wake.notify_all();
            let procs: Vec<(String, bool)> = st
                .facts
                .keys()
                .map(|p| (p.clone(), st.active.get(p).copied().unwrap_or(0) > 0))
                .collect();
            let reports: Vec<(String, Map<String, Value>)> = procs
                .into_iter()
                .map(|(p, open)| {
                    let mut r = self.inner.take(&mut st, &p);
                    r.insert("still_open".into(), json!(open));
                    (p, r)
                })
                .collect();
            let tokens: Vec<String> = st.tokens.drain().map(|(_, t)| t).collect();
            (reports, tokens)
        };
        if let Some(r) = &self.inner.cfg.redactor {
            for t in &tokens {
                r.forget_secret_value(t);
            }
        }
        for (p, rec) in reports {
            self.inner.report(&p, rec);
        }
        // wake the accept loop so it sees the flag
        let _ = TcpStream::connect(("127.0.0.1", self.inner.port));
    }

    /// Test seams with Python's names: record a connection, a refusal, bytes, an SNI.
    #[doc(hidden)]
    pub fn opened(
        &self,
        proc: &str,
        host: &str,
        port: u16,
        local_peer: Option<&LocalPeer>,
    ) -> bool {
        self.inner.opened(proc, host, port, local_peer)
    }
    #[doc(hidden)]
    pub fn note(&self, proc: &str, host: &str, port: u16, refused: &str) {
        self.inner.note(proc, host, port, refused)
    }
    #[doc(hidden)]
    pub fn count(&self, proc: &str, host: &str, port: u16, inbound: bool, n: usize) {
        self.inner.count(proc, host, port, inbound, n)
    }
    #[doc(hidden)]
    pub fn record_sni(&self, proc: &str, sni: &str, port: u16) {
        self.inner.record_sni(proc, sni, port)
    }
}

impl Drop for EgressProxy {
    fn drop(&mut self) {
        self.close();
    }
}

fn bind_listener() -> std::io::Result<TcpListener> {
    // the kernel's maximum backlog (HAND-228), clamped to kern.ipc.somaxconn
    let fd = {
        // SAFETY: a new socket; checked below.
        unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) }
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    use std::os::fd::FromRawFd;
    // SAFETY: `fd` is a socket we just made and own.
    let listener = unsafe { TcpListener::from_raw_fd(fd) };
    let addr = libc::sockaddr_in {
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        sin_len: std::mem::size_of::<libc::sockaddr_in>() as u8,
        sin_family: libc::AF_INET as libc::sa_family_t,
        sin_port: 0,
        sin_addr: libc::in_addr {
            s_addr: u32::from_ne_bytes([127, 0, 0, 1]),
        },
        sin_zero: [0; 8],
    };
    let one: libc::c_int = 1;
    // SAFETY: valid option pointer and length.
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_REUSEADDR,
            &one as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
    }
    // SAFETY: `addr` is a valid sockaddr_in of the size passed.
    let r = unsafe {
        libc::bind(
            fd,
            &addr as *const _ as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
        )
    };
    if r != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a bound socket.
    if unsafe { libc::listen(fd, libc::SOMAXCONN) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(listener)
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn report(&self, proc: &str, record: Map<String, Value>) {
        let f = self
            .on_report
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(f) = f {
            f(proc, record);
        }
    }

    fn serve(self: Arc<Self>) {
        loop {
            if self.shut.load(Ordering::SeqCst) {
                return;
            }
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if self.shut.load(Ordering::SeqCst) {
                        return;
                    }
                    let inner = self.clone();
                    let _ = std::thread::Builder::new()
                        .name("ferb-proxy-conn".into())
                        .spawn(move || inner.handle(stream));
                }
                Err(e) => {
                    if e.raw_os_error() == Some(libc::EMFILE) {
                        std::thread::sleep(Duration::from_millis(50)); // out of descriptors: no hot loop
                    }
                }
            }
        }
    }

    /// A new connection: pending until it authenticates, by an absolute deadline; past `max_unauth` the oldest
    /// pending connection is closed.
    fn admit(&self, conn: &TcpStream) -> u64 {
        let mut st = self.lock();
        st.next_conn += 1;
        let id = st.next_conn;
        if let Ok(clone) = conn.try_clone() {
            st.pending
                .insert(id, (Instant::now() + self.cfg.unauth_timeout, clone));
        }
        while st.pending.len() > self.cfg.max_unauth {
            let oldest = *st
                .pending
                .iter()
                .min_by_key(|(k, (due, _))| (*due, **k))
                .map(|(k, _)| k)
                .expect("non-empty");
            if let Some((_, s)) = st.pending.remove(&oldest) {
                hang_up(&s);
            }
        }
        self.wake.notify_all();
        id
    }

    fn settled(&self, id: u64) {
        self.lock().pending.remove(&id);
    }

    fn reap(self: Arc<Self>) {
        let mut st = self.lock();
        loop {
            if st.shut {
                return;
            }
            let now = Instant::now();
            let due: Vec<u64> = st
                .pending
                .iter()
                .filter(|(_, (d, _))| *d <= now)
                .map(|(k, _)| *k)
                .collect();
            for k in due {
                if let Some((_, s)) = st.pending.remove(&k) {
                    hang_up(&s);
                }
            }
            let next = st.pending.values().map(|(d, _)| *d).min();
            st = match next {
                Some(d) => match self.wake.wait_timeout(st, d.saturating_duration_since(now)) {
                    Ok((g, _)) => g,
                    Err(e) => e.into_inner().0,
                },
                None => self.wake.wait(st).unwrap_or_else(|e| e.into_inner()),
            };
        }
    }

    fn take(&self, st: &mut State, proc_id: &str) -> Map<String, Value> {
        let f = st.facts.remove(proc_id);
        let out = facts_json(f.as_ref(), &*self.cfg.excluded);
        st.ending.remove(proc_id);
        st.active.remove(proc_id);
        out
    }

    fn live(&self, proc: &str) -> bool {
        self.lock().tokens.contains_key(proc)
    }

    fn opened(&self, proc: &str, host: &str, port: u16, local_peer: Option<&LocalPeer>) -> bool {
        let first = {
            let mut st = self.lock();
            if !st.tokens.contains_key(proc) {
                return false;
            }
            let f = st.facts.entry(proc.to_string()).or_default();
            let key = (host.to_string(), port);
            let first = !f.destinations.contains_key(&key);
            f.destinations.entry(key).or_default().connections += 1;
            if let Some(peer) = local_peer {
                *f.local
                    .entry((
                        host.to_string(),
                        port,
                        peer.address.clone(),
                        peer.executable.clone(),
                    ))
                    .or_insert(0) += 1;
            }
            *st.active.entry(proc.to_string()).or_insert(0) += 1;
            first
        };
        if first {
            self.first(proc, host, port);
        }
        true
    }

    fn first(&self, proc: &str, host: &str, port: u16) {
        if let Some(f) = &self.cfg.on_destination {
            let shown = if (self.cfg.excluded)(host) {
                "excluded"
            } else {
                host
            };
            let f = f.clone();
            let (proc, shown) = (proc.to_string(), shown.to_string());
            // its failure never touches the connection
            let _ =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&proc, &shown, port)));
        }
    }

    fn closed(&self, proc: &str) {
        let report = {
            let mut st = self.lock();
            let mut report = None;
            if let Some(n) = st.active.get_mut(proc) {
                *n -= 1;
                if *n <= 0 {
                    st.active.remove(proc);
                    if st.ending.contains(proc) {
                        report = Some(self.take(&mut st, proc));
                    }
                }
            }
            report
        };
        if let Some(r) = report {
            self.report(proc, r);
        }
    }

    fn note(&self, proc: &str, host: &str, port: u16, refused: &str) {
        let mut st = self.lock();
        let f = st.facts.entry(proc.to_string()).or_default();
        *f.refused
            .entry((host.to_string(), port, refused.to_string()))
            .or_insert(0) += 1;
    }

    fn count(&self, proc: &str, host: &str, port: u16, inbound: bool, n: usize) {
        let mut st = self.lock();
        if let Some(f) = st.facts.get_mut(proc) {
            if let Some(c) = f.destinations.get_mut(&(host.to_string(), port)) {
                if inbound {
                    c.bytes_in += n as i64;
                } else {
                    c.bytes_out += n as i64;
                }
            }
        }
    }

    fn send(
        &self,
        sock: &mut TcpStream,
        data: &[u8],
        proc: &str,
        host: &str,
        port: u16,
        inbound: bool,
    ) -> std::io::Result<()> {
        self.count(proc, host, port, inbound, data.len());
        sock.write_all(data)
    }

    fn record_sni(&self, proc: &str, sni: &str, port: u16) {
        let first = {
            let mut st = self.lock();
            let Some(f) = st.facts.get_mut(proc) else {
                return;
            };
            let key = (sni.to_string(), port);
            let first = !f.destinations.contains_key(&key);
            f.destinations.entry(key).or_default();
            first
        };
        if first {
            self.first(proc, sni, port);
        }
    }

    fn handle(self: Arc<Self>, stream: TcpStream) {
        let id = self.admit(&stream);
        let mut h = Handler {
            proxy: self.clone(),
            conn: stream,
            conn_id: id,
            request_version: "HTTP/0.9".into(),
            command: None,
            path: String::new(),
            headers: Vec::new(),
        };
        if let Err(e) = h.handle_one_request() {
            *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(py_error_name(&e).to_string());
        }
        self.settled(id);
        let _ = h.conn.flush();
        let _ = h.conn.shutdown(Shutdown::Write);
    }

    /// Inspect the first client bytes of a CONNECT (S4 fronting, proxy.py:719). Returns the bytes to forward, or None.
    fn sni_gate(
        &self,
        chunk: Vec<u8>,
        client: &mut TcpStream,
        proc: &str,
        host: &str,
        port: u16,
        check: bool,
    ) -> Option<Vec<u8>> {
        if chunk.first() != Some(&0x16) {
            return Some(chunk); // not a TLS handshake: forward immediately
        }
        let mut reader = HelloReader::new();
        if !check {
            if let Some(sni) = reader.feed(&chunk) {
                if !sni.is_empty() && sni != NOT_HELLO && sni.trim_end_matches('.') != host {
                    self.record_sni(proc, &sni, port);
                }
            }
            return Some(chunk);
        }
        let mut buf = chunk.clone();
        let mut sni = reader.feed(&chunk);
        let mut more = vec![0u8; CHUNK];
        while sni.is_none() && buf.len() < SNI_MAX {
            match client.read(&mut more) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    buf.extend_from_slice(&more[..n]);
                    sni = reader.feed(&more[..n]);
                }
            }
        }
        if sni.as_deref() == Some("") && literal(host) {
            return Some(buf); // RFC 6066 forbids an IP literal in the SNI
        }
        let sni = match sni {
            Some(s) if !s.is_empty() && s != NOT_HELLO => s,
            _ => {
                self.note(proc, host, port, DEST_SNI_NONE);
                return None;
            }
        };
        if sni.trim_end_matches('.') != host {
            let (ok, why) = (self.cfg.allow)(proc, &sni, port);
            if !ok {
                let why = if why.is_empty() {
                    "FERB refused this destination".to_string()
                } else {
                    why
                };
                self.note(proc, &sni, port, &why);
                return None;
            }
            self.record_sni(proc, &sni, port);
        }
        Some(buf)
    }

    /// Both directions at once; the first client bytes inspected inline when `sni_check` is given (proxy.py:759).
    fn pump(
        self: &Arc<Self>,
        client: &mut TcpStream,
        upstream: &mut TcpStream,
        proc: &str,
        host: &str,
        port: u16,
        sni_check: Option<bool>,
    ) {
        let (Ok(mut up_read), Ok(mut client_write)) = (upstream.try_clone(), client.try_clone())
        else {
            return;
        };
        let inner = self.clone();
        let (p, h) = (proc.to_string(), host.to_string());
        let u2c = std::thread::spawn(move || {
            let mut buf = vec![0u8; CHUNK];
            loop {
                match up_read.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if inner
                            .send(&mut client_write, &buf[..n], &p, &h, port, true)
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            let _ = client_write.shutdown(Shutdown::Write);
        });
        let mut first = true;
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = match client.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let mut chunk = buf[..n].to_vec();
            if first && sni_check.is_some() {
                first = false;
                match self.sni_gate(chunk, client, proc, host, port, sni_check == Some(true)) {
                    None => {
                        hang_up(upstream); // a fronting refusal: nothing forwarded, both sides torn down
                        break;
                    }
                    Some(gated) => chunk = gated,
                }
            } else {
                first = false;
            }
            if self
                .send(upstream, &chunk, proc, host, port, false)
                .is_err()
            {
                break;
            }
        }
        let _ = upstream.shutdown(Shutdown::Write);
        let _ = u2c.join();
    }
}

// ---------- one request (http.server.BaseHTTPRequestHandler, Python 3.12) ----------

struct Handler {
    proxy: Arc<Inner>,
    conn: TcpStream,
    conn_id: u64,
    request_version: String,
    command: Option<String>,
    path: String,
    headers: Vec<(String, String)>,
}

/// http.HTTPStatus phrases and explanations for the codes this proxy sends.
fn status(code: u16) -> (&'static str, &'static str) {
    match code {
        100 => ("Continue", "Request received, please continue"),
        200 => ("OK", "Request fulfilled, document follows"),
        400 => ("Bad Request", "Bad request syntax or unsupported method"),
        403 => (
            "Forbidden",
            "Request forbidden -- authorization will not help",
        ),
        407 => (
            "Proxy Authentication Required",
            "You must authenticate with this proxy before proceeding",
        ),
        414 => ("Request-URI Too Long", "URI is too long"),
        431 => (
            "Request Header Fields Too Large",
            "The server refused this request because the request header fields are too large",
        ),
        501 => ("Not Implemented", "Server does not support this operation"),
        502 => ("Bad Gateway", "Invalid responses from another server/proxy"),
        505 => ("HTTP Version Not Supported", "Cannot fulfill request"),
        _ => ("???", "???"),
    }
}

/// email.utils.formatdate(now, usegmt=True).
pub fn http_date(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (h, m, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
    let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][days.rem_euclid(7) as usize];
    // civil from days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][(mo - 1) as usize];
    format!("{weekday}, {d:02} {month} {y:04} {h:02}:{m:02}:{s:02} GMT")
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn latin1(raw: &[u8]) -> String {
    raw.iter().map(|&b| b as char).collect()
}

/// email.parser on an HTTP head (feedparser `_parsegen` / `_parse_headers`, compat32 `header_source_parse`).
fn parse_header_lines(lines: &[String]) -> Vec<(String, String)> {
    let header_re = |l: &str| -> bool {
        if l.starts_with("From ") || l.starts_with('\t') || l.starts_with(' ') {
            return true;
        }
        let name_len = l
            .chars()
            .take_while(|&c| ('\x21'..='\x7e').contains(&c) && c != ':')
            .count();
        l[name_len..].starts_with(':')
    };
    let mut taken: Vec<&str> = Vec::new();
    for l in lines {
        if !header_re(l) {
            break; // the separator, or a line that ends the headers (the rest is body)
        }
        taken.push(l);
    }
    let mut out = Vec::new();
    let mut last: Vec<&str> = Vec::new();
    let flush = |last: &mut Vec<&str>, out: &mut Vec<(String, String)>| {
        if last.is_empty() {
            return;
        }
        let (name, value) = last[0].split_once(':').expect("a header line has a colon");
        let mut v = value.trim_start_matches([' ', '\t']).to_string();
        for more in &last[1..] {
            v.push_str(more);
        }
        out.push((
            name.to_string(),
            v.trim_end_matches(['\r', '\n']).to_string(),
        ));
        last.clear();
    };
    for (lineno, line) in taken.iter().enumerate() {
        if line.starts_with([' ', '\t']) {
            if !last.is_empty() {
                last.push(line);
            }
            continue;
        }
        flush(&mut last, &mut out);
        if line.starts_with("From ") {
            if lineno == taken.len() - 1 {
                break; // unread: becomes the body
            }
            continue; // the envelope line, or a misplaced one: not a header
        }
        if line.find(':') == Some(0) {
            continue; // missing header name
        }
        last.push(line);
    }
    flush(&mut last, &mut out);
    out
}

impl Handler {
    fn header(&self, name: &str) -> Option<&str> {
        let lower = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| k.to_ascii_lowercase() == lower)
            .map(|(_, v)| v.as_str())
    }

    /// `rfile.readline(limit)` on an unbuffered socket: one byte at a time, never past the newline.
    fn readline(&mut self, limit: usize) -> std::io::Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut one = [0u8; 1];
        while out.len() < limit {
            match self.conn.read(&mut one) {
                Ok(0) => break,
                Ok(_) => {
                    out.push(one[0]);
                    if one[0] == b'\n' {
                        break;
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.conn.write_all(data)
    }

    fn head(&self, code: u16, message: &str, extra: &[(&str, &str)], with_date: bool) -> Vec<u8> {
        if self.request_version == "HTTP/0.9" {
            return Vec::new();
        }
        let mut h = format!("HTTP/1.1 {code} {message}\r\n");
        if with_date {
            h.push_str(&format!(
                "Date: {}\r\n",
                http_date(crate::wall_time() as i64)
            ));
        }
        for (k, v) in extra {
            h.push_str(&format!("{k}: {v}\r\n"));
        }
        h.push_str("\r\n");
        h.chars().map(|c| c as u32 as u8).collect() // latin-1 (every character here is)
    }

    fn send_error(
        &mut self,
        code: u16,
        message: Option<&str>,
        explain: Option<&str>,
    ) -> std::io::Result<()> {
        let (short, long) = status(code);
        let message = message.unwrap_or(short).to_string();
        let explain = explain.unwrap_or(long);
        let body = format!(
            "<!DOCTYPE HTML>\n<html lang=\"en\">\n    <head>\n        <meta charset=\"utf-8\">\n        <title>Error response</title>\n    </head>\n    <body>\n        <h1>Error response</h1>\n        <p>Error code: {code}</p>\n        <p>Message: {}.</p>\n        <p>Error code explanation: {code} - {}.</p>\n    </body>\n</html>\n",
            html_escape(&message),
            html_escape(explain)
        );
        let len = body.len().to_string();
        let head = self.head(
            code,
            &message,
            &[
                ("Connection", "close"),
                ("Content-Type", "text/html;charset=utf-8"),
                ("Content-Length", &len),
            ],
            true,
        );
        self.write(&head)?;
        if self.command.as_deref() != Some("HEAD") {
            self.write(body.as_bytes())?;
        }
        Ok(())
    }

    /// proxy.py `_reply`.
    fn reply(&mut self, code: u16, text: &str, extra: &[(&str, &str)]) -> std::io::Result<()> {
        let body = if text.is_empty() {
            String::new()
        } else {
            format!("{text}\n")
        };
        let len = body.len().to_string();
        let mut headers: Vec<(&str, &str)> = extra.to_vec();
        headers.push(("Content-Type", "text/plain; charset=utf-8"));
        headers.push(("Content-Length", &len));
        headers.push(("Connection", "close"));
        let head = self.head(code, status(code).0, &headers, true);
        self.write(&head)?;
        if self.command.as_deref() != Some("HEAD") {
            self.write(body.as_bytes())?;
        }
        Ok(())
    }

    fn handle_one_request(&mut self) -> std::io::Result<()> {
        let raw = self.readline(65537)?;
        if raw.len() > 65536 {
            self.request_version = String::new();
            self.command = Some(String::new());
            return self.send_error(414, None, None);
        }
        if raw.is_empty() {
            return Ok(());
        }
        if !self.parse_request(&raw)? {
            return Ok(());
        }
        match self.command.clone().as_deref() {
            Some("CONNECT") => self.do_connect(),
            Some("GET" | "POST" | "PUT" | "DELETE" | "HEAD" | "PATCH" | "OPTIONS") => {
                self.forward()
            }
            Some(other) => {
                let msg = format!("Unsupported method ({})", repr_latin1(other));
                self.send_error(501, Some(&msg), None)
            }
            None => Ok(()),
        }
    }

    fn parse_request(&mut self, raw: &[u8]) -> std::io::Result<bool> {
        let line = latin1(raw);
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        let words = split_ws(&line);
        if words.is_empty() {
            return Ok(false);
        }
        if words.len() >= 3 {
            let version = words[words.len() - 1];
            let parsed = (|| -> Option<(u64, u64, String)> {
                let base = version.strip_prefix("HTTP/")?;
                let nums: Vec<&str> = base.split('.').collect();
                if nums.len() != 2
                    || nums
                        .iter()
                        .any(|n| n.is_empty() || !n.chars().all(ferb_py::text::is_digit))
                {
                    return None;
                }
                if nums.iter().any(|n| n.chars().count() > 10) {
                    return None;
                }
                let major = int_str(nums[0]).filter(|n| *n >= 0)?;
                let minor = int_str(nums[1]).filter(|n| *n >= 0)?;
                Some((major as u64, minor as u64, base.to_string()))
            })();
            let Some((major, _minor, base)) = parsed else {
                let msg = format!("Bad request version ({})", repr_latin1(version));
                self.send_error(400, Some(&msg), None)?;
                return Ok(false);
            };
            if major >= 2 {
                let msg = format!("Invalid HTTP version ({base})");
                self.send_error(505, Some(&msg), None)?;
                return Ok(false);
            }
            self.request_version = version.to_string();
        }
        if !(2..=3).contains(&words.len()) {
            let msg = format!("Bad request syntax ({})", repr_latin1(&line));
            self.send_error(400, Some(&msg), None)?;
            return Ok(false);
        }
        let (command, path) = (words[0].to_string(), words[1].to_string());
        if words.len() == 2 && command != "GET" {
            let msg = format!("Bad HTTP/0.9 request type ({})", repr_latin1(&command));
            self.send_error(400, Some(&msg), None)?;
            return Ok(false);
        }
        self.command = Some(command);
        self.path = if path.starts_with("//") {
            format!("/{}", path.trim_start_matches('/'))
        } else {
            path
        };
        // http.client.parse_headers: lines up to 65536 bytes, at most 100 (the blank line counts)
        let mut lines = Vec::new();
        loop {
            let l = self.readline(65537)?;
            if l.len() > 65536 {
                self.send_error(
                    431,
                    Some("Line too long"),
                    Some("got more than 65536 bytes when reading header line"),
                )?;
                return Ok(false);
            }
            let end = l.is_empty() || l == b"\r\n" || l == b"\n";
            lines.push(latin1(&l));
            if lines.len() > 100 {
                self.send_error(
                    431,
                    Some("Too many headers"),
                    Some("got more than 100 headers"),
                )?;
                return Ok(false);
            }
            if end {
                break;
            }
        }
        self.headers = parse_header_lines(&lines);
        let expect = self
            .header("Expect")
            .map(ferb_py::text::lower)
            .unwrap_or_default();
        if expect == "100-continue" && self.request_version.as_str() >= "HTTP/1.1" {
            let head = self.head(100, "Continue", &[], false);
            self.write(&head)?;
        }
        Ok(true)
    }

    /// The checked connection for this request, or None after answering 407/403/502 (proxy.py:277).
    fn open(
        &mut self,
        host: &str,
        port: u16,
    ) -> std::io::Result<Option<(String, TcpStream, bool)>> {
        let found = {
            let st = self.proxy.lock();
            self.header("Proxy-Authorization")
                .and_then(|v| who(v, &st.tokens))
        };
        let Some(proc) = found else {
            self.reply(
                407,
                "FERB: proxy credentials required",
                &[("Proxy-Authenticate", "Basic realm=\"FERB\"")],
            )?;
            return Ok(None);
        };
        self.proxy.settled(self.conn_id); // authenticated: no deadline, no idle timeout
        let host = ferb_py::text::lower(host);
        let local = if local_host(&host) {
            self.proxy.cfg.local_access.clone()
        } else {
            None
        };
        let decided: Result<Vec<SocketAddr>, Unreachable> = (|| {
            if local.is_some() {
                return Ok(local_addresses(&host, port)
                    .into_iter()
                    .filter_map(|(v6, h, p)| sockaddr(v6, &h, p))
                    .collect());
            }
            if literal(&host) && this_mac(&host) {
                return Err(Unreachable(OWN.into())); // before asking: an address needs no DNS
            }
            let (ok, why) = (self.proxy.cfg.allow)(&proc, &host, port);
            if !ok {
                return Err(Unreachable(if why.is_empty() {
                    "FERB refused this destination".into()
                } else {
                    why
                }));
            }
            (self.proxy.cfg.resolve)(&host, port)
        })();
        let where_ = match decided {
            Ok(w) => w,
            Err(e) => {
                self.proxy.note(&proc, &host, port, &e.0);
                self.reply(403, &format!("FERB: {e}"), &[])?;
                return Ok(None);
            }
        };
        let mut local_error: Option<String> = None;
        for addr in where_ {
            if !self.proxy.live(&proc) {
                self.reply(403, &format!("FERB: {ENDED}"), &[])?;
                return Ok(None);
            }
            let attempt = (|| -> Result<(TcpStream, Option<LocalPeer>), Result<String, ()>> {
                if let Some(l) = &local {
                    l.before_connect((&addr.ip().to_string(), port as u32))
                        .map_err(|e: LocalRefused| Ok(e.0))?;
                }
                let upstream = TcpStream::connect_timeout(&addr, Duration::from_secs(30))
                    .map_err(|_| Err(()))?;
                let mut peer = None;
                if let Some(l) = &local {
                    let target = upstream.peer_addr().map_err(|_| Err(()))?;
                    let source = upstream.local_addr().map_err(|_| Err(()))?;
                    let found = l
                        .connected(
                            (&target.ip().to_string(), target.port() as u32),
                            (&source.ip().to_string(), source.port() as u32),
                        )
                        .map_err(|e| Ok(e.0))?;
                    let allow_local = self
                        .proxy
                        .cfg
                        .allow_local
                        .clone()
                        .unwrap_or_else(|| self.proxy.cfg.allow.clone());
                    let (ok, why) = allow_local(&proc, &host, port);
                    if !ok {
                        return Err(Ok(if why.is_empty() {
                            "FERB refused this destination".into()
                        } else {
                            why
                        }));
                    }
                    peer = Some(found);
                }
                Ok((upstream, peer))
            })();
            match attempt {
                Ok((upstream, peer)) => {
                    if !self.proxy.opened(&proc, &host, port, peer.as_ref()) {
                        drop(upstream);
                        self.reply(403, &format!("FERB: {ENDED}"), &[])?;
                        return Ok(None);
                    }
                    return Ok(Some((proc, upstream, local.is_some())));
                }
                Err(Ok(refused)) => local_error = Some(refused),
                Err(Err(())) => {}
            }
        }
        if let Some(e) = local_error {
            self.proxy.note(&proc, &host, port, &e);
            self.reply(403, &format!("FERB: {e}"), &[])?;
            return Ok(None);
        }
        self.reply(502, "FERB: couldn't connect to the destination", &[])?;
        Ok(None)
    }

    fn do_connect(&mut self) -> std::io::Result<()> {
        let target = urlsplit(&format!("//{}", self.path));
        let (host, port) = match &target {
            Some(t) => match t.port() {
                Ok(p) => (t.hostname.clone(), p),
                Err(_) => (None, None),
            },
            None => (None, None),
        };
        let (Some(host), Some(port)) = (host.filter(|h| !h.is_empty()), port.filter(|p| *p != 0))
        else {
            return self.reply(400, "FERB: CONNECT needs host:port", &[]);
        };
        let Some((proc, mut upstream, local)) = self.open(&host, port)? else {
            return Ok(());
        };
        let host = ferb_py::text::lower(&host);
        let result = (|| -> std::io::Result<()> {
            let head = self.head(200, "Connection established", &[], true);
            self.write(&head)?;
            let check = if local {
                None
            } else {
                Some((self.proxy.cfg.inspect_sni)(&proc, &host, port))
            };
            let proxy = self.proxy.clone();
            proxy.pump(&mut self.conn, &mut upstream, &proc, &host, port, check);
            Ok(())
        })();
        drop(upstream);
        self.proxy.closed(&proc);
        result
    }

    /// A plain http:// request, forwarded byte for byte on its own connection (proxy.py:372).
    fn forward(&mut self) -> std::io::Result<()> {
        let url = urlsplit(&self.path);
        let parsed = url
            .as_ref()
            .and_then(|u| u.port().ok().map(|p| (u.clone(), p)));
        let ok = parsed.as_ref().is_some_and(|(u, _)| {
            u.scheme.eq_ignore_ascii_case("http")
                && u.hostname.as_deref().is_some_and(|h| !h.is_empty())
        });
        if !ok {
            return self.reply(
                400,
                "FERB: send an absolute http:// URL, or CONNECT for anything else",
                &[],
            );
        }
        let (url, port) = parsed.expect("checked");
        let port = port.filter(|p| *p != 0).unwrap_or(80);
        let host = url.hostname.clone().expect("checked");
        let conn: HashSet<String> = self
            .header("Connection")
            .unwrap_or("")
            .split(',')
            .map(|t| ferb_py::text::lower(strip(t)))
            .collect();
        let upgrading =
            conn.contains("upgrade") && self.header("Upgrade").is_some_and(|u| !u.is_empty());
        let Some((proc, mut upstream, _local)) = self.open(&host, port)? else {
            return Ok(());
        };
        let host = ferb_py::text::lower(&host);
        let target = format!(
            "{}{}",
            if url.path.is_empty() { "/" } else { &url.path },
            if url.query.is_empty() {
                String::new()
            } else {
                format!("?{}", url.query)
            }
        );
        let command = self.command.clone().unwrap_or_default();
        let mut head = vec![
            format!("{command} {target} HTTP/1.1"),
            format!(
                "Host: {}",
                url.netloc
                    .rsplit_once('@')
                    .map_or(url.netloc.as_str(), |(_, h)| h)
            ),
            if upgrading {
                "Connection: Upgrade, close".into()
            } else {
                "Connection: close".into()
            },
        ];
        for (k, v) in &self.headers {
            let lk = k.to_ascii_lowercase();
            let dropped =
                (HOP.contains(&lk.as_str()) && !(upgrading && lk == "upgrade")) || lk == "host";
            if !dropped {
                head.push(format!("{k}: {v}"));
            }
        }
        let bytes = format!("{}\r\n\r\n", head.join("\r\n")).into_bytes(); // .encode(): UTF-8
        let outcome: Result<(), Framing> = (|| {
            self.proxy
                .send(&mut upstream, &bytes, &proc, &host, port, false)?;
            self.body(&mut upstream, &proc, &host, port)?;
            self.response(&mut upstream, &proc, &host, port, upgrading)?;
            Ok(())
        })();
        let result = match outcome {
            Err(Framing::Malformed) => {
                self.reply(400, "FERB: the request framing is malformed", &[])
            }
            _ => Ok(()),
        };
        drop(upstream);
        self.proxy.closed(&proc);
        result
    }

    /// The request body exactly as framed (Content-Length or chunked), nothing after it.
    fn body(
        &mut self,
        upstream: &mut TcpStream,
        proc: &str,
        host: &str,
        port: u16,
    ) -> Result<(), Framing> {
        let te = self
            .header("Transfer-Encoding")
            .map(ferb_py::text::lower)
            .unwrap_or_default();
        if te.ends_with("chunked") {
            loop {
                let line = self.readline(CHUNK)?;
                self.proxy.send(upstream, &line, proc, host, port, false)?;
                let size_text = bytes_strip(line.split(|&b| b == b';').next().unwrap_or(&[]));
                let size = if size_text.is_empty() {
                    0
                } else {
                    int_hex_bytes(size_text).ok_or(Framing::Malformed)?
                };
                if size == 0 {
                    loop {
                        let trailer = self.readline(CHUNK)?;
                        if trailer == b"\r\n" || trailer == b"\n" || trailer.is_empty() {
                            break;
                        }
                        self.proxy
                            .send(upstream, &trailer, proc, host, port, false)?;
                    }
                    self.proxy
                        .send(upstream, b"\r\n", proc, host, port, false)?;
                    return Ok(());
                }
                self.copy(upstream, size + 2, proc, host, port)?;
            }
        } else {
            let length = match self.header("Content-Length").filter(|v| !v.is_empty()) {
                None => 0,
                Some(v) => int_str(v).ok_or(Framing::Malformed)?,
            };
            self.copy(upstream, length, proc, host, port)?;
            Ok(())
        }
    }

    fn copy(
        &mut self,
        upstream: &mut TcpStream,
        mut n: i128,
        proc: &str,
        host: &str,
        port: u16,
    ) -> Result<(), Framing> {
        let mut buf = vec![0u8; CHUNK];
        while n > 0 {
            let want = (n.min(CHUNK as i128)) as usize;
            let got = match self.conn.read(&mut buf[..want]) {
                Ok(0) => break,
                Ok(g) => g,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(Framing::Io(e)),
            };
            self.proxy
                .send(upstream, &buf[..got], proc, host, port, false)?;
            n -= got as i128;
        }
        Ok(())
    }

    /// One response head and the bytes read past it, or None when upstream closed first (its bytes flushed) or the
    /// head was too large (502).
    fn read_head(
        &mut self,
        upstream: &mut TcpStream,
        proc: &str,
        host: &str,
        port: u16,
        pre: Vec<u8>,
    ) -> Result<Head, Framing> {
        let mut buf = pre;
        let find = |b: &[u8], from: usize| {
            b.get(from..)
                .and_then(|s| s.windows(4).position(|w| w == b"\r\n\r\n"))
                .map(|i| i + from)
        };
        let mut cut = find(&buf, 0);
        let mut chunk = vec![0u8; CHUNK];
        while cut.is_none() {
            let start = buf.len().saturating_sub(3);
            let n = upstream.read(&mut chunk).map_err(Framing::Io)?;
            if n == 0 {
                self.write(&buf).map_err(Framing::Io)?;
                return Ok(None);
            }
            self.proxy.count(proc, host, port, true, n);
            buf.extend_from_slice(&chunk[..n]);
            cut = find(&buf, start);
            if cut.is_none() && buf.len() > MAX_HEAD {
                self.reply(502, "FERB: the upstream response head is too large", &[])
                    .map_err(Framing::Io)?;
                return Ok(None);
            }
        }
        let cut = cut.expect("found");
        Ok(Some((buf[..cut].to_vec(), buf[cut + 4..].to_vec())))
    }

    fn response(
        &mut self,
        upstream: &mut TcpStream,
        proc: &str,
        host: &str,
        port: u16,
        upgrading: bool,
    ) -> Result<(), Framing> {
        let mut rest = Vec::new();
        loop {
            let Some((head, after)) =
                self.read_head(upstream, proc, host, port, std::mem::take(&mut rest))?
            else {
                return Ok(());
            };
            rest = after;
            let lines: Vec<&[u8]> = split_crlf(&head);
            let fields: Vec<&[u8]> = lines[0].split(|&b| b == b' ').collect();
            let code: &[u8] = if fields.len() > 1
                && !fields[1].is_empty()
                && fields[1].iter().all(u8::is_ascii_digit)
            {
                fields[1]
            } else {
                b""
            };
            if code == b"101" && upgrading {
                let mut out = head.clone();
                out.extend_from_slice(b"\r\n\r\n");
                out.extend_from_slice(&rest);
                self.write(&out).map_err(Framing::Io)?;
                let proxy = self.proxy.clone();
                proxy.pump(&mut self.conn, upstream, proc, host, port, None);
                return Ok(());
            }
            if code.starts_with(b"1") {
                let mut out = head.clone();
                out.extend_from_slice(b"\r\n\r\n");
                self.write(&out).map_err(Framing::Io)?;
                continue;
            }
            let mut kept: Vec<&[u8]> = vec![lines[0]];
            for h in &lines[1..] {
                let name =
                    bytes_strip(h.split(|&b| b == b':').next().unwrap_or(&[])).to_ascii_lowercase();
                if !HOP.contains(&latin1(&name).as_str()) {
                    kept.push(h);
                }
            }
            kept.push(b"Connection: close");
            let mut out = kept.join(&b"\r\n"[..]);
            out.extend_from_slice(b"\r\n\r\n");
            out.extend_from_slice(&rest);
            self.write(&out).map_err(Framing::Io)?;
            break;
        }
        let mut chunk = vec![0u8; CHUNK];
        loop {
            let n = upstream.read(&mut chunk).map_err(Framing::Io)?;
            if n == 0 {
                return Ok(());
            }
            self.proxy.count(proc, host, port, true, n);
            self.write(&chunk[..n]).map_err(Framing::Io)?;
        }
    }
}

fn split_crlf(head: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i + 1 < head.len() {
        if &head[i..i + 2] == b"\r\n" {
            out.push(&head[start..i]);
            i += 2;
            start = i;
        } else {
            i += 1;
        }
    }
    out.push(&head[start..]);
    out
}

/// A forwarded request's outcome: malformed framing gets a 400; a socket error ends it quietly.
/// A response head and the bytes read past it; None when there is none to forward.
type Head = Option<(Vec<u8>, Vec<u8>)>;

enum Framing {
    Malformed,
    /// Python's `except OSError: pass`: kept for a debugger, never shown.
    Io(#[allow(dead_code)] std::io::Error),
}

impl From<std::io::Error> for Framing {
    fn from(e: std::io::Error) -> Framing {
        Framing::Io(e)
    }
}

/// `bytes.strip()`: ASCII whitespace, vertical tab included.
fn bytes_strip(b: &[u8]) -> &[u8] {
    let ws = |c: &u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
    let start = b.iter().position(|c| !ws(c)).unwrap_or(b.len());
    let end = b.iter().rposition(|c| !ws(c)).map_or(start, |e| e + 1);
    &b[start..end]
}
