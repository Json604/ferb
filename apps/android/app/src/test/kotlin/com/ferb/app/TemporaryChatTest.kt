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
        LocalFiles(root) { null }, say = { "" })

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

    // decision: PHN-818
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

    // decision: PHN-818
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

    // decision: PHN-818
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

    // decision: PHN-818
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

    // decision: PHN-818
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

    // decision: PHN-818
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
}
