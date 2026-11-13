//! Port of ferb/sanitize/stream.py: incremental redaction of process output (a PTY, a shell, a streamed reply):
//! text is released as it comes and masked exactly as `Redactor::text` masks the whole. Raw secret bytes are never
//! released, even partially.
//!
//! The stream releases complete lines at once, and the line being read up to its last word, except while later
//! text can still change a mask in it (see stream.py:1-10). Released text is masked by `Redactor::text` over it and
//! the few lines before it that it can relate to (`context`).
//!
//! `pending` is a `String`; every length Python counts in code points (MAX_PENDING, the 128 kept while a key is
//! suppressed, the long-line bounds) is counted in characters here.

use std::collections::BTreeSet;
use std::ops::Deref;

use crate::formats::{is_h, private_key_end, private_key_hits, private_key_starts, urls};
use crate::generated as g;
use crate::known::REDACTED;
use crate::labels::{
    self, code_alone, label_class, label_line, line_breaks, link_line, list_label_line, list_line,
    marks_from, quote_only, secret_after, secret_words, CODE_REACH,
};
use crate::redact::Redactor;
use crate::rx::{checked, Rx, Span, Unread, UNREADABLE};
use crate::unicode as u;

static HOLD_WORDS: Rx = Rx::new(&g::STREAM_HOLD_WORDS);
static LINK_START: Rx = Rx::new(&g::STREAM_LINK_START);
static UNFINISHED_TAG: Rx = Rx::new(&g::STREAM_UNFINISHED_TAG);
static CODE_LEAD_START: Rx = Rx::new(&g::STREAM_CODE_LEAD_START);
static CODE_START: Rx = Rx::new(&g::STREAM_CODE_START);
static LIST_LINE_START: Rx = Rx::new(&g::STREAM_LIST_LINE_START);
static NUMERIC_TAIL: Rx = Rx::new(&g::STREAM_NUMERIC_TAIL);
static NUMERIC_START: Rx = Rx::new(&g::STREAM_NUMERIC_START);
const HEAD_WORDS: usize = 2 * CODE_REACH + 2; // a released line's start kept as context
const HEAD_CHARS: usize = 4096;
const LONG_LINE: usize = 2048; // a relation line longer than this is kept as its last words

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Relation {
    Label,
    Secret,
    List,
}

/// stream.py:50: a line ending in a secret word and its ":" or "=", whose value may be on the next line.
fn secret_colon_end(line: &str) -> bool {
    let end = u::rstrip(line).len();
    secret_words(line).into_iter().any(|word| {
        secret_after(line, word.1).is_some_and(|(after_end, link)| {
            after_end >= end && line[link.0..link.1].contains([':', '='])
        })
    })
}

/// stream.py:59: whether the line being read can still become a code alone.
fn code_alone_start(line: &str) -> bool {
    let lead = line.len() - u::lstrip(line).len();
    let a = CODE_LEAD_START.match_at(line, lead).map_or(lead, |m| m.1);
    CODE_START
        .match_at(line, marks_from(line, a, line.len()))
        .is_some()
}

/// stream.py:66: a code alone so far, an unfinished tag at its end allowed.
fn code_alone_so_far(line: &str) -> bool {
    match line.rfind('<') {
        Some(lt) if UNFINISHED_TAG.fullmatch_in(line, lt, line.len()) => code_alone(&line[..lt]),
        _ => code_alone(line),
    }
}

/// `[\d{_H},.-]` (stream.py:41-43).
fn numeric_char(c: char) -> bool {
    u::is_decimal(c) || is_h(c) || matches!(c, ',' | '.' | '-')
}

/// stream.py:72: where the digits that end `tail` start; only the runs reaching the last word are tried.
fn numeric_tail(tail: &str) -> Option<usize> {
    // Python matches `_LAST_WORD` and `_NUMERIC_RUN` on the reversed text; read backwards here instead.
    let mut word = tail.len();
    for (i, c) in tail.char_indices().rev() {
        if u::is_space(c) {
            break;
        }
        word = i;
    }
    let run = tail[..word]
        .chars()
        .rev()
        .take_while(|&c| numeric_char(c))
        .count();
    // Python reads the run on the reversed text from offset len - word and subtracts the match's *end* (an offset,
    // not a length) from `word`: `start = word - ((len - word) + run)`, and a negative start reads from 0. The
    // search so starts earlier than the run by the last word's length; kept as Python has it (stream.py:76).
    let (word_chars, last_word) = (u::char_len(&tail[..word]), u::char_len(&tail[word..]));
    let start = u::forward(tail, 0, word_chars.saturating_sub(last_word + run));
    for run in NUMERIC_START.find_all_in(tail, start, tail.len()) {
        if let Some(digits) = NUMERIC_TAIL.match_at(tail, run.0) {
            return Some(digits.1);
        }
    }
    None
}

