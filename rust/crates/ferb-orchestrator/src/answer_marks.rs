//! ferb/orchestrator/answer_marks.py: the one structural change code makes to a worker's answer after a check
//! (ORCH-545): removing a quoted sentence or marking it "(I couldn't confirm this)", whether a fix changed only the
//! quoted sentences (ORCH-547), and stray closing tags (ORCH-604). Structure only, never meaning (rule 4).
//!
//! decision: ORCH-545, ORCH-546, ORCH-547, ORCH-604

use std::sync::OnceLock;

use regex::Regex;

use crate::py::{is_alnum, is_space, lstrip, rstrip, rstrip_chars, splitlines, strip};

pub const UNCONFIRMED_MARK: &str = "(I couldn't confirm this)";
pub const NOT_FINISHED: &str = "I couldn't finish this task.";
pub const STOPPED: &str = "Stopped, as you asked.";
pub const WHERE_STOOD: &str = "This is where things stood:";
pub const DECLINED_LINE: &str = "Not done, as you asked.";

/// answer_marks.py:21 `headline`.
pub fn headline(text: &str) -> String {
    let lines: Vec<&str> = splitlines(text)
        .into_iter()
        .map(strip)
        .filter(|l| !l.is_empty())
        .collect();
    let stood = format!("{STOPPED} {WHERE_STOOD}");
    if lines.len() > 1 && (lines[0] == NOT_FINISHED || lines[0] == STOPPED || lines[0] == stood) {
        return format!("{} {}", lines[0], lines[1]);
    }
    lines.first().map(|l| l.to_string()).unwrap_or_default()
}

/// answer_marks.py:31 `Settled`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Settled {
    pub answer: String,
    pub removed: Vec<String>,
    pub marked: Vec<String>,
    pub missing: Vec<String>,
    pub kept_contradicted: Vec<String>,
}

/// answer_marks.py:41 `removes_or_marks`.
pub fn removes_or_marks(find: &str, replace: &str) -> bool {
    if replace.is_empty() {
        return true;
    }
    let plain = replace.replace('\u{2019}', "'");
    let Some(at) = plain.find(UNCONFIRMED_MARK) else {
        return false;
    };
    let rest = format!("{}{}", &plain[..at], &plain[at + UNCONFIRMED_MARK.len()..]);
    let target = find.replace('\u{2019}', "'");
    if rest == target {
        return true;
    }
    // `at > 0 and rest[:at - 1] + rest[at:] == target and plain[at - 1] == " "`
    at > 0 && plain[..at].ends_with(' ') && format!("{}{}", &rest[..at - 1], &rest[at..]) == target
}

/// answer_marks.py:56 `mark_unconfirmed`.
pub fn mark_unconfirmed(answer: &str, find: &str) -> String {
    let Some(at) = answer.find(find) else {
        return answer.to_string();
    };
    let end = at + find.len();
    let body = rstrip(find);
    let stripped = rstrip_chars(body, ".!?;:,");
    let core = rstrip(stripped);
    let stop = &body[stripped.len()..];
    let after = lstrip(&answer[at + core.len()..]);
    if after.replace('\u{2019}', "'").starts_with(UNCONFIRMED_MARK) {
        return answer.to_string();
    }
    format!(
        "{}{} {}{}{}{}",
        &answer[..at],
        core,
        UNCONFIRMED_MARK,
        stop,
        &find[body.len()..],
        &answer[end..]
    )
}

/// answer_marks.py:73 `remove_quote`.
pub fn remove_quote(answer: &str, find: &str) -> String {
    let Some(at) = answer.find(find) else {
        return answer.to_string();
    };
    let mut before = &answer[..at];
    let after = &answer[at + find.len()..];
    if before.ends_with(' ')
        && (after.is_empty() || " \n.,;:!?".contains(after.chars().next().unwrap_or('x')))
    {
        before = &before[..before.len() - 1];
    }
    let mut joined = format!("{before}{after}");
    while joined.contains("\n\n\n") {
        joined = joined.replace("\n\n\n", "\n\n");
    }
    strip(&joined).to_string()
}

