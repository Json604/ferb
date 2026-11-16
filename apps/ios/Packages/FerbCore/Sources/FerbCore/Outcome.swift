import Foundation
import FerbMobile

/// A call's result: the value, or the library's typed error (one case per stable code).
public enum Outcome<T> {
    case ok(T)
    case failed(FerbError)

    public var value: T? {
        if case .ok(let v) = self { return v }
        return nil
    }

    public var error: FerbError? {
        if case .failed(let e) = self { return e }
        return nil
    }
}

extension Outcome: Sendable where T: Sendable {}

extension FerbError {
    /// The stable code (ferb-mobile `ErrorCode::as_str`, ferb-mobile-ffi `FerbError::code`). A new code fails this
    /// build (the switch is exhaustive), as Android's Outcome.kt does.
    public var code: String {
        switch self {
        case .Closed: "closed"
        case .BadKey: "bad_key"
        case .WrongKey: "wrong_key"
        case .StoreNewer: "store_newer"
        case .Store: "store"
        case .Offline: "offline"
        case .NeedsBetterPath: "needs_better_path"
        case .NeedsUnlock: "needs_unlock"
        case .Refused: "refused"
        case .NeedsSigner: "needs_signer"
        case .NotSigned: "not_signed"
        case .NotFound: "not_found"
        case .InFlight: "in_flight"
        case .BadReply: "bad_reply"
        case .Cancelled: "cancelled"
        case .BadArgument: "bad_argument"
        case .Internal: "internal"
        }
    }

    private var parts: (detail: String, linkCode: String?) {
        switch self {
        case .Closed(let d, let l), .BadKey(let d, let l), .WrongKey(let d, let l), .StoreNewer(let d, let l),
             .Store(let d, let l), .Offline(let d, let l), .NeedsBetterPath(let d, let l), .NeedsUnlock(let d, let l),
             .Refused(let d, let l), .NeedsSigner(let d, let l), .NotSigned(let d, let l), .NotFound(let d, let l),
             .InFlight(let d, let l), .BadReply(let d, let l), .Cancelled(let d, let l), .BadArgument(let d, let l),
             .Internal(let d, let l):
            (d, l)
        }
    }

    /// The words the library gave.
    public var detail: String { parts.detail }

    /// The link's own code (ferb-link errors.rs), when the failure came from there.
    public var linkCode: String? { parts.linkCode }
}
