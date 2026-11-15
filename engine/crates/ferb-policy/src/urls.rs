//! policy/urls.py: a URL parsed the way a browser does for special schemes (the WHATWG rules Chrome follows), so the
//! host FERB checks is the host the browser opens (S4). Shapes only.

use crate::{idna, py, pyurl};

pub const SPECIAL: &[&str] = &["http", "https", "ws", "wss", "ftp", "file"];
pub const MAX_PORT: u32 = 65535;
const FORBIDDEN_HOST: &str = " #%/:<>?@[\\]^|\x7f";

/// urls.py:26 `URL`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Url {
    pub scheme: String,
    pub username: String,
    pub password: String,
    /// "" when the URL names no host.
    pub host: String,
    pub port: String,
    pub path: String,
    pub query: String,
    pub fragment: String,
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn hex_part(s: &str) -> bool {
    (s.starts_with("0x") || s.starts_with("0X")) && s[2..].bytes().all(|b| b.is_ascii_hexdigit())
}

fn ipv4_part(part: &str) -> Option<u128> {
    if hex_part(part) {
        return Some(if part.len() > 2 {
            u128::from_str_radix(&part[2..], 16).ok()?
        } else {
            0
        });
    }
    if part.len() > 1 && part.starts_with('0') {
        return if part[1..].bytes().all(|b| (b'0'..=b'7').contains(&b)) {
            u128::from_str_radix(&part[1..], 8).ok()
        } else {
            None
        };
    }
    if digits(part) {
        part.parse().ok()
    } else {
        None
    }
}

/// urls.py:52 `_ipv4`: Some(host) unchanged when it doesn't end in a number; its dotted form when a browser reads
/// it as an address; None when it ends in a number but isn't one.
fn ipv4(host: &str) -> Option<String> {
    let mut parts: Vec<&str> = host.split('.').collect();
    if parts.len() > 1 && parts.last() == Some(&"") {
        parts.pop();
    }
    let last = *parts.last()?;
    if !(digits(last) || hex_part(last)) {
        return Some(host.to_string());
    }
    if parts.len() > 4 {
        return None;
    }
    let numbers: Vec<u128> = parts.iter().map(|p| ipv4_part(p)).collect::<Option<_>>()?;
    let (head, tail) = numbers.split_at(numbers.len() - 1);
    // values[-1] >= 256 ** (5 - len(values)): an integer this large can't be held as u128 past 2^128, which
    // no shorter form allows anyway
    let limit: u128 = 256u128.pow(5 - numbers.len() as u32);
    if head.iter().any(|&n| n > 255) || tail[0] >= limit {
        return None;
    }
    let mut address = tail[0];
    for (i, &n) in head.iter().enumerate() {
        address += n * 256u128.pow(3 - i as u32);
    }
    Some(pyurl::ipv4_text(address as u32))
}

fn host(raw: &str) -> Option<String> {
    if raw.starts_with('[') {
        if !raw.ends_with(']') || raw.len() < 2 {
            return None;
        }
        let (ip, scope) = pyurl::ipv6(&raw[1..raw.len() - 1])?;
        return Some(pyurl::ipv6_text(ip, scope.as_deref()));
    }
    let host = idna::uts46_remap(&pyurl::unquote(raw)).ok()?;
    if host
        .chars()
        .any(|c| FORBIDDEN_HOST.contains(c) || (c as u32) < 0x20)
    {
        return None;
    }
    let mut labels: Vec<String> = Vec::new();
    for label in host.split('.') {
        if label.is_ascii() {
            labels.push(label.to_ascii_lowercase());
            continue;
        }
        labels.push(idna::alabel(label).ok()?);
    }
    ipv4(&labels.join("."))
}

fn authority_and_path(scheme: &str, rest: &str) -> (String, String) {
    let text = rest.replace('\\', "/");
    let text = if scheme == "file" {
        match text.strip_prefix("//") {
            Some(t) => t.to_string(),
            None => return (String::new(), text),
        }
    } else {
        text.trim_start_matches('/').to_string()
    };
    match text.find('/') {
        Some(cut) => (text[..cut].to_string(), text[cut..].to_string()),
        None => (text, String::new()),
    }
}

fn scheme_ok(s: &str) -> bool {
    let mut cs = s.chars();
    cs.next().is_some_and(|c| c.is_ascii_alphabetic())
        && cs.all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
}

/// urls.py:126 `parse`: the URL's parts as a browser reads them, or None when a browser couldn't parse it.
pub fn parse(url: &str) -> Option<Url> {
    let text: String = url.chars().filter(|c| !"\t\n\r".contains(*c)).collect();
    let text = text.trim_matches(|c: char| (c as u32) <= 0x20);
    let (scheme, sep, rest) = py::partition(text, ":");
    if !sep || !scheme_ok(scheme) {
        return None;
    }
    let scheme = scheme.to_ascii_lowercase();
    if !SPECIAL.contains(&scheme.as_str()) {
        let p = pyurl::urlsplit(text).ok()?;
        let port = p.port().ok()?;
        return Some(Url {
            scheme,
            username: p.username().unwrap_or("").to_string(),
            password: p.password().unwrap_or("").to_string(),
            host: py::lower(&p.hostname().unwrap_or_default()),
            port: match port {
                Some(n) if n != 0 => n.to_string(),
                _ => String::new(),
            },
            path: p.path,
            query: p.query,
            fragment: p.fragment,
        });
    }
    let (head, _, fragment) = py::partition(rest, "#");
    let (head, _, query) = py::partition(head, "?");
    let (authority, path) = authority_and_path(&scheme, head);
    let (mut userinfo, at, mut hostport) = py::rpartition(&authority, "@");
    if !at {
        userinfo = "";
        hostport = &authority;
    }
    let (user, _, password) = py::partition(userinfo, ":");
    let (raw_host, port);
    if hostport.starts_with('[') {
        let (h, after) = match hostport.find(']') {
            Some(end) => (&hostport[..end + 1], &hostport[end + 1..]),
            None => (hostport, ""),
        };
        if !after.is_empty() && !after.starts_with(':') {
            return None;
        }
        raw_host = h;
        port = after.get(1..).unwrap_or("");
    } else {
        let (h, _, p) = py::partition(hostport, ":");
        raw_host = h;
        port = p;
    }
    if !port.is_empty()
        && (!digits(port) || port.parse::<u128>().map_or(true, |n| n > MAX_PORT as u128))
    {
        return None;
    }
    let host = if raw_host.is_empty() {
        String::new()
    } else {
        host(raw_host)?
    };
    Some(Url {
        scheme,
        username: user.to_string(),
        password: password.to_string(),
        host,
        port: port.to_string(),
        path,
        query: query.to_string(),
        fragment: fragment.to_string(),
    })
}
