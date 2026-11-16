package com.ferb.core

import java.io.File
import java.io.FileOutputStream
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.GeneralSecurityException
import javax.crypto.AEADBadTagException
import javax.crypto.Cipher
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** The wrapping key is gone or can no longer be used (the Keystore was reset, the key invalidated or never made). */
class KeyGone(message: String, cause: Throwable? = null) : Exception(message, cause)

/** The bytes are not something this key sealed for this purpose (changed, cut, from elsewhere). */
class Unreadable(message: String, cause: Throwable? = null) : Exception(message, cause)

/**
 * Seals the phone's few secrets on disk (the store's key, the link's state) under a key the app holds but cannot
 * read: on a phone, a non-exportable Android Keystore key ([KeystoreProtector]). kb PHN-448, PHN-496.
 */
interface Protector {
    /** [plain] sealed for [purpose] (a short fixed word: a blob sealed for one purpose never opens as another). */
    fun seal(plain: ByteArray, purpose: String): ByteArray

    /** @throws KeyGone when the key is missing or invalidated; @throws Unreadable when the blob is not ours. */
    fun open(sealed: ByteArray, purpose: String): ByteArray

    /** Deletes the key: everything sealed under it is unreadable from now on. */
    fun destroy()
}

/**
 * AES-256-GCM. The format: one version byte (1), the 12-byte IV the cipher drew, the ciphertext and its 16-byte tag;
 * the additional data is `ferb/<purpose>/1`. The IV is never chosen by the app (the Keystore refuses that by default).
 */
abstract class AesGcmProtector : Protector {
    /** The key, made when [create] and missing; null when missing and not to be made. */
    protected abstract fun key(create: Boolean): SecretKey?

    override fun seal(plain: ByteArray, purpose: String): ByteArray {
        val key = key(create = true) ?: throw KeyGone("No wrapping key.")
        val cipher = Cipher.getInstance(TRANSFORM)
        try {
            cipher.init(Cipher.ENCRYPT_MODE, key)
        } catch (e: GeneralSecurityException) {
            throw KeyGone("The wrapping key can't be used.", e)
        }
        cipher.updateAAD(aad(purpose))
        val body = cipher.doFinal(plain)
        val iv = cipher.iv
        check(iv.size == IV_BYTES) { "unexpected IV size ${iv.size}" }
        return byteArrayOf(VERSION) + iv + body
    }

    override fun open(sealed: ByteArray, purpose: String): ByteArray {
        if (sealed.size < 1 + IV_BYTES + TAG_BYTES || sealed[0] != VERSION) throw Unreadable("Not a sealed FERB file.")
        val key = try {
            key(create = false)
        } catch (e: GeneralSecurityException) {
            throw KeyGone("The wrapping key can't be read.", e)
        } ?: throw KeyGone("The wrapping key is gone.")
        val cipher = Cipher.getInstance(TRANSFORM)
        try {
            // KeyPermanentlyInvalidatedException is an InvalidKeyException: thrown here
            cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(TAG_BYTES * 8, sealed, 1, IV_BYTES))
        } catch (e: GeneralSecurityException) {
            throw KeyGone("The wrapping key can't be used.", e)
        }
        cipher.updateAAD(aad(purpose))
        return try {
            cipher.doFinal(sealed, 1 + IV_BYTES, sealed.size - 1 - IV_BYTES)
        } catch (e: AEADBadTagException) {
            throw Unreadable("Sealed under another key or changed.", e)
        } catch (e: GeneralSecurityException) {
            throw Unreadable("Could not be opened.", e)
        }
    }

    private fun aad(purpose: String) = "ferb/$purpose/1".toByteArray()

    companion object {
        private const val TRANSFORM = "AES/GCM/NoPadding"
        private const val VERSION: Byte = 1
        private const val IV_BYTES = 12
        private const val TAG_BYTES = 16
    }
}

/** Small files written whole or not at all. */
object SealedFiles {
    fun read(file: File): ByteArray? = if (file.isFile) file.readBytes() else null

    /** A temporary file in the same folder, flushed to disk, then renamed over the old one (atomic on one volume). */
    fun write(file: File, bytes: ByteArray) {
        file.parentFile?.mkdirs()
        val tmp = File(file.parentFile, ".${file.name}.tmp")
        FileOutputStream(tmp).use {
            it.write(bytes)
            it.fd.sync()
        }
        Files.move(tmp.toPath(), file.toPath(), StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE)
    }
}
