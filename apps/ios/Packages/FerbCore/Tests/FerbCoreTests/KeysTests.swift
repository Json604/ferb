import CryptoKit
import Foundation
import XCTest
@testable import FerbCore

/// The wire form the Mac verifies (engine/crates/ferb-daemon/src/phone/proof.rs) and the three keys' states, through
/// software keys: the simulator has no Secure Enclave, and the encoding path (raw point + SPKI header, DER signature)
/// is the one both backends share.
final class KeysTests: XCTestCase {
    private var dir: URL!

    override func setUpWithError() throws {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-keys-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws { try? FileManager.default.removeItem(at: dir) }

    /// The repository's engine/fixtures/phone, found from this file (the simulator reads the Mac's disk).
    static let fixtures = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        .appendingPathComponent("../../../../../../engine/fixtures/phone").standardized

    // decision: PHN-443, PHN-561
    func testKeysAndSignaturesAreInTheMacsWireForm() throws {
        let store = MemoryStore()
        let keys = PhoneKeys(backend: SoftwareBackend(store: store), record: dir.appendingPathComponent("keys.made"))
        let pubs = try keys.ensure()
        for (kind, pub) in [(KeyKind.device, pubs.device), (.lock, pubs.lock), (.strong, pubs.strong)] {
            let b64 = try XCTUnwrap(pub, "\(kind)")
            let der = try XCTUnwrap(Data(base64Encoded: b64))
            XCTAssertEqual(der.count, 91, "SubjectPublicKeyInfo of an uncompressed P-256 point")
            XCTAssertEqual([UInt8](der.prefix(26)), Proofs.spkiHeader)
            // CryptoKit reads it as a SubjectPublicKeyInfo: the same form Android's Keystore gives
            let key = try P256.Signing.PublicKey(derRepresentation: der)
            let payload = #"{"challenge":"C-\#(kind)","method":"link.unlock","params":{}}"#
            let sig = try XCTUnwrap(kind == .device ? keys.deviceSign(payload) : keys.sign(kind, payload, reason: "test"))
            let raw = try XCTUnwrap(Data(base64Encoded: sig))
            XCTAssertEqual(raw.first, 0x30, "DER")
            XCTAssertTrue(key.isValidSignature(try P256.Signing.ECDSASignature(derRepresentation: raw), for: Data(payload.utf8)))
            XCTAssertFalse(key.isValidSignature(try P256.Signing.ECDSASignature(derRepresentation: raw), for: Data("{}".utf8)))
        }
        XCTAssertNil(Proofs.publicKey(point: Data(repeating: 4, count: 64)))
        XCTAssertNil(Proofs.signature(der: Data([0x30, 0x02, 0x02, 0x00])))
    }

    /// Android's Keystore signatures (engine/fixtures/phone/android-signatures.json) read the same way here: one form.
    // decision: PHN-497, PHN-561
    func testAndroidsVectorsVerifyInSwiftToo() throws {
        try verifyVectors(Self.fixtures.appendingPathComponent("android-signatures.json"))
    }

    /// The vectors this simulator made (engine/fixtures/phone/ios-signatures.json; the Mac's proof.rs test reads them
    /// too). Written when missing (delete the file to make new ones).
    // decision: PHN-561
    func testIOSVectorsVerify() throws {
        let file = Self.fixtures.appendingPathComponent("ios-signatures.json")
        if ProcessInfo.processInfo.environment["FERB_WRITE_IOS_VECTORS"] != nil || !FileManager.default.fileExists(atPath: file.path) {
            let keys = PhoneKeys(backend: SoftwareBackend(store: MemoryStore()), record: dir.appendingPathComponent("keys.made"))
            let pub = try XCTUnwrap(try keys.ensure().device)
            var vectors: [[String: Any]] = []
            for i in 1...3 {
                let payload = #"{"challenge":"vector-\#(i)","method":"link.present","params":{}}"#
                vectors.append(["what": "ios simulator key \(i)", "key": pub, "payload": payload,
                                "sig": try XCTUnwrap(keys.deviceSign(payload)), "valid": true])
            }
            let first = #"{"challenge":"vector-1","method":"link.present","params":{}}"#
            vectors.append(["what": "ios simulator key, another payload", "key": pub, "payload": "{}",
                            "sig": try XCTUnwrap(keys.deviceSign(first)), "valid": false])
            let doc: [String: Any] = [
                "made_by": "apps/ios/Packages/FerbCore KeysTests.testIOSVectorsVerify on the iOS simulator (no Secure Enclave there): a CryptoKit P-256 key through FerbCore's Proofs, the encoding the Secure Enclave's keys take on a phone (raw point plus the SPKI header; DER signatures)",
                "vectors": vectors,
            ]
            let data = try JSONSerialization.data(withJSONObject: doc, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes])
            try data.write(to: file)
        }
        try verifyVectors(file)
    }

