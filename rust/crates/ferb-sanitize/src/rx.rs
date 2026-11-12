//! Python `re` patterns run by `fancy-regex`. The pattern text is Python's own (`generated.rs`), translated here
//! so that each construct means what it means in Python 3.12 on a `str`:
//!
//! - `\s` is `str.isspace()` (Rust's `\s` lacks U+001C..U+001F), `\d` is `str.isdecimal()` and `\b` is a boundary
//!   of Python's `\w` (`str.isalnum()` or "_"), each written out from Python's tables so the Rust regex crate's
//!   newer Unicode data changes nothing;
//! - `$` without re.M also matches before a final "\n": written `(?=\n?\z)`;
//! - re.I: Python matches U+0130 and U+0131 as "i" (their simple case mappings); Rust's simple case folding leaves
//!   them out, so every case-insensitive "i" also takes those two. "s"/U+017F and "k"/U+212A fold alike in both;
//! - re.X is applied here (Rust's verbose mode would also drop the spaces inside a class, Python keeps them);
//! - a class's "[", "&" and "~" are escaped (Rust reads nested classes and set operators there).
//!
//! Lookaround, backreferences (`\1`, `(?P=q)`) and possessive quantifiers (`*+`, `++`, `{n,}+`) are fancy-regex's
//! own; each pattern is compiled and compared with Python's results by the fixtures (ENG-3).
//! `match` is `\G` + the pattern, `fullmatch` is `\G(?:…)\z` on the text cut at `endpos`.
//!
//! No pattern can fail at run time: the backtrack limit is off (Python has none). Should fancy-regex still report
//! an error, the process stops rather than let text through unread (the message names the pattern, never the text).

use std::sync::OnceLock;

use fancy_regex::{Regex, RegexBuilder};

use crate::generated::{self as g, Pat};

pub const RE_I: u32 = 2;
pub const RE_M: u32 = 8;
pub const RE_S: u32 = 16;
pub const RE_X: u32 = 64;

/// A match: byte offsets.
pub type Span = (usize, usize);
/// A match with its groups (group 0 first); a group that took no part is None.
pub type Caps = Vec<Option<Span>>;

fn class_ranges(table: &[(u32, u32)], extra: &str) -> String {
    let mut out = String::from(extra);
    for &(a, b) in table {
        if a == b {
            out.push_str(&format!("\\x{{{a:x}}}"));
        } else {
            out.push_str(&format!("\\x{{{a:x}}}-\\x{{{b:x}}}"));
        }
    }
    out
}

fn space_ranges() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    S.get_or_init(|| class_ranges(g::SPACE, ""))
}

fn decimal_ranges() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    S.get_or_init(|| class_ranges(g::DECIMAL, ""))
}

fn word_ranges() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    S.get_or_init(|| class_ranges(g::ALNUM, "_"))
}

const DOTTED_I: &str = "\\x{130}\\x{131}";

/// One item of a class: what to write, and the character it is when it is a single one.
struct Item {
    text: String,
    cp: Option<u32>,
    dash: bool,
}

fn class_escape(c: char) -> String {
    match c {
        '[' | ']' | '&' | '~' | '\\' | '^' | '-' => format!("\\{c}"),
        _ => c.to_string(),
    }
}

