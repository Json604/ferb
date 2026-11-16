import Foundation
import LocalAuthentication
import Security

/// Small secrets by name: the Keychain on the phone ([KeychainStore]), memory in tests.
public protocol SecretStore: AnyObject, Sendable {
    func get(_ name: String) -> Data?
    /// Whether the item exists: nil when that can't be told now (the phone not unlocked since it started).
    func exists(_ name: String) -> Bool?
    func set(_ name: String, _ value: Data) throws
    func delete(_ name: String)
    func deleteAll()
}

/// Generic-password items of one service, AfterFirstUnlockThisDeviceOnly and never synchronizable (kb PHN-561):
/// - readable once the phone was unlocked after its start, so the link and the store open while the screen is locked
///   (iOS may run FERB briefly in the background then; Android's wrap key needs no authentication for the same
///   reason, PHN-496);
/// - ThisDeviceOnly: not in iCloud Keychain, and an encrypted backup restores them only to this very phone (never to
///   a new one); the files they open are left out of every backup ([Vault]), so a restored copy holds nothing usable.
/// Keychain items outlive the app's removal on iOS: [Vault] deletes them at the first start of a new install.
public final class KeychainStore: SecretStore, @unchecked Sendable {
    private let service: String

    public init(service: String) {
        self.service = service
    }

    private func base(_ name: String) -> [CFString: Any] {
        [kSecClass: kSecClassGenericPassword, kSecAttrService: service, kSecAttrAccount: name,
         kSecAttrSynchronizable: false]
    }

    public func get(_ name: String) -> Data? {
        var q = base(name)
        q[kSecReturnData] = true
        q[kSecMatchLimit] = kSecMatchLimitOne
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess else { return nil }
        return out as? Data
    }

    public func exists(_ name: String) -> Bool? {
        var q = base(name)
        q[kSecMatchLimit] = kSecMatchLimitOne
        switch SecItemCopyMatching(q as CFDictionary, nil) {
        case errSecSuccess: return true
        case errSecItemNotFound: return false
        default: return nil
        }
    }

    public func set(_ name: String, _ value: Data) throws {
        let found = SecItemUpdate(base(name) as CFDictionary, [kSecValueData: value] as CFDictionary)
        if found == errSecSuccess { return }
        guard found == errSecItemNotFound else { throw KeyError.store(found) }
        var add = base(name)
        add[kSecValueData] = value
        add[kSecAttrAccessible] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let status = SecItemAdd(add as CFDictionary, nil)
        guard status == errSecSuccess else { throw KeyError.store(status) }
    }

    public func delete(_ name: String) {
        SecItemDelete(base(name) as CFDictionary)
    }

    public func deleteAll() {
        SecItemDelete([kSecClass: kSecClassGenericPassword, kSecAttrService: service] as CFDictionary)
    }
}

/// Secrets in memory (tests).
public final class MemoryStore: SecretStore, @unchecked Sendable {
    private var items: [String: Data] = [:]
    private let lock = NSLock()
    public init() {}
    public func get(_ name: String) -> Data? { lock.lock(); defer { lock.unlock() }; return items[name] }
    public func exists(_ name: String) -> Bool? { get(name) != nil }
    public func set(_ name: String, _ value: Data) throws { lock.lock(); items[name] = value; lock.unlock() }
    public func delete(_ name: String) { lock.lock(); items[name] = nil; lock.unlock() }
    public func deleteAll() { lock.lock(); items = [:]; lock.unlock() }
    public var names: [String] { lock.lock(); defer { lock.unlock() }; return items.keys.sorted() }
}

/// What the phone opens with: Face ID, Touch ID (or Optic ID), or the passcode alone. The app's words name it (the
/// Android app says "fingerprint" where this says the phone's own kind).
public enum Biometry: Sendable, Equatable {
    case faceID, touchID, opticID, none

    /// The kind this phone has enrolled now (none: only the passcode, or no screen lock at all).
    public static var current: Biometry {
        let c = LAContext()
        guard c.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: nil) else { return .none }
        switch c.biometryType {
        case .faceID: return .faceID
        case .touchID: return .touchID
        case .opticID: return .opticID
        default: return .none
        }
    }

    /// PHN-8: a phone without a passcode cannot use the app.
    public static var screenLockSet: Bool { LAContext().canEvaluatePolicy(.deviceOwnerAuthentication, error: nil) }

    /// The system's fingerprint of the enrolled faces or fingers: it changes when one is added or removed.
    static func enrolledState() -> Data? {
        let c = LAContext()
        guard c.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: nil) else { return nil }
        if #available(iOS 18.0, *) { return c.domainState.biometry.stateHash }
        return c.evaluatedPolicyDomainState
    }
}