/// answer_marks.py:87 `settle`.
pub fn settle(answer: &str, findings: &[(String, String)]) -> Settled {
    let mut done = settled(answer, findings);
    if !done.removed.is_empty() && !has_text(&done.answer) {
        let marked: Vec<(String, String)> = findings
            .iter()
            .map(|(q, _)| (q.clone(), "unsupported".to_string()))
            .collect();
        done = settled(answer, &marked);
        done.kept_contradicted = findings
            .iter()
            .filter(|(q, kind)| kind == "contradicted" && done.marked.contains(q))
            .map(|(q, _)| q.clone())
            .collect();
    }
    done
}

/// answer_marks.py:101 `unmarked`.
pub fn unmarked(answer: &str, findings: &[(String, String)]) -> String {
    let contradicted: Vec<(String, String)> = findings
        .iter()
        .filter(|(_, kind)| kind == "contradicted")
        .cloned()
        .collect();
    let done = settled(answer, &contradicted);
    if has_text(&done.answer) {
        done.answer
    } else {
        String::new()
    }
}

/// answer_marks.py:110 `has_text`.
pub fn has_text(text: &str) -> bool {
    text.chars().any(is_alnum)
}

/// The pieces of `re.split(r"(?<=[.!?])\s+|\n+", text)`.
fn sentence_pieces(text: &str) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut prev: Option<char> = None;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (at, c) = chars[i];
        let after_stop = matches!(prev, Some('.') | Some('!') | Some('?'));
        if after_stop && is_space(c) {
            let mut j = i;
            while j < chars.len() && is_space(chars[j].1) {
                j += 1;
            }
            pieces.push(&text[start..at]);
            start = chars.get(j).map(|x| x.0).unwrap_or(text.len());
            prev = chars.get(j - 1).map(|x| x.1);
            i = j;
            continue;
        }
        if c == '\n' {
            let mut j = i;
            while j < chars.len() && chars[j].1 == '\n' {
                j += 1;
            }
            pieces.push(&text[start..at]);
            start = chars.get(j).map(|x| x.0).unwrap_or(text.len());
            prev = Some('\n');
            i = j;
            continue;
        }
        prev = Some(c);
        i += 1;
    }
    pieces.push(&text[start..]);
    pieces
}

/// answer_marks.py:115 `sentences`.
pub fn sentences(text: &str) -> usize {
    sentence_pieces(text)
        .into_iter()
        .filter(|p| has_text(p))
        .count()
}

fn newlines(text: &str) -> usize {
    strip(text).matches('\n').count()
}

/// answer_marks.py:121 `only_quoted_changed`.
pub fn only_quoted_changed(original: &str, revised: &str, quotes: &[String]) -> bool {
    // [start, end, sentences allowed, lines allowed]
    let mut found: Vec<(usize, usize)> = quotes
        .iter()
        .filter(|q| !strip(q).is_empty())
        .filter_map(|q| original.find(q.as_str()).map(|at| (at, at + q.len())))
        .collect();
    found.sort();
    let mut spans: Vec<[usize; 4]> = Vec::new();
    for (start, end) in found {
        let quote = &original[start..end];
        if let Some(last) = spans.last_mut() {
            let gap = if start > last[1] {
                &original[last[1]..start]
            } else {
                ""
            };
            if start <= last[1] || strip(gap).is_empty() {
                last[1] = last[1].max(end);
                last[2] += sentences(quote);
                last[3] += newlines(quote);
                continue;
            }
        }
        spans.push([start, end, sentences(quote), newlines(quote)]);
    }
    let text = strip(revised);
    if spans.is_empty() {
        return text == strip(original);
    }
    let mut pieces: Vec<&str> = vec![&original[..spans[0][0]]];
    for pair in spans.windows(2) {
        let (a, b) = (pair[0][1], pair[1][0]);
        pieces.push(if b > a { &original[a..b] } else { "" });
    }
    pieces.push(&original[spans[spans.len() - 1][1]..]);
    let head = strip(pieces[0]);
    let tail = strip(pieces[pieces.len() - 1]);
    let middle: Vec<&str> = pieces[1..pieces.len() - 1]
        .iter()
        .map(|p| strip(p))
        .collect();
    if !text.starts_with(head) || !text.ends_with(tail) || head.len() + tail.len() > text.len() {
        return false;
    }
    let mut places: Vec<&str> = Vec::new();
    let (mut at, stop) = (head.len(), text.len() - tail.len());
    for piece in middle {
        let Some(found) = text[at..stop].find(piece).map(|i| i + at) else {
            return false;
        };
        places.push(&text[at..found]);
        at = found + piece.len();
    }
    places.push(&text[at..stop]);
    places
        .iter()
        .zip(spans.iter())
        .all(|(place, span)| sentences(place) <= span[2] && newlines(place) <= span[3])
}

