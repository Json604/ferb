//! Rust-only: personal identifiers that work like credentials (government, tax, bank and health numbers), masked
//! like secrets and named like formats (GATE-599). Not Python-recorded: measured by
//! rust/fixtures/sanitize-shapes (the `identifiers` corpus) and tests/pii.rs.
//!
//! Two ways a value is found, both structural (AGENTS.md rule 4: a format, never a judgment of intent):
//! - bare: a shape distinctive enough on its own, with the identifier's own check digit wherever it has one
//!   (Verhoeff, Luhn, mod 11, mod 97, mod 23, ISO 7064, the national tables), and the separators the issuer prints
//!   (an Aadhaar in 4-4-4 groups, a CPF as 000.000.000-00): never a bare run of digits;
//! - labelled: a value right after a label that names the identifier, in English and the country's own language
//!   (`Passport No: K1234567`, `खाता संख्या 0123…`), with the kind's own shape and check. A kind with no check
//!   digit or a weak shape (passports, bank accounts, driving licences, voter ids) is found only this way.
//!
//! Contact data (names, emails, phone numbers, addresses, dates of birth) and technical identifiers (IP, MAC,
//! IMEI, VIN) are not masked by this layer: FERB needs the first to act for its owner and reads the second in
//! ordinary work. The payment card rule ported from Python (formats.rs: 13 to 19 Luhn-valid digits, plain or in
//! groups) still masks, as a "payment card", whatever has that shape: a 15-digit IMEI, a South African id written
//! as 13 plain digits, an Emirates id. It is left as it is: it errs toward masking.
//! A masked identifier stays usable: a field that needs it is filled through the hidden owner input (browser and
//! Mac `secret`, gateway `ask_secret`), which the model never reads.
//!
//! decision: GATE-599 (agent), GATE-600 (agent: checked against python-stdnum).

use crate::formats::Hit;
use crate::rx::Span;

// ---------------------------------------------------------------- check digits

fn digits(v: &str) -> Vec<u32> {
    v.bytes()
        .filter(u8::is_ascii_digit)
        .map(|b| u32::from(b - b'0'))
        .collect()
}

/// Verhoeff (Aadhaar, its VID).
fn verhoeff(ds: &[u32]) -> bool {
    const D: [[u32; 10]; 10] = [
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
        [1, 2, 3, 4, 0, 6, 7, 8, 9, 5],
        [2, 3, 4, 0, 1, 7, 8, 9, 5, 6],
        [3, 4, 0, 1, 2, 8, 9, 5, 6, 7],
        [4, 0, 1, 2, 3, 9, 5, 6, 7, 8],
        [5, 9, 8, 7, 6, 0, 4, 3, 2, 1],
        [6, 5, 9, 8, 7, 1, 0, 4, 3, 2],
        [7, 6, 5, 9, 8, 2, 1, 0, 4, 3],
        [8, 7, 6, 5, 9, 3, 2, 1, 0, 4],
        [9, 8, 7, 6, 5, 4, 3, 2, 1, 0],
    ];
    const P: [[u32; 10]; 8] = [
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
        [1, 5, 7, 6, 2, 8, 3, 0, 9, 4],
        [5, 8, 0, 3, 7, 9, 6, 1, 4, 2],
        [8, 9, 1, 6, 0, 4, 3, 5, 2, 7],
        [9, 4, 5, 3, 1, 2, 6, 8, 7, 0],
        [4, 2, 8, 6, 5, 7, 3, 9, 0, 1],
        [2, 7, 9, 3, 8, 0, 6, 4, 1, 5],
        [7, 0, 4, 6, 9, 1, 3, 2, 5, 8],
    ];
    let mut c = 0;
    for (i, &d) in ds.iter().rev().enumerate() {
        c = D[c as usize][P[i % 8][d as usize] as usize];
    }
    c == 0
}

/// Luhn (SIN, Swedish personnummer, South African id, Emirates id, Saudi id, NPI-style numbers).
fn luhn(ds: &[u32]) -> bool {
    let sum: u32 = ds
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            if i % 2 == 1 {
                if d * 2 > 9 {
                    d * 2 - 9
                } else {
                    d * 2
                }
            } else {
                d
            }
        })
        .sum();
    !ds.is_empty() && sum.is_multiple_of(10)
}

/// Σ dᵢ·wᵢ over the first `w.len()` digits.
fn weighted(ds: &[u32], w: &[u32]) -> u32 {
    ds.iter().zip(w).map(|(d, w)| d * w).sum()
}

fn alnum_value(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some(u32::from(c - b'0')),
        b'A'..=b'Z' => Some(u32::from(c - b'A') + 10),
        _ => None,
    }
}

/// A two-digit month and day (YYMMDD or DDMMYY parts), loosely: 01-12 and 01-31.
fn month_day(m: u32, d: u32) -> bool {
    (1..=12).contains(&m) && (1..=31).contains(&d)
}

pub(crate) fn compact(v: &str) -> String {
    v.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_uppercase()
}

// ---------------------------------------------------------------- the kinds' validators (on the compact value)

fn aadhaar(v: &str) -> bool {
    let ds = digits(v);
    ds.len() == 12 && ds[0] >= 2 && verhoeff(&ds)
}

fn aadhaar_vid(v: &str) -> bool {
    // python-stdnum's in_.vid: the first digit is 2-9, as an Aadhaar's
    let ds = digits(v);
    ds.len() == 16 && ds[0] >= 2 && verhoeff(&ds)
}

fn pan(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() == 10
        && b[..5].iter().all(u8::is_ascii_uppercase)
        && b"PCHFATBLJGK".contains(&b[3]) // K: a krish trust (python-stdnum's in_.pan)
        && b[5..9].iter().all(u8::is_ascii_digit)
        && b[9].is_ascii_uppercase()
}

fn us_ssn(v: &str) -> bool {
    let ds = digits(v);
    if ds.len() != 9 || v.len() != 9 {
        return false;
    }
    let area = ds[0] * 100 + ds[1] * 10 + ds[2];
    let group = ds[3] * 10 + ds[4];
    let serial = weighted(&ds[5..], &[1000, 100, 10, 1]);
    area != 0 && area != 666 && area < 900 && group != 0 && serial != 0
}

fn us_itin(v: &str) -> bool {
    let ds = digits(v);
    if ds.len() != 9 || v.len() != 9 || ds[0] != 9 {
        return false;
    }
    let group = ds[3] * 10 + ds[4];
    matches!(group, 50..=65 | 70..=88 | 90..=92 | 94..=99)
}

fn us_mbi(v: &str) -> bool {
    // C A AN N A AN N A A N N; letters exclude S L O I B Z
    let b = v.as_bytes();
    let letter = |c: u8| c.is_ascii_uppercase() && !b"SLOIBZ".contains(&c);
    let an = |c: u8| letter(c) || c.is_ascii_digit();
    b.len() == 11
        && (b'1'..=b'9').contains(&b[0])
        && letter(b[1])
        && an(b[2])
        && b[3].is_ascii_digit()
        && letter(b[4])
        && an(b[5])
        && b[6].is_ascii_digit()
        && letter(b[7])
        && letter(b[8])
        && b[9].is_ascii_digit()
        && b[10].is_ascii_digit()
}

fn us_routing(v: &str) -> bool {
    let ds = digits(v);
    ds.len() == 9
        && v.len() == 9
        && weighted(&ds, &[3, 7, 1, 3, 7, 1, 3, 7, 1]).is_multiple_of(10)
        && ds.iter().any(|&d| d != 0)
}

fn uk_nino(v: &str) -> bool {
    let b = v.as_bytes();
    if b.len() != 9 || !b[2..8].iter().all(u8::is_ascii_digit) || !b"ABCD".contains(&b[8]) {
        return false;
    }
    let (p, q) = (b[0], b[1]);
    p.is_ascii_uppercase()
        && q.is_ascii_uppercase()
        && !b"DFIQUV".contains(&p)
        && !b"DFIOQUV".contains(&q)
        && !matches!(
            &b[..2],
            b"BG" | b"GB" | b"NK" | b"KN" | b"TN" | b"NT" | b"ZZ"
        )
}

fn uk_nhs(v: &str) -> bool {
    let ds = digits(v);
    if ds.len() != 10 || v.len() != 10 {
        return false;
    }
    let r = 11 - weighted(&ds, &[10, 9, 8, 7, 6, 5, 4, 3, 2]) % 11;
    let check = if r == 11 { 0 } else { r };
    check != 10 && check == ds[9]
}

fn uk_driving(v: &str) -> bool {
    // SURNA 7 5 3 1 1 6 SM 9 IJ: 5 letters (9 pads), the encoded date of birth, 2 initials, a digit, 2 letters
    let b = v.as_bytes();
    let l9 = |c: u8| c.is_ascii_uppercase() || c == b'9';
    b.len() == 16
        && b[..5].iter().all(|&c| l9(c))
        && b[5..11].iter().all(u8::is_ascii_digit)
        && matches!(b[6], b'0' | b'1' | b'5' | b'6')
        && b[11..13].iter().all(|&c| l9(c))
        && b[13].is_ascii_digit()
        && b[14..].iter().all(u8::is_ascii_uppercase)
}

fn ca_sin(v: &str) -> bool {
    let ds = digits(v);
    ds.len() == 9 && v.len() == 9 && ds[0] != 0 && ds[0] != 8 && luhn(&ds)
}

