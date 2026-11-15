//! Python 3.12.14's `html.unescape` and `html.parser.HTMLParser` (with `convert_charrefs=True`, the only way the
//! Gemini reader uses it), ported state for state: start and end tags, attributes, comments, declarations,
//! processing instructions, CDATA sections and the raw-text elements (`script`, `style`, ... and `textarea`,
//! `title`). Line numbers are not tracked (nothing reads them).

use crate::generated as g;
use crate::pystr;

fn html5(name: &str) -> Option<&'static str> {
    g::HTML5
        .binary_search_by(|(k, _)| (*k).cmp(name))
        .ok()
        .map(|i| g::HTML5[i].1)
}

fn replace_charref(s: &str) -> String {
    if let Some(num) = s.strip_prefix('#') {
        let (digits, radix) = match num.strip_prefix(['x', 'X']) {
            Some(hex) => (hex.trim_end_matches(';'), 16),
            None => (num.trim_end_matches(';'), 10),
        };
        let mut n: u64 = 0;
        for c in digits.chars() {
            n = n
                .saturating_mul(radix)
                .saturating_add(u64::from(c.to_digit(radix as u32).unwrap_or(0)));
        }
        if let Ok(i) = g::INVALID_CHARREFS.binary_search_by_key(&n, |&(k, _)| u64::from(k)) {
            return g::INVALID_CHARREFS[i].1.to_owned();
        }
        if (0xD800..=0xDFFF).contains(&n) || n > 0x10FFFF {
            return "\u{FFFD}".to_owned();
        }
        if g::INVALID_CODEPOINTS.binary_search(&(n as u32)).is_ok() {
            return String::new();
        }
        return char::from_u32(n as u32).map_or_else(String::new, String::from);
    }
    if let Some(v) = html5(s) {
        return v.to_owned();
    }
    let chars: Vec<char> = s.chars().collect();
    for x in (2..chars.len()).rev() {
        let head: String = chars[..x].iter().collect();
        if let Some(v) = html5(&head) {
            let tail: String = chars[x..].iter().collect();
            return format!("{v}{tail}");
        }
    }
    format!("&{s}")
}

/// `html.unescape(s)`.
pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < n {
        if chars[i] != '&' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // &(#[0-9]+;?|#[xX][0-9a-fA-F]+;?|[^\t\n\f <&#;]{1,32};?)
        let mut end = None;
        if chars.get(i + 1) == Some(&'#') {
            let mut j = i + 2;
            while j < n && chars[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 2 {
                end = Some(j);
            } else if matches!(chars.get(i + 2), Some('x' | 'X')) {
                let mut j = i + 3;
                while j < n && chars[j].is_ascii_hexdigit() {
                    j += 1;
                }
                if j > i + 3 {
                    end = Some(j);
                }
            }
        } else {
            let mut j = i + 1;
            while j < n
                && j - (i + 1) < 32
                && !matches!(chars[j], '\t' | '\n' | '\x0c' | ' ' | '<' | '&' | '#' | ';')
            {
                j += 1;
            }
            if j > i + 1 {
                end = Some(j);
            }
        }
        match end {
            Some(mut j) => {
                if chars.get(j) == Some(&';') {
                    j += 1;
                }
                let group: String = chars[i + 1..j].iter().collect();
                out.push_str(&replace_charref(&group));
                i = j;
            }
            None => {
                out.push('&');
                i += 1;
            }
        }
    }
    out
}

/// What a parser user hears (`handle_starttag`, `handle_endtag`, `handle_data`; a `<x/>` is a start then an end).
pub trait Handler {
    fn start(&mut self, tag: &str, attrs: &[(String, Option<String>)]);
    fn end(&mut self, tag: &str);
    fn data(&mut self, data: &str);
}

const CDATA_CONTENT: [&str; 6] = ["script", "style", "xmp", "iframe", "noembed", "noframes"];
const RCDATA_CONTENT: [&str; 2] = ["textarea", "title"];

fn ws5(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\x0c' | ' ')
}

/// `[\t\n\r\f />]`.
fn name_stop(c: char) -> bool {
    ws5(c) || c == '/' || c == '>'
}