/// Translate a class starting at `chars[i]` == '['; returns the index after its "]".
fn class(chars: &[char], mut i: usize, icase: bool, out: &mut String) -> usize {
    i += 1;
    let negated = chars.get(i) == Some(&'^');
    if negated {
        i += 1;
    }
    let mut items: Vec<Item> = Vec::new();
    loop {
        let c = chars[i];
        if c == ']' {
            i += 1;
            break;
        }
        if c == '\\' {
            let n = chars[i + 1];
            i += 2;
            match n {
                's' => items.push(Item {
                    text: space_ranges().to_string(),
                    cp: None,
                    dash: false,
                }),
                'd' => items.push(Item {
                    text: decimal_ranges().to_string(),
                    cp: None,
                    dash: false,
                }),
                'x' => {
                    let hex: String = chars[i..i + 2].iter().collect();
                    i += 2;
                    let cp = u32::from_str_radix(&hex, 16).expect("a \\xHH escape");
                    items.push(Item {
                        text: format!("\\x{{{cp:x}}}"),
                        cp: Some(cp),
                        dash: false,
                    });
                }
                't' | 'r' | 'n' => {
                    let cp = match n {
                        't' => 9,
                        'r' => 13,
                        _ => 10,
                    };
                    items.push(Item {
                        text: format!("\\{n}"),
                        cp: Some(cp),
                        dash: false,
                    });
                }
                _ if n.is_ascii_alphanumeric() => {
                    panic!("ferb-sanitize: class escape \\{n} is not translated")
                }
                _ => items.push(Item {
                    text: class_escape(n),
                    cp: Some(n as u32),
                    dash: false,
                }),
            }
        } else if c == '-' {
            items.push(Item {
                text: "-".to_string(),
                cp: Some('-' as u32),
                dash: true,
            });
            i += 1;
        } else {
            items.push(Item {
                text: class_escape(c),
                cp: Some(c as u32),
                dash: false,
            });
            i += 1;
        }
    }
    // Ranges: a "-" between two single characters. Anything else leaves the "-" a literal.
    let mut has_i = false;
    let mut text = String::new();
    let mut k = 0;
    while k < items.len() {
        let range = k + 2 < items.len()
            && items[k + 1].dash
            && items[k].cp.is_some()
            && items[k + 2].cp.is_some();
        if range {
            let (a, b) = (items[k].cp.unwrap_or(0), items[k + 2].cp.unwrap_or(0));
            has_i |= (a..=b).contains(&('i' as u32)) || (a..=b).contains(&('I' as u32));
            let first = if items[k].dash { "\\-" } else { &items[k].text };
            let last = if items[k + 2].dash {
                "\\-"
            } else {
                &items[k + 2].text
            };
            text.push_str(first);
            text.push('-');
            text.push_str(last);
            k += 3;
        } else {
            has_i |= matches!(items[k].cp, Some(0x69) | Some(0x49));
            if items[k].dash {
                text.push_str("\\-");
            } else {
                text.push_str(&items[k].text);
            }
            k += 1;
        }
    }
    out.push('[');
    if negated {
        out.push('^');
    }
    out.push_str(&text);
    if icase && has_i {
        out.push_str(DOTTED_I);
    }
    out.push(']');
    i
}

