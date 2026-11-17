//! FERB's words as FERB.app draws them (kb PHN-720..): Markdown read into blocks of styled runs, for both phones from
//! one parser, so the Android and the iPhone bubble say the same as FERB.app's.
//!
//! The blocks are FERB.app's `MarkdownDocument` (apps/mac/FERBNotch/Sources/NotchCore/MarkdownDocument.swift), line for
//! line: paragraphs (their lines kept), headings 1-6, list items with their marker and level, quotes, fenced code with
//! its language, pipe tables, dividers. Inside a block, FERB.app hands the text to Foundation's inline Markdown
//! (`AttributedString(markdown:, .inlineOnlyPreservingWhitespace)`) and then marks the plain addresses a data detector
//! finds; here a CommonMark-style inline reader does the same: bold, italics, strikethrough, code, links, and bare
//! addresses. Only `http`, `https` and `mailto` links are made (APP-589, `MarkdownDocument.safeLink`); an image is its
//! words only and nothing is ever fetched; HTML stays text.
//!
//! The text is read from what FERB wrote, which can quote the web: every step is bounded in time and in what it makes
//! (`MAX_CHARS`, `MAX_BLOCKS`, `MAX_COLUMNS`, `MAX_ROWS`, `MAX_LEVEL`, the emphasis look-back and the link scans), so a
//! pathological answer costs at most linear work and a capped result.

use serde::Serialize;

/// The most characters read; the rest is left out and the text ends with "…".
pub const MAX_CHARS: usize = 100_000;
/// The most blocks made; past it, one last paragraph says "…".
pub const MAX_BLOCKS: usize = 2_000;
/// A table's most columns and rows (more stay out).
pub const MAX_COLUMNS: usize = 12;
pub const MAX_ROWS: usize = 200;
/// The deepest list level drawn.
pub const MAX_LEVEL: u32 = 8;
/// How far back a closing `*`, `_` or `~` looks for its opener (CommonMark looks to the start; this bounds it).
const LOOK_BACK: usize = 64;
/// How far a link's address or an autolink is read before it is taken as text.
const MAX_ADDRESS: usize = 2_048;

/// A run of text with one look.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    /// An address to open (http, https or mailto only).
    pub link: Option<String>,
}

impl Span {
    fn plain(text: impl Into<String>) -> Span {
        Span {
            text: text.into(),
            ..Span::default()
        }
    }
    fn same_look(&self, o: &Span) -> bool {
        self.bold == o.bold
            && self.italic == o.italic
            && self.code == o.code
            && self.strike == o.strike
            && self.link == o.link
    }
}

/// A table cell's text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Cell {
    pub spans: Vec<Span>,
}

/// One row of a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub cells: Vec<Cell>,
}

/// One block (`MarkdownDocument.Block`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "block", rename_all = "snake_case")]
pub enum Block {
    Paragraph {
        spans: Vec<Span>,
    },
    /// `level` 1 to 6 (FERB.app: 17, 15, then 13 points).
    Heading {
        level: u32,
        spans: Vec<Span>,
    },
    /// `marker` is "•" for -, + and *, else the number as written ("1." or "2)"); `level` counts two spaces each.
    ListItem {
        marker: String,
        level: u32,
        spans: Vec<Span>,
    },
    Quote {
        spans: Vec<Span>,
    },
    Code {
        text: String,
        language: String,
    },
    /// `align` per column: "leading", "center" or "trailing".
    Table {
        header: Vec<Cell>,
        rows: Vec<Row>,
        align: Vec<String>,
    },
    Divider,
}

