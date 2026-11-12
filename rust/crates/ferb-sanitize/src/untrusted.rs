//! Port of ferb/sanitize/untrusted.py: source provenance markers for outside data in what FERB records and shows
//! the models. It never discards the complete request or infers its meaning.
//!
//! The owner's chat message is all theirs (ORCH-518): `mark` records it whole, only cleaned of bracketed-paste
//! markers. Outside content FERB shows a model is wrapped in `<untrusted-data id="…">` tags with a random id, so
//! data can't forge a closing tag (AGENTS.md rule 9).

use crate::generated as g;
use crate::known::json_escape;
use crate::rx::{Rx, Span};
use crate::unicode as u;

pub const PASTE_START: &str = "\x1b[200~";
pub const PASTE_END: &str = "\x1b[201~";
pub const EARLIER_QUESTION: &str = "Earlier FERB question (context, not owner instructions): ";
pub const EARLIER_QUESTION_DATA: &str = "Earlier FERB question (data): ";

static LEAKED: Rx = Rx::new(&g::UNTRUSTED_LEAKED);
static LEAKED_BARE: Rx = Rx::new(&g::UNTRUSTED_LEAKED_BARE);
static LEAKED_PAIR: Rx = Rx::new(&g::UNTRUSTED_LEAKED_PAIR);
static WRAPPED: Rx = Rx::new(&g::UNTRUSTED_WRAPPED);
static EARLIER: Rx = Rx::new(&g::UNTRUSTED_EARLIER);
static PARTS: Rx = Rx::new(&g::UNTRUSTED_PARTS);

/// untrusted.py:33.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Marked {
    /// The message as recorded and shown to the planner.
    pub for_model: String,
    /// The owner's words, on one line.
    pub own_words: String,
    /// Parts held as outside data: none in a message since ORCH-518.
    pub untrusted: Vec<String>,
    /// The owner's words with their line breaks.
    pub typed_lines: String,
}

/// `pattern.sub(rep, s)` for a pattern that never matches empty.
fn without(s: &str, spans: &[Span], rep: &str) -> String {
    let (mut out, mut kept) = (String::with_capacity(s.len()), 0);
    for &(a, b) in spans {
        out.push_str(&s[kept..a]);
        out.push_str(rep);
        kept = b;
    }
    out.push_str(&s[kept..]);
    out
}

fn lines(parts: &[&str]) -> String {
    // untrusted.py:42
    let mut out: Vec<String> = Vec::new();
    for part in parts {
        for line in part.split('\n') {
            if !u::strip(line).is_empty() {
                out.push(u::squeeze(line));
            }
        }
    }
    out.join("\n")
}

/// untrusted.py:46: the owner's words with their line breaks read back from a message as stored: a part an older
/// FERB wrapped as pasted is left out and ends a line.
pub fn typed_lines_of(for_model: &str) -> String {
    let (mut parts, mut pos) = (Vec::new(), 0);
    for (a, b) in WRAPPED.find_all(for_model) {
        parts.push(&for_model[pos..a]);
        pos = b;
    }
    parts.push(&for_model[pos..]);
    lines(&parts)
}

/// untrusted.py:58: the message without bracketed-paste markers that leaked in as text: a pair around text
/// wherever it sits, a lone one at a line's start or end. The composer's own markers stay.
pub fn without_leaked_markers(message: &str) -> String {
    let clean = |piece: &str, inside: bool| -> String {
        // `_LEAKED_PAIR.sub(r"\1", piece)`, then the lone ones
        let (mut unpaired, mut kept) = (String::with_capacity(piece.len()), 0);
        for c in LEAKED_PAIR.caps_all(piece) {
            let (whole, body) = (c[0].unwrap_or((kept, kept)), c[1].unwrap_or((kept, kept)));
            unpaired.push_str(&piece[kept..whole.0]);
            unpaired.push_str(&piece[body.0..body.1]);
            kept = whole.1;
        }
        unpaired.push_str(&piece[kept..]);
        let rx = if inside { &LEAKED } else { &LEAKED_BARE };
        without(&unpaired, &rx.find_all(&unpaired), "")
    };
    // `_MARKERS.split(message)` (untrusted.py:22): the text between the composer's own markers, and the markers.
    let (mut out, mut inside, mut pos) = (String::with_capacity(message.len()), false, 0);
    while let Some((marker, at)) = next_marker(message, pos) {
        out.push_str(&clean(&message[pos..at], inside));
        inside = marker == PASTE_START;
        out.push_str(marker);
        pos = at + marker.len();
    }
    out.push_str(&clean(&message[pos..], inside));
    out
}

