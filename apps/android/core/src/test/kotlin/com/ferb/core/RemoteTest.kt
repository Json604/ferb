package com.ferb.core

import com.ferb.mobile.Attachment
import com.ferb.mobile.Change
import com.ferb.mobile.Clock
import com.ferb.mobile.FerbException
import com.ferb.mobile.Ids
import com.ferb.mobile.PreviewFrame
import com.ferb.mobile.PreviewSink
import com.ferb.mobile.Signer
import com.ferb.mobile.TransferListener
import java.io.File
import java.nio.file.Files
import java.util.Base64
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.async
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * T-1221's calls through FerbPhone over the host library and the fake Mac: the lock, Stop everything, the Mac's
 * status, files with key places, fetches and uploads with pause and resume, the public relay's refusal, the live
 * preview, FERB's settings, the Mac's tasks, the Mac's name and a key registered after pairing.
 */
class RemoteTest {
    private val mainThread = Executors.newSingleThreadExecutor { Thread(it, "test-main") }
    private val main = mainThread.asCoroutineDispatcher()
    private val dir: File = Files.createTempDirectory("ferb-core-remote").toFile()
    private val opened = CopyOnWriteArrayList<FerbPhone>()
    private val keys = SoftwareKeys()
    private val uplinks = FakeUplinks()

    private class FixedClock : Clock { override fun nowIso() = "2026-01-02T10:00:00+00:00" }
    private class Counted : Ids { val n = AtomicInteger(0); override fun newId() = "id-${n.incrementAndGet()}" }

    private fun phone(k: SoftwareKeys = keys): FerbPhone =
        FerbPhone(Vault(File(dir, "vault"), MemoryProtector(), k), uplinks, PhoneIdentity("Invented phone", "test"), FixedClock(), Counted(), main, Dispatchers.IO) { 0 }
            .also(opened::add)

    private val mac get() = uplinks.made.last().mac

    @After
    fun tearDown() {
        opened.forEach { it.close() }
        main.close()
        dir.deleteRecursively()
    }

    private fun signer(kind: KeyKind, k: SoftwareKeys = keys, asked: MutableList<KeyKind>? = null) = object : Signer {
        override fun sign(payload: String): String? { asked?.add(kind); return k.sign(kind, payload) }
    }

    private class Progress : TransferListener {
        val done = CopyOnWriteArrayList<Long>()
        val accepted = CopyOnWriteArrayList<String>()
        override fun progress(done: ULong, of: ULong) { this.done.add(done.toLong()) }
        override fun accepted(uploadId: String, offset: ULong) { accepted.add("$uploadId@$offset") }
    }

    private fun <T> ok(o: Outcome<T>): T = if (o is Outcome.Ok) o.value else error("failed: $o")

    // decision: PHN-15, PHN-125
    // decision: PHN-510
    @Test
    fun locking_tells_the_mac_and_only_the_status_is_read_until_the_unlock() = runBlocking {
        val p = phone()
        p.start()
        assertTrue(ok(p.lock()))
        assertTrue("link.lock" in mac.calls)
        assertTrue(ok(p.call { it.linkState() }).locked)
        val before = mac.calls.count { it == "chat.status" }
        val sync = p.call { it.sync() } as Outcome.Failed
        assertTrue(sync.error is FerbException.NeedsUnlock)
        assertEquals("only the status is read while locked", before + 1, mac.calls.count { it == "chat.status" })
        // Stop works locked, with no prompt
        assertTrue(p.stopEverything() is Outcome.Ok)
        assertTrue("stop.engage" in mac.calls)
        // an offline unlock by the prompt alone clears it
        ok(p.unlocked())
        assertFalse(ok(p.call { it.linkState() }).locked)
        assertTrue(p.call { it.sync() } is Outcome.Ok)
    }

