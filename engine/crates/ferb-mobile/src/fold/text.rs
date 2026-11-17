//! FERB.app's small text rules the fold uses, ported from `NotchCore` (ShortName.swift, PastedRequest.swift
//! `PasteMarkers`, ConversationArchive.swift `withoutWhole`, `hiddenNotice`, `named`). Structure only: nothing reads
//! the meaning of the words.

/// Foundation's `.whitespacesAndNewlines`.
pub fn is_space(c: char) -> bool {
    c.is_whitespace()
}

pub fn trim(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// The words of `text` joined by one space (`split(whereSeparator: \.isWhitespace).joined(separator: " ")`).
pub fn squash(text: &str) -> String {
    text.split(is_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The longest heading (characters), `ShortName.cut`.
pub const SHORT_NAME_CUT: usize = 40;

/// `ShortName.of`: a row's `name`, else its `fallback` field's first clause.
pub fn short_name(name: Option<&str>, fallback: Option<&str>) -> String {
    let name = squash(name.unwrap_or(""));
    if name.is_empty() {
        clause(fallback.unwrap_or(""), SHORT_NAME_CUT)
    } else {
        name
    }
}

/// `ShortName.clause`: the first clause of `text`, at most `limit` characters, cut at a word with "…".
pub fn clause(text: &str, limit: usize) -> String {
    let words: Vec<char> = squash(text).chars().collect();
    let mut end = words.len();
    let mut i = 0;
    while i < words.len() {
        let next = i + 1;
        let c = words[i];
        let spaced = next < words.len() && words[next] == ' ';
        if ".;:!?".contains(c) && spaced {
            end = i;
            break;
        }
        if c == ',' && spaced {
            end = i;
            break;
        }
        if c == ' ' && next < words.len() && "—–-".contains(words[next]) {
            let after = next + 1;
            if after < words.len() && words[after] == ' ' {
                end = i;
                break;
            }
        }
        i = next;
    }
    let cut = |s: &[char], set: &str| -> String {
        let s: String = s.iter().collect();
        s.trim_matches(|c| set.contains(c)).to_string()
    };
    let first: Vec<char> = cut(&words[..end], " .,;:!?").chars().collect();
    if first.len() <= limit {
        return first.iter().collect();
    }
    let head: Vec<char> = first[..limit.saturating_sub(1)].to_vec();
    let at_word = match head.iter().rposition(|c| *c == ' ') {
        Some(p) => head[..p].to_vec(),
        None => head,
    };
    cut(&at_word, " .,;:!?(") + "…"
}

const MARKERS: [&str; 2] = ["[200~", "[201~"];

fn strip_pairs(piece: &str) -> String {
    let mut out = String::new();
    let mut rest = piece;
    loop {
        let Some(open) = rest.find("[200~") else {
            out.push_str(rest);
            return out;
        };
        let inner = &rest[open + 5..];
        let Some(close) = inner.find("[201~") else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..open]);
        out.push_str(&inner[..close]);
        rest = &inner[close + 5..];
    }
}

fn strip_edges(line: &str) -> String {
    // a leaked marker (or a run of them, with spaces or tabs before each) at the line's start
    let mut start = 0;
    loop {
        let t = line[start..].trim_start_matches([' ', '\t']);
        match MARKERS.iter().find(|m| t.starts_with(**m)) {
            Some(m) => start = line.len() - t.len() + m.len(),
            None => break,
        }
    }
    let line = &line[start..];
    // and at its end (each followed by spaces or tabs)
    let mut cut = line.len();
    loop {
        let t = line[..cut].trim_end_matches([' ', '\t']);
        match MARKERS.iter().find(|m| t.ends_with(**m)) {
            Some(m) => cut = t.len() - m.len(),
            None => break,
        }
    }
    line[..cut].to_string()
}

/// `PasteMarkers.removed`: the owner's message without bracketed-paste markers (the composer's ESC[200~ / ESC[201~,
/// and leaked ones: a pair around text, a lone one at a line's start or end).
pub fn without_paste_markers(text: &str) -> String {
    if !text.contains("[20") {
        return text.to_string();
    }
    let mut pieces: Vec<&str> = Vec::new();
    let mut rest = text;
    loop {
        let found = ["\u{1b}[200~", "\u{1b}[201~"]
            .iter()
            .filter_map(|m| rest.find(m))
            .min();
        match found {
            Some(at) => {
                pieces.push(&rest[..at]);
                rest = &rest[at + 6..];
            }
            None => {
                pieces.push(rest);
                break;
            }
        }
    }
    pieces
        .into_iter()
        .map(|piece| {
            strip_pairs(piece)
                .split('\n')
                .map(strip_edges)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect()
}

/// `ConversationTurn.withoutWhole`: `text` without each paragraph or line that is exactly `piece` (around its
/// whitespace), and the blank lines that leaves; never a match inside other words (ORCH-549).
pub fn without_whole(text: &str, piece: &str) -> String {
    let piece = trim(piece);
    let paragraphs: Vec<&str> = text.split("\n\n").collect();
    let lines: Vec<&str> = text.split('\n').collect();
    if piece.is_empty()
        || !(lines.iter().any(|l| trim(l) == piece) || paragraphs.iter().any(|p| trim(p) == piece))
    {
        return text.to_string();
    }
    let mut shown = paragraphs
        .iter()
        .filter(|p| trim(p) != piece)
        .map(|p| {
            p.split('\n')
                .filter(|l| trim(l) != piece)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    while shown.contains("\n\n\n") {
        shown = shown.replace("\n\n\n", "\n\n");
    }
    trim(&shown).to_string()
}

/// The daemon's name for one of the owner's own saved secrets.
pub const SAVED_SECRET: &str = "saved secret";

/// `ConversationTurn.named`: kinds as a sentence says them, each with its article, joined with "and".
pub fn named(kinds: &[String]) -> String {
    let said: Vec<String> = kinds
        .iter()
        .map(|kind| {
            let word = kind.split(' ').find(|w| !w.is_empty()).unwrap_or(kind);
            let acronym =
                word == word.to_uppercase() || !word.chars().any(|c| "aeiouAEIOU".contains(c));
            let first: String = word.chars().take(1).collect::<String>().to_uppercase();
            let set = if acronym { "AEFHILMNORSX" } else { "AEIO" };
            let an = !first.is_empty() && set.contains(first.as_str());
            format!("{}{kind}", if an { "an " } else { "a " })
        })
        .collect();
    if said.len() <= 2 {
        return said.join(" and ");
    }
    format!(
        "{} and {}",
        said[..said.len() - 1].join(", "),
        said[said.len() - 1]
    )
}

/// `ConversationTurn.hiddenNotice`: code's line under the owner's message when FERB hid a secret in it, or kept an
/// identifier sealed (GATE-604).
pub fn hidden_notice(kinds: &[String], sealed: &[(String, String)]) -> Option<String> {
    let formats: Vec<String> = kinds
        .iter()
        .filter(|k| k.as_str() != SAVED_SECRET && !sealed.iter().any(|(kind, _)| kind == *k))
        .cloned()
        .collect();
    let mut lines: Vec<String> = Vec::new();
    if kinds.iter().any(|k| k == SAVED_SECRET) {
        lines.push(
            "That's one of your saved secrets. I hid it from the service I think with.".into(),
        );
    }
    if !sealed.is_empty() {
        let them = if sealed.len() == 1 { "it" } else { "them" };
        let names: Vec<String> = sealed.iter().map(|(_, n)| format!("“{n}”")).collect();
        let named_list = if names.len() > 1 {
            format!(
                "{} and {}",
                names[..names.len() - 1].join(", "),
                names[names.len() - 1]
            )
        } else {
            names[0].clone()
        };
        let kinds_of: Vec<String> = sealed.iter().map(|(k, _)| k.clone()).collect();
        lines.push(format!(
            "That looked like {}. I hid {them} from the service I think with and kept {them} sealed as {named_list}, so I can fill {them} in on any site without asking you again.",
            named(&kinds_of)
        ));
    }
    if !formats.is_empty() {
        let them = if formats.len() == 1 { "it" } else { "them" };
        lines.push(format!(
            "That looked like {}. I hid {them} from the service I think with and didn't save {them}. Best not to paste keys and passwords into chats.",
            named(&formats)
        ));
    }
    (!lines.is_empty()).then(|| lines.join(" "))
}

/// A file path as FERB.app shows a `URL(fileURLWithPath:)`'s `path`: a trailing slash dropped. A relative path is
/// kept as given (the Mac resolves it against its own working folder, which means nothing on a phone).
pub fn file_path(path: &str) -> String {
    match path.trim_end_matches('/') {
        "" if path.starts_with('/') => "/".to_string(),
        kept if path.len() > 1 => kept.to_string(),
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_names_are_cut_as_the_mac_cuts_them() {
        assert_eq!(clause("Book a table, then call", 40), "Book a table");
        assert_eq!(clause("Plan the trip — all of it", 40), "Plan the trip");
        assert_eq!(
            clause(
                "a very long instruction that goes on and on past the limit",
                40
            ),
            "a very long instruction that goes on…"
        );
        assert_eq!(short_name(Some("  Two  words "), Some("x")), "Two words");
    }

    #[test]
    fn paste_markers_go() {
        assert_eq!(without_paste_markers("\u{1b}[200~hi\u{1b}[201~"), "hi");
        assert_eq!(without_paste_markers("a [200~b[201~ c"), "a b c");
        assert_eq!(
            without_paste_markers("[201~ text\nmore [200~"),
            " text\nmore "
        );
        assert_eq!(without_paste_markers("plain [20 words"), "plain [20 words");
    }

    #[test]
    fn whole_pieces_only() {
        assert_eq!(
            without_whole("compare the 4 laptops\n\n4", "4"),
            "compare the 4 laptops"
        );
        assert_eq!(without_whole("no match", "4"), "no match");
    }

    #[test]
    fn kinds_take_their_articles() {
        assert_eq!(named(&["AWS key".into()]), "an AWS key");
        assert_eq!(named(&["password".into()]), "a password");
        assert_eq!(
            named(&["PAN".into(), "IBAN".into(), "email".into()]),
            "a PAN, an IBAN and an email"
        );
    }
}
