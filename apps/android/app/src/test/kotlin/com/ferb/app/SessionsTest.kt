package com.ferb.app

import android.app.Application
import android.content.Context
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.Why
import com.ferb.core.KeyKind
import com.ferb.mobile.LinkPath
import com.ferb.mobile.OlderState
import com.ferb.mobile.SessionCard
import com.ferb.mobile.SessionChoice
import com.ferb.mobile.TranscriptView
import com.ferb.design.components.PhoneTab
import java.nio.file.Files
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.channels.Channel
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
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * The owner's coding sessions on the phone (kb HAND-651..666), through the view model over a fake phone: the list and an open
 * session are read only while they show; a send the Mac asks about goes only after the owner's Yes, signed with the
 * fingerprint and with the very words he was shown; the agent's question is answered with the choice's id, signed;
 * stopping its turn takes control first; handing over, starting one and letting go; Watch never on the shared relay;
 * nothing of it stays past the lock.
 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class SessionsTest {
    private val context: Context = ApplicationProvider.getApplicationContext()
    private val phone = FakePhone()
    private lateinit var session: Session
    private val prefs = Prefs(context)
    private val signers = ArrayList<Pair<FakeSigner, Why>>()
    private var cancelPrompt = false
    private val root = Files.createTempDirectory("ferb-sessions").toFile()
    /** A rendezvous: a tick reaches only a read that waits for one (none is kept for later). */
    private val ticks = Channel<Unit>()

    @Before
    fun setUp() {
        Dispatchers.setMain(UnconfinedTestDispatcher())
        session = Session { 0L }
        session.unlocked()
        session.foreground(true)
        prefs.forgetPairing()
        phone.view = TranscriptView("c1", emptyList(), emptyList(), emptyList(), emptyList(), OlderState.Complete)
        phone.sessions += FakePhone.agentSession()
        phone.sessions += FakePhone.agentSession("codex:s2", title = "", status = "idle", host = "")
        phone.sessionItems["claude:s1"] = mutableListOf(
            FakePhone.item(1, "user", "Fix the flaky login test"),
            FakePhone.item(2, "assistant", "Looking at the test now."),
            FakePhone.item(3, "tool", "Bash: npm test", "Bash"),
        )
    }

    @After
    fun tearDown() {
        Dispatchers.resetMain()
        root.deleteRecursively()
    }

    private fun vm() = FerbViewModel(phone, session, prefs, { true }, null, SavedStateHandle(), LocalFiles(root) { null }).also {
        it.prompts = object : Prompts {
            override fun signer(kind: KeyKind, why: Why) = FakeSigner(kind, cancelPrompt).also { s -> signers += s to why }
            override suspend fun local() = true
        }
        it.sessionsWait = { ticks.receive() }
    }

    private fun tick() { ticks.trySend(Unit) }

    private fun count(prefix: String) = phone.calls.count { it.startsWith(prefix) }

    private fun opened(): FerbViewModel = vm().also { it.choose(PhoneTab.SESSIONS); it.openSession("claude:s1") }

    // ---- the list ---------------------------------------------------------------------------------------------------

    // decision: PHN-798
    @Test
    fun the_list_is_read_only_while_the_tab_shows() {
        val vm = vm()
        assertEquals("nothing is read before the tab shows", 0, count("sessions"))
        vm.choose(PhoneTab.SESSIONS)
        assertEquals(1, count("sessions"))
        val ui = vm.sessions.value
        assertTrue(ui.loaded && ui.enabled)
        assertEquals(listOf("claude:s1", "codex:s2"), ui.sessions.map { it.id })
        tick()
        assertEquals("read again while it shows", 2, count("sessions"))
        vm.choose(PhoneTab.CHAT)
        tick(); tick()
        assertEquals("the polling stops when the tab goes", 2, count("sessions"))
        vm.choose(PhoneTab.SESSIONS)
        assertEquals(3, count("sessions"))
        session.foreground(false)
        tick(); tick()
        assertEquals("nothing in the background", 3, count("sessions"))
    }

    // decision: PHN-133
    @Test
    fun off_shows_turn_on_which_is_a_signed_setting() {
        phone.sessionsEnabled = false
        val vm = vm()
        vm.choose(PhoneTab.SESSIONS)
        assertFalse(vm.sessions.value.enabled)
        cancelPrompt = true
        vm.turnOnSessions()
        assertNull("a cancelled prompt changes nothing", phone.settingsValues["sessions.enabled"])
        assertTrue(phone.signed.isEmpty())
        cancelPrompt = false
        phone.sessionsEnabled = true
        vm.turnOnSessions()
        assertTrue("set:sessions.enabled=true" in phone.calls)
        assertEquals(KeyKind.STRONG to Why.SETTING, signers.last().let { it.first.kind to it.second })
        assertTrue("read again once on", vm.sessions.value.enabled)
        assertEquals(2, vm.sessions.value.sessions.size)
    }

    // ---- one session ------------------------------------------------------------------------------------------------

    // decision: PHN-454
    @Test
    fun an_open_session_reads_its_newest_page_then_what_came_after_while_it_shows() {
        val vm = opened()
        assertEquals(Sheet.Session("claude:s1"), vm.sheet.value)
        assertTrue("its conversation is kept out of screenshots", vm.mustSecure.value)
        val o = vm.openSession.value!!
        assertEquals(listOf(1uL, 2uL, 3uL), o.items.map { it.seq })
        assertEquals(Triple("claude:s1", 0L, 0L), phone.timelineAsks.first())
        val listReads = count("sessions")
        phone.sessionItems["claude:s1"]!! += FakePhone.item(4, "assistant", "All tests pass.")
        tick()
        assertEquals("the next read asks after the newest held", Triple("claude:s1", 3L, 0L), phone.timelineAsks.last())
        assertEquals(listOf(1uL, 2uL, 3uL, 4uL), vm.openSession.value!!.items.map { it.seq })
        assertEquals("the list isn't read under an open session", listReads, count("sessions"))
        vm.closeSession()
        assertNull(vm.sheet.value)
        assertNull(vm.openSession.value)
        assertFalse(vm.mustSecure.value)
    }

    @Test
    fun show_earlier_pages_back_with_before() {
        phone.sessionItems["claude:s1"] = (1..60).map { FakePhone.item(it, if (it % 2 == 0) "assistant" else "user", "line $it") }.toMutableList()
        val vm = opened()
        assertEquals(50, vm.openSession.value!!.items.size)
        assertTrue(vm.openSession.value!!.more)
        vm.sessionOlder()
        assertEquals(Triple("claude:s1", 0L, 11L), phone.timelineAsks.last())
        assertEquals((1..60).map { it.toULong() }, vm.openSession.value!!.items.map { it.seq })
        assertFalse(vm.openSession.value!!.more)
    }

    // decision: GATE-622, PHN-133
    @Test
    fun a_send_the_mac_asks_about_goes_only_after_the_signed_yes_with_the_same_words() {
        val vm = opened()
        vm.sessionTyped("Run the tests again")
        vm.sessionSend()
        val ask = vm.openSession.value!!.ask
        assertNotNull("the Mac asked: nothing was sent", ask)
        assertEquals("Run the tests again", ask!!.text)
        assertTrue(phone.calls.none { it.startsWith("session-control") })
        // the owner cancels the fingerprint: still nothing sent, the question stays
        cancelPrompt = true
        vm.sessionYes()
        assertTrue("unsigned: nothing reached the Mac", phone.signed.isEmpty() && "claude:s1" !in phone.controlled)
        assertNotNull(vm.openSession.value!!.ask)
        cancelPrompt = false
        vm.sessionYes()
        assertTrue("session-control:claude:s1:Run the tests again" in phone.calls)
        assertEquals(KeyKind.STRONG to Why.YES, signers.last().let { it.first.kind to it.second })
        assertNull(vm.openSession.value!!.ask)
        assertEquals("what was sent leaves the box", "", vm.openSession.value!!.draft)
        // controlling it now: the next send goes without asking
        val controls = count("session-control")
        vm.sessionTyped("And commit")
        vm.sessionSend()
        assertNull(vm.openSession.value!!.ask)
        assertTrue("session-send:claude:s1:And commit" in phone.calls)
        assertEquals("no second question", controls, count("session-control"))
        assertEquals("", vm.openSession.value!!.draft)
    }

    @Test
    fun cancel_on_the_question_sends_nothing_and_keeps_the_words() {
        val vm = opened()
        vm.sessionTyped("Try again")
        vm.sessionSend()
        vm.sessionNo()
        assertNull(vm.openSession.value!!.ask)
        assertEquals("Try again", vm.openSession.value!!.draft)
        assertTrue(phone.calls.none { it.startsWith("session-control") })
    }

    // decision: HAND-663
    @Test
    fun an_answer_passes_the_choice_id_and_is_signed_each_time() {
        phone.sessionCards["claude:s1"] = mutableListOf(SessionCard("card-1", "permission", "Bash", "npm run build", "",
            listOf(SessionChoice("allow", "Yes"), SessionChoice("deny", "No"))))
        val vm = opened()
        assertEquals(1, vm.openSession.value!!.cards.size)
        vm.sessionAnswer("card-1", "deny")
        assertTrue("session-answer:claude:s1:card-1:deny" in phone.calls)
        assertEquals(KeyKind.STRONG to Why.YES, signers.last().let { it.first.kind to it.second })
        assertTrue(vm.openSession.value!!.cards.isEmpty())
    }

    // decision: GATE-622
    @Test
    fun stop_its_turn_takes_control_first_when_the_mac_refuses_it() {
        val vm = opened()
        vm.sessionInterrupt()
        val ask = vm.openSession.value!!.ask
        assertNotNull("refused: the same question, with no words", ask)
        assertEquals("", ask!!.text)
        assertTrue(ask.thenInterrupt)
        vm.sessionYes()
        assertTrue("session-control:claude:s1:" in phone.calls)
        assertEquals("the stop after the Yes", 2, count("session-interrupt"))
        assertEquals(R.string.sessions_interrupted, vm.ui.value.note?.res)
        // controlling it: Stop its turn goes at once
        vm.sessionInterrupt()
        assertEquals(3, count("session-interrupt"))
        assertNull(vm.openSession.value!!.ask)
    }

    @Test
    fun stop_controlling_lets_go() {
        phone.controlled += "claude:s1"
        val vm = opened()
        assertTrue(vm.openSession.value!!.session!!.controlledHere)
        vm.sessionRelease()
        assertTrue("session-release:claude:s1" in phone.calls)
        assertFalse(vm.openSession.value!!.session!!.controlledHere)
    }

    @Test
    fun hand_to_ferb_is_signed_and_said() {
        val vm = opened()
        vm.askHandOver()
        assertTrue(vm.openSession.value!!.handOver)
        vm.handOver("  ")
        assertTrue("no goal, nothing handed over", phone.calls.none { it.startsWith("session-delegate") })
        vm.handOver("Keep going until the tests pass")
        assertTrue("session-delegate:claude:s1:Keep going until the tests pass" in phone.calls)
        assertEquals(KeyKind.STRONG to Why.YES, signers.last().let { it.first.kind to it.second })
        assertFalse(vm.openSession.value!!.handOver)
        assertEquals(R.string.sessions_handed, vm.ui.value.note?.res)
    }

    @Test
    fun a_new_session_shows_the_exact_command_before_the_signed_start() {
        val vm = vm()
        vm.choose(PhoneTab.SESSIONS)
        vm.newSession()
        assertEquals(Sheet.NewSession, vm.sheet.value)
        assertTrue("session-prepare:::" in phone.calls)
        val n = vm.newSession.value!!
        assertEquals(phone.startFolders, n.folders)
        assertEquals(phone.startFolders.first(), n.folder)
        vm.newSessionAgent("codex")
        vm.newSessionFolder("~/code/blog")
        vm.newSessionText("Move the blog to the new theme")
        vm.newSessionNext()
        assertTrue("session-prepare:codex:~/code/blog:Move the blog to the new theme" in phone.calls)
        val yes = vm.newSession.value!!.yes
        assertNotNull("the command is shown before anything runs", yes)
        assertTrue(phone.calls.none { it.startsWith("session-start") })
        vm.newSessionStart()
        assertTrue("session-start:codex:~/code/blog:Move the blog to the new theme" in phone.calls)
        assertEquals(KeyKind.STRONG to Why.YES, signers.last().let { it.first.kind to it.second })
        assertNull(vm.sheet.value)
        assertNull(vm.newSession.value)
        assertEquals(R.string.sessions_started, vm.ui.value.note?.res)
    }

    // decision: PHN-797, PHN-454, PHN-444
    @Test
    fun watch_is_never_asked_for_on_the_shared_relay_and_is_out_of_screenshots() {
        phone.link.value = FakePhone.status(path = LinkPath.PUBLIC_RELAY)
        val vm = opened()
        vm.watchSession()
        assertEquals(R.string.not_on_shared_relay, vm.ui.value.note?.res)
        assertTrue(phone.calls.none { it.startsWith("session-live") })
        assertEquals(Sheet.Session("claude:s1"), vm.sheet.value)
        phone.link.value = FakePhone.status(path = LinkPath.DIRECT)
        vm.watchSession()
        assertTrue("session-live:claude:s1" in phone.calls)
        assertEquals(Sheet.SessionLive("claude:s1"), vm.sheet.value)
        assertTrue(vm.mustSecure.value)
        val reads = count("timeline")
        tick(); tick()
        assertEquals("the conversation isn't read while its window is watched", reads, count("timeline"))
        vm.closeLive()
        assertEquals("back to its conversation", Sheet.Session("claude:s1"), vm.sheet.value)
        assertEquals(1, phone.previewStopped)
    }

    // decision: PHN-15
    @Test
    fun nothing_of_a_session_stays_past_the_lock() {
        val vm = opened()
        vm.sessionTyped("half a thought")
        session.lockNow()
        assertNull(vm.sheet.value)
        assertNull(vm.openSession.value)
        assertFalse(vm.sessions.value.loaded)
        assertTrue(vm.sessions.value.sessions.isEmpty())
        val reads = phone.calls.size
        tick(); tick()
        assertEquals("nothing is read while locked", reads, phone.calls.size)
    }
}