struct Parser<'h> {
    raw: Vec<char>,
    cdata_elem: Option<String>,
    escapable: bool,
    handler: &'h mut dyn Handler,
}

fn text(chars: &[char]) -> String {
    chars.iter().collect()
}

impl Parser<'_> {
    fn find(&self, needle: &str, from: usize) -> Option<usize> {
        let pat: Vec<char> = needle.chars().collect();
        if from > self.raw.len() {
            return None;
        }
        (from..=self.raw.len().saturating_sub(pat.len()))
            .find(|&p| p + pat.len() <= self.raw.len() && self.raw[p..p + pat.len()] == pat[..])
    }

    fn starts(&self, at: usize, s: &str) -> bool {
        let pat: Vec<char> = s.chars().collect();
        at + pat.len() <= self.raw.len() && self.raw[at..at + pat.len()] == pat[..]
    }

    fn at(&self, i: usize) -> Option<char> {
        self.raw.get(i).copied()
    }

    fn data(&mut self, s: &str) {
        self.handler.data(s);
    }

    fn data_maybe_unescaped(&mut self, a: usize, b: usize) {
        let s = text(&self.raw[a..b]);
        if self.escapable {
            self.handler.data(&unescape(&s));
        } else {
            self.handler.data(&s);
        }
    }

    /// `</elem(?=[\t\n\r\f />])` (ASCII case-insensitive), or the end for `plaintext`.
    fn interesting(&self, from: usize) -> Option<usize> {
        let elem = self.cdata_elem.as_deref().unwrap_or("");
        if elem == "plaintext" {
            return Some(self.raw.len());
        }
        let pat: Vec<char> = format!("</{elem}").chars().collect();
        let n = self.raw.len();
        (from..n).find(|&p| {
            p + pat.len() < n
                && self.raw[p..p + pat.len()]
                    .iter()
                    .zip(&pat)
                    .all(|(a, b)| a.eq_ignore_ascii_case(b))
                && name_stop(self.raw[p + pat.len()])
        })
    }

    fn goahead(&mut self, end: bool) {
        let n = self.raw.len();
        let mut i = 0;
        while i < n {
            let j;
            if self.cdata_elem.is_none() {
                match self.find("<", i) {
                    Some(p) => j = p,
                    None => {
                        let lo = i.max(n.saturating_sub(34));
                        let amp = (lo..n).rev().find(|&p| self.raw[p] == '&');
                        if let Some(a) = amp {
                            if !self.raw[a..]
                                .iter()
                                .any(|&c| matches!(c, '\t' | '\n' | '\r' | '\x0c' | ' ' | ';'))
                            {
                                break;
                            }
                        }
                        j = n;
                    }
                }
            } else {
                match self.interesting(i) {
                    Some(p) => j = p,
                    None => break,
                }
            }
            if i < j {
                self.data_maybe_unescaped(i, j);
            }
            i = j;
            if i == n {
                break;
            }
            // raw[i] == '<' (with convert_charrefs the '&' branches are never reached)
            let next = self.at(i + 1);
            let mut k = if next.is_some_and(|c| c.is_ascii_alphabetic()) {
                self.parse_starttag(i)
            } else if self.starts(i, "</") {
                self.parse_endtag(i)
            } else if self.starts(i, "<!--") {
                self.parse_comment(i)
            } else if self.starts(i, "<?") {
                self.parse_pi(i)
            } else if self.starts(i, "<!") {
                self.parse_html_declaration(i)
            } else if i + 1 < n || end {
                self.data("<");
                Some(i + 1)
            } else {
                break;
            };
            if k.is_none() {
                if !end {
                    break;
                }
                // an unfinished start tag, end tag or markup is dropped; a bare "</" at the end is text
                if !next.is_some_and(|c| c.is_ascii_alphabetic())
                    && self.starts(i, "</")
                    && i + 2 == n
                {
                    self.data("</");
                }
                k = Some(n);
            }
            i = k.unwrap_or(n);
        }
        if end && i < n {
            self.data_maybe_unescaped(i, n);
            i = n;
        }
        self.raw.drain(..i);
    }

    fn parse_html_declaration(&mut self, i: usize) -> Option<usize> {
        if self.starts(i, "<!--") {
            return self.parse_comment(i);
        }
        if self.starts(i, "<![CDATA[") {
            return self.find("]]>", i + 9).map(|j| j + 3);
        }
        let head = text(&self.raw[i..(i + 9).min(self.raw.len())]);
        if pystr::lower(&head) == "<!doctype" {
            return self.find(">", i + 9).map(|g| g + 1);
        }
        if self.starts(i, "<![") {
            return self.find(">", i + 3).map(|j| j + 1);
        }
        self.parse_bogus_comment(i)
    }

    fn parse_comment(&mut self, i: usize) -> Option<usize> {
        // commentclose `--!?>` searched, else commentabruptclose `-?>` matched at i+4
        let n = self.raw.len();
        let mut p = i + 4;
        while p + 1 < n {
            if self.raw[p] == '-' && self.raw[p + 1] == '-' {
                if self.at(p + 2) == Some('>') {
                    return Some(p + 3);
                }
                if self.at(p + 2) == Some('!') && self.at(p + 3) == Some('>') {
                    return Some(p + 4);
                }
            }
            p += 1;
        }
        let q = i + 4;
        if self.at(q) == Some('>') {
            return Some(q + 1);
        }
        if self.at(q) == Some('-') && self.at(q + 1) == Some('>') {
            return Some(q + 2);
        }
        None
    }

    fn parse_bogus_comment(&mut self, i: usize) -> Option<usize> {
        self.find(">", i + 2).map(|p| p + 1)
    }

    fn parse_pi(&mut self, i: usize) -> Option<usize> {
        self.find(">", i + 2).map(|j| j + 1)
    }

    /// The value after an attribute name at `q2`: `[ws]*=[ws]*('...'|"..."|(?!['"])[^>ws]*)`, as the regex
    /// engine backtracks it. Returns (where the value starts, where it ends).
    fn value(&self, q2: usize) -> Option<(usize, usize)> {
        let n = self.raw.len();
        let mut a = q2;
        while a < n && ws5(self.raw[a]) {
            a += 1;
        }
        if self.at(a) != Some('=') {
            return None;
        }
        let b0 = a + 1;
        let mut b = b0;
        while b < n && ws5(self.raw[b]) {
            b += 1;
        }
        let value_at = |pos: usize| -> Option<usize> {
            match self.at(pos) {
                Some(q @ ('\'' | '"')) => (pos + 1..n).find(|&e| self.raw[e] == q).map(|e| e + 1),
                _ => {
                    let mut e = pos;
                    while e < n && !(self.raw[e] == '>' || ws5(self.raw[e])) {
                        e += 1;
                    }
                    Some(e)
                }
            }
        };
        if let Some(e) = value_at(b) {
            return Some((b, e));
        }
        if b > b0 {
            return Some((b - 1, b - 1));
        }
        None
    }

    /// An attribute starting at `q` (its lookbehind and first character checked): (name end, value).
    fn attr_name(&self, q: usize) -> Option<usize> {
        let n = self.raw.len();
        if q == 0 || q >= n {
            return None;
        }
        if !matches!(
            self.raw[q - 1],
            '\'' | '"' | '\t' | '\n' | '\r' | '\x0c' | ' ' | '/'
        ) {
            return None;
        }
        if name_stop(self.raw[q]) {
            return None;
        }
        let mut q2 = q + 1;
        while q2 < n && !(name_stop(self.raw[q2]) || self.raw[q2] == '=') {
            q2 += 1;
        }
        Some(q2)
    }

    /// `locatetagend.match(raw, p)` (p at the tag name's first letter): where the match ends.
    fn locate_tag_end(&self, p: usize) -> usize {
        let n = self.raw.len();
        let mut q = p + 1;
        while q < n && !name_stop(self.raw[q]) {
            q += 1;
        }
        while q < n && (ws5(self.raw[q]) || self.raw[q] == '/') {
            q += 1;
        }
        while let Some(q2) = self.attr_name(q) {
            q = self.value(q2).map_or(q2, |(_, e)| e);
            while q < n && (ws5(self.raw[q]) || self.raw[q] == '/') {
                q += 1;
            }
        }
        if self.at(q) == Some('>') {
            q += 1;
        }
        q
    }

    /// `tagfind_tolerant.match(raw, p)`: (the name, where the match ends).
    fn tag_find(&self, p: usize) -> (String, usize) {
        let n = self.raw.len();
        let mut q = p + 1;
        while q < n && !name_stop(self.raw[q]) {
            q += 1;
        }
        let name = text(&self.raw[p..q]);
        q = self.trailing(q);
        (name, q)
    }

    /// `(?:[\t\n\r\f ]|/(?!>))*`.
    fn trailing(&self, mut q: usize) -> usize {
        let n = self.raw.len();
        while q < n {
            if ws5(self.raw[q]) || (self.raw[q] == '/' && self.at(q + 1) != Some('>')) {
                q += 1;
            } else {
                break;
            }
        }
        q
    }

    fn parse_starttag(&mut self, i: usize) -> Option<usize> {
        let endpos = self.locate_tag_end(i + 1);
        if endpos == 0 || self.raw[endpos - 1] != '>' {
            return None;
        }
        let (name, mut k) = self.tag_find(i + 1);
        let tag = pystr::lower(&name);
        let mut attrs = Vec::new();
        while k < endpos {
            let Some(q2) = self.attr_name(k) else { break };
            let attrname = text(&self.raw[k..q2]);
            let (value, after) = match self.value(q2) {
                Some((vs, ve)) => {
                    let mut v = text(&self.raw[vs..ve]);
                    let quoted = v.len() >= 2
                        && ((v.starts_with('\'') && v.ends_with('\''))
                            || (v.starts_with('"') && v.ends_with('"')));
                    if quoted {
                        v = v[1..v.len() - 1].to_owned();
                    }
                    if !v.is_empty() {
                        v = unescape(&v);
                    }
                    (Some(v), ve)
                }
                None => (None, q2),
            };
            attrs.push((pystr::lower(&attrname), value));
            k = self.trailing(after);
        }
        let end_text = text(&self.raw[k.min(endpos)..endpos]);
        let end_text = pystr::strip(&end_text);
        if end_text != ">" && end_text != "/>" {
            let raw = text(&self.raw[i..endpos]);
            self.data(&raw);
            return Some(endpos);
        }
        if end_text.ends_with("/>") {
            self.handler.start(&tag, &attrs);
            self.handler.end(&tag);
        } else {
            self.handler.start(&tag, &attrs);
            if CDATA_CONTENT.contains(&tag.as_str()) || tag == "plaintext" {
                self.cdata_elem = Some(tag.clone());
                self.escapable = false;
            } else if RCDATA_CONTENT.contains(&tag.as_str()) {
                self.cdata_elem = Some(tag.clone());
                self.escapable = true;
            }
        }
        Some(endpos)
    }

    fn parse_endtag(&mut self, i: usize) -> Option<usize> {
        self.find(">", i + 2)?;
        if !self.at(i + 2).is_some_and(|c| c.is_ascii_alphabetic()) {
            if self.at(i + 2) == Some('>') {
                return Some(i + 3);
            }
            return self.parse_bogus_comment(i);
        }
        let j = self.locate_tag_end(i + 2);
        if self.raw[j - 1] != '>' {
            return None;
        }
        let (name, _) = self.tag_find(i + 2);
        self.handler.end(&pystr::lower(&name));
        self.cdata_elem = None;
        self.escapable = true;
        Some(j)
    }
}

/// `parser.feed(text)`, then `parser.close()` when `close` (Python's Gemini reader never closes its parsers; the
/// Rust reader does: see `import_sources`).
pub fn parse(text: &str, handler: &mut dyn Handler, close: bool) {
    let mut p = Parser {
        raw: text.chars().collect(),
        cdata_elem: None,
        escapable: true,
        handler,
    };
    p.goahead(false);
    if close {
        p.goahead(true);
    }
}