fn settled(answer: &str, findings: &[(String, String)]) -> Settled {
    let mut done = Settled {
        answer: answer.to_string(),
        ..Settled::default()
    };
    for (quote, kind) in findings {
        if strip(quote).is_empty() || !done.answer.contains(quote.as_str()) {
            done.missing.push(quote.clone());
        } else if kind == "contradicted" {
            done.answer = remove_quote(&done.answer, quote);
            done.removed.push(quote.clone());
        } else {
            let marked = mark_unconfirmed(&done.answer, quote);
            if marked != done.answer {
                done.marked.push(quote.clone());
            }
            done.answer = marked;
        }
    }
    done
}

// Python's `\s` in a str pattern, written out (Rust's `\s` lacks \x1c-\x1f).
const WS: &str = r"\t\n\x0B\x0C\r\x1C-\x1F \x{85}\x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}";

fn code_rx() -> &'static Regex {
    static RX: OnceLock<Regex> = OnceLock::new();
    RX.get_or_init(|| Regex::new(r"(?s)```.*?(?:```|\z)|`[^`\n]*`").expect("valid pattern"))
}

fn tag_rx() -> &'static Regex {
    static RX: OnceLock<Regex> = OnceLock::new();
    RX.get_or_init(|| {
        Regex::new(&format!(
            r"<(/?)([A-Za-z][A-Za-z0-9_.:-]*)([{WS}][^<>]*)?[{WS}]*(/?)>"
        ))
        .expect("valid pattern")
    })
}

/// answer_marks.py:182 `without_stray_closing_tags`.
pub fn without_stray_closing_tags(text: &str) -> String {
    if !text.contains("</") {
        return text.to_string();
    }
    let code: Vec<(usize, usize)> = code_rx()
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect();
    let mut opened: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut cut: Vec<(usize, usize)> = Vec::new();
    for m in tag_rx().captures_iter(text) {
        let whole = m.get(0).expect("group 0");
        if code
            .iter()
            .any(|&(a, b)| a <= whole.start() && whole.start() < b)
            || !m.get(4).map(|g| g.as_str()).unwrap_or("").is_empty()
        {
            continue;
        }
        let name = m.get(2).expect("name").as_str().to_ascii_lowercase();
        let closing = !m.get(1).map(|g| g.as_str()).unwrap_or("").is_empty();
        if !closing {
            *opened.entry(name).or_insert(0) += 1;
        } else if m.get(3).is_some_and(|g| !g.as_str().is_empty()) {
            continue;
        } else if opened.get(&name).copied().unwrap_or(0) > 0 {
            *opened.get_mut(&name).expect("present") -= 1;
        } else {
            cut.push((whole.start(), whole.end()));
        }
    }
    if cut.is_empty() {
        return text.to_string();
    }
    let mut out = String::new();
    let mut last = 0;
    for &(a, mut b) in &cut {
        if text[..a].ends_with(' ') && text[b..].starts_with(' ') {
            b += 1;
        }
        out.push_str(&text[last..a]);
        last = b;
    }
    out.push_str(&text[last..]);
    let tail_end = cut[cut.len() - 1].1;
    if strip(&text[tail_end..]).is_empty() {
        rstrip(&out).to_string()
    } else {
        out
    }
}
