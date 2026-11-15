//! `unicodedata.normalize` ("NFC", "NFD", "NFKC") from Python 3.12's own data (`tables.rs`, Unicode 15.0): full
//! decomposition, canonical ordering by combining class, canonical composition (Hangul by its algorithm).

use crate::py;
use crate::tables as t;

const S_BASE: u32 = 0xAC00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11A7;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const N_COUNT: u32 = V_COUNT * T_COUNT;
const S_COUNT: u32 = L_COUNT * N_COUNT;

fn decompose(s: &str, compat: bool) -> Vec<char> {
    let table = if compat { t::NFKD } else { t::NFD };
    let mut out: Vec<char> = Vec::with_capacity(s.len());
    for c in s.chars() {
        let cp = c as u32;
        if (S_BASE..S_BASE + S_COUNT).contains(&cp) {
            let i = cp - S_BASE;
            out.push(char::from_u32(L_BASE + i / N_COUNT).unwrap_or(c));
            out.push(char::from_u32(V_BASE + (i % N_COUNT) / T_COUNT).unwrap_or(c));
            if !i.is_multiple_of(T_COUNT) {
                out.push(char::from_u32(T_BASE + i % T_COUNT).unwrap_or(c));
            }
            continue;
        }
        if c.is_ascii() {
            out.push(c);
            continue;
        }
        match table.binary_search_by_key(&cp, |&(k, _)| k) {
            Ok(k) => out.extend(table[k].1.chars()),
            Err(_) => out.push(c),
        }
    }
    // canonical ordering: a stable sort by combining class within each run of non-starters
    let mut i = 0;
    while i < out.len() {
        if py::combining(out[i]) == 0 {
            i += 1;
            continue;
        }
        let start = i;
        while i < out.len() && py::combining(out[i]) != 0 {
            i += 1;
        }
        out[start..i].sort_by_key(|&c| py::combining(c));
    }
    out
}

fn compose_pair(a: char, b: char) -> Option<char> {
    let (a, b) = (a as u32, b as u32);
    if (L_BASE..L_BASE + L_COUNT).contains(&a) && (V_BASE..V_BASE + V_COUNT).contains(&b) {
        return char::from_u32(S_BASE + ((a - L_BASE) * V_COUNT + (b - V_BASE)) * T_COUNT);
    }
    if (S_BASE..S_BASE + S_COUNT).contains(&a)
        && (a - S_BASE).is_multiple_of(T_COUNT)
        && (T_BASE + 1..T_BASE + T_COUNT).contains(&b)
    {
        return char::from_u32(a + (b - T_BASE));
    }
    t::COMPOSE
        .binary_search_by(|&(x, y, _)| (x, y).cmp(&(a, b)))
        .ok()
        .and_then(|i| char::from_u32(t::COMPOSE[i].2))
}

fn compose(chars: Vec<char>) -> String {
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut starter: Option<usize> = None;
    let mut last: Option<u8> = None; // the class of the last character kept after the starter
    for c in chars {
        let cc = py::combining(c);
        if let Some(s) = starter {
            let blocked = match last {
                None => false,
                Some(lc) => lc == 0 || lc >= cc,
            };
            if !blocked {
                if let Some(comp) = compose_pair(out[s], c) {
                    out[s] = comp;
                    continue;
                }
            }
        }
        if cc == 0 {
            starter = Some(out.len());
            last = None;
        } else {
            last = Some(cc);
        }
        out.push(c);
    }
    out.into_iter().collect()
}

/// `unicodedata.normalize("NFD", s)`.
pub fn nfd(s: &str) -> String {
    if s.is_ascii() {
        return s.to_string();
    }
    decompose(s, false).into_iter().collect()
}

/// `unicodedata.normalize("NFC", s)`.
pub fn nfc(s: &str) -> String {
    if s.is_ascii() {
        return s.to_string();
    }
    compose(decompose(s, false))
}

/// `unicodedata.normalize("NFKC", s)`.
pub fn nfkc(s: &str) -> String {
    if s.is_ascii() {
        return s.to_string();
    }
    compose(decompose(s, true))
}
