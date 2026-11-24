//! PHN-828 (owner, 2026-11-24: "cant we enforce matching emails???"): a phone pairs only with a Mac signed in to the
//! same FERB account, matched on the account id, never the email text. The phone carries FERB's site's signed statement
//! of its account (the pairing assertion, `/api/app/assertion` for its own link endpoint) in the pairing hello; the link
//! helper only passes it on (PHN-441); this module checks it against the Mac's own account before the owner is asked.
//! PHN-102's QR proof and PHN-124's owner's Yes are unchanged: this is one more check before them.
//!
//! The statement: `fpa1.<b64url(payload)>.<b64url(signature)>`, payload JSON
//! `{"v":1,"kid","aud":"ferb-pair","sub":<account id>,"email","dev":<phone endpoint id>,"iat","exp"}`, signed by ECDSA
//! P-256 / SHA-256 over the ASCII bytes `ferb-pair/v1.` + the payload's base64 exactly as sent, raw r‖s (64 bytes).
//! The site signs it; this build trusts only the public keys pinned in [`PINNED_KEYS`], by kid. Nothing at run time adds
//! a trusted key: no setting, file or variable reaches [`AccountCheck`]; a test constructs one with its own key.

use std::sync::Arc;

use base64::alphabet::URL_SAFE;
use base64::engine::general_purpose::GeneralPurposeConfig;
use base64::engine::{DecodePaddingMode, GeneralPurpose};
use base64::Engine;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use serde_json::Value;

/// The statement's text starts so (its format's version).
pub const PREFIX: &str = "fpa1.";
/// What the site signs: these bytes, then the payload's base64 exactly as sent.
pub const SIGNED_PREFIX: &str = "ferb-pair/v1.";
/// The statement's audience: a FERB pairing, nothing else the site signs.
pub const AUDIENCE: &str = "ferb-pair";
/// The site's clock may run this far ahead of the Mac's (`iat` at most now + 60 s).
pub const SKEW_S: i64 = 60;
/// A statement lives at most this long (the site gives 300 s).
pub const MAX_LIFE_S: i64 = 600;
/// The longest statement read (the link helper takes 4096 bytes at most).
pub const MAX_LEN: usize = 4096;

/// FERB's site's public keys for pairing statements, by kid: uncompressed SEC1 points (65 bytes, `04 ‖ X ‖ Y`) in
/// standard base64. Rotation: add the new key, ship, then remove the old one.
///
/// EMPTY in this build: the owner generates the key pair (the private half is the site's `APP_ASSERTION_KEY`, its kid
/// `APP_ASSERTION_KID`) and pastes the public half here, as `("<kid>", "<base64 of the 65-byte point>")`. While it is
/// empty this build can't confirm any phone's account, so every pairing is refused with `account_unconfirmed`.
pub const PINNED_KEYS: &[(&str, &str)] = &[
    // ("<APP_ASSERTION_KID>", "<uncompressed SEC1 public key, base64>"),
];

/// The FERB account FERB on this Mac is signed in to (FERB.app's `account.save`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacAccount {
    pub id: String,
    pub email: String,
}

/// Reads the Mac's account now; `None` while FERB on this Mac isn't signed in.
pub type MacAccountRead = Arc<dyn Fn() -> Option<MacAccount> + Send + Sync>;

/// What a statement says, once checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirmed {
    /// The account id (`sub`).
    pub account_id: String,
    /// The account's email, as the site signed it (for the mismatch's words only).
    pub email: String,
}

/// Why a pairing is refused before the owner is asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountRefused {
    /// The statement couldn't be confirmed; the reason is a structural word for the journal, never shown.
    Unconfirmed(&'static str),
    /// FERB on this Mac isn't signed in.
    MacSignedOut,
    /// The phone and the Mac are signed in to different accounts.
    Mismatch {
        phone_email: String,
        mac_email: String,
    },
}

