//! Python 3.12's `urllib.parse` (`urlsplit` with its properties, `urlunsplit`, `quote`, `quote_plus`, `unquote`,
//! `unquote_plus`) and `ipaddress` (`ip_address`, `IPv4Address`, `IPv6Address` and their text), as the policy calls
//! them, including every case where they raise `ValueError`.

use crate::norm;
use crate::py;

/// `urlsplit` (or a property of its result) raised `ValueError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidUrl;

/// `urlsplit(url)`'s five parts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Split {
    pub scheme: String,
    pub netloc: String,
    pub path: String,
    pub query: String,
    pub fragment: String,
}

fn is_c0_or_space(c: char) -> bool {
    (c as u32) <= 0x20
}

/// `urlsplit(url)` (scheme "", fragments allowed).
pub fn urlsplit(url: &str) -> Result<Split, InvalidUrl> {
    let mut url: String = url
        .trim_start_matches(is_c0_or_space)
        .replace(['\t', '\r', '\n'], "");
    let mut out = Split::default();
    if let Some(i) = url.find(':') {
        let first = url.chars().next();
        let scheme_char = |c: char| c.is_ascii_alphanumeric() || "+-.".contains(c);
        if i > 0
            && first.is_some_and(|c| c.is_ascii_alphabetic())
            && url[..i].chars().all(scheme_char)
        {
            out.scheme = url[..i].to_ascii_lowercase();
            url = url[i + 1..].to_string();
        }
    }
    let mut rest = url.as_str();
    if rest.starts_with("//") {
        let after = &rest[2..];
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        out.netloc = after[..end].to_string();
        rest = &after[end..];
        let (open, close) = (out.netloc.contains('['), out.netloc.contains(']'));
        if open != close {
            return Err(InvalidUrl);
        }
        if open {
            check_bracketed_netloc(&out.netloc)?;
        }
    }
    let (mut path, fragment) = match rest.split_once('#') {
        Some((a, b)) => (a, b),
        None => (rest, ""),
    };
    out.fragment = fragment.to_string();
    if let Some((a, b)) = path.split_once('?') {
        out.query = b.to_string();
        path = a;
    }
    out.path = path.to_string();
    check_netloc(&out.netloc)?;
    Ok(out)
}

fn check_netloc(netloc: &str) -> Result<(), InvalidUrl> {
    if netloc.is_empty() || netloc.is_ascii() {
        return Ok(());
    }
    let n = netloc.replace(['@', ':', '#', '?'], "");
    let n2 = norm::nfkc(&n);
    if n == n2 {
        return Ok(());
    }
    if n2.contains(['/', '?', '#', '@', ':']) {
        return Err(InvalidUrl);
    }
    Ok(())
}

fn check_bracketed_netloc(netloc: &str) -> Result<(), InvalidUrl> {
    let host_port = py::rpartition(netloc, "@").2;
    let (before, open, bracketed) = py::partition(host_port, "[");
    let hostname = if open {
        if !before.is_empty() {
            return Err(InvalidUrl);
        }
        let (hostname, _, port) = py::partition(bracketed, "]");
        if !port.is_empty() && !port.starts_with(':') {
            return Err(InvalidUrl);
        }
        hostname
    } else {
        py::partition(host_port, ":").0
    };
    if let Some(rest) = hostname.strip_prefix('v') {
        // re.match(r"\Av[a-fA-F0-9]+\..+\Z", hostname): "." takes anything but "\n"
        let hex = rest.bytes().take_while(u8::is_ascii_hexdigit).count();
        if !(hex > 0
            && rest[hex..].starts_with('.')
            && rest.len() > hex + 1
            && !rest[hex + 1..].contains('\n'))
        {
            return Err(InvalidUrl);
        }
        Ok(())
    } else {
        match ip_address(hostname) {
            Some(Ip::V6(..)) => Ok(()),
            _ => Err(InvalidUrl),
        }
    }
}

impl Split {
    fn userinfo(&self) -> (Option<&str>, Option<&str>) {
        let (userinfo, have, _) = py::rpartition(&self.netloc, "@");
        if !have {
            return (None, None);
        }
        let (user, have_pw, pw) = py::partition(userinfo, ":");
        (Some(user), have_pw.then_some(pw))
    }

    fn hostinfo(&self) -> (&str, Option<&str>) {
        let hostinfo = py::rpartition(&self.netloc, "@").2;
        let (_, open, bracketed) = py::partition(hostinfo, "[");
        let (hostname, port) = if open {
            let (h, _, port) = py::partition(bracketed, "]");
            (h, py::partition(port, ":").2)
        } else {
            let (h, _, p) = py::partition(hostinfo, ":");
            (h, p)
        };
        (hostname, (!port.is_empty()).then_some(port))
    }

