//! `dashboard.py`: the owner-launched localhost memory dashboard's request rules (MEM-417): an exact Host, the page's
//! own Origin, its bearer token, JSON only with a content length, and only the listed operations; the page files
//! (dashboard.html, .css, .js) are served as they are. The listening socket is the daemon's; `handle` is the rule.

use serde_json::{json, Value};

/// dashboard.py:13 `ALLOWED`.
pub const ALLOWED: [&str; 10] = [
    "inventory",
    "search",
    "get",
    "confirm",
    "edit",
    "forget",
    "rollback",
    "digest",
    "identity",
    "retire",
];

/// The headers every reply carries (dashboard.py:27-34), after Content-Type and Content-Length.
pub const HEADERS: [(&str, &str); 4] = [
    ("Cache-Control", "no-store"),
    ("Referrer-Policy", "no-referrer"),
    ("X-Content-Type-Options", "nosniff"),
    (
        "Content-Security-Policy",
        "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'none'; \
         frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
    ),
];

/// One dashboard: its token (`secrets.token_urlsafe(32)`, made by the caller) and the address it listens on.
pub struct Dashboard {
    pub token: String,
    pub host: String,
}

/// A request as the handler reads it.
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub host: Option<String>,
    pub origin: Option<String>,
    pub authorization: Option<String>,
    pub content_type: Option<String>,
    pub content_length: Option<String>,
    pub transfer_encoding: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: &'static str,
}

impl Dashboard {
    pub fn new(token: String, port: u16) -> Dashboard {
        Dashboard {
            token,
            host: format!("127.0.0.1:{port}"),
        }
    }

    pub fn origin(&self) -> String {
        format!("http://{}", self.host)
    }

    pub fn url(&self) -> String {
        format!("{}/#{}", self.origin(), self.token)
    }

    /// dashboard.py:39-74: `files(name)` reads a page file; `call` is the daemon's memory operation.
    pub fn handle(
        &self,
        req: &Request,
        files: &dyn Fn(&str) -> std::io::Result<Vec<u8>>,
        call: &dyn Fn(Value) -> Result<Value, String>,
    ) -> Response {
        let forbidden = || Response {
            status: 403,
            body: b"Forbidden".to_vec(),
            content_type: "text/plain",
        };
        let valid_host = req.host.as_deref() == Some(self.host.as_str());
        if req.method == "GET" {
            if !valid_host {
                return forbidden();
            }
            let found = match req.path.as_str() {
                "/" => Some(("dashboard.html", "text/html; charset=utf-8")),
                "/app.js" => Some(("dashboard.js", "text/javascript; charset=utf-8")),
                "/style.css" => Some(("dashboard.css", "text/css; charset=utf-8")),
                _ => None,
            };
            return match found {
                None => Response {
                    status: 404,
                    body: b"Not found".to_vec(),
                    content_type: "text/plain",
                },
                Some((name, kind)) => match files(name) {
                    Ok(body) => Response {
                        status: 200,
                        body,
                        content_type: kind,
                    },
                    Err(_) => forbidden(),
                },
            };
        }
        let auth = req.authorization.as_deref().unwrap_or("");
        let expected = format!("Bearer {}", self.token);
        if !valid_host
            || req.path != "/api"
            || req.origin.as_deref() != Some(self.origin().as_str())
            || !constant_eq(auth.as_bytes(), expected.as_bytes())
            || req.content_type.as_deref() != Some("application/json")
        {
            return forbidden();
        }
        let answer = (|| -> Result<Value, String> {
            let raw = req.content_length.clone().unwrap_or_else(|| "-1".into());
            let size: i64 = raw.trim().parse().map_err(|_| {
                format!(
                    "invalid literal for int() with base 10: {}",
                    ferb_memory::pystr::repr(&raw)
                )
            })?;
            if size < 0
                || req
                    .transfer_encoding
                    .as_deref()
                    .is_some_and(|t| !t.is_empty())
            {
                return Err("requires a content length".into());
            }
            let body = &req.body[..(size as usize).min(req.body.len())];
            let text = String::from_utf8_lossy(body);
            let value = ferb_py::json::loads(&text)?;
            let op = value.get("op").and_then(Value::as_str).unwrap_or("");
            if !value.is_object() || !ALLOWED.contains(&op) {
                return Err("unsupported memory operation".into());
            }
            call(value)
        })();
        match answer {
            Ok(result) => Response {
                status: 200,
                body: ferb_py::json::dumps_utf8(&result).into_bytes(),
                content_type: "application/json",
            },
            Err(e) => Response {
                status: 400,
                body: ferb_py::json::dumps(&json!({"error": e})).into_bytes(),
                content_type: "application/json",
            },
        }
    }
}

/// `hmac.compare_digest`.
fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