fn es_dni(v: &str) -> bool {
    const L: &[u8] = b"TRWAGMYFPDXBNJZSQVHLCKE";
    let b = v.as_bytes();
    b.len() == 9
        && b[..8].iter().all(u8::is_ascii_digit)
        && v[..8]
            .parse::<u32>()
            .is_ok_and(|n| L[(n % 23) as usize] == b[8])
}

fn es_nie(v: &str) -> bool {
    let b = v.as_bytes();
    let lead = match b.first() {
        Some(b'X') => '0',
        Some(b'Y') => '1',
        Some(b'Z') => '2',
        _ => return false,
    };
    b.len() == 9 && es_dni(&format!("{lead}{}", &v[1..]))
}

fn it_cf(v: &str) -> bool {
    const ODD: [u32; 26] = [
        1, 0, 5, 7, 9, 13, 15, 17, 19, 21, 2, 4, 18, 20, 11, 3, 6, 8, 12, 14, 16, 10, 22, 25, 24,
        23,
    ];
    const ODD_DIGIT: [u32; 10] = [1, 0, 5, 7, 9, 13, 15, 17, 19, 21];
    const OMOCODE: &[u8] = b"LMNPQRSTUV";
    let b = v.as_bytes();
    if b.len() != 16 {
        return false;
    }
    let num = |c: u8| c.is_ascii_digit() || OMOCODE.contains(&c);
    let shaped = b[..6].iter().all(u8::is_ascii_uppercase)
        && num(b[6])
        && num(b[7])
        && b"ABCDEHLMPRST".contains(&b[8])
        && num(b[9])
        && num(b[10])
        && b[11].is_ascii_uppercase()
        && b[12..15].iter().all(|&c| num(c))
        && b[15].is_ascii_uppercase();
    if !shaped {
        return false;
    }
    let mut sum = 0;
    for (i, &c) in b[..15].iter().enumerate() {
        let odd = i % 2 == 0; // 1-based odd positions
        sum += match (odd, c.is_ascii_digit()) {
            (true, true) => ODD_DIGIT[(c - b'0') as usize],
            (true, false) => ODD[(c - b'A') as usize],
            (false, true) => u32::from(c - b'0'),
            (false, false) => u32::from(c - b'A'),
        };
    }
    u32::from(b[15] - b'A') == sum % 26
}

fn fr_nir(v: &str) -> bool {
    // sex, year, month, department (2A/2B for Corsica), commune, order, key = 97 - n mod 97
    if v.len() != 15 {
        return false;
    }
    // 2A and 2B only as the department (characters 6-7), as python-stdnum's fr.nir
    let body = match &v[5..7] {
        "2A" => format!("{}19{}", &v[..5], &v[7..13]),
        "2B" => format!("{}18{}", &v[..5], &v[7..13]),
        _ => v[..13].to_string(),
    };
    if !body.bytes().all(|c| c.is_ascii_digit()) || !v[13..].bytes().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let sex = body.as_bytes()[0];
    let month: u32 = body[3..5].parse().unwrap_or(0);
    let (Ok(n), Ok(key)) = (body.parse::<u64>(), v[13..].parse::<u64>()) else {
        return false;
    };
    matches!(sex, b'1' | b'2' | b'3' | b'4' | b'7' | b'8')
        && ((1..=12).contains(&month) || (20..=42).contains(&month) || (50..=99).contains(&month))
        && key == 97 - n % 97
}

fn se_pnr(v: &str) -> bool {
    let ds = digits(v);
    let ds = if ds.len() == 12 { ds[2..].to_vec() } else { ds };
    ds.len() == 10 && month_day(ds[2] * 10 + ds[3], (ds[4] * 10 + ds[5]) % 60) && luhn(&ds)
}

fn fi_hetu(v: &str) -> bool {
    const CHECK: &[u8] = b"0123456789ABCDEFHJKLMNPRSTUVWXY";
    // the compact value lost a "-" or "+" century sign: put one back
    let signed;
    let v = if v.len() == 10 {
        signed = format!("{}-{}", &v[..6], &v[6..]);
        signed.as_str()
    } else {
        v
    };
    let b = v.as_bytes();
    if b.len() != 11 || !b"+-ABCDEFUVWXY".contains(&b[6]) {
        return false;
    }
    let num = format!("{}{}", &v[..6], &v[7..10]);
    let ds = digits(&num);
    ds.len() == 9
        && month_day(ds[2] * 10 + ds[3], ds[0] * 10 + ds[1])
        && num
            .parse::<u64>()
            .is_ok_and(|n| CHECK[(n % 31) as usize] == b[10])
}

fn be_nrn(v: &str) -> bool {
    let ds = digits(v);
    if ds.len() != 11 || v.len() != 11 {
        return false;
    }
    let first: u64 = v[..9].parse().unwrap_or(0);
    let check = u64::from(ds[9] * 10 + ds[10]);
    let month = ds[2] * 10 + ds[3];
    (month <= 12 || (20..=32).contains(&month) || (40..=52).contains(&month))
        && (97 - first % 97 == check || 97 - (2_000_000_000 + first) % 97 == check)
}

fn mod11_cpf(ds: &[u32]) -> bool {
    let check = |n: usize| {
        let s: u32 = ds[..n]
            .iter()
            .enumerate()
            .map(|(i, d)| d * (n as u32 + 1 - i as u32))
            .sum();
        let r = (s * 10) % 11;
        if r == 10 {
            0
        } else {
            r
        }
    };
    check(9) == ds[9] && check(10) == ds[10]
}

fn br_cpf(v: &str) -> bool {
    let ds = digits(v);
    ds.len() == 11 && v.len() == 11 && ds.iter().any(|&d| d != ds[0]) && mod11_cpf(&ds)
}

fn mx_curp(v: &str) -> bool {
    const CHARS: &[u8] = b"0123456789ABCDEFGHIJKLMN&OPQRSTUVWXYZ";
    let b = v.as_bytes();
    if b.len() != 18
        || !b[..4].iter().all(u8::is_ascii_uppercase)
        || !b[4..10].iter().all(u8::is_ascii_digit)
        || !b"HMX".contains(&b[10])
        || !b[11..16].iter().all(u8::is_ascii_uppercase)
        || !b[16].is_ascii_alphanumeric()
        || !b[17].is_ascii_digit()
    {
        return false;
    }
    let sum: u32 = b[..17]
        .iter()
        .enumerate()
        .map(|(i, c)| CHARS.iter().position(|x| x == c).unwrap_or(0) as u32 * (18 - i as u32))
        .sum();
    (10 - sum % 10) % 10 == u32::from(b[17] - b'0')
        && month_day(digits(&v[6..8])[0] * 10 + digits(&v[6..8])[1], 1)
}

fn ar_cuil(v: &str) -> bool {
    let ds = digits(v);
    if ds.len() != 11 || v.len() != 11 || !matches!(ds[0] * 10 + ds[1], 20 | 23 | 24 | 27) {
        return false;
    }
    let r = 11 - weighted(&ds, &[5, 4, 3, 2, 7, 6, 5, 4, 3, 2]) % 11;
    let check = match r {
        11 => 0,
        10 => 9,
        r => r,
    };
    check == ds[10]
}

fn cl_rut(v: &str) -> bool {
    let (body, dv) = v.split_at(v.len().saturating_sub(1));
    let ds = digits(body);
    if !(7..=8).contains(&ds.len()) || ds.len() != body.len() {
        return false;
    }
    let mut sum = 0;
    for (i, d) in ds.iter().rev().enumerate() {
        sum += d * [2, 3, 4, 5, 6, 7][i % 6];
    }
    let r = 11 - sum % 11;
    let want = match r {
        11 => "0".to_string(),
        10 => "K".to_string(),
        r => r.to_string(),
    };
    dv == want
}

fn cn_rid(v: &str) -> bool {
    const W: [u32; 17] = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
    const C: &[u8] = b"10X98765432";
    let b = v.as_bytes();
    if b.len() != 18
        || !b[..17].iter().all(u8::is_ascii_digit)
        || !(b[17].is_ascii_digit() || b[17] == b'X')
    {
        return false;
    }
    let ds = digits(&v[..17]);
    let year = weighted(&ds[6..10], &[1000, 100, 10, 1]);
    (1900..=2099).contains(&year)
        && month_day(ds[10] * 10 + ds[11], ds[12] * 10 + ds[13])
        && (1..=8).contains(&ds[0])
        && C[(weighted(&ds, &W) % 11) as usize] == b[17]
}

fn kr_rrn(v: &str) -> bool {
    // no checksum: numbers issued since October 2020 end in random digits (python-stdnum's kr.rrn still checks it)
    let ds = digits(v);
    ds.len() == 13
        && v.len() == 13
        && month_day(ds[2] * 10 + ds[3], ds[4] * 10 + ds[5])
        && (1..=8).contains(&ds[6])
}

fn sg_nric(v: &str) -> bool {
    let b = v.as_bytes();
    if b.len() != 9 || !b"STFG".contains(&b[0]) || !b[1..8].iter().all(u8::is_ascii_digit) {
        return false;
    }
    let mut s = weighted(&digits(&v[1..8]), &[2, 7, 6, 5, 4, 3, 2]);
    if matches!(b[0], b'T' | b'G') {
        s += 4;
    }
    let table: &[u8] = if matches!(b[0], b'S' | b'T') {
        b"JZIHGFEDCBA"
    } else {
        b"XWUTRQPNMLK"
    };
    table[(s % 11) as usize] == b[8]
}

