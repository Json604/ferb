//! Sealed secret fills and the sealed mail feed: HKDF-SHA256 of the pairing key (empty salt, info
//! "ferb seal v1") makes an AES-256-GCM key; the additional data is `commandId|origin`, so a sealed
//! value opens only for that command and that origin (protocol.js `seal`/`unseal`). The
//! construction is `ferb_identity::seal` (ported from `ferb/identity/seal.py`); this module is how
//! the bridge uses it, checked against both Python and protocol.js run under node
//! (`rust/fixtures/browser/crypto.json`).

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
pub use ferb_identity::seal::{aad, seal_key, unseal, SealError, Sealed};
use ferb_identity::Secret;
use serde_json::{json, Value};

/// Seals a fill for one command and origin under the pairing key (Python: `seal(seal_key(key),
/// value, cid, origin)`), with a fresh nonce.
pub fn seal_fill(
    pairing_key: &[u8],
    value: &Secret,
    command_id: &str,
    origin: &str,
) -> Result<Sealed, SealError> {
    ferb_identity::seal::seal(&seal_key(pairing_key)[..], value, command_id, origin)
}

/// The same seal under a given nonce: for the fixtures only (a nonce must never repeat).
pub fn seal_with_nonce(
    seal_key: &[u8],
    nonce: &[u8; 12],
    value: &str,
    command_id: &str,
    origin: &str,
) -> Result<Sealed, SealError> {
    let cipher = Aes256Gcm::new_from_slice(seal_key).map_err(|_| SealError::Key)?;
    let ct = cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: value.as_bytes(),
                aad: &aad(command_id, origin),
            },
        )
        .map_err(|_| SealError::Seal)?;
    Ok(Sealed {
        nonce: ferb_identity::py::b64encode(nonce),
        ct: ferb_identity::py::b64encode(&ct),
    })
}

/// A seal as it travels in a command's arguments: `{"nonce": ..., "ct": ...}`.
pub fn wire(sealed: &Sealed) -> Value {
    json!({"nonce": sealed.nonce, "ct": sealed.ct})
}

/// A seal from a reply (`v["sealed"]`); `None` when it isn't one.
pub fn from_wire(v: &Value) -> Option<Sealed> {
    Some(Sealed {
        nonce: v.get("nonce")?.as_str()?.to_string(),
        ct: v.get("ct")?.as_str()?.to_string(),
    })
}
