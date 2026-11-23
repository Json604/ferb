package com.ferb.app

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.unit.dp
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.ChatRow
import com.ferb.app.model.LocalFiles
import com.ferb.app.ui.Host
import com.ferb.app.ui.Main
import com.ferb.core.KeyHealth
import com.ferb.core.KeyState
import com.ferb.design.Phone
import com.ferb.design.components.Connection
import com.ferb.design.components.LinkMark
import com.ferb.design.components.LocalConnection
import com.ferb.design.components.PhoneTab
import com.ferb.design.preview.labMoment
import com.ferb.design.screens.LabAndroidChrome
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.github.takahirom.roborazzi.captureRoboImage
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.io.File

/**
 * The top bar folded where there is no room (owner 2026-11-23, PHN-820): on a narrow phone the chat tab's temporary
 * bubble and the profile button become one menu button with three lines; while the temporary chat is on the button
 * carries the bubble's mark; its menu offers the temporary chat and this phone. Where everything fits, the bar is as
 * before (360 and the 412 goldens).
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
class TopBarFoldTest {
    @get:Rule val compose = createComposeRule()

    private val phone = FakePhone()
    private val session = Session { 0L }
    private val vm by lazy {
        FerbViewModel(phone, session, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
            LocalFiles(java.nio.file.Files.createTempDirectory("ferb-fold").toFile()) { null })
    }
    private val host = object : Host {
        override fun stayConnected(on: Boolean) {}
        override fun blockScreenshots(on: Boolean) {}
        override fun askNotifications() {}
        override fun pickFiles(forChat: Boolean) {}
        override fun handOut(key: String, file: File, name: String, share: Boolean) {}
    }
    private val rows = listOf(
        ChatRow.Owner("a", "What's left on my calendar today?", emptyList(), null),
        ChatRow.Ferb("b", "Two things: the design review at 4, and a call with the bank at 6:30."),
    )
    private val temp = listOf(
        ChatRow.Owner("t1", "Give me three names for a houseplant shop", emptyList(), null),
        ChatRow.Ferb("t2", "Leaf & Co, Rootbound, and The Green Room."),
    )
    private val base = UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK), rows = rows)

    private fun frame(width: Int, ui: UiState) {
        compose.setContent {
            CompositionLocalProvider(LocalConnection provides Connection(LinkMark.DIRECT, "Your Mac: Connected", {})) {
                Box(Modifier.size(width.dp, Phone.frameHeight).testTag("phone")) {
                    PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false, sheet = { LabAndroidChrome() }) {
                        Main(vm, ui, PhoneTab.CHAT, home = false, PhoneInsets.Lab, labMoment(12.5), host)
                    }
                }
            }
        }
        compose.waitForIdle()
    }

    private fun shoot(name: String) {
        compose.mainClock.autoAdvance = false
        repeat(40) { compose.mainClock.advanceTimeByFrame() }
        compose.onNodeWithTag("phone").captureRoboImage("src/test/screenshots/topbar/$name", roborazziOptions = ScreenShotsTest.options)
    }

    private fun shots(width: Int) {
        frame(width, base)
        compose.onNodeWithTag("topbar-menu").assertIsDisplayed()
        compose.onNodeWithTag("temporary-chat").assertDoesNotExist()
        shoot("folded-$width.png")
    }

    private fun shotsOn(width: Int) {
        frame(width, base.copy(temporary = TemporaryUi("T1", temp)))
        compose.onNodeWithTag("topbar-menu-temporary", useUnmergedTree = true).assertIsDisplayed()
        shoot("folded-on-$width.png")
    }

    private fun shotsMenu(width: Int) {
        frame(width, base.copy(temporary = TemporaryUi("T1", temp)))
        compose.onNodeWithTag("topbar-menu").performClick()
        compose.waitForIdle()
        compose.onNodeWithTag("topbar-menu-panel").assertIsDisplayed()
        shoot("menu-$width.png")
    }

    // decision: PHN-820
    @Test @Config(qualifiers = "w320dp-h915dp-port-mdpi") fun folded_at_320() = shots(320)

    // decision: PHN-820
    @Test @Config(qualifiers = "w320dp-h915dp-port-mdpi") fun folded_with_the_temporary_chat_on_at_320() = shotsOn(320)

    // decision: PHN-820
    @Test @Config(qualifiers = "w320dp-h915dp-port-mdpi") fun the_open_menu_at_320() = shotsMenu(320)

    /** Where it fits, nothing folds: the bubble and the profile button as built. */
    // decision: PHN-820
    @Test @Config(qualifiers = "w412dp-h915dp-port-mdpi")
    fun at_412_nothing_folds() = nothingFolds(412)

    /** The four tabs leave room at the smallest common width too: nothing folds at 360. */
    // decision: PHN-820
    @Test @Config(qualifiers = "w360dp-h915dp-port-mdpi")
    fun at_360_nothing_folds() = nothingFolds(360)

    private fun nothingFolds(width: Int) {
        frame(width, base)
        compose.onNodeWithTag("temporary-chat").assertIsDisplayed()
        compose.onNodeWithTag("topbar-menu").assertDoesNotExist()
    }

    /** The menu's rows do what the two buttons did: the temporary chat on and off, the profile page; a tap outside closes it. */
    // decision: PHN-820
    @Test @Config(qualifiers = "w320dp-h915dp-port-mdpi")
    fun the_menu_turns_the_temporary_chat_on_and_opens_the_profile() {
        session.unlocked()
        compose.setContent {
            val ui by vm.ui.collectAsState()
            Box(Modifier.size(320.dp, Phone.frameHeight)) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false) {
                    Main(vm, ui.copy(gate = Gate.MAIN), PhoneTab.CHAT, home = false, PhoneInsets.Lab, labMoment(12.5), host)
                }
            }
        }
        compose.waitForIdle()
        compose.onNodeWithContentDescription("More").performClick()
        compose.onNodeWithText("Temporary chat").performClick()
        compose.waitForIdle()
        assertTrue(phone.calls.contains("temporary:true"))
        compose.onNodeWithTag("topbar-menu-panel").assertDoesNotExist()
        compose.onNodeWithTag("topbar-menu-temporary", useUnmergedTree = true).assertIsDisplayed()
        compose.onNodeWithTag("topbar-menu").performClick()
        compose.onNodeWithText("End temporary chat").assertIsDisplayed()
        compose.onNodeWithTag("topbar-menu-profile-row").performClick()
        compose.waitForIdle()
        assertTrue("the profile page", vm.sheet.value == Sheet.Profile)
        compose.onNodeWithTag("topbar-menu").performClick()
        compose.onNodeWithTag("topbar-menu-scrim").performClick()
        compose.onNodeWithTag("topbar-menu-panel").assertDoesNotExist()
    }
}
