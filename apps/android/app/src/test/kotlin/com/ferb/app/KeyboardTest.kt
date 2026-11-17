package com.ferb.app

import android.app.Application
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performScrollToIndex
import androidx.compose.ui.unit.dp
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
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.io.File
import java.time.LocalDate
import java.time.ZoneId

/**
 * The chat while the keyboard moves (kb PHN-670): the owner saw the list draw nothing for as long as the keyboard
 * slid in or out. Every frame of a keyboard's slide (the bottom inset growing or shrinking, as WindowInsets.ime does)
 * must show the chat's rows at full strength, with its end against the message box when he was at the end, and the
 * place he reads unmoved when he was not. A row is FERB's black bubble: a frame with almost none of its black is blank.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(application = Application::class, sdk = [35], qualifiers = "w412dp-h915dp-port-mdpi")
class KeyboardTest {
    @get:Rule val compose = createAndroidComposeRule<androidx.activity.ComponentActivity>()

    private val phone = FakePhone()
    private val zone = ZoneId.systemDefault()
    private val today = LocalDate.now(zone)
    private val host = object : Host {
        override fun stayConnected(on: Boolean) {}
        override fun blockScreenshots(on: Boolean) {}
        override fun askNotifications() {}
        override fun pickFiles(forChat: Boolean) {}
        override fun handOut(key: String, file: File, name: String, share: Boolean) {}
    }

    private var bottom by mutableStateOf(Phone.gestureArea)
    private var ui by mutableStateOf(UiState())

    private fun chat(rows: List<ChatRow>, days: Map<String, Long> = emptyMap()) {
        ui = UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK),
            conversation = "c1", rows = rows, days = days)
        val vm = FerbViewModel(phone, Session { 0L }, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
            LocalFiles(java.nio.file.Files.createTempDirectory("ferb-kbd").toFile()) { null })
        compose.setContent {
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight)) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false) {
                    Main(vm, ui, PhoneTab.CHAT, home = false, PhoneInsets(Phone.statusBar, bottom), labMoment(12.5), host)
                }
            }
        }
    }

    private fun lines(n: Int) = (1..n).map { ChatRow.Ferb("f$it", "FERB's line number $it, long enough to fill a bubble.") }

    /**
     * How much of the list is FERB's black: a share of its pixels (the noon sky has none). Drawn from the window as it
     * stands on this frame (a capture that waits for the chat to be idle would skip the frames that matter).
     */
    private fun black(): Double {
        val r = compose.onNodeWithTag("chat-list").fetchSemanticsNode().boundsInWindow
        val view = compose.activity.window.decorView
        val bmp = android.graphics.Bitmap.createBitmap(view.width, view.height, android.graphics.Bitmap.Config.ARGB_8888)
        view.draw(android.graphics.Canvas(bmp))
        var dark = 0
        var all = 0
        for (y in r.top.toInt() until r.bottom.toInt().coerceAtMost(bmp.height) step 2) for (x in r.left.toInt() until r.right.toInt().coerceAtMost(bmp.width) step 2) {
            val c = bmp.getPixel(x, y)
            all++
            if (android.graphics.Color.red(c) < 20 && android.graphics.Color.green(c) < 20 && android.graphics.Color.blue(c) < 20) dark++
        }
        return dark.toDouble() / all.coerceAtLeast(1)
    }

    /** The keyboard slides in over [frames] frames to [to] (dp above the bottom), then out again, one frame at a time. */
    private fun slide(frames: Int, to: Float, check: (String) -> Unit) {
        val from = Phone.gestureArea.value
        for (i in 1..frames) {
            bottom = (from + (to - from) * i / frames).dp
            compose.mainClock.advanceTimeByFrame()
            check("opening, frame $i")
        }
        repeat(20) { compose.mainClock.advanceTimeByFrame() }
        for (i in 1..frames) {
            bottom = (to + (from - to) * i / frames).dp
            compose.mainClock.advanceTimeByFrame()
            check("closing, frame $i")
        }
    }

    // decision: PHN-670, PHN-671
    @Test
    fun at_the_end_every_frame_of_the_keyboard_shows_the_rows_and_the_end_stays_in_view() {
        chat(lines(14))
        compose.waitForIdle()
        compose.mainClock.autoAdvance = false
        repeat(30) { compose.mainClock.advanceTimeByFrame() }
        val rest = black()
        assertTrue("the chat is drawn before the keyboard: $rest", rest > 0.15)
        slide(frames = 12, to = 330f) { at ->
            val b = black()
            assertTrue("$at: the rows are drawn at full strength ($b of the list is FERB's black, at rest $rest)", b > rest * 0.6)
        }
        repeat(30) { compose.mainClock.advanceTimeByFrame() }
        // open, settled: the last line sits just above the message box
        bottom = 330.dp
        repeat(40) { compose.mainClock.advanceTimeByFrame() }
        val last = compose.onNodeWithText("FERB's line number 14", substring = true).fetchSemanticsNode().boundsInRoot
        val list = compose.onNodeWithTag("chat-list").fetchSemanticsNode().boundsInRoot
        assertTrue("the end against the message box: ${last.bottom} within ${list.bottom}", last.bottom <= list.bottom + 1 && last.bottom > list.bottom - 60)
    }

    // decision: PHN-670, PHN-671
    @Test
    fun scrolled_up_the_keyboard_neither_blanks_nor_moves_what_he_reads() {
        chat(lines(40))
        compose.waitForIdle()
        compose.onNodeWithTag("chat-list").performScrollToIndex(10)
        compose.waitForIdle()
        compose.mainClock.autoAdvance = false
        repeat(30) { compose.mainClock.advanceTimeByFrame() }
        val top = compose.onNodeWithText("FERB's line number 11", substring = true).fetchSemanticsNode().boundsInRoot.top
        val rest = black()
        slide(frames = 12, to = 330f) { at ->
            val b = black()
            assertTrue("$at: drawn ($b, at rest $rest)", b > rest * 0.5)
            val now = compose.onNodeWithText("FERB's line number 11", substring = true).fetchSemanticsNode().boundsInRoot.top
            assertTrue("$at: the row he reads stays put ($top -> $now)", kotlin.math.abs(now - top) < 2)
        }
    }

    // decision: PHN-670, PHN-671
    @Test
    fun the_first_draw_and_a_message_box_that_grows_show_every_row_at_once() {
        compose.mainClock.autoAdvance = false
        chat(lines(14))
        compose.mainClock.advanceTimeByFrame()
        compose.mainClock.advanceTimeByFrame()
        val first = black()
        repeat(30) { compose.mainClock.advanceTimeByFrame() }
        val rest = black()
        assertTrue("the first frames show the rows, not a fade from nothing ($first, at rest $rest)", first > rest * 0.8)
        // the message box grows by a line, and a status line pushes the list: no frame without the rows
        for (i in 1..6) {
            bottom = (Phone.gestureArea.value + i * 4).dp
            compose.mainClock.advanceTimeByFrame()
            assertTrue("growing, frame $i", black() > rest * 0.6)
        }
        ui = ui.copy(macLine = MacLine.SLOW)
        repeat(12) {
            compose.mainClock.advanceTimeByFrame()
            assertTrue("the status line comes, frame $it", black() > rest * 0.6)
        }
    }
}
