//! A secret value in memory: zeroed when dropped, never printed.
//!
//! AGENTS.md rule 7: a secret never reaches a model, a trace, a log or the database. Python
//! passes values as plain `str`; here every value the helper hands over, or the owner types, is
//! wrapped at the first point code holds it, so no `Debug`, `Display` or error text can carry it.

// The type itself lives in ferb-py, the lowest crate, so `ferb-core` (which this crate does not see) holds the same
// one: `ferb_identity::Secret` and `ferb_py::secret::Secret` are one type.
pub use ferb_py::secret::{Secret, MARKER};

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
