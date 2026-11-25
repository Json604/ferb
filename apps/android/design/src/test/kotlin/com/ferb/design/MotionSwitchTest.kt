package com.ferb.design

import android.provider.Settings
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.assertIsSelected
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onAllNodesWithContentDescription
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.click
import androidx.compose.ui.test.swipeDown
import androidx.compose.ui.unit.dp
import androidx.compose.foundation.text.BasicText
import androidx.test.core.app.ApplicationProvider
import com.ferb.design.components.PhoneTab
import com.ferb.design.components.Sheet
import com.ferb.design.components.TabBarHost
import com.ferb.design.components.TopBar
import com.ferb.design.components.pressable
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * The one motion system (MotionHosts.kt) as the owner meets it: what comes in moves and takes touches at once, what goes
 * never acts, the lock replaces everything in one frame, a sheet comes up and a drag down closes it, the tab bar is one
 * bar whose pill follows the chosen tab, a press sinks at touch-down, and with the system's animations off nothing moves.
 */
@RunWith(RobolectricTestRunner::class)
@Config(qualifiers = "w400dp-h800dp-port-mdpi")
class MotionSwitchTest {
    @get:Rule val compose = createComposeRule()

    private val resolver get() = ApplicationProvider.getApplicationContext<android.app.Application>().contentResolver

    @After
    fun animationsBackOn() {
        Settings.Global.putFloat(resolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f)
    }

    private fun top(tag: String) = compose.onNodeWithTag(tag).fetchSemanticsNode().boundsInRoot.top

    /** Frames with the clock held: a state written by the test (or a touch) is applied first, as a phone's frame would. */
    private fun frames(n: Int) = repeat(n) {
        androidx.compose.runtime.snapshots.Snapshot.sendApplyNotifications()
        compose.mainClock.advanceTimeByFrame()
    }

    // decision: PHN-623, PHN-624
    @Test
    fun a_new_page_rises_in_and_takes_touches_at_once_while_the_old_one_takes_none() {
        var target by mutableStateOf("a")
        var aTaps = 0
        var bTaps = 0
        compose.setContent {
            MotionSwitch(target, Modifier.size(300.dp)) { k ->
                if (k == "a") Box(Modifier.size(300.dp).testTag("a").clickable { aTaps++ })
                else Box(Modifier.padding(top = 150.dp).size(300.dp, 150.dp).testTag("b").clickable { bTaps++ })
            }
        }
        compose.mainClock.autoAdvance = false
        target = "b"
        frames(2)
        assertTrue("the new page is still rising", top("b") > 150f)
        compose.onNodeWithTag("a").assertExists()
        // the old page is still drawn, fading: a tap where only it is does nothing
        compose.onRoot().performTouchInput { click(Offset(150f, 50f)) }
        assertEquals(0, aTaps)
        // the new one takes a tap mid-way
        compose.onRoot().performTouchInput { click(Offset(150f, 280f)) }
        assertEquals(1, bTaps)
        compose.mainClock.advanceTimeBy(1_000)
        compose.onNodeWithTag("a").assertDoesNotExist()
        assertEquals(150f, top("b"), 0.5f)
    }

    // decision: PHN-623, PHN-835
    @Test
    fun the_lock_replaces_everything_in_the_same_frame() {
        var target by mutableStateOf("main")
        compose.setContent {
            MotionSwitch(target, Modifier.size(300.dp), instant = { it == "lock" }) { k -> Box(Modifier.size(100.dp).testTag(k)) }
        }
        compose.mainClock.autoAdvance = false
        target = "lock"
        frames(1)
        compose.onNodeWithTag("main").assertDoesNotExist()
        assertEquals(0f, top("lock"), 0.01f)
    }

