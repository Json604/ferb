//! A secret value in memory: zeroed when dropped, never printed. The one type for it in every crate:
//! `ferb_identity::Secret` re-exports it, and `ferb-core`'s escape grant holds it, so no crate below the identity
//! part needs a plain `String` for a secret.
//!
//! AGENTS.md rule 7: a secret never reaches a model, a trace, a log or the database. Python passes values as plain
//! `str`; here every value the helper hands over, or the owner types, is wrapped at the first point code holds it, so
//! no `Debug`, `Display` or error text can carry it.

use std::fmt;

use zeroize::Zeroizing;

/// What every `Debug` and `Display` of a [`Secret`] prints.
pub const MARKER: &str = "<secret>";

/// A secret's text. No `Clone`, no `PartialEq`, no serialization: the only way to its text is [`Secret::expose`],
/// which a reader can search for.
pub struct Secret(Zeroizing<String>);

impl Secret {
    /// Takes the string's own buffer (no copy is made), zeroed when the secret is dropped.
    pub fn new(value: String) -> Self {
        Secret(Zeroizing::new(value))
    }

    /// The value itself, for the one place that hands it on (the helper's stdin, a sealed fill).
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// A second copy, zeroed on its own drop. Explicit, so copies are countable.
    pub fn duplicate(&self) -> Secret {
        Secret(Zeroizing::new(self.0.as_str().to_owned()))
    }

    /// Whether the value is empty (no need to expose it to ask).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The same value, compared in time that depends only on the lengths (for a type that holds a secret and must
    /// keep `PartialEq`).
    pub fn same_as(&self, other: &Secret) -> bool {
        let (a, b) = (self.0.as_bytes(), other.0.as_bytes());
        a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Secret::new(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Secret::new(value.to_owned())
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(MARKER)
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(MARKER)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_prints_as_the_marker_only() {
        let secret = Secret::from("invented canary");
        assert_eq!(
            format!("{secret} {secret:?} {secret:#?}"),
            "<secret> <secret> <secret>"
        );
        assert_eq!(format!("{:?}", Some(secret.duplicate())), "Some(<secret>)");
        assert_eq!(secret.expose(), "invented canary");
    }

    #[test]
    fn two_secrets_compare_by_value() {
        let a = Secret::from("invented");
        assert!(a.same_as(&a.duplicate()));
        assert!(!a.same_as(&Secret::from("invented!")));
        assert!(!a.same_as(&Secret::from("inventeX")));
        assert!(Secret::from("").is_empty() && !a.is_empty());
    }
}
