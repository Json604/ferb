package com.ferb.core

import java.security.Signature

/**
 * The phone's three signing keys (kb PHN-443, PHN-133; the Mac's `device_pub`, `unlock_pub`, `strong_pub`), all
 * P-256, all held where the app can use them but never read them (the Android Keystore: [KeystoreKeys]).
 */
enum class KeyKind(val alias: String) {
    /** No prompt: proves a connection comes from this very phone (`link.present`; notification text, PHN-126). */
    DEVICE("ferb.key.device"),

    /** After the fingerprint, face or the phone's PIN, each use: the Mac's unlock window, a private answer (PHN-125). */
    LOCK("ferb.key.lock"),

    /** After a strong biometric only, each use; gone when a fingerprint is added: hard Yes, sensitive calls (PHN-137). */
    STRONG("ferb.key.strong"),
}

enum class KeyState {
    /** Present and usable (a prompt may still be needed). */
    OK,

    /** Not made: this phone could not hold it when the keys were made (no screen lock, no strong biometric). */
    NONE,

    /** Made once and since destroyed by Android: the screen lock was removed, or (STRONG) the fingerprints changed. */
    GONE,
}

/** The public halves, in the Mac's wire form ([Proofs.publicKey]), as the pairing hello carries them. */
data class PublicKeys(val device: String?, val lock: String?, val strong: String?)

/**
 * Which keys work now. [strong] is GONE also when this phone's fingerprint-only key is not the one the Mac holds (one
 * made for an update the owner didn't approve): the Mac would refuse its signature (PHN-133). [strongOnMac]: the Mac
 * holds a fingerprint-only key for this phone, so a new one can be given to it with Update fingerprint.
 */
data class KeyHealth(val device: KeyState, val lock: KeyState, val strong: KeyState, val strongOnMac: Boolean = false) {
    /**
     * The Mac can no longer be unlocked from this phone (its lock key is gone, or there never was one) or no longer
     * recognises it as this phone: pair again (after setting a screen lock).
     */
    val pairAgain: Boolean get() = lock != KeyState.OK || device != KeyState.OK

    /** Only the fingerprint-only key is gone: chat works; a hard Yes and sensitive changes wait for a new key. */
    val fingerprintsChanged: Boolean get() = strong == KeyState.GONE

    /** The fingerprints changed and the Mac can take a new key without pairing again: Update fingerprint. */
    val updateFingerprint: Boolean get() = fingerprintsChanged && strongOnMac && !pairAgain
}

interface PhoneKeys {
    /** Makes each key this phone can hold now and is missing; leaves a GONE one as it is. The public halves. */
    fun ensure(): PublicKeys

    /** Deletes every GONE key, then [ensure]: fresh keys for a pairing (never for a phone already paired). */
    fun renew(): PublicKeys

    /**
     * The fingerprint-only key to offer the Mac in Update fingerprint: a GONE one is deleted and a new one made when
     * a strong biometric is enrolled; a usable one is offered as it is. Null: this phone can't hold one now.
     */
    fun strongForUpdate(): String?

    /** The fingerprint-only key's public half when it is usable, else null. No prompt. */
    fun strongPublic(): String?

    fun health(): KeyHealth

    /** The device key's signature over [payload] (base64 DER), or null when it is missing or failed. No prompt. */
    fun deviceSign(payload: String): String?

    /** A signature object for [kind] (LOCK or STRONG), ready for the prompt's CryptoObject. @throws KeyGone */
    fun signatureFor(kind: KeyKind): Signature

    /** Deletes all three. */
    fun deleteAll()
}
