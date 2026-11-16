//! One error type for every call the apps make, with the core's stable codes, and the boundary guard that turns a
//! panic into one of them.

use std::panic::{AssertUnwindSafe, catch_unwind};

use ferb_mobile::transport::{FailureKind, LinkFailure};
use ferb_mobile::{ErrorCode, MobileError};

/// Every failure a call returns. The variant is the stable code (`code()`: the core's `ErrorCode::as_str`, plus
/// `internal` and `bad_argument` of this layer); `detail` is the words to show; `link_code` the link's own code when it
/// came from there (ferb-link `errors.rs`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Error)]
pub enum FerbError {
    /// Forgotten or closed: open again.
    Closed {
        detail: String,
        link_code: Option<String>,
    },
    /// The store's key is not 32 bytes.
    BadKey {
        detail: String,
        link_code: Option<String>,
    },
    /// The store is not this key's.
    WrongKey {
        detail: String,
        link_code: Option<String>,
    },
    /// The store was written by a newer app.
    StoreNewer {
        detail: String,
        link_code: Option<String>,
    },
    /// The store failed.
    Store {
        detail: String,
        link_code: Option<String>,
    },
    /// No link to the Mac now: `link_code` says which failure.
    Offline {
        detail: String,
        link_code: Option<String>,
    },
    /// The public relay refuses this: available when connected directly or through the owner's relay.
    NeedsBetterPath {
        detail: String,
        link_code: Option<String>,
    },
    /// The phone must be unlocked toward the Mac first.
    NeedsUnlock {
        detail: String,
        link_code: Option<String>,
    },
    /// The Mac (or the link's rules) refused it: `link_code` and the words say why.
    Refused {
        detail: String,
        link_code: Option<String>,
    },
    /// A hidden answer needs the phone's key, and the call had no signer.
    NeedsSigner {
        detail: String,
        link_code: Option<String>,
    },
    /// The owner cancelled the fingerprint prompt (or the key is gone): nothing was sent.
    NotSigned {
        detail: String,
        link_code: Option<String>,
    },
    /// No such outbox message or question.
    NotFound {
        detail: String,
        link_code: Option<String>,
    },
    /// The message is being sent now: it can't be cancelled at this moment.
    InFlight {
        detail: String,
        link_code: Option<String>,
    },
    /// The Mac's reply could not be read.
    BadReply {
        detail: String,
        link_code: Option<String>,
    },
    /// Something the app handed over can't be used (the link's saved state is not what the link wrote).
    BadArgument {
        detail: String,
        link_code: Option<String>,
    },
    /// The library failed inside (a panic caught at the boundary); the object stays usable.
    Internal {
        detail: String,
        link_code: Option<String>,
    },
}

impl FerbError {
    /// The stable code.
    pub fn code(&self) -> &'static str {
        match self {
            FerbError::Closed { .. } => ErrorCode::Closed.as_str(),
            FerbError::BadKey { .. } => ErrorCode::BadKey.as_str(),
            FerbError::WrongKey { .. } => ErrorCode::WrongKey.as_str(),
            FerbError::StoreNewer { .. } => ErrorCode::StoreNewer.as_str(),
            FerbError::Store { .. } => ErrorCode::Store.as_str(),
            FerbError::Offline { .. } => ErrorCode::Offline.as_str(),
            FerbError::NeedsBetterPath { .. } => ErrorCode::NeedsBetterPath.as_str(),
            FerbError::NeedsUnlock { .. } => ErrorCode::NeedsUnlock.as_str(),
            FerbError::Refused { .. } => ErrorCode::Refused.as_str(),
            FerbError::NeedsSigner { .. } => ErrorCode::NeedsSigner.as_str(),
            FerbError::NotSigned { .. } => ErrorCode::NotSigned.as_str(),
            FerbError::NotFound { .. } => ErrorCode::NotFound.as_str(),
            FerbError::InFlight { .. } => ErrorCode::InFlight.as_str(),
            FerbError::BadReply { .. } => ErrorCode::BadReply.as_str(),
            FerbError::BadArgument { .. } => "bad_argument",
            FerbError::Internal { .. } => "internal",
        }
    }

    /// The words to show.
    pub fn detail(&self) -> &str {
        self.parts().0
    }

    /// The link's own code, when the failure came from the link.
    pub fn link_code(&self) -> Option<&str> {
        self.parts().1
    }

    fn parts(&self) -> (&str, Option<&str>) {
        match self {
            FerbError::Closed { detail, link_code }
            | FerbError::BadKey { detail, link_code }
            | FerbError::WrongKey { detail, link_code }
            | FerbError::StoreNewer { detail, link_code }
            | FerbError::Store { detail, link_code }
            | FerbError::Offline { detail, link_code }
            | FerbError::NeedsBetterPath { detail, link_code }
            | FerbError::NeedsUnlock { detail, link_code }
            | FerbError::Refused { detail, link_code }
            | FerbError::NeedsSigner { detail, link_code }
            | FerbError::NotSigned { detail, link_code }
            | FerbError::NotFound { detail, link_code }
            | FerbError::InFlight { detail, link_code }
            | FerbError::BadReply { detail, link_code }
            | FerbError::BadArgument { detail, link_code }
            | FerbError::Internal { detail, link_code } => (detail, link_code.as_deref()),
        }
    }

    pub(crate) fn closed() -> FerbError {
        FerbError::Closed {
            detail: "This phone's copy of FERB's chat is closed.".into(),
            link_code: None,
        }
    }

    pub(crate) fn link_closed() -> FerbError {
        FerbError::Closed {
            detail: "This link was closed.".into(),
            link_code: Some(ferb_mobile::transport::codes::LINK_CLOSED.into()),
        }
    }

    pub(crate) fn internal() -> FerbError {
        FerbError::Internal {
            detail: "FERB's phone library failed inside. Try again; if it keeps failing, close and open the app."
                .into(),
            link_code: None,
        }
    }

    pub(crate) fn bad_argument(detail: impl Into<String>) -> FerbError {
        FerbError::BadArgument {
            detail: detail.into(),
            link_code: None,
        }
    }

    /// A failure the link reported, by what its code means (ferb-mobile `transport::codes::kind`).
    pub(crate) fn from_link(f: LinkFailure) -> FerbError {
        let (detail, link_code) = (f.message.clone(), Some(f.code.clone()));
        match f.kind() {
            FailureKind::Transient => FerbError::Offline { detail, link_code },
            FailureKind::NeedsUnlock => FerbError::NeedsUnlock { detail, link_code },
            FailureKind::NeedsBetterPath => FerbError::NeedsBetterPath { detail, link_code },
            FailureKind::Refused => FerbError::Refused { detail, link_code },
        }
    }
}

