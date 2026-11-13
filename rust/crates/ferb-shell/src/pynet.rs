//! Python 3.12's `ipaddress.ip_address(text)` (parse, `str()`, `is_loopback`, `is_unspecified`, `ipv4_mapped`) and
//! the parts of `urllib.parse.urlsplit` the proxy reads (scheme, netloc, path, query, hostname, port), with every case
//! where Python raises ValueError. The hostname and the ValueError checks come from ferb-sanitize's port of
//! `urlsplit(url).hostname`, so both crates refuse the same URLs.

/// An address as `ipaddress.ip_address` parsed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ip {
    V4([u8; 4]),
    V6([u16; 8]),
}

/// `IPv4Address(text)`: exactly four dot-separated octets of 1-3 ASCII digits, no leading zero, at most 255.
pub fn parse_v4(s: &str) -> Option<[u8; 4]> {
    if s.is_empty() || s.contains('/') {
        return None;
    }
    let octets: Vec<&str> = s.split('.').collect();
    if octets.len() != 4 {
        return None;
    }
    let mut out = [0u8; 4];
    for (i, o) in octets.iter().enumerate() {
        if o.is_empty() || o.len() > 3 || !o.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if *o != "0" && o.starts_with('0') {
            return None;
        }
        let n: u32 = o.parse().ok()?;
        if n > 255 {
            return None;
        }
        out[i] = n as u8;
    }
    Some(out)
}

fn hextet(h: &str) -> Option<u16> {
    if h.is_empty() || h.len() > 4 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u16::from_str_radix(h, 16).ok()
}

/// `IPv6Address(text)` without a scope (the callers split it off first): ipaddress.py `_ip_int_from_string`.
pub fn parse_v6(s: &str) -> Option<[u16; 8]> {
    if s.is_empty() || s.contains('/') || s.chars().count() > 45 {
        return None;
    }
    let mut parts: Vec<String> = s.splitn(10, ':').map(str::to_string).collect();
    if parts.len() < 3 {
        return None;
    }
    if parts[parts.len() - 1].contains('.') {
        let v4 = parse_v4(&parts.pop()?)?;
        parts.push(format!("{:x}", (v4[0] as u16) << 8 | v4[1] as u16));
        parts.push(format!("{:x}", (v4[2] as u16) << 8 | v4[3] as u16));
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
    let (hi, lo, skipped);
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
        let used = parts_hi + parts_lo;
        if used >= 8 {
            return None;
        }
        (hi, lo, skipped) = (parts_hi, parts_lo, 8 - used);
    } else {
        if parts.len() != 8 {
            return None;
        }
        (hi, lo, skipped) = (parts.len(), 0, 0);
    }
    let mut out = [0u16; 8];
    let mut k = 0;
    for p in &parts[..hi] {
        out[k] = hextet(p)?;
        k += 1;
    }
    k += skipped;
    for p in &parts[parts.len() - lo..] {
        out[k] = hextet(p)?;
        k += 1;
    }
    Some(out)
}

/// `ipaddress.ip_address(text)`: IPv4 first, then IPv6 (a scope after "%" accepted when non-empty and alone).
pub fn ip_address(s: &str) -> Option<Ip> {
    if let Some(v4) = parse_v4(s) {
        return Some(Ip::V4(v4));
    }
    let addr = match s.split_once('%') {
        Some((addr, scope)) => {
            if scope.is_empty() || scope.contains('%') {
                return None;
            }
            addr
        }
        None => s,
    };
    parse_v6(addr).map(Ip::V6)
}

impl Ip {
    /// `.ipv4_mapped` of an IPv6 address (::ffff:a.b.c.d), else itself.
    pub fn unmapped(self) -> Ip {
        match self {
            Ip::V6([0, 0, 0, 0, 0, 0xffff, a, b]) => {
                Ip::V4([(a >> 8) as u8, a as u8, (b >> 8) as u8, b as u8])
            }
            other => other,
        }
    }

    pub fn is_loopback(self) -> bool {
        match self {
            Ip::V4(o) => o[0] == 127,
            Ip::V6(h) => h == [0, 0, 0, 0, 0, 0, 0, 1],
        }
    }

    pub fn is_unspecified(self) -> bool {
        match self {
            Ip::V4(o) => o == [0, 0, 0, 0],
            Ip::V6(h) => h == [0; 8],
        }
    }

    pub fn is_v6(self) -> bool {
        matches!(self, Ip::V6(_))
    }

