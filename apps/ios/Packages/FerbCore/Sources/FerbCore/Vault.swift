import Foundation
import LocalAuthentication

/// Why a kept pairing was given up (everything was wiped; the app pairs again).
public enum Lost: String, Sendable {
    /// The Keychain no longer holds the link's state while this install kept it: it was reset under the app.
    case keychainReset = "KEYCHAIN_RESET"
    /// The link refused the kept state as not its own (ferb-mobile-ffi `bad_argument`).
    case stateRefused = "STATE_REFUSED"
}

public struct VaultLost: Error, Sendable {
    public let why: Lost
}

/// The Keychain can't be read now (the phone was not unlocked since it started): try again after an unlock.
public struct VaultNotReady: Error, Sendable {}

/// What the phone keeps, where, under what (kb PHN-448, PHN-494, PHN-561; the twin of Android's Vault, PHN-496).
/// - The Keychain ([secrets], AfterFirstUnlockThisDeviceOnly, never synchronized): `link.state` (the link's state as
///   `LinkListener.persist` hands it: the phone's PRIVATE link key and where the Mac is) and `store.key` (the 32 random
///   bytes that encrypt the SQLCipher store). iOS encrypts Keychain items with keys of its own Secure Enclave, so there
///   is no wrap key of the app's to manage, as Android needs.
/// - One folder, [dir] (Application Support/ferb), excluded from backups (`isExcludedFromBackup`, so neither iCloud
///   nor a computer backup nor a move to a new phone carries it) with the Data Protection class
///   completeUntilFirstUserAuthentication (the store is written while the screen may be locked): `phone.db` (and its
///   -wal, -shm: the store, SQLCipher-encrypted anyway), `client.id` (this install's id; not secret), `keys.mac` (the
///   public keys the Mac holds), `keys.made` (which signing keys were made; [PhoneKeys]), `link.kept` (a mark that this
///   install kept a link state).
/// - The signing keys: [keys], in the Secure Enclave.
///
/// The store is a copy of the Mac's (ENG-10): a store key that can't be read costs a refetch. Keychain items outlive
/// the app's removal on iOS (Android's Keystore keys don't): a new install finds no `client.id` and deletes every item
/// and key first, so an earlier install's link key and keys are never reused.
public final class Vault: @unchecked Sendable {
    public let dir: URL
    public let keys: PhoneKeys
    private let secrets: SecretStore

    public var storeFile: URL { dir.appendingPathComponent("phone.db") }
    private var idFile: URL { dir.appendingPathComponent("client.id") }
    private var registeredFile: URL { dir.appendingPathComponent("keys.mac") }
    private var keptMark: URL { dir.appendingPathComponent("link.kept") }

    private static let storeKey = "store.key"
    private static let linkState = "link.state"

    public init(dir: URL, secrets: SecretStore, keys: PhoneKeys) {
        self.dir = dir
        self.secrets = secrets
        self.keys = keys
        Self.prepare(dir)
        if !FileManager.default.fileExists(atPath: idFile.path) {
            // a new install (or Forget's wipe): nothing an earlier install left in the Keychain is used
            secrets.deleteAll()
            keys.deleteAll()
            for name in ["phone.db", "phone.db-wal", "phone.db-shm", "phone.db-journal", "keys.mac", "link.kept"] {
                try? FileManager.default.removeItem(at: dir.appendingPathComponent(name))
            }
        }
    }

