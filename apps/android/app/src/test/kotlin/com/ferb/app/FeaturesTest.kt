package com.ferb.app

import android.app.Application
import android.os.Looper
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.Phase
import com.ferb.app.model.Picked
import com.ferb.app.model.Way
import com.ferb.app.model.Why
import com.ferb.core.KeyHealth
import com.ferb.core.KeyKind
import com.ferb.core.KeyState
import com.ferb.core.Outcome
import com.ferb.core.PhoneState
import com.ferb.core.PublicKeys
import com.ferb.design.components.MarkKind
import com.ferb.design.components.PhoneTab
import com.ferb.mobile.AnswerInput
import com.ferb.mobile.FerbException
import com.ferb.mobile.FileEntry
import com.ferb.mobile.Folder
import com.ferb.mobile.LinkPath
import com.ferb.mobile.MacInfo
import com.ferb.mobile.OlderState
import com.ferb.mobile.PhoneStatus
import com.ferb.mobile.PreviewFrame
import com.ferb.mobile.QuestionNeeds
import com.ferb.mobile.StatusQuestion
import com.ferb.mobile.TaskPage
import com.ferb.mobile.TaskRow
import com.ferb.mobile.TranscriptView
import java.io.File
import java.nio.file.Files
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
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

/** T-1221's features over a fake phone: the lock, Stop everything, the Mac's status, files, Watch live, settings, tasks. */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class FeaturesTest {
    private val phone = FakePhone()
    private lateinit var session: Session
    private val signers = ArrayList<FakeSigner>()
    private val whys = ArrayList<Why>()
    private var cancelPrompt = false
    private val root: File = Files.createTempDirectory("ferb-app-features").toFile()
    private val picked = HashMap<String, ByteArray>()
    private val local = LocalFiles(root) { uri -> picked[uri]?.let { b -> Picked(uri.substringAfterLast('/'), b.size.toLong()) { b.inputStream() } } }

    @Before
    fun setUp() {
        Dispatchers.setMain(UnconfinedTestDispatcher())
        session = Session { 0L }
    }

    @After
    fun tearDown() {
        Dispatchers.resetMain()
        root.deleteRecursively()
    }

    private fun idle() = shadowOf(Looper.getMainLooper()).idle()

    /** Work that hops to the IO threads (files on disk) comes back by itself: wait for it, bounded. */
    private fun until(what: String = "", cond: () -> Boolean) {
        val end = System.currentTimeMillis() + 5_000
        while (!cond() && System.currentTimeMillis() < end) { idle(); Thread.sleep(10) }
        idle()
        assertTrue("waited for $what", cond())
    }

    private fun vm(): FerbViewModel =
        FerbViewModel(phone, session, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(), local).also {
            it.prompts = object : Prompts {
                override fun signer(kind: KeyKind, why: Why) = FakeSigner(kind, cancelPrompt).also { s -> signers.add(s); whys.add(why) }
                override suspend fun local() = true
            }
        }

    private fun needs(unlock: Boolean, fingerprint: Boolean = false, can: Boolean = true, hidden: Boolean = false) =
        QuestionNeeds(can, unlock, hidden, fingerprint, "ch-1", if (can) null else "Answer it on your Mac.")

    private fun asked(id: String, n: QuestionNeeds, held: Boolean = false) =
        StatusQuestion(id, "approve", "ferb.confirm", "t-1", if (held) "FERB has a question" else "Pay $5?", if (held) emptyList() else listOf("Yes", "No"), held, n)

    private fun status(vararg q: StatusQuestion, stopped: Boolean = false) = PhoneStatus(false, true, stopped, q.toList(), emptyList(), emptyList())

    private fun empty() = TranscriptView("c1", emptyList(), emptyList(), emptyList(), emptyList(), OlderState.Complete)

    // decision: PHN-15, PHN-125
    // decision: PHN-515
    // decision: PHN-510
    @Test
    fun locking_tells_the_mac_once_and_the_fetched_files_go() {
        val scope = MainScope()
        File(local.fetched, "k/a.txt").apply { parentFile!!.mkdirs(); writeText("x") }
        tellMacOnLock(scope, session, phone, local)
        idle()
        assertFalse("a cold start is locked already: nothing told", "lock" in phone.calls)
        session.unlocked()
        idle()
        session.lockNow()
        until("the wipe") { !local.fetched.exists() }
        assertEquals(1, phone.calls.count { it == "lock" })
        scope.cancel()
    }

    // decision: PHN-13, PHN-125
    @Test
    fun an_offline_unlock_tells_the_library_and_reads_the_macs_name_when_online() {
        phone.link.value = FakePhone.status(com.ferb.mobile.LinkPhase.WAITING, paired = false)
        val vm = vm()
        vm.unlockApp()
        // the prompt alone opened the app (the Mac out of reach): the library is told
        until("unlocked") { "unlocked" in phone.calls }
        assertFalse(session.locked.value)
        // online: the Mac's unlock, then its name
        session.lockNow()
        phone.link.value = FakePhone.status()
        phone.mac = MacInfo("Studio Mac", "2026-11-10T09:00:00+05:30", "Pixel")
        vm.unlockApp()
        idle()
        assertEquals("Studio Mac", vm.ui.value.mac?.name)
    }

    // decision: PHN-125, DMN-1
    @Test
    fun stop_everything_after_a_plain_confirmation_with_no_prompt() {
        session.unlocked()
        phone.status = status(stopped = true)
        val vm = vm()
        vm.show(Sheet.StopAll)
        vm.stopEverything()
        assertTrue("stop-everything" in phone.calls)
        assertTrue("no prompt", signers.isEmpty())
        assertNull(vm.sheet.value)
        assertEquals(R.string.stop_all_done, vm.ui.value.note?.res)
        assertTrue(vm.ui.value.status!!.stopped)
    }

    // decision: PHN-417, PHN-470, PHN-137
    // decision: PHN-520
    @Test
    fun a_question_that_needs_the_fingerprint_asks_for_it_up_front_and_a_plain_no_never() {
        session.unlocked()
        phone.view = empty()
        phone.status = status(asked("ap-1", needs(unlock = true, fingerprint = true)), asked("ap-2", needs(unlock = true)), asked("ap-3", needs(unlock = false)))
        val vm = vm()
        assertEquals(3, vm.ui.value.questions.size)
        vm.answer("ap-1", AnswerInput.Approve(true, 1))
        assertEquals("the fingerprint key, before the first try", listOf(KeyKind.STRONG), signers.map { it.kind })
        assertEquals(listOf("answer:ap-1:true"), phone.calls.filter { it.startsWith("answer") })
        vm.answer("ap-2", AnswerInput.Approve(true, 1))
        assertEquals("no fingerprint key needed: the lock key", KeyKind.LOCK, signers.last().kind)
        signers.clear()
        vm.answer("ap-1", AnswerInput.Approve(false, null))
        vm.answer("ap-3", AnswerInput.Approve(true, 1))
        assertTrue("a plain No and a routine Yes ask nothing", signers.isEmpty())
        // the refusal fallback stays: unknown needs, the Mac asks, one prompt
        phone.status = null
        phone.view = empty()
        vm.refresh()
        phone.answerScript.add(FakePhone.refused("unlock_required"))
        vm.answer("ap-9", AnswerInput.Approve(true, 1))
        assertEquals(listOf(KeyKind.STRONG), signers.map { it.kind })
    }

    // decision: PHN-125, PHN-126
    // decision: PHN-519
    // decision: PHN-511
    @Test
    fun the_macs_status_brings_questions_the_chat_does_not_hold_yet() {
        session.unlocked()
        phone.view = empty()
        phone.status = status(asked("ap-7", needs(unlock = false)), asked("ap-8", needs(unlock = true, can = false)))
        val vm = vm()
        val (a, b) = vm.ui.value.questions
        assertTrue(a.routine)
        assertEquals(listOf("Yes", "No"), a.options)
        assertTrue(b.onlyOnMac)
        assertEquals(listOf("ap-7", "ap-8"), vm.ui.value.work.waiting.map { it.approvalId })
    }

    // decision: PHN-13
    @Test
    fun the_chat_on_screen_is_marked_read_at_its_latest_entry() {
        session.unlocked()
        session.foreground(true)
        phone.view = empty()
        val vm = vm()
        assertFalse("the home screen is not the chat", phone.calls.any { it.startsWith("mark:") })
        vm.openChat()
        idle()
        assertTrue("mark:c1" in phone.calls)
    }

    // decision: PHN-113, PHN-121, PHN-129, PHN-11
    // decision: PHN-516
    // decision: PHN-521
    @Test
    fun a_key_place_asks_for_the_fingerprint_for_its_path_and_its_file_goes_when_the_owner_is_back() {
        session.unlocked()
        session.foreground(true)
        phone.folders["~/Documents"] = Folder("~/Documents", "~", false, listOf(FileEntry("Keys", "~/Documents/Keys", true, false, 0u, 0, true)), 1u, false)
        phone.folders["~/Documents/Keys"] = Folder("~/Documents/Keys", "~/Documents", true, listOf(FileEntry("id_ed25519", "~/Documents/Keys/id_ed25519", false, false, 10u, 0, true)), 1u, false)
        val vm = vm()
        vm.openFolder("~/Documents")
        assertTrue("an ordinary folder asks nothing", signers.isEmpty())
        vm.openEntry(vm.ui.value.files.folder!!.entries.single())
        assertEquals(listOf(KeyKind.STRONG), signers.map { it.kind })
        assertEquals(listOf(Why.FILE), whys)
        assertEquals("~/Documents/Keys", vm.ui.value.files.folder?.path)
        vm.openEntry(vm.ui.value.files.folder!!.entries.single())
        until("the fetch") { vm.transfers.list.value.singleOrNull()?.phase == Phase.DONE }
        val t = vm.transfers.list.value.single()
        assertTrue(t.keyPlace)
        assertTrue("fetch:~/Documents/Keys/id_ed25519@0:true" in phone.calls)
        assertArrayEquals(phone.fetchBytes, File(t.local).readBytes())
        // opened in another app: gone when the owner is back in FERB
        vm.handedOut(t.key)
        session.foreground(false)
        session.foreground(true)
        until("the key place's file goes") { vm.transfers.list.value.isEmpty() }
        assertFalse(File(t.local).exists())
        // without the fingerprint key (Android destroyed it): pair again, nothing asked
        phone.state.value = PhoneState.Open(KeyHealth(KeyState.OK, KeyState.OK, KeyState.GONE))
        signers.clear()
        vm.openFolder("~/Documents/Keys", keyPlace = true)
        assertEquals(R.string.needs_pairing_again, vm.ui.value.files.problem?.res)
        assertTrue(signers.isEmpty())
    }

    // decision: PHN-13, PHN-115
    @Test
    fun a_fetch_pauses_and_goes_on_from_the_bytes_saved() {
        session.unlocked()
        val vm = vm()
        phone.fetchGate = CompletableDeferred()
        val key = vm.transfers.fetch("~/Downloads/a.bin", "a.bin", 10, false)
        until("half") { vm.transfers.list.value.single().done == 5L }
        assertEquals(Phase.RUNNING, vm.transfers.list.value.single().phase)
        vm.pause(key)
        until("paused") { vm.transfers.list.value.single().phase == Phase.PAUSED }
        val paused = vm.transfers.list.value.single()
        assertEquals(5L, File(paused.local).length())
        phone.fetchGate = null
        vm.transfers.resume(key)
        until("done") { vm.transfers.list.value.single().phase == Phase.DONE }
        assertTrue("fetch:~/Downloads/a.bin@5:false" in phone.calls)
    }

    // decision: PHN-410, PHN-444
    @Test
    fun on_the_public_relay_a_fetch_waits_and_is_tried_once_more_when_files_cross_again() {
        session.unlocked()
        phone.link.value = FakePhone.status(path = LinkPath.PUBLIC_RELAY)
        val vm = vm()
        phone.fetchRefusal = Outcome.Failed(FerbException.NeedsBetterPath("This file is available when connected directly or through your relay.", null))
        vm.transfers.fetch("~/a.pdf", "a.pdf", 10, false)
        until("waiting") { vm.transfers.list.value.single().phase == Phase.WAITING_PATH }
        val t = vm.transfers.list.value.single()
        assertEquals(R.string.err_needs_better_path, t.problem?.res)
        phone.fetchRefusal = null
        phone.link.value = FakePhone.status(path = LinkPath.OWN_RELAY)
        until("tried once more") { vm.transfers.list.value.single().phase == Phase.DONE }
        assertEquals(2, phone.calls.count { it.startsWith("fetch:~/a.pdf") })
    }

    // decision: PHN-127, PHN-115
    // decision: PHN-517
    // decision: PHN-513
    @Test
    fun picked_files_are_copied_here_sent_and_ride_with_the_next_message() {
        session.unlocked()
        phone.view = empty()
        picked["content://picker/IMG_1.jpg"] = ByteArray(30) { 1 }
        picked["content://picker/IMG_2.jpg"] = ByteArray(20) { 2 }
        val vm = vm()
        vm.sendFiles(listOf("content://picker/IMG_1.jpg", "content://picker/IMG_2.jpg", "content://picker/gone"), forChat = true)
        until("both sent") { vm.transfers.ready.value.size == 2 }
        assertEquals("the unreadable one is said", R.string.files_unreadable, vm.ui.value.note?.res)
        assertEquals(listOf("upload:IMG_1.jpg:-", "upload:IMG_2.jpg:-"), phone.calls.filter { it.startsWith("upload") })
        assertEquals(listOf("up-IMG_1.jpg", "up-IMG_2.jpg"), vm.transfers.ready.value.map { it.uploadId })
        until("the staged copies are gone once sent") { local.outgoing.listFiles().orEmpty().all { it.listFiles().isNullOrEmpty() } }
        vm.typed("Here are the scans")
        vm.send()
        assertEquals("Here are the scans", phone.sentWith.single().first)
        assertEquals(2, phone.sentWith.single().second.size)
        assertTrue(vm.transfers.ready.value.isEmpty())
    }

    // decision: PHN-454, PHN-420
    // decision: PHN-518
    @Test
    fun watch_live_never_asks_on_the_public_relay_and_stops_in_the_background() {
        session.unlocked()
        session.foreground(true)
        phone.link.value = FakePhone.status(path = LinkPath.PUBLIC_RELAY)
        val vm = vm()
        vm.watchLive("t-1")
        assertEquals(R.string.not_on_shared_relay, vm.ui.value.note?.res)
        assertFalse(phone.calls.any { it.startsWith("live:") })
        phone.link.value = FakePhone.status()
        vm.watchLive("t-1")
        idle()
        assertEquals(Sheet.Live("t-1"), vm.sheet.value)
        assertTrue("kept out of screenshots whatever the switch says", vm.mustSecure.value)
        phone.preview!!.frame(PreviewFrame(null, 3, "working", "Reading delivery options", false, true, false, null))
        assertEquals("Reading delivery options", vm.live.view.value?.line)
        phone.preview!!.ended("needs_unlock", "Unlock to watch live.")
        assertTrue(vm.live.view.value!!.needsUnlock)
        session.foreground(false)
        idle()
        assertEquals(1, phone.previewStopped)
        assertNull(vm.sheet.value)
        assertFalse(vm.mustSecure.value)
    }

    // decision: PHN-404, PHN-133, PHN-112
    // decision: PHN-521
    @Test
    fun settings_change_one_at_a_time_each_signed_after_the_fingerprint() {
        session.unlocked()
        phone.settingsValues["sense.window_titles"] = "true"
        phone.settingsValues["retention.trace_days"] = "30"
        phone.settingsValues["privacy.exclude_apps"] = """["Password manager"]"""
        phone.settingsValues["browsing.owner_profile"] = "\"Default\""
        val vm = vm()
        // the Mac's window had closed: one unlock, then the read
        phone.settingsRefusal = FakePhone.refused("locked")
        vm.openSettings()
        assertEquals(Sheet.Settings, vm.sheet.value)
        assertEquals(listOf("sense.window_titles", "privacy.exclude_apps", "retention.trace_days"), vm.ui.value.settings!!.map { it.spec.key })
        assertEquals(listOf(KeyKind.LOCK), signers.map { it.kind })
        signers.clear(); whys.clear()
        vm.setSwitch("sense.window_titles", false)
        assertEquals(listOf(KeyKind.STRONG), signers.map { it.kind })
        assertEquals(listOf(Why.SETTING), whys)
        assertFalse(vm.ui.value.settings!!.first().on)
        vm.show(Sheet.Days("retention.trace_days"))
        vm.setDays("retention.trace_days", "0")
        assertEquals(R.string.set_days_invalid, vm.ui.value.note?.res)
        vm.setDays("retention.trace_days", "14")
        assertEquals("set:retention.trace_days=14", phone.calls.last())
        assertEquals(14, vm.ui.value.settings!!.single { it.spec.key == "retention.trace_days" }.days)
        assertEquals(Sheet.Settings, vm.sheet.value)
        // a cancelled prompt changes nothing and says nothing
        cancelPrompt = true
        vm.setSwitch("sense.window_titles", true)
        assertFalse(vm.ui.value.settings!!.first().on)
    }

    // decision: PHN-13
    @Test
    fun tasks_come_from_the_macs_list_and_from_the_chat_offline() {
        session.unlocked()
        phone.view = empty()
        phone.status = status(asked("ap-1", needs(unlock = false)))
        phone.tasks = Outcome.Ok(TaskPage(listOf(
            TaskRow("t2", "Compare desks", "Compare the three desks", "working", null, null, "", 2u, 3u),
            TaskRow("t1", "Back up the thesis", "", "done", null, null, "Copied to the drive.", null, null),
            TaskRow("t0", "Invoice", "", "failed", null, null, "The site asked for a code.", null, null),
        ), false))
        val vm = vm()
        vm.choose(PhoneTab.TASKS)
        val w = vm.ui.value.work
        assertEquals(listOf("ap-1"), w.waiting.map { it.approvalId })
        assertEquals(listOf("Compare desks"), w.running.map { it.title })
        assertEquals(2 to 3, w.running.single().progress)
        assertEquals(listOf(MarkKind.DONE, MarkKind.FAILED), w.earlier.map { it.kind })
        // the Mac's list can't be read: the chat's stays
        val vm2 = vm()
        phone.tasks = Outcome.Failed(FerbException.Offline("no", null))
        vm2.choose(PhoneTab.TASKS)
        assertNull(vm2.ui.value.macTasks)
        assertTrue(vm2.ui.value.work.running.isEmpty())
    }

    // decision: PHN-133
    // decision: PHN-522
    @Test
    fun a_fingerprint_added_after_pairing_is_offered_and_registered() {
        session.unlocked()
        phone.newKeys = PublicKeys(null, null, "STRONG-PUB")
        phone.mac = MacInfo("Studio Mac", null, null)
        val vm = vm()
        vm.unlockMac()
        idle()
        assertTrue(vm.ui.value.newKeys)
        signers.clear(); whys.clear()
        vm.registerKeys()
        assertTrue("keys:STRONG-PUB" in phone.calls)
        assertEquals(listOf(Why.KEYS), whys)
        assertFalse(vm.ui.value.newKeys)
        assertEquals(R.string.keys_registered, vm.ui.value.note?.res)
    }

    // decision: PHN-408
    // decision: PHN-515
    @Test
    fun forgetting_the_mac_removes_every_file_here() {
        session.unlocked()
        File(local.outgoing, "x/a.jpg").apply { parentFile!!.mkdirs(); writeText("x") }
        val vm = vm()
        vm.transfers.fetch("~/a.txt", "a.txt", 10, false)
        until("fetched") { vm.transfers.list.value.single().phase == Phase.DONE }
        vm.forget()
        until("everything here goes") { !root.exists() }
        assertTrue(vm.transfers.list.value.isEmpty())
        assertEquals(Way.FETCH, Way.valueOf("FETCH"))
    }
}