/// `text` read as FERB.app reads it.
pub fn document(source: &str) -> Vec<Block> {
    let (source, cut) = capped(source);
    let normal = source.replace("\r\n", "\n");
    let lines: Vec<&str> = normal.split('\n').collect();
    let mut out: Vec<Block> = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let flush = |paragraph: &mut Vec<&str>, out: &mut Vec<Block>| {
        if !paragraph.is_empty() {
            out.push(Block::Paragraph {
                spans: inline(&paragraph.join("\n")),
            });
            paragraph.clear();
        }
    };
    let mut i = 0;
    while i < lines.len() {
        if out.len() >= MAX_BLOCKS {
            paragraph.clear();
            out.push(Block::Paragraph {
                spans: vec![Span::plain("…")],
            });
            return out;
        }
        let line = lines[i];
        let trimmed = trim_ws(line);
        if trimmed.is_empty() {
            flush(&mut paragraph, &mut out);
            i += 1;
            continue;
        }
        if let Some(fence) = fence(trimmed) {
            flush(&mut paragraph, &mut out);
            let language = trim_ws(&trimmed[fence.len()..]).to_string();
            let mark = fence.chars().next().unwrap_or('`');
            let mut code: Vec<&str> = Vec::new();
            i += 1;
            while i < lines.len() {
                let closing = trim_ws(lines[i]);
                if closing.starts_with(fence.as_str())
                    && closing[fence.len()..]
                        .chars()
                        .all(|c| c == mark || c.is_whitespace())
                {
                    i += 1;
                    break;
                }
                code.push(lines[i]);
                i += 1;
            }
            out.push(Block::Code {
                text: code.join("\n"),
                language,
            });
            continue;
        }
        if i + 1 < lines.len() && line.contains('|') {
            if let Some(align) = table_alignment(lines[i + 1]) {
                let header = cells(line);
                if align.len() == header.len() {
                    flush(&mut paragraph, &mut out);
                    let columns = header.len().min(MAX_COLUMNS);
                    let mut rows: Vec<Row> = Vec::new();
                    i += 2;
                    while i < lines.len() && lines[i].contains('|') && !trim_ws(lines[i]).is_empty()
                    {
                        if rows.len() < MAX_ROWS {
                            let mut c = cells(lines[i]);
                            c.resize(header.len().max(c.len()), String::new());
                            rows.push(Row {
                                cells: c
                                    .iter()
                                    .take(columns)
                                    .map(|t| Cell { spans: inline(t) })
                                    .collect(),
                            });
                        }
                        i += 1;
                    }
                    out.push(Block::Table {
                        header: header
                            .iter()
                            .take(columns)
                            .map(|t| Cell { spans: inline(t) })
                            .collect(),
                        rows,
                        align: align
                            .into_iter()
                            .take(columns)
                            .map(str::to_string)
                            .collect(),
                    });
                    continue;
                }
            }
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes)
            && trimmed[hashes..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
        {
            flush(&mut paragraph, &mut out);
            out.push(Block::Heading {
                level: hashes as u32,
                spans: inline(trim_ws(&trimmed[hashes..])),
            });
        } else if let Some((indent, marker, text)) = list_item(line) {
            flush(&mut paragraph, &mut out);
            out.push(Block::ListItem {
                marker,
                level: ((indent / 2) as u32).min(MAX_LEVEL),
                spans: inline(text),
            });
        } else if let Some(rest) = trimmed.strip_prefix('>') {
            flush(&mut paragraph, &mut out);
            out.push(Block::Quote {
                spans: inline(trim_ws(rest)),
            });
        } else if ["---", "***", "___"].contains(&trimmed) {
            flush(&mut paragraph, &mut out);
            out.push(Block::Divider);
        } else {
            paragraph.push(line);
        }
        i += 1;
    }
    flush(&mut paragraph, &mut out);
    if cut {
        out.push(Block::Paragraph {
            spans: vec![Span::plain("…")],
        });
    }
    out
}

/// The text's words without their look: what Copy gives (blocks one per line, a blank line between them as FERB.app's
/// selection copies them, list markers and table cells kept).
pub fn plain_text(blocks: &[Block]) -> String {
    let words = |s: &[Span]| s.iter().map(|x| x.text.as_str()).collect::<String>();
    blocks
        .iter()
        .map(|b| match b {
            Block::Paragraph { spans } | Block::Heading { spans, .. } | Block::Quote { spans } => {
                words(spans)
            }
            Block::ListItem {
                marker,
                level,
                spans,
            } => {
                format!(
                    "{}{}\t{}",
                    "  ".repeat(*level as usize),
                    marker,
                    words(spans)
                )
            }
            Block::Code { text, .. } => text.clone(),
            Block::Table { header, rows, .. } => std::iter::once(header)
                .chain(rows.iter().map(|r| &r.cells))
                .map(|cells| {
                    cells
                        .iter()
                        .map(|c| words(&c.spans))
                        .collect::<Vec<_>>()
                        .join("\t")
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Block::Divider => "────────".to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn capped(source: &str) -> (&str, bool) {
    match source.char_indices().nth(MAX_CHARS) {
        Some((at, _)) => (&source[..at], true),
        None => (source, false),
    }
}

/// Swift's `trimmingCharacters(in: .whitespaces)`: spaces and tabs (and other horizontal spaces), not line breaks.
fn trim_ws(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() && c != '\n' && c != '\r')
}

fn fence(line: &str) -> Option<String> {
    let first = line.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let count = line.chars().take_while(|c| *c == first).count();
    (count >= 3).then(|| first.to_string().repeat(count))
}

/// `^(\s*)([-+*]|[0-9]+[.)])\s+(.+)$`: the indent, the marker as drawn, the text.
fn list_item(line: &str) -> Option<(usize, String, &str)> {
    let indent_bytes = line.len() - line.trim_start().len();
    let indent = line[..indent_bytes].chars().count();
    let rest = &line[indent_bytes..];
    let (marker, after) =
        if let Some(c) = rest.chars().next().filter(|c| matches!(c, '-' | '+' | '*')) {
            ("•".to_string(), &rest[c.len_utf8()..])
        } else {
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            let close = rest[digits..]
                .chars()
                .next()
                .filter(|c| *c == '.' || *c == ')')?;
            (
                rest[..digits + 1].to_string(),
                &rest[digits + close.len_utf8()..],
            )
        };
    let spaces = after.len() - after.trim_start().len();
    if spaces == 0 {
        return None;
    }
    let text = &after[spaces..];
    // `(.+)$`: at least one character; the regex's `\s+` is greedy but gives back one so `.+` matches
    let text = if text.is_empty() {
        let last = after[..spaces].chars().last()?;
        if last == '\n' {
            return None;
        }
        &after[spaces - last.len_utf8()..]
    } else {
        text
    };
    Some((indent, marker, text))
}

fn table_alignment(line: &str) -> Option<Vec<&'static str>> {
    let c = cells(line);
    if c.is_empty() {
        return None;
    }
    let ok = c.iter().all(|cell| {
        let stripped = cell.trim_matches(':');
        stripped.chars().count() >= 3 && stripped.chars().all(|x| x == '-')
    });
    if !ok {
        return None;
    }
    Some(
        c.iter()
            .map(|cell| {
                if cell.ends_with(':') {
                    if cell.starts_with(':') {
                        "center"
                    } else {
                        "trailing"
                    }
                } else {
                    "leading"
                }
            })
            .collect(),
    )
}

/// `MarkdownDocument.cells`: split on `|` outside code, escapes kept, the outer empty cells dropped.
fn cells(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut values: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut fence = 0usize;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            cell.push(c);
            cell.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c == '`' {
            let mut count = 1;
            while i + count < chars.len() && chars[i + count] == '`' {
                count += 1;
            }
            if fence == 0 {
                fence = count;
            } else if fence == count {
                fence = 0;
            }
            cell.push_str(&"`".repeat(count));
            i += count;
            continue;
        }
        if c == '|' && fence == 0 {
            values.push(trim_ws(&cell).to_string());
            cell.clear();
        } else {
            cell.push(c);
        }
        i += 1;
    }
    values.push(trim_ws(&cell).to_string());
    if values.first().is_some_and(String::is_empty) {
        values.remove(0);
    }
    if values.last().is_some_and(String::is_empty) {
        values.pop();
    }
    values
}

// ---- inline --------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Tok {
    Text(String),
    Code(String),
    /// A run of `*`, `_` or `~`: what is left of it as text, and the looks it opens and closes.
    Delim {
        ch: char,
        left: usize,
        opens: Vec<Look>,
        closes: Vec<Look>,
    },
    LinkStart(Option<String>),
    LinkEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Look {
    Bold,
    Italic,
    Strike,
}

struct DelimInfo {
    tok: usize,
    ch: char,
    count: usize,
    orig: usize,
    can_open: bool,
    can_close: bool,
    group: usize,
    active: bool,
}

fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation() || (!c.is_alphanumeric() && !c.is_whitespace() && !c.is_control())
}

/// Inline Markdown in one block's text (`MarkdownDocument.inline`), with the plain addresses made links.
pub fn inline(source: &str) -> Vec<Span> {
    let chars: Vec<char> = source.chars().collect();
    let n = chars.len();
    let mut toks: Vec<Tok> = Vec::new();
    let mut delims: Vec<DelimInfo> = Vec::new();
    // open brackets: (token index, is an image, active)
    let mut brackets: Vec<(usize, bool, bool)> = Vec::new();
    let mut groups = 0usize;
    let mut text = String::new();
    // code spans: a run length known to have no closer from where it was last looked for
    let mut no_closer: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let push_text = |toks: &mut Vec<Tok>, text: &mut String| {
        if !text.is_empty() {
            toks.push(Tok::Text(std::mem::take(text)));
        }
    };
    let mut i = 0;
    while i < n {
        let c = chars[i];
        match c {
            '\\' if i + 1 < n && chars[i + 1].is_ascii_punctuation() => {
                text.push(chars[i + 1]);
                i += 2;
            }
            '`' => {
                let run = chars[i..].iter().take_while(|x| **x == '`').count();
                let mut found = None;
                if !no_closer.contains(&run) {
                    let mut j = i + run;
                    while j < n {
                        if chars[j] == '`' {
                            let r = chars[j..].iter().take_while(|x| **x == '`').count();
                            if r == run {
                                found = Some(j);
                                break;
                            }
                            j += r;
                        } else {
                            j += 1;
                        }
                    }
                    if found.is_none() {
                        no_closer.insert(run);
                    }
                }
                match found {
                    Some(j) => {
                        push_text(&mut toks, &mut text);
                        let mut code: String = chars[i + run..j]
                            .iter()
                            .map(|x| if *x == '\n' { ' ' } else { *x })
                            .collect();
                        if code.len() >= 2
                            && code.starts_with(' ')
                            && code.ends_with(' ')
                            && code.trim() != ""
                        {
                            code = code[1..code.len() - 1].to_string();
                        }
                        toks.push(Tok::Code(code));
                        i = j + run;
                    }
                    None => {
                        text.push_str(&"`".repeat(run));
                        i += run;
                    }
                }
            }
            '*' | '_' | '~' => {
                let run = chars[i..].iter().take_while(|x| **x == c).count();
                push_text(&mut toks, &mut text);
                let before = if i == 0 { ' ' } else { chars[i - 1] };
                let after = chars.get(i + run).copied().unwrap_or(' ');
                let left = !after.is_whitespace()
                    && (!is_punct(after) || before.is_whitespace() || is_punct(before));
                let right = !before.is_whitespace()
                    && (!is_punct(before) || after.is_whitespace() || is_punct(after));
                let (can_open, can_close) = match c {
                    '_' => (
                        left && (!right || is_punct(before)),
                        right && (!left || is_punct(after)),
                    ),
                    _ => (left, right),
                };
                delims.push(DelimInfo {
                    tok: toks.len(),
                    ch: c,
                    count: run,
                    orig: run,
                    can_open,
                    can_close,
                    group: 0,
                    active: true,
                });
                toks.push(Tok::Delim {
                    ch: c,
                    left: run,
                    opens: Vec::new(),
                    closes: Vec::new(),
                });
                i += run;
            }
            '!' if chars.get(i + 1) == Some(&'[') => {
                push_text(&mut toks, &mut text);
                brackets.push((toks.len(), true, true));
                toks.push(Tok::Text("![".into()));
                i += 2;
            }
            '[' => {
                push_text(&mut toks, &mut text);
                brackets.push((toks.len(), false, true));
                toks.push(Tok::Text("[".into()));
                i += 1;
            }
            ']' => {
                let Some((open, image, active)) = brackets.pop() else {
                    text.push(']');
                    i += 1;
                    continue;
                };
                let dest = if active {
                    destination(&chars, i + 1)
                } else {
                    None
                };
                match dest {
                    Some((url, end)) => {
                        push_text(&mut toks, &mut text);
                        if image {
                            // an image is its words only: nothing is fetched (MarkdownDocument: "images never create
                            // network requests")
                            toks[open] = Tok::Text(String::new());
                        } else {
                            groups += 1;
                            let g = groups;
                            toks[open] = Tok::LinkStart(safe(&url));
                            toks.push(Tok::LinkEnd);
                            for d in delims.iter_mut().filter(|d| d.tok > open) {
                                d.group = g;
                            }
                            // no link inside a link: the brackets still open before it can't make one
                            for b in brackets.iter_mut().filter(|b| !b.1) {
                                b.2 = false;
                            }
                        }
                        i = end;
                    }
                    None => {
                        text.push(']');
                        i += 1;
                    }
                }
            }
            '<' => match autolink(&chars, i) {
                Some((url, end)) => {
                    push_text(&mut toks, &mut text);
                    let link = safe(&url);
                    toks.push(Tok::LinkStart(link));
                    toks.push(Tok::Text(url));
                    toks.push(Tok::LinkEnd);
                    i = end;
                }
                None => {
                    text.push('<');
                    i += 1;
                }
            },
            _ => {
                text.push(c);
                i += 1;
            }
        }
    }
    push_text(&mut toks, &mut text);
    emphasis(&mut toks, &mut delims);
    detect(flatten(toks))
}

/// `(address "title")` after a `]`: the address and where the link ends.
fn destination(chars: &[char], at: usize) -> Option<(String, usize)> {
    if chars.get(at) != Some(&'(') {
        return None;
    }
    let mut i = at + 1;
    let limit = (at + MAX_ADDRESS).min(chars.len());
    while i < limit && chars[i] == ' ' {
        i += 1;
    }
    let mut url = String::new();
    if chars.get(i) == Some(&'<') {
        i += 1;
        while i < limit && chars[i] != '>' && chars[i] != '\n' {
            url.push(chars[i]);
            i += 1;
        }
        if chars.get(i) != Some(&'>') {
            return None;
        }
        i += 1;
    } else {
        let mut depth = 0i32;
        while i < limit {
            let c = chars[i];
            if c.is_whitespace() || c.is_control() {
                break;
            }
            if c == '\\' && i + 1 < limit && chars[i + 1].is_ascii_punctuation() {
                url.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '(' {
                depth += 1;
            }
            if c == ')' {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            url.push(c);
            i += 1;
        }
    }
    while i < limit && chars[i].is_whitespace() {
        i += 1;
    }
    // an optional title, left out
    if let Some(&q) = chars.get(i).filter(|q| matches!(q, '"' | '\'' | '(')) {
        let close = if q == '(' { ')' } else { q };
        i += 1;
        while i < limit && chars[i] != close {
            i += 1;
        }
        if i >= limit {
            return None;
        }
        i += 1;
        while i < limit && chars[i].is_whitespace() {
            i += 1;
        }
    }
    (chars.get(i) == Some(&')')).then_some((url, i + 1))
}

/// `<scheme:…>` or `<name@host>`.
fn autolink(chars: &[char], at: usize) -> Option<(String, usize)> {
    let limit = (at + MAX_ADDRESS).min(chars.len());
    let mut i = at + 1;
    let mut s = String::new();
    while i < limit && chars[i] != '>' {
        let c = chars[i];
        if c.is_whitespace() || c == '<' {
            return None;
        }
        s.push(c);
        i += 1;
    }
    if chars.get(i) != Some(&'>') {
        return None;
    }
    let scheme = s.split(':').next().unwrap_or("");
    let is_uri = s.contains(':')
        && scheme.len() >= 2
        && scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'));
    let is_mail =
        !s.contains(':') && s.contains('@') && s.split('@').nth(1).is_some_and(|h| h.contains('.'));
    // the address as written; `safe` makes a mail address a mailto link
    (is_uri || is_mail).then_some((s, i + 1))
}

/// The address made a link only when it is http, https or mailto (APP-589).
fn safe(url: &str) -> Option<String> {
    let u = url.trim();
    let scheme = u.split(':').next()?.to_ascii_lowercase();
    if u.contains('@') && !u.contains(':') {
        return Some(format!("mailto:{u}"));
    }
    match scheme.as_str() {
        "http" | "https" => {
            let rest = &u[scheme.len() + 1..];
            rest.starts_with("//")
                .then(|| u.to_string())
                .filter(|_| rest.len() > 2)
        }
        "mailto" => (u.len() > 7).then(|| u.to_string()),
        _ => None,
    }
}

/// CommonMark's "process emphasis", each closer looking back at most `LOOK_BACK` delimiters, within its own link.
fn emphasis(toks: &mut [Tok], delims: &mut [DelimInfo]) {
    let mut c = 0;
    while c < delims.len() {
        if !(delims[c].can_close && delims[c].active && delims[c].count > 0) {
            c += 1;
            continue;
        }
        let mut found = None;
        let mut looked = 0;
        let mut o = c;
        while o > 0 && looked < LOOK_BACK {
            o -= 1;
            looked += 1;
            let (a, b) = (&delims[o], &delims[c]);
            if !a.active || a.count == 0 || !a.can_open || a.ch != b.ch || a.group != b.group {
                continue;
            }
            if a.ch == '~' && (a.count < 2 || b.count < 2) {
                continue;
            }
            if (a.can_close || b.can_open)
                && (a.orig + b.orig) % 3 == 0
                && !(a.orig % 3 == 0 && b.orig % 3 == 0)
            {
                continue;
            }
            found = Some(o);
            break;
        }
        let Some(o) = found else {
            if !delims[c].can_open {
                delims[c].active = false;
            }
            c += 1;
            continue;
        };
        // `~~` always takes two (its single mark is text); `*` and `_` take two when both runs have them
        let use_ = if delims[c].ch == '~' || (delims[o].count >= 2 && delims[c].count >= 2) {
            2
        } else {
            1
        };
        let look = match (delims[c].ch, use_) {
            ('~', _) => Look::Strike,
            (_, 2) => Look::Bold,
            _ => Look::Italic,
        };
        delims[o].count -= use_;
        delims[c].count -= use_;
        if let Tok::Delim { left, opens, .. } = &mut toks[delims[o].tok] {
            *left -= use_;
            opens.insert(0, look);
        }
        if let Tok::Delim { left, closes, .. } = &mut toks[delims[c].tok] {
            *left -= use_;
            closes.push(look);
        }
        for d in &mut delims[o + 1..c] {
            d.active = false;
        }
        if delims[c].count == 0 {
            c += 1;
        }
    }
}

fn flatten(toks: Vec<Tok>) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let (mut bold, mut italic, mut strike) = (0i32, 0i32, 0i32);
    let mut link: Option<String> = None;
    let push = |out: &mut Vec<Span>, s: Span| {
        if s.text.is_empty() {
            return;
        }
        match out.last_mut() {
            Some(last) if last.same_look(&s) => last.text.push_str(&s.text),
            _ => out.push(s),
        }
    };
    let look =
        |t: String, code: bool, bold: i32, italic: i32, strike: i32, link: &Option<String>| Span {
            text: t,
            bold: bold > 0,
            italic: italic > 0,
            code,
            strike: strike > 0,
            link: link.clone(),
        };
    let step = |l: Look, by: i32, bold: &mut i32, italic: &mut i32, strike: &mut i32| match l {
        Look::Bold => *bold += by,
        Look::Italic => *italic += by,
        Look::Strike => *strike += by,
    };
    for t in toks {
        match t {
            Tok::Text(s) => push(&mut out, look(s, false, bold, italic, strike, &link)),
            Tok::Code(s) => push(&mut out, look(s, true, bold, italic, strike, &link)),
            Tok::Delim {
                ch,
                left,
                opens,
                closes,
            } => {
                for l in closes {
                    step(l, -1, &mut bold, &mut italic, &mut strike);
                }
                push(
                    &mut out,
                    look(
                        ch.to_string().repeat(left),
                        false,
                        bold,
                        italic,
                        strike,
                        &link,
                    ),
                );
                for l in opens {
                    step(l, 1, &mut bold, &mut italic, &mut strike);
                }
            }
            Tok::LinkStart(u) => link = u,
            Tok::LinkEnd => link = None,
        }
    }
    out
}

/// The plain addresses in text that is no link and no code (FERB.app's data detector): http and https addresses,
/// `www.` names (opened over https) and mail addresses.
fn detect(spans: Vec<Span>) -> Vec<Span> {
    let mut out = Vec::with_capacity(spans.len());
    for s in spans {
        if s.link.is_some()
            || s.code
            || !(s.text.contains("://") || s.text.contains("www.") || s.text.contains('@'))
        {
            out.push(s);
            continue;
        }
        let chars: Vec<char> = s.text.chars().collect();
        let mut i = 0;
        let mut plain = String::new();
        while i < chars.len() {
            let start_ok = i == 0
                || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '.' || chars[i - 1] == '/');
            let at = |p: &str| {
                chars[i..]
                    .iter()
                    .take(p.len())
                    .collect::<String>()
                    .eq_ignore_ascii_case(p)
            };
            let found = if start_ok && (at("https://") || at("http://") || at("www.")) {
                address_end(&chars, i).map(|end| {
                    let t: String = chars[i..end].iter().collect();
                    let url = if t.to_ascii_lowercase().starts_with("www.") {
                        format!("https://{t}")
                    } else {
                        t.clone()
                    };
                    (end, t, url)
                })
            } else if chars[i] == '@' {
                // the name before the `@` is already in `plain`: it moves into the link
                mail_at(&chars, i, plain.chars().count())
                    .map(|(back, end)| {
                        let t: String = chars[i - back..end].iter().collect();
                        (end, t.clone(), format!("mailto:{t}"), back)
                    })
                    .map(|(end, t, u, back)| {
                        let keep = plain.chars().count() - back;
                        plain = plain.chars().take(keep).collect();
                        (end, t, u)
                    })
            } else {
                None
            };
            match found {
                Some((end, t, url)) if safe(&url).is_some() => {
                    if !plain.is_empty() {
                        out.push(Span {
                            text: std::mem::take(&mut plain),
                            link: None,
                            ..s.clone()
                        });
                    }
                    out.push(Span {
                        text: t,
                        link: safe(&url),
                        ..s.clone()
                    });
                    i = end;
                }
                _ => {
                    plain.push(chars[i]);
                    i += 1;
                }
            }
        }
        if !plain.is_empty() {
            out.push(Span {
                text: plain,
                link: None,
                ..s
            });
        }
    }
    out
}

/// Where an address that starts at `i` ends: at white space or `<`, without its closing punctuation and without a
/// `)` it doesn't open.
fn address_end(chars: &[char], i: usize) -> Option<usize> {
    let limit = (i + MAX_ADDRESS).min(chars.len());
    let mut end = i;
    while end < limit && !chars[end].is_whitespace() && !matches!(chars[end], '<' | '>' | '"' | '`')
    {
        end += 1;
    }
    loop {
        let &last = chars[i..end].last()?;
        if matches!(last, '.' | ',' | ':' | ';' | '!' | '?' | '\'' | '*' | '_') {
            end -= 1;
        } else if last == ')' {
            let opens = chars[i..end].iter().filter(|c| **c == '(').count();
            let closes = chars[i..end].iter().filter(|c| **c == ')').count();
            if closes > opens {
                end -= 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    let body: String = chars[i..end].iter().collect();
    let host = body.split("://").last().unwrap_or("");
    (host.contains('.') && host.len() > 3).then_some(end)
}

/// A mail address around the `@` at `at`: how many characters before it belong to it (at most `before`, the plain text
/// gathered so far) and where it ends.
fn mail_at(chars: &[char], at: usize, before: usize) -> Option<(usize, usize)> {
    let local = |c: char| c.is_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-');
    let mut back = 0;
    while back < before.min(64) && at > back && local(chars[at - back - 1]) {
        back += 1;
    }
    if back == 0 {
        return None;
    }
    let mut end = at + 1;
    while end < chars.len()
        && end - at < 256
        && (chars[end].is_alphanumeric() || matches!(chars[end], '.' | '-'))
    {
        end += 1;
    }
    while end > at + 1 && matches!(chars[end - 1], '.' | '-') {
        end -= 1;
    }
    let host: String = chars[at + 1..end].iter().collect();
    let tld = host.rsplit('.').next().unwrap_or("");
    (host.contains('.') && tld.chars().count() >= 2 && tld.chars().all(char::is_alphabetic))
        .then_some((back, end))
}
