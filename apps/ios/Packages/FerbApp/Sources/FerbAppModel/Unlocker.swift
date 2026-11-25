import FerbCore
import FerbMobile
import Foundation

/// What a prompt is for: its words (the system prompt's one line of the app's, `subtitleKey` in the catalog).
public enum Why: Sendable, CaseIterable {
    case open, yes, file, setting, keys, insomnia
    /// Turning off the app's lock (kb PHN-846): the Mac's signed call, the Face ID key where the phone has one.
    case lockOff

    public var subtitleKey: String {
        switch self {
        case .open: "prompt_unlock_subtitle"
        case .yes: "prompt_yes_subtitle"
        case .file: "prompt_file_subtitle"
        case .setting: "prompt_setting_subtitle"
        case .keys: "prompt_keys_subtitle"
        case .insomnia: "prompt_insomnia_subtitle"
        case .lockOff: "prompt_lock_off_subtitle"
        }
    }

    /// The words the system prompt shows (Face ID's sheet names the app itself).
    public var reason: String { L.bio(subtitleKey) }
}

/// The one path every call to the Mac takes when it may need the owner (PHN-13, kb PHN-521): a call refused because
/// the Mac's unlock window closed is unlocked (one prompt, the lock key) and made once more; a call that needs the
/// phone's fresh proof (a key place, a sensitive change, PHN-121, PHN-129, PHN-133) is signed by the Face ID / Touch
/// ID key where the phone has one, else the lock key. Nothing here decides what needs a proof: the Mac says so, or the
/// listing marks the entry (`locked`) and the proof is asked up front. (Android: model/Unlocker.kt.)
@MainActor
public final class Unlocker {
    /// The Mac's codes for "this call needs the phone's own proof" (not "the window closed").
    public static let proofCodes: Set<String> = ["unlock_required", "unlock_invalid", "fresh_unlock_required"]

    /// The app's own code: the key that would sign is gone (the enrolled faces or fingers changed): pair again.
    public static let pairAgain = "pair_again"

    private let api: PhoneApi
    private let keys: () -> KeyHealth?
    private let signer: (KeyKind, Why) -> Signer?

    public init(api: PhoneApi, keys: @escaping () -> KeyHealth?, signer: @escaping (KeyKind, Why) -> Signer?) {
        self.api = api
        self.keys = keys
        self.signer = signer
    }

    /// The key for a fresh proof, or nil: this phone's keys changed and it must pair again.
    public func freshKind() -> KeyKind? {
        guard let k = keys() else { return nil }
        if k.strong == .ok { return .strong }
        if k.strong == .gone { return nil }
        if k.lock == .ok { return .lock }
        return nil
    }

    /// A signer for a fresh proof, or nil (pair again; [pairAgain]).
    public func fresh(_ why: Why) -> Signer? { freshKind().flatMap { signer($0, why) } }

    /// `block`, and once more after the Mac's unlock when the Mac's window had closed.
    public func run<T>(_ block: () async -> Outcome<T>) async -> Outcome<T> {
        let r = await block()
        guard case .failed(let e) = r, case .NeedsUnlock = e, !Self.proofCodes.contains(e.linkCode ?? "") else { return r }
        guard let s = signer(.lock, .open) else { return r }
        if case .failed(let u) = await api.unlock(s) { return .failed(u) }
        return await block()
    }

    /// `block` with a fresh proof: asked up front when `upfront` (the listing marked the place), else only when the
    /// Mac asks for one. Each try with a signer is its own prompt (the Mac's challenge is one-time).
    public func withProof<T>(_ upfront: Bool, _ why: Why, _ block: (Signer?) async -> Outcome<T>) async -> Outcome<T> {
        var proof: Signer?
        if upfront {
            guard let s = fresh(why) else { return Self.pairAgainFailure() }
            proof = s
        }
        let first = proof
        let r = await run { await block(first) }
        guard proof == nil, case .failed(let e) = r, case .NeedsUnlock = e, Self.proofCodes.contains(e.linkCode ?? "") else { return r }
        guard let s = fresh(why) else { return Self.pairAgainFailure() }
        return await run { await block(s) }
    }

    static func pairAgainFailure<T>() -> Outcome<T> { .failed(.NeedsSigner(detail: "Pair this phone again.", linkCode: pairAgain)) }
}
