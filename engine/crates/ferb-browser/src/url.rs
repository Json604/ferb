//! `urllib.parse.urlsplit` as Python 3.12 computes it, for the addresses the browser reports.
//! `hostname` is ferb-sanitize's Python-exact port (Lib/urllib/parse.py and Lib/ipaddress.py: an
//! invalid bracketed host or a non-ASCII host whose NFKC form adds a delimiter raises; Unicode
//! lowercase); `parts` is urlsplit's scheme, netloc and path.

/// Where Python's `urlsplit` raises `ValueError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidUrl;

/// `urlsplit(url).hostname`.
pub fn hostname(url: &str) -> Result<Option<String>, InvalidUrl> {
    ferb_py::url::hostname(url).map_err(|_| InvalidUrl)
}

/// `urlsplit(url).hostname or None`, where a URL Python can't split counts as none.
pub fn host(url: &str) -> Option<String> {
    hostname(url).ok().flatten().filter(|h| !h.is_empty())
}

/// `urlsplit(url)`'s (scheme, netloc, path): the scheme lowercased, the netloc as written.
pub fn parts(url: &str) -> Result<(String, String, String), InvalidUrl> {
    let url = url.trim_start_matches(|c: char| c <= ' ');
    let url: String = url
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let mut rest = url.as_str();
    let mut scheme = String::new();
    if let Some(i) = rest.find(':') {
        let s = &rest[..i];
        if i > 0
            && s.as_bytes()[0].is_ascii_alphabetic()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.')
        {
            scheme = s.to_ascii_lowercase();
            rest = &rest[i + 1..];
        }
    }
    let mut netloc = String::new();
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        netloc = after[..end].to_string();
        if netloc.contains('[') != netloc.contains(']') {
            return Err(InvalidUrl);
        }
        rest = &after[end..];
    }
    let path = rest.split(['#', '?']).next().unwrap_or("").to_string();
    Ok((scheme, netloc, path))
}

#[cfg(test)]
mod tests {
    use super::{hostname, parts};

    #[test]
    fn hosts_as_urlsplit_reads_them() {
        assert_eq!(
            hostname("https://A.Example:8443/x").unwrap().as_deref(),
            Some("a.example")
        );
        assert_eq!(
            hostname("https://user:pw@b.example/").unwrap().as_deref(),
            Some("b.example")
        );
        assert_eq!(
            hostname("http://[::1]:80/").unwrap().as_deref(),
            Some("::1")
        );
        assert_eq!(hostname("about:blank").unwrap(), None);
        assert_eq!(hostname("file:///tmp/x").unwrap(), None);
        assert!(hostname("http://[::1/").is_err());
        assert!(hostname("http://[nothex]/").is_err()); // Python checks the IPv6 literal
        assert_eq!(
            parts("HTTPS://a.example:1/p/q?x#y").unwrap(),
            ("https".into(), "a.example:1".into(), "/p/q".into())
        );
        assert_eq!(
            parts("about:blank").unwrap(),
            ("about".into(), String::new(), "blank".into())
        );
    }
}
