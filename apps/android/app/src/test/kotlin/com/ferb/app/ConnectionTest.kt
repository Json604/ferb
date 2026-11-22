package com.ferb.app

import android.app.Application
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
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
import com.ferb.app.ui.Sheets
import com.ferb.app.ui.linkMark
import com.ferb.app.ui.rememberConnection
import com.ferb.core.KeyHealth
import com.ferb.core.KeyState
import com.ferb.core.PhoneState
import com.ferb.design.Phone
import com.ferb.design.components.LinkMark
import com.ferb.design.components.LocalConnection
import com.ferb.design.components.PhoneTab
import com.ferb.design.preview.labMoment
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.ferb.mobile.LinkPath
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.OutboxItem
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestCoroutineScheduler
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * The connection control and "can't reach" (kb PHN-751..PHN-754): the owner saw the amber "Can't reach your Mac" for
 * several seconds at every open while the link was only connecting. On the fake phone with a test clock: connecting
 * never shows amber, however long it takes up to the point; after it, can't reach does; coming back online ends it;
 * every open counts again; the control says each state and opens its sheet; the chat's line speaks only when messages
 * of his wait.
 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35], qualifiers = "w412dp-h915dp-port-mdpi")
class ConnectionTest {
    @get:Rule val compose = createComposeRule()

