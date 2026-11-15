//! Port of ferb/sanitize/terminal.py: making text from models, pages and apps safe to print on a terminal. It
//! never changes what the text says: only control characters go, and nothing judges the text.

/// terminal.py:9: the text without C0/C1 control characters (and DEL); newlines and tabs stay.
pub fn printable(text: &str) -> String {
    text.chars()
        .filter(|&c| !matches!(c as u32, 0x00..=0x08 | 0x0b..=0x1f | 0x7f..=0x9f))
        .collect()
}
