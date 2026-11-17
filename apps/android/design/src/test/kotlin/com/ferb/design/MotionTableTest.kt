package com.ferb.design

import androidx.compose.animation.core.CubicBezierEasing
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.exp
import kotlin.math.sin
import kotlin.math.sqrt

/**
 * The motion tokens are one table for both phones (src/test/resources/motion/motion.json; FerbDesign's MotionTests read
 * the same file): every number Motion.kt moves with is the table's, and the table's springs are PairIt's curves.
 */
class MotionTableTest {
    private val table = MiniJson.resource("motion/motion.json") as Map<*, *>

    private fun obj(vararg path: String): Map<*, *> = path.fold(table) { m, k -> m[k] as Map<*, *> }
    private fun num(m: Map<*, *>, k: String) = (m[k] as Number).toDouble()

    private fun assertSpring(name: String, s: LabSpring) {
        val t = obj("springs", name)
        assertEquals("$name response", num(t, "response"), s.duration, 1e-6)
        assertEquals("$name damping", num(t, "damping"), 1 - s.bounce, 1e-9)
    }

    private fun assertCurve(name: String, ms: Int, easing: CubicBezierEasing?) {
        val t = obj("curves", name)
        assertEquals("$name ms", num(t, "ms").toInt(), ms)
        val b = (t["bezier"] as List<*>).map { (it as Number).toFloat() }
        if (easing != null) assertEquals("$name curve", CubicBezierEasing(b[0], b[1], b[2], b[3]), easing)
        else assertEquals("$name linear", listOf(0f, 0f, 1f, 1f), b)
    }

    // decision: PHN-620, PHN-621
    @Test
    fun everyMotionNumberIsTheSharedTables() {
        assertSpring("tab", Motion.tab)
        assertSpring("press_down", Motion.pressDown)
        assertSpring("press_up", Motion.pressUp)
        assertSpring("sheet", Motion.sheet)
        assertCurve("enter", Motion.ENTER_MS, Motion.EnterEasing)
        assertCurve("exit", Motion.EXIT_MS, null)
        assertCurve("item", Motion.ITEM_MS, Motion.ItemEasing)
        assertCurve("reduced", Motion.REDUCED_MS, null)
        assertEquals(num(obj("curves", "enter"), "shift"), Motion.ENTER_SHIFT_DP.toDouble(), 0.0)
        assertEquals(num(obj("curves", "item"), "shift"), Motion.ITEM_SHIFT_DP.toDouble(), 0.0)
        assertEquals(num(obj("press"), "scale"), Motion.PRESS_SCALE.toDouble(), 1e-6)
        assertEquals(num(obj("press"), "alpha"), Motion.PRESS_ALPHA.toDouble(), 1e-6)
        assertEquals(num(obj("scrim"), "alpha"), Motion.SCRIM_ALPHA.toDouble(), 1e-6)
        assertEquals(num(obj("dismiss"), "fraction"), Motion.DISMISS_FRACTION.toDouble(), 1e-6)
        assertEquals(num(obj("dismiss"), "velocity"), Motion.DISMISS_VELOCITY_DP.toDouble(), 1e-6)
        // Compose's spring is the same equation: stiffness (2 pi / response)^2
        assertEquals(((2 * PI / 0.326726) * (2 * PI / 0.326726)).toFloat(), Motion.tab.stiffness, 0.01f)
        assertEquals(0.6f, Motion.tab.dampingRatio, 1e-6f)
    }

    /**
     * PairIt moves its tab indicator and its presses along 1 - e^(-7.5t)(cos 10t + 0.75 sin 10t) over 650, 220 and
     * 550 ms (MainActivity.java:186, GlassUI.java:34). That is exactly the step of a spring with damping 0.6 whose
     * response is the duration times 2 pi / 12.5: the table's springs reproduce PairIt's curves at every instant.
     */
    // decision: PHN-620, PHN-621
    @Test
    fun theSpringsArePairItsCurves() {
        for ((spring, ms) in listOf(Motion.tab to 650.0, Motion.pressDown to 220.0, Motion.pressUp to 550.0)) {
            val zeta = 1 - spring.bounce
            val omega = 2 * PI / spring.duration
            val wd = omega * sqrt(1 - zeta * zeta)
            for (i in 0..100) {
                val u = i / 100.0
                val seconds = u * ms / 1000
                val pairIt = 1 - exp(-7.5 * u) * (cos(10 * u) + 0.75 * sin(10 * u))
                val step = 1 - exp(-zeta * omega * seconds) * (cos(wd * seconds) + zeta * omega / wd * sin(wd * seconds))
                assertEquals("$ms ms at $u", pairIt, step, 2e-5)
            }
        }
        assertTrue("fast: settled within PairIt's 650 ms", Motion.tab.duration < 0.65)
    }
}
