//! The `idna` package's functions the policy calls (`uts46_remap` with std3_rules=False, `alabel`, `ulabel`), and
//! Python's `punycode` codec, over idna's own tables (`tables.rs`). Only whether a call raises `IDNAError` and what
//! it returns matter to the policy; the error's words are never shown.

use crate::norm;
use crate::py;
use crate::tables as t;

/// `idna.IDNAError` (any subclass).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdnaError;

const MAX_INPUT: usize = 1024;

fn uts46_index(cp: u32) -> usize {
    if cp < 256 {
        cp as usize
    } else {
        t::UTS46.partition_point(|&(s, _, _)| s <= cp) - 1
    }
}

/// `idna.uts46_remap(domain, std3_rules=False, transitional=False)`.
pub fn uts46_remap(domain: &str) -> Result<String, IdnaError> {
    if py::char_len(domain) > MAX_INPUT {
        return Err(IdnaError);
    }
    if domain.is_ascii() {
        return Ok(domain.to_ascii_lowercase());
    }
    let mut out = String::with_capacity(domain.len());
    for c in domain.chars() {
        let (_, status, replacement) = t::UTS46[uts46_index(c as u32)];
        match status {
            b'V' | b'D' => out.push(c),
            b'M' => out.push_str(replacement),
            b'I' => {}
            _ => return Err(IdnaError),
        }
    }
    Ok(norm::nfc(&out))
}

fn joining_type(cp: u32) -> Option<char> {
    let tables: [&[(u32, u32)]; 5] = [
        t::JOINING_C,
        t::JOINING_D,
        t::JOINING_L,
        t::JOINING_R,
        t::JOINING_T,
    ];
    t::JOINING_ORDER
        .iter()
        .zip(tables)
        .find(|(_, table)| py::in_intranges(table, cp))
        .map(|(jt, _)| *jt)
}

fn combining_class(c: char) -> Result<u8, IdnaError> {
    let v = py::combining(c);
    if v == 0 && !py::has_name(c) {
        return Err(IdnaError); // ValueError, raised as IDNAError
    }
    Ok(v)
}

fn valid_contextj(label: &[char], pos: usize) -> Result<bool, IdnaError> {
    let cp = label[pos] as u32;
    if cp == 0x200C {
        if pos > 0 && combining_class(label[pos - 1])? == 9 {
            return Ok(true);
        }
        let mut ok = false;
        for i in (0..pos).rev() {
            match joining_type(label[i] as u32) {
                Some('T') => continue,
                Some('L') | Some('D') => {
                    ok = true;
                    break;
                }
                _ => break,
            }
        }
        if !ok {
            return Ok(false);
        }
        ok = false;
        for c in &label[pos + 1..] {
            match joining_type(*c as u32) {
                Some('T') => continue,
                Some('R') | Some('D') => {
                    ok = true;
                    break;
                }
                _ => break,
            }
        }
        return Ok(ok);
    }
    if cp == 0x200D {
        return Ok(pos > 0 && combining_class(label[pos - 1])? == 9);
    }
    Ok(false)
}

fn valid_contexto(label: &[char], pos: usize) -> bool {
    let cp = label[pos] as u32;
    let script = |c: char, table: &[(u32, u32)]| py::in_intranges(table, c as u32);
    match cp {
        0x00B7 => {
            0 < pos && pos < label.len() - 1 && label[pos - 1] == 'l' && label[pos + 1] == 'l'
        }
        0x0375 => {
            pos < label.len() - 1 && label.len() > 1 && script(label[pos + 1], t::SCRIPT_GREEK)
        }
        0x05F3 | 0x05F4 => pos > 0 && script(label[pos - 1], t::SCRIPT_HEBREW),
        0x30FB => label.iter().any(|&c| {
            c != '\u{30fb}'
                && (script(c, t::SCRIPT_HIRAGANA)
                    || script(c, t::SCRIPT_KATAKANA)
                    || script(c, t::SCRIPT_HAN))
        }),
        0x660..=0x669 => !label.iter().any(|&c| (0x6F0..=0x6F9).contains(&(c as u32))),
        0x6F0..=0x6F9 => !label.iter().any(|&c| (0x660..=0x669).contains(&(c as u32))),
        _ => false,
    }
}

