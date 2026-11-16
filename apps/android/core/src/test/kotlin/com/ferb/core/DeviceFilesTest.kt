package com.ferb.core

import com.ferb.mobile.Attachment
import com.ferb.mobile.Change
import com.ferb.mobile.Clock
import com.ferb.mobile.DevicePlace
import com.ferb.mobile.DeviceReport
import com.ferb.mobile.Ids
import com.ferb.mobile.LocationFix
import com.ferb.mobile.TransferListener
import java.io.File
import java.nio.file.Files
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The calls the phone app gained for T-1214 and T-1211's rest, through FerbPhone over the host library and the fake
 * Mac: the phone's own facts (sent once per change and again on a new connection), the Mac's request for one exact fix
 * and its answer, recent files, a search of names, a folder fetched whole from its listing, a folder sent whole by its
 * group, and a finished task's file by its place.
 */
class DeviceFilesTest {
    private val mainThread = Executors.newSingleThreadExecutor { Thread(it, "test-main") }
    private val main = mainThread.asCoroutineDispatcher()
    private val dir: File = Files.createTempDirectory("ferb-core-device").toFile()
    private val opened = CopyOnWriteArrayList<FerbPhone>()
    private val uplinks = FakeUplinks()

    private class FixedClock : Clock { override fun nowIso() = "2026-01-02T10:00:00+00:00" }
    private class Counted : Ids { val n = AtomicInteger(0); override fun newId() = "id-${n.incrementAndGet()}" }

    private fun phone(): FerbPhone =
        FerbPhone(Vault(File(dir, "vault"), MemoryProtector(), SoftwareKeys()), uplinks, PhoneIdentity("Invented phone", "test"), FixedClock(), Counted(), main, Dispatchers.IO) { 0 }
            .also(opened::add)

    private val mac get() = uplinks.made.last().mac

    private object Quiet : TransferListener {
        override fun progress(done: ULong, of: ULong) {}
        override fun accepted(uploadId: String, offset: ULong) {}
    }

    private fun <T> ok(o: Outcome<T>): T = if (o is Outcome.Ok) o.value else error("failed: $o")

    private fun report(battery: Int = 80, place: DevicePlace? = null) = DeviceReport(
        "Invented phone", "Invented Model", "Android 16 (API 36)", "Asia/Kolkata", 330, if (place != null) "on" else "off", place,
        battery.toUByte(), false, false, "wifi", "en-IN", listOf("en-IN", "hi-IN"), true, false,
    )

    @After
    fun tearDown() {
        opened.forEach { it.close() }
        main.close()
        dir.deleteRecursively()
    }

    // decision: SENSE-420
    // decision: PHN-530
    @Test
    fun the_phones_facts_go_once_per_change_and_again_on_a_new_connection() = runBlocking {
        val p = phone()
        p.start()
        val r = report(place = DevicePlace("Pune", "Maharashtra", "India", "IN"))
        assertTrue("sent at once while online", ok(p.call { it.reportDevice(r) }))
        assertFalse("the same again is not sent", ok(p.call { it.reportDevice(r) }))
        assertTrue(ok(p.call { it.reportDevice(report(battery = 75, place = r.place)) }))
        assertEquals(2, mac.reports.size)
        val sent = mac.reports.first()
        assertTrue(sent, sent.contains("\"city\":\"Pune\"") && sent.contains("\"time_zone\":\"Asia/Kolkata\""))
        assertFalse("never coordinates", sent.contains("latitude") || sent.contains("longitude"))
        // the link drops and comes back: the Mac hears the record again (the core's own sync)
        p.call { it.linkEvent("""{"type":"state","phase":"offline"}""") }
        p.call { it.linkEvent("""{"type":"state","phase":"online","tier":"direct"}""") }
        val until = System.currentTimeMillis() + 5_000
        while (mac.reports.size < 3 && System.currentTimeMillis() < until) Thread.sleep(10)
        assertEquals(3, mac.reports.size)
    }

    // decision: GATE-613, SENSE-420
    // decision: PHN-537
    @Test
    fun the_macs_request_for_the_exact_place_is_kept_until_answered() = runBlocking {
        val p = phone()
        p.start()
        val heard = launch { withTimeout(5_000) { p.changes.first { it is Change.LocationWanted } } }
        p.call { it.linkEvent("""{"type":"location_wanted","request_id":"loc-1"}""") }
        heard.join()
        assertEquals("loc-1", ok(p.call { it.locationWanted() }))
        ok(p.call { it.answerLocation("loc-1", LocationFix(18.52, 73.85, 12.0), null) })
        assertNull(ok(p.call { it.locationWanted() }))
        assertTrue(mac.reports.last().contains("\"exact\""))
        ok(p.call { it.answerLocation("loc-2", null, "The owner said no on the phone.") })
        assertTrue(mac.reports.last().contains("The owner said no on the phone."))
        assertFalse(mac.reports.last().contains("exact"))
    }

