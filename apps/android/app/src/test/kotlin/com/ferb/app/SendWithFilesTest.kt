package com.ferb.app

import android.app.Application
import android.os.Looper
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.Picked
import com.ferb.app.model.SendPhase
import com.ferb.app.model.Why
import com.ferb.core.KeyKind
import com.ferb.mobile.LinkPath
import com.ferb.mobile.LinkPhase
import java.io.File
import java.nio.file.Files
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/**
 * Files picked for the chat wait in the message box and go only with the message (kb PHN-690, PHN-691): the owner
 * picked a photo and the app at once showed "Sending IMG-….jpg to your Mac" above the box. On the fake phone: what is
 * uploaded, what is sent, and how a message with files stands when the Mac is away, the path is the shared relay, he
 * stops it, or FERB closes while its files go.
 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class SendWithFilesTest {
    private val phone = FakePhone()
    private lateinit var session: Session
    private val root: File = Files.createTempDirectory("ferb-send-files").toFile()
    private val picked = HashMap<String, ByteArray>()
    private val local = LocalFiles(root) { uri -> picked[uri]?.let { b -> Picked(uri.substringAfterLast('/'), b.size.toLong()) { b.inputStream() } } }

    /** True when no file is left under [dir]. The app removes emptied folders while this looks, so a folder that
     *  vanishes mid-walk (Kotlin's walk then asserts) reads as "look again", never as a failure. */
    private fun noFilesUnder(dir: File): Boolean =
        !dir.exists() || runCatching { dir.walk().none { it.isFile } }.getOrDefault(false)

    /** Swaps Dispatchers.Main once no other thread is using it. The app's file work may still post to Main from its own
     *  thread as a test ends; kotlinx.coroutines then refuses the swap ("used concurrently with setting it"), which is
     *  the test's timing, not the app's behaviour: try again for up to a second. */
    private fun settled(swap: () -> Unit) {
        val end = System.currentTimeMillis() + 1_000
        while (true) {
            val tried = runCatching(swap)
            if (tried.isSuccess) return
            if (System.currentTimeMillis() > end) tried.getOrThrow()
            Thread.sleep(10)
        }
    }

    @Before
    fun setUp() {
        settled { Dispatchers.setMain(UnconfinedTestDispatcher()) }
        session = Session { 0L }
        session.unlocked()
        phone.view = com.ferb.mobile.TranscriptView("c1", emptyList(), emptyList(), emptyList(), emptyList(), com.ferb.mobile.OlderState.Complete)
        picked["content://picker/IMG_1.jpg"] = ByteArray(30) { 1 }
        picked["content://picker/IMG_2.jpg"] = ByteArray(20) { 2 }
    }

    @After
    fun tearDown() {
        phone.uploadGate?.complete(Unit)
        settled { Dispatchers.resetMain() }
        // the app may still be removing a copy on its own thread: a folder that went meanwhile is no matter
        runCatching { root.deleteRecursively() }
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
                override fun signer(kind: KeyKind, why: Why) = FakeSigner(kind, false)
                override suspend fun local() = true
            }
        }

    private fun pickTwo(vm: FerbViewModel) {
        vm.sendFiles(listOf("content://picker/IMG_1.jpg", "content://picker/IMG_2.jpg"), forChat = true)
        until("both in the box") { vm.outgoing.chips.value.size == 2 }
    }

    private val uploads get() = phone.calls.filter { it.startsWith("upload") }

    // decision: PHN-690
    @Test
    fun picked_files_wait_in_the_box_and_nothing_leaves_the_phone() {
        val vm = vm()
        pickTwo(vm)
        assertEquals(listOf("IMG_1.jpg", "IMG_2.jpg"), vm.outgoing.chips.value.map { it.name })
        assertEquals(listOf(30L, 20L), vm.outgoing.chips.value.map { it.size })
        assertTrue("copied here", vm.outgoing.chips.value.all { File(it.path).isFile })
        assertTrue("nothing uploaded", uploads.isEmpty())
        assertTrue("nothing sent", phone.sentWith.isEmpty())
        assertTrue("no transfer card", vm.transfers.list.value.isEmpty())
    }

    // decision: PHN-690
    @Test
    fun the_cross_takes_a_file_out_of_the_box_and_its_copy_with_it() {
        val vm = vm()
        pickTwo(vm)
        val gone = vm.outgoing.chips.value.first()
        vm.unpick(gone.key)
        assertEquals(listOf("IMG_2.jpg"), vm.outgoing.chips.value.map { it.name })
        until("its copy is gone") { !File(gone.path).exists() }
        assertTrue(uploads.isEmpty() && phone.sentWith.isEmpty())
    }

    // decision: PHN-690, PHN-691
    @Test
    fun send_uploads_the_files_then_sends_the_message_with_their_ids() {
        val vm = vm()
        pickTwo(vm)
        vm.typed("Here are the scans")
        vm.send()
        assertTrue("the box is empty at once", vm.outgoing.chips.value.isEmpty())
        assertEquals("the draft went with it", "", vm.draft.value)
        until("sent") { phone.sentWith.isNotEmpty() }
        assertEquals(listOf("upload:IMG_1.jpg:-", "upload:IMG_2.jpg:-"), uploads)
        val (text, files) = phone.sentWith.single()
        assertEquals("Here are the scans", text)
        assertEquals(listOf("up-IMG_1.jpg", "up-IMG_2.jpg"), files.map { it.uploadId })
        assertEquals(listOf("IMG_1.jpg", "IMG_2.jpg"), files.map { it.name })
        until("the message left the phone's list") { vm.outgoing.messages.value.isEmpty() }
        assertTrue("the transfers are not on the Files tab", vm.transfers.list.value.isEmpty())
        until("the copies are gone") { noFilesUnder(local.outgoing) }
    }

    // decision: PHN-691
    @Test
    fun with_the_mac_away_it_waits_and_goes_when_the_mac_is_back() {
        phone.link.value = FakePhone.status(LinkPhase.WAITING)
        val vm = vm()
        pickTwo(vm)
        vm.send()
        until("waiting") { vm.outgoing.messages.value.singleOrNull()?.phase == SendPhase.WAITING_LINK }
        assertTrue(uploads.isEmpty())
        phone.link.value = FakePhone.status()
        until("sent once the Mac is back") { phone.sentWith.size == 1 }
        assertEquals(2, uploads.size)
    }

    // decision: PHN-691, PHN-410
    @Test
    fun on_the_shared_relay_files_wait_and_the_words_can_go_alone() {
        phone.link.value = FakePhone.status(path = LinkPath.PUBLIC_RELAY)
        val vm = vm()
        pickTwo(vm)
        vm.typed("For later")
        vm.send()
        until("waiting for a path") { vm.outgoing.messages.value.singleOrNull()?.phase == SendPhase.WAITING_PATH }
        assertTrue("no file on the shared relay", uploads.isEmpty())
        val id = vm.outgoing.messages.value.single().id
        vm.sendTextOnly(id)
        until("the words went") { phone.calls.contains("send:For later") }
        until("and the message with files is gone") { vm.outgoing.messages.value.isEmpty() }
        assertTrue(uploads.isEmpty() && phone.sentWith.isEmpty())
    }

    // decision: PHN-691, PHN-444
    @Test
    fun on_the_shared_relay_a_message_with_files_goes_by_itself_on_a_better_path() {
        phone.link.value = FakePhone.status(path = LinkPath.PUBLIC_RELAY)
        val vm = vm()
        pickTwo(vm)
        vm.send()
        until("waiting for a path") { vm.outgoing.messages.value.singleOrNull()?.phase == SendPhase.WAITING_PATH }
        phone.link.value = FakePhone.status(path = LinkPath.OWN_RELAY)
        until("sent") { phone.sentWith.size == 1 }
    }

    // decision: PHN-691
    @Test
    fun cancel_while_the_files_go_keeps_the_message_not_sent_and_try_again_goes_on() {
        phone.uploadGate = CompletableDeferred()
        val vm = vm()
        pickTwo(vm)
        vm.send()
        until("half way") { (vm.outgoing.messages.value.singleOrNull()?.done ?: 0) > 0 }
        val id = vm.outgoing.messages.value.single().id
        vm.stopSending(id)
        until("stopped") { vm.outgoing.messages.value.single().phase == SendPhase.STOPPED }
        idle()
        assertEquals(SendPhase.STOPPED, vm.outgoing.messages.value.single().phase)
        assertTrue("nothing sent", phone.sentWith.isEmpty())
        phone.uploadGate = null
        vm.retrySending(id)
        until("sent") { phone.sentWith.size == 1 }
        assertTrue("went on from its upload id", uploads.contains("upload:IMG_1.jpg:up-IMG_1.jpg"))
    }

    // decision: PHN-691
    @Test
    fun remove_takes_a_stopped_message_away_and_nothing_is_sent() {
        phone.uploadGate = CompletableDeferred()
        val vm = vm()
        pickTwo(vm)
        vm.send()
        until("half way") { (vm.outgoing.messages.value.singleOrNull()?.done ?: 0) > 0 }
        val id = vm.outgoing.messages.value.single().id
        vm.stopSending(id)
        vm.removeSending(id)
        until("gone") { vm.outgoing.messages.value.isEmpty() }
        until("its copies too") { noFilesUnder(local.outgoing) }
        assertTrue(phone.sentWith.isEmpty())
    }

    // decision: PHN-691
    @Test
    fun ferb_closed_while_the_files_went_says_so_and_try_again_goes_on_where_it_stopped() {
        phone.uploadGate = CompletableDeferred()
        val first = vm()
        pickTwo(first)
        first.typed("Scans")
        first.send()
        until("half way, its upload id kept") {
            File(root, "messages.json").takeIf { it.isFile }?.readText()?.contains("up-IMG_1.jpg") == true
        }
        // FERB closes here: a new process reads what was kept
        val gate = phone.uploadGate!!
        phone.uploadGate = null
        val vm = vm()
        until("the message is back") { vm.outgoing.messages.value.isNotEmpty() }
        val m = vm.outgoing.messages.value.single()
        assertEquals(SendPhase.CLOSED, m.phase)
        assertEquals("Scans", m.text)
        assertFalse("not sent", phone.sentWith.isNotEmpty())
        vm.retrySending(m.id)
        until("sent") { phone.sentWith.isNotEmpty() }
        assertTrue("went on from its upload id", uploads.contains("upload:IMG_1.jpg:up-IMG_1.jpg"))
        assertEquals("Scans", phone.sentWith.last().first)
        gate.complete(Unit)
    }

    // decision: PHN-692
    @Test
    fun the_files_tabs_send_is_the_send_and_goes_at_once() {
        val vm = vm()
        vm.sendFiles(listOf("content://picker/IMG_1.jpg"), forChat = false)
        until("on the Mac") { vm.transfers.list.value.singleOrNull()?.phase == com.ferb.app.model.Phase.DONE }
        assertEquals(listOf("upload:IMG_1.jpg:-"), uploads)
        assertTrue("never in the message box", vm.outgoing.chips.value.isEmpty())
    }

    // The owner (2026-11-24, ORCH-715): the temporary chat is the chat except that nothing is saved. Its + picks files
    // into the same box; Send uploads them as the chat does, then the message goes with their ids, `fresh` (the Mac moves
    // them into the temporary chat's own folder and deletes them when it ends), never the outbox; the phone's copies go,
    // and the message is never written on the phone. Before, a file picked there waited as a chip and was never sent.
    // decision: ORCH-715, PHN-838
    @Test
    fun in_the_temporary_chat_plus_sends_files_as_the_chat_does() {
        phone.temporaryOn = true
        val vm = vm()
        until("the temporary chat shows") { vm.ui.value.temporary != null }
        pickTwo(vm)
        vm.typed("What are these")
        vm.send()
        assertTrue("the box is empty at once", vm.outgoing.chips.value.isEmpty())
        until("sent") { phone.temporarySent.isNotEmpty() }
        assertEquals(listOf("upload:IMG_1.jpg:-", "upload:IMG_2.jpg:-"), uploads)
        assertEquals(listOf("What are these"), phone.temporarySent)
        assertEquals(listOf(listOf("up-IMG_1.jpg", "up-IMG_2.jpg")), phone.temporaryFiles)
        assertTrue("never the chat's send", phone.sentWith.isEmpty() && phone.outbox.isEmpty())
        until("the message left the phone's list") { vm.outgoing.messages.value.isEmpty() }
        until("the copies are gone") { noFilesUnder(local.outgoing) }
        val kept = File(local.root, "messages.json")
        assertFalse("never written on the phone", kept.isFile && kept.readText().contains("What are these"))
    }

    // Its message on its way shows only in the temporary chat; the chat ending (on any device) takes it away with its
    // copies, so nothing of it waits to go later.
    // decision: PHN-838
    @Test
    fun a_temporary_message_with_files_shows_only_there_and_goes_when_the_chat_ends() {
        phone.temporaryOn = true
        phone.link.value = FakePhone.status(LinkPhase.WAITING)
        val vm = vm()
        until("the temporary chat shows") { vm.ui.value.temporary != null }
        pickTwo(vm)
        vm.send()
        until("waiting") { vm.outgoing.messages.value.singleOrNull()?.phase == SendPhase.WAITING_LINK }
        assertTrue(vm.outgoing.messages.value.single().temporary)
        val copies = vm.outgoing.messages.value.single().chips.map { File(it.path) }
        phone.temporaryOn = false
        phone.changes.tryEmit(com.ferb.mobile.Change.Temporary)
        until("gone with the chat") { vm.outgoing.messages.value.isEmpty() }
        until("its copies too") { copies.none { it.exists() } }
        assertTrue(phone.temporarySent.isEmpty() && phone.sentWith.isEmpty())
    }
}
