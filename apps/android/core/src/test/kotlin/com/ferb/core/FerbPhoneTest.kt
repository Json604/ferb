package com.ferb.core

import com.ferb.mobile.Change
import com.ferb.mobile.Clock
import com.ferb.mobile.Ids
import com.ferb.mobile.Item
import com.ferb.mobile.Signer
import java.io.File
import java.lang.ref.WeakReference
import java.nio.file.Files
import java.security.KeyFactory
import java.security.Signature
import java.security.spec.X509EncodedKeySpec
import java.util.Base64
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** FerbPhone over the host library, the fake Mac and software stand-ins for the Keystore. */
class FerbPhoneTest {
    private val mainThread = Executors.newSingleThreadExecutor { Thread(it, "test-main") }
    private val main = mainThread.asCoroutineDispatcher()
    private val dir: File = Files.createTempDirectory("ferb-core-phone").toFile()
    private val opened = CopyOnWriteArrayList<FerbPhone>()

    private class FixedClock : Clock { override fun nowIso() = "2026-01-02T10:00:00+00:00" }
    private class Counted : Ids { val n = AtomicInteger(0); override fun newId() = "sub-${n.incrementAndGet()}" }

    private fun phone(vault: Vault, uplinks: UplinkFactory): FerbPhone =
        FerbPhone(vault, uplinks, PhoneIdentity("Invented phone", "test"), FixedClock(), Counted(), main, Dispatchers.IO) { 0 }
            .also(opened::add)

    @After
    fun tearDown() {
        opened.forEach { it.close() }
        main.close()
        dir.deleteRecursively()
    }

    private fun verify(pubB64: String, payload: String, sigB64: String): Boolean {
        val pub = KeyFactory.getInstance("EC").generatePublic(X509EncodedKeySpec(Base64.getDecoder().decode(pubB64)))
        return Signature.getInstance(Proofs.ALGORITHM).run {
            initVerify(pub)
            update(payload.toByteArray())
            verify(Base64.getDecoder().decode(sigB64))
        }
    }

    /** A top-level string member of compact JSON (android.jar's org.json is a stub on the JVM). */
    private fun jsonString(json: String, key: String): String {
        var i = json.indexOf("\"$key\":\"") + key.length + 4
        val out = StringBuilder()
        while (json[i] != '"') {
            if (json[i] == '\\') {
                i++
                out.append(if (json[i] == 'n') '\n' else json[i])
            } else {
                out.append(json[i])
            }
            i++
        }
        return out.toString()
    }

    // decision: PHN-448
    // decision: PHN-494
    // decision: PHN-498
    @Test
    fun a_first_start_keeps_the_link_state_sealed_and_delivers_changes_on_main() = runBlocking {
        val keys = SoftwareKeys()
        val vault = Vault(dir, MemoryProtector(), keys)
        val uplinks = FakeUplinks()
        val p = phone(vault, uplinks)
        val threads = CopyOnWriteArrayList<String>()
        val heard = CopyOnWriteArrayList<Change>()
        val collector = CoroutineScope(Dispatchers.Unconfined).launch {
            p.changes.collect {
                threads.add(Thread.currentThread().name.substringBefore(" @"))
                heard.add(it)
            }
        }
        val state = p.start()
        assertTrue("$state", state is PhoneState.Open && state.wiped == null && !state.keys.pairAgain)
        // the first start had no state and offered the three keys
        val start = uplinks.starts.single()
        assertNull(start.state)
        assertEquals(keys.ensure(), start.keys)
        assertEquals("Invented phone", start.name)
        // the link's persist was sealed at once: never in the clear on disk
        val sealed = File(dir, "link.state").readBytes()
        assertFalse(String(sealed, Charsets.ISO_8859_1).contains("invented-link-state"))
        assertArrayEquals("invented-link-state".toByteArray(), vault.linkState())
        // the store key is sealed too, and the store is encrypted with it
        assertEquals(1 + 12 + 32 + 16, File(dir, "store.key").length())
        p.call { it.sync() }
        val view = (p.call { it.transcript("c1") } as Outcome.Ok).value
        assertEquals("Invented question", (view.items.single() as Item.Turn).turn.words?.text)
        val until = System.currentTimeMillis() + 5_000
        while (heard.none { it is Change.Transcript } && System.currentTimeMillis() < until) Thread.sleep(10)
        assertTrue(heard.isNotEmpty())
        assertEquals(setOf("test-main"), threads.toSet())
        assertFalse(String(File(dir, "phone.db").readBytes(), Charsets.ISO_8859_1).contains("Invented question"))
        collector.cancel()
    }

