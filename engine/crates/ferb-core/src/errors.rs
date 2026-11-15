//! Errors shared across FERB (ferb/core/errors.py). One enum; the Python class tree is kept as questions a caller
//! can ask (`is_refused`, `is_unsupported`), and `class` gives the Python class name traces and clients see.

use crate::types::Dict;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FerbError {
    /// The base class, raised as itself.
    #[error("{0}")]
    Ferb(String),
    /// Adapter startup failed; its message contains only safe, owner-facing diagnostics. (A ValueError in Python.)
    #[error("{0}")]
    RuntimeUnavailable(String),
    /// A guarantee this component can't provide; it fails explicitly instead of approximating.
    #[error("{0}")]
    Unsupported(String),
    /// A runtime can't report its model's documented context window and reply limit; never guessed.
    #[error("{0}")]
    ModelLimitsUnknown(String),
    /// Text FERB must send whole is longer than the runtime's documented per-input cap: refused before anything
    /// is sent (errors.py:25-33). `runtime`: the runtime it wasn't sent to, where the caller knows it.
    #[error("{what} is {chars} characters, more than the {max_chars} characters the model runtime accepts in one input; nothing was sent")]
    InputTooLarge {
        chars: u64,
        max_chars: u64,
        what: String,
        runtime: String,
    },
    /// The planner's answer still failed FERB's code checks after its one repair (errors.py:36-42); `problems`
    /// are code's own lines, told to the owner as they are. (A ValueError in Python.)
    #[error("The planner's answer still had problems after one repair: {}", .problems.join("; "))]
    PlanRejected { problems: Vec<String> },
    /// A connection or run token failed authentication or asked for a method its role doesn't allow.
    #[error("{0}")]
    AuthError(String),
    /// The daemon isn't running, or went away mid-request.
    #[error("{0}")]
    NotRunning(String),
    /// Another daemon is already answering on the socket.
    #[error("{0}")]
    AlreadyRunning(String),
    /// Work refused because the daemon is stopping; it is done again after the next start.
    #[error("{0}")]
    Deferred(String),
    /// A safety check said no. The message is safe to show a worker (never contains a secret).
    #[error("{0}")]
    Refused(String),
    /// A tab or element reference from an older snapshot, navigation or connection generation.
    #[error("{0}")]
    StaleRef(String),
    /// What was asked for isn't there right now; nothing was attempted.
    #[error("{0}")]
    NoTarget(String),
    /// A capture whose secret fields couldn't be painted over: only that image's pixels are withheld.
    #[error("{0}")]
    Withheld(String),
    /// A driver failed or needs an explicit route; `facts` contain observations, not command arguments.
    #[error("{message}")]
    DriverError { message: String, facts: Dict },
}

impl FerbError {
    /// errors.py:29: `what` defaults to "this call's context"; the runtime is filled in by the caller.
    pub fn input_too_large(chars: u64, max_chars: u64, what: Option<&str>) -> Self {
        FerbError::InputTooLarge {
            chars,
            max_chars,
            what: what.unwrap_or("this call's context").to_string(),
            runtime: String::new(),
        }
    }

    /// The Python class name.
    pub fn class(&self) -> &'static str {
        match self {
            FerbError::Ferb(_) => "FerbError",
            FerbError::RuntimeUnavailable(_) => "RuntimeUnavailable",
            FerbError::Unsupported(_) => "Unsupported",
            FerbError::ModelLimitsUnknown(_) => "ModelLimitsUnknown",
            FerbError::InputTooLarge { .. } => "InputTooLarge",
            FerbError::PlanRejected { .. } => "PlanRejected",
            FerbError::AuthError(_) => "AuthError",
            FerbError::NotRunning(_) => "NotRunning",
            FerbError::AlreadyRunning(_) => "AlreadyRunning",
            FerbError::Deferred(_) => "Deferred",
            FerbError::Refused(_) => "Refused",
            FerbError::StaleRef(_) => "StaleRef",
            FerbError::NoTarget(_) => "NoTarget",
            FerbError::Withheld(_) => "Withheld",
            FerbError::DriverError { .. } => "DriverError",
        }
    }

    /// `isinstance(e, Refused)`: Refused, StaleRef, NoTarget, Withheld.
    pub fn is_refused(&self) -> bool {
        matches!(
            self,
            FerbError::Refused(_)
                | FerbError::StaleRef(_)
                | FerbError::NoTarget(_)
                | FerbError::Withheld(_)
        )
    }

    /// `isinstance(e, Unsupported)`: Unsupported, ModelLimitsUnknown, InputTooLarge.
    pub fn is_unsupported(&self) -> bool {
        matches!(
            self,
            FerbError::Unsupported(_)
                | FerbError::ModelLimitsUnknown(_)
                | FerbError::InputTooLarge { .. }
        )
    }

    /// `isinstance(e, ValueError)`: the two classes that are not FerbErrors in Python.
    pub fn is_value_error(&self) -> bool {
        matches!(
            self,
            FerbError::RuntimeUnavailable(_) | FerbError::PlanRejected { .. }
        )
    }

    /// `isinstance(e, FerbError)`.
    pub fn is_ferb_error(&self) -> bool {
        !self.is_value_error()
    }
}
