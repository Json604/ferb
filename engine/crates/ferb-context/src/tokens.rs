//! Token estimates (ferb/core/context.py:24-162). Counts use o200k_base as an estimate, not a claim about every
//! provider's tokenizer, and count each `TOKEN_SLICE`-character slice of a text on its own (that bounds the work on
//! a huge unbroken string and makes the count add up over slices; see `GrowingCount`).
//!
//! Python loads the ranks from FERB's pinned copy under its home, else tiktoken's cache, and falls back to a UTF-8
//! byte ceiling when neither is there. Here the ranks are compiled in (tiktoken-rs carries the same file, SHA-256
//! 446a9538…, the one context.py pins), so `Tokenizer::O200k` is always available; `Tokenizer::Utf8Bytes` is kept
//! for the parity of the fallback's text (`token_method`) and for a caller that wants the ceiling.
//!
//! Python's memory counts 4 characters per token (`memory/profile.py:39`): a different estimate, not this one.

use std::sync::LazyLock;

use tiktoken_rs::CoreBPE;

/// context.py:24 `_ENCODING_URL` and :25 `_ENCODING_SHA256`: where Python's copy comes from and its pinned hash.
pub const TOKENIZER_URL: &str =
    "https://openaipublic.blob.core.windows.net/encodings/o200k_base.tiktoken";
pub const TOKENIZER_SHA256: &str =
    "446a9538cb6c348e3516120d7c08b09f57c36495e2acfffe59a5bf8b0cfb1a2d";

/// context.py:27 `_PATTERN`: o200k_base's public pattern; tiktoken-rs splits with the same one (checked in a test).
pub const PATTERN: &str = concat!(
    r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]*[\p{Ll}\p{Lm}\p{Lo}\p{M}]+(?i:'s|'t|'re|'ve|'m|'ll|'d)?",
    "|",
    r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]+[\p{Ll}\p{Lm}\p{Lo}\p{M}]*(?i:'s|'t|'re|'ve|'m|'ll|'d)?",
    "|",
    r"\p{N}{1,3}",
    "|",
    r" ?[^\s\p{L}\p{N}]+[\r\n/]*",
    "|",
    r"\s*[\r\n]+",
    "|",
    r"\s+(?!\S)",
    "|",
    r"\s+",
);

/// context.py:81: `count_tokens` counts each slice of this many characters (code points) on its own.
pub const TOKEN_SLICE: usize = 8192;

static O200K: LazyLock<&'static CoreBPE> = LazyLock::new(tiktoken_rs::o200k_base_singleton);

/// How a text's tokens are estimated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tokenizer {
    /// o200k_base, slice by slice (always available here).
    #[default]
    O200k,
    /// The conservative ceiling Python uses when its tokenizer file is missing: one token per UTF-8 byte.
    Utf8Bytes,
}

impl Tokenizer {
    /// context.py:77 `token_method`: what the budget report says the count is.
    pub fn method(self) -> &'static str {
        match self {
            Tokenizer::O200k => "o200k_base_chunked8192_estimate_provider_may_differ",
            Tokenizer::Utf8Bytes => "utf8_byte_ceiling_tokenizer_unavailable",
        }
    }

    /// context.py:84 `count_tokens`.
    pub fn count(self, text: &str) -> usize {
        match self {
            Tokenizer::Utf8Bytes => text.len(),
            Tokenizer::O200k => slices(text, TOKEN_SLICE)
                .map(|slice| O200K.encode_ordinary(slice).len())
                .sum(),
        }
    }
}

/// `count_tokens(text)` with o200k_base.
pub fn count_tokens(text: &str) -> usize {
    Tokenizer::O200k.count(text)
}

/// `token_method()` with o200k_base (always available here).
pub fn token_method() -> &'static str {
    Tokenizer::O200k.method()
}