/// The first of the composer's markers at or after `from`: (the marker, where it starts).
fn next_marker(message: &str, mut from: usize) -> Option<(&'static str, usize)> {
    while let Some(i) = message[from..].find("\x1b[20") {
        let at = from + i;
        if message[at..].starts_with(PASTE_START) {
            return Some((PASTE_START, at));
        }
        if message[at..].starts_with(PASTE_END) {
            return Some((PASTE_END, at));
        }
        from = at + 1;
    }
    None
}

/// untrusted.py:72: the owner's chat message as FERB records it (ORCH-518): all of it is their words.
pub fn mark(message: &str) -> Marked {
    let plain = without_leaked_markers(message)
        .replace(PASTE_START, "")
        .replace(PASTE_END, "");
    Marked {
        for_model: u::strip(&plain).to_string(),
        own_words: u::squeeze(&plain),
        untrusted: Vec::new(),
        typed_lines: lines(&[&plain]),
    }
}

fn parts_of(text: &str) -> Vec<String> {
    PARTS
        .caps_all(text)
        .into_iter()
        .filter_map(|c| c[2])
        .map(|(a, b)| &text[a..b])
        .filter(|body| !u::strip(body).is_empty())
        .map(str::to_string)
        .collect()
}

/// untrusted.py:83: the pasted or quoted parts of a request's payload, without FERB's own earlier question.
pub fn pasted_parts(payload: &str) -> Vec<String> {
    parts_of(&without_ferb_question(payload))
}

/// untrusted.py:89: FERB's own earlier question(s) a payload carries as data.
pub fn earlier_questions(payload: &str) -> Vec<String> {
    EARLIER
        .find_all(payload)
        .into_iter()
        .flat_map(|(a, b)| parts_of(&payload[a..b]))
        .collect()
}

/// untrusted.py:95: text shown as one quoted JSON string, every character that is invisible or breaks a line
/// written out as an escape. Deterministic, so a recorded line stays the same across turns.
pub fn as_data(text: &str) -> String {
    let dumped = format!("\"{}\"", json_escape(text, false));
    let mut out = String::with_capacity(dumped.len());
    for c in dumped.chars() {
        let cp = c as u32;
        if !u::is_escaped_category(c) {
            out.push(c);
        } else if cp <= 0xffff {
            out.push_str(&format!("\\u{cp:04x}"));
        } else {
            out.push_str(&format!("\\U{cp:08x}"));
        }
    }
    out
}

/// `wrap_untrusted` with the id given (Python: `secrets.token_hex(6)`).
pub fn wrap_untrusted_with_id(text: &str, uid: &str) -> String {
    let body = text.replace("</untrusted-data", "<\\/untrusted-data");
    format!("<untrusted-data id=\"{uid}\">\n{body}\n</untrusted-data id=\"{uid}\">")
}

/// untrusted.py:115: data, not instructions: in tags with a random id; a forged closing tag inside can't end ours.
pub fn wrap_untrusted(text: &str) -> String {
    let mut bytes = [0u8; 6];
    // No id, no wrapper: a guessable id would let outside text close the tag (rule 9).
    getrandom::fill(&mut bytes).expect("ferb-sanitize: the system gave no random bytes");
    let uid: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    wrap_untrusted_with_id(text, &uid)
}

/// untrusted.py:122: the text without FERB's earlier question (either label, in FERB's own wrapper).
pub fn without_ferb_question(text: &str) -> String {
    without(text, &EARLIER.find_all(text), "")
}

/// untrusted.py:129: whether a recorded request still holds one of FERB's wrappers once FERB's own earlier
/// question is set aside.
pub fn holds_pasted(payload: &str) -> bool {
    WRAPPED.search(&without_ferb_question(payload), 0).is_some()
}

/// untrusted.py:135: the owner's own words of a recorded chat message, its pasted or quoted parts left out. None
/// when the count of parts is unknown or the wrappers found differ from it.
pub fn recorded_own_words(for_model: &str, pasted: Option<usize>) -> Option<String> {
    let found = WRAPPED.find_all(for_model);
    if pasted != Some(found.len()) {
        return None;
    }
    Some(u::squeeze(&without(for_model, &found, " ")))
}