    /// The folder, out of every backup, readable after the first unlock.
    static func prepare(_ dir: URL) {
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true,
                                                 attributes: [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication])
        var u = dir
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? u.setResourceValues(values)
    }

    public var isExcludedFromBackup: Bool {
        (try? dir.resourceValues(forKeys: [.isExcludedFromBackupKey]).isExcludedFromBackup) ?? false
    }

    public func clientId() -> String {
        if let s = try? String(contentsOf: idFile, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines), !s.isEmpty {
            return s
        }
        let id = UUID().uuidString.lowercased()
        write(Data(id.utf8), to: idFile)
        return id
    }

    /// The store's key, or a new one (and the old store dropped) when none can be read. The caller zeroes it.
    public func storeKey() throws -> Data {
        if let k = secrets.get(Self.storeKey), k.count == 32 { return k }
        if secrets.exists(Self.storeKey) == nil { throw VaultNotReady() }
        dropStore()
        var key = Data(count: 32)
        let ok = key.withUnsafeMutableBytes { SecRandomCopyBytes(kSecRandomDefault, 32, $0.baseAddress!) } == errSecSuccess
        guard ok else { throw KeyError.store(errSecAllocate) }
        try secrets.set(Self.storeKey, key)
        return key
    }

    /// The store's files and its key.
    public func dropStore() {
        for suffix in ["", "-journal", "-wal", "-shm"] {
            try? FileManager.default.removeItem(at: dir.appendingPathComponent("phone.db" + suffix))
        }
        secrets.delete(Self.storeKey)
    }

    /// The kept link state, or nil (never started here). Throws [VaultLost] when this install kept one that is gone.
    public func linkState() throws -> Data? {
        if let s = secrets.get(Self.linkState) { return s }
        guard let exists = secrets.exists(Self.linkState) else { throw VaultNotReady() }
        if exists { throw VaultNotReady() }
        if FileManager.default.fileExists(atPath: keptMark.path) { throw VaultLost(why: .keychainReset) }
        return nil
    }

    public func saveLinkState(_ state: Data) throws {
        try secrets.set(Self.linkState, state)
        if !FileManager.default.fileExists(atPath: keptMark.path) { write(Data(), to: keptMark) }
    }

    /// The public keys the paired Mac was given, or nil when none were recorded. Public halves only.
    public func registeredKeys() -> PublicKeys? {
        guard let text = try? String(contentsOf: registeredFile, encoding: .utf8) else { return nil }
        let l = text.components(separatedBy: "\n")
        func at(_ i: Int) -> String? { i < l.count && !l[i].isEmpty && l[i] != "-" ? l[i] : nil }
        return PublicKeys(device: at(0), lock: at(1), strong: at(2))
    }

    public func saveRegisteredKeys(_ k: PublicKeys) {
        write(Data([k.device, k.lock, k.strong].map { $0 ?? "-" }.joined(separator: "\n").utf8), to: registeredFile)
    }

    /// "Forget this Mac" (PHN-408, PHN-646): the store, its key, the three signing keys and the link's state, its
    /// identity and pairing with it: a forgotten Mac is never dialled again, and the next pairing is a new phone to it.
    public func forget() {
        dropStore()
        secrets.delete(Self.linkState)
        for f in [registeredFile, keptMark] { try? FileManager.default.removeItem(at: f) }
        keys.deleteAll()
    }

    /// Paired, with the lock key the Mac knows (PHN-649): the opening prompt can sign an unlock statement at once.
    public func statementReady() -> Bool {
        (secrets.exists(Self.linkState) ?? false) && registeredKeys()?.lock != nil && keys.health().lock == .ok
    }

    /// Everything: the next start is a first start.
    public func wipe() {
        dropStore()
        secrets.deleteAll()
        keys.deleteAll()
        for f in [idFile, registeredFile, keptMark] { try? FileManager.default.removeItem(at: f) }
    }

    private func write(_ data: Data, to url: URL) {
        try? data.write(to: url, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }

    /// The phone's vault: Application Support/ferb, the Keychain, the Secure Enclave's keys (software keys on the
    /// simulator, which has none; [SoftwareBackend] says what that means).
    public static func forPhone() -> Vault {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        let dir = support.appendingPathComponent("ferb", isDirectory: true)
        prepare(dir)
        let backend: KeyBackend
        #if targetEnvironment(simulator)
        backend = SoftwareBackend(
            store: KeychainStore(service: "com.ferb.phone.simulator-keys"),
            holds: { kind in
                switch kind {
                case .device: true
                case .lock: Biometry.screenLockSet
                case .strong: Biometry.current != .none
                }
            },
            biometry: { Biometry.enrolledState() },
            authenticate: { kind, reason in
                // the UI tests' stand-in (`-FERBUITestUnlock YES`, Debug on the simulator only): a UI test can't
                // answer the system's sheet, and the opening prompt's statement is signed through this key too, so
                // an install that already had keys showed "Enter iPhone Passcode" in a test run (kb PHN-685)
                #if DEBUG
                if UserDefaults.standard.bool(forKey: "FERBUITestUnlock") { return true }
                #endif
                return Prompt.evaluate(biometricOnly: kind == .strong, reason: reason)
            })
        #else
        backend = SecureEnclaveBackend()
        #endif
        let keys = PhoneKeys(backend: backend, record: dir.appendingPathComponent("keys.made"))
        return Vault(dir: dir, secrets: KeychainStore(service: "com.ferb.phone.vault"), keys: keys)
    }
}

/// The system prompt alone (no key): the app opening while the Mac can't be reached, and the simulator's stand-in
/// for a Secure Enclave key's own prompt. Blocks the calling thread: never call it on the main thread.
public enum Prompt {
    public static func evaluate(biometricOnly: Bool, reason: String) -> Bool {
        let c = LAContext()
        let policy: LAPolicy = biometricOnly ? .deviceOwnerAuthenticationWithBiometrics : .deviceOwnerAuthentication
        let done = DispatchSemaphore(value: 0)
        nonisolated(unsafe) var ok = false
        c.evaluatePolicy(policy, localizedReason: reason) { success, _ in
            ok = success
            done.signal()
        }
        done.wait()
        return ok
    }

    /// The same, for async callers.
    public static func evaluate(reason: String) async -> Bool {
        await withCheckedContinuation { cont in
            LAContext().evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason) { success, _ in cont.resume(returning: success) }
        }
    }
}