/// stream.py:83: a released line's start as context: its last words.
fn head(line: &str) -> &str {
    let tail = u::last_chars(line, HEAD_CHARS);
    let found = u::word_spans(tail);
    if found.len() > HEAD_WORDS {
        &tail[found[found.len() - HEAD_WORDS].0..]
    } else {
        tail
    }
}

/// stream.py:90: the complete lines of `s`: (start, end, end of its line break).
fn lines_of(s: &str) -> Vec<(usize, usize, usize)> {
    let (mut out, mut begin) = (Vec::new(), 0);
    for (a, b) in line_breaks(s) {
        out.push((begin, a, b));
        begin = b;
    }
    out
}

/// stream.py:99: how the line after `lines` relates to the last of them that isn't blank.
fn relation(lines: &[(usize, usize, usize)], s: &str) -> (usize, BTreeSet<Relation>) {
    let text = |i: isize| &s[lines[i as usize].0..lines[i as usize].1];
    let blank = |i: isize| u::strip(text(i)).is_empty();
    let mut i = lines.len() as isize - 1;
    while i >= 0 && blank(i) {
        i -= 1;
    }
    let mut quoted = false;
    while i >= 0 && quote_only(text(i)) {
        quoted = true;
        i -= 1;
        while i >= 0 && blank(i) {
            i -= 1;
        }
    }
    let mut kinds = BTreeSet::new();
    if i < 0 {
        return (s.len(), kinds);
    }
    let line = text(i);
    let start = lines[i as usize].0;
    if label_line(line) {
        kinds.insert(Relation::Label);
    }
    if quoted {
        return (start, kinds);
    }
    if secret_colon_end(line) {
        kinds.insert(Relation::Secret);
    }
    if list_label_line(line) {
        kinds.insert(Relation::List);
    } else if list_line(line) {
        let mut j = i - 1;
        while j >= 0 {
            let above = text(j);
            if !u::strip(above).is_empty() && !list_line(above) {
                if list_label_line(above) {
                    kinds.insert(Relation::List);
                    return (lines[j as usize].0, kinds);
                }
                break;
            }
            j -= 1;
        }
    }
    (start, kinds)
}

/// stream.py:132: what of the released text `raw` later text can relate to.
fn context_of(raw: &str) -> String {
    let lines = lines_of(raw);
    let Some(&(_, _, last_end)) = lines.last() else {
        return head(raw).to_string();
    };
    let tail = &raw[last_end..];
    let head_text = if tail.is_empty() { "" } else { head(tail) };
    let (start, kinds) = relation(&lines, raw);
    if start >= raw.len() {
        return head_text.to_string();
    }
    let mut kept = raw[start..last_end].to_string();
    let Some(&(_, line_end, label_end)) = lines.iter().find(|l| l.0 == start) else {
        return head_text.to_string();
    };
    let after: Vec<usize> = lines
        .iter()
        .filter(|l| l.0 >= label_end && !u::strip(&raw[l.0..l.1]).is_empty())
        .map(|l| l.0)
        .collect();
    if kinds.contains(&Relation::List) && !after.is_empty() {
        // a list's label, then its last list line and what follows it
        kept = format!(
            "{}{}",
            &raw[start..label_end],
            &raw[after[after.len() - 1]..last_end]
        );
    } else if u::longer_than(&kept, LONG_LINE) && !kinds.contains(&Relation::List) {
        kept = format!(
            "{}{}",
            head(&raw[start..line_end]),
            u::last_chars(&raw[line_end..last_end], LONG_LINE)
        );
    }
    kept + head_text
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Suppress {
    Private,
    Record,
}

/// stream.py:154: incremental UTF-8 redaction. Emits complete words, retaining possible secrets across chunks.
///
/// An oversized unfinished record is replaced by an explicit marker rather than spilling raw bytes. Private-key
/// blocks are suppressed until their end marker, even when they exceed the buffer bound.
pub struct RedactionStream<R: Deref<Target = Redactor>> {
    redactor: R,
    /// Python's `MAX_PENDING` (65536): how many characters may wait before the oversized rule. A field so a test
    /// can lower it, as a Python test can on an instance.
    pub max_pending: usize,
    undecoded: Vec<u8>,
    pending: String,
    suppress: Option<Suppress>,
    context: String,
}

impl<R: Deref<Target = Redactor>> std::fmt::Debug for RedactionStream<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // never the held text: it is what must not leak
        write!(
            f,
            "RedactionStream({} bytes pending, suppress {:?})",
            self.pending.len(),
            self.suppress
        )
    }
}