    /// proof.rs's own fixed key (the scalar 1…32, RFC 6979), printed by a verbatim copy of its verify: the Mac's form
    /// and iOS's are the same bytes.
    // decision: PHN-561
    func testTheMacsFixedVectorReadsHere() throws {
        let key = "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEUVw9brnjlrkE0/7Kf1T9zQzB6Ze/N13KUVrQpsO0A19FNr46UPMY+/mlR1kCoiFQK+8NV+CMU7LMClbxfZ+TVA=="
        let sig = "MEUCIQC5XccdVzetTczXIIQ1CmFPqGVlw0QrddabLwS2zBD+bgIgBpgv0ck1jiOCyJbJ2U21+XmqUAgjOi2ANDNuKZj9hv0="
        let payload = #"{"challenge":"abc","method":"link.unlock","params":{}}"#
        let pub = try P256.Signing.PublicKey(derRepresentation: XCTUnwrap(Data(base64Encoded: key)))
        XCTAssertEqual(Proofs.publicKey(point: pub.x963Representation), key, "the SPKI iOS builds equals the Mac's")
        let der = try XCTUnwrap(Data(base64Encoded: sig))
        XCTAssertEqual(Proofs.signature(der: der), sig)
        XCTAssertTrue(pub.isValidSignature(try P256.Signing.ECDSASignature(derRepresentation: der), for: Data(payload.utf8)))
    }