fn hk_hkid(v: &str) -> bool {
    let b = v.as_bytes();
    let letters = b.iter().take_while(|c| c.is_ascii_uppercase()).count();
    if !(1..=2).contains(&letters)
        || b.len() != letters + 7
        || !b[letters..letters + 6].iter().all(u8::is_ascii_digit)
    {
        return false;
    }
    let mut vals: Vec<u32> = Vec::new();
    if letters == 1 {
        vals.push(36);
    }
    vals.extend(b[..letters].iter().map(|&c| u32::from(c - b'A') + 10));
    vals.extend(b[letters..letters + 6].iter().map(|&c| u32::from(c - b'0')));
    let s = weighted(&vals, &[9, 8, 7, 6, 5, 4, 3, 2]);
    let check = (11 - s % 11) % 11;
    let last = b[letters + 6];
    match check {
        10 => last == b'A',
        c => last == b'0' + c as u8,
    }
}

fn tw_id(v: &str) -> bool {
    const MAP: [u32; 26] = [
        10, 11, 12, 13, 14, 15, 16, 17, 34, 18, 19, 20, 21, 22, 35, 23, 24, 25, 26, 27, 28, 29, 32,
        30, 31, 33,
    ];
    let b = v.as_bytes();
    if b.len() != 10
        || !b[0].is_ascii_uppercase()
        || !matches!(b[1], b'1' | b'2' | b'8' | b'9') // 8, 9: the resident certificate's number (統一證號), same check
        || !b[1..].iter().all(u8::is_ascii_digit)
    {
        return false;
    }
    let n = MAP[(b[0] - b'A') as usize];
    let ds = digits(&v[1..]);
    let s = n / 10 + (n % 10) * 9 + weighted(&ds, &[8, 7, 6, 5, 4, 3, 2, 1, 1]);
    s.is_multiple_of(10)
}

fn my_mykad(v: &str) -> bool {
    let ds = digits(v);
    ds.len() == 12 && v.len() == 12 && month_day(ds[2] * 10 + ds[3], ds[4] * 10 + ds[5])
}

fn pk_cnic(v: &str) -> bool {
    let ds = digits(v);
    ds.len() == 13 && v.len() == 13 && (1..=7).contains(&ds[0])
}

fn lk_nic_old(v: &str) -> bool {
    let b = v.as_bytes();
    if b.len() != 10 || !b[..9].iter().all(u8::is_ascii_digit) || !b"VX".contains(&b[9]) {
        return false;
    }
    let day: u32 = v[2..5].parse().unwrap_or(0);
    (1..=366).contains(&day) || (501..=866).contains(&day)
}

/// Sri Lanka's 12-digit NIC (since 2016): the year of birth (1900-2099), the day of the year (001-366, women
/// 501-866), a serial and a check digit whose rule is not published: shape only, label-only.
fn lk_nic_new(v: &str) -> bool {
    let ds = digits(v);
    if ds.len() != 12 || v.len() != 12 {
        return false;
    }
    let year = weighted(&ds[..4], &[1000, 100, 10, 1]);
    let day = weighted(&ds[4..7], &[100, 10, 1]);
    (1900..=2099).contains(&year) && ((1..=366).contains(&day) || (501..=866).contains(&day))
}

/// Singapore's M-series FIN (issued since 2022): M, seven digits and a check letter. Its check-letter table is not
/// published, and none is invented here: the shape only, and only after an NRIC/FIN label.
fn sg_fin_m(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() == 9
        && b[0] == b'M'
        && b[1..8].iter().all(u8::is_ascii_digit)
        && b[8].is_ascii_uppercase()
}

/// Norway's fødselsnummer (python-stdnum's no.fodselsnummer): a date of birth DDMMYY (a D-number adds 40 to the
/// day, an H-number 40 to the month), three individual digits that give the century, and two mod-11 check digits.
/// FH-numbers (day 80 and above) carry no date and fail, as in stdnum. stdnum also refuses a date in the future;
/// the redactor has no clock, so that one rule is not applied (a number dated in the future is masked).
fn no_fnr(v: &str) -> bool {
    let ds = digits(v);
    if ds.len() != 11 || v.len() != 11 {
        return false;
    }
    let c1 = (11 - weighted(&ds, &[3, 7, 6, 1, 8, 9, 4, 5, 2]) % 11) % 11;
    let c2 = (11 - weighted(&ds, &[5, 4, 3, 2, 7, 6, 5, 4, 3, 2]) % 11) % 11;
    if c1 != ds[9] || c2 != ds[10] {
        return false;
    }
    let (mut day, mut month) = (ds[0] * 10 + ds[1], ds[2] * 10 + ds[3]);
    let (yy, ind) = (ds[4] * 10 + ds[5], weighted(&ds[6..9], &[100, 10, 1]));
    if day >= 80 {
        return false;
    }
    if day > 40 {
        day -= 40;
    }
    if month > 40 {
        month -= 40;
    }
    let year = if ind < 500 {
        1900 + yy
    } else if ind < 750 && yy >= 54 {
        1800 + yy
    } else if yy < 40 {
        2000 + yy
    } else if ind >= 900 {
        1900 + yy
    } else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day)
}

/// Mexico's CLABE (18 digits: bank, branch, account, check digit): the check is 10 minus the sum of each digit
/// times 3, 7, 1 (each product mod 10), mod 10 (Banxico's rule; python-stdnum 2.2 has no CLABE module).
fn mx_clabe(v: &str) -> bool {
    let ds = digits(v);
    if ds.len() != 18 || v.len() != 18 {
        return false;
    }
    let s: u32 = ds[..17]
        .iter()
        .zip([3, 7, 1].iter().cycle())
        .map(|(d, w)| d * w % 10)
        .sum();
    (10 - s % 10) % 10 == ds[17]
}

fn ae_eid(v: &str) -> bool {
    let ds = digits(v);
    ds.len() == 15 && v.len() == 15 && v.starts_with("784") && luhn(&ds)
}

fn za_id(v: &str) -> bool {
    let ds = digits(v);
    ds.len() == 13
        && v.len() == 13
        && month_day(ds[2] * 10 + ds[3], ds[4] * 10 + ds[5])
        && ds[10] <= 2
        && luhn(&ds)
}

fn ru_snils(v: &str) -> bool {
    let ds = digits(v);
    if ds.len() != 11 || v.len() != 11 {
        return false;
    }
    let s = weighted(&ds, &[9, 8, 7, 6, 5, 4, 3, 2, 1]);
    let check = match s {
        s if s < 100 => s,
        100 | 101 => 0,
        s => match s % 101 {
            100 => 0,
            r => r,
        },
    };
    check == ds[9] * 10 + ds[10]
}

fn iban(v: &str) -> bool {
    // ISO 13616: the country's length, then mod 97 of the rearranged number is 1
    const LENGTHS: &[(&str, usize)] = &[
        ("AD", 24),
        ("AE", 23),
        ("AL", 28),
        ("AT", 20),
        ("AZ", 28),
        ("BA", 20),
        ("BE", 16),
        ("BG", 22),
        ("BH", 22),
        ("BI", 27),
        ("BR", 29),
        ("BY", 28),
        ("CH", 21),
        ("CR", 22),
        ("CY", 28),
        ("CZ", 24),
        ("DE", 22),
        ("DJ", 27),
        ("DK", 18),
        ("DO", 28),
        ("EE", 20),
        ("EG", 29),
        ("ES", 24),
        ("FI", 18),
        ("FK", 18),
        ("FO", 18),
        ("FR", 27),
        ("GB", 22),
        ("GE", 22),
        ("GI", 23),
        ("GL", 18),
        ("GR", 27),
        ("GT", 28),
        ("HN", 28),
        ("HR", 21),
        ("HU", 28),
        ("IE", 22),
        ("IL", 23),
        ("IQ", 23),
        ("IS", 26),
        ("IT", 27),
        ("JO", 30),
        ("KW", 30),
        ("KZ", 20),
        ("LB", 28),
        ("LC", 32),
        ("LI", 21),
        ("LT", 20),
        ("LU", 20),
        ("LV", 21),
        ("LY", 25),
        ("MC", 27),
        ("MD", 24),
        ("ME", 22),
        ("MK", 19),
        ("MN", 20),
        ("MR", 27),
        ("MT", 31),
        ("MU", 30),
        ("NI", 28),
        ("NL", 18),
        ("NO", 15),
        ("OM", 23),
        ("PK", 24),
        ("PL", 28),
        ("PS", 29),
        ("PT", 25),
        ("QA", 29),
        ("RO", 24),
        ("RS", 22),
        ("RU", 33),
        ("SA", 24),
        ("SC", 31),
        ("SD", 18),
        ("SE", 24),
        ("SI", 19),
        ("SK", 24),
        ("SM", 27),
        ("SO", 23),
        ("ST", 25),
        ("SV", 28),
        ("TL", 23),
        ("TN", 24),
        ("TR", 26),
        ("UA", 29),
        ("VA", 22),
        ("VG", 24),
        ("XK", 20),
        ("YE", 30),
    ];
    let b = v.as_bytes();
    if b.len() < 15
        || !b[..2].iter().all(u8::is_ascii_uppercase)
        || !b[2..4].iter().all(u8::is_ascii_digit)
    {
        return false;
    }
    if LENGTHS
        .iter()
        .find(|(c, _)| c.as_bytes() == &b[..2])
        .map(|&(_, n)| n)
        != Some(b.len())
    {
        return false;
    }
    let mut r: u32 = 0;
    for &c in b[4..].iter().chain(&b[..4]) {
        let Some(x) = alnum_value(c) else {
            return false;
        };
        r = if x >= 10 {
            (r * 100 + x) % 97
        } else {
            (r * 10 + x) % 97
        };
    }
    r == 1
}

