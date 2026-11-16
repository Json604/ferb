import Foundation

/// The wire form of the phone's keys and signatures, as the Mac verifies them (engine/crates/ferb-daemon/src/phone/
/// proof.rs): a public key is the SubjectPublicKeyInfo DER of an uncompressed P-256 point, a signature is ECDSA over
/// SHA-256 of the exact UTF-8 payload in DER (X9.62); both in standard base64 without line breaks. The Secure Enclave
/// and CryptoKit both give the raw point (X9.63, 65 bytes) and a DER signature, so the one conversion here is adding
/// the SPKI header; Android's Keystore gives the SPKI itself (apps/android :core Proofs.kt). kb PHN-443, PHN-561.
public enum Proofs {
    /// proof.rs `SPKI`: the header of an uncompressed P-256 key's SubjectPublicKeyInfo.
    public static let spkiHeader: [UInt8] = [
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
        0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
    ]

    /// The key as the Mac registers it, from the raw uncompressed point (`04 || X || Y`), or nil when it is not one.
    public static func publicKey(point: Data) -> String? {
        guard point.count == 65, point.first == 0x04 else { return nil }
        return (Data(spkiHeader) + point).base64EncodedString()
    }

    /// A DER signature as the Mac reads it (proof.rs `signature`: one-byte lengths, two INTEGERs), else nil.
    public static func signature(der: Data) -> String? {
        let d = [UInt8](der)
        let n = d.count
        guard n >= 8, d[0] == 0x30, Int(d[1]) == n - 2, d[2] == 0x02 else { return nil }
        let rl = Int(d[3])
        let at = 4 + rl
        guard n >= at + 2, d[at] == 0x02, Int(d[at + 1]) == n - 6 - rl else { return nil }
        return der.base64EncodedString()
    }
}
