import CryptoKit
import Foundation
import LocalAuthentication
import Security

/// The phone's keys in the Secure Enclave (kb PHN-561): P-256, made inside it, never readable by anyone, the app
/// included. Each key's access control is the twin of Android's (PHN-497):
/// - device: `.privateKeyUsage` alone, AfterFirstUnlockThisDeviceOnly: no prompt, usable while the phone is locked
///   (the link proves a new session by itself, PHN-493).
/// - lock: `.privateKeyUsage` + `.userPresence` (Face ID, Touch ID or the passcode, each use: a new LAContext each
///   time, no reuse window), WhenPasscodeSetThisDeviceOnly: made only with a passcode; iOS deletes it when the
///   passcode is turned off. Not tied to the enrolled biometrics (Android's lock key is kept across new fingerprints).
/// - strong: `.privateKeyUsage` + `.biometryCurrentSet` (Face ID or Touch ID only, each use), WhenPasscodeSet:
///   unusable once a face or finger is added or removed, and deleted with the passcode.
/// ThisDeviceOnly: never in iCloud Keychain, never restored to another phone. The Secure Enclave's keys are not in
/// any backup at all.
public final class SecureEnclaveBackend: KeyBackend, @unchecked Sendable {
    private let prefix: String

    public init(prefix: String = "com.ferb.phone.key.") {
        self.prefix = prefix
    }

    public static var isAvailable: Bool { SecureEnclave.isAvailable }

    private func tag(_ kind: KeyKind) -> Data { Data((prefix + kind.rawValue).utf8) }

    public func canHold(_ kind: KeyKind) -> Bool {
        guard SecureEnclave.isAvailable else { return false }
        switch kind {
        case .device: return true
        case .lock: return LAContext().canEvaluatePolicy(.deviceOwnerAuthentication, error: nil)
        case .strong: return LAContext().canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: nil)
        }
    }

    public func make(_ kind: KeyKind) throws -> Data {
        delete(kind)
        let (accessible, flags): (CFString, SecAccessControlCreateFlags) = switch kind {
        case .device: (kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly, [.privateKeyUsage])
        case .lock: (kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly, [.privateKeyUsage, .userPresence])
        case .strong: (kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly, [.privateKeyUsage, .biometryCurrentSet])
        }
        var error: Unmanaged<CFError>?
        guard let access = SecAccessControlCreateWithFlags(nil, accessible, flags, &error) else {
            throw KeyError.make(kind, error?.takeRetainedValue())
        }
        let attributes: [CFString: Any] = [
            kSecAttrKeyType: kSecAttrKeyTypeECSECPrimeRandom,
            kSecAttrKeySizeInBits: 256,
            kSecAttrTokenID: kSecAttrTokenIDSecureEnclave,
            kSecPrivateKeyAttrs: [
                kSecAttrIsPermanent: true,
                kSecAttrApplicationTag: tag(kind),
                kSecAttrAccessControl: access,
            ] as [CFString: Any],
        ]
        guard let key = SecKeyCreateRandomKey(attributes as CFDictionary, &error),
              let point = Self.point(of: key) else {
            throw KeyError.make(kind, error?.takeRetainedValue())
        }
        return point
    }

    private func key(_ kind: KeyKind, context: LAContext) -> SecKey? {
        let query: [CFString: Any] = [
            kSecClass: kSecClassKey,
            kSecAttrApplicationTag: tag(kind),
            kSecAttrKeyType: kSecAttrKeyTypeECSECPrimeRandom,
            kSecAttrKeyClass: kSecAttrKeyClassPrivate,
            kSecReturnRef: true,
            kSecUseAuthenticationContext: context,
        ]
        var item: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess, let item else { return nil }
        return (item as! SecKey)
    }

    public func point(_ kind: KeyKind) -> Data? {
        let quiet = LAContext()
        quiet.interactionNotAllowed = true   // reading the public half never prompts
        return key(kind, context: quiet).flatMap(Self.point(of:))
    }

    public func sign(_ kind: KeyKind, _ message: Data, reason: String) -> Data? {
        let context = LAContext()
        context.localizedReason = reason
        context.touchIDAuthenticationAllowableReuseDuration = 0   // each use is its own prompt (PHN-497)
        guard let key = key(kind, context: context) else { return nil }
        var error: Unmanaged<CFError>?
        // The prompt (lock, strong) appears inside this call, on whichever thread made it: never the main one.
        let der = SecKeyCreateSignature(key, .ecdsaSignatureMessageX962SHA256, message as CFData, &error)
        return der as Data?
    }

    public func delete(_ kind: KeyKind) {
        SecItemDelete([kSecClass: kSecClassKey, kSecAttrApplicationTag: tag(kind)] as CFDictionary)
    }

    public func biometryState() -> Data? { Biometry.enrolledState() }

    static func point(of privateKey: SecKey) -> Data? {
        guard let pub = SecKeyCopyPublicKey(privateKey) else { return nil }
        return SecKeyCopyExternalRepresentation(pub, nil) as Data?
    }
}

public enum KeyError: Error, CustomStringConvertible {
    case make(KeyKind, CFError?)
    case store(OSStatus)

    public var description: String {
        switch self {
        case .make(let kind, let e): "the \(kind) key could not be made: \(e.map { CFErrorCopyDescription($0) as String } ?? "unknown")"
        case .store(let status): "the Keychain said \(status)"
        }
    }
}

/// Software P-256 keys (CryptoKit) for the SIMULATOR, which has no Secure Enclave, and for tests: the same points, the
/// same DER signatures, so the wire form is exercised exactly as on a phone; only where the private half lives
/// differs (a Keychain item or memory, readable by the app). `authenticate` stands in for the prompt the Secure
/// Enclave would show (the simulator's Face ID can be enrolled and matched from its Features menu); nil signs without
/// asking (tests). A phone never uses this: [Vault.forPhone] picks the Secure Enclave whenever it exists.
public final class SoftwareBackend: KeyBackend, @unchecked Sendable {
    private let store: SecretStore
    private let holds: @Sendable (KeyKind) -> Bool
    private let biometry: @Sendable () -> Data?
    private let authenticate: (@Sendable (KeyKind, String) -> Bool)?

    public init(store: SecretStore, holds: @escaping @Sendable (KeyKind) -> Bool = { _ in true },
                biometry: @escaping @Sendable () -> Data? = { nil },
                authenticate: (@Sendable (KeyKind, String) -> Bool)? = nil) {
        self.store = store
        self.holds = holds
        self.biometry = biometry
        self.authenticate = authenticate
    }

    private func name(_ kind: KeyKind) -> String { "key." + kind.rawValue }

    private func load(_ kind: KeyKind) -> P256.Signing.PrivateKey? {
        store.get(name(kind)).flatMap { try? P256.Signing.PrivateKey(rawRepresentation: $0) }
    }

    public func canHold(_ kind: KeyKind) -> Bool { holds(kind) }

    public func make(_ kind: KeyKind) throws -> Data {
        let key = P256.Signing.PrivateKey()
        try store.set(name(kind), key.rawRepresentation)
        return key.publicKey.x963Representation
    }

    public func point(_ kind: KeyKind) -> Data? { load(kind)?.publicKey.x963Representation }

    public func sign(_ kind: KeyKind, _ message: Data, reason: String) -> Data? {
        guard let key = load(kind) else { return nil }
        if kind != .device, let authenticate, !authenticate(kind, reason) { return nil }
        return try? key.signature(for: message).derRepresentation
    }

    public func delete(_ kind: KeyKind) { store.delete(name(kind)) }

    public func biometryState() -> Data? { biometry() }
}