fn de_steuer_id(v: &str) -> bool {
    // ISO 7064 MOD 11,10; the first digit is not 0; in the first ten digits exactly one digit appears two or three
    // times and every other at most once (python-stdnum's de.idnr)
    let ds = digits(v);
    if ds.len() != 11 || v.len() != 11 || ds[0] == 0 {
        return false;
    }
    let mut count = [0u32; 10];
    for &d in &ds[..10] {
        count[d as usize] += 1;
    }
    let repeated: Vec<u32> = count.iter().copied().filter(|&c| c > 1).collect();
    if repeated.len() != 1 || !(2..=3).contains(&repeated[0]) {
        return false;
    }
    let mut product = 10;
    for &d in &ds[..10] {
        let mut sum = (d + product) % 10;
        if sum == 0 {
            sum = 10;
        }
        product = (sum * 2) % 11;
    }
    let check = (11 - product) % 10;
    check == ds[10]
}

fn de_ausweis(v: &str) -> bool {
    // 9 characters (digits and C F G H J K L M N P R T V W X Y Z) and a check digit, weights 7 3 1; or the nine
    // alone, as the card's front prints them (the check digit is only in the machine-readable zone): label-only
    let b = v.as_bytes();
    let serial = |c: &u8| c.is_ascii_digit() || b"CFGHJKLMNPRTVWXYZ".contains(c);
    if b.len() == 9 {
        return b.iter().all(serial) && b.iter().any(u8::is_ascii_digit);
    }
    if b.len() != 10
        || !b[9].is_ascii_digit()
        || !b[..9]
            .iter()
            .all(|c| c.is_ascii_digit() || b"CFGHJKLMNPRTVWXYZ".contains(c))
    {
        return false;
    }
    let s: u32 = b[..9]
        .iter()
        .enumerate()
        .map(|(i, &c)| alnum_value(c).unwrap_or(0) * [7, 3, 1][i % 3])
        .sum();
    s % 10 == u32::from(b[9] - b'0') && b[..9].iter().any(u8::is_ascii_digit)
}

fn nl_bsn(v: &str) -> bool {
    let ds = digits(v);
    if ds.len() != 9 || v.len() != 9 {
        return false;
    }
    let s = weighted(&ds[..8], &[9, 8, 7, 6, 5, 4, 3, 2]) as i64 - i64::from(ds[8]);
    s > 0 && s % 11 == 0
}

fn pl_pesel(v: &str) -> bool {
    let ds = digits(v);
    if ds.len() != 11 || v.len() != 11 {
        return false;
    }
    let s = weighted(&ds, &[1, 3, 7, 9, 1, 3, 7, 9, 1, 3]);
    (10 - s % 10) % 10 == ds[10] && month_day((ds[2] * 10 + ds[3]) % 20, ds[4] * 10 + ds[5])
}

fn dk_cpr(v: &str) -> bool {
    let ds = digits(v);
    ds.len() == 10 && v.len() == 10 && month_day(ds[2] * 10 + ds[3], ds[0] * 10 + ds[1])
}

fn pt_nif(v: &str) -> bool {
    // any first digit but 0, as python-stdnum's pt.nif (4 and 7 are issued too)
    let ds = digits(v);
    if ds.len() != 9 || v.len() != 9 || ds[0] == 0 {
        return false;
    }
    let r = 11 - weighted(&ds, &[9, 8, 7, 6, 5, 4, 3, 2]) % 11;
    (if r >= 10 { 0 } else { r }) == ds[8]
}

fn ie_ppsn(v: &str) -> bool {
    // python-stdnum's ie.pps: seven digits, the check letter, and an optional ninth letter; the 2013 format's A, B
    // or H counts in the check, the old format's W, T or X is ignored
    const L: &[u8] = b"WABCDEFGHIJKLMNOPQRSTUV";
    let b = v.as_bytes();
    if !(8..=9).contains(&b.len())
        || !b[..7].iter().all(u8::is_ascii_digit)
        || !L.contains(&b[7])
        || b.get(8).is_some_and(|c| !b"ABHWTX".contains(c))
    {
        return false;
    }
    let mut s = weighted(&digits(&v[..7]), &[8, 7, 6, 5, 4, 3, 2]);
    if let Some(&c @ (b'A' | b'B' | b'H')) = b.get(8) {
        s += u32::from(c - b'A' + 1) * 9;
    }
    L[(s % 23) as usize] == b[7]
}

fn au_tfn(v: &str) -> bool {
    let ds = digits(v);
    let w: &[u32] = match ds.len() {
        9 => &[1, 4, 3, 7, 5, 8, 6, 9, 10],
        8 => &[10, 7, 8, 4, 6, 3, 5, 1],
        _ => return false,
    };
    ds.len() == v.len() && weighted(&ds, w).is_multiple_of(11)
}

fn au_medicare(v: &str) -> bool {
    let ds = digits(v);
    (10..=11).contains(&ds.len())
        && ds.len() == v.len()
        && (2..=6).contains(&ds[0])
        && weighted(&ds, &[1, 3, 7, 9, 1, 3, 7, 9]) % 10 == ds[8]
}

fn jp_my_number(v: &str) -> bool {
    let ds = digits(v);
    if ds.len() != 12 || v.len() != 12 {
        return false;
    }
    let s: u32 = (1..=11)
        .map(|n| ds[11 - n] * if n <= 6 { n as u32 + 1 } else { n as u32 - 5 })
        .sum();
    let r = s % 11;
    (if r <= 1 { 0 } else { 11 - r }) == ds[11]
}

fn id_nik(v: &str) -> bool {
    let ds = digits(v);
    ds.len() == 16 && v.len() == 16 && month_day(ds[8] * 10 + ds[9], (ds[6] * 10 + ds[7]) % 40)
}

fn tr_tckn(v: &str) -> bool {
    let ds = digits(v);
    if ds.len() != 11 || v.len() != 11 || ds[0] == 0 {
        return false;
    }
    let odd = ds[0] + ds[2] + ds[4] + ds[6] + ds[8];
    let even = ds[1] + ds[3] + ds[5] + ds[7];
    ((odd * 7 + 100 - even) % 10) == ds[9] && ds[..10].iter().sum::<u32>() % 10 == ds[10]
}

fn il_id(v: &str) -> bool {
    // a shorter number is the same number with leading zeros (python-stdnum's il.idnr pads to nine): 3933742-3 is
    // 039337423; label-only, so five to eight digits are read only after the label
    let mut ds = digits(v);
    if !(5..=9).contains(&ds.len()) || v.len() != ds.len() || ds.iter().all(|&d| d == 0) {
        return false;
    }
    while ds.len() < 9 {
        ds.insert(0, 0);
    }
    let s: u32 = ds
        .iter()
        .enumerate()
        .map(|(i, &d)| {
            let x = d * if i % 2 == 0 { 1 } else { 2 };
            x / 10 + x % 10
        })
        .sum();
    s.is_multiple_of(10)
}

fn sa_id(v: &str) -> bool {
    let ds = digits(v);
    ds.len() == 10 && v.len() == 10 && matches!(ds[0], 1 | 2) && luhn(&ds)
}

fn ru_inn(v: &str) -> bool {
    let ds = digits(v);
    if ds.len() != 12 || v.len() != 12 {
        return false;
    }
    let c1 = weighted(&ds, &[7, 2, 4, 10, 3, 5, 9, 4, 6, 8]) % 11 % 10;
    let c2 = weighted(&ds, &[3, 7, 2, 4, 10, 3, 5, 9, 4, 6, 8]) % 11 % 10;
    c1 == ds[10] && c2 == ds[11]
}

fn mx_rfc(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() == 13
        && b[..4].iter().all(|c| c.is_ascii_uppercase() || *c == b'&')
        && b[4..10].iter().all(u8::is_ascii_digit)
        && b[10..].iter().all(u8::is_ascii_alphanumeric)
        && month_day(
            u32::from(b[6] - b'0') * 10 + u32::from(b[7] - b'0'),
            u32::from(b[8] - b'0') * 10 + u32::from(b[9] - b'0'),
        )
}

fn in_epic(v: &str) -> bool {
    // three letters and seven digits, Luhn over the digits (python-stdnum's in_.epic)
    let b = v.as_bytes();
    b.len() == 10
        && b[..3].iter().all(u8::is_ascii_uppercase)
        && b[3..].iter().all(u8::is_ascii_digit)
        && luhn(&digits(&v[3..]))
}

fn ke_kra(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() == 11
        && matches!(b[0], b'A' | b'P')
        && b[1..10].iter().all(u8::is_ascii_digit)
        && b[10].is_ascii_uppercase()
}

/// A document number after its label: 6-16 letters and digits, at least five of them digits (passports, driving
/// licences, health cards: no check digit, so only after a label).
fn document(v: &str) -> bool {
    (6..=16).contains(&v.len()) && v.bytes().filter(u8::is_ascii_digit).count() >= 5
}

/// A bank account number after its label: 6-20 digits (letters allowed only as the first two characters).
fn account(v: &str) -> bool {
    let ds = v.bytes().filter(u8::is_ascii_digit).count();
    (6..=20).contains(&ds) && v.bytes().skip(2).all(|c| c.is_ascii_digit()) && ds + 2 >= v.len()
}

