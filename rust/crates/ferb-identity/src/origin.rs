//! `normalize_origin` (`ferb/identity/secrets.py:24-29`): the form allowed origins and login
//! places are compared in. Python builds it on `urllib.parse.urlsplit`; this is that function's
//! scheme, netloc, hostname and port reading (CPython 3.12.14, `urllib/parse.py`), refusals
//! included, compared with Python case by case in `tests/parity.rs`.

use std::{net::Ipv6Addr, str::FromStr};

/// Python's `ValueError` from `urlsplit` or `.port`: no address reads this way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadAddress;

/// Every character outside ASCII whose NFKC form holds one of `/ ? # @ :`
/// (`urllib.parse._checknetloc` refuses a netloc with one, e.g. U+2100 "a/c"). From Python
/// 3.12's Unicode 15.0 tables: `rust/fixtures/identity/make.py` `nfkc_delimiters`, and a test
/// holds this list to it.
pub const NFKC_DELIMITERS: [char; 19] = [
    '\u{2047}', '\u{2048}', '\u{2049}', '\u{2100}', '\u{2101}', '\u{2105}', '\u{2106}', '\u{2a74}',
    '\u{fe13}', '\u{fe16}', '\u{fe55}', '\u{fe56}', '\u{fe5f}', '\u{fe6b}', '\u{ff03}', '\u{ff0f}',
    '\u{ff1a}', '\u{ff1f}', '\u{ff20}',
];

/// `scheme://host[:non-default port]`, the host lowercased (`secrets.py:24-29`).
pub fn normalize_origin(origin: &str) -> Result<String, BadAddress> {
    let (scheme, netloc) = split(origin)?;
    let (host, port) = host_and_port(&netloc);
    let default = match scheme.as_str() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    };
    let port = match port {
        None => None,
        Some(text) => {
            // `SplitResult.port`: ASCII digits only, at most 65535.
            if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
                return Err(BadAddress);
            }
            let digits = text.trim_start_matches('0');
            let port: u32 = if digits.is_empty() {
                0
            } else {
                digits.parse().map_err(|_| BadAddress)?
            };
            if port > 65535 {
                return Err(BadAddress);
            }
            Some(port)
        }
    };
    let mut out = format!("{scheme}://{host}");
    if let Some(port) = port.filter(|p| *p != 0 && Some(*p) != default) {
        out.push_str(&format!(":{port}"));
    }
    Ok(out)
}

/// `urlsplit(url)`'s scheme and netloc.
fn split(url: &str) -> Result<(String, String), BadAddress> {
    let url: String = url
        .trim_start_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let mut scheme = String::new();
    let mut rest = url.as_str();
    if let Some(i) = url.find(':') {
        let head = &url[..i];
        if head.starts_with(|c: char| c.is_ascii_alphabetic())
            && head
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            scheme = head.to_ascii_lowercase();
            rest = &url[i + 1..];
        }
    }
    let mut netloc = "";
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        netloc = &after[..end];
        let (open, close) = (netloc.contains('['), netloc.contains(']'));
        if open != close {
            return Err(BadAddress);
        }
        if open {
            check_bracketed(netloc)?;
        }
    }
    if netloc.chars().any(|c| NFKC_DELIMITERS.contains(&c)) {
        return Err(BadAddress);
    }
    Ok((scheme, netloc.to_owned()))
}

/// `_check_bracketed_netloc` and `_check_bracketed_host`.
fn check_bracketed(netloc: &str) -> Result<(), BadAddress> {
    let host_port = netloc.rsplit_once('@').map_or(netloc, |(_, after)| after);
    let host = match host_port.split_once('[') {
        Some((before, bracketed)) => {
            if !before.is_empty() {
                return Err(BadAddress);
            }
            let (host, after) = bracketed.split_once(']').unwrap_or((bracketed, ""));
            if !after.is_empty() && !after.starts_with(':') {
                return Err(BadAddress);
            }
            host
        }
        None => host_port
            .split_once(':')
            .map_or(host_port, |(host, _)| host),
    };
    if let Some(future) = host.strip_prefix('v') {
        // r"\Av[a-fA-F0-9]+\..+\Z"
        let hex = future.bytes().take_while(u8::is_ascii_hexdigit).count();
        let after = &future[hex..];
        return if hex > 0 && after.starts_with('.') && after.len() > 1 {
            Ok(())
        } else {
            Err(BadAddress)
        };
    }
    // `ipaddress.ip_address`: only an IPv6 address passes (an IPv4 one is refused in brackets),
    // with an optional `%zone` that is neither empty nor holds another `%`.
    let address = match host.split_once('%') {
        Some((_, zone)) if zone.is_empty() || zone.contains('%') => return Err(BadAddress),
        Some((address, _)) => address,
        None => host,
    };
    Ipv6Addr::from_str(address)
        .map(|_| ())
        .map_err(|_| BadAddress)
}

/// `SplitResult.hostname` ("" for none) and the text after the port's colon (`None` for none).
fn host_and_port(netloc: &str) -> (String, Option<&str>) {
    let hostinfo = netloc.rsplit_once('@').map_or(netloc, |(_, after)| after);
    let (host, port) = match hostinfo.split_once('[') {
        Some((_, bracketed)) => {
            let (host, after) = bracketed.split_once(']').unwrap_or((bracketed, ""));
            (host, after.split_once(':').map_or("", |(_, port)| port))
        }
        None => hostinfo.split_once(':').unwrap_or((hostinfo, "")),
    };
    // A scoped IPv6 address's zone is not lowercased.
    let host = match host.split_once('%') {
        Some((address, zone)) => format!("{}%{zone}", address.to_lowercase()),
        None => host.to_lowercase(),
    };
    (host, if port.is_empty() { None } else { Some(port) })
}