    // decision: PHN-417, PHN-470, PHN-126
    @Test
    fun the_macs_pushed_status_says_what_each_question_needs() = runBlocking {
        val p = phone()
        val heard = CopyOnWriteArrayList<Change>()
        val collector = CoroutineScope(Dispatchers.Unconfined).launch { p.changes.collect { heard.add(it) } }
        p.start()
        val status = """{"locked":true,"words":true,"stopped":true,"approvals":[
            {"approval_id":"a1","kind":"approve","reason":"Go ahead?","options":["Yes","No"],"phone":{"can_answer":true,"needs_unlock":false}},
            {"approval_id":"a2","kind":"approve","capability":"ferb.confirm","reason":"Pay $5?","options":["Yes","No"],"phone":{"can_answer":true,"needs_unlock":true,"fingerprint":true,"challenge":"ch-2"}},
            {"approval_id":"a3","kind":"input","reason":"","options":[],"held":true,"phone":{"can_answer":false,"needs_unlock":true,"hidden":true,"error":{"message":"Answer it on your Mac."}}}],
            "running":[],"results":[{"task_id":"t1","status":"done","name":"Back up","answer":"Copied.","held":false}]}""".replace("\n", "")
        p.call { it.linkEvent("""{"type":"status","status":$status}""") }
        val until = System.currentTimeMillis() + 5_000
        while (heard.none { it is Change.Status } && System.currentTimeMillis() < until) Thread.sleep(10)
        assertTrue(heard.any { it is Change.Status })
        val s = ok(p.call { it.lastStatus() })!!
        assertTrue(s.stopped)
        val (a1, a2, a3) = s.questions
        assertTrue("routine", a1.needs.canAnswer && !a1.needs.needsUnlock && !a1.needs.hidden)
        assertTrue(a2.needs.needsUnlock && a2.needs.fingerprint)
        assertEquals("ch-2", a2.needs.challenge)
        assertFalse(a3.needs.canAnswer)
        assertTrue(a3.held && a3.needs.hidden)
        assertEquals("Answer it on your Mac.", a3.needs.problem)
        assertEquals("Back up", s.results.single().name)
        collector.cancel()
    }

    // decision: PHN-113, PHN-121, PHN-129
    @Test
    fun a_key_place_lists_only_with_a_fresh_proof_for_that_path() = runBlocking {
        val p = phone()
        p.start()
        mac.folders["~"] = listOf(FakeEntry("Desktop", dir = true), FakeEntry(".ssh", dir = true, locked = true))
        mac.folders["~/.ssh"] = listOf(FakeEntry("id_ed25519", size = 411, locked = true))
        mac.keyPlaces.add("~/.ssh")
        val home = ok(p.call { it.files(null, null) })
        assertEquals(listOf("~/Desktop", "~/.ssh"), home.entries.map { it.path })
        assertTrue(home.entries[1].locked && home.entries[1].dir)
        val refused = p.call { it.files("~/.ssh", null) } as Outcome.Failed
        assertTrue(refused.error is FerbException.NeedsUnlock)
        assertEquals("fresh_unlock_required", refused.error.linkCode)
        val asked = ArrayList<KeyKind>()
        val ssh = ok(p.call { it.files("~/.ssh", signer(KeyKind.STRONG, asked = asked)) })
        assertTrue(ssh.locked)
        assertEquals(listOf(KeyKind.STRONG), asked)
        assertTrue("signed over the path", mac.proofs.last().contains("~/.ssh"))
    }

    // decision: PHN-13, PHN-115
    @Test
    fun a_fetch_reports_progress_pauses_and_goes_on_where_it_stopped() = runBlocking {
        val p = phone()
        p.start()
        val bytes = ByteArray(40) { it.toByte() }
        mac.files["~/Documents/Lease.pdf"] = bytes
        mac.gate = CountDownLatch(1)
        val out = File(dir, "fetched/Lease.pdf").also { it.parentFile.mkdirs() }
        val progress = Progress()
        val first = async(Dispatchers.IO) { p.call { it.fetch("f-1", "~/Documents/Lease.pdf", out.path, 0u, null, progress) } }
        val until = System.currentTimeMillis() + 5_000
        while (progress.done.isEmpty() && System.currentTimeMillis() < until) Thread.sleep(5)
        ok(p.call { it.cancelTransfer("f-1") })
        val paused = first.await() as Outcome.Failed
        assertEquals("cancelled", paused.code)
        assertEquals(4L, out.length())
        mac.gate = null
        val done = ok(p.call { it.fetch("f-2", "~/Documents/Lease.pdf", out.path, out.length().toULong(), null, Progress()) })
        assertArrayEquals(bytes, out.readBytes())
        assertEquals(40uL, done.total)
        assertEquals("Lease.pdf", done.name)
    }

