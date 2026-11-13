//! Reading a zip archive the way the import does with Python's `zipfile` (`is_zipfile`, `infolist`, `read`):
//! the end record (with a comment, or Zip64), the central directory in order, cp437 or UTF-8 names, stored and
//! deflated members with their CRC checked. Python also reads bzip2 and LZMA members; here those, and encrypted
//! members, stop the import (`Crash`), as an unknown method does in Python.

use crate::generated as g;
use crate::pyval::Crash;

pub struct Member {
    pub name: String,
    pub file_size: u64,
    compress_size: u64,
    method: u16,
    flags: u16,
    crc: u32,
    header_offset: u64,
}

impl Member {
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/')
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// `_EndRecData`: the end-of-central-directory record's offset.
fn end_record(data: &[u8]) -> Option<usize> {
    if data.len() < 22 {
        return None;
    }
    let last = data.len() - 22;
    if &data[last..last + 4] == b"PK\x05\x06" && data[last + 20..last + 22] == [0, 0] {
        return Some(last);
    }
    let from = data.len().saturating_sub(65_535 + 22);
    let window = &data[from..];
    let at = window.windows(4).rposition(|w| w == b"PK\x05\x06")?;
    let at = from + at;
    (at + 22 <= data.len()).then_some(at)
}

/// `zipfile.is_zipfile(bytes)`.
pub fn is_zip(data: &[u8]) -> bool {
    end_record(data).is_some()
}

fn decode_name(raw: &[u8], utf8: bool) -> String {
    if utf8 {
        return String::from_utf8_lossy(raw).into_owned();
    }
    let high: Vec<char> = g::CP437_HIGH.chars().collect();
    raw.iter()
        .map(|&b| {
            if b < 0x80 {
                char::from(b)
            } else {
                high[usize::from(b - 0x80)]
            }
        })
        .collect()
}

/// `ZipFile(...).infolist()`.
pub fn members(data: &[u8]) -> Result<Vec<Member>, Crash> {
    let bad = Crash("BadZipFile");
    let end = end_record(data).ok_or(bad)?;
    let mut size = u64::from(u32_at(data, end + 12).ok_or(bad)?);
    let mut offset = u64::from(u32_at(data, end + 16).ok_or(bad)?);
    let mut dir_end = end as u64;
    // Zip64 (`_EndRecData64`): the locator just before the end record, the Zip64 record just before it
    if end >= 20 + 56 && &data[end - 20..end - 16] == b"PK\x06\x07" {
        let rec = end - 20 - 56;
        if &data[rec..rec + 4] == b"PK\x06\x06" {
            size = u64_at(data, rec + 40).ok_or(bad)?;
            offset = u64_at(data, rec + 48).ok_or(bad)?;
            dir_end = rec as u64;
        }
    }
    // the archive may have bytes before it (concat = where the directory really starts minus where it says)
    let concat = dir_end
        .checked_sub(size)
        .and_then(|x| x.checked_sub(offset))
        .ok_or(bad)?;
    let mut at = (offset + concat) as usize;
    let dir_start = at;
    let mut out = Vec::new();
    while ((at - dir_start) as u64) < size {
        if data.get(at..at + 4) != Some(b"PK\x01\x02") {
            return Err(bad);
        }
        let flags = u16_at(data, at + 8).ok_or(bad)?;
        let method = u16_at(data, at + 10).ok_or(bad)?;
        let crc = u32_at(data, at + 16).ok_or(bad)?;
        let mut compress_size = u64::from(u32_at(data, at + 20).ok_or(bad)?);
        let mut file_size = u64::from(u32_at(data, at + 24).ok_or(bad)?);
        let name_len = usize::from(u16_at(data, at + 28).ok_or(bad)?);
        let extra_len = usize::from(u16_at(data, at + 30).ok_or(bad)?);
        let comment_len = usize::from(u16_at(data, at + 32).ok_or(bad)?);
        let mut header_offset = u64::from(u32_at(data, at + 42).ok_or(bad)?);
        let name_raw = data.get(at + 46..at + 46 + name_len).ok_or(bad)?;
        let extra = data
            .get(at + 46 + name_len..at + 46 + name_len + extra_len)
            .ok_or(bad)?;
        // the Zip64 extra field carries the sizes and offset that didn't fit
        let mut e = 0;
        while e + 4 <= extra.len() {
            let tag = u16_at(extra, e).ok_or(bad)?;
            let len = usize::from(u16_at(extra, e + 2).ok_or(bad)?);
            if tag == 1 {
                let mut p = e + 4;
                if file_size == 0xFFFF_FFFF {
                    file_size = u64_at(extra, p).ok_or(bad)?;
                    p += 8;
                }
                if compress_size == 0xFFFF_FFFF {
                    compress_size = u64_at(extra, p).ok_or(bad)?;
                    p += 8;
                }
                if header_offset == 0xFFFF_FFFF {
                    header_offset = u64_at(extra, p).ok_or(bad)?;
                }
            }
            e += 4 + len;
        }
        out.push(Member {
            name: decode_name(name_raw, flags & 0x800 != 0),
            file_size,
            compress_size,
            method,
            flags,
            crc,
            header_offset: header_offset + concat,
        });
        at += 46 + name_len + extra_len + comment_len;
    }
    Ok(out)
}

/// `ZipFile.read(member)`.
pub fn read(data: &[u8], m: &Member) -> Result<Vec<u8>, Crash> {
    let bad = Crash("BadZipFile");
    let at = m.header_offset as usize;
    if data.get(at..at + 4) != Some(b"PK\x03\x04") {
        return Err(bad);
    }
    let name_len = usize::from(u16_at(data, at + 26).ok_or(bad)?);
    let extra_len = usize::from(u16_at(data, at + 28).ok_or(bad)?);
    let local_name = data.get(at + 30..at + 30 + name_len).ok_or(bad)?;
    if decode_name(local_name, m.flags & 0x800 != 0) != m.name {
        return Err(bad);
    }
    if m.flags & 1 != 0 {
        return Err(Crash("RuntimeError"));
    }
    let start = at + 30 + name_len + extra_len;
    let body = data
        .get(start..start + m.compress_size as usize)
        .ok_or(Crash("EOFError"))?;
    let out = match m.method {
        0 => body.to_vec(),
        8 => inflate(body, m.file_size as usize).ok_or(Crash("error"))?,
        _ => return Err(Crash("NotImplementedError")),
    };
    if crc32(&out) != m.crc {
        return Err(bad);
    }
    Ok(out)
}

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *slot = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    bit: u32,
    nbits: u32,
}

