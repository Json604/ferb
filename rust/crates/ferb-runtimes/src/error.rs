//! What an adapter raises, as the Python classes it raises (the orchestrator and the traces read the class and the
//! words). No variant ever holds a key, a token or a provider's whole error body.

use std::fmt;

use ferb_core::errors::FerbError;

/// An exception Python lets escape without a class of FERB's own (`KeyError`, `AttributeError`, `OSError`,
/// `TimeoutError`, ...): its class name and its `str()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raised {
    pub class: String,
    pub message: String,
}

impl Raised {
    pub fn new(class: &str, message: impl Into<String>) -> Self {
        Raised {
            class: class.to_string(),
            message: message.into(),
        }
    }

    /// `str(KeyError(key))`: the key's repr.
    pub fn key_error(key: &str) -> Self {
        Raised::new("KeyError", format!("'{key}'"))
    }

    /// `AttributeError` from `.get` on a value that isn't a dict.
    pub fn no_get(value: &serde_json::Value) -> Self {
        Raised::new(
            "AttributeError",
            format!("'{}' object has no attribute 'get'", py_type(value)),
        )
    }

    pub fn timeout() -> Self {
        Raised::new("TimeoutError", "")
    }
}

/// Python's type name of a decoded JSON value.
pub fn py_type(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "NoneType",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(n) if n.is_f64() => "float",
        serde_json::Value::Number(_) => "int",
        serde_json::Value::String(_) => "str",
        serde_json::Value::Array(_) => "list",
        serde_json::Value::Object(_) => "dict",
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeError {
    /// codex_stream.ProtocolError (a ValueError): Codex's own failure, marked.
    Protocol {
        message: String,
        overflow: bool,
        account: String,
        empty: bool,
    },
    /// codex_stream.AccountError: fixed categories and a numeric code only (codex_stream.py:125-136).
    Account { reason: String, code: Option<i64> },
    /// catalog.SignedOut (an Unsupported): FERB's own sign-in for `runtime` is missing.
    SignedOut { message: String, runtime: String },
    /// api.ProviderError: a provider failure worded without the key or the request.
    Provider {
        message: String,
        overflow: bool,
        account: String,
        image: bool,
        status: u16,
    },
    /// One of FERB's own errors (Unsupported, InputTooLarge, ModelLimitsUnknown, Refused, ...).
    Ferb(FerbError),
    /// Anything else Python would have let escape.
    Raised(Raised),
}

impl RuntimeError {
    pub fn protocol(message: impl Into<String>) -> Self {
        RuntimeError::Protocol {
            message: message.into(),
            overflow: false,
            account: String::new(),
            empty: false,
        }
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        RuntimeError::Ferb(FerbError::Unsupported(message.into()))
    }

    pub fn provider(message: impl Into<String>) -> Self {
        RuntimeError::Provider {
            message: message.into(),
            overflow: false,
            account: String::new(),
            image: false,
            status: 0,
        }
    }

    /// codex_stream.AccountError(reason, code).
    pub fn account_error(reason: &str, code: Option<i64>) -> Self {
        let reason = if reason == "auth_required" {
            "auth_required"
        } else {
            "account_unavailable"
        };
        RuntimeError::Account {
            reason: reason.to_string(),
            code,
        }
    }

    /// The Python class name.
    pub fn class(&self) -> String {
        match self {
            RuntimeError::Protocol { .. } => "ProtocolError".into(),
            RuntimeError::Account { .. } => "AccountError".into(),
            RuntimeError::SignedOut { .. } => "SignedOut".into(),
            RuntimeError::Provider { .. } => "ProviderError".into(),
            RuntimeError::Ferb(e) => e.class().into(),
            RuntimeError::Raised(r) => r.class.clone(),
        }
    }

    /// `account` as the orchestrator reads it (ORCH-315): "signed_out", "usage_limit" or "".
    pub fn account(&self) -> &str {
        match self {
            RuntimeError::Protocol { account, .. } | RuntimeError::Provider { account, .. } => {
                account
            }
            RuntimeError::Account { reason, .. } => {
                if reason == "auth_required" {
                    "signed_out"
                } else {
                    ""
                }
            }
            RuntimeError::SignedOut { .. } => "signed_out",
            _ => "",
        }
    }

    /// As `ferb_core`'s error, for the core trait.
    pub fn to_ferb(&self) -> FerbError {
        match self {
            RuntimeError::Ferb(e) => e.clone(),
            RuntimeError::SignedOut { message, .. } => FerbError::Unsupported(message.clone()),
            RuntimeError::Account { .. } => FerbError::RuntimeUnavailable(self.to_string()),
            other => FerbError::Ferb(other.to_string()),
        }
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeError::Protocol { message, .. } | RuntimeError::Provider { message, .. } => {
                f.write_str(message)
            }
            RuntimeError::Account { reason, code } => {
                let recovery = if reason == "auth_required" {
                    "Codex isn't signed in to a ChatGPT subscription for FERB: sign in to ChatGPT in Settings → AI."
                } else {
                    "Codex could not check the account. Try again shortly."
                };
                match code {
                    Some(c) => write!(f, "{recovery} ({reason}, code {c})"),
                    None => write!(f, "{recovery} ({reason})"),
                }
            }
            RuntimeError::SignedOut { message, .. } => f.write_str(message),
            RuntimeError::Ferb(e) => write!(f, "{e}"),
            RuntimeError::Raised(r) => f.write_str(&r.message),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<FerbError> for RuntimeError {
    fn from(e: FerbError) -> Self {
        RuntimeError::Ferb(e)
    }
}

impl From<Raised> for RuntimeError {
    fn from(r: Raised) -> Self {
        RuntimeError::Raised(r)
    }
}

impl From<std::io::Error> for RuntimeError {
    fn from(e: std::io::Error) -> Self {
        RuntimeError::Raised(os_error(&e))
    }
}

/// Python's class for an `OSError` of this kind.
pub fn os_error(e: &std::io::Error) -> Raised {
    use std::io::ErrorKind::*;
    let class = match e.kind() {
        NotFound => "FileNotFoundError",
        PermissionDenied => "PermissionError",
        BrokenPipe => "BrokenPipeError",
        ConnectionReset => "ConnectionResetError",
        AlreadyExists => "FileExistsError",
        _ => "OSError",
    };
    Raised::new(class, e.to_string())
}