pub const MAX_PENDING: usize = 65536;
/// What replaces a record that outgrew the buffer with no end in sight (stream.py:317).
pub const OVERSIZED: &str = "[redacted: oversized unfinished record]\n";

impl<R: Deref<Target = Redactor>> RedactionStream<R> {
    pub fn new(redactor: R) -> RedactionStream<R> {
        RedactionStream {
            redactor,
            max_pending: MAX_PENDING,
            undecoded: Vec::new(),
            pending: String::new(),
            suppress: None,
            context: String::new(),
        }
    }

    /// `codecs.getincrementaldecoder("utf-8")(errors="replace").decode(chunk, final)`: each ill-formed sequence
    /// becomes one U+FFFD; a sequence cut by the chunk's end waits for the next chunk (or, at the end, is one
    /// U+FFFD).
    fn decode(&mut self, chunk: &[u8], last: bool) {
        self.undecoded.extend_from_slice(chunk);
        let bytes = std::mem::take(&mut self.undecoded);
        let mut rest: &[u8] = &bytes;
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    self.pending.push_str(text);
                    break;
                }
                Err(e) => {
                    let (valid, bad) = rest.split_at(e.valid_up_to());
                    self.pending
                        .push_str(std::str::from_utf8(valid).unwrap_or(""));
                    match e.error_len() {
                        Some(n) => {
                            self.pending.push('\u{fffd}');
                            rest = &bad[n..];
                        }
                        None if last => {
                            self.pending.push('\u{fffd}');
                            break;
                        }
                        None => {
                            self.undecoded = bad.to_vec();
                            break;
                        }
                    }
                }
            }
        }
    }

    /// stream.py:171: where the first BEGIN line with no END line after it starts.
    fn unfinished_key(&self) -> Option<usize> {
        let mut end: Option<Span> = None;
        for opening in private_key_starts(&self.pending) {
            if end.is_none_or(|e| e.0 < opening.1) {
                end = private_key_end(&self.pending, opening.1);
                if end.is_none() {
                    return Some(opening.0);
                }
            }
        }
        None
    }

    /// stream.py:180: whether later text in the line being read can still change a mask in what has been read.
    fn hold_line(&self, line: &str, kinds: &BTreeSet<Relation>) -> bool {
        if labels::CODE_WORD.search(line, 0).is_some()
            || labels::SECRET_WORD.search(line, 0).is_some()
            || HOLD_WORDS.search(line, 0).is_some()
            || labels::OTP_LABEL.search(line, 0).is_some()
            || code_alone_so_far(line)
        {
            return true;
        }
        if labels::KEY_LABEL.caps_all(line).iter().any(|m| {
            m[2].is_some_and(|(a, b)| label_class(line[a..b].trim_start_matches('-')).is_some())
        }) {
            return true;
        }
        if !u::strip(line).is_empty()
            && (kinds.contains(&Relation::Secret)
                || (kinds.contains(&Relation::List) && LIST_LINE_START.match_at(line, 0).is_some())
                || (kinds.contains(&Relation::Label) && code_alone_start(line)))
        {
            return true;
        }
        let url_spans = urls(line); // a URL's own digits are never a code
        labels::CODE_VALUE.find_all(line).into_iter().any(|m| {
            (labels::CODE_LINK.match_at(line, m.1).is_some()
                || LINK_START.fullmatch_in(line, m.1, line.len()))
                && !url_spans.iter().any(|&(a, b)| a <= m.0 && m.0 < b)
        })
    }

    /// stream.py:194.
    fn safe_cut(&self) -> usize {
        let p = self.pending.as_str();
        let mut lines = lines_of(p);
        if lines.last().is_some_and(|l| l.2 == p.len()) && p.ends_with('\r') {
            lines.pop(); // a "\r" that ends the text may be half of "\r\n"
        }
        let line_start = lines.last().map_or(0, |l| l.2);
        let mut cut = p.len();
        let released = self.context.as_str();
        // the released start of a line
        let head = lines_of(released)
            .last()
            .map_or(released, |l| &released[l.2..]);
        let tail = &p[line_start..];
        if !tail.is_empty() {
            let line = format!("{}{}", if line_start == 0 { head } else { "" }, tail);
            let above = format!("{}{}", released, &p[..line_start]);
            let kinds = relation(&lines_of(&above), &above).1;
            if self.hold_line(&line, &kinds) {
                cut = line_start;
                if u::longer_than(tail, self.max_pending / 2) && !kinds.contains(&Relation::Secret)
                {
                    let found = u::word_spans(tail); // a very long line: all but its last words
                    if found.len() > HEAD_WORDS {
                        cut = line_start + found[found.len() - HEAD_WORDS].0;
                    }
                }
            } else {
                // `re.search(r"\s(?=\S*$)", tail)`: the last whitespace of the line
                let last = tail
                    .char_indices()
                    .rev()
                    .find(|&(_, c)| u::is_space(c))
                    .map(|(i, c)| i + c.len_utf8());
                cut = line_start + last.unwrap_or(0);
                if let Some(numeric) = numeric_tail(tail) {
                    let mut start = line_start + numeric;
                    while start > line_start {
                        match u::char_before(p, start) {
                            Some(c) if !u::is_space(c) => start -= c.len_utf8(),
                            _ => break,
                        }
                    }
                    cut = cut.min(start);
                }
            }
            if let Some(begin) = tail.find("-----BEGIN") {
                cut = cut.min(line_start + begin);
            }
        }
        if let Some(opening) = self.unfinished_key() {
            cut = cut.min(opening);
        }
        let blocks = if p.contains("-----BEGIN") {
            private_key_hits(p)
        } else {
            Vec::new()
        };
        loop {
            // each guard only moves the cut back: until none moves it
            let before = cut;
            for hit in &blocks {
                if hit.start < cut && cut < hit.end {
                    cut = hit.start; // never inside a whole key block
                }
            }
            cut = self.known_cut(cut);
            while cut > 0 && cut < p.len() {
                // never inside a token: its rest may still mask its start
                match (u::char_before(p, cut), u::char_at(p, cut)) {
                    (Some(a), Some(b)) if !u::is_space(a) && !u::is_space(b) => cut -= a.len_utf8(),
                    _ => break,
                }
            }
            cut = self.line_cut(cut, head);
            cut = self.code_alone_cut(cut, head);
            if cut == before {
                return cut;
            }
        }
    }

    /// stream.py:241: a code alone on the last line before the cut waits while the line after it is still missing,
    /// or is released with it when that line names it.
    fn code_alone_cut(&self, cut: usize, head: &str) -> usize {
        let p = self.pending.as_str();
        let head_content = !u::strip(head).is_empty();
        let before = lines_of(&p[..cut])
            .into_iter()
            .rfind(|&(a, b, _)| !u::strip(&p[a..b]).is_empty() || (a == 0 && head_content));
        let Some((a, b, _)) = before else { return cut };
        let line = format!("{}{}", if a == 0 { head } else { "" }, &p[a..b]);
        if !code_alone(&line) {
            return cut;
        }
        let after = lines_of(p)
            .into_iter()
            .find(|&(x, y, _)| x >= cut && !u::strip(&p[x..y]).is_empty());
        match after {
            Some((x, y, _)) if !link_line(&p[x..y]) => cut,
            _ => a,
        }
    }

    /// stream.py:254: a cut a guard moved inside a line goes back to the line's start when the line holds anything
    /// a later word in it can change; a very long line is cut by its own rule.
    fn line_cut(&self, cut: usize, head: &str) -> usize {
        let p = self.pending.as_str();
        let bytes = p.as_bytes();
        if cut == 0 || cut >= p.len() || matches!(bytes[cut - 1], b'\r' | b'\n') {
            return cut;
        }
        let start = p[..cut].rfind(['\n', '\r']).map_or(0, |i| i + 1);
        let end = p[cut..].find(['\n', '\r']).map_or(p.len(), |i| cut + i);
        if u::longer_than(&p[start..end], self.max_pending / 2) {
            return cut; // a very long line keeps only its last words (`safe_cut`)
        }
        let above = format!("{}{}", self.context, &p[..start]);
        let line = format!("{}{}", if start == 0 { head } else { "" }, &p[start..end]);
        if self.hold_line(&line, &relation(&lines_of(&above), &above).1) {
            start
        } else {
            cut
        }
    }

    /// stream.py:268: never inside a known value, nor after the start of one that may still be coming.
    fn known_cut(&self, mut cut: usize) -> usize {
        let p = self.pending.as_str();
        for value in self.redactor.secret_values().iter() {
            let mut from = 0;
            while let Some(i) = p[from..].find(value) {
                let start = from + i;
                if start >= cut {
                    break;
                }
                if cut < start + value.len() {
                    cut = start;
                }
                from = crate::rx::next_char(p, start);
            }
            // the longest proper prefix of the value that ends the text
            let mut prefixes: Vec<usize> = value
                .char_indices()
                .map(|(i, _)| i)
                .filter(|&i| i > 0)
                .collect();
            prefixes.reverse();
            for len in prefixes {
                if len <= p.len() && p.ends_with(&value[..len]) {
                    cut = cut.min(p.len() - len);
                    break;
                }
            }
        }
        cut
    }

    /// stream.py:283: `pending[:n]` masked as `Redactor::text` masks it after the released text it can relate to.
    fn release(&mut self, n: usize) -> String {
        let released: String = self.pending.drain(..n).collect();
        if released.is_empty() {
            return String::new();
        }
        let out = if self.context.is_empty() {
            self.redactor.text(&released)
        } else {
            let before = self.redactor.text(&self.context);
            let whole = self.redactor.text(&format!("{}{}", self.context, released));
            match whole.strip_prefix(before.as_str()) {
                Some(rest) => rest.to_string(),
                None => self.redactor.text(&released),
            }
        };
        self.context = context_of(&format!("{}{}", self.context, released));
        out
    }

    /// stream.py:297: feed the next bytes; returns what may be shown. `last`: Python's `final`, no more follows.
    ///
    /// When a pattern cannot be run over the held text (rx.rs), nothing of it is shown: `UNREADABLE` and a line
    /// break stand for it, the held text is dropped, and what follows is dropped up to the next line break (as
    /// for an oversized record) or, inside a private key, up to its END line.
    pub fn feed(&mut self, chunk: &[u8], last: bool) -> Vec<u8> {
        let in_key = self.suppress == Some(Suppress::Private);
        match checked(|| self.feed_read(chunk, last)) {
            Ok(out) => out,
            Err(Unread) => {
                self.pending.clear();
                self.context.clear();
                self.suppress = Some(if in_key {
                    Suppress::Private
                } else {
                    Suppress::Record
                });
                format!("{UNREADABLE}\n").into_bytes()
            }
        }
    }

    fn feed_read(&mut self, chunk: &[u8], last: bool) -> Vec<u8> {
        self.decode(chunk, last);
        if let Some(suppress) = self.suppress {
            let end = match suppress {
                Suppress::Private => private_key_end(&self.pending, 0).map(|m| m.1),
                Suppress::Record => self.pending.find('\n').map(|i| i + 1),
            };
            match end {
                None => {
                    self.pending = if suppress == Suppress::Private && !last {
                        u::last_chars(&self.pending, 128).to_string()
                    } else {
                        String::new()
                    };
                    return Vec::new();
                }
                Some(end) => {
                    self.pending.drain(..end);
                    self.suppress = None;
                    self.context.clear();
                }
            }
        }
        if last {
            let n = self.pending.len();
            return self.release(n).into_bytes();
        }
        let cut = self.safe_cut();
        let mut out = self.release(cut);
        let longest = self
            .redactor
            .secret_values()
            .iter()
            .map(u::char_len)
            .max()
            .unwrap_or(0);
        let bound = self.max_pending.max(longest + 1);
        if u::longer_than(&self.pending, bound) {
            if let Some(opening) = self.unfinished_key() {
                out.push_str(&self.release(opening));
                out.push_str(REDACTED);
                self.pending = u::last_chars(&self.pending, 128).to_string();
                self.suppress = Some(Suppress::Private);
            } else {
                out.push_str(OVERSIZED);
                self.pending.clear();
                self.suppress = Some(Suppress::Record);
            }
            self.context.clear();
        }
        out.into_bytes()
    }
}