    // decision: PHN-125
    // decision: PHN-493
    @Test
    fun unlock_signs_the_macs_challenge_with_the_lock_key_and_errors_come_back_typed() = runBlocking {
        val keys = SoftwareKeys()
        val vault = Vault(dir, MemoryProtector(), keys)
        val uplinks = FakeUplinks()
        val p = phone(vault, uplinks)
        p.start()
        val signer = object : Signer { override fun sign(payload: String) = keys.sign(KeyKind.LOCK, payload) }
        assertTrue(p.unlock(signer) is Outcome.Ok)
        val proof = uplinks.made.single().mac.proofs.single()
        val payload = jsonString(proof, "payload")
        assertTrue(payload, payload.contains("\"method\":\"link.unlock\""))
        assertTrue(verify(keys.ensure().lock!!, payload, jsonString(proof, "sig")))
        // a cancelled prompt is the typed not_signed
        val cancelled = p.unlock(object : Signer { override fun sign(payload: String): String? = null })
        assertEquals("not_signed", (cancelled as Outcome.Failed).code)
        // a call after close is closed, not a crash
        p.close()
        assertEquals("closed", (p.call { it.sync() } as Outcome.Failed).code)
        assertEquals(PhoneState.Closed, p.state.value)
    }

    @Test
    fun a_restart_uses_the_kept_state_and_store_and_a_tampered_state_is_wiped() = runBlocking {
        val keys = SoftwareKeys()
        val protector = MemoryProtector()
        val vault = Vault(dir, protector, keys)
        val first = phone(vault, FakeUplinks())
        first.start()
        first.call { it.sync() }
        first.close()
        // a new process: the same files and Keystore
        val uplinks = FakeUplinks()
        val second = phone(Vault(dir, protector, keys), uplinks)
        val state = second.start()
        assertTrue("$state", state is PhoneState.Open && state.wiped == null)
        assertArrayEquals("invented-link-state".toByteArray(), uplinks.starts.single().state)
        assertEquals(1, (second.call { it.transcript("c1") } as Outcome.Ok).value.items.size)
        second.close()
        // the link state changed on disk: given up, everything wiped, a first start again
        val f = File(dir, "link.state")
        f.writeBytes(f.readBytes().also { it[20] = (it[20].toInt() xor 1).toByte() })
        val deletesBefore = keys.deletes.get()
        val third = FakeUplinks()
        val s3 = phone(Vault(dir, protector, keys), third).start()
        assertEquals(Lost.STATE_UNREADABLE, (s3 as PhoneState.Open).wiped)
        assertNull(third.starts.single().state)
        assertTrue(keys.deletes.get() > deletesBefore)
    }

    // decision: PHN-496
    @Test
    fun a_lost_keystore_key_means_pair_again_and_a_lost_store_key_only_a_refetch() = runBlocking {
        val keys = SoftwareKeys()
        val vault = Vault(dir, MemoryProtector(), keys)
        phone(vault, FakeUplinks()).apply { start(); close() }
        // the store key unreadable: the store is dropped and made again; the pairing stays
        File(dir, "store.key").writeBytes(ByteArray(61) { 1 })
        val again = FakeUplinks()
        val s = phone(vault, again).start()
        assertTrue("$s", s is PhoneState.Open && s.wiped == null)
        assertArrayEquals("invented-link-state".toByteArray(), again.starts.single().state)
        opened.last().close()
        // the wrapping key gone (a Keystore reset, files back without it): pair again
        val reset = FakeUplinks()
        val s2 = phone(Vault(dir, MemoryProtector(), keys), reset).start()
        assertEquals(Lost.KEYSTORE_RESET, (s2 as PhoneState.Open).wiped)
        assertNull(reset.starts.single().state)
    }