impl From<MobileError> for FerbError {
    fn from(e: MobileError) -> FerbError {
        let MobileError {
            code,
            message: detail,
            link_code,
        } = e;
        match code {
            ErrorCode::Closed => FerbError::Closed { detail, link_code },
            ErrorCode::BadKey => FerbError::BadKey { detail, link_code },
            ErrorCode::WrongKey => FerbError::WrongKey { detail, link_code },
            ErrorCode::StoreNewer => FerbError::StoreNewer { detail, link_code },
            ErrorCode::Store => FerbError::Store { detail, link_code },
            ErrorCode::Offline => FerbError::Offline { detail, link_code },
            ErrorCode::NeedsBetterPath => FerbError::NeedsBetterPath { detail, link_code },
            ErrorCode::NeedsUnlock => FerbError::NeedsUnlock { detail, link_code },
            ErrorCode::Refused => FerbError::Refused { detail, link_code },
            ErrorCode::NeedsSigner => FerbError::NeedsSigner { detail, link_code },
            ErrorCode::NotSigned => FerbError::NotSigned { detail, link_code },
            ErrorCode::NotFound => FerbError::NotFound { detail, link_code },
            ErrorCode::InFlight => FerbError::InFlight { detail, link_code },
            ErrorCode::BadReply => FerbError::BadReply { detail, link_code },
        }
    }
}

impl std::fmt::Display for FerbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code(), self.detail())
    }
}

impl std::error::Error for FerbError {}

/// A failure of a transport the app supplies (`Transport`): the link's stable code (ferb-link `errors.rs`), its words,
/// what the owner may do, and when to try again.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Error)]
pub enum TransportError {
    Failed {
        code: String,
        detail: String,
        allow: Option<String>,
        retry_after_s: Option<u64>,
    },
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let TransportError::Failed { code, detail, .. } = self;
        write!(f, "{code}: {detail}")
    }
}

impl std::error::Error for TransportError {}

/// A foreign callback threw where it may not (UniFFI's `UnexpectedUniFFICallbackError`): the link failed inside.
impl From<uniffi::UnexpectedUniFFICallbackError> for TransportError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> TransportError {
        TransportError::Failed {
            code: ferb_mobile::transport::codes::LINK_INTERNAL_ERROR.into(),
            detail: format!("The app's transport failed ({}).", e.reason),
            allow: None,
            retry_after_s: None,
        }
    }
}

impl From<TransportError> for LinkFailure {
    fn from(e: TransportError) -> LinkFailure {
        let TransportError::Failed {
            code,
            detail,
            allow,
            retry_after_s,
        } = e;
        LinkFailure {
            code,
            message: detail,
            allow,
            retry_after_s,
        }
    }
}

/// Runs one boundary call: a panic inside becomes `FerbError::Internal`, never an abort of the app's process
/// (kb PHN-446). The objects' state is behind mutexes that recover from poisoning, so the object stays usable.
pub(crate) fn guard<T>(f: impl FnOnce() -> Result<T, FerbError>) -> Result<T, FerbError> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(FerbError::internal()))
}

/// The same for a call with no error to return: a panic gives `fallback`.
pub(crate) fn guard_or<T>(fallback: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(fallback)
}
