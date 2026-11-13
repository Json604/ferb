//! The extension's key table (`protocol.js` `keyEvent` and `entersText`), so the daemon refuses
//! exactly the key presses the extension itself would put into a secret field (decision: HAND-218).
//! The extension is the unchanged other end: this follows its JavaScript, not Python's
//! `enters_text`, which disagrees on six specs (`Cmd+V`, `cmd+shift+v`, `a+`, `é`, `hyper+a`,
//! `x+a`; fixed against Python, named in `tests/keys.rs`). Checked against protocol.js run under
//! node (`rust/fixtures/browser/keys.json`).
//!
//! JavaScript's own rules kept: `trim` strips JavaScript's whitespace (not Python's), `length` counts
//! UTF-16 units (a character outside the BMP is two), `toLowerCase` is Unicode's, and a name that is
//! a property of `Object.prototype` reads that property: as a modifier it adds no bit; as a key the
//! extension's `keyEvent` throws ([`Throws`]).

use serde_json::{json, Value};

/// `MOD_BITS`: Alt 1, Ctrl 2, Meta/Cmd 4, Shift 8 (CDP's Input domain).
const MOD_BITS: [(&str, i64); 9] = [
    ("alt", 1),
    ("opt", 1),
    ("option", 1),
    ("ctrl", 2),
    ("control", 2),
    ("meta", 4),
    ("cmd", 4),
    ("command", 4),
    ("shift", 8),
];

/// `NAMED_KEYS`: key -> (code, windowsVirtualKeyCode, text).
const NAMED_KEYS: [(&str, &str, i64, Option<&str>); 27] = [
    ("Enter", "Enter", 13, Some("\r")),
    ("Tab", "Tab", 9, None),
    ("Escape", "Escape", 27, None),
    ("Backspace", "Backspace", 8, None),
    ("Delete", "Delete", 46, None),
    ("Space", "Space", 32, Some(" ")),
    ("ArrowDown", "ArrowDown", 40, None),
    ("ArrowUp", "ArrowUp", 38, None),
    ("ArrowLeft", "ArrowLeft", 37, None),
    ("ArrowRight", "ArrowRight", 39, None),
    ("Home", "Home", 36, None),
    ("End", "End", 35, None),
    ("PageUp", "PageUp", 33, None),
    ("PageDown", "PageDown", 34, None),
    ("Insert", "Insert", 45, None),
    ("F1", "F1", 112, None),
    ("F2", "F2", 113, None),
    ("F3", "F3", 114, None),
    ("F4", "F4", 115, None),
    ("F5", "F5", 116, None),
    ("F6", "F6", 117, None),
    ("F7", "F7", 118, None),
    ("F8", "F8", 119, None),
    ("F9", "F9", 120, None),
    ("F10", "F10", 121, None),
    ("F11", "F11", 122, None),
    ("F12", "F12", 123, None),
];

const KEY_ALIASES: [(&str, &str); 23] = [
    ("enter", "Enter"),
    ("return", "Enter"),
    ("tab", "Tab"),
    ("esc", "Escape"),
    ("escape", "Escape"),
    ("backspace", "Backspace"),
    ("delete", "Delete"),
    ("del", "Delete"),
    ("space", "Space"),
    ("spacebar", "Space"),
    ("down", "ArrowDown"),
    ("up", "ArrowUp"),
    ("left", "ArrowLeft"),
    ("right", "ArrowRight"),
    ("arrowdown", "ArrowDown"),
    ("arrowup", "ArrowUp"),
    ("arrowleft", "ArrowLeft"),
    ("arrowright", "ArrowRight"),
    ("home", "Home"),
    ("end", "End"),
    ("pageup", "PageUp"),
    ("pagedown", "PageDown"),
    ("insert", "Insert"),
];

const PUNCT: [(char, &str, i64); 11] = [
    ('-', "Minus", 189),
    ('=', "Equal", 187),
    (',', "Comma", 188),
    ('.', "Period", 190),
    ('/', "Slash", 191),
    (';', "Semicolon", 186),
    ('\'', "Quote", 222),
    ('[', "BracketLeft", 219),
    (']', "BracketRight", 221),
    ('\\', "Backslash", 220),
    ('`', "Backquote", 192),
];