    /// `str(address)`: dotted quad, or RFC 5952 hextets (the leftmost longest run of two or more zeros as "::").
    pub fn text(self) -> String {
        match self {
            Ip::V4(o) => format!("{}.{}.{}.{}", o[0], o[1], o[2], o[3]),
            Ip::V6(h) => {
                let (mut best, mut best_len, mut start, mut len) = (0usize, 0usize, 0usize, 0usize);
                for (i, &x) in h.iter().enumerate() {
                    if x == 0 {
                        if len == 0 {
                            start = i;
                        }
                        len += 1;
                        if len > best_len {
                            best = start;
                            best_len = len;
                        }
                    } else {
                        len = 0;
                    }
                }
                let parts: Vec<String> = h.iter().map(|x| format!("{x:x}")).collect();
                if best_len > 1 {
                    let head = parts[..best].join(":");
                    let tail = parts[best + best_len..].join(":");
                    format!("{head}::{tail}")
                } else {
                    parts.join(":")
                }
            }
        }
    }
}

/// `.port` raised ValueError (not ASCII digits, or past 65535).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadPort;

/// What `urlsplit(text)` gives the proxy: ValueError as `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    pub scheme: String,
    pub netloc: String,
    pub path: String,
    pub query: String,
    /// `.hostname`: lowercased, None when empty.
    pub hostname: Option<String>,
    /// `.port`'s text: None when there is none; `port()` reads it as Python does.
    port_text: Option<String>,
}

impl Split {
    /// `.port`: Ok(None) when absent, Err where Python raises ValueError (not ASCII digits, past 65535).
    pub fn port(&self) -> Result<Option<u16>, BadPort> {
        match &self.port_text {
            None => Ok(None),
            Some(p) => {
                if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(BadPort);
                }
                let trimmed = p.trim_start_matches('0');
                if trimmed.len() > 5 {
                    return Err(BadPort);
                }
                let n: u32 = if trimmed.is_empty() {
                    0
                } else {
                    trimmed.parse().map_err(|_| BadPort)?
                };
                if n > 65535 {
                    return Err(BadPort);
                }
                Ok(Some(n as u16))
            }
        }
    }
}

/// `urllib.parse.urlsplit(url)` (allow_fragments=True), or None where it raises ValueError.
pub fn urlsplit(url: &str) -> Option<Split> {
    let hostname = ferb_py::url::hostname(url).ok()?;
    let cleaned: String = url
        .trim_start_matches(|c: char| (c as u32) <= 0x20)
        .replace(['\t', '\r', '\n'], "");
    let mut rest = cleaned.as_str();
    let mut scheme = String::new();
    if let Some(i) = rest.find(':') {
        let scheme_char = |c: char| c.is_ascii_alphanumeric() || "+-.".contains(c);
        if i > 0
            && rest.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && rest[..i].chars().all(scheme_char)
        {
            scheme = rest[..i].to_ascii_lowercase();
            rest = &rest[i + 1..];
        }
    }
    let mut netloc = String::new();
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        netloc = after[..end].to_string();
        rest = &after[end..];
    }
    let rest = rest.split_once('#').map_or(rest, |(r, _)| r);
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (rest.to_string(), String::new()),
    };
    // _hostinfo: the port is after the bracketed host, or after the first ":"
    let hostinfo = netloc.rsplit_once('@').map_or(netloc.as_str(), |(_, h)| h);
    let port = match hostinfo.split_once('[') {
        Some((_, bracketed)) => {
            let after = bracketed.split_once(']').map_or("", |(_, p)| p);
            after.split_once(':').map_or("", |(_, p)| p)
        }
        None => hostinfo.split_once(':').map_or("", |(_, p)| p),
    };
    Some(Split {
        scheme,
        path,
        query,
        hostname,
        port_text: if port.is_empty() {
            None
        } else {
            Some(port.to_string())
        },
        netloc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hextets_compress_as_python() {
        assert_eq!(ip_address("1:0:0:2:0:0:0:3").unwrap().text(), "1:0:0:2::3");
        assert_eq!(ip_address("::1.2.3.4").unwrap().text(), "::102:304");
        assert_eq!(
            ip_address("1:2:3:4:5:6:7::").unwrap().text(),
            "1:2:3:4:5:6:7:0"
        );
        assert_eq!(
            ip_address("a:b:c:d:e:f:0:0").unwrap().text(),
            "a:b:c:d:e:f::"
        );
        assert!(ip_address("01.2.3.4").is_none());
    }
}
