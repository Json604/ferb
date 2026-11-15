//! Authenticated encryption of secret fills and the mail feed's text: a port of
//! `ferb/identity/seal.py`. HKDF-SHA256 derives a per-install AES-256-GCM key from the pairing
//! key; the command id and the allowed origin are bound to every sealed payload as associated
//! data; every seal has a fresh nonce. It protects transport and logs, not against malware
//! running as the owner.
//!
//! In the daemon a fill is sealed by the signed helper ([`crate::Helper::seal`],
//! `ferb/daemon/areas/browsers.py:54`), so the value never crosses this process in the clear
//! longer than the request buffer lives; [`seal`] is the same construction for the callers
//! Python gives `seal.seal`, and [`unseal`] opens what the extension sealed (the mail feed,
//! `ferb/act/browser/ext_bridge.py:3332`).

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

use crate::{py, Secret};

/// `seal.py:12`.
pub const INFO: &[u8] = b"ferb seal v1";

/// A sealed payload as it travels: both parts standard base64 (`seal.py:26`). `ct` ends with the
/// 16-byte tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub nonce: String,
    pub ct: String,
}

/// Why nothing was sealed or opened. Python raises `ValueError` / `InvalidTag` here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealError {
    /// The key is not 32 bytes (the only size `seal_key` makes).
    Key,
    /// Not base64, not a 12-byte nonce, a failed tag, or text that is not UTF-8.
    Seal,
    /// The system gave no random bytes.
    Random,
}

/// `seal.py:15-16`: the seal key of a pairing key.
pub fn seal_key(pairing_key: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut key = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(None, pairing_key)
        .expand(INFO, &mut key[..])
        .expect("32 bytes is a valid HKDF-SHA256 length");
    key
}

/// `seal.py:19-20`.
pub fn aad(command_id: &str, origin: &str) -> Vec<u8> {
    format!("{command_id}|{origin}").into_bytes()
}

/// `seal.py:23-26`.
pub fn seal(
    key: &[u8],
    value: &Secret,
    command_id: &str,
    origin: &str,
) -> Result<Sealed, SealError> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| SealError::Key)?;
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut nonce).map_err(|_| SealError::Random)?;
    let ct = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: value.expose().as_bytes(),
                aad: &aad(command_id, origin),
            },
        )
        .map_err(|_| SealError::Seal)?;
    Ok(Sealed {
        nonce: py::b64encode(&nonce),
        ct: py::b64encode(&ct),
    })
}

/// `seal.py:29-32`: open a seal made for this command and origin.
pub fn unseal(
    key: &[u8],
    sealed: &Sealed,
    command_id: &str,
    origin: &str,
) -> Result<Secret, SealError> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| SealError::Key)?;
    let nonce = py::b64decode(&sealed.nonce)
        .filter(|n| n.len() == 12)
        .ok_or(SealError::Seal)?;
    let ct = py::b64decode(&sealed.ct).ok_or(SealError::Seal)?;
    let plain = cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &ct,
                aad: &aad(command_id, origin),
            },
        )
        .map_err(|_| SealError::Seal)?;
    match String::from_utf8(plain) {
        Ok(text) => Ok(Secret::new(text)),
        Err(e) => {
            e.into_bytes().zeroize();
            Err(SealError::Seal)
        }
    }
}
