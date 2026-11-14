//! HAND-523: FERB's wake page, opened only when a profile's windowless start (HAND-520) got no answer and pairing
//! falls back to the profile's window. Python's `WakeServer` (ext_bridge.py:809) served a blank local page there and
//! closed it through the extension (`wake_close`) only after a pairing that succeeded, so every other outcome left a
//! "FERB" tab behind.
//!
//! The page here closes itself: its script asks this server whether the pairing that opened it is over
//! (`/wake/<id>/done`: `0` while it runs, `1` once its [`WakePage`] is dropped, which the pairing's wait does on every
//! outcome: paired, refused, timed out, its caller gone) and then calls `window.close()`; it closes itself too when
//! the server is gone (the daemon stopped). Chromium lets a page close its own tab when that tab wasn't opened by
//! another page's script only while the tab's history has one entry (`DOMWindow::Close`: `BackForwardLength() > 1`
//! refuses); a page FERB opens from the browser's command line is the tab's first and only entry, as with the
//! extension's own pairing page (pair.js closes itself the same way).

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// The page: blank, titled FERB as Python's, plus the script that closes it once its pairing is over (asked every
/// half second; a background tab's timers run at most once a second, enough for a wait of a minute).
pub const PAGE: &str = "<!doctype html><meta charset=\"utf-8\"><title>FERB</title><script>\
(function look(){fetch(location.pathname+\"/done\",{cache:\"no-store\"})\
.then(function(r){return r.text()})\
.then(function(t){if(t===\"1\")window.close();else setTimeout(look,500)},function(){window.close()})})()\
</script>";

/// The local server of the wake pages: 127.0.0.1 on a free port, for the daemon's life, answering each page and
/// whether its pairing is over. It holds no secret: a page's id only names which wait it belongs to.
pub struct WakePages {
    port: u16,
    open: Arc<Mutex<HashSet<String>>>,
}

/// One wake page, open while this is held; dropping it tells the page to close itself.
pub struct WakePage {
    pub url: String,
    id: String,
    open: Arc<Mutex<HashSet<String>>>,
}

impl Drop for WakePage {
    fn drop(&mut self) {
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
    }
}

impl WakePages {
    /// Listens on 127.0.0.1 (any free port) on the current runtime.
    pub fn start() -> std::io::Result<Arc<WakePages>> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let handle = tokio::runtime::Handle::try_current().map_err(std::io::Error::other)?;
        let _entered = handle.enter();
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let open: Arc<Mutex<HashSet<String>>> = Arc::default();
        let served = open.clone();
        handle.spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        tokio::spawn(answer(stream, served.clone()));
                    }
                    Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                }
            }
        });
        Ok(Arc::new(WakePages { port, open }))
    }

    /// A new page, `http://127.0.0.1:<port>/wake/<16 hex>` (never about:blank: its address tells it apart from the
    /// owner's tabs), open until the returned value is dropped.
    pub fn open(&self) -> WakePage {
        let id = ferb_runtimes::codex::token_hex(8);
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.clone());
        WakePage {
            url: format!("http://127.0.0.1:{}/wake/{id}", self.port),
            id,
            open: self.open.clone(),
        }
    }
}

/// One connection: its request line read (a browser may open a connection and send nothing: 5 s), then
/// `/wake/<id>/done`'s answer (`1` unless that page's pairing still runs: an unknown page is over) or the page.
async fn answer(stream: tokio::net::TcpStream, open: Arc<Mutex<HashSet<String>>>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut reader = BufReader::new(stream);
    let mut first = String::new();
    let read = tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut first)).await;
    if !matches!(read, Ok(Ok(_))) {
        return;
    }
    let path = first.split_whitespace().nth(1).unwrap_or("");
    let done = path
        .strip_prefix("/wake/")
        .and_then(|rest| rest.strip_suffix("/done"));
    let (kind, body) = match done {
        Some(id) => {
            let running = open
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(id);
            ("text/plain", if running { "0" } else { "1" })
        }
        None => ("text/html", PAGE),
    };
    let mut out = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {kind}\r\ncache-control: no-store\r\ncontent-length: {}\r\n\
connection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body.as_bytes());
    let mut stream = reader.into_inner();
    let _ = stream.write_all(&out).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::{WakePages, PAGE};

    async fn get(url: &str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let rest = url.strip_prefix("http://127.0.0.1:").unwrap();
        let (port, path) = rest.split_once('/').unwrap();
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap()))
            .await
            .unwrap();
        s.write_all(format!("GET /{path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut got = String::new();
        s.read_to_string(&mut got).await.unwrap();
        got
    }

    /// Each wake is its own local address; it answers the page, and `done` says 0 while its pairing runs and 1 once
    /// it is dropped (and for a page it never knew).
    // decision: HAND-523
    #[tokio::test]
    async fn each_wake_page_is_unique_and_says_when_its_pairing_is_over() {
        let pages = WakePages::start().expect("the wake pages start");
        let (a, b) = (pages.open(), pages.open());
        assert_ne!(a.url, b.url);
        let path = a.url.split_once("/wake/").unwrap().1.to_string();
        assert_eq!(path.len(), 16, "{}", a.url);
        let page = get(&a.url).await;
        assert!(page.starts_with("HTTP/1.1 200 OK\r\n"), "{page}");
        assert!(page.ends_with(PAGE), "{page}");
        assert!(get(&format!("{}/done", a.url)).await.ends_with("\r\n\r\n0"));
        let url = a.url.clone();
        drop(a);
        assert!(get(&format!("{url}/done")).await.ends_with("\r\n\r\n1"));
        assert!(get(&format!("{}/done", b.url)).await.ends_with("\r\n\r\n0"));
        let unknown = format!("{}/done", url.replace(&path, "0123456789abcdef"));
        assert!(get(&unknown).await.ends_with("\r\n\r\n1"));
    }
}
