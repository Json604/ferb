import Foundation

/// The phone's three signing keys (kb PHN-443, PHN-133; the Mac's `device_pub`, `unlock_pub`, `strong_pub`), all
/// P-256, held where the app can use them but never read them: the Secure Enclave on a phone ([SecureEnclaveBackend]).
/// The same semantics as Android's Keystore keys (apps/android :core Keys.kt, kb PHN-497), kb PHN-561.
public enum KeyKind: String, CaseIterable, Sendable {
    /// No prompt: proves a connection comes from this very phone (`link.present`; notification words, PHN-126).
    case device
    /// After Face ID, Touch ID or the phone's passcode, each use: the Mac's unlock window, a private answer (PHN-125).
    case lock
    /// After Face ID or Touch ID only, each use; unusable once the enrolled faces or fingers change: a hard Yes,
    /// sensitive calls (PHN-137, PHN-129).
    case strong
}

public enum KeyState: Sendable, Equatable {
    /// Present and usable (a prompt may still be needed).
    case ok
    /// Not made: this phone could not hold it when the keys were made (no passcode, no Face ID or Touch ID).
    case none
    /// Made once and since lost: the passcode was turned off (iOS deletes the lock and strong keys), or (strong) the
    /// enrolled faces or fingers changed, or (strong, [Vault.keyHealth]) the Mac holds another strong key than this one.
    case gone
}

/// The public halves, in the Mac's wire form ([Proofs.publicKey]), as the pairing hello carries them.
public struct PublicKeys: Equatable, Sendable {
    public var device: String?
    public var lock: String?
    public var strong: String?
    public init(device: String?, lock: String?, strong: String?) {
        self.device = device; self.lock = lock; self.strong = strong
    }
}

/// Which keys work now. As the app reads it ([Vault.keyHealth]) `strong` is GONE also when this phone's Face ID / Touch
/// ID key is not the one the Mac holds (one made for an update the owner didn't approve): the Mac would refuse its
/// signature. `strongOnMac`: the Mac holds a Face ID / Touch ID key for this phone, so a new one can be given to it with
/// Update fingerprint (PHN-849).
public struct KeyHealth: Equatable, Sendable {
    public var device: KeyState
    public var lock: KeyState
    public var strong: KeyState
    public var strongOnMac: Bool
    public init(device: KeyState, lock: KeyState, strong: KeyState, strongOnMac: Bool = false) {
        self.device = device; self.lock = lock; self.strong = strong; self.strongOnMac = strongOnMac
    }

    /// The Mac can no longer be unlocked from this phone (its lock key is gone, or there never was one) or no longer
    /// recognises it as this phone: pair again (after turning a passcode on).
    public var pairAgain: Bool { lock != .ok || device != .ok }

    /// Only the Face ID / Touch ID key is gone, or the Mac holds another one ([Vault.keyHealth]): chat works; a hard Yes
    /// and sensitive changes wait for Update fingerprint (PHN-849).
    public var fingerprintsChanged: Bool { strong == .gone }

    /// The Face ID / Touch ID changed and the Mac can take a new key without pairing again: Update fingerprint.
    public var updateFingerprint: Bool { fingerprintsChanged && strongOnMac && !pairAgain }
}

/// Where the private halves live and how they sign: the Secure Enclave on a phone, CryptoKit in software on the
/// simulator (it has no Secure Enclave) and in tests. Both give the raw point and a DER signature, which [Proofs]
/// turns into the wire form, so the simulator exercises the same encoding as a phone.
public protocol KeyBackend: AnyObject, Sendable {
    /// Whether this phone can hold `kind` now: the lock key needs a passcode, the strong key enrolled biometrics.
    func canHold(_ kind: KeyKind) -> Bool
    /// Makes the key (replacing any of that kind) and returns its uncompressed point (65 bytes).
    func make(_ kind: KeyKind) throws -> Data
    /// The key's point, or nil when there is none.
    func point(_ kind: KeyKind) -> Data?
    /// ECDSA P-256 over SHA-256 of `message`, DER. The lock and strong keys show the system prompt with `reason`
    /// (blocking the calling thread); nil when the owner cancelled or the key can't sign.
    func sign(_ kind: KeyKind, _ message: Data, reason: String) -> Data?
    func delete(_ kind: KeyKind)
    /// What the system says of the enrolled faces or fingers (LAContext.evaluatedPolicyDomainState): it changes when
    /// one is added or removed. Nil without biometrics.
    func biometryState() -> Data?
}

/// The three keys over a [KeyBackend], with a small record of which were made (so a lost one reads as GONE, not
/// NONE) and of the enrolled biometrics when the strong key was made (so a change reads as GONE without a prompt,
/// as Android's KeyPermanentlyInvalidatedException does). The record holds nothing secret.
public final class PhoneKeys: @unchecked Sendable {
    public let backend: KeyBackend
    private let recordURL: URL
    private let lock = NSLock()

