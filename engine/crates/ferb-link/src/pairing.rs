//! Pairing: the QR carries the Mac's id, where to reach it and a one-time secret. The phone proves it saw the
//! QR with a keyed hash over both endpoint ids (which the QUIC/TLS handshake authenticates), so a proof is
//! useless to any other endpoint and the secret never crosses the wire. Then the owner says Yes on the Mac.

use crate::errors::{self, LinkError, LinkResult};
use crate::net::RelaySpec;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use iroh::EndpointId;
use std::{
    str::FromStr,
    time::{Duration, Instant},
};

pub const QR_SCHEME: &str = "ferblink";
/// How the short code starts.
pub const CODE_PREFIX: &str = "FERB-";
/// The short code's first byte: the link version it was made for.
const CODE_VERSION: u8 = 2;
const SAME_AS_OWN: u8 = 0x7f;
/// Shared relays the short code names by number. The order is part of the code: only ever add at the end.
pub const KNOWN_RELAYS: [&str; 4] = [
    "https://use1-1.relay.n0.iroh.link./",
    "https://usw1-1.relay.n0.iroh.link./",
    "https://euc1-1.relay.n0.iroh.link./",
    "https://aps1-1.relay.n0.iroh.link./",
];
/// The characters of a QR code's dense text mode, without the space, `$`, `%` and `*`: 41, so two bytes are three
/// characters, and nothing in it is changed by a paste, a message app or a terminal.
const B41: &[u8; 41] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ+-./:";

fn b41_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3 / 2 + 2);
    for pair in bytes.chunks(2) {
        let (mut n, chars) = if pair.len() == 2 {
            ((pair[0] as usize) << 8 | pair[1] as usize, 3)
        } else {
            (pair[0] as usize, 2)
        };
        for _ in 0..chars {
            out.push(B41[n % 41] as char);
            n /= 41;
        }
    }
    out
}
fn b41_decode(text: &str) -> Option<Vec<u8>> {
    let digits = text
        .bytes()
        .map(|c| B41.iter().position(|b| *b == c))
        .collect::<Option<Vec<_>>>()?;
    let mut out = Vec::with_capacity(digits.len() * 2 / 3 + 1);
    for group in digits.chunks(3) {
        let n = group.iter().rev().fold(0usize, |n, d| n * 41 + d);
        match group.len() {
            3 if n <= 0xffff => out.extend_from_slice(&[(n >> 8) as u8, n as u8]),
            2 if n <= 0xff => out.push(n as u8),
            _ => return None,
        }
    }
    Some(out)
}

/// The name relay/setup.sh gives the relay's own authority (`CN=ferb-relay authority`), as a certificate holds it.
fn setup_authority_name() -> Vec<u8> {
    [
        &[
            0x31, 0x1d, 0x30, 0x1b, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x14,
        ][..],
        b"ferb-relay authority",
    ]
    .concat()
}

