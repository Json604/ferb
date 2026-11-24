package com.ferb.app

import android.app.Application
import android.os.Looper
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.Phase
import com.ferb.app.model.PickedFile
import com.ferb.app.model.PickedFolder
import com.ferb.app.model.Way
import com.ferb.app.model.Why
import com.ferb.core.KeyKind
import com.ferb.core.Outcome
import com.ferb.design.components.PhoneTab
import com.ferb.mobile.Attachment
import com.ferb.mobile.FerbException
import com.ferb.mobile.FileEntry
import com.ferb.mobile.Folder
import com.ferb.mobile.FoundFile
import com.ferb.mobile.FoundFiles
import com.ferb.mobile.RecentFiles
import com.ferb.mobile.Tree
import com.ferb.mobile.TreeFile
import java.io.File
import java.lang.ref.WeakReference
import java.nio.file.Files
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/**
 * T-1211's rest over a fake phone: a folder fetched whole and sent whole, recent files, a search of names, a finished
 * task's file from its card, and the hidden answer's secret handled as numbers only and gone after.
 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class FilesMoreTest {
    private val phone = FakePhone()
    private lateinit var session: Session
    private val root: File = Files.createTempDirectory("ferb-app-more").toFile()
    private val folders = HashMap<String, PickedFolder>()
    private val local = LocalFiles(root, pickFolder = { folders[it] }) { null }
    private var cancelPrompt = false
    private val signers = ArrayList<FakeSigner>()

    @Before
    fun setUp() {
        Dispatchers.setMain(UnconfinedTestDispatcher())
        session = Session { 0L }
        session.unlocked()
        session.foreground(true)
    }

    @After
    fun tearDown() {
        Dispatchers.resetMain()
        root.deleteRecursively()
    }

    private fun idle() = shadowOf(Looper.getMainLooper()).idle()

    private fun until(what: String = "", cond: () -> Boolean) {
        val end = System.currentTimeMillis() + 5_000
        while (!cond() && System.currentTimeMillis() < end) { idle(); Thread.sleep(10) }
        idle()
        assertTrue("waited for $what", cond())
    }

    private fun vm(): FerbViewModel =
        FerbViewModel(phone, session, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(), local, account = testAccount()).also {
            it.prompts = object : Prompts {
                override fun signer(kind: KeyKind, why: Why) = FakeSigner(kind, cancelPrompt).also(signers::add)
                override suspend fun local() = true
            }
        }

    private val taxes = Folder("~/Documents/Taxes", "~/Documents", false, listOf(FileEntry("notes.txt", "~/Documents/Taxes/notes.txt", false, false, 10u, 0, false)), 1u, false, 0u, null)

    /** "Load more" (PHN-609): a page at a time, added in place, while the Mac's page carries a marker; none, none shown. */
    // decision: PHN-609, PHN-606
    @Test
    fun load_more_adds_the_next_page_of_a_folder_and_of_a_search_in_place() {
        fun entry(n: Int) = FileEntry("f$n.txt", "~/Big/f$n.txt", false, false, 1u, 0, false)
        val first = Folder("~/Big", "~", false, (1..3).map(::entry), 7u, true, 0u, "p2")
        val second = Folder("~/Big", "~", false, (4..6).map(::entry), 7u, true, 3u, "p3")
        val last = Folder("~/Big", "~", false, listOf(entry(7)), 7u, false, 6u, null)
        phone.folders[first.path] = first
        phone.morePages += second; phone.morePages += last
        val vm = vm()
        vm.openFolder("~/Big")
        until("opened") { vm.ui.value.files.folder != null }
        assertEquals("p2", vm.ui.value.files.folder!!.next)
        vm.folderMore()
        until("page two") { vm.ui.value.files.folder!!.entries.size == 6 }
        assertTrue("files-more:~/Big:p2:false" in phone.calls)
        vm.folderMore()
        until("page three") { vm.ui.value.files.folder!!.entries.size == 7 }
        assertEquals((1..7).map { "f$it.txt" }, vm.ui.value.files.folder!!.entries.map { it.name })
        assertEquals("no more after the last page", null, vm.ui.value.files.folder!!.next)
        vm.folderMore()
        idle()
        assertEquals(2, phone.calls.count { it.startsWith("files-more") })
        // a search: the next page of matches under the first; the 100,000-name bound still says so on the last page
        fun hit(n: Int) = FoundFile("tax$n.pdf", "~/Big/tax$n.pdf", false, 1u, 0, "", null, null, null)
        val p1 = FoundFiles("~/Big", "tax", listOf(hit(1), hit(2)), 50_000u, 0u, true, false, 0u, "s2")
        val p2 = FoundFiles("~/Big", "tax", listOf(hit(3)), 100_000u, 0u, false, true, 2u, null)
        phone.found = Outcome.Ok(p1)
        phone.moreFound += p2
        vm.search("tax")
        until("found") { vm.ui.value.files.search?.found != null }
        assertEquals("s2", vm.ui.value.files.search!!.found!!.next)
        vm.searchMore()
        until("more found") { vm.ui.value.files.search!!.found!!.files.size == 3 }
        val s = vm.ui.value.files.search!!
        assertEquals(null, s.found!!.next)
        assertTrue(s.found!!.incomplete)
        assertTrue("search-more:tax:s2" in phone.calls)
    }

    // decision: PHN-13, PHN-115, PHN-11
    // decision: PHN-533
    @Test
    fun a_folder_is_fetched_whole_with_its_progress_pause_and_what_the_mac_left_out() {
        phone.folders[taxes.path] = taxes
        phone.trees[taxes.path] = Tree(taxes.path, "Taxes", listOf(TreeFile("2025/return.pdf", 10u), TreeFile("notes.txt", 10u)), 20u, 3u, true)
        val vm = vm()
        vm.openFolder(taxes.path)
        phone.fetchGate = CompletableDeferred()
        vm.fetchFolder()
        until("the first file half there") { vm.transfers.list.value.firstOrNull()?.done == 5L }
        val batch = vm.transfers.list.value.first().batch!!
        val b = vm.transfers.batches.value.getValue(batch)
        assertEquals("Taxes", b.folder)
        assertEquals(3L, b.skipped)
        assertTrue(b.cut)
        // paused: the second file waits with it
        vm.pause(vm.transfers.list.value.first().key)
        until("paused") { vm.transfers.list.value.first().phase == Phase.PAUSED }
        assertEquals(Phase.PAUSED, vm.transfers.list.value[1].phase)
        assertFalse(phone.calls.any { it.startsWith("fetch:~/Documents/Taxes/notes.txt") })
        phone.fetchGate = null
        vm.transfers.resume(vm.transfers.list.value.first().key)
        until("both here") { vm.transfers.list.value.all { it.phase == Phase.DONE } }
        assertTrue("fetch:~/Documents/Taxes/2025/return.pdf@5:false" in phone.calls)
        val files = vm.transfers.list.value.map { File(it.local) }
        assertTrue(files[0].path.endsWith("/Taxes/2025/return.pdf") && files[0].path.startsWith(local.fetched.path))
        files.forEach { assertArrayEquals(phone.fetchBytes, it.readBytes()) }
        // the lock removes them, as every fetched file (PHN-15)
        vm.transfers.locked()
        assertTrue(vm.transfers.list.value.isEmpty())
        assertFalse(files[0].exists())
        // a key place's folder is not fetched whole (each of its files asks by itself)
        phone.folders["~/Documents/Keys"] = Folder("~/Documents/Keys", "~/Documents", true, emptyList(), 0u, false, 0u, null)
        vm.openFolder("~/Documents/Keys", keyPlace = true)
        vm.fetchFolder()
        assertFalse(phone.calls.any { it.startsWith("tree:~/Documents/Keys") })
    }

    // decision: PHN-127, PHN-115
    // decision: PHN-534
    @Test
    fun a_picked_folder_is_sent_whole_and_rides_with_a_message_as_one() {
        phone.view = com.ferb.mobile.TranscriptView("c1", emptyList(), emptyList(), emptyList(), emptyList(), com.ferb.mobile.OlderState.Complete)
        folders["content://tree/Scans"] = PickedFolder("Scans", listOf(
            PickedFile("a.jpg", 4) { ByteArray(4) { 1 }.inputStream() },
            PickedFile("2026/b.jpg", 6) { ByteArray(6) { 2 }.inputStream() },
            PickedFile("broken.jpg", 3) { null },
        ))
        val vm = vm()
        vm.sendFolder("content://tree/Scans", forChat = false)
        until("sent") { vm.transfers.list.value.isNotEmpty() && vm.transfers.list.value.all { it.phase == Phase.DONE } }
        assertEquals("one folder on the Mac", 1, phone.calls.count { it == "group:Scans" })
        assertEquals(listOf(Triple("a.jpg", "g-1", "a.jpg"), Triple("b.jpg", "g-1", "2026/b.jpg")), phone.uploads)
        val batch = vm.transfers.list.value.first().batch!!
        assertEquals(1, vm.transfers.batches.value.getValue(batch).unreadable)
        assertTrue("copies go once sent", File(local.outgoing, batch).walk().none { it.isFile })
        vm.attachToChat(batch)
        assertEquals(listOf(Attachment("g-1", "Scans")), vm.transfers.ready.value)
        vm.typed("The scans")
        vm.send()
        assertEquals(listOf(Attachment("g-1", "Scans")), phone.sentWith.single().second)
        // a folder that can't be read is said
        vm.sendFolder("content://tree/gone", forChat = false)
        until("said") { vm.ui.value.note?.res == R.string.files_folder_unreadable }
    }

    // decision: PHN-113
    // decision: PHN-538
    @Test
    fun recent_files_and_a_search_of_names_show_with_what_the_mac_left_out() {
        phone.recent = Outcome.Ok(RecentFiles(listOf(
            FoundFile("Report.pdf", "~/Downloads/Report.pdf", false, 10u, 0, "made", "2026-01-02T10:00:00+00:00", "t1", "Write the report"),
        ), 2u))
        phone.folders["~/Documents"] = Folder("~/Documents", "~", false, emptyList(), 0u, false, 0u, null)
        phone.folders[taxes.path] = taxes
        val vm = vm()
        vm.choose(PhoneTab.FILES)
        assertTrue("recent:20" in phone.calls)
        assertEquals("Write the report", vm.ui.value.files.recent!!.files.single().taskName)
        assertEquals(2uL, vm.ui.value.files.recent!!.skipped)
        // a search inside the folder open
        vm.openFolder("~/Documents")
        phone.found = Outcome.Ok(FoundFiles("~/Documents", "tax", listOf(
            FoundFile("Taxes", taxes.path, true, 0u, 0, "", null, null, null),
            FoundFile("tax.pdf", "~/Documents/tax.pdf", false, 10u, 0, "", null, null, null),
        ), 4000u, 1u, false, true, 0u, null))
        vm.search("  tax ")
        assertTrue("search:tax:~/Documents" in phone.calls)
        val s = vm.ui.value.files.search!!
        assertFalse(s.loading)
        assertTrue(s.found!!.incomplete)
        // a folder found opens in the browser; a file found is fetched
        vm.openFound(s.found!!.files[1])
        until("fetched") { vm.transfers.list.value.singleOrNull()?.phase == Phase.DONE }
        vm.openFound(s.found!!.files[0])
        assertNull(vm.ui.value.files.search)
        assertEquals(taxes.path, vm.ui.value.files.folder?.path)
        // nothing matched: an empty answer, said as such on screen
        phone.found = null
        vm.search("x")
        assertEquals(0, vm.ui.value.files.search!!.found!!.files.size)
    }

    // decision: PHN-535
    @Test
    fun a_finished_tasks_file_is_fetched_from_its_card_by_the_task_and_its_place() {
        phone.taskFiles["t1" to 1L] = ByteArray(7) { 5 }
        val vm = vm()
        vm.fetchTaskFile("t1", 1, "summary.pdf")
        until("fetched") { vm.transfers.ofTask("t1", 1)?.phase == Phase.DONE }
        val t = vm.transfers.ofTask("t1", 1)!!
        assertEquals(Way.FETCH, t.way)
        assertArrayEquals(ByteArray(7) { 5 }, File(t.local).readBytes())
        assertTrue("task-file:t1:1" in phone.calls)
        assertFalse("no path was sent", phone.calls.any { it.startsWith("fetch:") })
        // a tap again doesn't fetch it twice
        vm.fetchTaskFile("t1", 1, "summary.pdf")
        assertEquals(1, phone.calls.count { it == "task-file:t1:1" })
        // a file the Mac can't find says so
        vm.fetchTaskFile("t1", 9, "gone.pdf")
        until("refused") { vm.transfers.ofTask("t1", 9)?.phase == Phase.FAILED }
        assertEquals(R.string.err_mac_said, vm.transfers.ofTask("t1", 9)!!.problem?.res)
    }

    /** Runs one hidden answer and returns the typed array and a weak hold on the library's input. */
    private fun hidden(vm: FerbViewModel, id: String): CharArray {
        val typed = "hunter2".toCharArray()
        vm.answerHidden(id, typed, 7)
        assertTrue("zeroed at once, before anything else", typed.all { it == '\u0000' })
        until("answered") { "answer:$id" !in vm.ui.value.busy }
        return typed
    }

    private fun collected(ref: WeakReference<*>): Boolean {
        repeat(20) {
            System.gc()
            System.runFinalization()
            if (ref.get() == null) return true
            Thread.sleep(20)
        }
        return false
    }

    // decision: PHN-110, PHN-109
    // decision: PHN-536
    // (Android 13's sandbox: from 14 on, UniFFI's cleaner is android.system.SystemCleaner, which a JVM can't load)
    @Config(application = Application::class, sdk = [33])
    @Test
    fun a_hidden_answer_goes_as_numbers_and_nothing_keeps_it_after_sent_refused_or_cancelled() {
        val vm = vm()
        // sent
        val sent = hidden(vm, "ap-1")
        assertEquals(listOf(7), phone.hiddenUnits)
        assertEquals("signed by the lock key", KeyKind.LOCK, signers.last().kind)
        var ref = phone.hiddenSecret!!
        assertTrue("the library's input is destroyed", runCatching { ref.get()?.len() }.let { it.isFailure || ref.get() == null })
        assertTrue("nothing the app holds still references it", collected(ref))
        assertTrue(sent.all { it == '\u0000' })
        // refused by the Mac
        phone.hiddenScript.add(Outcome.Failed(FerbException.Refused("That question isn't waiting any more.", "question_not_waiting")))
        hidden(vm, "ap-2")
        ref = phone.hiddenSecret!!
        assertTrue(runCatching { ref.get()?.len() }.let { it.isFailure || ref.get() == null })
        assertTrue(collected(ref))
        assertEquals(R.string.err_mac_said, vm.ui.value.note?.res)
        // the prompt cancelled: nothing sent, nothing kept (the owner types it again)
        cancelPrompt = true
        hidden(vm, "ap-3")
        ref = phone.hiddenSecret!!
        assertTrue(runCatching { ref.get()?.len() }.let { it.isFailure || ref.get() == null })
        assertTrue(collected(ref))
        // the view model's state holds no secret: only ids and words the Mac gave
        assertFalse(vm.ui.value.toString().contains("hunter2"))
        assertNull(vm.sheet.value)
    }
}