impl Bits<'_> {
    fn need(&mut self, n: u32) -> Option<()> {
        while self.nbits < n {
            let b = *self.data.get(self.pos)?;
            self.pos += 1;
            self.bit |= u32::from(b) << self.nbits;
            self.nbits += 8;
        }
        Some(())
    }

    fn take(&mut self, n: u32) -> Option<u32> {
        if n == 0 {
            return Some(0);
        }
        self.need(n)?;
        let v = self.bit & ((1u32 << n) - 1);
        self.bit >>= n;
        self.nbits -= n;
        Some(v)
    }
}

struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Option<Huffman> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[usize::from(l)] += 1;
        }
        counts[0] = 0;
        let mut offs = [0u16; 16];
        for i in 1..16 {
            offs[i] = offs[i - 1] + counts[i - 1];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (s, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[usize::from(offs[usize::from(l)])] = s as u16;
                offs[usize::from(l)] += 1;
            }
        }
        Some(Huffman { counts, symbols })
    }

    fn decode(&self, bits: &mut Bits<'_>) -> Option<u16> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= bits.take(1)? as i32;
            let count = i32::from(self.counts[len]);
            if code - count < first {
                return self.symbols.get((index + (code - first)) as usize).copied();
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        None
    }
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// Raw DEFLATE (RFC 1951).
pub fn inflate(data: &[u8], hint: usize) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(hint.min(1 << 30));
    let mut bits = Bits {
        data,
        pos: 0,
        bit: 0,
        nbits: 0,
    };
    loop {
        let last = bits.take(1)?;
        match bits.take(2)? {
            0 => {
                bits.bit = 0;
                bits.nbits = 0;
                let len = u16_at(data, bits.pos)?;
                let nlen = u16_at(data, bits.pos + 2)?;
                if len != !nlen {
                    return None;
                }
                let start = bits.pos + 4;
                out.extend_from_slice(data.get(start..start + usize::from(len))?);
                bits.pos = start + usize::from(len);
            }
            1 => {
                let mut l = [0u8; 288];
                l[..144].fill(8);
                l[144..256].fill(9);
                l[256..280].fill(7);
                l[280..].fill(8);
                let lit = Huffman::new(&l)?;
                let dist = Huffman::new(&[5u8; 30])?;
                block(&mut bits, &mut out, &lit, &dist)?;
            }
            2 => {
                let hlit = bits.take(5)? as usize + 257;
                let hdist = bits.take(5)? as usize + 1;
                let hclen = bits.take(4)? as usize + 4;
                const ORDER: [usize; 19] = [
                    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
                ];
                let mut cl = [0u8; 19];
                for &o in ORDER.iter().take(hclen) {
                    cl[o] = bits.take(3)? as u8;
                }
                let clh = Huffman::new(&cl)?;
                let mut lengths = vec![0u8; hlit + hdist];
                let mut i = 0;
                while i < hlit + hdist {
                    let sym = clh.decode(&mut bits)?;
                    match sym {
                        0..=15 => {
                            lengths[i] = sym as u8;
                            i += 1;
                        }
                        16 => {
                            let prev = *lengths.get(i.checked_sub(1)?)?;
                            for _ in 0..3 + bits.take(2)? {
                                *lengths.get_mut(i)? = prev;
                                i += 1;
                            }
                        }
                        17 => i += 3 + bits.take(3)? as usize,
                        _ => i += 11 + bits.take(7)? as usize,
                    }
                }
                if i > hlit + hdist {
                    return None;
                }
                let lit = Huffman::new(&lengths[..hlit])?;
                let dist = Huffman::new(&lengths[hlit..])?;
                block(&mut bits, &mut out, &lit, &dist)?;
            }
            _ => return None,
        }
        if last == 1 {
            return Some(out);
        }
    }
}

fn block(bits: &mut Bits<'_>, out: &mut Vec<u8>, lit: &Huffman, dist: &Huffman) -> Option<()> {
    loop {
        let sym = lit.decode(bits)?;
        if sym < 256 {
            out.push(sym as u8);
        } else if sym == 256 {
            return Some(());
        } else {
            let i = usize::from(sym - 257);
            let len = usize::from(*LEN_BASE.get(i)?) + bits.take(u32::from(LEN_EXTRA[i]))? as usize;
            let d = usize::from(dist.decode(bits)?);
            let back =
                usize::from(*DIST_BASE.get(d)?) + bits.take(u32::from(DIST_EXTRA[d]))? as usize;
            let start = out.len().checked_sub(back)?;
            for k in 0..len {
                let b = out[start + k];
                out.push(b);
            }
        }
    }
}