    @Test
    fun a_state_the_link_refuses_is_given_up_and_started_fresh() = runBlocking {
        val keys = SoftwareKeys()
        val protector = MemoryProtector()
        phone(Vault(dir, protector, keys), FakeUplinks()).apply { start(); close() }
        val refusing = FakeUplinks().apply { refuseState = true }
        val s = phone(Vault(dir, protector, keys), refusing).start()
        assertEquals(Lost.STATE_REFUSED, (s as PhoneState.Open).wiped)
        assertNull(refusing.starts.single().state)
    }

    // decision: PHN-408, PHN-646
    @Test
    fun forget_this_mac_wipes_the_store_and_the_keys_and_starts_again_ready_to_pair() = runBlocking {
        val keys = SoftwareKeys()
        val vault = Vault(dir, MemoryProtector(), keys)
        val uplinks = FakeUplinks()
        val p = phone(vault, uplinks)
        p.start()
        p.call { it.sync() }
        val before = keys.ensure()
        val firstLink = uplinks.made.single()
        val oldListener = uplinks.starts.single().listener
        oldListener.persist("paired-state".toByteArray()) // the link's state with the Mac in it
        assertTrue(vault.hasLinkState())
        uplinks.online = false // the store after forget stays as made: empty (online would sync it again at once)
        assertTrue(p.forgetThisMac() is Outcome.Ok)
        // the link's identity and pairing are gone: the new link starts with no state, a new phone to any Mac
        assertNull(uplinks.starts.last().state)
        // a late save from the stopped link never brings them back
        oldListener.persist("paired-state".toByteArray())
        assertFalse(vault.linkState()?.contentEquals("paired-state".toByteArray()) == true)
        assertEquals(1, firstLink.shutDown)
        assertEquals(1, firstLink.closed)
        val after = uplinks.starts.last().keys
        assertNotEquals(before.device, after.device)
        assertNotEquals(before.lock, after.lock)
        assertNotEquals(before.strong, after.strong)
        // the store is new and empty until the next sync
        val view = (p.call { it.conversations() } as Outcome.Ok).value
        assertTrue("$view", view.isEmpty())
        assertTrue(p.state.value is PhoneState.Open)
    }

    // decision: PHN-133
    @Test
    fun pairing_makes_destroyed_keys_anew_and_restarts_the_link_with_them() = runBlocking {
        val keys = SoftwareKeys()
        val uplinks = FakeUplinks()
        val p = phone(Vault(dir, MemoryProtector(), keys), uplinks)
        p.start()
        assertTrue((p.state.value as PhoneState.Open).keys.strong == KeyState.OK)
        keys.gone.add(KeyKind.STRONG) // a fingerprint was added
        p.call { } // nothing else changes
        val old = keys.pairs[KeyKind.STRONG]!!.public
        assertTrue(p.pair("FERB-INVENTED") is Outcome.Ok)
        assertEquals(2, uplinks.starts.size)
        assertEquals(1, uplinks.made.first().shutDown)
        assertEquals(listOf("FERB-INVENTED"), uplinks.made.last().paired)
        assertNotEquals(Proofs.publicKey(old), uplinks.starts.last().keys.strong)
        assertEquals(KeyState.OK, (p.state.value as PhoneState.Open).keys.strong)
    }

    // decision: PHN-498
    @Test
    fun close_frees_the_native_objects_so_nothing_keeps_the_phone_alive() {
        fun startAndMaybeClose(close: Boolean): WeakReference<FerbPhone> {
            val p = FerbPhone(Vault(File(dir, "p$close"), MemoryProtector(), SoftwareKeys()), FakeUplinks(), PhoneIdentity("n", "a"), FixedClock(), Counted(), main, Dispatchers.IO) { 0 }
            runBlocking { p.start() }
            if (close) p.close()
            return WeakReference(p)
        }
        fun collected(ref: WeakReference<FerbPhone>): Boolean {
            repeat(50) {
                System.gc()
                if (ref.get() == null) return true
                Thread.sleep(20)
            }
            return false
        }
        assertTrue("a closed phone is collected", collected(startAndMaybeClose(true)))
        // the control: an open phone is held by the library's callbacks (so the check above means something)
        val open = startAndMaybeClose(false)
        assertFalse("an open phone is held by its callbacks", collected(open))
        open.get()?.close()
    }
}
