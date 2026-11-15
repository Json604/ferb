//! Python 3.12's `shlex.shlex(text, posix=True, punctuation_chars=";&|<>()\n")` with whitespace " \t\r",
//! whitespace_split and no commenters, read to its end (`list(lex)`), as policy/tls.py's `_words` sets it up. Err
//! where Python raises ValueError (an unclosed quote, a final escape).

const PUNCTUATION: &str = ";&|<>()\n";
const WHITESPACE: &str = " \t\r";
const QUOTES: &str = "'\"";

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Space,
    Word,
    Punct,
    Quote(char),
    Escape,
    Eof,
}

/// Python's ValueError: an unclosed quote, or an escape at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unclosed;

/// The tokens, or Err for Python's ValueError.
pub fn split(text: &str) -> Result<Vec<String>, Unclosed> {
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0usize;
    let mut pushback: Vec<char> = Vec::new();
    let mut state = State::Space;
    let mut out = Vec::new();
    loop {
        // read_token
        let mut token = String::new();
        let mut quoted = false;
        let mut escaped_state = State::Word;
        loop {
            let next: Option<char> = if let Some(c) = pushback.pop() {
                Some(c)
            } else if at < chars.len() {
                at += 1;
                Some(chars[at - 1])
            } else {
                None
            };
            match state {
                State::Eof => {
                    token.clear();
                    break;
                }
                State::Space => match next {
                    None => {
                        state = State::Eof;
                        break;
                    }
                    Some(c) if WHITESPACE.contains(c) => {
                        if !token.is_empty() || quoted {
                            break;
                        }
                    }
                    Some('\\') => {
                        escaped_state = State::Word;
                        state = State::Escape;
                    }
                    Some(c) if PUNCTUATION.contains(c) => {
                        token.push(c);
                        state = State::Punct;
                    }
                    Some(c) if QUOTES.contains(c) => state = State::Quote(c),
                    Some(c) => {
                        token.push(c);
                        state = State::Word;
                    }
                },
                State::Quote(q) => {
                    quoted = true;
                    match next {
                        None => return Err(Unclosed),
                        Some(c) if c == q => state = State::Word,
                        Some('\\') if q == '"' => {
                            escaped_state = state;
                            state = State::Escape;
                        }
                        Some(c) => token.push(c),
                    }
                }
                State::Escape => {
                    let Some(c) = next else {
                        return Err(Unclosed);
                    };
                    if let State::Quote(q) = escaped_state {
                        if c != '\\' && c != q {
                            token.push('\\');
                        }
                    }
                    token.push(c);
                    state = escaped_state;
                }
                State::Word | State::Punct => match next {
                    None => {
                        state = State::Eof;
                        break;
                    }
                    Some(c) if WHITESPACE.contains(c) => {
                        state = State::Space;
                        if !token.is_empty() || quoted {
                            break;
                        }
                    }
                    Some(c) if state == State::Punct => {
                        if PUNCTUATION.contains(c) {
                            token.push(c);
                        } else {
                            if !WHITESPACE.contains(c) {
                                pushback.push(c);
                            }
                            state = State::Space;
                            break;
                        }
                    }
                    Some(c) if QUOTES.contains(c) => state = State::Quote(c),
                    Some('\\') => {
                        escaped_state = State::Word;
                        state = State::Escape;
                    }
                    Some(c) if !PUNCTUATION.contains(c) => token.push(c),
                    Some(c) => {
                        pushback.push(c);
                        state = State::Space;
                        if !token.is_empty() || quoted {
                            break;
                        }
                    }
                },
            }
        }
        if !quoted && token.is_empty() {
            return Ok(out); // None: the end
        }
        out.push(token);
    }
}