fn check_bidi(label: &[char]) -> Result<(), IdnaError> {
    let mut bidi_label = false;
    for &c in label {
        let d = py::bidirectional(c);
        if d.is_empty() {
            return Err(IdnaError);
        }
        if matches!(d, "R" | "AL" | "AN") {
            bidi_label = true;
        }
    }
    if !bidi_label {
        return Ok(());
    }
    let rtl = match py::bidirectional(label[0]) {
        "R" | "AL" => true,
        "L" => false,
        _ => return Err(IdnaError),
    };
    let mut valid_ending = false;
    let mut number_type: Option<&str> = None;
    for &c in label {
        let d = py::bidirectional(c);
        if rtl {
            if !matches!(
                d,
                "R" | "AL" | "AN" | "EN" | "ES" | "CS" | "ET" | "ON" | "BN" | "NSM"
            ) {
                return Err(IdnaError);
            }
            if matches!(d, "R" | "AL" | "EN" | "AN") {
                valid_ending = true;
            } else if d != "NSM" {
                valid_ending = false;
            }
            if matches!(d, "AN" | "EN") {
                match number_type {
                    None => number_type = Some(d),
                    Some(n) if n != d => return Err(IdnaError),
                    _ => {}
                }
            }
        } else {
            if !matches!(d, "L" | "EN" | "ES" | "CS" | "ET" | "ON" | "BN" | "NSM") {
                return Err(IdnaError);
            }
            if matches!(d, "L" | "EN") {
                valid_ending = true;
            } else if d != "NSM" {
                valid_ending = false;
            }
        }
    }
    if valid_ending {
        Ok(())
    } else {
        Err(IdnaError)
    }
}

/// `idna.check_label` on a label (a str).
fn check_label(label: &[char]) -> Result<(), IdnaError> {
    if label.len() > MAX_INPUT || label.is_empty() || label.len() > 254 {
        return Err(IdnaError);
    }
    let s: String = label.iter().collect();
    if norm::nfc(&s) != s {
        return Err(IdnaError);
    }
    if label.len() >= 4 && label[2] == '-' && label[3] == '-' {
        return Err(IdnaError);
    }
    if label[0] == '-' || label[label.len() - 1] == '-' {
        return Err(IdnaError);
    }
    if py::is_mark(label[0]) {
        return Err(IdnaError);
    }
    for (pos, &c) in label.iter().enumerate() {
        let cp = c as u32;
        if py::in_intranges(t::IDNA_PVALID, cp) {
            continue;
        }
        if py::in_intranges(t::IDNA_CONTEXTJ, cp) {
            if !valid_contextj(label, pos)? {
                return Err(IdnaError);
            }
        } else if py::in_intranges(t::IDNA_CONTEXTO, cp) {
            if !valid_contexto(label, pos) {
                return Err(IdnaError);
            }
        } else {
            return Err(IdnaError);
        }
    }
    check_bidi(label)
}

/// `idna.alabel(label)`, as text.
pub fn alabel(label: &str) -> Result<String, IdnaError> {
    if py::char_len(label) > MAX_INPUT {
        return Err(IdnaError);
    }
    if label.is_ascii() {
        ulabel(label)?;
        if label.len() > 63 {
            return Err(IdnaError);
        }
        return Ok(label.to_string());
    }
    let chars: Vec<char> = label.chars().collect();
    check_label(&chars)?;
    let cps: Vec<u32> = chars.iter().map(|&c| c as u32).collect();
    let out = format!("xn--{}", punycode_encode(&cps));
    if out.len() > 63 {
        return Err(IdnaError);
    }
    Ok(out)
}

/// `idna.ulabel(label)` for a str label.
pub fn ulabel(label: &str) -> Result<String, IdnaError> {
    if py::char_len(label) > MAX_INPUT {
        return Err(IdnaError);
    }
    if !label.is_ascii() {
        let chars: Vec<char> = label.chars().collect();
        check_label(&chars)?;
        return Ok(label.to_string());
    }
    let lowered = label.to_ascii_lowercase();
    let Some(body) = lowered.strip_prefix("xn--") else {
        let chars: Vec<char> = lowered.chars().collect();
        check_label(&chars)?;
        return Ok(lowered);
    };
    if body.is_empty() || body.ends_with('-') {
        return Err(IdnaError);
    }
    let decoded = punycode_decode(body)?;
    if punycode_encode(&decoded) != body {
        return Err(IdnaError);
    }
    // a decoded surrogate is no Rust char: check_label refuses one (not PVALID) whatever comes before it
    let chars: Vec<char> = decoded
        .iter()
        .map(|&cp| char::from_u32(cp).ok_or(IdnaError))
        .collect::<Result<_, _>>()?;
    check_label(&chars)?;
    Ok(chars.into_iter().collect())
}

