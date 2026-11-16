package com.ferb.core

import android.content.pm.ApplicationInfo
import android.system.Os
import android.system.OsConstants
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.ferb.mobile.Clock
import com.ferb.mobile.Ids
import com.ferb.mobile.Item
import com.ferb.mobile.Mobile
import com.ferb.mobile.MobileConfig
import com.ferb.mobile.libraryVersion
import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.security.KeyStore
import java.security.Signature
import java.util.Base64
import java.util.UUID
import java.util.concurrent.atomic.AtomicInteger
import java.util.zip.ZipFile
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import org.junit.runner.RunWith

/**
 * :core on a real Android: the library through JNA, the Keystore, the vault, the backup rules. Every line it logs
 * under the tag ferb-core-test is evidence for journal 113 (`adb logcat -s ferb-core-test`).
 */
@RunWith(AndroidJUnit4::class)
class OnPhoneTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private fun say(line: String) = Log.i(TAG, line)

    private class FixedClock : Clock { override fun nowIso() = "2026-01-02T10:00:00+00:00" }
    private class Counted : Ids { val n = AtomicInteger(0); override fun newId() = "sub-${n.incrementAndGet()}" }

    /** The p_align of each PT_LOAD segment of an ELF64 little-endian file. */
    private fun loadAlignments(elf: ByteArray): List<Long> {
        val b = ByteBuffer.wrap(elf).order(ByteOrder.LITTLE_ENDIAN)
        check(b.getInt(0) == 0x464c457f) { "not ELF" }
        val phoff = b.getLong(0x20).toInt()
        val phentsize = b.getShort(0x36).toInt()
        val phnum = b.getShort(0x38).toInt()
        return (0 until phnum).mapNotNull { i ->
            val at = phoff + i * phentsize
            if (b.getInt(at) == 1) b.getLong(at + 0x30) else null
        }
    }

    // decision: PHN-446
    // decision: PHN-495
    @Test
    fun the_library_loads_through_jna_and_every_native_library_is_16k_aligned() {
        assertEquals("0.28.0", libraryVersion())
        say("page size ${Os.sysconf(OsConstants._SC_PAGESIZE)}; abi ${android.os.Build.SUPPORTED_ABIS.first()}; sdk ${android.os.Build.VERSION.SDK_INT}")
        val apk = context.applicationInfo.sourceDir
        val abi = android.os.Build.SUPPORTED_ABIS.first()
        ZipFile(apk).use { zip ->
            val libs = zip.entries().toList().filter { it.name.startsWith("lib/$abi/") && it.name.endsWith(".so") }
            assertTrue(libs.map { it.name }.toString(), libs.any { it.name.endsWith("libferb_mobile_ffi.so") })
            assertTrue(libs.map { it.name }.toString(), libs.any { it.name.endsWith("libjnidispatch.so") })
            for (e in libs) {
                val align = loadAlignments(zip.getInputStream(e).readBytes())
                say("${e.name}: ${e.size} bytes, stored ${e.method == java.util.zip.ZipEntry.STORED}, LOAD alignment $align")
                assertTrue("${e.name} $align", align.all { it >= 16384 })
                assertEquals("${e.name} is stored uncompressed", java.util.zip.ZipEntry.STORED, e.method)
            }
        }
    }

    // decision: PHN-448
    // decision: PHN-496
    @Test
    fun the_store_opens_and_closes_with_a_keystore_wrapped_key_and_nothing_is_backed_up() {
        val flags = context.applicationInfo.flags
        assertEquals("allowBackup is off", 0, flags and ApplicationInfo.FLAG_ALLOW_BACKUP)
        val dir = File(context.noBackupFilesDir, "ferb-test-${UUID.randomUUID()}")
        val protector = KeystoreProtector(context, "ferb.test.wrap")
        try {
            val vault = Vault(dir, protector, SoftwareKeys())
            val key = vault.storeKey()
            say("wrapping key in ${protector.place()}; vault under noBackupFilesDir ${dir.path.startsWith(context.noBackupFilesDir.path)}")
            val m = Mobile.open(vault.storeFile.path, key.copyOf(), MobileConfig("phone-1", 0, null, null, null), FixedClock(), Counted(), object : com.ferb.mobile.Listener { override fun changed(change: com.ferb.mobile.Change) {} })
            m.connectTransport(FakeMac())
            m.linkEvent("""{"type":"state","phase":"online","tier":"direct"}""")
            m.sync()
            m.shutdown()
            m.close()
            // a second vault object, as after a new process: the key comes back from the sealed file
            val again = Vault(dir, KeystoreProtector(context, "ferb.test.wrap"), SoftwareKeys()).storeKey()
            assertArrayEquals(key, again)
            val m2 = Mobile.open(vault.storeFile.path, again, MobileConfig("phone-1", 0, null, null, null), FixedClock(), Counted(), object : com.ferb.mobile.Listener { override fun changed(change: com.ferb.mobile.Change) {} })
            val view = m2.transcript("c1")
            assertEquals("Invented question", (view.items.single() as Item.Turn).turn.words?.text)
            m2.shutdown()
            m2.close()
            // the database on disk is not readable text
            assertFalse(String(vault.storeFile.readBytes(), Charsets.ISO_8859_1).contains("Invented question"))
        } finally {
            protector.destroy()
            dir.deleteRecursively()
        }
    }

    // decision: PHN-443
    // decision: PHN-497
    @Test
    fun the_keystore_keys_are_in_the_macs_form_and_the_prompted_ones_refuse_without_the_owner() {
        val keys = KeystoreKeys(context)
        keys.deleteAll()
        try {
            val pubs = keys.ensure()
            val secure = keys.deviceSecure()
            val strongBio = keys.strongBiometric()
            say("device secure $secure; strong biometric $strongBio; strongbox ${Keystore.hasStrongBox(context)}")
            for (kind in KeyKind.entries) say("$kind key: ${keys.state(kind)} in ${keys.place(kind)}")
            assertNotNull(pubs.device)
            assertEquals(secure, pubs.lock != null)
            assertEquals(secure && strongBio, pubs.strong != null)
            // the device key signs with no prompt, in the Mac's wire form; vectors for the Rust verifier
            val vectors = mutableListOf<String>()
            for (i in 1..3) {
                val payload = """{"challenge":"vector-$i","method":"link.present","params":{}}"""
                val sig = keys.deviceSign(payload)!!
                val pub = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.getCertificate(KeyKind.DEVICE.alias).publicKey
                assertTrue(Signature.getInstance(Proofs.ALGORITHM).run { initVerify(pub); update(payload.toByteArray()); verify(Base64.getDecoder().decode(sig)) })
                vectors += """{"what":"android device key $i","key":"${pubs.device}","payload":${quote(payload)},"sig":"$sig","valid":true}"""
            }
            vectors += """{"what":"android device key, another payload","key":"${pubs.device}","payload":"{}","sig":"${keys.deviceSign("""{"a":1}""")}","valid":false}"""
            Log.i(VECTORS, """{"vectors":[${vectors.joinToString(",")}]}""")
            // a prompted key: initSign works, signing without the owner's prompt does not
            for (kind in listOf(KeyKind.LOCK, KeyKind.STRONG)) {
                if (keys.state(kind) != KeyState.OK) continue
                val s = keys.signatureFor(kind)
                try {
                    s.update("x".toByteArray())
                    s.sign()
                    fail("$kind signed without the owner")
                } catch (e: Exception) {
                    say("$kind without a prompt: ${e.javaClass.simpleName}")
                }
            }
        } finally {
            keys.deleteAll()
        }
        assertEquals(KeyHealth(KeyState.NONE, KeyState.NONE, KeyState.NONE), keys.health())
    }

    // decision: PHN-408
    @Test
    fun forget_this_mac_wipes_the_store_its_key_and_the_keystore_keys() = runBlocking<Unit> {
        val dir = File(context.noBackupFilesDir, "ferb-test-${UUID.randomUUID()}")
        val keys = KeystoreKeys(context)
        val protector = KeystoreProtector(context, "ferb.test.wrap2")
        val uplinks = FakeUplinks()
        val phone = FerbPhone(Vault(dir, protector, keys), uplinks, PhoneIdentity("Invented phone", "test"), FixedClock(), Counted(), Dispatchers.Main, Dispatchers.IO) { 0 }
        try {
            assertTrue(phone.start() is PhoneState.Open)
            phone.call { it.sync() }
            val before = uplinks.starts.single().keys
            val storeKey = File(dir, "store.key").readBytes()
            uplinks.online = false // the store after forget stays as made: empty (online would sync it again at once)
            assertTrue(phone.forgetThisMac() is Outcome.Ok)
            val after = uplinks.starts.last().keys
            assertNotEquals(before.device, after.device)
            if (before.lock != null) assertNotEquals(before.lock, after.lock)
            assertFalse(storeKey.contentEquals(File(dir, "store.key").readBytes()))
            assertTrue((phone.call { it.conversations() } as Outcome.Ok).value.isEmpty())
            say("forget: new keys ${after.device != before.device}, new store key, empty store")
        } finally {
            phone.close()
            keys.deleteAll()
            protector.destroy()
            dir.deleteRecursively()
        }
    }

    // decision: PHN-508, PHN-496
    @Test
    fun two_threads_making_the_wrap_key_at_once_both_seal_with_the_one_key() {
        // S11 found it on the emulator: the link's first persist and the store's key made the key at the same time
        repeat(10) { round ->
            val alias = "ferb.test.race.$round"
            val a = KeystoreProtector(context, alias)
            val b = KeystoreProtector(context, alias)
            val start = java.util.concurrent.CountDownLatch(1)
            val sealed = arrayOfNulls<ByteArray>(2)
            val errors = java.util.concurrent.ConcurrentLinkedQueue<Throwable>()
            val threads = listOf(a, b).mapIndexed { i, p ->
                Thread { start.await(); runCatching { sealed[i] = p.seal("x$i".toByteArray(), "race") }.onFailure(errors::add) }.also(Thread::start)
            }
            start.countDown()
            threads.forEach(Thread::join)
            try {
                assertTrue("round $round: ${errors.map { it.javaClass.simpleName }}", errors.isEmpty())
                // both open with the key that is left: one key was made, not two
                assertEquals("x0", String(a.open(sealed[0]!!, "race")))
                assertEquals("x1", String(a.open(sealed[1]!!, "race")))
            } finally {
                a.destroy()
            }
        }
    }

    private fun quote(s: String) = "\"" + s.replace("\\", "\\\\").replace("\"", "\\\"") + "\""

    companion object {
        const val TAG = "ferb-core-test"
        const val VECTORS = "ferb-core-vectors"
    }
}

