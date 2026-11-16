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
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
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
    private lateinit var session: Session
    private var screenLock = true
    private val signers = ArrayList<FakeSigner>()
    private var localPrompt = true
    private var cancelPrompt = false
    private val whys = ArrayList<Why>()
    private val root: File = Files.createTempDirectory("ferb-app-files").toFile()
    private val picked = HashMap<String, ByteArray>()
    private val local = LocalFiles(root) { uri -> picked[uri]?.let { b -> Picked(uri.substringAfterLast('/'), b.size.toLong()) { b.inputStream() } } }

    @Before
    fun setUp() {
        Dispatchers.setMain(UnconfinedTestDispatcher())
        session = Session { t }
    }

    @After
    fun tearDown() {
        Dispatchers.resetMain()
        root.deleteRecursively()
    }

    private fun vm(): FerbViewModel {
        val prefs = Prefs(ApplicationProvider.getApplicationContext())
        return FerbViewModel(phone, session, prefs, { screenLock }, null, SavedStateHandle(), local).also {
            it.prompts = object : Prompts {
                override fun signer(kind: KeyKind, why: Why) = FakeSigner(kind, cancelPrompt).also { s -> signers.add(s); whys.add(why) }
                override suspend fun local() = localPrompt
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