const SHIFTED: [(char, char); 21] = [
    ('!', '1'),
    ('@', '2'),
    ('#', '3'),
    ('$', '4'),
    ('%', '5'),
    ('^', '6'),
    ('&', '7'),
    ('*', '8'),
    ('(', '9'),
    (')', '0'),
    ('_', '-'),
    ('+', '='),
    ('{', '['),
    ('}', ']'),
    ('|', '\\'),
    (':', ';'),
    ('"', '\''),
    ('<', ','),
    ('>', '.'),
    ('?', '/'),
    ('~', '`'),
];

const MAC_COMMANDS: [(&str, &str); 33] = [
    ("4+a", "selectAll"),
    ("4+c", "copy"),
    ("4+x", "cut"),
    ("4+v", "paste"),
    ("4+z", "undo"),
    ("12+z", "redo"),
    ("0+Backspace", "deleteBackward"),
    ("0+Delete", "deleteForward"),
    ("1+Backspace", "deleteWordBackward"),
    ("1+Delete", "deleteWordForward"),
    ("4+Backspace", "deleteToBeginningOfLine"),
    ("4+Delete", "deleteToEndOfLine"),
    ("0+ArrowLeft", "moveLeft"),
    ("0+ArrowRight", "moveRight"),
    ("0+ArrowUp", "moveUp"),
    ("0+ArrowDown", "moveDown"),
    ("1+ArrowLeft", "moveWordLeft"),
    ("1+ArrowRight", "moveWordRight"),
    ("4+ArrowLeft", "moveToBeginningOfLine"),
    ("4+ArrowRight", "moveToEndOfLine"),
    ("4+ArrowUp", "moveToBeginningOfDocument"),
    ("4+ArrowDown", "moveToEndOfDocument"),
    ("8+ArrowLeft", "moveLeftAndModifySelection"),
    ("8+ArrowRight", "moveRightAndModifySelection"),
    ("8+ArrowUp", "moveUpAndModifySelection"),
    ("8+ArrowDown", "moveDownAndModifySelection"),
    ("9+ArrowLeft", "moveWordLeftAndModifySelection"),
    ("9+ArrowRight", "moveWordRightAndModifySelection"),
    ("12+ArrowLeft", "moveToBeginningOfLineAndModifySelection"),
    ("12+ArrowRight", "moveToEndOfLineAndModifySelection"),
    ("12+ArrowUp", "moveToBeginningOfDocumentAndModifySelection"),
    ("12+ArrowDown", "moveToEndOfDocumentAndModifySelection"),
    ("0+Escape", "cancelOperation"),
];

/// `Object.prototype`'s own property names: a plain object lookup by one of them finds it.
const PROTOTYPE: [&str; 12] = [
    "constructor",
    "__proto__",
    "toString",
    "toLocaleString",
    "valueOf",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
];

/// The extension's `keyEvent` threw (a key named like a property of `Object.prototype`): its command
/// fails there, so nothing is typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Throws;

/// `keyEvent`'s answer: CDP's key event for the spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: String,
    pub code: String,
    pub key_code: i64,
    pub text: Option<String>,
    pub modifiers: i64,
    pub commands: Option<Vec<String>>,
}

impl KeyEvent {
    /// As `JSON.stringify` writes it (undefined fields left out).
    pub fn to_json(&self) -> Value {
        let mut v = json!({"key": self.key, "code": self.code, "keyCode": self.key_code,
                           "modifiers": self.modifiers});
        if let Some(t) = &self.text {
            v["text"] = t.clone().into();
        }
        if let Some(c) = &self.commands {
            v["commands"] = c.clone().into();
        }
        v
    }
}

/// JavaScript's `String.prototype.trim` (WhiteSpace and LineTerminator).
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| {
        matches!(
            c,
            '\u{9}'
                | '\u{a}'
                | '\u{b}'
                | '\u{c}'
                | '\u{d}'
                | ' '
                | '\u{a0}'
                | '\u{1680}'
                | '\u{2000}'
                ..='\u{200a}'
                    | '\u{2028}'
                    | '\u{2029}'
                    | '\u{202f}'
                    | '\u{205f}'
                    | '\u{3000}'
                    | '\u{feff}'
        )
    })
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `MOD_BITS[m.toLowerCase()]`: `None` when the lookup finds nothing (falsy), `Some(0)` for a
/// prototype property (a function or object: `|=` adds no bit).
fn mod_bit(m: &str) -> Option<i64> {
    let lower = m.to_lowercase();
    if let Some((_, bit)) = MOD_BITS.iter().find(|(k, _)| *k == lower) {
        return Some(*bit);
    }
    PROTOTYPE.contains(&lower.as_str()).then_some(0)
}

