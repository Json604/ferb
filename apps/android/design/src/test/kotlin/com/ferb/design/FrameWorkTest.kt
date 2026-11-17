package com.ferb.design

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composer
import androidx.compose.runtime.CompositionTracer
import androidx.compose.runtime.InternalComposeTracingApi
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.Snapshot
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.unit.dp
import com.ferb.design.components.PhoneSwitch
import com.ferb.design.components.PhoneText
import com.ferb.design.components.pressable
import com.ferb.design.preview.labMoment
import com.ferb.design.sky.Sky
import java.util.concurrent.ConcurrentHashMap
import org.junit.After
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * What moves draws itself and recomposes nothing (kb PHN-711): the sky's glide, a press and a switch read their
 * animated values while drawing or placing, never in composition. Counted by the compiler's trace markers: each
 * composable body that runs once more is one recomposition of it. (A test clock cancels endless animations, so the
 * turning mark and FERB's typing dots are measured on a phone: the app's FrameBudgetOnPhoneTest.)
 */
@RunWith(RobolectricTestRunner::class)
@Config(qualifiers = "w400dp-h800dp-port-mdpi")
class FrameWorkTest {
    @get:Rule val compose = createComposeRule()

    @Before fun count() = Runs.install()

    @After fun stop() = Runs.uninstall()

    private fun frames(n: Int) = repeat(n) { compose.mainClock.advanceTimeByFrame() }

    // decision: PHN-711
    @Test
    fun the_sky_glides_without_recomposing() {
        var moment by mutableStateOf(labMoment(6.0))
        compose.setContent { Sky(moment, Modifier.size(400.dp, 800.dp)) }
        compose.waitForIdle()
        compose.mainClock.autoAdvance = false
        Runs.reset()
        // a new minute after a long gap: the sun glides for 2.2 s
        moment = labMoment(7.0)
        Snapshot.sendApplyNotifications()
        frames(150)
        assertTrue("the sky recomposed ${Runs.of("com.ferb.design.sky.Sky")} times while it glided", Runs.of("com.ferb.design.sky.Sky") <= 2)
    }

    // decision: PHN-711
    @Test
    fun a_press_and_a_switch_recompose_only_when_they_change() {
        var on by mutableStateOf(false)
        compose.setContent {
            Box {
                PhoneSwitch(on, onChange = { on = it })
                Box(Modifier.testTag("press").size(80.dp).pressable({})) { PhoneText("Press", 15f) }
            }
        }
        compose.waitForIdle()
        compose.mainClock.autoAdvance = false
        Runs.reset()
        on = true
        Snapshot.sendApplyNotifications()
        frames(30)
        assertTrue("the switch recomposed ${Runs.of("com.ferb.design.components.PhoneSwitch")} times as its knob slid", Runs.of("com.ferb.design.components.PhoneSwitch") <= 1)
        Runs.reset()
        compose.onNodeWithTag("press").performTouchInput { down(center) }
        frames(30)
        compose.onNodeWithTag("press").performTouchInput { up() }
        frames(40)
        // the press itself recomposes its holder twice (down, up), never once per frame of the sink and the rise
        assertTrue("a press recomposed its holder ${Runs.of("com.ferb.design.components.pressable")} times", Runs.of("com.ferb.design.components.pressable") <= 4)
    }
}

/** Counts the composable bodies that run, by function (the compiler's trace markers). */
@OptIn(InternalComposeTracingApi::class)
internal object Runs : CompositionTracer {
    private val counts = ConcurrentHashMap<String, Int>()
    override fun isTraceInProgress() = true
    override fun traceEventStart(key: Int, dirty1: Int, dirty2: Int, info: String) {
        counts.merge(info.substringBefore(" ("), 1, Int::plus)
    }
    override fun traceEventEnd() {}
    fun install() { counts.clear(); Composer.setTracer(this) }
    fun uninstall() = Composer.setTracer(Off)
    fun reset() = counts.clear()
    fun of(name: String) = counts[name] ?: 0

    private object Off : CompositionTracer {
        override fun isTraceInProgress() = false
        override fun traceEventStart(key: Int, dirty1: Int, dirty2: Int, info: String) {}
        override fun traceEventEnd() {}
    }
}
