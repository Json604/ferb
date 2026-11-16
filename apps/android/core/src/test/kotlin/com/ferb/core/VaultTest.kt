package com.ferb.core

import java.io.File
import java.nio.file.Files
import java.security.KeyFactory
import java.security.Signature
import java.security.spec.X509EncodedKeySpec
import java.util.Base64
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class VaultTest {
    private val dir: File = Files.createTempDirectory("ferb-core-vault").toFile()

    @After
    fun tearDown() {
        dir.deleteRecursively()
    }

    // decision: PHN-448
    @Test
    fun a_sealed_blob_opens_only_for_its_purpose_under_its_key() {
        val p = MemoryProtector()
        val sealed = p.seal("invented".toByteArray(), "link")
        assertArrayEquals("invented".toByteArray(), p.open(sealed, "link"))
        assertEquals(1 + 12 + 8 + 16, sealed.size)
        // another purpose, a changed byte, a cut blob, another key: never opened
        for (bad in listOf<() -> Unit>(
            { p.open(sealed, "store") },
            { p.open(sealed.copyOf().also { it[15] = (it[15].toInt() xor 1).toByte() }, "link") },
            { p.open(sealed.copyOf(20), "link") },
            { MemoryProtector().also { it.seal(byteArrayOf(1), "x") }.open(sealed, "link") },
        )) {
            try {
                bad()
                fail("opened")
            } catch (_: Unreadable) {
            }
        }
        // a missing key is KeyGone, not Unreadable
        try {
            MemoryProtector().open(sealed, "link")
            fail("opened without a key")
        } catch (_: KeyGone) {
        }
        // two seals of the same bytes differ (a fresh IV each time)
        assertFalse(sealed.contentEquals(p.seal("invented".toByteArray(), "link")))
    }

    @Test
    fun the_store_key_is_made_once_kept_sealed_and_remade_with_the_store_dropped_when_unreadable() {
        val p = MemoryProtector()
        val v = Vault(dir, p, SoftwareKeys())
        val k1 = v.storeKey()
        assertEquals(32, k1.size)
        assertArrayEquals(k1, v.storeKey())
        File(dir, "phone.db").writeText("invented store")
        File(dir, "phone.db-wal").writeText("invented wal")
        // under another wrapping key (a Keystore reset): a new key, and the old store goes
        val v2 = Vault(dir, MemoryProtector(), SoftwareKeys())
        val k2 = v2.storeKey()
        assertFalse(k1.contentEquals(k2))
        assertFalse(File(dir, "phone.db").exists())
        assertFalse(File(dir, "phone.db-wal").exists())
    }

    // decision: PHN-496
    @Test
    fun the_link_state_is_kept_sealed_and_a_loss_says_why() {
        val p = MemoryProtector()
        val v = Vault(dir, p, SoftwareKeys())
        assertNull(v.linkState())
        v.saveLinkState("invented-state".toByteArray())
        assertArrayEquals("invented-state".toByteArray(), v.linkState())
        try {
            Vault(dir, MemoryProtector(), SoftwareKeys()).linkState()
            fail("opened under another key")
        } catch (e: VaultLost) {
            assertEquals(Lost.KEYSTORE_RESET, e.why)
        }
        File(dir, "link.state").writeBytes(byteArrayOf(9, 9, 9))
        try {
            v.linkState()
            fail("opened a foreign file")
        } catch (e: VaultLost) {
            assertEquals(Lost.STATE_UNREADABLE, e.why)
        }
        // the client id is made once
        assertEquals(v.clientId(), v.clientId())
        // wipe: nothing left, the wrapping key gone
        v.saveLinkState("x".toByteArray())
        v.storeKey()
        val before = p.seal(byteArrayOf(1), "x")
        v.wipe()
        assertTrue(dir.listFiles()!!.none { it.name in setOf("link.state", "store.key", "phone.db", "client.id") })
        try {
            p.open(before, "x")
            fail("the destroyed key opened something")
        } catch (_: KeyGone) {
        }
    }

    // decision: PHN-443
    // decision: PHN-497
    @Test
    fun keys_and_signatures_are_in_the_macs_wire_form() {
        val keys = SoftwareKeys()
        val pubs = keys.ensure()
        for (k in listOf(pubs.device, pubs.lock, pubs.strong)) {
            val der = Base64.getDecoder().decode(k)
            assertEquals(91, der.size)
            assertArrayEquals(Proofs.SPKI_HEADER, der.copyOfRange(0, 26))
            assertEquals(4, der[26].toInt())
        }
        val payload = """{"challenge":"C-1","method":"link.present","params":{}}"""
        val sig = keys.deviceSign(payload)!!
        val pub = KeyFactory.getInstance("EC").generatePublic(X509EncodedKeySpec(Base64.getDecoder().decode(pubs.device)))
        assertTrue(Signature.getInstance(Proofs.ALGORITHM).run { initVerify(pub); update(payload.toByteArray()); verify(Base64.getDecoder().decode(sig)) })
        // no screen lock: no lock key; no fingerprint: no strong key
        val bare = SoftwareKeys(lockScreen = false, fingerprint = false).ensure()
        assertNotNull(bare.device)
        assertNull(bare.lock)
        assertNull(bare.strong)
        assertTrue(KeyHealth(KeyState.OK, KeyState.NONE, KeyState.NONE).pairAgain)
        assertTrue(KeyHealth(KeyState.OK, KeyState.OK, KeyState.GONE).fingerprintsChanged)
        assertFalse(KeyHealth(KeyState.OK, KeyState.OK, KeyState.GONE).pairAgain)
    }

    /**
     * The Rust side's own vector: proof.rs's fixed key (the scalar 1…32) and its RFC 6979 signature over the unlock
     * payload, printed by the daemon's verifier code (a verbatim copy of proof.rs, journal 113). Java reads the key
     * and the DER signature as the Mac writes and reads them: the forms agree in both directions with the Android
     * vectors the Rust copy verified.
     */
    // decision: PHN-497
    @Test
    fun the_rust_verifiers_fixed_vector_reads_and_verifies_here() {
        val key = "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEUVw9brnjlrkE0/7Kf1T9zQzB6Ze/N13KUVrQpsO0A19FNr46UPMY+/mlR1kCoiFQK+8NV+CMU7LMClbxfZ+TVA=="
        val payload = """{"challenge":"abc","method":"link.unlock","params":{}}"""
        val sig = "MEUCIQC5XccdVzetTczXIIQ1CmFPqGVlw0QrddabLwS2zBD+bgIgBpgv0ck1jiOCyJbJ2U21+XmqUAgjOi2ANDNuKZj9hv0="
        val pub = KeyFactory.getInstance("EC").generatePublic(X509EncodedKeySpec(Base64.getDecoder().decode(key)))
        assertEquals(key, Proofs.publicKey(pub))
        val der = Base64.getDecoder().decode(sig)
        assertEquals(sig, Proofs.signature(der))
        assertTrue(Signature.getInstance(Proofs.ALGORITHM).run { initVerify(pub); update(payload.toByteArray()); verify(der) })
        assertFalse(Signature.getInstance(Proofs.ALGORITHM).run { initVerify(pub); update("{}".toByteArray()); verify(der) })
        // the 64-byte raw form is not what the phone sends
        assertNull(Proofs.signature(ByteArray(64) { 1 }))
    }
}