impl AccountRefused {
    /// The refusal's code (`ferb_rpc::phone_gate`), which the link gives the phone as its own.
    pub fn code(&self) -> i64 {
        use ferb_rpc::phone_gate as code;
        match self {
            AccountRefused::Unconfirmed(_) => code::ACCOUNT_UNCONFIRMED,
            AccountRefused::MacSignedOut => code::MAC_SIGNED_OUT,
            AccountRefused::Mismatch { .. } => code::ACCOUNT_MISMATCH,
        }
    }

    /// The link's name for it (the journal's `why`).
    pub fn name(&self) -> &'static str {
        match self {
            AccountRefused::Unconfirmed(_) => "account_unconfirmed",
            AccountRefused::MacSignedOut => "mac_signed_out",
            AccountRefused::Mismatch { .. } => "account_mismatch",
        }
    }

    /// The words the phone shows (the spec's, exactly).
    pub fn words(&self) -> String {
        match self {
            AccountRefused::Unconfirmed(_) => "FERB couldn't confirm which account this phone is signed in to. Check \
                                               that the phone is online, then scan a new code on the Mac."
                .into(),
            AccountRefused::MacSignedOut => {
                "FERB on this Mac isn't signed in yet. Sign in on the Mac, then scan a new code.".into()
            }
            AccountRefused::Mismatch {
                phone_email,
                mac_email,
            } => format!(
                "This phone is signed in as {phone_email}, but FERB on this Mac is signed in as {mac_email}. Sign in \
                 with the same account on both."
            ),
        }
    }
}

/// The pairing's account check: the trusted keys and the Mac's account. Built by ferbd from [`PINNED_KEYS`]
/// ([`AccountCheck::pinned`]) or by a test with its own key ([`AccountCheck::new`]); nothing else makes one.
#[derive(Clone)]
pub struct AccountCheck {
    keys: Vec<(String, VerifyingKey)>,
    mac: MacAccountRead,
}

