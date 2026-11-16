package com.ferb.core

import android.app.KeyguardManager
import android.content.Context
import android.content.pm.PackageManager
import android.hardware.biometrics.BiometricManager
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyInfo
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.util.Log
import java.security.Key
import java.security.KeyFactory
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.ProviderException
import java.security.Signature
import java.security.spec.ECGenParameterSpec
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.SecretKeyFactory

/** Where a Keystore key lives: StrongBox (its own secure chip), the TEE, or software (an emulator without either). */
enum class KeyPlace { STRONGBOX, TEE, SOFTWARE, UNKNOWN }

internal object Keystore {
    const val PROVIDER = "AndroidKeyStore"
    const val TAG = "ferb-core"

    fun store(): KeyStore = KeyStore.getInstance(PROVIDER).apply { load(null) }

    fun hasStrongBox(context: Context) =
        context.packageManager.hasSystemFeature(PackageManager.FEATURE_STRONGBOX_KEYSTORE)

    /** StrongBox when the phone has one, else the TEE: a StrongBox that refuses the key falls back once. */
    fun <T> generate(strongBox: Boolean, make: (strongBox: Boolean) -> T): T {
        if (strongBox) {
            try {
                return make(true)
            } catch (e: ProviderException) { // StrongBoxUnavailableException is one
                Log.i(TAG, "StrongBox refused a key (${e.javaClass.simpleName}); the TEE holds it")
            }
        }
        return make(false)
    }

    fun place(key: Key): KeyPlace = try {
        val info = when (key) {
            is SecretKey -> SecretKeyFactory.getInstance(key.algorithm, PROVIDER).getKeySpec(key, KeyInfo::class.java) as KeyInfo
            else -> KeyFactory.getInstance(key.algorithm, PROVIDER).getKeySpec(key, KeyInfo::class.java)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            when (info.securityLevel) {
                KeyProperties.SECURITY_LEVEL_STRONGBOX -> KeyPlace.STRONGBOX
                KeyProperties.SECURITY_LEVEL_TRUSTED_ENVIRONMENT -> KeyPlace.TEE
                KeyProperties.SECURITY_LEVEL_SOFTWARE -> KeyPlace.SOFTWARE
                else -> KeyPlace.UNKNOWN
            }
        } else {
            @Suppress("DEPRECATION")
            if (info.isInsideSecureHardware) KeyPlace.TEE else KeyPlace.SOFTWARE
        }
    } catch (e: Exception) {
        KeyPlace.UNKNOWN
    }
}

/**
 * The wrapping key: AES-256-GCM in the Android Keystore, non-exportable, StrongBox when present. No user
 * authentication: the link must start and the store open while the phone is locked in a pocket (Stay connected,
 * notifications), and the app's own lock (S11) guards the screen. It is bound to this app on this phone: uninstalling
 * or clearing the app's data deletes it, and no backup or transfer can carry it.
 */
class KeystoreProtector(private val context: Context, private val alias: String = "ferb.key.wrap") : AesGcmProtector() {
    // One maker at a time (S11 found the race on the emulator): the link's first persist (its own thread) and the
    // store's key (the start's thread) both make the key on a first start; two makers replaced each other's key and
    // the first seal failed with KeyGone ("the link state could not be kept").
    override fun key(create: Boolean): SecretKey? = synchronized(MAKING) {
        val ks = Keystore.store()
        (ks.getKey(alias, null) as? SecretKey)?.let { return it }
        if (!create) return null
        return Keystore.generate(Keystore.hasStrongBox(context)) { strongBox ->
            KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, Keystore.PROVIDER).run {
                init(
                    KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                        .setKeySize(256)
                        .setIsStrongBoxBacked(strongBox)
                        .build(),
                )
                generateKey()
            }
        }
    }

    override fun destroy() {
        synchronized(MAKING) { runCatching { Keystore.store().deleteEntry(alias) } }
    }

    private companion object {
        val MAKING = Any()
    }

    /** Where the key lives (null: not made yet). */
    fun place(): KeyPlace? = (Keystore.store().getKey(alias, null) as? SecretKey)?.let(Keystore::place)
}

/**
 * The three signing keys in the Android Keystore (kb PHN-443, PHN-133, PHN-497):
 * - DEVICE: no authentication; used by the link on its own threads, so it must not wait for a person.
 * - LOCK: user authentication on every use (timeout 0: only a signature made inside that prompt's CryptoObject),
 *   class 3 biometric or the device credential; made only on a phone with a secure screen lock; kept when the
 *   fingerprints change (a new fingerprint needs the PIN, which this key already accepts).
 * - STRONG: every use, class 3 biometric only, invalidated when a biometric is enrolled or all are removed (Android
 *   destroys it, so a fingerprint added by someone who knows the PIN cannot use it); made only when one is enrolled.
 * Removing the screen lock destroys LOCK and STRONG (Android's rule for authentication-bound keys).
 */
class KeystoreKeys(private val context: Context) : PhoneKeys {
    private val strongBox get() = Keystore.hasStrongBox(context)

