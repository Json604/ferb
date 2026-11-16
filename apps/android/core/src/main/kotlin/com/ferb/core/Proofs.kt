package com.ferb.core

import java.security.PublicKey
import java.util.Base64

/**
 * The wire form of the phone's keys and signatures, as the Mac verifies them (engine/crates/ferb-daemon/src/phone/
 * proof.rs): a public key is the SubjectPublicKeyInfo DER of an uncompressed P-256 point, a signature is ECDSA over
 * SHA-256 of the exact UTF-8 payload in DER; both in standard base64 without line breaks. That is what the Android
 * Keystore gives (`PublicKey.encoded`, `SHA256withECDSA`), so nothing is converted, only checked. kb PHN-443, PHN-497.
 */
object Proofs {
    /** proof.rs `SPKI`: the header of an uncompressed P-256 key's SubjectPublicKeyInfo. */
    val SPKI_HEADER: ByteArray = byteArrayOf(
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86.toByte(), 0x48, 0xce.toByte(), 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
        0x86.toByte(), 0x48, 0xce.toByte(), 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
    )

    const val ALGORITHM = "SHA256withECDSA"

    /** The key as the Mac registers it, or null when it is not an uncompressed P-256 key. */
    fun publicKey(key: PublicKey): String? {
        val der = key.encoded ?: return null
        val ok = der.size == SPKI_HEADER.size + 65 &&
            der.copyOfRange(0, SPKI_HEADER.size).contentEquals(SPKI_HEADER) &&
            der[SPKI_HEADER.size] == 0x04.toByte()
        return if (ok) b64(der) else null
    }

    /** A DER signature as the Mac reads it (proof.rs `signature`: one-byte lengths, two INTEGERs), else null. */
    fun signature(der: ByteArray): String? {
        val n = der.size
        if (n < 8 || der[0] != 0x30.toByte() || (der[1].toInt() and 0xff) != n - 2 || der[2] != 0x02.toByte()) return null
        val rl = der[3].toInt() and 0xff
        val at = 4 + rl
        if (n < at + 2 || der[at] != 0x02.toByte() || (der[at + 1].toInt() and 0xff) != n - 6 - rl) return null
        return b64(der)
    }

    fun b64(bytes: ByteArray): String = Base64.getEncoder().encodeToString(bytes)
}