    private val phone = FakePhone()
    private val scheduler = TestCoroutineScheduler()
    private lateinit var session: Session
    private val root: File = java.nio.file.Files.createTempDirectory("ferb-conn").toFile()
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
    }

    @After
    fun tearDown() {
        Dispatchers.resetMain()
        root.deleteRecursively()
    }

    private fun vm() = FerbViewModel(phone, session, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
        LocalFiles(root) { null })

    private fun open() {
        session.foreground(true)
        session.unlocked()
    }

    private fun seconds(s: Long) = scheduler.advanceTimeBy(s * 1000)

    private fun mark(vm: FerbViewModel) = linkMark(vm.ui.value.link, vm.ui.value.linkLost)

    // decision: PHN-750, PHN-758, PHN-755, PHN-757
    @Test
    fun an_open_that_connects_never_says_it_cant_reach_the_mac() {
        phone.link.value = FakePhone.status(LinkPhase.CONNECTING)
        val vm = vm()
        open()
        assertEquals(LinkMark.CONNECTING, mark(vm))
        // a dial fails and the next answers, inside the owner's 10 s: still connecting, never amber
        seconds(3)
        phone.link.value = FakePhone.status(LinkPhase.WAITING)
        seconds(3)
        phone.link.value = FakePhone.status(LinkPhase.CONNECTING)
        seconds(3)
        assertFalse(vm.ui.value.linkLost)
        assertEquals(LinkMark.CONNECTING, mark(vm))
        phone.link.value = FakePhone.status()
        scheduler.runCurrent()
        assertEquals(LinkMark.DIRECT, mark(vm))
        seconds(60)
        assertFalse(vm.ui.value.linkLost)
    }

    // decision: PHN-758, PHN-757
    @Test
    fun connecting_that_fails_past_the_point_says_it_cant_reach_and_a_reconnect_ends_it() {
        assertEquals("the owner's point (PHN-757)", 10_000L, FerbViewModel.LINK_LOST_AFTER_MS)
        phone.link.value = FakePhone.status(LinkPhase.CONNECTING)
        val vm = vm()
        open()
        phone.link.value = FakePhone.status(LinkPhase.WAITING)
        seconds(FerbViewModel.LINK_LOST_AFTER_MS / 1000 - 1)
        assertFalse("not yet", vm.ui.value.linkLost)
        seconds(2)
        assertTrue(vm.ui.value.linkLost)
        assertEquals(LinkMark.UNREACHABLE, mark(vm))
        phone.link.value = FakePhone.status(path = LinkPath.PUBLIC_RELAY)
        scheduler.runCurrent()
        assertFalse(vm.ui.value.linkLost)
        assertEquals(LinkMark.SHARED_RELAY, mark(vm))
    }

    // decision: PHN-758
    @Test
    fun every_open_counts_again_and_stopped_is_at_once() {
        phone.link.value = FakePhone.status(LinkPhase.WAITING)
        val vm = vm()
        open()
        seconds(FerbViewModel.LINK_LOST_AFTER_MS / 1000 + 1)
        assertTrue(vm.ui.value.linkLost)
        // to the background and back: the link dials again from the start, so the count starts again too
        session.foreground(false)
        scheduler.runCurrent()
        session.foreground(true)
        scheduler.runCurrent()
        assertFalse(vm.ui.value.linkLost)
        seconds(FerbViewModel.LINK_LOST_AFTER_MS / 1000 + 1)
        assertTrue(vm.ui.value.linkLost)
        phone.link.value = FakePhone.status(LinkPhase.STOPPED)
        scheduler.runCurrent()
        assertTrue("stopped for good: at once", vm.ui.value.linkLost)
    }

    // decision: PHN-801, PHN-752
    @Test
    fun the_profile_button_carries_the_link_and_its_page_opens_the_connection_sheet() {
        phone.link.value = FakePhone.status(LinkPhase.CONNECTING)
        val vm = vm()
        open()
        var ui by mutableStateOf(vm.ui.value)
        var sheet by mutableStateOf<Sheet?>(null)
        compose.setContent {
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight)) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false, sheet = { Sheets(vm, ui, sheet, PhoneInsets.Lab, host) }) {
                    CompositionLocalProvider(LocalConnection provides rememberConnection(vm, ui)) {
                        Main(vm, ui, PhoneTab.CHAT, home = true, PhoneInsets.Lab, labMoment(12.5), host)
                    }
                }
            }
        }
        // one round button: the profile's, with the link's state in what TalkBack says (no second control)
        compose.onNodeWithContentDescription("This phone. Your Mac: Connecting to your Mac…").assertExists()
        ui = ui.copy(link = FakePhone.status(path = LinkPath.OWN_RELAY))
        compose.onNodeWithContentDescription("This phone. Your Mac: Connected, Through your own relay").assertExists()
        ui = ui.copy(link = FakePhone.status(LinkPhase.WAITING), linkLost = true)
        compose.onNodeWithContentDescription("This phone. Your Mac: Can't reach your Mac").assertExists().performClick()
        compose.waitForIdle()
        assertEquals(Sheet.Profile, vm.sheet.value)
        // the profile page's first row is the connection; its sheet keeps Try again, the relay and the Mac tab
        sheet = Sheet.Profile
        compose.waitForIdle()
        compose.onNodeWithText("Can't reach your Mac").assertExists().performClick()
        compose.waitForIdle()
        assertEquals(Sheet.Connection, vm.sheet.value)
    }

    /** His message waiting in the outbox, as the chat draws it. */
    private val waiting = ChatRow.Outbox("o:1", OutboxItem("1", "Move the call to 4", "2026-01-02T10:00:00+00:00", "waiting", "link", null, false, emptyList()))

    // decision: PHN-755, PHN-756
    @Test
    fun the_chat_never_speaks_of_the_link_and_his_waiting_message_says_it_waits() {
        val vm = vm()
        var ui by mutableStateOf(UiState(gate = Gate.MAIN, link = FakePhone.status(LinkPhase.CONNECTING), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK),
            conversation = "c1", rows = listOf(waiting)))
        compose.setContent {
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight)) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false) { Main(vm, ui, PhoneTab.CHAT, home = false, PhoneInsets.Lab, labMoment(12.5), host) }
            }
        }
        for (state in listOf(
            ui,
            ui.copy(link = FakePhone.status(LinkPhase.WAITING)),
            ui.copy(link = FakePhone.status(LinkPhase.WAITING), linkLost = true),
            ui.copy(link = FakePhone.status(LinkPhase.STOPPED), linkLost = true),
            ui.copy(link = FakePhone.status(LinkPhase.WAITING), linkLost = true, rows = emptyList()),
        )) {
            ui = state
            compose.waitForIdle()
            compose.onNodeWithText("reach your Mac", substring = true).assertDoesNotExist()
            compose.onNodeWithTag("status-line").assertDoesNotExist()
            if (state.rows.isNotEmpty()) compose.onNodeWithText("Waiting for your Mac").assertExists()
        }
    }
}