    // decision: PHN-624
    @Test
    fun with_the_systems_animations_off_every_change_is_instant() {
        Settings.Global.putFloat(resolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
        var target by mutableStateOf("a")
        compose.setContent { MotionSwitch(target, Modifier.size(300.dp)) { k -> Box(Modifier.size(100.dp).testTag(k)) } }
        compose.mainClock.autoAdvance = false
        target = "b"
        frames(1)
        compose.onNodeWithTag("a").assertDoesNotExist()
        assertEquals(0f, top("b"), 0.01f)
    }

    // decision: PHN-623
    @Test
    fun a_going_sheet_shows_what_it_showed() {
        var target by mutableStateOf<String?>("q")
        var words by mutableStateOf("Run this in Terminal?")
        compose.setContent {
            MotionSwitch(target, Modifier.fillMaxSize(), MotionKind.SHEET) {
                Sheet(onDismiss = {}) { BasicText(heldWhileLeaving(words), Modifier.testTag("words")) }
            }
        }
        compose.mainClock.autoAdvance = false
        target = null
        words = ""
        frames(2)
        compose.onNodeWithTag("words").assertExists()
        assertEquals("Run this in Terminal?", compose.onNodeWithTag("words").fetchSemanticsNode().config[androidx.compose.ui.semantics.SemanticsProperties.Text].first().text)
        compose.mainClock.advanceTimeBy(1_000)
        compose.onNodeWithTag("words").assertDoesNotExist()
    }

    // decision: PHN-623
    @Test
    fun a_sheet_comes_up_from_below_and_a_drag_down_closes_it_while_a_short_one_springs_back() {
        var target by mutableStateOf<String?>(null)
        var dismissed = 0
        var pageTaps = 0
        compose.setContent {
            Box(Modifier.fillMaxSize().clickable { pageTaps++ })
            MotionSwitch(target, Modifier.fillMaxSize(), MotionKind.SHEET) {
                Sheet(onDismiss = { dismissed++; target = null }) { Box(Modifier.size(200.dp).testTag("panel").background(Color.Gray)) }
            }
        }
        compose.mainClock.autoAdvance = false
        target = "s"
        frames(2)
        val moving = top("panel")
        compose.mainClock.advanceTimeBy(1_000)
        val rest = top("panel")
        assertTrue("it came up from below: $moving then $rest", moving > rest + 50)
        // a short drag springs back
        compose.onNodeWithTag("panel").performTouchInput { swipeDown(startY = 10f, endY = 40f, durationMillis = 300) }
        compose.mainClock.advanceTimeBy(1_000)
        assertEquals(0, dismissed)
        assertEquals(rest, top("panel"), 0.5f)
        // a long one closes it; on its way down the page under it takes a tap
        compose.onNodeWithTag("panel").performTouchInput { swipeDown(startY = 10f, endY = 190f, durationMillis = 300) }
        assertEquals(1, dismissed)
        frames(1)
        compose.onRoot().performTouchInput { click(Offset(200f, 50f)) }
        assertEquals(1, pageTaps)
        compose.mainClock.advanceTimeBy(1_000)
        compose.onNodeWithTag("panel").assertDoesNotExist()
    }

    // decision: PHN-622
    @Test
    fun the_tab_bar_is_one_bar_over_the_pages_and_follows_the_chosen_tab() {
        var tab by mutableStateOf(PhoneTab.CHAT)
        compose.setContent {
            TabBarHost(Modifier.fillMaxSize()) {
                MotionSwitch(tab, Modifier.fillMaxSize(), MotionKind.TAB, direction = { a, b -> b.ordinal.compareTo(a.ordinal) }) { t ->
                    Column(Modifier.fillMaxSize()) { TopBar(t, waiting = null, onTab = { tab = it }, onProfile = {}) }
                }
            }
        }
        compose.onAllNodesWithContentDescription("Files").assertCountEquals(1)
        compose.onNodeWithContentDescription("Files").performTouchInput { click() }
        compose.mainClock.advanceTimeBy(1_000)
        assertEquals(PhoneTab.FILES, tab)
        compose.onAllNodesWithContentDescription("Files").assertCountEquals(1)
        compose.onNodeWithContentDescription("Files").assertIsSelected()
    }

    // decision: PHN-624
    @Test
    fun a_press_sinks_at_touch_down_even_inside_a_scrolling_list() {
        compose.setContent {
            Column(Modifier.size(300.dp).verticalScroll(rememberScrollState())) {
                Box(Modifier.size(200.dp, 100.dp).pressable({}).testTag("row"))
                Box(Modifier.size(200.dp, 900.dp))
            }
        }
        val rest = compose.onNodeWithTag("row").fetchSemanticsNode().boundsInRoot.width
        compose.mainClock.autoAdvance = false
        compose.onNodeWithTag("row").performTouchInput { down(center) }
        frames(3) // 48 ms: Compose's own press would still wait for its tap timeout here
        val down = compose.onNodeWithTag("row").fetchSemanticsNode().boundsInRoot.width
        assertTrue("sunk at touch-down: $rest then $down", down < rest - 1)
        compose.onNodeWithTag("row").performTouchInput { up() }
        compose.mainClock.advanceTimeBy(1_000)
        assertEquals(rest, compose.onNodeWithTag("row").fetchSemanticsNode().boundsInRoot.width, 0.5f)
    }

    private fun androidx.compose.ui.test.SemanticsNodeInteractionCollection.assertCountEquals(n: Int) =
        assertEquals(n, fetchSemanticsNodes().size)
}