    // decision: PHN-127, PHN-115
    @Test
    fun an_upload_resumes_by_its_id_and_rides_with_a_message() = runBlocking {
        val p = phone()
        p.start()
        val local = File(dir, "outgoing/photo.jpg").also { it.parentFile.mkdirs(); it.writeBytes(ByteArray(20) { 9 }) }
        mac.gate = CountDownLatch(1)
        val progress = Progress()
        val first = async(Dispatchers.IO) { p.call { it.upload("u-1", local.path, "photo.jpg", null, null, null, progress) } }
        val until = System.currentTimeMillis() + 5_000
        while (progress.done.isEmpty() && System.currentTimeMillis() < until) Thread.sleep(5)
        ok(p.call { it.cancelTransfer("u-1") })
        assertEquals("cancelled", (first.await() as Outcome.Failed).code)
        assertEquals(listOf("up-1@0"), progress.accepted)
        mac.gate = null
        val again = Progress()
        val up = ok(p.call { it.upload("u-2", local.path, "photo.jpg", "up-1", null, null, again) })
        assertEquals("up-1", up.uploadId)
        assertEquals(listOf("up-1@4"), again.accepted)
        assertEquals(20, mac.uploads["up-1"]!!.size)
        val sent = ok(p.call { it.sendWith("Here they are", listOf(Attachment(up.uploadId, "photo.jpg"))) })
        assertEquals(listOf(Attachment("up-1", "photo.jpg")), sent.attachments)
    }

    // decision: PHN-410, PHN-444, PHN-454
    @Test
    fun files_and_the_preview_wait_for_a_better_path_on_the_public_relay() = runBlocking {
        val p = phone()
        p.start()
        mac.files["~/a.txt"] = ByteArray(3)
        p.call { it.linkEvent("""{"type":"state","phase":"online","tier":"shared_relay"}""") }
        assertFalse(ok(p.call { it.linkState() }).bulkAllowed)
        val out = File(dir, "a.txt").path
        val refused = p.call { it.fetch("f", "~/a.txt", out, 0u, null, Progress()) } as Outcome.Failed
        assertTrue(refused.error is FerbException.NeedsBetterPath)
        assertTrue("nothing was asked", mac.gets.isEmpty())
        assertTrue((p.call { it.preview("t1", null) } as Outcome.Failed).error is FerbException.NeedsBetterPath)
        p.call { it.linkEvent("""{"type":"bulk","allowed":true,"tier":"own_relay"}""") }
        assertTrue(ok(p.call { it.linkState() }).bulkAllowed)
        assertTrue(p.call { it.fetch("f2", "~/a.txt", out, 0u, null, Progress()) } is Outcome.Ok)
    }

    // decision: PHN-454
    @Test
    fun watch_live_hands_frames_in_memory_and_stops() = runBlocking {
        val p = phone()
        p.start()
        val jpeg = byteArrayOf(-1, -40, -1, -32, 1, 2, 3)
        val b64 = Base64.getEncoder().encodeToString(jpeg)
        mac.frames = listOf(
            """{"meta":{"seq":1,"state":"working","line":"Reading delivery options","fresh":true,"busy":false},"size":7,"received":7,"data":"$b64"}""",
            """{"meta":{"seq":1,"state":"working","line":"Comparing","fresh":false,"busy":true},"size":0,"received":0}""",
        )
        val frames = CopyOnWriteArrayList<PreviewFrame>()
        val ended = CountDownLatch(1)
        var endCode: String? = "unset"
        val live = ok(p.call {
            it.watchLive("t1", object : PreviewSink {
                override fun frame(frame: PreviewFrame) { frames.add(frame) }
                override fun ended(code: String?, detail: String?) { endCode = code; ended.countDown() }
            })
        })
        val until = System.currentTimeMillis() + 5_000
        while (frames.size < 2 && System.currentTimeMillis() < until) Thread.sleep(10)
        assertArrayEquals(jpeg, frames[0].jpeg)
        assertEquals("Reading delivery options", frames[0].line)
        assertNull("the same frame is not sent again", frames[1].jpeg)
        assertTrue(mac.gets.last().contains("\"have\":1"))
        live.stop()
        assertTrue(ended.await(5, TimeUnit.SECONDS))
        assertNull("stopped by the app: no code", endCode)
    }