    /// `.username`
    pub fn username(&self) -> Option<&str> {
        self.userinfo().0
    }

    /// `.password`
    pub fn password(&self) -> Option<&str> {
        self.userinfo().1
    }

    /// `.hostname`
    pub fn hostname(&self) -> Option<String> {
        let hostname = self.hostinfo().0;
        if hostname.is_empty() {
            return None;
        }
        let (h, pct, zone) = py::partition(hostname, "%");
        Some(format!(
            "{}{}{}",
            py::lower(h),
            if pct { "%" } else { "" },
            zone
        ))
    }

    /// `.port`: Err where Python raises.
    pub fn port(&self) -> Result<Option<u32>, InvalidUrl> {
        let Some(port) = self.hostinfo().1 else {
            return Ok(None);
        };
        if !(port.is_ascii() && port.chars().all(|c| c.is_ascii_digit())) {
            return Err(InvalidUrl);
        }
        let digits = port.trim_start_matches('0');
        if digits.len() > 5 {
            return Err(InvalidUrl);
        }
        let n: u32 = if digits.is_empty() {
            0
        } else {
            digits.parse().map_err(|_| InvalidUrl)?
        };
        if n > 65535 {
            return Err(InvalidUrl);
        }
        Ok(Some(n))
    }
}

/// `urlunsplit((scheme, netloc, path, query, fragment))`.
pub fn urlunsplit(s: &Split) -> String {
    const USES_NETLOC: &[&str] = &[
        "",
        "ftp",
        "http",
        "gopher",
        "nntp",
        "telnet",
        "imap",
        "wais",
        "file",
        "mms",
        "https",
        "shttp",
        "snews",
        "prospero",
        "rtsp",
        "rtsps",
        "rtspu",
        "rsync",
        "svn",
        "svn+ssh",
        "sftp",
        "nfs",
        "git",
        "git+ssh",
        "ws",
        "wss",
        "itms-services",
    ];
    let mut url = s.path.clone();
    if !s.netloc.is_empty() {
        if !url.is_empty() && !url.starts_with('/') {
            url = format!("/{url}");
        }
        url = format!("//{}{url}", s.netloc);
    } else if url.starts_with("//")
        || (!s.scheme.is_empty()
            && USES_NETLOC.contains(&s.scheme.as_str())
            && (url.is_empty() || url.starts_with('/')))
    {
        url = format!("//{url}");
    }
    if !s.scheme.is_empty() {
        url = format!("{}:{url}", s.scheme);
    }
    if !s.query.is_empty() {
        url = format!("{url}?{}", s.query);
    }
    if !s.fragment.is_empty() {
        url = format!("{url}#{}", s.fragment);
    }
    url
}

fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn unquote_bytes(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut parts = s.split(|&b| b == b'%');
    if let Some(first) = parts.next() {
        out.extend_from_slice(first);
    }
    for item in parts {
        match (
            item.first().and_then(|&b| hexval(b)),
            item.get(1).and_then(|&b| hexval(b)),
        ) {
            (Some(h), Some(l)) => {
                out.push(h * 16 + l);
                out.extend_from_slice(&item[2..]);
            }
            _ => {
                out.push(b'%');
                out.extend_from_slice(item);
            }
        }
    }
    out
}

/// `unquote(s)`: each ASCII run's escapes decoded as UTF-8 with replacement; other text kept.
pub fn unquote(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut run_start: Option<usize> = None;
    for (i, c) in s.char_indices() {
        if c.is_ascii() {
            run_start.get_or_insert(i);
        } else {
            if let Some(a) = run_start.take() {
                out.push_str(&String::from_utf8_lossy(&unquote_bytes(
                    &s.as_bytes()[a..i],
                )));
            }
            out.push(c);
        }
    }
    if let Some(a) = run_start {
        out.push_str(&String::from_utf8_lossy(&unquote_bytes(&s.as_bytes()[a..])));
    }
    out
}

/// `unquote_plus(s)`.
pub fn unquote_plus(s: &str) -> String {
    unquote(&s.replace('+', " "))
}

/// `quote(s, safe)`.
pub fn quote(s: &str, safe: &str) -> String {
    let safe: Vec<u8> = safe.bytes().filter(|b| b.is_ascii()).collect();
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) || safe.contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `quote_plus(s)` (safe "").
pub fn quote_plus(s: &str) -> String {
    if !s.contains(' ') {
        return quote(s, "");
    }
    quote(s, " ").replace(' ', "+")
}

/// An address `ipaddress.ip_address` read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ip {
    V4(u32),
    /// The address and its scope id.
    V6(u128, Option<String>),
}