    public init(backend: KeyBackend, record: URL) {
        self.backend = backend
        self.recordURL = record
    }

    private struct Record: Codable {
        var made: [String] = []
        var biometry: Data?
    }

    private func readRecord() -> Record {
        guard let data = try? Data(contentsOf: recordURL), let r = try? JSONDecoder().decode(Record.self, from: data) else { return Record() }
        return r
    }

    private func writeRecord(_ r: Record) {
        if let data = try? JSONEncoder().encode(r) { try? data.write(to: recordURL, options: .atomic) }
    }

    private func state(_ kind: KeyKind, _ r: Record) -> KeyState {
        let made = r.made.contains(kind.rawValue)
        guard backend.point(kind) != nil else { return made ? .gone : .none }
        if kind == .strong, let was = r.biometry, backend.biometryState() != was { return .gone }
        return .ok
    }

    public func health() -> KeyHealth {
        lock.lock(); defer { lock.unlock() }
        let r = readRecord()
        return KeyHealth(device: state(.device, r), lock: state(.lock, r), strong: state(.strong, r))
    }

    /// Makes each key this phone can hold now and is missing; leaves a GONE one as it is. The public halves.
    @discardableResult
    public func ensure() throws -> PublicKeys {
        lock.lock(); defer { lock.unlock() }
        var r = readRecord()
        for kind in KeyKind.allCases where state(kind, r) == .none && backend.canHold(kind) {
            _ = try backend.make(kind)
            if !r.made.contains(kind.rawValue) { r.made.append(kind.rawValue) }
            if kind == .strong { r.biometry = backend.biometryState() }
        }
        writeRecord(r)
        return publicKeysLocked(r)
    }

    /// Deletes every GONE key, then [ensure]: fresh keys for a pairing (never for a phone already paired).
    @discardableResult
    public func renew() throws -> PublicKeys {
        lock.lock()
        var r = readRecord()
        for kind in KeyKind.allCases where state(kind, r) == .gone {
            backend.delete(kind)
            r.made.removeAll { $0 == kind.rawValue }
            if kind == .strong { r.biometry = nil }
        }
        writeRecord(r)
        lock.unlock()
        return try ensure()
    }

    /// The fingerprint-only key for Update fingerprint (PHN-849; owner 2026-11-28): a GONE one (the enrolled faces or
    /// fingers changed) is deleted and made anew; a usable one is kept, as it may be one an earlier update made that the
    /// Mac never took. Its public half, or nil when this phone can't hold one now (no Face ID or Touch ID enrolled).
    /// Only the strong key changes: the device and lock keys the Mac holds stay.
    public func strongForUpdate() throws -> String? {
        lock.lock(); defer { lock.unlock() }
        var r = readRecord()
        let now = state(.strong, r)
        if now != .ok {
            guard backend.canHold(.strong) else { return nil }
            if now == .gone { backend.delete(.strong) }
            _ = try backend.make(.strong)
            if !r.made.contains(KeyKind.strong.rawValue) { r.made.append(KeyKind.strong.rawValue) }
            r.biometry = backend.biometryState()
            writeRecord(r)
        }
        return publicKeysLocked(r).strong
    }

    /// The Face ID / Touch ID key's public half when it is usable, else nil. No prompt.
    public func strongPublic() -> String? { publicKeys().strong }

    /// The public halves of the keys that are OK now.
    public func publicKeys() -> PublicKeys {
        lock.lock(); defer { lock.unlock() }
        return publicKeysLocked(readRecord())
    }

    private func publicKeysLocked(_ r: Record) -> PublicKeys {
        func pub(_ k: KeyKind) -> String? { state(k, r) == .ok ? backend.point(k).flatMap { Proofs.publicKey(point: $0) } : nil }
        return PublicKeys(device: pub(.device), lock: pub(.lock), strong: pub(.strong))
    }

    /// The device key's signature over `payload` in the wire form, or nil. No prompt.
    public func deviceSign(_ payload: String) -> String? {
        backend.sign(.device, Data(payload.utf8), reason: "").flatMap { Proofs.signature(der: $0) }
    }

    /// `kind`'s signature over `payload` in the wire form after the system prompt with `reason`; nil when cancelled
    /// or the key is gone (a strong key whose biometrics changed never signs).
    public func sign(_ kind: KeyKind, _ payload: String, reason: String) -> String? {
        if health().state(of: kind) != .ok { return nil }
        return backend.sign(kind, Data(payload.utf8), reason: reason).flatMap { Proofs.signature(der: $0) }
    }

    /// Deletes all three and the record.
    public func deleteAll() {
        lock.lock(); defer { lock.unlock() }
        for kind in KeyKind.allCases { backend.delete(kind) }
        try? FileManager.default.removeItem(at: recordURL)
    }
}

extension KeyHealth {
    public func state(of kind: KeyKind) -> KeyState {
        switch kind {
        case .device: device
        case .lock: lock
        case .strong: strong
        }
    }
}
