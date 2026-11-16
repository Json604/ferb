//! A phone's proofs: ECDSA P-256 over SHA-256, exactly as the phone lab verifies them (ferb-phone-link@57b3f34
//! `standin/p256.py`, read, not run): a key is SubjectPublicKeyInfo DER of an uncompressed point, or the bare 65-byte
//! point, in base64 of either alphabet with or without padding; a signature is DER (what Android's Keystore and the
//! iPhone's Secure Enclave give) or 64 raw bytes (r ‖ s), each 0 < r, s < n. A high s is accepted, as the lab accepts it:
//! Android does not normalize. Verification handles no secret.

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};

/// The SubjectPublicKeyInfo header of an uncompressed P-256 key (`p256.py` SPKI).
pub const SPKI: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

/// `b64_any`: either alphabet, padding optional, surrounding white space ignored; `None` when it isn't base64.
pub fn b64_any(text: &str) -> Option<Vec<u8>> {
    let t: String = text
        .trim()
        .chars()
        .map(|c| match c {
            '-' => '+',
            '_' => '/',
            c => c,
        })
        .collect();
    let pad = (4 - t.len() % 4) % 4;
    STANDARD.decode(format!("{t}{}", "=".repeat(pad))).ok()
}

/// URL-safe base64 without padding (`b64u`): challenges, the sealed answer's hash.
pub fn b64u(raw: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(raw)
}

/// `public_key`: the key a phone registered, when it is one.
pub fn key(b64: &str) -> Option<VerifyingKey> {
    let raw = b64_any(b64)?;
    let point = raw.strip_prefix(&SPKI[..]).unwrap_or(&raw);
    if point.len() != 65 || point[0] != 4 {
        return None;
    }
    VerifyingKey::from_sec1_bytes(point).ok()
}

/// `_signature`: (r, s) from DER as the lab reads it, or from 64 raw bytes.
fn signature(raw: &[u8]) -> Option<Signature> {
    let (r, s): (&[u8], &[u8]) = if raw.len() == 64 {
        (&raw[..32], &raw[32..])
    } else {
        if raw.len() < 6 || raw[0] != 0x30 || raw[1] as usize != raw.len() - 2 || raw[2] != 0x02 {
            return None;
        }
        let rl = raw[3] as usize;
        let at = 4 + rl;
        if raw.len() < at + 2 || raw[at] != 0x02 || raw[at + 1] as usize != raw.len() - 6 - rl {
            return None;
        }
        (&raw[4..at], &raw[at + 2..])
    };
    let scalar = |b: &[u8]| -> Option<[u8; 32]> {
        let b = &b[b.iter().take_while(|x| **x == 0).count()..];
        if b.len() > 32 {
            return None;
        }
        let mut out = [0u8; 32];
        out[32 - b.len()..].copy_from_slice(b);
        Some(out)
    };
    // from_scalars refuses 0 and anything at or above n
    Signature::from_scalars(scalar(r)?, scalar(s)?).ok()
}

