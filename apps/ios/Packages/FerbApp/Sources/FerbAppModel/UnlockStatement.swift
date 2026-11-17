import Foundation
import Security

/// The unlock the lock key signs in the app's opening prompt before the link is up (kb PHN-649; Android's
/// UnlockStatement): the phone's own statement `{domain, nonce, made}`, its signature, and when it was signed by the
/// phone's steady clock (the age the Mac is told; the wall clock in `made` is only reported). One per prompt.
final class UnlockStatement {
    /// The domain the Mac checks (ferb-daemon phone::gate::STATEMENT_DOMAIN); nothing else the phone signs has it.
    static let domain = "ferb.unlock-statement.v1"
    let payload: String
    let sig: String
    let signedAt: Double

    init(payload: String, sig: String, signedAt: Double) {
        self.payload = payload
        self.sig = sig
        self.signedAt = signedAt
    }

    /// A new statement's text: a fresh 128-bit nonce and the phone's wall time in ms.
    static func make(wallMs: Int64) -> String {
        var bytes = [UInt8](repeating: 0, count: 16)
        _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        let nonce = bytes.map { String(format: "%02x", $0) }.joined()
        return "{\"domain\":\"\(domain)\",\"nonce\":\"\(nonce)\",\"made\":\(wallMs)}"
    }
}
