//! The errors the Mac hands raise (the Python classes they stand for: ferb/core/errors.py, input.py `Yielded` and
//! `PointerInterrupted`, accessibility.py `NoAnswer`, screenshot.py `ScreenOff`, `CaptureBusy`, `CaptureError`).

use ferb_core::errors::FerbError;

use crate::ax::NoAnswer;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum HandError {
    /// A safety check or a precondition said no; nothing was sent.
    #[error("{0}")]
    Refused(String),
    /// An element or screenshot reference from an older look.
    #[error("{0}")]
    StaleRef(String),
    /// What was asked for isn't there; nothing was attempted.
    #[error("{0}")]
    NoTarget(String),
    /// A capture whose secret fields couldn't be painted: its pixels are withheld.
    #[error("{0}")]
    Withheld(String),
    /// Foreground input stopped for the owner part-way (input.py:23): outcome unknown.
    #[error("stopped for the owner after {done} of {total} steps ({what}); the owner used the mouse/keyboard")]
    Yielded {
        done: usize,
        total: usize,
        what: String,
    },
    /// Some pointer input may have landed: the outcome stays unknown (input.py:30).
    #[error("{0}")]
    PointerInterrupted(String),
    /// An AX action the app didn't answer: it may have happened.
    #[error("{0}")]
    NoAnswer(#[from] NoAnswer),
    /// The screen is locked or asleep (HAND-437).
    #[error("the screen is {0}, and macOS lets no app capture windows until it's back: FERB can still read and act through Accessibility")]
    ScreenOff(String),
    /// A live preview's capture found another under way (DMN-406).
    #[error("{0}")]
    CaptureBusy(String),
    /// A ScreenCaptureKit failure with its domain and code (I7).
    #[error("{message}")]
    Capture {
        message: String,
        domain: String,
        code: Option<i64>,
    },
    /// The capture didn't answer in time.
    #[error("{0}")]
    Timeout(String),
    /// Anything else a driver or the OS reported.
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, HandError>;

impl HandError {
    /// The Python class name traces see.
    pub fn class(&self) -> &'static str {
        match self {
            HandError::Refused(_) => "Refused",
            HandError::StaleRef(_) => "StaleRef",
            HandError::NoTarget(_) => "NoTarget",
            HandError::Withheld(_) => "Withheld",
            HandError::Yielded { .. } => "Yielded",
            HandError::PointerInterrupted(_) => "PointerInterrupted",
            HandError::NoAnswer(_) => "NoAnswer",
            HandError::ScreenOff(_) => "ScreenOff",
            HandError::CaptureBusy(_) => "CaptureBusy",
            HandError::Capture { .. } => "CaptureError",
            HandError::Timeout(_) => "TimeoutError",
            HandError::Other(_) => "RuntimeError",
        }
    }

    /// Whether some input may have been delivered (the gateway records the outcome as unknown).
    pub fn outcome_unknown(&self) -> bool {
        matches!(
            self,
            HandError::Yielded { .. } | HandError::PointerInterrupted(_) | HandError::NoAnswer(_)
        )
    }
}

impl From<HandError> for FerbError {
    fn from(e: HandError) -> FerbError {
        match e {
            HandError::Refused(m) => FerbError::Refused(m),
            HandError::StaleRef(m) => FerbError::StaleRef(m),
            HandError::NoTarget(m) => FerbError::NoTarget(m),
            HandError::Withheld(m) => FerbError::Withheld(m),
            other => FerbError::Ferb(other.to_string()),
        }
    }
}

pub fn refused<T>(m: impl Into<String>) -> Result<T> {
    Err(HandError::Refused(m.into()))
}

pub fn stale<T>(m: impl Into<String>) -> Result<T> {
    Err(HandError::StaleRef(m.into()))
}