    // decision: PHN-404, PHN-112, PHN-133
    @Test
    fun settings_are_read_and_each_change_is_signed() = runBlocking {
        val p = phone()
        p.start()
        mac.settings["sense.window_titles"] = "true"
        mac.settings["retention.trace_days"] = "30"
        mac.settings["privacy.exclude_apps"] = """["Password manager"]"""
        val values = ok(p.call { it.settings() }).associate { it.key to it.valueJson }
        assertEquals("30", values["retention.trace_days"])
        assertEquals("""["Password manager"]""", values["privacy.exclude_apps"])
        val asked = ArrayList<KeyKind>()
        val stored = ok(p.call { it.setSetting("retention.trace_days", "14", signer(KeyKind.STRONG, asked = asked)) })
        assertEquals("14", stored)
        assertEquals(listOf(KeyKind.STRONG), asked)
        assertTrue(mac.proofs.last().contains("settings.set"))
        assertEquals("not_signed", (p.call { it.setSetting("retention.trace_days", "7", object : Signer { override fun sign(payload: String): String? = null }) } as Outcome.Failed).code)
    }

    // decision: PHN-13
    @Test
    fun the_macs_tasks_and_its_name_come_from_the_mac() = runBlocking {
        val p = phone()
        val heard = CopyOnWriteArrayList<Change>()
        val collector = CoroutineScope(Dispatchers.Unconfined).launch { p.changes.collect { heard.add(it) } }
        p.start()
        mac.tasks = """{"tasks":[{"task_id":"t1","name":"Compare desks","goal":"Compare the desks","status":"working","created_at":"2026-01-02T09:00:00+00:00","answer":"","steps":{"done":2,"total":3}},
            {"task_id":"t0","name":"Back up","goal":"","status":"done","answer":"Copied to the drive."}],"more":true}""".replace("\n", "")
        val page = ok(p.call { it.tasks(30u, null) })
        assertTrue(page.more)
        assertEquals(listOf("t1", "t0"), page.tasks.map { it.taskId })
        assertEquals(2u, page.tasks[0].stepsDone)
        assertNull(ok(p.call { it.cachedMacInfo() }))
        val info = ok(p.call { it.macInfo() })
        assertEquals("MacBook Pro", info.name)
        assertEquals("2026-11-10T09:00:00+05:30", info.pairedAt)
        assertEquals(info, ok(p.call { it.cachedMacInfo() }))
        val until = System.currentTimeMillis() + 5_000
        while (heard.none { it is Change.Mac } && System.currentTimeMillis() < until) Thread.sleep(10)
        assertTrue(heard.any { it is Change.Mac })
        collector.cancel()
    }

    // decision: PHN-133
    @Test
    fun a_fingerprint_enrolled_after_pairing_is_registered_under_the_lock_key() = runBlocking {
        val k = SoftwareKeys(fingerprint = false)
        val p = phone(k)
        p.start()
        assertTrue(p.pair("FERB-INVENTED") is Outcome.Ok)
        assertNull("nothing to register right after pairing", p.unregisteredKeys())
        k.fingerprint = true
        val fresh = p.unregisteredKeys()
        assertNotNull(fresh!!.strong)
        val asked = ArrayList<KeyKind>()
        assertTrue(ok(p.registerKeys { kind -> signer(kind, k, asked) }))
        assertEquals("the Mac holds no fingerprint key yet: the lock key signs", listOf(KeyKind.LOCK), asked)
        assertTrue("link.keys" in mac.calls)
        assertTrue(mac.proofs.last().contains(fresh.strong!!.take(20)))
        assertNull(p.unregisteredKeys())
        assertFalse(ok(p.registerKeys { kind -> signer(kind, k, asked) }))
    }
}