    // decision: PHN-113, PHN-121
    // decision: PHN-538
    @Test
    fun recent_files_and_a_search_of_names_come_from_the_mac() = runBlocking {
        val p = phone()
        p.start()
        mac.recent = """{"files":[{"name":"Report.pdf","path":"~/Downloads/Report.pdf","dir":false,"size":2048,"modified":1767348000,"source":"made","at":"2026-01-02T10:00:00+00:00","task_id":"t1","task_name":"Write the report"}],"skipped":2}"""
        val recent = ok(p.call { it.recentFiles(20u) })
        assertEquals("made", recent.files.single().source)
        assertEquals("Write the report", recent.files.single().taskName)
        assertEquals(2uL, recent.skipped)
        mac.search = """{"path":"~/Documents","query":"lease","files":[{"name":"Lease.pdf","path":"~/Documents/Lease.pdf","dir":false,"size":10,"modified":0}],"looked":5000,"skipped":1,"cut":false,"incomplete":true}"""
        val found = ok(p.call { it.searchFiles("lease", "~/Documents") })
        assertTrue(found.incomplete)
        assertEquals(5000uL, found.looked)
        assertTrue(mac.searches.single().contains("\"path\":\"~/Documents\""))
        // an empty query is refused before anything is asked
        assertTrue(p.call { it.searchFiles("  ", null) } is Outcome.Failed)
        assertEquals(1, mac.searches.size)
    }

    // decision: PHN-13, PHN-115, PHN-127
    // decision: PHN-533, PHN-534
    @Test
    fun a_folder_goes_whole_either_way() = runBlocking {
        val p = phone()
        p.start()
        mac.trees["~/Documents/Taxes"] = linkedMapOf("2025/return.pdf" to ByteArray(9) { 1 }, "notes.txt" to ByteArray(3) { 2 })
        mac.treeSkipped = 1
        val tree = ok(p.call { it.tree("~/Documents/Taxes", null) })
        assertEquals(listOf("2025/return.pdf", "notes.txt"), tree.files.map { it.rel })
        assertEquals(1uL, tree.skipped)
        val out = File(dir, "fetched/Taxes/2025/return.pdf").also { it.parentFile.mkdirs() }
        ok(p.call { it.fetch("f-1", "~/Documents/Taxes/2025/return.pdf", out.path, 0u, null, Quiet) })
        assertArrayEquals(ByteArray(9) { 1 }, out.readBytes())
        // sending one: the Mac starts the folder, each file goes with its group and its place inside
        val group = ok(p.call { it.putGroup("Scans") })
        assertEquals("g-1", group)
        val local = File(dir, "outgoing/a.jpg").also { it.parentFile.mkdirs(); it.writeBytes(ByteArray(6) { 3 }) }
        ok(p.call { it.upload("u-1", local.path, "a.jpg", null, group, "2026/a.jpg", Quiet) })
        assertTrue(mac.puts.single(), mac.puts.single().contains("\"group\":\"g-1\"") && mac.puts.single().contains("\"rel\":\"2026/a.jpg\""))
        val sent = ok(p.call { it.sendWith("The scans", listOf(Attachment(group, "Scans"))) })
        assertEquals(listOf(Attachment("g-1", "Scans")), sent.attachments)
    }

    // decision: PHN-535
    @Test
    fun a_finished_tasks_file_is_fetched_by_its_task_and_place() = runBlocking {
        val p = phone()
        p.start()
        mac.taskFiles["t1#1"] = ByteArray(7) { 5 }
        val out = File(dir, "fetched/t/summary.pdf").also { it.parentFile.mkdirs() }
        ok(p.call { it.fetchTaskFile("tf-1", "t1", 1u, out.path, Quiet) })
        assertArrayEquals(ByteArray(7) { 5 }, out.readBytes())
        val asked = mac.gets.last()
        assertTrue(asked, asked.contains("\"what\":\"file\"") && asked.contains("\"task_id\":\"t1\"") && !asked.contains("\"path\""))
    }
}