    private func verifyVectors(_ file: URL) throws {
        let doc = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: file)) as? [String: Any])
        let vectors = try XCTUnwrap(doc["vectors"] as? [[String: Any]])
        XCTAssertGreaterThanOrEqual(vectors.count, 4)
        for v in vectors {
            let key = try P256.Signing.PublicKey(derRepresentation: XCTUnwrap(Data(base64Encoded: v["key"] as! String)))
            let sig = try P256.Signing.ECDSASignature(derRepresentation: XCTUnwrap(Data(base64Encoded: v["sig"] as! String)))
            XCTAssertEqual(key.isValidSignature(sig, for: Data((v["payload"] as! String).utf8)), v["valid"] as! Bool, "\(v["what"]!)")
        }
    }

    // decision: PHN-561, PHN-133
    func testKeyStatesFollowWhatThePhoneCanHoldAndWhatItLost() throws {
        let store = MemoryStore()
        let phone = FakeSettings()
        let backend = SoftwareBackend(store: store, holds: { kind in kind == .lock ? phone.passcode : true }, biometry: { phone.faces })
        let keys = PhoneKeys(backend: backend, record: dir.appendingPathComponent("keys.made"))
        try keys.ensure()
        XCTAssertEqual(keys.health(), KeyHealth(device: .ok, lock: .none, strong: .ok))
        XCTAssertTrue(keys.health().pairAgain, "no lock key: the Mac can't be unlocked from here")

        phone.passcode = true   // a passcode was set
        let keys2 = keys
        try keys2.ensure()
        XCTAssertEqual(keys2.health(), KeyHealth(device: .ok, lock: .ok, strong: .ok))
        XCTAssertFalse(keys2.health().pairAgain)

        phone.faces = Data("faces-2".utf8)   // a face added
        XCTAssertEqual(keys2.health().strong, .gone)
        XCTAssertTrue(keys2.health().fingerprintsChanged)
        XCTAssertNil(keys2.sign(.strong, "{}", reason: "test"), "a strong key whose biometrics changed never signs")
        XCTAssertNil(keys2.publicKeys().strong)

        store.delete("key.lock")   // the passcode turned off: iOS deletes the lock key
        XCTAssertEqual(keys2.health().lock, .gone)
        XCTAssertTrue(keys2.health().pairAgain)

        let fresh = try keys2.renew()
        XCTAssertNotNil(fresh.lock)
        XCTAssertNotNil(fresh.strong)
        XCTAssertEqual(keys2.health(), KeyHealth(device: .ok, lock: .ok, strong: .ok))

        keys2.deleteAll()
        XCTAssertEqual(keys2.health(), KeyHealth(device: .none, lock: .none, strong: .none))
    }

    /// Update fingerprint's key (PHN-849): a strong key iOS made unusable is replaced by a new one; a usable one is
    /// kept (an earlier update the Mac never took made it); the device and lock keys never change; a phone with no Face
    /// ID or Touch ID gets none.
    // decision: PHN-849
    func testUpdateFingerprintReplacesOnlyAGoneStrongKey() throws {
        let store = MemoryStore()
        let phone = FakeSettings()
        phone.passcode = true
        let backend = SoftwareBackend(store: store, holds: { kind in kind == .lock ? phone.passcode : kind == .strong ? phone.biometrics : true },
                                      biometry: { phone.faces })
        let keys = PhoneKeys(backend: backend, record: dir.appendingPathComponent("keys.made"))
        let paired = try keys.ensure()

        XCTAssertEqual(try keys.strongForUpdate(), paired.strong, "a usable key is kept")
        phone.faces = Data("faces-2".utf8)   // a face added: the strong key is gone
        XCTAssertEqual(keys.health().strong, .gone)
        let fresh = try XCTUnwrap(try keys.strongForUpdate())
        XCTAssertNotEqual(fresh, paired.strong, "a new key")
        XCTAssertEqual(keys.health(), KeyHealth(device: .ok, lock: .ok, strong: .ok), "it works with the faces as they are now")
        XCTAssertEqual(keys.publicKeys().device, paired.device)
        XCTAssertEqual(keys.publicKeys().lock, paired.lock)
        XCTAssertNotNil(keys.sign(.strong, "{}", reason: "test"))
        XCTAssertEqual(try keys.strongForUpdate(), fresh, "made once: a second scan reuses it")

        phone.faces = Data("faces-3".utf8)
        phone.biometrics = false   // Face ID turned off: nothing to make
        XCTAssertNil(try keys.strongForUpdate())
    }

    /// The Mac's view of the strong key (PHN-849): a usable key that isn't the one the Mac holds counts as changed.
    // decision: PHN-849
    func testAStrongKeyTheMacDoesntHoldReadsAsChanged() throws {
        let store = MemoryStore()
        let phone = FakeSettings()
        phone.passcode = true
        let keys = PhoneKeys(backend: SoftwareBackend(store: store, holds: { kind in kind == .lock ? phone.passcode : true },
                                                      biometry: { phone.faces }),
                             record: dir.appendingPathComponent("keys.made"))
        let vault = Vault(dir: dir.appendingPathComponent("ferb"), secrets: store, keys: keys)
        let paired = try keys.ensure()
        vault.saveRegisteredKeys(paired)
        XCTAssertEqual(vault.keyHealth().strong, .ok)

        phone.faces = Data("faces-2".utf8)
        let made = try XCTUnwrap(try keys.strongForUpdate())   // made for an update the owner then said No to
        XCTAssertEqual(keys.health().strong, .ok)
        XCTAssertEqual(vault.keyHealth().strong, .gone, "the Mac still holds the old key")
        XCTAssertTrue(vault.keyHealth().fingerprintsChanged)
        XCTAssertFalse(vault.keyHealth().pairAgain, "chat still works")
        XCTAssertTrue(vault.keyHealth().updateFingerprint, "the Mac holds a strong key: it can take a new one")

        vault.saveRegisteredKeys(PublicKeys(device: paired.device, lock: paired.lock, strong: made))   // the Mac's Done
        XCTAssertEqual(vault.keyHealth().strong, .ok)

        vault.saveRegisteredKeys(PublicKeys(device: paired.device, lock: paired.lock, strong: nil))   // paired without one
        XCTAssertEqual(vault.keyHealth().strong, .ok, "a key the Mac was never given is registered, not changed")
        XCTAssertFalse(vault.keyHealth().strongOnMac)
    }

    /// The prompt stands between the app and the lock and strong keys; the device key never asks.
    // decision: PHN-497, PHN-561
    func testOnlyTheDeviceKeySignsWithoutThePrompt() throws {
        let phone = FakeSettings()
        let keys = PhoneKeys(backend: SoftwareBackend(store: MemoryStore(), authenticate: { kind, _ in phone.asked.append(kind); return phone.allow }),
                             record: dir.appendingPathComponent("keys.made"))
        try keys.ensure()
        XCTAssertNotNil(keys.deviceSign("{}"))
        XCTAssertNil(keys.sign(.lock, "{}", reason: "Unlock FERB"))
        XCTAssertNil(keys.sign(.strong, "{}", reason: "Confirm"))
        XCTAssertEqual(phone.asked, [.lock, .strong])
        phone.allow = true
        XCTAssertNotNil(keys.sign(.lock, "{}", reason: "Unlock FERB"))
    }
}

/// The phone's settings as a test changes them.
final class FakeSettings: @unchecked Sendable {
    var passcode = false
    var biometrics = true
    var faces: Data? = Data("faces-1".utf8)
    var asked: [KeyKind] = []
    var allow = false
}
