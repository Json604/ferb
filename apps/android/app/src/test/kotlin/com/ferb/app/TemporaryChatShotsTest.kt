package com.ferb.app

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.ChatRow
import com.ferb.app.model.LocalFiles
import com.ferb.app.ui.Host
import com.ferb.app.ui.Main
import com.ferb.core.KeyHealth
import com.ferb.core.KeyState
import com.ferb.design.components.PhoneTab
import com.ferb.design.preview.labMoment
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.ferb.mobile.OutboxItem
import com.github.takahirom.roborazzi.captureRoboImage
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.io.File

/**
 * The temporary chat drawn by the chat's own screen (APP-1126, PHN-837), at the smallest common Android width (360 dp)
 * and the lab's (412 dp): the banner, its turns, and his message on its way as the chat draws his outbox ("Sending…").
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
class TemporaryChatShotsTest {
    @get:Rule val compose = createComposeRule()

    private val host = object : Host {
        override fun stayConnected(on: Boolean) {}
        override fun blockScreenshots(on: Boolean) {}
        override fun askNotifications() {}
        override fun pickFiles(forChat: Boolean) {}
        override fun handOut(key: String, file: File, name: String, share: Boolean) {}
    }

    private fun shoot(name: String) {
        val vm = FerbViewModel(FakePhone(), Session { 0L }, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
            LocalFiles(java.nio.file.Files.createTempDirectory("ferb-temporary-shots").toFile()) { null }, account = testAccount())
        val rows = listOf(
            ChatRow.Owner("a", "What's a good name for a grey cat?", emptyList(), null),
            ChatRow.Ferb("b", "Smokey, Ash or Pebble. Pebble suits a small one."),
            ChatRow.Outbox("o:temporary-1", OutboxItem("temporary-1", "And for a black one?", "2026-11-24T10:00:00Z", "sending", null, null, false, emptyList())),
        )
        val ui = UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK),
            temporary = TemporaryUi("T1", rows))
        compose.setContent {
            Box(Modifier.fillMaxSize().testTag("phone")) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false) {
                    Main(vm, ui, PhoneTab.CHAT, home = false, PhoneInsets.Lab, labMoment(12.5), host)
                }
            }
        }
        compose.mainClock.autoAdvance = false
        repeat(40) { compose.mainClock.advanceTimeByFrame() }
        compose.onNodeWithTag("phone").captureRoboImage("src/test/screenshots/$name", roborazziOptions = ScreenShotsTest.options)
    }

    // decision: APP-1126, PHN-837
    @Test @Config(qualifiers = "w360dp-h780dp-port-mdpi") fun his_message_on_its_way_at_360() = shoot("chat-temporary-sending-360.png")

    // decision: APP-1126, PHN-837
    @Test @Config(qualifiers = "w412dp-h915dp-port-mdpi") fun his_message_on_its_way_at_412() = shoot("chat-temporary-sending-412.png")
}
