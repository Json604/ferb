//! A secret value in memory: zeroed when dropped, never printed.
//!
//! AGENTS.md rule 7: a secret never reaches a model, a trace, a log or the database. Python
//! passes values as plain `str`; here every value the helper hands over, or the owner types, is
//! wrapped at the first point code holds it, so no `Debug`, `Display` or error text can carry it.

use std::fmt;

use zeroize::Zeroizing;

/// What every `Debug` and `Display` of a [`Secret`] prints.
pub const MARKER: &str = "<secret>";

/// A secret's text. No `Clone`, no `PartialEq`, no serialization: the only way to its text is
/// [`Secret::expose`], which a reader can search for.
pub struct Secret(Zeroizing<String>);

impl Secret {
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
}
