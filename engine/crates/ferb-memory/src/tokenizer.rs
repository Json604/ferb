//! FERB's own WordPiece tokenizer (`embeddings.py:117-202`): the BERT normalizer, pre-tokenizer and greedy
//! longest-match WordPiece the model's `tokenizer.json` declares, reading text as Python 3.12 does (`pystr`).

use std::collections::HashMap;

use serde_json::Value;

use crate::pystr;
use crate::pyval::{self, Crash};
use crate::Error;

/// model2vec's own cut: at most this many tokens of a text (`embeddings.py:52`).
pub const MAX_TOKENS: usize = 512;

/// `_punctuation`: ASCII punctuation ranges, or a Unicode category P*.
fn punctuation(c: char) -> bool {
    let cp = c as u32;
    (33..=47).contains(&cp)
        || (58..=64).contains(&cp)
        || (91..=96).contains(&cp)
        || (123..=126).contains(&cp)
        || pystr::is_punctuation_category(c)
}

/// `_cjk`.
fn cjk(c: char) -> bool {
    let cp = c as u32;
    (0x4E00..=0x9FFF).contains(&cp)
        || (0x3400..=0x4DBF).contains(&cp)
        || (0x20000..=0x2A6DF).contains(&cp)
        || (0x2A700..=0x2B73F).contains(&cp)
        || (0x2B740..=0x2B81F).contains(&cp)
        || (0x2B820..=0x2CEAF).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
        || (0x2F800..=0x2FA1F).contains(&cp)
}

/// The tokenizer `tokenizer.json` describes.
#[derive(Clone, Debug)]
pub struct WordPiece {
    vocab: HashMap<String, i64>,
    prefix: String,
    max_chars: usize,
    lowercase: bool,
    strip_accents: bool,
}

impl WordPiece {
    /// `WordPiece(spec)`.
    pub fn from_spec(spec: &Value) -> Result<WordPiece, Error> {
        let model = pyval::get(spec, "model")?.ok_or(Crash("KeyError"))?;
        if pyval::get(model, "type")?.and_then(Value::as_str) != Some("WordPiece") {
            return Err(Error::Refused(
                "the embedding model's tokenizer is not WordPiece".into(),
            ));
        }
        let Some(Value::Object(words)) = pyval::get(model, "vocab")? else {
            return Err(Crash("AttributeError").into());
        };
        let mut vocab = HashMap::with_capacity(words.len());
        for (k, v) in words {
            vocab.insert(k.clone(), pyval::int(v)?);
        }
        let prefix = match pyval::get(model, "continuing_subword_prefix")? {
            Some(v) if pyval::truthy(v) => pyval::to_str(v),
            _ => "##".to_owned(),
        };
        let max_chars = match pyval::get(model, "max_input_chars_per_word")? {
            Some(v) if pyval::truthy(v) => pyval::int(v)?,
            _ => 100,
        };
        let norm = pyval::obj_or_empty(pyval::get(spec, "normalizer")?)?;
        let lowercase = norm.get("lowercase").is_none_or(pyval::truthy);
        let strip_accents = match norm.get("strip_accents") {
            None | Some(Value::Null) => lowercase,
            Some(v) => pyval::truthy(v),
        };
        Ok(WordPiece {
            vocab,
            prefix,
            max_chars: usize::try_from(max_chars).unwrap_or(0),
            lowercase,
            strip_accents,
        })
    }

    /// `_normalize`: control characters out, whitespace to spaces, CJK spaced apart, lowercased, accents
    /// stripped (NFD, nonspacing marks out).
    pub fn normalize(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for c in text.chars() {
            let cp = c as u32;
            if cp == 0 || cp == 0xFFFD || (pystr::is_other(c) && !matches!(c, '\t' | '\n' | '\r')) {
                continue;
            }
            if pystr::is_space(c) {
                out.push(' ');
            } else if cjk(c) {
                out.push(' ');
                out.push(c);
                out.push(' ');
            } else {
                out.push(c);
            }
        }
        let mut text = if self.lowercase {
            pystr::lower(&out)
        } else {
            out
        };
        if self.strip_accents {
            text = pystr::nfd(&text)
                .chars()
                .filter(|&c| !pystr::is_nonspacing_mark(c))
                .collect();
        }
        text
    }

    /// `_words`: whitespace-separated chunks, each punctuation character a word of its own.
    pub fn words(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        for chunk in pystr::split_ws(text) {
            let mut word = String::new();
            for c in chunk.chars() {
                if punctuation(c) {
                    if !word.is_empty() {
                        out.push(std::mem::take(&mut word));
                    }
                    out.push(c.to_string());
                } else {
                    word.push(c);
                }
            }
            if !word.is_empty() {
                out.push(word);
            }
        }
        out
    }

    /// `ids`: token ids, a word with an unknown piece left out whole, at most [`MAX_TOKENS`].
    pub fn ids(&self, text: &str) -> Vec<i64> {
        let mut out: Vec<i64> = Vec::new();
        let mut piece = String::new();
        for word in Self::words(&self.normalize(text)) {
            let chars: Vec<char> = word.chars().collect();
            if chars.len() > self.max_chars {
                continue;
            }
            let mut start = 0;
            let mut pieces = Vec::new();
            while start < chars.len() {
                let mut end = chars.len();
                let mut found = None;
                while start < end {
                    piece.clear();
                    if start != 0 {
                        piece.push_str(&self.prefix);
                    }
                    piece.extend(&chars[start..end]);
                    if let Some(&id) = self.vocab.get(&piece) {
                        found = Some(id);
                        break;
                    }
                    end -= 1;
                }
                match found {
                    Some(id) => pieces.push(id),
                    None => {
                        pieces.clear();
                        break;
                    }
                }
                start = end;
            }
            out.extend(pieces);
            if out.len() >= MAX_TOKENS {
                out.truncate(MAX_TOKENS);
                return out;
            }
        }
        out
    }
}
