import FerbCore
import Foundation
import XCTest
@testable import FerbAppModel

/// Unlock statements (kb PHN-649) as the iPhone app makes and signs them: the app's own statement text, signed through
/// FerbCore's keys in the Mac's wire form, written as engine/fixtures/phone/ios-statements.json when missing (delete it
/// to make new ones), where the Mac's real gate takes each (ferb-daemon tests/phone_open.rs). The simulator has no
/// Secure Enclave: a CryptoKit P-256 key, the form the Secure Enclave's keys take on a phone.
final class StatementVectorTests: XCTestCase {
    // decision: PHN-649
    func testStatementsAreSignedInTheMacsForm() throws {
        let file = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../../../../../engine/fixtures/phone/ios-statements.json").standardized
        if !FileManager.default.fileExists(atPath: file.path) {
            let dir = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-statements-\(UUID().uuidString)")
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: dir) }
            let keys = PhoneKeys(backend: SoftwareBackend(store: MemoryStore()), record: dir.appendingPathComponent("keys.made"))
            let pub = try XCTUnwrap(try keys.ensure().device)
            var vectors: [[String: Any]] = []
            for i in 1...2 {
                let payload = UnlockStatement.make(wallMs: 1_700_000_000_000 + Int64(i))
                vectors.append(["what": "ios unlock statement \(i)", "statement": true, "key": pub, "payload": payload,
                                "sig": try XCTUnwrap(keys.deviceSign(payload)), "valid": true])
            }
            let doc: [String: Any] = [
                "made_by": "apps/ios/Packages/FerbApp StatementVectorTests on the iOS simulator: the app's own UnlockStatement text signed through FerbCore's keys (CryptoKit P-256; the Secure Enclave's form on a phone), kb PHN-649. ferb-daemon tests/phone_open.rs registers each key as a phone's lock key and the real gate takes each statement once.",
                "vectors": vectors,
            ]
            try JSONSerialization.data(withJSONObject: doc, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]).write(to: file)
        }
        let doc = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: file)) as? [String: Any])
        let vectors = try XCTUnwrap(doc["vectors"] as? [[String: Any]])
        XCTAssertFalse(vectors.isEmpty)
        for v in vectors {
            XCTAssertTrue((v["payload"] as? String ?? "").contains("\"domain\":\"\(UnlockStatement.domain)\""))
        }
    }
}