/// Python pattern text → fancy-regex pattern text (see the module's note).
pub fn translate(src: &str, flags: u32) -> String {
    let chars: Vec<char> = src.chars().collect();
    let multi = flags & RE_M != 0;
    let verbose = flags & RE_X != 0;
    let mut icase = flags & RE_I != 0;
    let mut out = String::from("(?");
    if icase {
        out.push('i');
    }
    if multi {
        out.push('m');
    }
    if flags & RE_S != 0 {
        out.push('s');
    }
    if out.len() == 2 {
        out.clear();
    } else {
        out.push(')');
    }
    let mut scopes: Vec<bool> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                let n = chars[i + 1];
                i += 2;
                match n {
                    's' => out.push_str(&format!("[{}]", space_ranges())),
                    'S' => out.push_str(&format!("[^{}]", space_ranges())),
                    'd' => out.push_str(&format!("[{}]", decimal_ranges())),
                    'b' => {
                        let w = word_ranges();
                        out.push_str(&format!(
                            "(?-i:(?:(?<=[{w}])(?![{w}])|(?<![{w}])(?=[{w}])))"
                        ));
                    }
                    'w' | 'W' | 'D' | 'B' | 'A' | 'Z' => {
                        panic!("ferb-sanitize: escape \\{n} is not translated")
                    }
                    _ => {
                        out.push('\\');
                        out.push(n);
                    }
                }
            }
            '[' => i = class(&chars, i, icase, &mut out),
            '(' => {
                if chars.get(i + 1) != Some(&'?') {
                    scopes.push(icase);
                    out.push('(');
                    i += 1;
                    continue;
                }
                let rest: String = chars[i + 2..chars.len().min(i + 12)].iter().collect();
                if rest.starts_with("P=") {
                    let close = i + chars[i..]
                        .iter()
                        .position(|&x| x == ')')
                        .expect("a closed backreference");
                    out.extend(&chars[i..=close]);
                    i = close + 1;
                } else if rest.starts_with("P<") {
                    let close = i + chars[i..]
                        .iter()
                        .position(|&x| x == '>')
                        .expect("a closed group name");
                    out.extend(&chars[i..=close]);
                    scopes.push(icase);
                    i = close + 1;
                } else if let Some(len) = ["<=", "<!", ":", "=", "!", ">"]
                    .iter()
                    .find(|p| rest.starts_with(**p))
                    .map(|p| p.len())
                {
                    out.extend(&chars[i..i + 2 + len]);
                    scopes.push(icase);
                    i += 2 + len;
                } else {
                    // flags: "(?i)", "(?ix)" for the whole pattern (already in `flags`), or "(?i:" for a group
                    let mut j = i + 2;
                    let mut set_i = false;
                    while matches!(chars[j], 'i' | 'x' | 's' | 'm') {
                        set_i |= chars[j] == 'i';
                        assert!(
                            chars[j] == 'i' || chars[j] == 'x',
                            "ferb-sanitize: inline flag not translated"
                        );
                        j += 1;
                    }
                    match chars[j] {
                        ')' => {}
                        ':' => {
                            assert!(
                                set_i && j == i + 3,
                                "ferb-sanitize: scoped flags other than (?i: not translated"
                            );
                            out.push_str("(?i:");
                            scopes.push(icase);
                            icase = true;
                        }
                        other => panic!("ferb-sanitize: group (?{other} is not translated"),
                    }
                    i = j + 1;
                }
            }
            ')' => {
                icase = scopes.pop().expect("balanced groups");
                out.push(')');
                i += 1;
            }
            '{' => {
                let close = i + chars[i..]
                    .iter()
                    .position(|&x| x == '}')
                    .expect("a closed quantifier");
                out.extend(&chars[i..=close]);
                i = close + 1;
            }
            '$' => {
                out.push_str(if multi { "$" } else { "(?=\\n?\\z)" });
                i += 1;
            }
            'i' | 'I' if icase => {
                out.push_str(&format!("[iI{DOTTED_I}]"));
                i += 1;
            }
            _ if verbose && c.is_whitespace() => i += 1,
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

fn build(name: &str, pattern: &str) -> Regex {
    RegexBuilder::new(pattern)
        .backtrack_limit(usize::MAX)
        .delegate_size_limit(1 << 28)
        .delegate_dfa_size_limit(1 << 26)
        .build()
        .unwrap_or_else(|e| panic!("ferb-sanitize: pattern {name} does not compile: {e}"))
}

/// A Python pattern, compiled when first used.
pub struct Rx {
    pat: &'static Pat,
    search: OnceLock<Regex>,
    anchored: OnceLock<Regex>,
    full: OnceLock<Regex>,
}

fn fail(name: &str) -> ! {
    // Never the text: it may hold a secret.
    panic!("ferb-sanitize: pattern {name} could not be run");
}

impl Rx {
    pub const fn new(pat: &'static Pat) -> Rx {
        Rx {
            pat,
            search: OnceLock::new(),
            anchored: OnceLock::new(),
            full: OnceLock::new(),
        }
    }

    fn body(&self) -> String {
        translate(self.pat.src, self.pat.flags)
    }

    fn search_re(&self) -> &Regex {
        self.search
            .get_or_init(|| build(self.pat.name, &self.body()))
    }

    fn anchored_re(&self) -> &Regex {
        self.anchored
            .get_or_init(|| build(self.pat.name, &format!("\\G(?:(?:{})()|)", self.body())))
    }

    fn full_re(&self) -> &Regex {
        self.full
            .get_or_init(|| build(self.pat.name, &format!("\\G(?:(?:{})\\z()|)", self.body())))
    }

    /// Compile all three forms (the tests' check that every pattern translates).
    pub fn compile(&self) {
        self.search_re();
        self.anchored_re();
        self.full_re();
    }

    fn caps_with(&self, re: &Regex, s: &str, pos: usize) -> Option<Caps> {
        match re.captures_from_pos(s, pos) {
            Ok(Some(c)) => Some(
                (0..c.len())
                    .map(|i| c.get(i).map(|m| (m.start(), m.end())))
                    .collect(),
            ),
            Ok(None) => None,
            Err(_) => fail(self.pat.name),
        }
    }

