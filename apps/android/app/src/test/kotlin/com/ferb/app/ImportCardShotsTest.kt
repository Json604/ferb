package com.ferb.app

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.ChatModel
import com.ferb.app.model.ChatRow
import com.ferb.app.model.LocalFiles
import com.ferb.app.ui.Host
import com.ferb.app.ui.Main
import com.ferb.core.KeyHealth
import com.ferb.core.KeyState
import com.ferb.design.Phone
import com.ferb.design.components.PhoneTab
import com.ferb.design.preview.labMoment
import com.ferb.design.screens.LabAndroidChrome
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.ferb.mobile.ImportCard
import com.ferb.mobile.ImportRow
import com.ferb.mobile.PhoneStatus
import com.github.takahirom.roborazzi.captureRoboImage
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.io.File
import java.time.ZoneId

/**
 * The memory import's card in the real chat (kb MEM-456), goldens in app/src/test/screenshots: from the Mac's status
 * through Main to the end of today's chat, on FERB's dark surface over the sky at noon and at night; the earlier days
 * never show it.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(qualifiers = "w412dp-h1000dp-port-mdpi")
class ImportCardShotsTest {
    @get:Rule val compose = createComposeRule()

    private val phone = FakePhone()
    private val vm by lazy {
        FerbViewModel(phone, Session { 0L }, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
            LocalFiles(java.nio.file.Files.createTempDirectory("ferb-import-shots").toFile()) { null })
    }
    private val host = object : Host {
        override fun stayConnected(on: Boolean) {}
        override fun blockScreenshots(on: Boolean) {}
        override fun askNotifications() {}
        override fun pickFiles(forChat: Boolean) {}
        override fun handOut(key: String, file: File, name: String, share: Boolean) {}
    }
    private val today = ChatModel.day(System.currentTimeMillis(), ZoneId.systemDefault())

    // the Mac's own words (the daemon composes them; the phone adds none)
    private val card = ImportCard("Bringing in what Claude Code knows about you", listOf(
        ImportRow("run:r1", "claude_code", "Claude Code", "reading", "Reading what you've typed", "", 33u, 40u, "33 of 40", 221u,
            "221 things found so far", ""),
    ))
    private val status = PhoneStatus(false, true, false, emptyList(), emptyList(), emptyList(), null, memoryImport = card)

    private val rows = listOf(
        ChatRow.Owner("y1", "Compare the three standing desks", emptyList(), null),
        ChatRow.Ferb("y2:answer", "The Flexispot E7 is cheapest at $389, delivered Thursday."),
        ChatRow.Owner("t1", "Bring in what Claude Code knows about me", emptyList(), null),
        ChatRow.Ferb("t2:answer", "Thanks! I'm reading through Claude Code now and I'll tell you what I learned."),
    )
    private val days = mapOf("y1" to today - 1, "y2" to today - 1, "t1" to today, "t2" to today)
    private val ui = UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK),
        conversation = "c1", rows = rows, days = days, chatDay = today, status = status)

    @Composable
    private fun Frame(ui: UiState, hour: Double, home: Boolean = false) {
        Box(Modifier.size(Phone.frameWidth, Phone.frameHeight).testTag("phone")) {
            PhoneScreen(labMoment(hour), insets = PhoneInsets.Lab, glide = false, sheet = { LabAndroidChrome() }) {
                Main(vm, ui, PhoneTab.CHAT, home = home, PhoneInsets.Lab, labMoment(hour), host)
            }
        }
    }

    // a new day with nothing said yet: the chat tab is the home page (kb PHN-626), and the card shows there too
    private val homeUi = ui.copy(rows = rows.take(2), days = days.filterKeys { it.startsWith("y") })

    // decision: MEM-457
    @Test
    fun the_card_shows_on_the_home_page_at_noon() {
        compose.setContent { Frame(homeUi, 12.5, home = true) }
        compose.onNodeWithTag("memory-import").assertExists()
        shoot("home-import-noon.png")
    }

    // decision: MEM-457
    @Test
    fun the_card_shows_on_the_home_page_at_night() {
        compose.setContent { Frame(homeUi, 22.5, home = true) }
        compose.onNodeWithTag("memory-import").assertExists()
        shoot("home-import-night.png")
    }

    // decision: MEM-457
    @Test
    fun the_home_page_has_no_card_without_an_import() {
        compose.setContent { Frame(homeUi.copy(status = status.copy(memoryImport = null)), 12.5, home = true) }
        compose.waitForIdle()
        compose.onNodeWithTag("memory-import").assertDoesNotExist()
    }

    private fun shoot(name: String) {
        compose.mainClock.autoAdvance = false
        repeat(40) { compose.mainClock.advanceTimeByFrame() }
        compose.onNodeWithTag("phone").captureRoboImage("src/test/screenshots/$name", roborazziOptions = ScreenShotsTest.options)
    }

    // decision: MEM-456
    @Test
    fun the_card_ends_todays_chat_at_noon() {
        compose.setContent { Frame(ui, 12.5) }
        compose.onNodeWithTag("memory-import").assertExists()
        shoot("chat-import-noon.png")
    }

    // decision: MEM-456
    @Test
    fun the_card_ends_todays_chat_at_night() {
        compose.setContent { Frame(ui, 22.5) }
        compose.onNodeWithTag("memory-import").assertExists()
        shoot("chat-import-night.png")
    }

    // decision: MEM-456
    @Test
    fun without_an_import_or_in_the_earlier_days_there_is_no_card() {
        compose.setContent { Frame(ui.copy(status = status.copy(memoryImport = null)), 12.5) }
        compose.waitForIdle()
        compose.onNodeWithTag("memory-import").assertDoesNotExist()
    }

    // decision: MEM-456, PHN-696
    @Test
    fun the_earlier_days_never_show_it() {
        vm.openChat()
        compose.setContent { Frame(ui, 12.5) }
        compose.waitForIdle()
        compose.onNodeWithTag("memory-import").assertDoesNotExist()
    }
}
