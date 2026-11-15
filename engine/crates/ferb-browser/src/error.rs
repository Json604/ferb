//! What a browser call raises: one of FERB's errors (`ferb_core::errors::FerbError`), Python's
//! `OutcomeUnknown` (a `DriverError` whose action may or may not have happened), and the
//! extension's step times when a read hung (`e.timing`, decision: HAND-436; never shown to a model).

use std::fmt;

use ferb_core::errors::FerbError;
use ferb_core::types::Dict;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Error {
    /// Boxed: a `Result` of this crate stays small.
    pub error: Box<FerbError>,
    /// `OutcomeUnknown`: the connection dropped, or the command timed out, while it was in flight.
    pub unknown: bool,
    /// `{"read": {step: ms}}` for a read that timed out (0.19.6).
    pub timing: Option<Value>,
    /// A `DriverError` subclass's name (`CaptureUnavailable`, HAND-497).
    pub subclass: Option<&'static str>,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn driver(message: impl Into<String>) -> Error {
        Error::driver_with(message, Dict::new())
    }

    pub fn driver_with(message: impl Into<String>, facts: Dict) -> Error {
        FerbError::DriverError {
            message: message.into(),
            facts,
        }
        .into()
    }

    pub fn unknown(message: impl Into<String>) -> Error {
        Error {
            unknown: true,
            ..Error::driver(message)
        }
    }

    /// `CaptureUnavailable`: the tab couldn't be captured for the owner's card (HAND-497).
    pub fn capture_unavailable(message: impl Into<String>) -> Error {
        Error {
            subclass: Some("CaptureUnavailable"),
            ..Error::driver(message)
        }
    }

    pub fn refused(message: impl Into<String>) -> Error {
        FerbError::Refused(message.into()).into()
    }

    pub fn stale(message: impl Into<String>) -> Error {
        FerbError::StaleRef(message.into()).into()
    }

    pub fn withheld(message: impl Into<String>) -> Error {
        FerbError::Withheld(message.into()).into()
    }

    /// The Python class name.
    pub fn class(&self) -> &'static str {
        if self.unknown {
            "OutcomeUnknown"
        } else if let Some(name) = self.subclass {
            name
        } else {
            self.error.class()
        }
    }

    pub fn message(&self) -> String {
        self.error.to_string()
    }

    /// A `DriverError`'s facts (empty for every other class).
    pub fn facts(&self) -> Option<&Dict> {
        match &*self.error {
            FerbError::DriverError { facts, .. } => Some(facts),
            _ => None,
        }
    }

    pub fn facts_mut(&mut self) -> Option<&mut Dict> {
        match &mut *self.error {
            FerbError::DriverError { facts, .. } => Some(facts),
            _ => None,
        }
    }

    /// `isinstance(e, DriverError)`.
    pub fn is_driver_error(&self) -> bool {
        matches!(*self.error, FerbError::DriverError { .. })
    }

    /// `e.facts.get("reason")`.
    pub fn reason(&self) -> Option<&str> {
        self.facts()
            .and_then(|f| f.get("reason"))
            .and_then(Value::as_str)
    }
}

impl From<FerbError> for Error {
    fn from(error: FerbError) -> Error {
        Error {
            error: Box::new(error),
            unknown: false,
            timing: None,
            subclass: None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for Error {}