/// `verify`: the signature over `payload` is this key's.
pub fn verify(key_b64: &str, payload: &[u8], sig_b64: &str) -> bool {
    let (Some(k), Some(raw)) = (key(key_b64), b64_any(sig_b64)) else {
        return false;
    };
    let Some(sig) = signature(&raw) else {
        return false;
    };
    // (r, s) and (r, n − s) verify alike; the lab takes either
    let sig = sig.normalize_s().unwrap_or(sig);
    k.verify(payload, &sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Signer;
    use p256::ecdsa::SigningKey;

    /// A fixed key (the scalar 1…32), its public half as SPKI DER base64, and RFC 6979 signatures: the same bytes on
    /// every run.
    fn fixed() -> (SigningKey, String) {
        let secret: [u8; 32] = core::array::from_fn(|i| i as u8 + 1);
        let sk = SigningKey::from_bytes(&secret.into()).unwrap();
        let point = sk.verifying_key().to_encoded_point(false);
        let mut spki = SPKI.to_vec();
        spki.extend_from_slice(point.as_bytes());
        (sk, STANDARD.encode(spki))
    }

    // decision: PHN-105
    // decision: PHN-443
    #[test]
    fn the_lab_s_key_and_signature_forms_verify() {
        let (sk, spki) = fixed();
        let payload = br#"{"challenge":"abc","method":"link.unlock","params":{}}"#;
        let sig: Signature = sk.sign(payload);
        let der = sig.to_der();
        // DER in standard base64, url-safe without padding, 64 raw bytes
        assert!(verify(&spki, payload, &STANDARD.encode(der.as_bytes())));
        assert!(verify(
            &spki,
            payload,
            &URL_SAFE_NO_PAD.encode(der.as_bytes())
        ));
        assert!(verify(&spki, payload, &STANDARD.encode(sig.to_bytes())));
        // the bare point is a key too; a compressed one is not (the lab reads only uncompressed points)
        let point = sk.verifying_key().to_encoded_point(false);
        assert!(verify(
            &STANDARD.encode(point.as_bytes()),
            payload,
            &STANDARD.encode(der.as_bytes())
        ));
        let compressed = sk.verifying_key().to_encoded_point(true);
        assert!(key(&STANDARD.encode(compressed.as_bytes())).is_none());
        // a high s (Android does not normalize) verifies as the lab's does
        let high = {
            let (r, s) = sig.split_scalars();
            Signature::from_scalars(r.to_bytes(), (-*s).to_bytes()).unwrap()
        };
        assert!(high.normalize_s().is_some() || sig.normalize_s().is_some());
        assert!(verify(
            &spki,
            payload,
            &STANDARD.encode(high.to_der().as_bytes())
        ));
        // another payload, another key, garbage
        assert!(!verify(&spki, b"{}", &STANDARD.encode(der.as_bytes())));
        let other = SigningKey::from_bytes(&[7u8; 32].into()).unwrap();
        let theirs: Signature = other.sign(payload);
        assert!(!verify(
            &spki,
            payload,
            &STANDARD.encode(theirs.to_der().as_bytes())
        ));
        assert!(!verify(&spki, payload, "not base64!"));
        assert!(!verify("AAAA", payload, &STANDARD.encode(der.as_bytes())));
        assert!(!verify(&spki, payload, &STANDARD.encode([0u8; 64])));
    }

    #[test]
    fn base64_of_either_alphabet_with_or_without_padding() {
        assert_eq!(b64_any("-_8"), Some(vec![0xfb, 0xff]));
        assert_eq!(b64_any("+/8="), Some(vec![0xfb, 0xff]));
        assert_eq!(b64_any("  Zm9v \n"), Some(b"foo".to_vec()));
        assert_eq!(b64_any("Zm9v!"), None);
        assert_eq!(b64u(&[0xfb, 0xff]), "-_8");
    }

    /// Signatures made on Android by the Keystore (P-256, SHA256withECDSA, DER; key as SubjectPublicKeyInfo), exactly
    /// as the phone app sends them (apps/android :core Proofs): the Mac verifies them, and refuses one over another
    /// payload.
    // decision: PHN-497
    #[test]
    fn android_keystore_signatures_verify() {
        let text = include_str!("../../../../fixtures/phone/android-signatures.json");
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        for c in v["vectors"].as_array().unwrap() {
            let ok = verify(
                c["key"].as_str().unwrap(),
                c["payload"].as_str().unwrap().as_bytes(),
                c["sig"].as_str().unwrap(),
            );
            assert_eq!(ok, c["valid"].as_bool().unwrap(), "{}", c["what"]);
        }
    }

    /// Signatures made by the iPhone app's FerbCore on the iOS simulator (a CryptoKit P-256 key through Proofs.swift:
    /// the raw point plus the SPKI header, DER signatures, the encoding the Secure Enclave's keys take on a phone):
    /// the Mac verifies them, and refuses one over another payload.
    // decision: PHN-561
    #[test]
    fn ios_signatures_verify() {
        let text = include_str!("../../../../fixtures/phone/ios-signatures.json");
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        for c in v["vectors"].as_array().unwrap() {
            let ok = verify(
                c["key"].as_str().unwrap(),
                c["payload"].as_str().unwrap().as_bytes(),
                c["sig"].as_str().unwrap(),
            );
            assert_eq!(ok, c["valid"].as_bool().unwrap(), "{}", c["what"]);
        }
    }
}