/**
 * The wrap and unwrap across a process's death: [a_seal] and [b_open] are run as two `am instrument` invocations
 * with `am force-stop` between (two processes); [b_open] then reads what [a_seal] sealed under the Keystore key.
 */
@RunWith(AndroidJUnit4::class)
class ProcessDeathTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private val file get() = File(context.noBackupFilesDir, "ferb-test-death/sealed.bin")
    private val protector get() = KeystoreProtector(context, "ferb.test.death")

    @Test
    fun a_seal() {
        SealedFiles.write(file, protector.seal("sealed in process ${android.os.Process.myPid()}".toByteArray(), "test"))
        Log.i(OnPhoneTest.TAG, "sealed in process ${android.os.Process.myPid()}")
    }

    @Test
    fun b_open() {
        val sealed = SealedFiles.read(file)
        org.junit.Assume.assumeTrue("run a_seal first, in another process", sealed != null)
        val text = String(protector.open(sealed!!, "test"))
        Log.i(OnPhoneTest.TAG, "opened in process ${android.os.Process.myPid()}: '$text'")
        assertTrue(text.startsWith("sealed in process "))
        // run alone after a_seal in another process (-e crossProcess true): it must have been another process
        if (InstrumentationRegistry.getArguments().getString("crossProcess") == "true") {
            assertNotEquals("sealed in process ${android.os.Process.myPid()}", text)
        }
        protector.destroy()
        file.parentFile?.deleteRecursively()
        assertNull(SealedFiles.read(file))
    }
}
