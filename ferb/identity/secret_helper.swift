// FERB's signed secret transport: one stdin JSON request, one stdout JSON response. Never log input or errors.
import Foundation
import Security
import CryptoKit

func perform(_ request: [String: Any]) throws -> [String: Any] {
    let operation = request["operation"] as? String ?? ""
    if operation == "seal" {
        guard let encoded = request["key"] as? String, let key = Data(base64Encoded: encoded), key.count == 32,
              let value = request["value"] as? String, let command = request["command"] as? String,
              let origin = request["origin"] as? String else { throw NSError(domain: "input", code: 1) }
        let box = try AES.GCM.seal(Data(value.utf8), using: SymmetricKey(data: key),
                                   authenticating: Data("\(command)|\(origin)".utf8))
        return ["nonce": box.nonce.withUnsafeBytes { Data($0).base64EncodedString() },
                "ct": (box.ciphertext + box.tag).base64EncodedString()]
    }
    guard let service = request["service"] as? String, ["ferb", "ferb.owner-secrets"].contains(service),
          let account = request["account"] as? String else { throw NSError(domain: "input", code: 2) }
    var query: [String: Any] = [kSecClass as String: kSecClassGenericPassword,
                               kSecAttrService as String: service, kSecAttrAccount as String: account]
    if operation == "store", let value = request["value"] as? String {
        let attrs = [kSecValueData as String: Data(value.utf8)]
        var status = SecItemUpdate(query as CFDictionary, attrs as CFDictionary)
        if status == errSecItemNotFound {
            query[kSecValueData as String] = Data(value.utf8)
            status = SecItemAdd(query as CFDictionary, nil)
        }
        guard status == errSecSuccess else { throw NSError(domain: "keychain", code: Int(status)) }
        return ["ok": true]
    }
    if operation == "delete" {                  // the owner's Delete in FERB.app (APP-2): never read first
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw NSError(domain: "keychain", code: Int(status))
        }
        return ["deleted": status == errSecSuccess]
    }
    guard operation == "lookup" || operation == "present" else { throw NSError(domain: "input", code: 3) }
    query[kSecReturnData as String] = operation == "lookup"
    query[kSecMatchLimit as String] = kSecMatchLimitOne
    var result: CFTypeRef?
    let status = SecItemCopyMatching(query as CFDictionary, &result)
    if operation == "present" { return ["present": status == errSecSuccess] }
    guard status == errSecSuccess, let data = result as? Data,
          let value = String(data: data, encoding: .utf8) else { throw NSError(domain: "keychain", code: Int(status)) }
    return ["value": value]
}

do {
    let input = FileHandle.standardInput.readDataToEndOfFile()
    guard let request = try JSONSerialization.jsonObject(with: input) as? [String: Any] else {
        throw NSError(domain: "input", code: 0)
    }
    let output = try perform(request)
    FileHandle.standardOutput.write(try JSONSerialization.data(withJSONObject: output))
} catch {
    FileHandle.standardOutput.write(Data("{\"error\":\"secret operation failed\"}".utf8))
    exit(1)
}
