import Foundation
import XCTest
@testable import FerbCore

final class VaultTests: XCTestCase {
    private var dir: URL!

    override func setUpWithError() throws {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-vault-\(UUID().uuidString)/ferb")
    }

    override func tearDownWithError() throws { try? FileManager.default.removeItem(at: dir.deletingLastPathComponent()) }

    private func vault(_ secrets: MemoryStore) -> Vault {
        Vault(dir: dir, secrets: secrets, keys: PhoneKeys(backend: SoftwareBackend(store: secrets), record: dir.appendingPathComponent("keys.made")))
    }

    // decision: PHN-448, PHN-494, PHN-561
    func testTheFolderIsOutOfBackupsAndTheSecretsAreKeychainItems() throws {
        let secrets = MemoryStore()
        let v = vault(secrets)
        XCTAssertTrue(v.isExcludedFromBackup, "no backup or move to a new phone carries the folder")
        let id = v.clientId()
        XCTAssertEqual(v.clientId(), id, "one id per install")
        let key = try v.storeKey()
        XCTAssertEqual(key.count, 32)
        XCTAssertEqual(try v.storeKey(), key)
        XCTAssertNil(try v.linkState(), "never started here")
        try v.saveLinkState(Data("{\"invented\":\"state\"}".utf8))
        XCTAssertEqual(try v.linkState(), Data("{\"invented\":\"state\"}".utf8))
        XCTAssertEqual(secrets.names, ["link.state", "store.key"], "the two secrets live only in the Keychain")
        for name in try FileManager.default.contentsOfDirectory(atPath: dir.path) {
            let bytes = try Data(contentsOf: dir.appendingPathComponent(name))
            XCTAssertNil(bytes.range(of: Data("invented".utf8)), "\(name) holds no secret")
            XCTAssertNil(bytes.range(of: key), "\(name) holds no key")
        }
    }

    /// Keychain items outlive the app on iOS: a new install (no client.id) never reuses what an earlier one left.
    // decision: PHN-561
    func testANewInstallDeletesWhatAnEarlierInstallLeftInTheKeychain() throws {
        let secrets = MemoryStore()
        try secrets.set("link.state", Data("old".utf8))
        try secrets.set("store.key", Data(repeating: 1, count: 32))
        try secrets.set("key.device", Data(repeating: 2, count: 32))
        let v = vault(secrets)
        XCTAssertEqual(secrets.names, [])
        XCTAssertNil(try v.linkState())
    }

    /// This install kept a link state and the Keychain no longer has it: the pairing is given up, said, and wiped.
    // decision: PHN-561
    func testALinkStateTheKeychainLostIsSaidNotSilentlyReplaced() throws {
        let secrets = MemoryStore()
        let v = vault(secrets)
        _ = v.clientId()
        try v.saveLinkState(Data("state".utf8))
        secrets.delete("link.state")
        XCTAssertThrowsError(try v.linkState()) { XCTAssertEqual(($0 as? VaultLost)?.why, .keychainReset) }
        v.wipe()
        XCTAssertNil(try vault(secrets).linkState(), "after the wipe a first start")
    }

    // decision: PHN-408
    func testForgetKeepsTheLinkKeyAndDropsTheStoreAndTheSigningKeys() throws {
        let secrets = MemoryStore()
        let v = vault(secrets)
        _ = v.clientId()
        try v.saveLinkState(Data("state".utf8))
        let key = try v.storeKey()
        try v.keys.ensure()
        v.saveRegisteredKeys(v.keys.publicKeys())
        FileManager.default.createFile(atPath: v.storeFile.path, contents: Data("db".utf8))
        v.forget()
        XCTAssertFalse(FileManager.default.fileExists(atPath: v.storeFile.path))
        XCTAssertNil(v.registeredKeys())
        XCTAssertEqual(v.keys.health(), KeyHealth(device: .none, lock: .none, strong: .none))
        XCTAssertEqual(try v.linkState(), Data("state".utf8), "Link.forget rewrites it; the phone's link key stays")
        XCTAssertNotEqual(try v.storeKey(), key, "a new store, a new key")
    }

    /// The real Keychain item class (AfterFirstUnlockThisDeviceOnly, never synchronizable). An unhosted test bundle
    /// may have no Keychain access on the simulator; then this says so and skips.
    // decision: PHN-561
    func testTheKeychainStoreRoundTrips() throws {
        let store = KeychainStore(service: "com.ferb.phone.tests.\(UUID().uuidString)")
        do { try store.set("x", Data("one".utf8)) } catch {
            throw XCTSkip("no Keychain for this test bundle on the simulator: \(error)")
        }
        defer { store.deleteAll() }
        XCTAssertEqual(store.get("x"), Data("one".utf8))
        XCTAssertEqual(store.exists("x"), true)
        try store.set("x", Data("two".utf8))
        XCTAssertEqual(store.get("x"), Data("two".utf8))
        store.delete("x")
        XCTAssertNil(store.get("x"))
        XCTAssertEqual(store.exists("x"), false)
    }
}