fn uk_sort_code(v: &str) -> bool {
    v.len() == 6 && v.bytes().all(|c| c.is_ascii_digit())
}

fn plain_digits(n: &'static [usize]) -> impl Fn(&str) -> bool {
    move |v: &str| n.contains(&v.len()) && v.bytes().all(|c| c.is_ascii_digit())
}

// ---------------------------------------------------------------- the table

/// How a kind's value may be written when it stands alone (bare). `groups`: digit or letter groups and the
/// separators between them, as the issuer prints them; a bare value must match one of these layouts exactly.
pub(crate) struct Kind {
    pub name: &'static str,
    pub check: fn(&str) -> bool,
    /// Layouts a bare value may have: each a list of group widths with the separators allowed between groups
    /// ("" means none). Empty: never bare.
    pub bare: &'static [&'static [(&'static str, usize)]],
    /// Labels that name it (lowercase ASCII words, or the script's own words written as they appear).
    pub labels: &'static [&'static str],
}

// Separators: S a space, D "-", P ".", any of them when the issuer prints several.
const SP: &str = " ";
const DASH: &str = "-";
const SD: &str = " -";
const DOT: &str = ".";

pub(crate) static KINDS: &[Kind] = &[
    Kind { name: "Aadhaar number", check: aadhaar, bare: &[&[("", 4), (SD, 4), (SD, 4)]],
        labels: &["aadhaar", "aadhar", "adhaar", "adhar", "uid", "uidai", "\u{906}\u{927}\u{93e}\u{930}", "aadhaar no", "aadhaar number"] },
    Kind { name: "Aadhaar VID", check: aadhaar_vid, bare: &[],
        labels: &["vid", "virtual id", "aadhaar vid"] },
    Kind { name: "PAN", check: pan, bare: &[&[("", 10)]],
        labels: &["pan", "pan no", "pan number", "permanent account number", "\u{92a}\u{948}\u{928}"] },
    Kind { name: "voter ID", check: in_epic, bare: &[],
        labels: &["epic", "epic no", "voter id", "voter card", "elector's photo identity card", "\u{92e}\u{924}\u{926}\u{93e}\u{924}\u{93e} \u{92a}\u{939}\u{91a}\u{93e}\u{928} \u{92a}\u{924}\u{94d}\u{930}"] },
    Kind { name: "Social Security number", check: us_ssn, bare: &[&[("", 3), (DASH, 2), (DASH, 4)]],
        labels: &["ssn", "social security", "social security number", "social security no"] },
    Kind { name: "ITIN", check: us_itin, bare: &[&[("", 3), (DASH, 2), (DASH, 4)]],
        labels: &["itin", "individual taxpayer identification number"] },
    Kind { name: "Medicare number", check: us_mbi, bare: &[&[("", 11)], &[("", 4), (DASH, 3), (DASH, 4)]],
        labels: &["mbi", "medicare beneficiary identifier", "medicare number"] },
    Kind { name: "routing number", check: us_routing, bare: &[],
        labels: &["routing", "routing number", "routing no", "aba", "aba number", "rtn", "routing transit number"] },
    Kind { name: "National Insurance number", check: uk_nino,
        bare: &[&[("", 9)], &[("", 2), (SP, 2), (SP, 2), (SP, 2), (SP, 1)]],
        labels: &["nino", "national insurance", "national insurance number", "ni number", "ni no"] },
    Kind { name: "NHS number", check: uk_nhs, bare: &[],
        labels: &["nhs", "nhs number", "nhs no"] },
    Kind { name: "driving licence number", check: uk_driving, bare: &[&[("", 16)]], labels: &[] },
    Kind { name: "Social Insurance Number", check: ca_sin, bare: &[],
        labels: &["sin", "social insurance number", "social insurance", "numéro d'assurance sociale",
            "num\u{e9}ro d\u{2019}assurance sociale", "nas"] },
    Kind { name: "DNI", check: es_dni, bare: &[&[("", 8), ("-", 1)], &[("", 9)]],
        labels: &["dni", "nif", "documento nacional de identidad", "d.n.i", "n.i.f"] },
    Kind { name: "NIE", check: es_nie, bare: &[&[("", 9)], &[("", 1), (DASH, 7), (DASH, 1)]],
        labels: &["nie", "número de identidad de extranjero", "nif", "n.i.f", "n.i.e"] },
    Kind { name: "codice fiscale", check: it_cf, bare: &[&[("", 16)]],
        labels: &["codice fiscale", "cf"] },
    Kind { name: "NIR", check: fr_nir, bare: &[&[("", 15)], &[("", 1), (SP, 2), (SP, 2), (SP, 2), (SP, 3), (SP, 3), (SP, 2)]],
        labels: &["nir", "insee", "numéro de sécurité sociale", "numero de securite sociale", "sécurité sociale", "securite sociale"] },
    Kind { name: "personnummer", check: se_pnr, bare: &[&[("", 6), ("-+", 4)], &[("", 8), ("-", 4)]],
        labels: &["personnummer", "personal identity number", "personnr", "personbeteckning"] },
    Kind { name: "henkilötunnus", check: fi_hetu, bare: &[&[("", 11)], &[("", 6), ("-+", 4)]],
        labels: &["henkilötunnus", "hetu", "personal identity code", "sotu", "personbeteckning"] },
    Kind { name: "national register number", check: be_nrn, bare: &[&[("", 2), (DOT, 2), (DOT, 2), (DASH, 3), (DOT, 2)]],
        labels: &["rijksregisternummer", "numéro de registre national", "national register number", "insz", "niss"] },
    Kind { name: "CPF", check: br_cpf, bare: &[&[("", 3), (DOT, 3), (DOT, 3), (DASH, 2)]],
        labels: &["cpf"] },
    Kind { name: "CURP", check: mx_curp, bare: &[&[("", 18)]], labels: &["curp"] },
    Kind { name: "RFC", check: mx_rfc, bare: &[], labels: &["rfc"] },
    Kind { name: "CUIL", check: ar_cuil, bare: &[&[("", 2), (DASH, 8), (DASH, 1)]],
        labels: &["cuil", "cuit"] },
    Kind { name: "RUT", check: cl_rut, bare: &[&[("", 2), (DOT, 3), (DOT, 3), (DASH, 1)], &[("", 1), (DOT, 3), (DOT, 3), (DASH, 1)]],
        labels: &["rut", "run", "r.u.t"] },
    Kind { name: "resident identity number", check: cn_rid, bare: &[&[("", 18)]],
        labels: &["\u{8eab}\u{4efd}\u{8bc1}", "\u{8eab}\u{4efd}\u{8bc1}\u{53f7}", "\u{516c}\u{6c11}\u{8eab}\u{4efd}\u{53f7}\u{7801}",
            "\u{8eab}\u{4efd}\u{53f7}\u{7801}", "resident identity card", "resident id"] },
    Kind { name: "resident registration number", check: kr_rrn, bare: &[&[("", 6), (DASH, 7)]],
        labels: &["\u{c8fc}\u{bbfc}\u{b4f1}\u{b85d}\u{bc88}\u{d638}", "\u{c8fc}\u{bbfc}\u{bc88}\u{d638}", "rrn", "resident registration number"] },
    Kind { name: "NRIC", check: sg_nric, bare: &[&[("", 9)]], labels: &["nric", "fin"] },
    Kind { name: "NRIC", check: sg_fin_m, bare: &[], labels: &["nric", "fin"] },
    Kind { name: "HKID", check: hk_hkid, bare: &[],
        labels: &["hkid", "hong kong identity card", "\u{8eab}\u{4efd}\u{8b49}"] },
    Kind { name: "Taiwan ID", check: tw_id, bare: &[],
        labels: &["\u{8eab}\u{5206}\u{8b49}", "\u{8eab}\u{5206}\u{8b49}\u{5b57}\u{865f}", "\u{7d71}\u{4e00}\u{8b49}\u{865f}", "national id"] },
    Kind { name: "MyKad number", check: my_mykad, bare: &[&[("", 6), (DASH, 2), (DASH, 4)]],
        labels: &["mykad", "no. k/p", "ic no", "ic number", "nric", "kad pengenalan"] },
    Kind { name: "CNIC", check: pk_cnic, bare: &[&[("", 5), (DASH, 7), (DASH, 1)]],
        labels: &["cnic", "nicop"] },
    Kind { name: "NIC", check: lk_nic_old, bare: &[&[("", 10)]], labels: &["nic"] },
    Kind { name: "NIC", check: lk_nic_new, bare: &[], labels: &["nic"] },
    Kind { name: "Emirates ID", check: ae_eid, bare: &[&[("", 3), (DASH, 4), (DASH, 7), (DASH, 1)]],
        labels: &["emirates id", "eid"] },
    Kind { name: "South African ID", check: za_id, bare: &[], labels: &["id number", "identity number", "id no", "sa id"] },
    Kind { name: "SNILS", check: ru_snils, bare: &[&[("", 3), (DASH, 3), (DASH, 3), (SP, 2)]],
        labels: &["snils", "\u{441}\u{43d}\u{438}\u{43b}\u{441}", "\u{421}\u{41d}\u{418}\u{41b}\u{421}"] },
    Kind { name: "INN", check: ru_inn, bare: &[], labels: &["inn", "\u{438}\u{43d}\u{43d}", "\u{418}\u{41d}\u{41d}"] },
    Kind { name: "IBAN", check: iban, bare: &[], labels: &["iban"] },
    Kind { name: "Steuer-ID", check: de_steuer_id, bare: &[],
        labels: &["steuer-id", "steuerid", "steueridentifikationsnummer", "steuerliche identifikationsnummer", "idnr", "tax id", "tin",
            "identifikationsnummer", "steuer-idnr"] },
    Kind { name: "Personalausweis number", check: de_ausweis, bare: &[],
        labels: &["personalausweis", "ausweisnummer", "personalausweisnummer", "id card number", "identity card number",
            "ausweis-nr"] },
    Kind { name: "BSN", check: nl_bsn, bare: &[], labels: &["bsn", "burgerservicenummer", "sofinummer", "sofi-nummer"] },
    Kind { name: "PESEL", check: pl_pesel, bare: &[], labels: &["pesel"] },
    Kind { name: "CPR number", check: dk_cpr, bare: &[], labels: &["cpr", "cpr-nr", "cpr nr", "cpr-nummer", "personnummer"] },
    Kind { name: "fødselsnummer", check: no_fnr, bare: &[],
        labels: &["f\u{f8}dselsnummer", "fodselsnummer", "personnummer"] },
    Kind { name: "NIF", check: pt_nif, bare: &[], labels: &["nif", "contribuinte", "número de contribuinte", "numero de contribuinte", "n.i.f",
            "n\u{fa}mero de identifica\u{e7}\u{e3}o fiscal", "numero de identificacao fiscal"] },
    Kind { name: "PPS number", check: ie_ppsn, bare: &[], labels: &["pps", "ppsn", "pps number", "pps no", "personal public service number"] },
    Kind { name: "tax file number", check: au_tfn, bare: &[], labels: &["tfn", "tax file number"] },
    Kind { name: "Medicare number", check: au_medicare, bare: &[], labels: &["medicare", "medicare card", "medicare no"] },
    Kind { name: "My Number", check: jp_my_number, bare: &[],
        labels: &["my number", "\u{30de}\u{30a4}\u{30ca}\u{30f3}\u{30d0}\u{30fc}", "\u{500b}\u{4eba}\u{756a}\u{53f7}"] },
    Kind { name: "NIK", check: id_nik, bare: &[], labels: &["nik", "nomor induk kependudukan", "ktp"] },
    Kind { name: "TC kimlik number", check: tr_tckn, bare: &[],
        labels: &["tckn", "tc kimlik", "t.c. kimlik", "kimlik no", "tc kimlik no", "kimlik numarası"] },
    Kind { name: "Israeli ID", check: il_id, bare: &[],
        labels: &["teudat zehut", "\u{5ea}.\u{5d6}", "\u{5ea}\"\u{5d6}", "\u{5de}\u{5e1}\u{5e4}\u{5e8} \u{5d6}\u{5d4}\u{5d5}\u{5ea}"] },
    Kind { name: "Saudi national ID", check: sa_id, bare: &[],
        labels: &["iqama", "\u{647}\u{648}\u{64a}\u{629}", "\u{625}\u{642}\u{627}\u{645}\u{629}", "saudi id", "national id number"] },
    Kind { name: "KRA PIN", check: ke_kra, bare: &[], labels: &["kra pin"] },
    Kind { name: "NIN", check: plain_digits_11, bare: &[], labels: &["nin", "national identification number"] },
    Kind { name: "sort code", check: uk_sort_code, bare: &[], labels: &["sort code"] },
    Kind { name: "CLABE", check: mx_clabe, bare: &[], labels: &["clabe"] },
    Kind { name: "passport number", check: document, bare: &[],
        labels: &["passport", "passport no", "passport number", "pasaporte", "passeport", "reisepass", "passaporto",
            "passaporte", "paspoort", "\u{92a}\u{93e}\u{938}\u{92a}\u{94b}\u{930}\u{94d}\u{91f}", "\u{43f}\u{430}\u{441}\u{43f}\u{43e}\u{440}\u{442}",
            "\u{41f}\u{430}\u{441}\u{43f}\u{43e}\u{440}\u{442}", "\u{5d3}\u{5e8}\u{5db}\u{5d5}\u{5df}", "\u{62c}\u{648}\u{627}\u{632} \u{633}\u{641}\u{631}",
            "\u{62c}\u{648}\u{627}\u{632}", "pass-nr", "passnummer",
            "\u{62a4}\u{7167}", "\u{65c5}\u{5238}", "\u{c5ec}\u{ad8c}", "pasaport"] },
    Kind { name: "driving licence number", check: document, bare: &[],
        labels: &["driving licence", "driving license", "driver's license", "drivers license", "driver license", "dl no",
            "licence no", "license no", "führerschein", "fuhrerschein", "permis de conduire", "licencia de conducir",
            "carteira de motorista", "cnh", "patente di guida",
            "\u{921}\u{94d}\u{930}\u{93e}\u{907}\u{935}\u{93f}\u{902}\u{917} \u{932}\u{93e}\u{907}\u{938}\u{947}\u{902}\u{938}"] },
    Kind { name: "national ID number", check: document, bare: &[],
        labels: &["national id", "national identity", "national identity number", "identity card", "citizen id",
            "cédula", "cedula", "documento de identidad", "carnet de identidad", "nid", "national registration", "dni"] },
    Kind { name: "health insurance number", check: document, bare: &[],
        labels: &["health insurance number", "insurance member id", "member id", "medical record number", "mrn",
            "ehic", "krankenversicherungsnummer", "carte vitale", "philhealth", "health card number"] },
    Kind { name: "bank account number", check: account, bare: &[],
        labels: &["account number", "account no", "acct no", "a/c no", "a/c", "bank account", "account #",
            "kontonummer", "numéro de compte", "numero de compte", "número de cuenta", "numero de cuenta",
            "número da conta", "numero da conta", "conto corrente",
            "\u{916}\u{93e}\u{924}\u{93e} \u{938}\u{902}\u{916}\u{94d}\u{92f}\u{93e}",
            "\u{916}\u{93e}\u{924}\u{93e} \u{928}\u{902}\u{92c}\u{930}", "acct", "acct #", "a/c number",
            "konto-nr", "kontonr", "n\u{b0} de compte", "c/c"] },
];

fn plain_digits_11(v: &str) -> bool {
    plain_digits(&[11])(v)
}

// ---------------------------------------------------------------- finding them

fn boundary_before(s: &str, a: usize) -> bool {
    // no letter, digit or "_" before, and not the end of another digit group ("1234 " before a 4-4-4 run)
    let b = s.as_bytes();
    if a == 0 {
        return true;
    }
    let c = b[a - 1];
    if c.is_ascii_alphanumeric() || c == b'_' {
        return false;
    }
    !(matches!(c, b' ' | b'-' | b'.' | b'/') && a >= 2 && b[a - 2].is_ascii_digit())
}

fn boundary_after(s: &str, z: usize) -> bool {
    let b = s.as_bytes();
    match b.get(z) {
        None => true,
        Some(c) if c.is_ascii_alphanumeric() || *c == b'_' => false,
        Some(b' ' | b'-' | b'.' | b'/') => !b.get(z + 1).is_some_and(u8::is_ascii_digit),
        _ => true,
    }
}

/// The layout `groups` read at `a`: where it ends.
fn layout_at(s: &str, a: usize, groups: &[(&str, usize)]) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = a;
    for (n, &(sep, width)) in groups.iter().enumerate() {
        if n > 0 && !sep.is_empty() {
            if i < b.len() && sep.as_bytes().contains(&b[i]) {
                i += 1;
            } else {
                return None;
            }
        }
        if i + width > b.len() || !b[i..i + width].iter().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        i += width;
    }
    Some(i)
}