impl std::fmt::Debug for AccountCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountCheck")
            .field(
                "kids",
                &self.keys.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl AccountCheck {
    /// The keys this build pins ([`PINNED_KEYS`]) and the Mac's account.
    pub fn pinned(mac: MacAccountRead) -> AccountCheck {
        AccountCheck::new(PINNED_KEYS, mac)
    }

    /// These keys (`(kid, uncompressed SEC1 point in base64)`) and the Mac's account. A key that isn't a P-256 point is
    /// left out (the pinned list's own test parses every one).
    pub fn new(keys: &[(&str, &str)], mac: MacAccountRead) -> AccountCheck {
        AccountCheck {
            keys: keys
                .iter()
                .filter_map(|(kid, b64)| Some((kid.to_string(), public_key(b64)?)))
                .collect(),
            mac,
        }
    }

    /// Whether this build can confirm any phone's account (a key is pinned).
    pub fn can_confirm(&self) -> bool {
        !self.keys.is_empty()
    }

    /// The Mac's account now.
    pub fn mac_account(&self) -> Option<MacAccount> {
        (self.mac)()
    }

    /// Checks the phone's statement for a pairing from `endpoint` at `now` (unix seconds) against `mac` (the Mac's
    /// account, read by the caller off the async runtime). Order: no key pinned, the Mac signed out, the statement
    /// itself, then the account ids.
    pub fn check(
        &self,
        statement: Option<&str>,
        endpoint: &str,
        now: i64,
        mac: Option<&MacAccount>,
    ) -> Result<Confirmed, AccountRefused> {
        if self.keys.is_empty() {
            return Err(AccountRefused::Unconfirmed("no_key"));
        }
        let Some(mac) = mac else {
            return Err(AccountRefused::MacSignedOut);
        };
        let confirmed = self
            .verify(statement, endpoint, now)
            .map_err(AccountRefused::Unconfirmed)?;
        if confirmed.account_id != mac.id {
            return Err(AccountRefused::Mismatch {
                phone_email: confirmed.email,
                mac_email: mac.email.clone(),
            });
        }
        Ok(confirmed)
    }

    /// The statement's signature and claims; `Err(why)` with a structural word.
    pub fn verify(
        &self,
        statement: Option<&str>,
        endpoint: &str,
        now: i64,
    ) -> Result<Confirmed, &'static str> {
        let text = statement.map(str::trim).unwrap_or_default();
        if text.is_empty() {
            return Err("missing");
        }
        if text.len() > MAX_LEN {
            return Err("too_long");
        }
        let rest = text.strip_prefix(PREFIX).ok_or("format")?;
        let (payload_b64, sig_b64) = rest.split_once('.').ok_or("format")?;
        if sig_b64.contains('.') || payload_b64.is_empty() || sig_b64.is_empty() {
            return Err("format");
        }
        let payload = b64url(payload_b64).ok_or("format")?;
        let sig = b64url(sig_b64).ok_or("format")?;
        let claims: Value = serde_json::from_slice(&payload).map_err(|_| "format")?;
        let claims = claims.as_object().ok_or("format")?;
        let kid = claims.get("kid").and_then(Value::as_str).ok_or("kid")?;
        let key = self
            .keys
            .iter()
            .find(|(k, _)| k == kid)
            .map(|(_, key)| key)
            .ok_or("kid")?;
        if sig.len() != 64 {
            return Err("signature");
        }
        let sig = Signature::from_slice(&sig).map_err(|_| "signature")?;
        let sig = sig.normalize_s().unwrap_or(sig);
        let mut signed = Vec::with_capacity(SIGNED_PREFIX.len() + payload_b64.len());
        signed.extend_from_slice(SIGNED_PREFIX.as_bytes());
        signed.extend_from_slice(payload_b64.as_bytes());
        key.verify(&signed, &sig).map_err(|_| "signature")?;
        // the claims, once the signature is the site's
        if claims.get("v").and_then(Value::as_i64) != Some(1) {
            return Err("version");
        }
        if claims.get("aud").and_then(Value::as_str) != Some(AUDIENCE) {
            return Err("audience");
        }
        if claims.get("dev").and_then(Value::as_str) != Some(endpoint) {
            return Err("other_phone");
        }
        let iat = claims.get("iat").and_then(Value::as_i64).ok_or("time")?;
        let exp = claims.get("exp").and_then(Value::as_i64).ok_or("time")?;
        if iat > now + SKEW_S || exp <= iat || exp - iat > MAX_LIFE_S {
            return Err("time");
        }
        if now >= exp {
            return Err("expired");
        }
        let sub = claims
            .get("sub")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= 64)
            .ok_or("subject")?;
        let email = claims
            .get("email")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|e| !e.is_empty() && e.len() <= 320)
            .ok_or("email")?;
        Ok(Confirmed {
            account_id: sub.to_string(),
            email: email.to_string(),
        })
    }
}

/// Base64url, padding optional (the site sends none).
fn b64url(text: &str) -> Option<Vec<u8>> {
    const ENGINE: GeneralPurpose = GeneralPurpose::new(
        &URL_SAFE,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
    );
    ENGINE.decode(text).ok()
}

/// A pinned key: an uncompressed P-256 point (or its SubjectPublicKeyInfo) in base64.
fn public_key(b64: &str) -> Option<VerifyingKey> {
    super::proof::key(b64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key this build pins is a P-256 point, each kid once (an empty list is allowed: no key yet).
    // decision: PHN-828
    #[test]
    fn every_pinned_key_parses_and_each_kid_is_named_once() {
        let mut kids = std::collections::BTreeSet::new();
        for (kid, b64) in PINNED_KEYS {
            assert!(public_key(b64).is_some(), "{kid}");
            assert!(kids.insert(*kid), "{kid} twice");
            assert!(!kid.trim().is_empty());
        }
        let check = AccountCheck::pinned(Arc::new(|| None));
        assert_eq!(check.can_confirm(), !PINNED_KEYS.is_empty());
    }
}