fn named_key(
    name: &str,
) -> Option<&'static (&'static str, &'static str, i64, Option<&'static str>)> {
    NAMED_KEYS.iter().find(|(k, ..)| *k == name)
}

fn alias(lower: &str) -> Option<&'static str> {
    KEY_ALIASES
        .iter()
        .find(|(k, _)| *k == lower)
        .map(|(_, v)| *v)
}

fn mac_command(key: &str) -> Option<Vec<String>> {
    MAC_COMMANDS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, c)| vec![(*c).to_string()])
}

/// `keyEvent(spec)`: "cmd+shift+a" -> the key event, `None` when the key isn't known.
pub fn key_event(spec: Option<&str>) -> Result<Option<KeyEvent>, Throws> {
    let spec = match spec.unwrap_or("") {
        " " => "Space",
        s => s,
    };
    let parts: Vec<&str> = js_trim(spec)
        .split('+')
        .map(js_trim)
        .filter(|p| !p.is_empty())
        .collect();
    let Some(raw) = parts.last().copied() else {
        return Ok(None);
    };
    let mut modifiers = 0i64;
    for m in &parts[..parts.len() - 1] {
        match mod_bit(m) {
            Some(bit) => modifiers |= bit,
            None => return Ok(None),
        }
    }
    // `NAMED_KEYS[raw] ? raw : KEY_ALIASES[raw.toLowerCase()]`
    if PROTOTYPE.contains(&raw) {
        return Err(Throws); // NAMED_KEYS[raw] is the prototype's: destructuring it throws
    }
    let lower = raw.to_lowercase();
    let named: Option<&str> = if named_key(raw).is_some() {
        Some(raw)
    } else if lower == "constructor" || lower == "__proto__" {
        return Err(Throws); // KEY_ALIASES[...] is the prototype's: NAMED_KEYS[it] is undefined
    } else {
        alias(&lower)
    };
    let (key, code, key_code, mut text);
    if let Some(n) = named {
        let (_, c, kc, t) = named_key(n).expect("a named key");
        code = (*c).to_string();
        key_code = *kc;
        text = t.map(String::from);
        key = if n == "Space" {
            " ".to_string()
        } else {
            n.to_string()
        };
    } else if utf16_len(raw) == 1 {
        let mut ch = raw.chars().next().expect("one character");
        if let Some((_, base)) = SHIFTED.iter().find(|(s, _)| *s == ch) {
            modifiers |= 8;
            ch = *base;
        } else if ch.is_ascii_uppercase() {
            modifiers |= 8;
            ch = ch.to_ascii_lowercase();
        }
        if ch.is_ascii_lowercase() {
            code = format!("Key{}", ch.to_ascii_uppercase());
            key_code = i64::from(ch.to_ascii_uppercase() as u8);
        } else if ch.is_ascii_digit() {
            code = format!("Digit{ch}");
            key_code = i64::from(ch as u8);
        } else if let Some((_, c, kc)) = PUNCT.iter().find(|(p, ..)| *p == ch) {
            code = (*c).to_string();
            key_code = *kc;
        } else if ch == ' ' {
            code = "Space".into();
            key_code = 32;
        } else {
            return Ok(None);
        }
        key = if modifiers & 8 != 0 && ch.is_ascii_lowercase() {
            ch.to_ascii_uppercase().to_string()
        } else {
            raw.to_string()
        };
        text = (modifiers & !8 == 0).then(|| raw.to_string());
    } else {
        return Ok(None);
    }
    if text.is_some() && modifiers & !8 != 0 {
        text = None;
    }
    let first = format!(
        "{modifiers}+{}",
        named.map_or_else(|| key.to_lowercase(), String::from)
    );
    let second = format!("{modifiers}+{}", named.unwrap_or(&key));
    let commands = mac_command(&first).or_else(|| mac_command(&second));
    Ok(Some(KeyEvent {
        key,
        code,
        key_code,
        text,
        modifiers,
        commands,
    }))
}

/// `entersText(spec)`: whether a key press puts text into the focused field (HAND-218): a key with
/// text that isn't Enter, or ⌘V. Where the extension's `keyEvent` throws, `true`: nothing is sent to
/// a secret field either way (the extension's command would fail).
pub fn enters_text(spec: Option<&str>) -> bool {
    match key_event(spec) {
        Err(Throws) => true,
        Ok(None) => false,
        Ok(Some(k)) => {
            (k.text.is_some() && k.key != "Enter")
                || k.commands.is_some_and(|c| c.iter().any(|c| c == "paste"))
        }
    }
}