/// IBANs written whole or in groups of four (ISO 13616's paper form), at a word's start: the country, the check
/// digits, then the longest run of groups that passes mod 97.
fn iban_hits(s: &str, out: &mut Vec<Hit>) {
    let b = s.as_bytes();
    let mut i = 0;
    while i + 15 <= b.len() {
        let starts = b[i].is_ascii_uppercase()
            && b[i + 1].is_ascii_uppercase()
            && b[i + 2].is_ascii_digit()
            && b[i + 3].is_ascii_digit()
            && (i == 0 || !b[i - 1].is_ascii_alphanumeric());
        if !starts {
            i += 1;
            continue;
        }
        // the groups: letters and digits, single spaces between them, at most 34 characters of IBAN
        let mut ends = Vec::new();
        let mut j = i;
        let mut n = 0;
        loop {
            let g = j;
            while j < b.len() && b[j].is_ascii_alphanumeric() && n < 34 {
                j += 1;
                n += 1;
            }
            if j == g {
                break;
            }
            ends.push(j);
            if j + 1 < b.len() && b[j] == b' ' && b[j + 1].is_ascii_alphanumeric() && n < 34 {
                j += 1;
            } else {
                break;
            }
        }
        let found = ends
            .iter()
            .rev()
            .find(|&&z| boundary_after(s, z) && iban(&compact(&s[i..z])))
            .copied();
        match found {
            Some(z) => {
                out.push(Hit {
                    start: i,
                    end: z,
                    kind: "IBAN",
                });
                i = z;
            }
            None => i += 1,
        }
    }
}