    fun deviceSecure(): Boolean = context.getSystemService(KeyguardManager::class.java).isDeviceSecure

    fun strongBiometric(): Boolean =
        context.getSystemService(BiometricManager::class.java)
            .canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_STRONG) == BiometricManager.BIOMETRIC_SUCCESS

    override fun ensure(): PublicKeys = PublicKeys(
        device = publicKey(KeyKind.DEVICE) ?: make(KeyKind.DEVICE),
        lock = publicKey(KeyKind.LOCK) ?: if (state(KeyKind.LOCK) == KeyState.NONE && deviceSecure()) make(KeyKind.LOCK) else null,
        strong = publicKey(KeyKind.STRONG) ?: if (state(KeyKind.STRONG) == KeyState.NONE && strongBiometric()) make(KeyKind.STRONG) else null,
    ).let { keys ->
        // a key that is GONE has no public half to offer
        PublicKeys(
            device = keys.device.takeIf { state(KeyKind.DEVICE) == KeyState.OK },
            lock = keys.lock.takeIf { state(KeyKind.LOCK) == KeyState.OK },
            strong = keys.strong.takeIf { state(KeyKind.STRONG) == KeyState.OK },
        )
    }

    override fun renew(): PublicKeys {
        val ks = Keystore.store()
        KeyKind.entries.filter { state(it) == KeyState.GONE }.forEach { ks.deleteEntry(it.alias) }
        return ensure()
    }

    override fun health() = KeyHealth(state(KeyKind.DEVICE), state(KeyKind.LOCK), state(KeyKind.STRONG))

    /** Present, absent or destroyed, learnt without a prompt: initSign on a destroyed key throws at once. */
    fun state(kind: KeyKind): KeyState {
        val key = try {
            Keystore.store().getKey(kind.alias, null) as? PrivateKey
        } catch (e: Exception) {
            return KeyState.GONE // an entry that can't even be read
        } ?: return KeyState.NONE
        return try {
            Signature.getInstance(Proofs.ALGORITHM).initSign(key)
            KeyState.OK
        } catch (e: KeyPermanentlyInvalidatedException) {
            KeyState.GONE
        } catch (e: Exception) {
            // UserNotAuthenticatedException is not thrown here for a per-use key; anything else: unusable
            Log.w(Keystore.TAG, "the ${kind.name} key can't be used: ${e.javaClass.simpleName}")
            KeyState.GONE
        }
    }

    fun publicKey(kind: KeyKind): String? =
        Keystore.store().getCertificate(kind.alias)?.publicKey?.let(Proofs::publicKey)

    fun place(kind: KeyKind): KeyPlace? = (Keystore.store().getKey(kind.alias, null) as? PrivateKey)?.let(Keystore::place)

    private fun make(kind: KeyKind): String? = try {
        Keystore.generate(strongBox) { sb ->
            KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, Keystore.PROVIDER).apply {
                val spec = KeyGenParameterSpec.Builder(kind.alias, KeyProperties.PURPOSE_SIGN)
                    .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
                    .setDigests(KeyProperties.DIGEST_SHA256)
                    .setIsStrongBoxBacked(sb)
                when (kind) {
                    KeyKind.DEVICE -> {}
                    KeyKind.LOCK -> spec.setUserAuthenticationRequired(true)
                        .setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG or KeyProperties.AUTH_DEVICE_CREDENTIAL)
                        .setInvalidatedByBiometricEnrollment(false)
                    KeyKind.STRONG -> spec.setUserAuthenticationRequired(true)
                        .setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG)
                        .setInvalidatedByBiometricEnrollment(true)
                }
                initialize(spec.build())
            }.generateKeyPair()
        }
        publicKey(kind)
    } catch (e: Exception) {
        Log.w(Keystore.TAG, "the ${kind.name} key could not be made: ${e.javaClass.simpleName}")
        runCatching { Keystore.store().deleteEntry(kind.alias) }
        null
    }

    override fun deviceSign(payload: String): String? = try {
        val key = Keystore.store().getKey(KeyKind.DEVICE.alias, null) as PrivateKey
        val sig = Signature.getInstance(Proofs.ALGORITHM).apply {
            initSign(key)
            update(payload.toByteArray(Charsets.UTF_8))
        }.sign()
        Proofs.signature(sig)
    } catch (e: Exception) {
        Log.w(Keystore.TAG, "the device key did not sign: ${e.javaClass.simpleName}")
        null
    }

    override fun signatureFor(kind: KeyKind): Signature {
        require(kind != KeyKind.DEVICE) { "the device key signs without a prompt" }
        val key = Keystore.store().getKey(kind.alias, null) as? PrivateKey ?: throw KeyGone("This phone has no ${kind.name} key.")
        return try {
            Signature.getInstance(Proofs.ALGORITHM).apply { initSign(key) }
        } catch (e: KeyPermanentlyInvalidatedException) {
            throw KeyGone("Android destroyed the ${kind.name} key.", e)
        }
    }

    override fun deleteAll() {
        val ks = Keystore.store()
        KeyKind.entries.forEach { runCatching { ks.deleteEntry(it.alias) } }
    }
}
