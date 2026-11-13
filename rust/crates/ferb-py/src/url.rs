//! `urllib.parse.urlsplit(url).hostname` as Python 3.12 computes it (Lib/urllib/parse.py:470-523, 165-217,
//! 421-465; Lib/ipaddress.py for a bracketed host), including every case where it raises ValueError: the redactor
//! skips such a URL for the exclusion list (redact.py:185-188) and origin labels fall back to the text
//! (origin.py:96-99), so the refusals are part of the behaviour. The one copy: ferb-sanitize (`parity::hostname`)
//! re-exports it.

use crate::text as u;

/// urlsplit raised ValueError.
#[derive(Debug, PartialEq, Eq)]
pub struct InvalidUrl;

fn ipv4_ok(s: &str) -> bool {
    // IPv4Address(str): no "/", exactly 4 octets, each 1-3 ASCII digits, no leading zero, at most 255.
    if s.is_empty() || s.contains('/') {
        return false;
    }
    let octets: Vec<&str> = s.split('.').collect();
    octets.len() == 4
        && octets.iter().all(|o| {
            !o.is_empty()
                && o.bytes().all(|b| b.is_ascii_digit())
                && o.len() <= 3
                && (*o == "0" || !o.starts_with('0'))
                && o.parse::<u32>().is_ok_and(|n| n <= 255)
        })
}

fn hextet_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 4 && h.bytes().all(|b| b.is_ascii_hexdigit())
}

fn ipv6_ok(s: &str) -> bool {
    // IPv6Address(str) (ipaddress.py: __init__, _split_scope_id, _ip_int_from_string).
    if s.contains('/') {
        return false;
    }
    let addr = match s.split_once('%') {
        Some((addr, scope)) => {
            if scope.is_empty() || scope.contains('%') {
                return false;
            }
            addr
        }
        None => s,
    };
    if addr.is_empty() || crate::value::char_len(addr) > 45 {
        return false;
    }
    let mut parts: Vec<&str> = addr.splitn(10, ':').collect(); // split(":", maxsplit=9)
    if parts.len() < 3 {
        return false;
    }
    if parts[parts.len() - 1].contains('.') {
        if !ipv4_ok(parts[parts.len() - 1]) {
            return false;
        }
        parts.pop();
        parts.push("0"); // the two hextets the IPv4 part becomes: always valid
        parts.push("0");
    }
    if parts.len() > 9 {
        return false;
    }
    let mut skip: Option<usize> = None;
    for (i, part) in parts.iter().enumerate().take(parts.len() - 1).skip(1) {
        if part.is_empty() {
            if skip.is_some() {
                return false;
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
                return false;
            }
        }
        if parts[parts.len() - 1].is_empty() {
            parts_lo -= 1;
            if parts_lo != 0 {
                return false;
            }
        }
        if 8 < parts_hi + parts_lo + 1 {
            return false;
        }
        (hi, lo) = (parts_hi, parts_lo);
    } else {
        if parts.len() != 8 || parts[0].is_empty() || parts[parts.len() - 1].is_empty() {
            return false;
        }
        (hi, lo) = (parts.len(), 0);
    }
    parts[..hi].iter().all(|p| hextet_ok(p))
        && parts[parts.len() - lo..].iter().all(|p| hextet_ok(p))
}

fn bracketed_host_ok(hostname: &str) -> bool {
    if let Some(rest) = hostname.strip_prefix('v') {
        // re.match(r"\Av[a-fA-F0-9]+\..+\Z", hostname): "." takes anything but "\n"
        let hex = rest.bytes().take_while(u8::is_ascii_hexdigit).count();
        hex > 0
            && rest[hex..].starts_with('.')
            && rest.len() > hex + 1
            && !rest[hex + 1..].contains('\n')
    } else {
        // ipaddress.ip_address: an IPv4 address parses first and is then refused in brackets
        !ipv4_ok(hostname) && ipv6_ok(hostname)
    }
}

/// `urlsplit(url).netloc`, or the ValueError.
fn netloc(url: &str) -> Result<String, InvalidUrl> {
    let url: String = url
        .trim_start_matches(|c: char| (c as u32) <= 0x20)
        .replace(['\t', '\r', '\n'], "");
    let mut rest = url.as_str();
    if let Some(i) = rest.find(':') {
        let scheme_char = |c: char| c.is_ascii_alphanumeric() || "+-.".contains(c);
        if i > 0
            && rest.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && rest[..i].chars().all(scheme_char)
        {
            rest = &rest[i + 1..];
        }
    }
    let Some(after) = rest.strip_prefix("//") else {
        return Ok(String::new());
    };
    let netloc = &after[..after.find(['/', '?', '#']).unwrap_or(after.len())];
    let (open, close) = (netloc.contains('['), netloc.contains(']'));
    if open != close {
        return Err(InvalidUrl);
    }
    if open {
        // _check_bracketed_netloc
        let host_port = netloc.rsplit_once('@').map_or(netloc, |(_, h)| h);
        let hostname = match host_port.split_once('[') {
            Some((before, bracketed)) => {
                if !before.is_empty() {
                    return Err(InvalidUrl);
                }
                let (hostname, port) = bracketed.split_once(']').unwrap_or((bracketed, ""));
                if !port.is_empty() && !port.starts_with(':') {
                    return Err(InvalidUrl);
                }
                hostname
            }
            None => host_port.split_once(':').map_or(host_port, |(h, _)| h),
        };
        if !bracketed_host_ok(hostname) {
            return Err(InvalidUrl);
        }
    }
    // _checknetloc: a host whose NFKC form would hold one of "/?#@:" (see generated.rs NFKC_DELIMS)
    if !netloc.is_ascii() && netloc.chars().any(u::nfkc_delim) {
        return Err(InvalidUrl);
    }
    Ok(netloc.to_string())
}

/// `urlsplit(url).hostname`: None when there is none, Err when urlsplit raises.
pub fn hostname(url: &str) -> Result<Option<String>, InvalidUrl> {
    let netloc = netloc(url)?;
    let hostinfo = netloc.rsplit_once('@').map_or(netloc.as_str(), |(_, h)| h);
    let hostname = match hostinfo.split_once('[') {
        Some((_, bracketed)) => bracketed.split_once(']').map_or(bracketed, |(h, _)| h),
        None => hostinfo.split_once(':').map_or(hostinfo, |(h, _)| h),
    };
    if hostname.is_empty() {
        return Ok(None);
    }
    // a scoped IPv6 address's zone keeps its case
    Ok(Some(match hostname.split_once('%') {
        Some((host, zone)) => format!("{}%{}", u::lower(host), zone),
        None => u::lower(hostname),
    }))
}