/// Every bare identifier in `s`.
fn bare_hits(s: &str, out: &mut Vec<Hit>) {
    iban_hits(s, out);
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_alphanumeric() || !boundary_before(s, i) {
            i += 1;
            continue;
        }
        // the run of groups from here (single separators between them): fewer than four digits is no identifier
        let mut z = i;
        let mut digits_seen = 0;
        while z < b.len() && z - i < 24 {
            if b[z].is_ascii_digit() {
                digits_seen += 1;
            } else if !(b[z].is_ascii_alphanumeric()
                || (matches!(b[z], b' ' | b'-' | b'.' | b'/' | b'+')
                    && z + 1 < b.len()
                    && b[z + 1].is_ascii_alphanumeric()))
            {
                break;
            }
            z += 1;
        }
        let mut found = None;
        if digits_seen < 4 {
            while i < b.len() && b[i].is_ascii_alphanumeric() {
                i += 1;
            }
            continue;
        }
        'kinds: for kind in KINDS {
            for groups in kind.bare {
                if let Some(z) = layout_at(s, i, groups) {
                    if boundary_after(s, z)
                        && (kind.check)(&compact(&s[i..z]))
                        && b[i..z].iter().any(u8::is_ascii_digit)
                    {
                        found = Some((z, kind.name));
                        break 'kinds;
                    }
                }
            }
        }
        match found {
            Some((z, name)) => {
                out.push(Hit {
                    start: i,
                    end: z,
                    kind: name,
                });
                i = z;
            }
            None => {
                // to the next token
                while i < b.len() && b[i].is_ascii_alphanumeric() {
                    i += 1;
                }
            }
        }
    }
}

/// Where a label's value starts: after spaces, ":", "#", "-", "=", "is", "no", "no.", "number" and brackets, on
/// the same line, at most 24 characters on.
fn value_start(s: &str, mut i: usize) -> Option<usize> {
    let start = i;
    let lower = |j: usize, w: &str| {
        s.get(j..j + w.len())
            .is_some_and(|x| x.eq_ignore_ascii_case(w))
    };
    loop {
        if i - start > 24 {
            return None;
        }
        let c = s[i..].chars().next()?;
        if c == '\n' || c == '\r' {
            return None;
        }
        if c.is_whitespace() || ":#-=–—().\"'*".contains(c) {
            i += c.len_utf8();
            continue;
        }
        let word_end = |w: &str| {
            s.as_bytes()
                .get(i + w.len())
                .is_none_or(|x| !x.is_ascii_alphanumeric())
        };
        let mut stepped = false;
        for w in [
            "number", "num", "no", "nr", "nº", "n°", "is", "card", "id", "numéro", "número",
            "numero", "nummer",
        ] {
            if lower(i, w) && word_end(w) {
                i += w.len();
                stepped = true;
                break;
            }
        }
        // a word in another script between the label and its value: "आधार संख्या", "पासपोर्ट नंबर"
        if !stepped && !c.is_ascii() && c.is_alphabetic() {
            while let Some(ch) = s[i..].chars().next() {
                if ch.is_ascii() || !(ch.is_alphabetic() || unicode_mark(ch)) {
                    break;
                }
                i += ch.len_utf8();
            }
            stepped = true;
        }
        if !stepped {
            return Some(i);
        }
    }
}

/// The value after a label: groups of letters and digits joined by single spaces, "-", "." or "/", longest first;
/// the first the kind's check accepts.
fn labelled_value(s: &str, a: usize, kind: &Kind) -> Option<Span> {
    let b = s.as_bytes();
    let mut ends = Vec::new();
    let mut i = a;
    // a group, then a separator and another group, up to 40 characters, never across prose words
    loop {
        let g = i;
        while i < b.len() && b[i].is_ascii_alphanumeric() {
            i += 1;
        }
        if i == g || i - a > 40 {
            break;
        }
        let group = &s[g..i];
        if group.bytes().any(|c| c.is_ascii_lowercase())
            && !group.bytes().any(|c| c.is_ascii_digit())
            && g > a
        {
            break; // a word after the value
        }
        // a check character in brackets: HKID A123456(3)
        if i + 2 < b.len() && b[i] == b'(' && b[i + 1].is_ascii_alphanumeric() && b[i + 2] == b')' {
            i += 3;
        }
        ends.push(i);
        if i + 1 < b.len()
            && matches!(b[i], b' ' | b'-' | b'.' | b'/')
            && b[i + 1].is_ascii_alphanumeric()
        {
            i += 1;
        } else {
            break;
        }
    }
    for &z in ends.iter().rev() {
        let v = compact(&s[a..z]);
        if (kind.check)(&v) && boundary_after(s, z) && !ipv4(&s[a..z]) {
            return Some((a, z));
        }
    }
    None
}

/// Short labels that are no word in any language FERB reads and whose kind's check is strong (Steuer-ID's ISO 7064
/// with its repetition rule, the henkilötunnus' date and mod 31): any case counts (`IdNr`, `sotu`).
const SHORT_ANY_CASE: &[&str] = &["idnr", "sotu"];

/// An IPv4 address (four groups of 1-3 digits joined by "."): a technical identifier, never a labelled value
/// (`NAS 192.168.1.10`: nine digits that pass Luhn one time in ten).
fn ipv4(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|p| (1..=3).contains(&p.len()) && p.bytes().all(|c| c.is_ascii_digit()))
}

/// A label stands as its own word; a label of four letters or fewer counts only written in capitals (SSN, SIN,
/// PAN, RUN, INN, TIN: "run", "sin", "inn" and "tin" are English words), but for `SHORT_ANY_CASE`.
fn label_bounded(s: &str, a: usize, z: usize) -> bool {
    let b = s.as_bytes();
    let ascii_word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let bounded = (a == 0 || !ascii_word(b[a - 1]) || !b[a].is_ascii())
        && (z >= b.len() || !ascii_word(b[z]) || !b[z - 1].is_ascii());
    let text = &s[a..z];
    let one_short_word = text.len() <= 4 && text.bytes().all(|c| c.is_ascii_alphabetic());
    bounded
        && (!one_short_word
            || !text.bytes().any(|c| c.is_ascii_lowercase())
            || SHORT_ANY_CASE.iter().any(|w| w.eq_ignore_ascii_case(text)))
}

fn find_label(s: &str, lower: &str, label: &str, out: &mut Vec<(usize, usize)>) {
    if label.is_ascii() {
        if !lower.contains(label) {
            return;
        }
        let (bs, n) = (s.as_bytes(), label.as_bytes());
        let mut i = 0;
        while i + n.len() <= bs.len() {
            if bs[i].eq_ignore_ascii_case(&n[0]) && bs[i..i + n.len()].eq_ignore_ascii_case(n) {
                out.push((i, i + n.len()));
                i += n.len();
            } else {
                i += 1;
            }
        }
    } else {
        out.extend(s.match_indices(label).map(|(i, m)| (i, i + m.len())));
        // a cased script's other case (Cyrillic, Latin with marks): the lowercased text's offsets match only
        // when lowercasing kept every length, so the text itself is searched for the capitalised form too
        let mut cap = label.chars();
        if let Some(first) = cap.next() {
            let upper: String = first.to_uppercase().chain(cap).collect();
            if upper != label {
                out.extend(
                    s.match_indices(upper.as_str())
                        .map(|(i, m)| (i, i + m.len())),
                );
            }
        }
    }
}

/// Every ASCII label, by its first byte (lowercase), with its kind's index: the text is read once.
/// One bucket of labels: (the label's bytes, its kind's index).
type Bucket = Vec<(&'static [u8], usize)>;

fn ascii_labels() -> &'static [Bucket] {
    static T: std::sync::OnceLock<Vec<Bucket>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t: Vec<Bucket> = vec![Vec::new(); 256];
        for (k, kind) in KINDS.iter().enumerate() {
            for label in kind.labels.iter().filter(|l| l.is_ascii()) {
                t[label.as_bytes()[0] as usize].push((label.as_bytes(), k));
            }
        }
        t
    })
}

