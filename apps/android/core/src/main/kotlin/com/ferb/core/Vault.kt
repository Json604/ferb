package com.ferb.core

import android.content.Context
import java.io.File
import java.security.SecureRandom
import java.util.UUID

/** Why a kept pairing was given up (everything was wiped; the app pairs again). */
enum class Lost {
    /** The wrapping key is gone: the Keystore was reset, or the files came back without it (they never should). */
    KEYSTORE_RESET,

    /** The kept link state was changed or is not ours. */
    STATE_UNREADABLE,

    /** The link refused the kept state as not its own (ferb-mobile-ffi `bad_argument`). */
    STATE_REFUSED,
}

class VaultLost(val why: Lost, cause: Throwable? = null) : Exception("The kept pairing can't be used: $why", cause)

/**
 * What the phone keeps, where, under what (kb PHN-448, PHN-494, PHN-496). One folder, [dir]: on a phone
 * `noBackupFilesDir/ferb` (app-private, credential-encrypted storage, and outside every backup by definition).
 * - `link.state`: the link's state as `LinkListener.persist` hands it (it holds the phone's PRIVATE link key and where
 *   the Mac is), sealed by [protector] for "link".
 * - `store.key`: the 32 random bytes that encrypt the SQLCipher store, sealed for "store".
 * - `phone.db` (and its -wal, -shm): ferb-mobile's SQLCipher store (the fetched chat, cursors, the outbox).
 * - `client.id`: this install's id among the conversation's clients (a random UUID; not secret).
 * The signing keys are in the Keystore ([keys]), never in a file.
 *
 * The store is a copy of what the Mac holds (ENG-10), so a store key that can't be read costs a refetch only: the
 * store's files go and a new key is made. The link state is the pairing: when it can't be read, [linkState] throws
 * [VaultLost] and the caller wipes and pairs again.
 */
class Vault(
    val dir: File,
    private val protector: Protector,
    val keys: PhoneKeys,
    private val random: SecureRandom = SecureRandom(),
) {
    val storeFile: File get() = File(dir, "phone.db")
    private val storeKeyFile get() = File(dir, "store.key")
    private val linkFile get() = File(dir, "link.state")
    private val idFile get() = File(dir, "client.id")
    private val registeredFile get() = File(dir, "keys.mac")

    init {
        dir.mkdirs()
    }

    fun hasLinkState(): Boolean = linkFile.isFile

    fun clientId(): String {
        SealedFiles.read(idFile)?.toString(Charsets.UTF_8)?.trim()?.takeIf { it.isNotEmpty() }?.let { return it }
        val id = UUID.randomUUID().toString()
        SealedFiles.write(idFile, id.toByteArray())
        return id
    }

    /** The store's key, unwrapped, or made (and the old store dropped) when none can be read. The caller zeroes it. */
    fun storeKey(): ByteArray {
        SealedFiles.read(storeKeyFile)?.let { sealed ->
            try {
                val key = protector.open(sealed, STORE)
                if (key.size == 32) return key
            } catch (_: KeyGone) {
            } catch (_: Unreadable) {
            }
        }
        dropStore()
        val key = ByteArray(32).also(random::nextBytes)
        SealedFiles.write(storeKeyFile, protector.seal(key, STORE))
        return key
    }

    /** The store's files and its key. */
    fun dropStore() {
        for (suffix in listOf("", "-journal", "-wal", "-shm")) File(dir, "phone.db$suffix").delete()
        storeKeyFile.delete()
    }

    /** The kept link state, or null (never started here). @throws VaultLost when it is kept but can't be read. */
    fun linkState(): ByteArray? {
        val sealed = SealedFiles.read(linkFile) ?: return null
        return try {
            protector.open(sealed, LINK)
        } catch (e: KeyGone) {
            throw VaultLost(Lost.KEYSTORE_RESET, e)
        } catch (e: Unreadable) {
            throw VaultLost(Lost.STATE_UNREADABLE, e)
        }
    }

    fun saveLinkState(state: ByteArray) = SealedFiles.write(linkFile, protector.seal(state, LINK))

    /**
     * The public keys the paired Mac was given (the pairing hello's, then each one registered after it), or null when
     * none were recorded (a phone paired before this record existed). Public halves only: nothing secret.
     */
    fun registeredKeys(): PublicKeys? {
        if (!registeredFile.isFile) return null
        val l = registeredFile.readLines()
        fun at(i: Int) = l.getOrNull(i)?.takeIf { it.isNotEmpty() && it != "-" }
        return PublicKeys(at(0), at(1), at(2))
    }

    fun saveRegisteredKeys(k: PublicKeys) {
        SealedFiles.write(registeredFile, listOf(k.device, k.lock, k.strong).joinToString("\n") { it ?: "-" }.toByteArray())
    }

    /**
     * "Forget this Mac" (PHN-408): the store, its key and the three signing keys go (the next pairing registers new
     * ones, so whatever the forgotten Mac still holds of this phone verifies nothing new). The link state stays as
     * `Link.forget` rewrites it: the phone's link key without the Mac.
     */
    fun forget() {
        dropStore()
        registeredFile.delete()
        keys.deleteAll()
    }

    /** Everything, the wrapping key too: the next start is a first start. */
    fun wipe() {
        dropStore()
        linkFile.delete()
        idFile.delete()
        registeredFile.delete()
        keys.deleteAll()
        protector.destroy()
    }

    companion object {
        private const val STORE = "store"
        private const val LINK = "link"

        /** The phone's vault: `noBackupFilesDir/ferb`, the Keystore's wrapping key and signing keys. */
        fun forPhone(context: Context): Vault =
            Vault(File(context.noBackupFilesDir, "ferb"), KeystoreProtector(context), KeystoreKeys(context))
    }
}