/// `text[i:i + size]` for i in range(0, len(text), size), by code point.
pub fn slices(text: &str, size: usize) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let end = rest
            .char_indices()
            .nth(size)
            .map_or(rest.len(), |(at, _)| at);
        let (head, tail) = rest.split_at(end);
        rest = tail;
        Some(head)
    })
}

/// context.py:117 `GrowingCount`: the count of a text that only grows at its end, kept exact without counting it
/// again (each complete slice's count is kept; only the last, partial slice is counted again). `count` is
/// `count_tokens` or another count that adds up over `TOKEN_SLICE`-character slices.
pub struct GrowingCount<F: Fn(&str) -> usize> {
    count: F,
    /// the counts of the complete slices
    pub closed: usize,
    /// the text after them, shorter than `TOKEN_SLICE` characters
    pub tail: String,
}

impl<F: Fn(&str) -> usize> GrowingCount<F> {
    pub fn new(count: F) -> GrowingCount<F> {
        GrowingCount {
            count,
            closed: 0,
            tail: String::new(),
        }
    }

    /// `count_tokens(text + suffix)`; the text itself is unchanged.
    pub fn with_suffix(&self, suffix: &str) -> usize {
        let rest = format!("{}{suffix}", self.tail);
        self.closed
            + slices(&rest, TOKEN_SLICE)
                .map(|s| (self.count)(s))
                .sum::<usize>()
    }

    pub fn append(&mut self, more: &str) {
        let rest = format!("{}{more}", self.tail);
        let chars = rest.chars().count();
        let whole = chars - chars % TOKEN_SLICE;
        let cut = rest
            .char_indices()
            .nth(whole)
            .map_or(rest.len(), |(at, _)| at);
        self.closed += slices(&rest[..cut], TOKEN_SLICE)
            .map(|s| (self.count)(s))
            .sum::<usize>();
        self.tail = rest[cut..].to_string();
    }
}

/// context.py:138 `comma_bounds`: each position (in characters) right after a comma that a space follows; an
/// o200k piece always ends there.
pub fn comma_bounds(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut chars = text.chars().enumerate().peekable();
    while let Some((at, c)) = chars.next() {
        if c == ',' && chars.peek().is_some_and(|(_, next)| *next == ' ') {
            out.push(at + 1);
        }
    }
    out
}

/// context.py:147 `uncut_count`: at most what atoms `lo..hi-1` add to `count_tokens` of the text they're in. Atom
/// i runs from `shift + ends[i]` to `shift + ends[i + 1]` (characters); `totals[i]` is the sum of the counts of
/// the atoms before i. An atom that a slice edge falls inside is left out.
pub fn uncut_count(ends: &[i64], totals: &[i64], lo: usize, hi: usize, shift: i64) -> i64 {
    if hi <= lo {
        return 0;
    }
    let slice = TOKEN_SLICE as i64;
    let mut bound = totals[hi] - totals[lo];
    let mut edge = (shift + ends[lo]).div_euclid(slice) * slice + slice;
    let end = shift + ends[hi];
    let mut cut: i64 = -1;
    while edge < end {
        // bisect_right(ends, edge - shift, lo, hi) - 1: ends[i] <= edge - shift < ends[i + 1]
        // (edge - shift > ends[lo] always, so i >= lo)
        let target = edge - shift;
        let i = lo + ends[lo..hi].partition_point(|&e| e <= target) - 1;
        if shift + ends[i] < edge && i as i64 != cut {
            bound -= totals[i + 1] - totals[i];
            cut = i as i64;
        }
        edge += slice;
    }
    bound
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pattern_is_the_one_tiktoken_rs_splits_with() {
        assert_eq!(PATTERN, tiktoken_rs::O200K_BASE_PAT_STR);
    }

    #[test]
    fn slices_are_by_code_point() {
        let text = "é".repeat(5);
        let parts: Vec<&str> = slices(&text, 2).collect();
        assert_eq!(parts, ["éé", "éé", "é"]);
        assert_eq!(slices("", 3).count(), 0);
    }
}