    /// A `\G` form run at `pos`. Both forms end in an empty marker group and an empty alternative: when the pattern
    /// doesn't match at `pos` the empty alternative does, at once (fancy-regex would otherwise try every later
    /// start, each failing on `\G`: a cost in the text's length per call). The marker tells the two apart.
    fn anchored(&self, re: &Regex, s: &str, pos: usize) -> Option<Caps> {
        let mut caps = self.caps_with(re, s, pos)?;
        caps.pop()??;
        Some(caps)
    }

    fn span_with(&self, re: &Regex, s: &str, pos: usize) -> Option<Span> {
        match re.find_from_pos(s, pos) {
            Ok(m) => m.map(|m| (m.start(), m.end())),
            Err(_) => fail(self.pat.name),
        }
    }

    /// `pattern.search(s, pos)`.
    pub fn search(&self, s: &str, pos: usize) -> Option<Span> {
        self.span_with(self.search_re(), s, pos)
    }

    /// `pattern.search(s, pos, end)`: the text ends at `end`.
    pub fn search_in(&self, s: &str, pos: usize, end: usize) -> Option<Span> {
        if pos > end {
            return None;
        }
        self.span_with(self.search_re(), &s[..end], pos)
    }

    pub fn search_caps(&self, s: &str, pos: usize) -> Option<Caps> {
        self.caps_with(self.search_re(), s, pos)
    }

    /// `pattern.match(s, pos)`.
    pub fn match_at(&self, s: &str, pos: usize) -> Option<Span> {
        self.anchored(self.anchored_re(), s, pos).and_then(|c| c[0])
    }

    /// `pattern.match(s, pos, end)`.
    pub fn match_in(&self, s: &str, pos: usize, end: usize) -> Option<Span> {
        if pos > end {
            return None;
        }
        self.anchored(self.anchored_re(), &s[..end], pos)
            .and_then(|c| c[0])
    }

    pub fn match_caps(&self, s: &str, pos: usize) -> Option<Caps> {
        self.anchored(self.anchored_re(), s, pos)
    }

    /// `pattern.fullmatch(s)`.
    pub fn fullmatch(&self, s: &str) -> bool {
        self.anchored(self.full_re(), s, 0).is_some()
    }

    /// `pattern.fullmatch(s, pos, end)`.
    pub fn fullmatch_in(&self, s: &str, pos: usize, end: usize) -> bool {
        pos <= end && self.anchored(self.full_re(), &s[..end], pos).is_some()
    }

    /// `pattern.finditer(s, pos, end)` as spans.
    pub fn find_all_in(&self, s: &str, pos: usize, end: usize) -> Vec<Span> {
        let (s, re) = (&s[..end], self.search_re());
        let (mut out, mut pos) = (Vec::new(), pos);
        while pos <= s.len() {
            let Some((a, b)) = self.span_with(re, s, pos) else {
                break;
            };
            out.push((a, b));
            pos = if b > a { b } else { next_char(s, b) };
        }
        out
    }

    /// `pattern.finditer(s)` as spans.
    pub fn find_all(&self, s: &str) -> Vec<Span> {
        self.find_all_in(s, 0, s.len())
    }

    /// `pattern.finditer(s)` with groups.
    pub fn caps_all(&self, s: &str) -> Vec<Caps> {
        let re = self.search_re();
        let (mut out, mut pos) = (Vec::new(), 0);
        while pos <= s.len() {
            let Some(c) = self.caps_with(re, s, pos) else {
                break;
            };
            let (a, b) = c[0].unwrap_or((pos, pos));
            out.push(c);
            pos = if b > a { b } else { next_char(s, b) };
        }
        out
    }
}

/// The offset after the character at `pos` (one past the end when there is none).
pub fn next_char(s: &str, pos: usize) -> usize {
    pos + s[pos..].chars().next().map_or(1, char::len_utf8)
}

impl std::fmt::Debug for Rx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rx({})", self.pat.name)
    }
}

/// Every pattern of the Python build, compiled: a test's check, and a daemon's at start if it wants the cost up
/// front.
pub fn compile_all() {
    for pat in g::ALL_PATTERNS {
        Rx::new(pat).compile();
    }
    for f in g::FORMAT_TABLE {
        Rx::new(&f.pat).compile();
    }
}
