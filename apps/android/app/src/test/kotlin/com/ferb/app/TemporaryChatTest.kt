package com.ferb.app

import android.app.Application
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.ChatRow
import com.ferb.app.model.LocalFiles
import com.ferb.app.ui.Host
import com.ferb.app.ui.Main
import com.ferb.core.KeyHealth
import com.ferb.core.KeyState
import com.ferb.core.Outcome
import com.ferb.core.PhoneState
import com.ferb.design.Phone
import com.ferb.design.components.PhoneTab
import com.ferb.design.preview.labMoment
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.ferb.mobile.Change
import com.ferb.mobile.FerbException
import com.ferb.mobile.Item
import com.ferb.mobile.NoticeItem
import com.ferb.mobile.OlderState
import com.ferb.mobile.PhoneStatus
import com.ferb.mobile.TranscriptView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestCoroutineScheduler
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.io.File
import java.time.LocalDate
import java.time.ZoneId

/**
 * The temporary chat (the owner, 2026-11-23): the dashed bubble in the chat tab's top bar turns it on and the same
 * bubble turns it off; while on, the banner and only its own turns show; empty, one quiet line; the box sends with
 * `fresh: true` (never the outbox); a relaunch or unlock with it on shows it again; off (from any client) is the normal
 * chat; nothing of it is stored on the phone.
 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35], qualifiers = "w412dp-h915dp-port-mdpi")
class TemporaryChatTest {
    @get:Rule val compose = createComposeRule()

    private val phone = FakePhone()
    private val scheduler = TestCoroutineScheduler()
    private val zone = ZoneId.systemDefault()
    private val today = LocalDate.now(zone)
    private lateinit var session: Session
    private val root: File = java.nio.file.Files.createTempDirectory("ferb-temporary").toFile()
    private val host = object : Host {
        override fun stayConnected(on: Boolean) {}
        override fun blockScreenshots(on: Boolean) {}
        override fun askNotifications() {}
        override fun pickFiles(forChat: Boolean) {}
        override fun handOut(key: String, file: File, name: String, share: Boolean) {}
    }

    @Before
    fun setUp() {
        Dispatchers.setMain(UnconfinedTestDispatcher(scheduler))
        session = Session { 0L }
        phone.state.value = PhoneState.Open(KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK))
        phone.view = TranscriptView("c1", listOf(said("n1", "The normal chat's line.")), emptyList(), emptyList(), emptyList(), OlderState.Complete)
    }

    @After
    fun tearDown() {
        Dispatchers.resetMain()
        root.deleteRecursively()
    }

    private fun vm() = FerbViewModel(phone, session, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
        LocalFiles(root) { null }, say = { "" }, account = testAccount())

    private fun said(id: String, text: String): Item {
        val at = today.atTime(9, 0).atZone(zone).toEpochSecond() - 978_307_200.0
        return Item.Notice(NoticeItem(id, at, false, id, null, text, emptyList(), null))
    }

    private fun show(vm: FerbViewModel) {
        compose.setContent {
            val ui by vm.ui.collectAsState()
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight)) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false) {
                    Main(vm, ui.copy(gate = Gate.MAIN), PhoneTab.CHAT, home = false, PhoneInsets.Lab, labMoment(12.5), host)
                }
            }
        }
        compose.waitForIdle()
    }

    // decision: PHN-837
    @Test
    fun the_bubble_turns_it_on_with_the_banner_and_only_its_own_turns() {
        phone.temporaryItems = listOf(said("t1", "An answer from the temporary chat."))
        val vm = vm()
        session.unlocked()
        show(vm)
        compose.onNodeWithText("The normal chat's line.").assertIsDisplayed()
        compose.onNodeWithContentDescription("Temporary chat").performClick()
        compose.waitForIdle()
        assertTrue(phone.calls.contains("temporary:true"))
        compose.onNodeWithTag("temporary-banner").assertIsDisplayed()
        compose.onNodeWithText("Temporary chat · FERB won't use what it knows or keep this chat").assertIsDisplayed()
        compose.onNodeWithText("An answer from the temporary chat.").assertIsDisplayed()
        compose.onNodeWithText("The normal chat's line.").assertDoesNotExist()
        compose.onNodeWithContentDescription("End temporary chat").assertIsDisplayed()
    }

    // decision: PHN-837
    @Test
    fun the_same_bubble_turns_it_off_and_the_normal_chat_returns() {
        phone.temporaryOn = true
        phone.temporaryItems = listOf(said("t1", "An answer from the temporary chat."))
        val vm = vm()
        session.unlocked()
        show(vm)
        compose.onNodeWithText("An answer from the temporary chat.").assertIsDisplayed()
        compose.onNodeWithContentDescription("End temporary chat").performClick()
        compose.waitForIdle()
        assertTrue(phone.calls.contains("temporary:false"))
        assertNull(vm.ui.value.temporary)
        compose.onNodeWithText("The normal chat's line.").assertIsDisplayed()
        compose.onNodeWithTag("temporary-banner").assertDoesNotExist()
        compose.onNodeWithContentDescription("Temporary chat").assertIsDisplayed()
    }

    // decision: PHN-837
    @Test
    fun a_relaunch_with_it_on_shows_it_and_an_empty_one_says_it_starts_fresh() {
        phone.temporaryOn = true
        val vm = vm()
        session.unlocked()
        assertNotNull("read from the Mac at start", vm.ui.value.temporary)
        show(vm)
        compose.onNodeWithText("This chat starts fresh. Tap the dashed bubble again to end it.").assertIsDisplayed()
        compose.onNodeWithText("The normal chat's line.").assertDoesNotExist()
        // locking the phone never ends it: unlocked again, it is read and shown again
        session.lockNow()
        phone.temporaryItems = listOf(said("t1", "Still here after the lock."))
        session.unlocked()
        compose.waitForIdle()
        compose.onNodeWithText("Still here after the lock.").assertIsDisplayed()
    }

    // decision: PHN-837
    @Test
    fun a_message_goes_fresh_never_to_the_outbox_and_nothing_is_stored() {
        phone.temporaryOn = true
        val vm = vm()
        session.unlocked()
        vm.typed("what is 2+2")
        phone.temporaryItems = listOf(said("t1", "4"))
        vm.send()
        assertEquals(listOf("what is 2+2"), phone.temporarySent)
        assertTrue("nothing in the outbox", phone.outbox.isEmpty())
        assertFalse(phone.calls.any { it.startsWith("send:") })
        assertEquals("", vm.draft.value)
        assertEquals(1, vm.ui.value.temporary?.rows?.size)
        // a dropped link: the words stay in the box, nothing waits to go later
        phone.temporaryRefusal = Outcome.Failed(FerbException.Offline("FERB's Mac can't be reached now.", "mac_unreachable"))
        vm.typed("again")
        vm.send()
        assertEquals("again", vm.draft.value)
        assertTrue(phone.outbox.isEmpty())
    }

    // decision: PHN-837
    @Test
    fun a_failed_toggle_says_so_and_an_end_on_another_client_returns_to_the_normal_chat() {
        val vm = vm()
        session.unlocked()
        phone.temporaryRefusal = Outcome.Failed(FerbException.Offline("no", null))
        vm.toggleTemporary()
        assertEquals(R.string.temporary_start_failed, vm.ui.value.note?.res)
        assertNull(vm.ui.value.temporary)
        phone.temporaryRefusal = null
        vm.toggleTemporary()
        assertNotNull(vm.ui.value.temporary)
        // ended on the Mac: the status moves, the library says Temporary, the read says off
        phone.temporaryOn = false
        phone.changes.tryEmit(Change.Temporary)
        assertNull(vm.ui.value.temporary)
        phone.temporaryRefusal = Outcome.Failed(FerbException.Offline("no", null))
        phone.temporaryOn = true
        vm.toggleTemporary()
        assertEquals(R.string.temporary_start_failed, vm.ui.value.note?.res)
    }

    // The owner's live case (2026-11-23): in the temporary chat FERB's questions sat above his newest message and stayed
    // after he answered them. Its questions are at its end, after everything said, as FERB.app draws them; once the Mac's
    // temporary chat says one is answered (its rev moved: `Change.Temporary`), it is gone.
    // decision: PHN-824, PHN-837
    @Test
    fun its_questions_are_at_its_end_and_go_once_answered() {
        val c = Fixtures.conversations().first { it.scenario == "question_left_open" }
        @Suppress("UNCHECKED_CAST")
        fun loose(answered: Set<String>) = c.copy(entries = c.entries.map { e ->
            var x = if (e["approval_id"] == "ap-1") e + ("body" to ((e["body"] as Map<String, Any?>) + ("part" to null))) else e
            if (x["approval_id"] in answered) x = x + mapOf("state" to "answered", "open" to false, "rev" to 9.0, "seq" to 20.0)
            x
        })
        phone.temporaryOn = true
        phone.temporaryView = FixtureMac.transcript(FixtureMac(loose(emptySet())))
        val vm = vm()
        session.unlocked()
        val rows = vm.ui.value.temporary!!.rows
        val tidy = rows.indexOfFirst { it is ChatRow.Owner && it.text == "tidy my desktop" }
        val asks = rows.withIndex().filter { it.value is ChatRow.Asking }
        assertEquals(listOf("ap-1", "ap-2"), asks.map { (it.value as ChatRow.Asking).approvalId })
        assertTrue("below his newer message", asks.all { it.index > tidy })

        phone.temporaryView = FixtureMac.transcript(FixtureMac(loose(setOf("ap-1"))))
        phone.changes.tryEmit(Change.Temporary)
        val after = vm.ui.value.temporary!!
        assertEquals(listOf("ap-2"), after.rows.filterIsInstance<ChatRow.Asking>().map { it.approvalId })
        assertEquals(listOf("ap-2"), after.questions.map { it.approvalId })
    }

    // decision: PHN-837
    @Test
    fun a_temporary_chat_s_question_shows_only_in_its_own_view() {
        val q = { id: String, temp: Boolean ->
            com.ferb.mobile.StatusQuestion(id, "approve", null, null, "Open it?", listOf("Yes", "No"), false,
                com.ferb.mobile.QuestionNeeds(true, false, false, false, null, null, false), null, temp)
        }
        phone.status = PhoneStatus(false, true, false, listOf(q("q-normal", false), q("q-temp", true)), emptyList(), emptyList(), null, null, emptyList(), null, null)
        phone.temporaryOn = true
        val vm = vm()
        session.unlocked()
        assertEquals(listOf("q-normal"), vm.ui.value.questions.map { it.approvalId })
        assertEquals(listOf("q-temp"), vm.ui.value.temporary?.questions?.map { it.approvalId })
    }

    /**
     * A Mac holding a temporary chat (T1) in memory, with the chat (c1) empty: the one watch follows whichever
     * conversation the phone asks for, and the test hands frames to it as the daemon's `chat.entries.watch` would.
     */
    private class TemporaryMac : com.ferb.mobile.Transport {
        val calls = java.util.concurrent.CopyOnWriteArrayList<String>()
        @Volatile var watched: String = ""
        @Volatile var sink: com.ferb.mobile.WatchSink? = null
        private val chat = """{"on":true,"id":"T1","conversation":"temporary:T1","started_at":"2026-01-02T10:05:00+00:00","rev":1,"entries":[]}"""
        override fun call(method: String, paramsJson: String, proofJson: String?): String {
            calls.add(method)
            return when (method) {
                "chat.entries.page" -> """{"conversation":"c1","epoch":"E1","head":0,"entries":[],"more":false,"before_pos":null}"""
                "chat.entries.changes" -> """{"conversation":"c1","epoch":"E1","head":0,"entries":[],"more":false,"next_seq":0}"""
                "chat.entries.mark" -> """{"marks":{}}"""
                "chat.temporary.get", "chat.temporary.set" -> chat
                else -> throw com.ferb.mobile.TransportException.Failed("method_not_allowed", "$method is not allowed", null, null)
            }
        }
        override fun send(paramsJson: String): String = throw com.ferb.mobile.TransportException.Failed("method_not_allowed", "no sends here", null, null)
        override fun watch(paramsJson: String, sink: com.ferb.mobile.WatchSink): ULong {
            @Suppress("UNCHECKED_CAST")
            val p = com.ferb.app.model.Json.parse(paramsJson) as Map<String, Any?>
            val c = p["conversation"] as String
            this.sink = sink
            watched = c
            val epoch = if (c == "temporary:T1") "T1" else "E1"
            sink.frame("""{"type":"caught_up","conversation":"$c","epoch":"$epoch","head":0}""")
            return 1u
        }
        override fun closeWatch(watchId: ULong) {}
        override fun get(id: String, paramsJson: String, saveTo: String?, append: Boolean, progress: com.ferb.mobile.TransferProgress): String =
            throw com.ferb.mobile.TransportException.Failed("not_found", "none", null, null)
        override fun put(id: String, path: String, optsJson: String, progress: com.ferb.mobile.TransferProgress): String =
            throw com.ferb.mobile.TransportException.Failed("not_found", "none", null, null)
        override fun cancel(id: String) {}
    }

    // The owner (2026-11-24, ORCH-715): "isnt everything same but only its a non saving chat???". Through the real library
    // on the host: once the temporary chat is on, the one watch follows it, and each frame the Mac streams (his working
    // turn, the plan line as it is said) reaches the temporary chat on screen as it reaches the chat, read from the
    // library's memory on `Change.Temporary`, with no read of the whole chat from the Mac per change. Before, the phone
    // only read it again (`chat.temporary.get`) on each status rev, about 1.5 s late, and never saw a streamed frame.
    // decision: ORCH-715, PHN-837
    @Test
    fun its_streamed_frames_reach_it_through_the_one_watch_as_the_chat_s() {
        val mac = TemporaryMac()
        val dir = java.nio.file.Files.createTempDirectory("ferb-temporary-watch").toFile()
        val n = java.util.concurrent.atomic.AtomicInteger()
        val m = com.ferb.mobile.Mobile.open(File(dir, "phone.db").path, ByteArray(32) { 7 }, com.ferb.mobile.MobileConfig("client-test", 0, null, null, null),
            object : com.ferb.mobile.Clock { override fun nowIso() = "2026-01-02T10:00:00+00:00" },
            object : com.ferb.mobile.Ids { override fun newId() = "id-${n.incrementAndGet()}" },
            object : com.ferb.mobile.Listener { override fun changed(change: Change) { phone.changes.tryEmit(change) } })
        try {
            m.connectTransport(mac)
            m.linkEvent("""{"type":"state","phase":"online","tier":"direct"}""")
            m.sync()
            assertEquals("c1", mac.watched)
            phone.realTemporary = m
            phone.temporaryOn = true
            val vm = vm()
            session.unlocked()
            m.setTemporaryChat(true)
            val end = System.currentTimeMillis() + 5_000
            while (mac.watched != "temporary:T1" && System.currentTimeMillis() < end) Thread.sleep(10)
            assertEquals("the one watch follows the temporary chat", "temporary:T1", mac.watched)
            val reads = mac.calls.count { it == "chat.temporary.get" }
            val sink = mac.sink!!
            sink.frame("""{"type":"change","entry":{"id":"T1-e1","conversation_id":"temporary:T1","pos":1,"seq":1,"rev":1,"kind":"message",
                "state":"working","open":true,"request_id":"temporary-r1","created_at":"2026-01-02T10:06:00+00:00",
                "updated_at":"2026-01-02T10:06:00+00:00","day":"2026-01-02","body":{"text":"look this up","attachments":[],"hidden":[]},
                "deleted":false}}""")
            sink.frame("""{"type":"live","request_id":"temporary-r1","frame":{"type":"progress","planning":true,"text":"Reading the page"}}""")
            val rows = vm.ui.value.temporary!!.rows
            assertTrue("his turn: $rows", rows.any { it is ChatRow.Owner && it.text == "look this up" })
            assertTrue("the plan line streamed in: $rows", rows.any { it is ChatRow.Line && it.text == "Reading the page" })
            assertEquals("never read again per change", reads, mac.calls.count { it == "chat.temporary.get" })
            assertTrue(phone.calls.contains("last-temporary"))
        } finally {
            phone.realTemporary = null
            m.close()
            dir.deleteRecursively()
        }
    }
}