fn threshold(j: u64, bias: u64) -> u64 {
    let res = 36 * (j + 1);
    if res <= bias {
        1
    } else {
        (res - bias).clamp(1, 26)
    }
}

fn adapt(mut delta: u64, first: bool, numchars: u64) -> u64 {
    delta = if first { delta / 700 } else { delta / 2 };
    delta += delta / numchars;
    let mut divisions = 0;
    while delta > 455 {
        delta /= 35;
        divisions += 36;
    }
    divisions + (36 * delta / (delta + 38))
}

const DIGITS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// `text.encode("punycode")` (Lib/encodings/punycode.py).
pub fn punycode_encode(text: &[u32]) -> String {
    let base: String = text
        .iter()
        .filter(|&&c| c < 128)
        .map(|&c| c as u8 as char)
        .collect();
    let mut extended: Vec<u32> = text.iter().copied().filter(|&c| c >= 128).collect();
    extended.sort_unstable();
    extended.dedup();
    // insertion_unsort
    let mut deltas: Vec<u64> = Vec::new();
    let mut oldchar: u64 = 0x80;
    let mut oldindex: i64 = -1;
    for &c in &extended {
        let char = c as u64;
        let curlen = text.iter().filter(|&&x| (x as u64) < char).count() as u64;
        let mut delta = (curlen + 1) * (char - oldchar);
        let (mut index, mut pos): (i64, i64) = (-1, -1);
        loop {
            // selective_find
            let l = text.len() as i64;
            let mut found = false;
            loop {
                pos += 1;
                if pos == l {
                    index = -1;
                    pos = -1;
                    break;
                }
                let x = text[pos as usize];
                if x == c {
                    index += 1;
                    found = true;
                    break;
                } else if x < c {
                    index += 1;
                }
            }
            if !found {
                break;
            }
            delta = (delta as i64 + index - oldindex) as u64;
            deltas.push(delta - 1);
            oldindex = index;
            delta = 0;
        }
        oldchar = char;
    }
    // generate_integers
    let mut out = String::new();
    let mut bias = 72;
    for (points, &delta) in deltas.iter().enumerate() {
        let mut n = delta;
        let mut j = 0;
        loop {
            let tt = threshold(j, bias);
            if n < tt {
                out.push(DIGITS[n as usize] as char);
                break;
            }
            out.push(DIGITS[(tt + ((n - tt) % (36 - tt))) as usize] as char);
            n = (n - tt) / (36 - tt);
            j += 1;
        }
        bias = adapt(delta, points == 0, base.len() as u64 + points as u64 + 1);
    }
    if base.is_empty() {
        out
    } else {
        format!("{base}-{out}")
    }
}

/// `text.decode("punycode")` with errors="strict"; code points (a surrogate may come out).
pub fn punycode_decode(text: &str) -> Result<Vec<u32>, IdnaError> {
    let (base, extended) = match text.rfind('-') {
        None => ("", text.to_ascii_uppercase()),
        Some(pos) => (&text[..pos], text[pos + 1..].to_ascii_uppercase()),
    };
    let mut out: Vec<u32> = base.chars().map(|c| c as u32).collect();
    let ext = extended.as_bytes();
    let mut char: u64 = 0x80;
    let mut pos: i64 = -1;
    let mut bias: u64 = 72;
    let mut extpos = 0usize;
    while extpos < ext.len() {
        // decode_generalized_number
        let mut result: u64 = 0;
        let mut w: u64 = 1;
        let mut j = 0;
        let mut at = extpos;
        let delta = loop {
            let Some(&c) = ext.get(at) else {
                return Err(IdnaError);
            };
            at += 1;
            let digit = match c {
                b'A'..=b'Z' => (c - b'A') as u64,
                b'0'..=b'9' => (c - b'0') as u64 + 26,
                _ => return Err(IdnaError),
            };
            let tt = threshold(j, bias);
            result = result
                .checked_add(digit.checked_mul(w).ok_or(IdnaError)?)
                .ok_or(IdnaError)?;
            if digit < tt {
                break result;
            }
            w = w.checked_mul(36 - tt).ok_or(IdnaError)?;
            j += 1;
        };
        pos = pos.checked_add(delta as i64 + 1).ok_or(IdnaError)?;
        let n = out.len() as i64 + 1;
        char += (pos / n) as u64;
        if char > 0x10FFFF {
            return Err(IdnaError);
        }
        pos %= n;
        out.insert(pos as usize, char as u32);
        bias = adapt(delta, extpos == 0, out.len() as u64);
        extpos = at;
    }
    Ok(out)
}
