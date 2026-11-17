package com.ferb.app

import android.app.Application
import android.os.Looper
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.Picked
import com.ferb.app.model.Why
import java.io.File
import java.nio.file.Files
import com.ferb.core.KeyHealth
import com.ferb.core.KeyKind
import com.ferb.core.KeyState
import com.ferb.core.Outcome
import com.ferb.core.PhoneState
import com.ferb.mobile.FerbException
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.LinkProblem
import com.ferb.mobile.OlderState
import com.ferb.mobile.QuestionItem
import com.ferb.mobile.TranscriptView
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestCoroutineScheduler
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.After
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

/** The app's state over a fake phone: the gate, unlocking, the outbox, questions and Stop. */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class ViewModelTest {
    private val phone = FakePhone()
    private var t = 0L
    private val scheduler = TestCoroutineScheduler()
    private lateinit var session: Session
    private var screenLock = true
    private val signers = ArrayList<FakeSigner>()
    private var localPrompt = true
    private var cancelPrompt = false
    private var localAsked = 0
    private val whys = ArrayList<Why>()
    private val root: File = Files.createTempDirectory("ferb-app-files").toFile()
    private val picked = HashMap<String, ByteArray>()
    private val local = LocalFiles(root) { uri -> picked[uri]?.let { b -> Picked(uri.substringAfterLast('/'), b.size.toLong()) { b.inputStream() } } }

    @Before
    fun setUp() {
        Dispatchers.setMain(UnconfinedTestDispatcher(scheduler))
        session = Session { t }
    }

    @After
    fun tearDown() {
        Dispatchers.resetMain()
        root.deleteRecursively()
    }

    private fun vm(): FerbViewModel {
        val prefs = Prefs(ApplicationProvider.getApplicationContext())
        return FerbViewModel(phone, session, prefs, { screenLock }, null, SavedStateHandle(), local, wait = {}).also {
            it.prompts = object : Prompts {
                override fun signer(kind: KeyKind, why: Why) = FakeSigner(kind, cancelPrompt).also { s -> signers.add(s); whys.add(why) }
                override suspend fun local(): Boolean { localAsked++; return localPrompt }
            }
        }
    }

    private fun question(id: String, open: Boolean = true, body: String = """{"kind":"owner_question","reason":"Go ahead?","options":["Yes","No"],"capability":"plan.ask"}""") =
        QuestionItem("e-$id", id, "r-1", "t-1", if (open) "open" else "answered", open, body, "end_of_chat", null)

    private fun view(vararg qs: QuestionItem) = TranscriptView("c1", emptyList(), qs.toList(), emptyList(), emptyList(), OlderState.Complete)

    // decision: PHN-8, PHN-125, PHN-505
    @Test
    fun nothing_shows_before_the_screen_lock_and_the_unlock() {
        phone.state.value = PhoneState.Starting
        val vm = vm()
        assertEquals(Gate.STARTING, vm.ui.value.gate)
        phone.state.value = PhoneState.Open(KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK))
        screenLock = false
        vm.recheck()
        assertEquals(Gate.NO_SCREEN_LOCK, vm.ui.value.gate)
        screenLock = true
        vm.recheck()
        assertEquals(Gate.LOCKED, vm.ui.value.gate)
        session.unlocked()
        assertEquals(Gate.MAIN, vm.ui.value.gate)
        phone.link.value = FakePhone.status(LinkPhase.IDLE, paired = false)
        assertEquals(Gate.PAIR, vm.ui.value.gate)
        phone.link.value = FakePhone.status(LinkPhase.PAIRING_WAIT_FOR_MAC, paired = false, code = "482 719")
        assertEquals(Gate.CODES, vm.ui.value.gate)
    }

    // decision: PHN-13, PHN-125, PHN-505
    @Test
    fun one_prompt_opens_the_app_and_unlocks_the_mac() {
        val vm = vm()
        vm.unlockApp()
        shadowOf(Looper.getMainLooper()).idle()
        assertFalse(session.locked.value)
        assertEquals(listOf(KeyKind.LOCK), signers.map { it.kind })
        assertTrue("unlock" in phone.calls)
        assertFalse(vm.ui.value.needsMacUnlock)
    }

    // decision: PHN-8
    @Test
    fun a_cancelled_prompt_leaves_the_app_locked() {
        cancelPrompt = true
        localPrompt = false
        val vm = vm()
        vm.unlockApp()
        shadowOf(Looper.getMainLooper()).idle()
        assertTrue(session.locked.value)
    }

    /** Every prompt the owner saw: a signing prompt that was asked to sign, or the prompt alone. */
    private fun shown() = signers.sumOf { it.asked } + localAsked

    private fun idle() = shadowOf(Looper.getMainLooper()).idle()

    /** The app leaves for [ms] and comes back: paused, the process away, then in front, resumed and focused. */
    private fun away(vm: FerbViewModel, ms: Long) {
        vm.windowReady(false)
        session.foreground(false)
        t += ms
        session.foreground(true)
        vm.windowReady(true)
        idle()
    }

    // decision: PHN-615, PHN-616, PHN-8
    @Test
    fun the_prompt_appears_by_itself_once_when_the_app_opens() {
        val vm = vm()
        vm.windowReady(true)
        idle()
        assertEquals("nothing before the app is in front", 0, shown())
        vm.windowReady(false)
        session.foreground(true)
        idle()
        assertEquals("nothing while the activity isn't resumed and focused", 0, shown())
        vm.windowReady(true)
        idle()
        assertEquals(1, shown())
        assertFalse(session.locked.value)
        vm.windowReady(false)
        vm.windowReady(true)
        idle()
        assertEquals("the prompt's own window closing asks nothing more", 1, shown())
    }

    // decision: PHN-615, PHN-15
    @Test
    fun back_after_the_relock_it_asks_once_and_a_short_absence_asks_nothing() {
        val vm = vm()
        session.foreground(true)
        vm.windowReady(true)
        idle()
        assertEquals(1, shown())
        away(vm, 2 * 60_000L)
        assertFalse("inside five minutes the app stays open", session.locked.value)
        assertEquals(1, shown())
        away(vm, com.ferb.app.model.AppLock.LOCK_AFTER_MS + 1)
        assertEquals("one prompt on the return after the relock", 2, shown())
        assertFalse(session.locked.value)
    }

    // decision: PHN-615, PHN-616
    @Test
    fun a_cancelled_prompt_waits_for_the_button_or_a_return() {
        cancelPrompt = true
        localPrompt = false
        val vm = vm()
        session.foreground(true)
        vm.windowReady(true)
        idle()
        assertEquals(1, shown())
        assertTrue(session.locked.value)
        assertEquals("the lock page stays", Gate.LOCKED, vm.ui.value.gate)
        vm.windowReady(false)
        vm.windowReady(true)
        idle()
        assertEquals("no loop after a cancel", 1, shown())
        vm.unlockApp()
        idle()
        assertEquals("the button asks again", 2, shown())
        vm.windowReady(false)
        vm.windowReady(true)
        idle()
        assertEquals(2, shown())
        away(vm, 10_000L)
        assertEquals("leaving and coming back asks again", 3, shown())
    }

    // decision: PHN-616
    @Test
    fun the_owners_own_lock_waits_for_his_tap() {
        val vm = vm()
        session.foreground(true)
        session.unlocked()
        vm.windowReady(true)
        idle()
        session.lockNow()
        idle()
        assertEquals(Gate.LOCKED, vm.ui.value.gate)
        vm.windowReady(false)
        vm.windowReady(true)
        idle()
        assertEquals(0, shown())
    }

    // decision: PHN-615, PHN-616
    @Test
    fun a_turn_of_the_screen_never_asks_twice() {
        cancelPrompt = true
        val vm = vm()
        session.foreground(true)
        vm.windowReady(true)
        idle()
        assertEquals(1, shown())
        // the activity is made again over the same view model: paused, resumed, focused
        vm.windowReady(false)
        vm.windowReady(true)
        vm.windowReady(true)
        idle()
        assertEquals(1, shown())
    }

    // decision: PHN-615, PHN-13
    @Test
    fun a_cold_start_waiting_for_the_link_asks_once_and_only_on_the_owners_screen() {
        phone.link.value = FakePhone.status(LinkPhase.CONNECTING)
        val vm = vm()
        session.foreground(true)
        vm.windowReady(true)
        idle()
        assertEquals("waiting for the link (one prompt for the app and the Mac)", 0, shown())
        // a turn of the screen while it waits: no second prompt is started
        vm.windowReady(false)
        vm.windowReady(true)
        phone.link.value = FakePhone.status()
        idle()
        assertEquals(1, shown())
        assertFalse(session.locked.value)
    }

    // decision: PHN-615
    @Test
    fun a_prompt_due_while_the_screen_is_elsewhere_waits_for_it() {
        phone.link.value = FakePhone.status(LinkPhase.CONNECTING)
        val vm = vm()
        session.foreground(true)
        vm.windowReady(true)
        idle()
        vm.windowReady(false) // a system dialog over FERB while the link was awaited
        phone.link.value = FakePhone.status()
        idle()
        assertEquals(0, shown())
        assertTrue(session.locked.value)
        vm.windowReady(true)
        idle()
        assertEquals(1, shown())
    }

    // decision: PHN-640, PHN-641, PHN-13
    @Test
    fun the_app_opens_when_the_prompt_passes_not_when_the_slow_mac_answers() {
        phone.answerGate = CompletableDeferred()
        val vm = vm()
        vm.unlockApp()
        idle()
        assertFalse("open at once after the fingerprint", session.locked.value)
        assertEquals(1, shown())
        assertEquals(MacLine.OPENING, vm.ui.value.macLine)
        assertFalse("the lock button is free again", "unlock" in vm.ui.value.busy)
        scheduler.advanceTimeBy(FerbViewModel.MAC_SLOW_MS + 1)
        idle()
        assertEquals("a slow Mac is said, calmly", MacLine.SLOW, vm.ui.value.macLine)
        phone.answerGate!!.complete(Unit)
        idle()
        assertNull("the line clears by itself", vm.ui.value.macLine)
        assertFalse(vm.ui.value.needsMacUnlock)
        assertEquals("nothing asked twice", 1, shown())
    }

    // decision: PHN-640, PHN-641
    @Test
    fun a_mac_that_cant_be_reached_never_holds_the_prompt_or_the_app() {
        phone.link.value = FakePhone.status(LinkPhase.CONNECTING)
        val vm = vm()
        vm.unlockApp()
        idle()
        assertEquals("waiting briefly for the link", 0, shown())
        scheduler.advanceTimeBy(FerbViewModel.PROMPT_WAIT_MS + 1)
        idle()
        assertEquals("the prompt alone after the bound", 1, localAsked)
        assertTrue("no challenge prompt", signers.isEmpty())
        assertFalse(session.locked.value)
        assertFalse("no control to tap: the line says the Mac can't be reached", vm.ui.value.needsMacUnlock)
    }

    // decision: PHN-640, PHN-641
    @Test
    fun a_slow_challenge_gives_the_prompt_alone_and_never_a_late_second_prompt() {
        phone.challengeGate = CompletableDeferred()
        val vm = vm()
        vm.unlockApp()
        idle()
        assertEquals(0, shown())
        scheduler.advanceTimeBy(FerbViewModel.PROMPT_WAIT_MS + 1)
        idle()
        assertEquals(1, localAsked)
        assertFalse(session.locked.value)
        phone.challengeGate!!.complete(Unit) // the challenge comes after all: no prompt appears out of nowhere
        idle()
        assertEquals(0, signers.sumOf { it.asked })
    }

    // decision: PHN-640, PHN-641
    @Test
    fun a_second_tap_while_waiting_shows_the_prompt_at_once() {
        phone.link.value = FakePhone.status(LinkPhase.CONNECTING)
        val vm = vm()
        vm.unlockApp()
        idle()
        assertEquals(0, shown())
        vm.unlockApp()
        idle()
        assertEquals("the second tap is never a silent no-op", 1, shown())
        assertFalse(session.locked.value)
    }

    // decision: PHN-640, PHN-641, PHN-225
    @Test
    fun a_reconnect_while_open_neither_locks_nor_asks() {
        val vm = vm()
        vm.unlockApp()
        idle()
        assertEquals(1, shown())
        // the link drops and comes back; the first read on the new connection races the phone's own proof
        phone.syncScript.add(Outcome.Failed(FerbException.NeedsUnlock("locked", "locked")))
        phone.link.value = FakePhone.status(LinkPhase.CONNECTING)
        phone.link.value = FakePhone.status()
        idle()
        assertFalse(session.locked.value)
        assertEquals("nothing asked again", 1, shown())
        assertFalse(vm.ui.value.needsMacUnlock)
    }

    // decision: PHN-640, PHN-641, PHN-13
    @Test
    fun the_mac_locked_after_the_open_is_unlocked_once_by_itself_then_waits_for_a_tap() {
        val vm = vm()
        vm.windowReady(true)
        vm.unlockApp()
        idle()
        assertEquals(1, shown())
        repeat(3) { phone.syncScript.add(Outcome.Failed(FerbException.NeedsUnlock("locked", "locked"))) }
        phone.link.value = FakePhone.status(LinkPhase.CONNECTING)
        phone.link.value = FakePhone.status()
        idle()
        assertEquals("one automatic unlock", 2, shown())
        assertFalse(vm.ui.value.needsMacUnlock)
        repeat(3) { phone.syncScript.add(Outcome.Failed(FerbException.NeedsUnlock("locked", "locked"))) }
        phone.link.value = FakePhone.status(LinkPhase.CONNECTING)
        phone.link.value = FakePhone.status()
        idle()
        assertEquals("never a loop", 2, shown())
        assertTrue("then the line he can tap", vm.ui.value.needsMacUnlock)
    }

    // decision: PHN-401, PHN-476
    @Test
    fun a_message_with_the_mac_off_waits_in_the_outbox_and_can_be_cancelled() {
        session.unlocked()
        phone.view = view()
        val vm = vm()
        vm.typed("Water the plants at 6")
        vm.send()
        assertEquals("", vm.draft.value)
        val row = vm.ui.value.rows.last() as com.ferb.app.model.ChatRow.Outbox
        assertEquals("waiting", row.item.state)
        vm.cancelOutbox(row.item.id)
        assertTrue(vm.ui.value.rows.none { it is com.ferb.app.model.ChatRow.Outbox })
        phone.sendWaitingFor = "unlock"
        repeat(3) { phone.syncScript.add(Outcome.Failed(FerbException.NeedsUnlock("locked", "locked"))) }
        vm.typed("And the balcony")
        vm.send()
        assertTrue("a message waiting for the unlock says so", vm.ui.value.needsMacUnlock)
    }

    // decision: PHN-415, PHN-302
    @Test
    fun a_question_slides_up_once_and_goes_when_answered_elsewhere() {
        session.unlocked()
        session.foreground(true)
        phone.view = view(question("ap-1"))
        val vm = vm()
        assertEquals(Sheet.Asking("ap-1"), vm.sheet.value)
        vm.show(null)
        vm.refresh()
        assertNull("shown once: not again by itself", vm.sheet.value)
        vm.show(Sheet.Asking("ap-1"))
        phone.view = view(question("ap-1", open = false))
        vm.refresh()
        assertNull(vm.sheet.value)
        assertEquals(R.string.question_gone, vm.ui.value.note?.res)
    }

    // decision: PHN-125
    @Test
    fun stop_needs_no_unlock() {
        session.unlocked()
        val vm = vm()
        vm.stop("t-9")
        assertEquals(listOf("stop:t-9"), phone.calls.filter { it.startsWith("stop") })
        assertTrue("no prompt for Stop", signers.isEmpty())
    }

    // decision: PHN-442, PHN-302
    @Test
    fun pairing_problems_are_said_in_plain_words() {
        session.unlocked()
        phone.link.value = FakePhone.status(LinkPhase.IDLE, paired = false)
        val vm = vm()
        vm.pair("FERB-ABC")
        assertTrue(vm.ui.value.pairing)
        phone.link.value = FakePhone.status(LinkPhase.STOPPED, paired = false, problem = LinkProblem("pair_window_closed", "window closed", null))
        assertFalse(vm.ui.value.pairing)
        assertEquals(R.string.pair_expired, vm.ui.value.pairError?.res)
        assertEquals(R.string.pair_limit, FerbViewModel.pairNote("device_limit", null).res)
        assertEquals(R.string.pair_wrong_code, FerbViewModel.pairNote("pair_secret_wrong", null).res)
    }

    // decision: PHN-133, PHN-497
    @Test
    fun a_mac_that_removed_this_phone_or_lost_keys_asks_to_pair_again() {
        session.unlocked()
        val vm = vm()
        phone.link.value = FakePhone.status(LinkPhase.STOPPED, paired = true, problem = LinkProblem("device_revoked", "removed", null))
        assertEquals(Gate.REMOVED, vm.ui.value.gate)
        assertEquals(PairReason.REMOVED, vm.ui.value.pairReason)
        phone.link.value = FakePhone.status()
        phone.state.value = PhoneState.Open(KeyHealth(KeyState.OK, KeyState.GONE, KeyState.GONE))
        assertEquals(Gate.REMOVED, vm.ui.value.gate)
        assertEquals(PairReason.KEYS, vm.ui.value.pairReason)
    }

    // decision: PHN-302
    @Test
    fun errors_are_said_in_the_apps_words_or_the_macs_own() {
        assertEquals(R.string.err_offline, FerbViewModel.errorNote(FerbException.Offline("no link", null)).res)
        assertEquals(R.string.err_needs_better_path, FerbViewModel.errorNote(FerbException.NeedsBetterPath("bulk", "needs_direct_or_own_relay")).res)
        val mac = FerbViewModel.errorNote(FerbException.Refused("That question isn't waiting any more, so nothing was sent.", "question_not_waiting"))
        assertEquals(R.string.err_mac_said, mac.res)
        assertEquals("That question isn't waiting any more, so nothing was sent.", mac.words)
        assertEquals("offline", Outcome.Failed(FerbException.Offline("x", null)).code)
    }
}