/// Where each label stands in `s` as its own word: (start, end, kind index).
fn label_hits(s: &str) -> Vec<(usize, usize, usize)> {
    let b = s.as_bytes();
    let table = ascii_labels();
    let mut out = Vec::new();
    for i in 0..b.len() {
        let c = b[i].to_ascii_lowercase();
        if !c.is_ascii_alphabetic() || (i > 0 && b[i - 1].is_ascii_alphanumeric()) {
            continue;
        }
        for &(label, k) in &table[c as usize] {
            if b.len() - i >= label.len()
                && b[i..i + label.len()].eq_ignore_ascii_case(label)
                && label_bounded(s, i, i + label.len())
            {
                out.push((i, i + label.len(), k));
            }
        }
    }
    if !s.is_ascii() {
        let mut found = Vec::new();
        for (k, kind) in KINDS.iter().enumerate() {
            for label in kind.labels.iter().filter(|l| !l.is_ascii()) {
                found.clear();
                find_label(s, "", label, &mut found);
                out.extend(
                    found
                        .iter()
                        .filter(|&&(a, z)| label_bounded(s, a, z))
                        .map(|&(a, z)| (a, z, k)),
                );
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn labelled_hits(s: &str, out: &mut Vec<Hit>) {
    for (_, z, k) in label_hits(s) {
        let kind = &KINDS[k];
        let Some(v0) = value_start(s, z) else {
            continue;
        };
        if let Some((x, y)) = labelled_value(s, v0, kind) {
            out.push(Hit {
                start: x,
                end: y,
                kind: kind.name,
            });
        }
    }
}

/// A combining mark (Devanagari's vowel signs and virama are marks, not letters).
fn unicode_mark(c: char) -> bool {
    matches!(c as u32, 0x0900..=0x0903 | 0x093a..=0x094f | 0x0951..=0x0957 | 0x0962..=0x0963 | 0x0300..=0x036f)
}

/// Whether a line names an identifier (one of the labels): the stream holds such a line until it ends, so a value
/// written in groups is never released a group at a time.
pub(crate) fn names_identifier(line: &str) -> bool {
    !label_hits(line).is_empty()
}

/// Every personal identifier of category (i) in `s`, named by kind.
pub(crate) fn pii_hits(s: &str) -> Vec<Hit> {
    let mut out = Vec::new();
    if s.len() < 6 || !s.bytes().any(|c| c.is_ascii_digit()) {
        return out;
    }
    bare_hits(s, &mut out);
    labelled_hits(s, &mut out);
    out
}

/// A dict key that names an identifier: its value is checked as if the key were its label (`{"aadhaar":
/// "234567890124"}`).
pub(crate) fn key_hit(key: &str, value: &str) -> Option<&'static str> {
    let words = crate::labels::words(key).join(" ");
    if words.is_empty() || value.len() > 64 {
        return None;
    }
    let v = compact(value);
    if v.is_empty() || !v.bytes().any(|c| c.is_ascii_digit()) || value.matches(' ').count() > 6 {
        return None;
    }
    for kind in KINDS {
        let named = kind.labels.iter().any(|l| {
            l.is_ascii()
                && (words == *l
                    || words.ends_with(&format!(" {l}"))
                    || words.starts_with(&format!("{l} ")))
        });
        if named && (kind.check)(&v) {
            return Some(kind.name);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // decision: GATE-599
    /// Specimens the issuers publish (or the algorithm's own worked example), so a misread algorithm fails here and
    /// not only against the corpus generator, which was written by the same hand.
    #[test]
    fn the_checks_accept_the_published_specimens() {
        assert!(
            verhoeff(&digits("2363")),
            "Verhoeff's worked example: 236 has check digit 3"
        );
        assert!(
            uk_nhs("9434765919"),
            "NHS Digital's documented test number 943 476 5919"
        );
        // Service Canada's sample SIN 046 454 286 passes Luhn; a SIN starting with 0 is never issued (fictitious)
        assert!(luhn(&digits("046454286")) && !ca_sin("046454286"));
        assert!(cn_rid("11010519491231002X"), "GB 11643-1999's example");
        assert!(
            fi_hetu("131052-308T"),
            "the Finnish Digital and Population Data Services Agency's example"
        );
        assert!(iban("DE89370400440532013000"), "the ECBS example IBAN");
        assert!(iban("GB82WEST12345698765432"), "the ECBS UK example IBAN");
        assert!(es_dni("12345678Z"), "12345678 mod 23 = 14, the letter Z");
        assert!(tw_id("A123456789"), "the textbook Taiwan example");
        assert!(
            us_routing("021000021"),
            "a published ABA routing number (a bank's, not a person's)"
        );
        // and a changed digit fails each
        assert!(
            !uk_nhs("9434765918") && !luhn(&digits("046454287")) && !cn_rid("110105194912310021")
        );
        assert!(!iban("DE89370400440532013001") && !es_dni("12345678A") && !tw_id("A123456788"));
    }

    // decision: GATE-600
    /// The corrections from the independent review, each with python-stdnum 2.2's documented examples (or the
    /// issuer's published example) and a changed check character that fails. python-stdnum was the oracle for every
    /// kind it has (300 generated valid numbers each, all accepted; see rust/JOURNAL.md); these are the kept few.
    #[test]
    fn the_reviewed_rules_accept_stdnum_specimens() {
        // Portugal NIF: any first digit but 0 (stdnum pt.nif's example 501964843; 494313650 and 754016072 were missed)
        assert!(pt_nif("501964843") && pt_nif("494313650") && pt_nif("754016072"));
        assert!(!pt_nif("501964842") && !pt_nif("094313650"));
        // Ireland PPS: the old format's ninth letter W, T or X is ignored; the 2013 format's counts
        for v in [
            "6433435F",
            "6433435FT",
            "6433435FW",
            "6433435FX",
            "6433435OA",
        ] {
            assert!(ie_ppsn(v), "stdnum ie.pps example {v}");
        }
        assert!(!ie_ppsn("6433435E") && !ie_ppsn("6433435VH") && !ie_ppsn("6433435FC"));
        // Israel: a shorter number is padded to nine (stdnum il.idnr's 3933742-3)
        assert!(il_id("39337423") && il_id("039337423") && !il_id("39337422"));
        assert_eq!(pii_hits("ת.ז. 3933742-3")[0].kind, "Israeli ID");
        // Germany Steuer-ID: stdnum de.idnr's example, one the oracle generated, and its "more digits repeated"
        assert!(de_steuer_id("36574261809") && de_steuer_id("39170342364"));
        assert!(!de_steuer_id("36554266806") && !de_steuer_id("36574261890"));
        assert_eq!(pii_hits("IdNr 36574261809")[0].kind, "Steuer-ID");
        // Germany Personalausweis: the specimen card's T22000129, with its check digit (MRZ) and as printed
        assert!(de_ausweis("T220001293") && de_ausweis("T22000129") && !de_ausweis("T220001294"));
        assert_eq!(
            pii_hits("Ausweis-Nr. T22000129")[0].kind,
            "Personalausweis number"
        );
        // France NIR: 2A/2B only as the department (stdnum fr.nir's examples)
        assert!(
            fr_nir("295109912611193") && fr_nir("253072B07300470") && fr_nir("253072A07300443")
        );
        assert!(!fr_nir("253072C07300443") && !fr_nir("295109912611199"));
        assert!(
            !fr_nir("2A5037512345636"),
            "2A at the start was read as 19 before"
        );
        // IBAN: the SWIFT registry's examples of the twelve countries added (each valid for stdnum's iban)
        for v in [
            "BI4210000100010000332045181",
            "DJ2100010000000154000100186",
            "FK88SC123456789012",
            "HN88CABF00000000000250005469",
            "LY83002048000020100120361",
            "MN121234123456789123",
            "NI45BAPR00000013000003558124",
            "OM810180000001299123456",
            "RU0304452522540817810538091310419",
            "SD2129010501234001",
            "SO211000001001000100141",
            "YE15CBYE0001018861234567891234",
        ] {
            assert!(iban(v), "{v}");
        }
        assert!(!iban("RU0304452522540817810538091310418"));
        // PAN: K (a krish trust) as the fourth letter; stdnum in_.pan's examples
        assert!(pan("ACUPA7085R") && pan("AAPPV8261K") && pan("AAAKA1234A") && !pan("AAAEA1234A"));
        // voter ID: Luhn over the seven digits (stdnum in_.epic's WKH1186253 valid, WKH1186263 not)
        assert!(in_epic("WKH1186253") && !in_epic("WKH1186263"));
        // Aadhaar VID: never starts with 0 or 1 (stdnum in_.vid)
        assert!(aadhaar_vid("2341234123412341") && !aadhaar_vid("1341234123412341"));
        // Norway: stdnum no.fodselsnummer's 151086 95088, its 15108695077 fails
        assert!(no_fnr("15108695088") && !no_fnr("15108695077"));
        assert_eq!(
            pii_hits("fødselsnummer 151086 95088")[0].kind,
            "fødselsnummer"
        );
        assert_eq!(
            pii_hits("Personnummer: 15108695088")[0].kind,
            "fødselsnummer"
        );
        // Mexico CLABE: Banxico's worked example (as Wikipedia gives it)
        assert!(mx_clabe("032180000118359719") && !mx_clabe("032180000118359718"));
        assert_eq!(pii_hits("CLABE 032180000118359719")[0].kind, "CLABE");
        // shape-only, label-only: Singapore's M series, Taiwan's 8/9, Sri Lanka's twelve digits
        assert_eq!(pii_hits("FIN: M1234567K")[0].kind, "NRIC");
        assert!(pii_hits("ref M1234567K sent").is_empty());
        assert!(tw_id("A800000014") && !tw_id("A800000015") && !tw_id("A300000014"));
        assert_eq!(pii_hits("統一證號 A800000014")[0].kind, "Taiwan ID");
        assert!(
            lk_nic_new("199012345678") && lk_nic_new("200058212345") && !lk_nic_new("199040012345")
        );
        assert_eq!(pii_hits("NIC 199012345678")[0].kind, "NIC");
        assert!(pii_hits("build 199012345678 ok").is_empty());
        // an IPv4 address after a label is no identifier (192168110 and 192168128 pass the SIN's Luhn)
        assert!(ca_sin("192168110") && ca_sin("192168128"));
        assert!(pii_hits("NAS 192.168.1.10 and SIN: 192.168.1.28").is_empty());
    }

    // decision: GATE-599
    #[test]
    fn a_bare_run_of_digits_is_never_an_identifier() {
        // twelve digits that pass Verhoeff, written as one run: only a label or the issuer's 4-4-4 grouping counts
        let n = (0..10)
            .map(|d| format!("23412341234{d}"))
            .find(|n| aadhaar(n))
            .expect("one of ten check digits passes");
        assert!(pii_hits(&format!("order {n} shipped")).is_empty());
        let grouped = format!("{} {} {}", &n[..4], &n[4..8], &n[8..]);
        assert_eq!(
            pii_hits(&format!("card {grouped} issued"))[0].kind,
            "Aadhaar number"
        );
        assert_eq!(pii_hits(&format!("Aadhaar: {n}"))[0].kind, "Aadhaar number");
        // the same digits one group longer are part of another number
        assert!(pii_hits(&format!("{grouped} 1234")).is_empty());
    }
}