/// `IPv4Address(text)` (strict: four decimal octets, no leading zeros).
pub fn ipv4(s: &str) -> Option<u32> {
    if s.is_empty() || s.contains('/') {
        return None;
    }
    let octets: Vec<&str> = s.split('.').collect();
    if octets.len() != 4 {
        return None;
    }
    let mut out: u32 = 0;
    for o in octets {
        if o.is_empty()
            || !o.bytes().all(|b| b.is_ascii_digit())
            || o.len() > 3
            || (o != "0" && o.starts_with('0'))
        {
            return None;
        }
        let n: u32 = o.parse().ok()?;
        if n > 255 {
            return None;
        }
        out = out * 256 + n;
    }
    Some(out)
}

/// `IPv6Address(text)`.
pub fn ipv6(s: &str) -> Option<(u128, Option<String>)> {
    if s.contains('/') {
        return None;
    }
    let (addr, sep, scope) = py::partition(s, "%");
    let scope = if sep {
        if scope.is_empty() || scope.contains('%') {
            return None;
        }
        Some(scope.to_string())
    } else {
        None
    };
    if addr.is_empty() || py::char_len(addr) > 45 {
        return None;
    }
    let mut parts: Vec<String> = addr.splitn(10, ':').map(str::to_string).collect();
    if parts.len() < 3 {
        return None;
    }
    if parts[parts.len() - 1].contains('.') {
        let v4 = ipv4(&parts.pop()?)?;
        parts.push(format!("{:x}", (v4 >> 16) & 0xFFFF));
        parts.push(format!("{:x}", v4 & 0xFFFF));
    }
    if parts.len() > 9 {
        return None;
    }
    let mut skip: Option<usize> = None;
    for (i, part) in parts.iter().enumerate().take(parts.len() - 1).skip(1) {
        if part.is_empty() {
            if skip.is_some() {
                return None;
            }
            skip = Some(i);
        }
    }
    let (hi, lo);
    if let Some(skip) = skip {
        let (mut parts_hi, mut parts_lo) = (skip, parts.len() - skip - 1);
        if parts[0].is_empty() {
            parts_hi -= 1;
            if parts_hi != 0 {
                return None;
            }
        }
        if parts[parts.len() - 1].is_empty() {
            parts_lo -= 1;
            if parts_lo != 0 {
                return None;
            }
        }
        if parts_hi + parts_lo + 1 > 8 {
            return None;
        }
        (hi, lo) = (parts_hi, parts_lo);
    } else {
        if parts.len() != 8 {
            return None;
        }
        (hi, lo) = (parts.len(), 0);
    }
    let hextet = |h: &str| -> Option<u128> {
        if h.is_empty() || h.len() > 4 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u128::from_str_radix(h, 16).ok()
    };
    let mut ip: u128 = 0;
    for p in &parts[..hi] {
        ip = (ip << 16) | hextet(p)?;
    }
    if skip.is_some() {
        ip = ip.checked_shl(16 * (8 - hi - lo) as u32).unwrap_or(0);
    }
    for p in &parts[parts.len() - lo..] {
        ip = (ip << 16) | hextet(p)?;
    }
    Some((ip, scope))
}

/// `ipaddress.ip_address(text)`: None where it raises.
pub fn ip_address(s: &str) -> Option<Ip> {
    if let Some(v4) = ipv4(s) {
        return Some(Ip::V4(v4));
    }
    ipv6(s).map(|(ip, scope)| Ip::V6(ip, scope))
}

/// `str(IPv4Address(n))`.
pub fn ipv4_text(n: u32) -> String {
    let b = n.to_be_bytes();
    format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
}

fn hextets_text(ip: u128) -> String {
    let mut hextets: Vec<String> = (0..8)
        .map(|i| format!("{:x}", (ip >> (112 - 16 * i)) & 0xFFFF))
        .collect();
    let (mut best_start, mut best_len, mut start, mut len) = (-1i64, 0, -1i64, 0);
    for (i, h) in hextets.iter().enumerate() {
        if h == "0" {
            len += 1;
            if start == -1 {
                start = i as i64;
            }
            if len > best_len {
                best_len = len;
                best_start = start;
            }
        } else {
            len = 0;
            start = -1;
        }
    }
    if best_len > 1 {
        let s = best_start as usize;
        let end = s + best_len;
        if end == hextets.len() {
            hextets.push(String::new());
        }
        hextets.splice(s..end, [String::new()]);
        if s == 0 {
            hextets.insert(0, String::new());
        }
    }
    hextets.join(":")
}

/// `str(IPv6Address(...))`.
pub fn ipv6_text(ip: u128, scope: Option<&str>) -> String {
    let text = if ip >> 32 == 0xFFFF {
        format!("{}:{}", hextets_text(0xFFFF), ipv4_text(ip as u32))
    } else {
        hextets_text(ip)
    };
    match scope {
        Some(s) => format!("{text}%{s}"),
        None => text,
    }
}
