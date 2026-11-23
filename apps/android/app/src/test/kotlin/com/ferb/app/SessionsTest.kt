package com.ferb.app

import com.ferb.core.Outcome
import com.ferb.mobile.FerbException

import android.app.Application
import android.content.Context
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.SessionSendPhase
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
        // PHN-800: the owner's screenshot switch decides here (only Watch is kept out always)
        assertFalse("its conversation follows the owner's switch", vm.mustSecure.value)
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

    // decision: PHN-816
    @Test
    fun while_the_first_read_is_on_its_way_nothing_says_not_found_and_only_the_mac_s_answer_does() {
        val gate = kotlinx.coroutines.CompletableDeferred<Unit>()
        phone.timelineGate = gate
        val vm = opened()
        var o = vm.openSession.value!!
        assertFalse("still waiting for the Mac", o.loaded)
        assertNull("no error before the Mac answered", o.problem)
        phone.timelineRefusal = Outcome.Failed(FerbException.Refused("FERB can't find that session.", "not_found"))
        gate.complete(Unit)
        o = vm.openSession.value!!
        assertFalse(o.loaded)
        assertEquals("the Mac's answer, once it came", "FERB can't find that session.", o.problem?.words)
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

    // decision: PHN-800
    @Test
    fun a_send_the_mac_refuses_after_the_yes_shows_as_not_sent_with_why_and_goes_on_try_again() {
        val why = "FERB couldn't tell that the terminal in front in Code is this session's, so it typed nothing."
        phone.sendRefusal = Outcome.Failed(FerbException.Refused(why, "not_the_session"))
        val vm = opened()
        vm.sessionTyped("Run the tests again")
        vm.sessionSend()
        vm.sessionYes()
        val o = vm.openSession.value!!
        assertNull("the question closes: never stuck on the fingerprint", o.ask)
        assertEquals("Run the tests again", o.outgoing!!.text)
        assertEquals(why, o.outgoing!!.reason!!.words)
        assertEquals("the words left the box for the not-sent bubble", "", o.draft)
        assertTrue("nothing shows as sent", o.items.none { it.text == "Run the tests again" })
        // controlling it now: a direct send the Mac refuses is not sent either
        vm.sessionTyped("And commit")
        vm.sessionSend()
        assertEquals("And commit", vm.openSession.value!!.outgoing!!.text)
        // Try again, once the Mac can send: it goes, and the not-sent bubble leaves
        phone.sendRefusal = null
        vm.sessionRetry()
        assertTrue("session-send:claude:s1:And commit" in phone.calls)
        assertEquals("sent: the not-sent bubble is his sent message now", SessionSendPhase.SENT, vm.openSession.value!!.outgoing!!.phase)
    }

    // decision: PHN-812
    @Test
    fun a_send_shows_at_once_as_sending_then_as_sent_and_once_the_session_holds_it_only_once() {
        phone.controlled += "claude:s1"
        val vm = opened()
        val gate = kotlinx.coroutines.CompletableDeferred<Unit>()
        phone.sendGate = gate
        vm.sessionTyped("Run the tests again")
        vm.sessionSend()
        // FERB is still typing it on the Mac: the box is empty at once, his words show as sending
        var o = vm.openSession.value!!
        assertEquals("", o.draft)
        assertEquals("Run the tests again", o.outgoing!!.text)
        assertEquals(SessionSendPhase.SENDING, o.outgoing!!.phase)
        assertTrue("session-send" in vm.ui.value.busy)
        // a second tap while it goes sends nothing more
        vm.sessionTyped("And commit")
        vm.sessionSend()
        assertEquals(1, count("session-send:claude:s1:"))
        vm.sessionTyped("")
        gate.complete(Unit)
        o = vm.openSession.value!!
        assertEquals("the Mac typed it: his plain bubble", SessionSendPhase.SENT, o.outgoing!!.phase)
        assertFalse("session-send" in vm.ui.value.busy)
        // the session's own record of it arrives: the bubble gives way to it, never shown twice
        phone.sessionItems["claude:s1"]!! += FakePhone.item(4, "user", "Run the tests again")
        tick()
        o = vm.openSession.value!!
        assertNull(o.outgoing)
        assertEquals(1, o.items.count { it.text == "Run the tests again" && it.seq == 4uL })
    }

    // decision: PHN-812, PHN-800
    @Test
    fun a_send_that_fails_while_sending_shows_not_sent_with_its_words_and_try_again_sends_them() {
        phone.controlled += "claude:s1"
        val why = "FERB couldn't tell that the terminal in front in Code is this session's, so it typed nothing."
        phone.sendRefusal = Outcome.Failed(FerbException.Refused(why, "not_the_session"))
        val vm = opened()
        val gate = kotlinx.coroutines.CompletableDeferred<Unit>()
        phone.sendGate = gate
        vm.sessionTyped("Deploy to staging")
        vm.sessionSend()
        assertEquals(SessionSendPhase.SENDING, vm.openSession.value!!.outgoing!!.phase)
        gate.complete(Unit)
        val o = vm.openSession.value!!
        assertEquals(SessionSendPhase.NOT_SENT, o.outgoing!!.phase)
        assertEquals("his words are kept", "Deploy to staging", o.outgoing!!.text)
        assertEquals(why, o.outgoing!!.reason!!.words)
        phone.sendRefusal = null
        vm.sessionRetry()
        assertEquals(2, count("session-send:claude:s1:Deploy to staging"))
        assertEquals(SessionSendPhase.SENT, vm.openSession.value!!.outgoing!!.phase)
    }

    // decision: PHN-812, GATE-622
    @Test
    fun after_the_yes_the_question_closes_and_his_words_show_as_sending() {
        val vm = opened()
        vm.sessionTyped("Run the tests again")
        vm.sessionSend()
        assertNull("asked first: nothing on its way yet", vm.openSession.value!!.outgoing)
        assertNotNull(vm.openSession.value!!.ask)
        val gate = kotlinx.coroutines.CompletableDeferred<Unit>()
        phone.sendGate = gate
        vm.sessionYes()
        assertNull("the question closes at once", vm.openSession.value!!.ask)
        assertEquals(SessionSendPhase.SENDING, vm.openSession.value!!.outgoing!!.phase)
        gate.complete(Unit)
        assertEquals(SessionSendPhase.SENT, vm.openSession.value!!.outgoing!!.phase)
    }

    // decision: HAND-683
    @Test
    fun when_the_mac_cant_tell_the_terminal_he_chooses_one_signed_or_doesnt_send() {
        val why = "FERB couldn't tell which terminal in Code is this session's, so it typed nothing. Choose it, and FERB types only there."
        val offered = listOf(
            com.ferb.mobile.SessionChoice("front", "The terminal in front in Code now (Terminal 1, 2.1.330)"),
            com.ferb.mobile.SessionChoice("w7:1:Terminal 2, 2.1.330", "main.rs — app · Terminal 2, 2.1.330"),
        )
        phone.sessions[0] = FakePhone.agentSession(terminals = offered)
        phone.sendRefusal = Outcome.Failed(FerbException.Refused(why, "choose_terminal"))
        val vm = opened()
        vm.sessionTyped("Run the tests again")
        vm.sessionSend()
        vm.sessionYes()
        val o = vm.openSession.value!!
        assertNull(o.ask)
        assertEquals("the terminals the Mac can see, to choose from", offered, o.pick!!.terminals)
        assertEquals(why, o.pick!!.why)
        assertEquals("Run the tests again", o.outgoing!!.text)
        // Don't send: nothing typed, the words stay as not sent
        vm.sessionPickNo()
        assertNull(vm.openSession.value!!.pick)
        assertEquals("Run the tests again", vm.openSession.value!!.outgoing!!.text)
        // Try again asks to choose again; his choice is signed as control is, and the words go there
        vm.sessionRetry()
        assertNotNull(vm.openSession.value!!.pick)
        val signed = phone.signed.size
        vm.sessionPickTerminal("w7:1:Terminal 2, 2.1.330")
        assertTrue("session-control:claude:s1:Run the tests again:terminal=w7:1:Terminal 2, 2.1.330" in phone.calls)
        assertEquals(signed + 1, phone.signed.size)
        assertEquals(KeyKind.STRONG to Why.YES, signers.last().let { it.first.kind to it.second })
        assertNull(vm.openSession.value!!.pick)
        assertEquals("sent: the not-sent bubble is his sent message now", SessionSendPhase.SENT, vm.openSession.value!!.outgoing!!.phase)
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

    // decision: HAND-691, HAND-695
    @Test
    fun a_new_session_is_a_terminal_with_no_agent_to_pick_and_only_a_command_is_asked_about() {
        val vm = vm()
        vm.choose(PhoneTab.SESSIONS)
        vm.newSession()
        assertEquals(Sheet.NewSession, vm.sheet.value)
        assertTrue("session-prepare:::" in phone.calls)
        val n = vm.newSession.value!!
        assertEquals(phone.startFolders, n.folders)
        assertEquals(phone.startFolders.first(), n.chosen)
        // no command: it opens at once (signed), nothing asked, nothing typed
        vm.newSessionFolder("~/code/blog")
        vm.newSessionNext()
        assertTrue("session-prepare:terminal:~/code/blog:" in phone.calls)
        assertTrue("session-start:terminal:~/code/blog:" in phone.calls)
        assertEquals(KeyKind.STRONG to Why.YES, signers.last().let { it.first.kind to it.second })
        assertNull(vm.newSession.value)
        assertEquals(R.string.sessions_started, vm.ui.value.note?.res)
        // decision: PHN-822 — the Mac named the new session: it opens at once
        assertEquals(Sheet.Session("term:300"), vm.sheet.value)
        vm.closeSession()
        // a command: shown exactly before anything runs, then typed exactly
        phone.calls.clear()
        vm.newSession()
        vm.newSessionFolder("~/code/web-app")
        vm.newSessionText(" agy ")
        vm.newSessionNext()
        assertTrue("session-prepare:terminal:~/code/web-app:agy" in phone.calls)
        assertEquals("agy", vm.newSession.value!!.yes!!.detail)
        assertTrue(phone.calls.none { it.startsWith("session-start") })
        vm.newSessionStart()
        assertTrue("session-start:terminal:~/code/web-app:agy" in phone.calls)
        assertEquals(Sheet.Session("term:300"), vm.sheet.value)
    }

    // decision: PHN-822
    @Test
    fun start_shows_its_pending_state_until_the_mac_answers_and_a_refusal_stays_with_the_reason() {
        val vm = vm()
        vm.choose(PhoneTab.SESSIONS)
        vm.newSession()
        val gate = kotlinx.coroutines.CompletableDeferred<Unit>()
        phone.startGate = gate
        phone.startRefusal = Outcome.Failed(FerbException.Refused("That folder isn't there any more. Choose another one.", "ferb_error"))
        vm.newSessionNext()
        assertTrue("pending at once: Start turns, the form takes no taps", "session-prepare" in vm.ui.value.busy)
        assertEquals(Sheet.NewSession, vm.sheet.value)
        gate.complete(Unit)
        assertFalse("session-prepare" in vm.ui.value.busy)
        val n = vm.newSession.value!!
        assertEquals("the sheet stays with the Mac's reason", Sheet.NewSession, vm.sheet.value)
        assertEquals("That folder isn't there any more. Choose another one.", n.problem?.words)
        assertEquals("what he chose is kept", phone.startFolders.first(), n.chosen)
        // tried again and opened: the reason goes with the sheet
        phone.startRefusal = null
        phone.startGate = null
        vm.newSessionNext()
        assertNull(vm.newSession.value)
    }

    // decision: PHN-822
    @Test
    fun a_new_terminal_the_mac_hadn_t_named_yet_shows_a_quiet_line_until_it_appears() {
        phone.startedSession = ""
        val vm = vm()
        vm.choose(PhoneTab.SESSIONS)
        vm.newSession()
        vm.newSessionNext()
        assertNull("the sheet closed", vm.sheet.value)
        assertNotNull("the list says it will show in a moment", vm.sessions.value.coming)
        tick()
        assertNotNull("still not there: the line stays", vm.sessions.value.coming)
        phone.sessions += FakePhone.agentSession("term:300", "", "idle", "Terminal").copy(agent = "terminal", agentName = "Terminal")
        tick()
        assertNull("it showed: the line goes", vm.sessions.value.coming)
        assertTrue(vm.sessions.value.sessions.any { it.id == "term:300" })
    }

    // decision: PHN-821
    @Test
    fun another_folder_is_chosen_in_the_macs_folders_and_back_leaves_the_sheet_as_it_was() {
        phone.folders["~"] = com.ferb.mobile.Folder("/Users/you", "/Users", false, listOf(
            com.ferb.mobile.FileEntry("code", "/Users/you/code", true, false, 0u, 0, false),
            com.ferb.mobile.FileEntry("notes.txt", "/Users/you/notes.txt", false, false, 12u, 0, false),
        ), 2u, false, 0u, null)
        phone.folders["/Users/you/code"] = com.ferb.mobile.Folder("/Users/you/code", "/Users/you", false, listOf(
            com.ferb.mobile.FileEntry("site", "/Users/you/code/site", true, false, 0u, 0, false),
        ), 1u, false, 0u, null)
        phone.recent = Outcome.Ok(com.ferb.mobile.RecentFiles(listOf(
            com.ferb.mobile.FoundFile("blog", "/Users/you/code/blog", true, 0u, 0, "fetched", null, null, null),
            com.ferb.mobile.FoundFile("a.pdf", "/Users/you/a.pdf", false, 1u, 0, "fetched", null, null, null),
        ), 0u))
        val vm = vm()
        vm.choose(PhoneTab.SESSIONS)
        vm.newSession()
        val before = vm.newSession.value!!
        vm.newSessionBrowse()
        assertEquals("the recent folders only, never files", listOf("/Users/you/code/blog"), vm.newSession.value!!.browse!!.recent)
        // Back from the places: the sheet exactly as it was
        vm.newSessionBrowseBack()
        assertEquals(before, vm.newSession.value)
        vm.newSessionBrowse()
        vm.newSessionBrowseOpen("~")
        vm.newSessionBrowseOpen("/Users/you/code")
        assertEquals("/Users/you/code", vm.newSession.value!!.browse!!.shown!!.path)
        // Back goes up a folder without asking the Mac again
        val reads = count("files:")
        vm.newSessionBrowseBack()
        assertEquals("/Users/you", vm.newSession.value!!.browse!!.shown!!.path)
        assertEquals(reads, count("files:"))
        vm.newSessionBrowseOpen("/Users/you/code")
        vm.newSessionBrowseUse()
        val n = vm.newSession.value!!
        assertNull(n.browse)
        assertEquals("the folder showing is the chosen one", "/Users/you/code", n.chosen)
        assertEquals("first in the list, the Mac's own kept as they were", listOf("/Users/you/code") + phone.startFolders, n.rows)
        vm.newSessionNext()
        assertTrue("session-start:terminal:/Users/you/code:" in phone.calls)
    }

    // decision: PHN-821
    @Test
    fun closing_the_folder_picker_changes_nothing() {
        phone.folders["~/Desktop"] = com.ferb.mobile.Folder("/Users/you/Desktop", "/Users/you", false, emptyList(), 0u, false, 0u, null)
        val vm = vm()
        vm.choose(PhoneTab.SESSIONS)
        vm.newSession()
        val before = vm.newSession.value!!
        vm.newSessionBrowse()
        vm.newSessionBrowseOpen("~/Desktop")
        vm.newSessionBrowseClose()
        assertEquals(before, vm.newSession.value)
    }

    // ---- FERB's hand-over (HAND-702) ----------------------------------------------------------------------------------

    // decision: HAND-702
    @Test
    fun hand_over_shows_its_pending_state_and_a_refusal_keeps_the_sheet_with_the_reason() {
        val vm = opened()
        vm.askHandOver()
        val gate = kotlinx.coroutines.CompletableDeferred<Unit>()
        phone.delegateGate = gate
        phone.delegateRefusal = Outcome.Failed(FerbException.Refused("FERB can't take this session: it is not running.", "ferb_error"))
        vm.handOver("Keep going until the tests pass")
        assertTrue("pending at once", "session-hand" in vm.ui.value.busy)
        gate.complete(Unit)
        val o = vm.openSession.value!!
        assertTrue("the sheet stays", o.handOver)
        assertEquals("FERB can't take this session: it is not running.", o.handProblem?.words)
        assertFalse("session-hand" in vm.ui.value.busy)
        // done: the sheet closes and the session is read at once
        phone.delegateRefusal = null
        phone.delegateGate = null
        val reads = phone.timelineAsks.size
        vm.handOver("Keep going until the tests pass")
        assertFalse(vm.openSession.value!!.handOver)
        assertNull(vm.openSession.value!!.handProblem)
        assertTrue("read at once", phone.timelineAsks.size > reads)
    }

    // decision: HAND-702
    @Test
    fun take_it_back_stops_ferbs_task_closes_the_menu_and_reads_the_session_at_once() {
        phone.sessions[0] = FakePhone.agentSession(withFerb = true, ferbTask = "task-9", ferbNote = "FERB is working in this session.")
        val vm = opened()
        assertEquals("FERB is working in this session.", vm.openSession.value!!.session!!.ferbNote)
        vm.sessionMenu(true)
        phone.sessions[0] = FakePhone.agentSession(ferbNote = "FERB stopped. The session is yours again.")
        val reads = phone.timelineAsks.size
        vm.takeBack()
        assertTrue("the task stop with FERB's task", "stop:task-9" in phone.calls)
        val o = vm.openSession.value!!
        assertFalse("the menu closes", o.menu)
        assertTrue("read at once", phone.timelineAsks.size > reads)
        assertEquals("", o.session!!.ferbTask)
        assertFalse(o.session!!.withFerb)
        assertEquals("FERB stopped. The session is yours again.", o.session!!.ferbNote)
    }

    // decision: HAND-702
    @Test
    fun take_it_back_refused_says_why() {
        phone.sessions[0] = FakePhone.agentSession(withFerb = true, ferbTask = "task-9")
        phone.stopResult = Outcome.Failed(FerbException.Refused("That task already ended.", "not_found"))
        val vm = opened()
        vm.sessionMenu(true)
        vm.takeBack()
        val note = vm.ui.value.note!!
        assertEquals(R.string.sessions_take_back_failed, note.res)
        assertEquals("That task already ended.", note.inner?.words)
        assertEquals("Couldn't take it back: That task already ended.", context.getString(note.res, context.getString(note.inner!!.res, note.inner!!.words)))
        assertTrue("the menu stays to try again", vm.openSession.value!!.menu)
    }

    // decision: HAND-692, HAND-710
    @Test
    fun a_terminal_row_is_named_by_its_window_and_an_app_s_window_by_its_title_then_its_app() {
        phone.sessions += FakePhone.agentSession("term:110", "", "idle", "Terminal").copy(agent = "terminal", agentName = "agy", program = "agy",
            folder = "/Users/owner/Desktop", folderName = "Desktop", window = "Desktop — agy — 120×30", watch = true)
        phone.sessions += appWindow
        val vm = vm()
        vm.choose(PhoneTab.SESSIONS)
        val ui = vm.sessions.value
        assertEquals("Desktop — agy — 120×30", com.ferb.app.model.Sessions.title(ui.sessions.first { it.id == "term:110" }))
        assertEquals("checkout.ts — shop", com.ferb.app.model.Sessions.title(ui.sessions.first { it.id == "app:cursor:1" }))
        assertEquals("no title: the app's name", "Cursor", com.ferb.app.model.Sessions.title(appWindow.copy(title = "", window = "")))
    }

    // ---- a session FERB reads no words of: its window, live, and "Type here" (HAND-710) -------------------------------

    /** A terminal FERB reads no words of. */
    private val watched = FakePhone.agentSession("term:5", "", "idle", "Terminal").copy(agent = "terminal", agentName = "zsh", program = "zsh",
        folder = "/Users/owner/code/web-app", folderName = "web-app", window = "web-app — zsh — 120×30", watch = true)

    /** One open window of an agent app. */
    private val appWindow = FakePhone.agentSession("app:cursor:1", "checkout.ts — shop", "idle", "Cursor").copy(agent = "app_chat",
        agentName = "Cursor", folder = "", folderName = "", window = "checkout.ts — shop", watch = true)

    private fun openedWatch(): FerbViewModel {
        phone.sessions += watched
        return vm().also { it.choose(PhoneTab.SESSIONS); it.openSession("term:5") }
    }

    // decision: HAND-710, PHN-810
    @Test
    fun a_watch_session_shows_its_window_live_and_polls_no_conversation() {
        val vm = openedWatch()
        assertEquals(Sheet.Session("term:5"), vm.sheet.value)
        assertTrue("its window is asked for at once", "session-live:term:5" in phone.calls)
        assertEquals("term:5", vm.live.view.value?.taskId)
        assertFalse("its window follows the owner's switch (PHN-810)", vm.mustSecure.value)
        val reads = count("timeline:term:5")
        assertEquals("its record is read once", 1, reads)
        tick(); tick()
        assertEquals("its conversation is always empty: nothing is polled", reads, count("timeline:term:5"))
        assertEquals("asked for once", 1, count("session-live:term:5"))
        vm.closeSession()
        assertNull(vm.live.view.value)
        assertEquals("its picture stops with its screen", 1, phone.previewStopped)
    }

    // decision: HAND-710, PHN-454
    @Test
    fun a_watch_session_s_window_stops_in_the_background_and_comes_back_in_front() {
        val vm = openedWatch()
        session.foreground(false)
        assertEquals(1, phone.previewStopped)
        assertNull(vm.live.view.value)
        session.foreground(true)
        assertEquals("watched again once in front", 2, count("session-live:term:5"))
        assertEquals("term:5", vm.live.view.value?.taskId)
    }

    // decision: HAND-710, PHN-797
    @Test
    fun on_the_shared_relay_a_watch_session_says_the_relay_s_rule_where_its_window_would_be() {
        phone.link.value = FakePhone.status(path = LinkPath.PUBLIC_RELAY)
        val vm = openedWatch()
        assertTrue(phone.calls.none { it.startsWith("session-live") })
        assertEquals(R.string.not_on_shared_relay, vm.live.view.value?.ended?.res)
    }

    // decision: HAND-710
    @Test
    fun a_watch_session_is_never_handed_to_ferb() {
        val vm = openedWatch()
        vm.askHandOver()
        assertFalse(vm.openSession.value!!.handOver)
    }

    // decision: HAND-710, PHN-812
    @Test
    fun a_send_to_a_watch_session_ends_sending_at_the_mac_s_answer() {
        phone.controlled += "term:5"
        val vm = openedWatch()
        val gate = kotlinx.coroutines.CompletableDeferred<Unit>()
        phone.sendGate = gate
        vm.sessionTyped("npm test")
        vm.sessionSend()
        var o = vm.openSession.value!!
        assertEquals("", o.draft)
        assertEquals(SessionSendPhase.SENDING, o.outgoing!!.phase)
        gate.complete(Unit)
        o = vm.openSession.value!!
        assertNull("typed: no record will come, so nothing waits for one", o.outgoing)
        assertFalse("the box takes the next words at once", "session-send" in vm.ui.value.busy)
        assertTrue("session-send:term:5:npm test" in phone.calls)
        tick(); tick()
        assertNull(vm.openSession.value!!.outgoing)
    }

    // decision: HAND-710, GATE-622
    @Test
    fun a_watch_session_asks_first_as_today_and_after_the_yes_sending_ends_at_the_answer() {
        val vm = openedWatch()
        vm.sessionTyped("npm test")
        vm.sessionSend()
        assertNotNull("the Mac asked: nothing was sent", vm.openSession.value!!.ask)
        vm.sessionYes()
        assertTrue("session-control:term:5:npm test" in phone.calls)
        val o = vm.openSession.value!!
        assertNull(o.ask)
        assertNull("sent: gone at the answer", o.outgoing)
    }

    // decision: HAND-710, PHN-800
    @Test
    fun a_send_to_a_watch_session_the_mac_refuses_shows_not_sent_with_why_and_try_again() {
        phone.controlled += "term:5"
        val why = "FERB couldn't bring that window to the front, so it typed nothing."
        phone.sendRefusal = Outcome.Failed(FerbException.Refused(why, "ferb_error"))
        val vm = openedWatch()
        vm.sessionTyped("npm test")
        vm.sessionSend()
        var o = vm.openSession.value!!
        assertEquals(SessionSendPhase.NOT_SENT, o.outgoing!!.phase)
        assertEquals("npm test", o.outgoing!!.text)
        assertEquals(why, o.outgoing!!.reason!!.words)
        phone.sendRefusal = null
        vm.sessionRetry()
        o = vm.openSession.value!!
        assertNull("sent on Try again: gone at the answer", o.outgoing)
        assertEquals(2, count("session-send:term:5:npm test"))
    }

    // decision: HAND-702
    @Test
    fun a_row_s_title_is_never_empty() {
        val bare = FakePhone.agentSession("term:7", "", "idle", "").copy(agent = "terminal", agentName = "Terminal", folder = "", folderName = "", window = "")
        assertEquals("the agent's name last", "Terminal", com.ferb.app.model.Sessions.title(bare))
        assertEquals("the folder's name before it", "site", com.ferb.app.model.Sessions.title(bare.copy(folder = "/Users/you/site/")))
        assertEquals("web-app", com.ferb.app.model.Sessions.title(bare.copy(folder = "/Users/you/web-app", folderName = "web-app")))
        assertEquals("the window's words before that", "zsh — 80×24", com.ferb.app.model.Sessions.title(bare.copy(window = "zsh — 80×24", folderName = "web-app")))
        assertEquals("Codex", com.ferb.app.model.Sessions.title(FakePhone.agentSession("codex:9", "", "idle", "").copy(folder = "", folderName = "")))
    }

    // decision: HAND-716
    @Test
    fun a_reply_s_parts_become_one_reply_and_markdown_only_where_it_is_markdown() {
        val read = ArrayList<String>()
        val rich: (String) -> List<com.ferb.design.components.RichBlock> = { read += it; listOf(com.ferb.design.components.RichBlock.Paragraph(listOf(com.ferb.design.components.RichRun(it)))) }
        val reply = FakePhone.item(9, "assistant", "", parts = listOf(
            FakePhone.part("thinking", label = "Thought 1s"),
            FakePhone.part("text", "I'll look through this folder."),
            FakePhone.part("tool", "  Length  Name\n   19357  a.json", "List zip sizes and contents", "ls, echo, unzip, head"),
            FakePhone.part("tool", "", "Explored", "2 searches"),
            FakePhone.part("something_new", "Words of a kind the app doesn't know."),
            FakePhone.part("meta", "Just now"),
        ))
        val line = com.ferb.app.model.Sessions.line(reply, "", rich = rich)
        val k = com.ferb.design.components.SessionPartKind.entries
        assertEquals(listOf(k[1], k[0], k[2], k[2], k[0], k[3]), line.parts.map { it.kind })
        assertEquals("only the words are read as Markdown", listOf("I'll look through this folder.", "Words of a kind the app doesn't know."), read)
        assertEquals("Thought 1s", line.parts[0].label)
        assertEquals("", line.parts[0].text)
        assertEquals("List zip sizes and contents" to "ls, echo, unzip, head", line.parts[2].label to line.parts[2].detail)
        assertTrue("its output opens under it", line.parts[2].text.isNotBlank())
        assertTrue("nothing to open: no chevron", line.parts[3].text.isBlank())
        assertEquals("Just now", line.parts[5].text)
        // without parts: an agent's words as Markdown, a terminal's output as it is, the owner's words plain
        read.clear()
        assertNotNull(com.ferb.app.model.Sessions.line(FakePhone.item(10, "assistant", "| a | b |"), "", rich = rich).blocks)
        assertNull(com.ferb.app.model.Sessions.line(FakePhone.item(11, "assistant", "$ ls\nsrc  tests"), "", terminal = true, rich = rich).blocks)
        assertNull(com.ferb.app.model.Sessions.line(FakePhone.item(12, "user", "**hi**"), "", rich = rich).blocks)
        assertEquals(listOf("| a | b |"), read)
    }

    // decision: HAND-709
    @Test
    fun a_message_ferb_sent_is_marked_as_ferbs_and_the_owners_own_is_not() {
        val sent = com.ferb.app.model.Sessions.line(FakePhone.item(20, "user", "run the tests", ferb = true), "", sentByFerb = "Sent by FERB")
        assertEquals(com.ferb.design.components.SessionLineKind.OWNER, sent.kind)
        assertEquals("Sent by FERB", sent.by)
        assertNull(com.ferb.app.model.Sessions.line(FakePhone.item(21, "user", "hi"), "", sentByFerb = "Sent by FERB").by)
    }

    // decision: PHN-797, PHN-454, PHN-444
    // decision: PHN-810
    @Test
    fun watch_is_never_asked_for_on_the_shared_relay_and_follows_the_screenshot_switch() {
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
        assertFalse("its window follows the owner's switch (PHN-810)", vm.mustSecure.value)
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