/// A P-256 public key as certificates carry it is these fixed bytes and then the point's two coordinates; the
/// code carries the point alone, compressed (33 bytes).
const P256_HEAD: [u8; 25] = [
    0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48,
    0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00, 0x04,
];
fn p256_short(spki: &[u8]) -> Option<Vec<u8>> {
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    if spki.len() != P256_HEAD.len() + 64 || !spki.starts_with(&P256_HEAD) {
        return None;
    }
    let key = p256::PublicKey::from_sec1_bytes(&spki[P256_HEAD.len() - 1..]).ok()?;
    Some(key.to_encoded_point(true).as_bytes().to_vec())
}
fn p256_long(point: &[u8]) -> Option<Vec<u8>> {
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    let key = p256::PublicKey::from_sec1_bytes(point).ok()?;
    Some(
        [
            &P256_HEAD[..P256_HEAD.len() - 1],
            key.to_encoded_point(false).as_bytes(),
        ]
        .concat(),
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct Invite {
    pub mac: EndpointId,
    /// Relays the Mac can be reached at right now.
    pub relays: Vec<String>,
    /// The owner's own relay, when one is set (the phone then uses it as its relay too).
    pub own: Option<RelaySpec>,
    pub secret: [u8; 32],
}

impl Invite {
    pub fn to_url(&self) -> String {
        let mut u = url::Url::parse(&format!("{QR_SCHEME}://pair")).unwrap();
        {
            let mut q = u.query_pairs_mut();
            q.append_pair("v", "2")
                .append_pair("id", &self.mac.to_string())
                .append_pair("s", &B64.encode(self.secret));
            for r in &self.relays {
                q.append_pair("r", r);
            }
            if let Some(own) = &self.own {
                q.append_pair("own", &own.url);
                if let Some(t) = &own.token {
                    q.append_pair("tok", t);
                }
                if let Some(c) = &own.ca {
                    q.append_pair("ca", c);
                }
            }
        }
        u.to_string()
    }
    /// What the QR shows and what the owner may type or paste: `FERB-` and the same facts as bytes, in the 41
    /// characters a QR stores most densely that also survive a paste (no space, no lower case). An own relay's
    /// certificate authority travels as its name and public key, all the phone needs to accept that relay
    /// before the Mac's welcome brings the whole certificate.
    ///
    /// Bytes: `2` (the link version) · flags (1 own relay, 2 its token, 4 its authority) · Mac id (32) ·
    /// secret (32) · relay count, then each relay · own relay · token · authority's name and key.
    pub fn to_code(&self) -> LinkResult<String> {
        let long = || {
            LinkError::new(
                errors::BAD_REQUEST,
                "This pairing code does not fit the short form.",
            )
        };
        let text = |out: &mut Vec<u8>, t: &str| -> LinkResult<()> {
            let t = t.strip_prefix("https://").unwrap_or(t);
            if t.is_empty() || t.len() > 126 {
                return Err(long());
            }
            out.push(t.len() as u8);
            out.extend_from_slice(t.as_bytes());
            Ok(())
        };
        let own_url = self.own.as_ref().map(|o| o.url.as_str());
        let token = self.own.as_ref().and_then(|o| o.token.as_deref());
        let key = match &self.own {
            Some(o) if o.ca.is_some() => Some(o.authority()?),
            _ => None,
        };
        let mut out = vec![
            CODE_VERSION,
            u8::from(own_url.is_some())
                | u8::from(token.is_some()) << 1
                | u8::from(key.is_some()) << 2,
        ];
        out.extend_from_slice(self.mac.as_bytes());
        out.extend_from_slice(&self.secret);
        if self.relays.len() > 8 {
            return Err(long());
        }
        out.push(self.relays.len() as u8);
        for r in &self.relays {
            if Some(r.as_str()) == own_url {
                out.push(SAME_AS_OWN);
            } else if let Some(i) = KNOWN_RELAYS.iter().position(|k| k == r) {
                out.push(0x80 | i as u8);
            } else {
                text(&mut out, r)?;
            }
        }
        if let Some(u) = own_url {
            text(&mut out, u)?;
        }
        if let Some(t) = token {
            // a token made of base64url characters (the setup script's) travels as the bytes it stands for
            match B64
                .decode(t)
                .ok()
                .filter(|raw| !raw.is_empty() && raw.len() < 128 && B64.encode(raw) == t)
            {
                Some(raw) => {
                    out.push(0x80 | raw.len() as u8);
                    out.extend_from_slice(&raw);
                }
                None if !t.is_empty() && t.len() < 128 => {
                    out.push(t.len() as u8);
                    out.extend_from_slice(t.as_bytes());
                }
                None => return Err(long()),
            }
        }
        if let Some((name, spki)) = key {
            if name.len() > 255 || spki.len() > 65535 {
                return Err(long());
            }
            // (the name the setup script gives its authority is not spelled out: length 0 stands for it)
            let name = if name == setup_authority_name() {
                vec![]
            } else {
                name
            };
            out.push(name.len() as u8);
            out.extend_from_slice(&name);
            match p256_short(&spki) {
                Some(point) => {
                    out.push(1);
                    out.extend_from_slice(&point);
                }
                None => {
                    out.push(0);
                    out.extend_from_slice(&(spki.len() as u16).to_be_bytes());
                    out.extend_from_slice(&spki);
                }
            }
        }
        Ok(format!("{CODE_PREFIX}{}", b41_encode(&out)))
    }

    fn parse_code(text: &str) -> LinkResult<Self> {
        let bad = |why: &str| {
            LinkError::new(
                errors::BAD_REQUEST,
                format!("This is not a FERB pairing code ({why})."),
            )
            .allow("Scan the code FERB shows on the Mac")
        };
        let bytes = b41_decode(&text[CODE_PREFIX.len()..])
            .ok_or_else(|| bad("it has a character that does not belong, or it is cut off"))?;
        let mut at = 0usize;
        let mut take = |n: usize| -> LinkResult<&[u8]> {
            let part = bytes.get(at..at + n).ok_or_else(|| bad("it is cut off"))?;
            at += n;
            Ok(part)
        };
        if take(1)?[0] != CODE_VERSION {
            return Err(LinkError::new(
                errors::VERSION_MISMATCH,
                "This pairing code is from a different FERB version.",
            )
            .allow("Update the phone app and FERB on the Mac"));
        }
        let flags = take(1)?[0];
        let mac =
            EndpointId::from_bytes(take(32)?.try_into().unwrap()).map_err(|_| bad("bad Mac id"))?;
        let secret: [u8; 32] = take(32)?.try_into().unwrap();
        let url = |t: &[u8]| -> LinkResult<String> {
            let t = std::str::from_utf8(t).map_err(|_| bad("bad relay address"))?;
            Ok(if t.contains("://") {
                t.to_string()
            } else {
                format!("https://{t}")
            })
        };
        let mut relays: Vec<Option<String>> = vec![];
        for _ in 0..take(1)?[0] {
            let tag = take(1)?[0];
            relays.push(match tag {
                SAME_AS_OWN => None,
                t if t & 0x80 != 0 => Some(
                    KNOWN_RELAYS
                        .get((t & 0x7f) as usize)
                        .ok_or_else(|| {
                            LinkError::new(
                                errors::VERSION_MISMATCH,
                                "This pairing code names a relay this app does not know yet.",
                            )
                            .allow("Update the phone app")
                        })?
                        .to_string(),
                ),
                n => Some(url(take(n as usize)?)?),
            });
        }
        let mut own = None;
        if flags & 1 != 0 {
            let n = take(1)?[0] as usize;
            let mut spec = RelaySpec::new(url(take(n)?)?, None);
            if flags & 2 != 0 {
                let tag = take(1)?[0];
                let raw = take((tag & 0x7f) as usize)?;
                spec.token = Some(if tag & 0x80 != 0 {
                    B64.encode(raw)
                } else {
                    String::from_utf8(raw.to_vec()).map_err(|_| bad("bad relay token"))?
                });
            }
            if flags & 4 != 0 {
                let n = take(1)?[0] as usize;
                let name = if n == 0 {
                    setup_authority_name()
                } else {
                    take(n)?.to_vec()
                };
                let spki = match take(1)?[0] {
                    1 => p256_long(take(33)?).ok_or_else(|| bad("bad relay key"))?,
                    0 => {
                        let n = u16::from_be_bytes(take(2)?.try_into().unwrap()) as usize;
                        take(n)?.to_vec()
                    }
                    _ => return Err(bad("unknown kind of relay key")),
                };
                spec.ca = Some(RelaySpec::authority_text(&name, &spki));
            }
            own = Some(spec);
        }
        if at != bytes.len() {
            return Err(bad("it is longer than it should be"));
        }
        let own_url = own.as_ref().map(|o: &RelaySpec| o.url.clone());
        let relays = relays
            .into_iter()
            .map(|r| r.or_else(|| own_url.clone()).ok_or_else(|| bad("no relay")))
            .collect::<LinkResult<Vec<_>>>()?;
        Ok(Self {
            mac,
            relays,
            own,
            secret,
        })
    }

    /// Reads what was scanned, typed or pasted: the short code, or the link older builds show.
    pub fn parse(text: &str) -> LinkResult<Self> {
        // spaces and line breaks are never part of either form (a wrapped paste)
        let joined: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        if joined.len() >= CODE_PREFIX.len()
            && joined.is_char_boundary(CODE_PREFIX.len())
            && joined[..CODE_PREFIX.len()].eq_ignore_ascii_case(CODE_PREFIX)
        {
            return Self::parse_code(&joined.to_ascii_uppercase());
        }
        let text = if joined
            .to_ascii_lowercase()
            .starts_with(&format!("{QR_SCHEME}://"))
        {
            joined.as_str()
        } else {
            text
        };
        let bad = |why: &str| {
            LinkError::new(
                errors::BAD_REQUEST,
                format!("This is not a FERB pairing code ({why})."),
            )
            .allow("Scan the code FERB shows on the Mac")
        };
        let u = url::Url::parse(text.trim()).map_err(|_| bad("not a link"))?;
        if u.scheme() != QR_SCHEME || u.host_str() != Some("pair") {
            return Err(bad("wrong kind of link"));
        }
        let (mut id, mut secret, mut relays, mut own, mut tok, mut ca, mut v) =
            (None, None, vec![], None, None, None, None);
        for (k, val) in u.query_pairs() {
            match &*k {
                "v" => v = Some(val.to_string()),
                "id" => id = Some(val.to_string()),
                "s" => secret = Some(val.to_string()),
                "r" => relays.push(val.to_string()),
                "own" => own = Some(val.to_string()),
                "tok" => tok = Some(val.to_string()),
                "ca" => ca = Some(val.to_string()),
                _ => {}
            }
        }
        if v.as_deref() != Some("2") {
            return Err(LinkError::new(
                errors::VERSION_MISMATCH,
                "This pairing code is from a different FERB version.",
            )
            .allow("Update the phone app and FERB on the Mac"));
        }
        let mac = EndpointId::from_str(&id.ok_or_else(|| bad("no Mac id"))?)
            .map_err(|_| bad("bad Mac id"))?;
        let secret: [u8; 32] = B64
            .decode(secret.ok_or_else(|| bad("no secret; codes without one are not accepted"))?)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| bad("bad secret"))?;
        Ok(Self {
            mac,
            relays,
            own: own.map(|url| RelaySpec {
                url,
                token: tok,
                ca,
            }),
            secret,
        })
    }
}

pub fn proof(secret: &[u8; 32], mac: EndpointId, phone: EndpointId) -> [u8; 32] {
    let mut h = blake3::Hasher::new_keyed(secret);
    h.update(b"ferb/phone/pair/1");
    h.update(mac.as_bytes());
    h.update(phone.as_bytes());
    *h.finalize().as_bytes()
}

/// A short code both screens show while the Mac asks: the same proof gives the same six digits, so the owner
/// can see that the phone asking is the one in his hand (a look-alike name does not have the same code).
pub fn code(proof: &[u8]) -> String {
    let h = blake3::hash(&[b"ferb/phone/code/1".as_slice(), proof].concat());
    let n = u32::from_be_bytes(h.as_bytes()[..4].try_into().unwrap()) % 1_000_000;
    format!("{:03} {:03}", n / 1000, n % 1000)
}

/// The open pairing window on the Mac: one secret, a deadline, a wrong-proof counter, used once.
pub struct Window {
    secret: [u8; 32],
    until: Instant,
    wrong: u32,
}

pub enum Checked {
    Right,
    Wrong { ended: bool },
}

impl Window {
    pub fn open(seconds: u64) -> (Self, [u8; 32]) {
        let mut secret = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rng(), &mut secret);
        (
            Self {
                secret,
                until: Instant::now() + Duration::from_secs(seconds),
                wrong: 0,
            },
            secret,
        )
    }
    pub fn expired(&self) -> bool {
        Instant::now() >= self.until
    }
    pub fn seconds_left(&self) -> u64 {
        self.until
            .saturating_duration_since(Instant::now())
            .as_secs()
    }
    pub fn check(
        &mut self,
        given: &[u8],
        mac: EndpointId,
        phone: EndpointId,
        wrong_max: u32,
    ) -> Checked {
        let want = proof(&self.secret, mac, phone);
        // constant-time comparison
        let same = given.len() == 32
            && given
                .iter()
                .zip(want.iter())
                .fold(0u8, |a, (x, y)| a | (x ^ y))
                == 0;
        if same {
            Checked::Right
        } else {
            self.wrong += 1;
            Checked::Wrong {
                ended: self.wrong >= wrong_max,
            }
        }
    }
}
