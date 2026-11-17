package com.ferb.design

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The launch's state machine on a fake clock: ready early cuts cleanly into the settle, slow loading loops calmly,
 * reduced motion keeps the mark still. Its numbers and its values at chosen moments are the shared table's
 * (motion/motion.json `launch`, the samples written by tools/launch_samples.py), which FerbDesign's LaunchTests read too.
 */
class LaunchTest {
    private val table = MiniJson.resource("motion/motion.json") as Map<*, *>
    private val launch = table["launch"] as Map<*, *>

    private fun obj(k: String) = launch[k] as Map<*, *>
    private fun num(m: Map<*, *>, k: String) = (m[k] as Number).toFloat()

    // decision: PHN-631
    @Test
    fun theNumbersAreTheSharedTables() {
        assertEquals(num(launch, "size"), Launch.SIZE_DP, 0f)
        assertEquals(num(obj("breath"), "after_ms"), Launch.BREATH_AFTER_MS, 0f)
        assertEquals(num(obj("breath"), "period_ms"), Launch.BREATH_PERIOD_MS, 0f)
        assertEquals(num(obj("breath"), "scale"), Launch.BREATH_SCALE, 0f)
        assertEquals(num(obj("breath"), "alpha"), Launch.BREATH_ALPHA, 0f)
        assertEquals(num(obj("settle"), "ms"), Launch.SETTLE_MS, 0f)
        assertEquals(num(obj("settle"), "prompt_ms"), Launch.SETTLE_PROMPT_MS, 0f)
        val b = (obj("settle")["bezier"] as List<*>).map { (it as Number).toFloat() }
        assertEquals(androidx.compose.animation.core.CubicBezierEasing(b[0], b[1], b[2], b[3]), Launch.SettleEasing)
        assertEquals(num(obj("reduced"), "ms"), Launch.REDUCED_MS, 0f)
        assertEquals("ink0", launch["background"])
    }

    // decision: PHN-631
    @Test
    fun everySampleIsTheSameOnThisPhone() {
        val samples = launch["samples"] as List<*>
        assertTrue(samples.size >= 20)
        for (s in samples) {
            s as Map<*, *>
            val t = num(s, "t")
            val ready = (s["ready"] as Number?)?.toFloat()
            val f = Launch.at(t, ready, s["reduced"] as Boolean)
            val at = "t=$t ready=$ready reduced=${s["reduced"]}"
            assertEquals("$at dotY", num(s, "dotY"), f.dotY, 2e-4f)
            assertEquals("$at dotAlpha", num(s, "dotAlpha"), f.dotAlpha, 2e-4f)
            assertEquals("$at dotScale", num(s, "dotScale"), f.dotScale, 2e-4f)
            assertEquals("$at fScale", num(s, "fScale"), f.fScale, 2e-4f)
            assertEquals("$at handover", num(s, "handover"), f.handover, 2e-3f)
            assertEquals("$at promptOk", s["promptOk"], f.promptOk)
        }
    }

    /**
     * The first frame is the system's splash: the whole mark, the dot in its place (owner PHN-700). Until the breath
     * begins nothing of the mark moves: no drop, no hop, no give of the f.
     */
    // decision: PHN-632, PHN-700, PHN-701, PHN-702
    @Test
    fun theFirstFrameIsTheWholeMarkAndNothingDrops() {
        for (reduced in listOf(false, true)) {
            val f = Launch.at(0f, null, reduced)
            assertEquals(1f, f.dotAlpha, 0f)
            assertEquals(0f, f.handover, 0f)
        }
        for (ms in 0 until Launch.BREATH_AFTER_MS.toInt() step 2) {
            val f = Launch.at(ms.toFloat(), null, reduced = false)
            assertEquals("$ms dotY", 0f, f.dotY, 0f)
            assertEquals("$ms dotAlpha", 1f, f.dotAlpha, 0f)
            assertEquals("$ms dotScale", 1f, f.dotScale, 0f)
            assertEquals("$ms fScale", 1f, f.fScale, 0f)
        }
        // and the dot never leaves its place, breathing or settling
        for (ms in 0..6000 step 10) for (ready in listOf(null, 1200f)) assertEquals(0f, Launch.at(ms.toFloat(), ready, false).dotY, 0f)
    }

    /** Ready at the first frame: no minimum, no intro; the settle starts at once and ends at rest in 450 ms. */
    // decision: PHN-631, PHN-633
    @Test
    fun readyEarlyCutsCleanly() {
        val clock = LaunchClock(reduced = false)
        clock.elapsed(NANOS)
        clock.ready(NANOS)
        var last = -1f
        var promptAt: Float? = null
        for (ms in 0..Launch.SETTLE_MS.toInt() + 20 step 4) {
            val f = clock.frame(NANOS + ms * 1_000_000L)
            assertTrue("handover never goes back ($ms ms)", f.handover >= last)
            assertTrue("no breath while settling", f.dotScale >= 0.999f)
            last = f.handover
            if (f.promptOk && promptAt == null) promptAt = ms.toFloat()
        }
        assertEquals(Launch.Phase.DONE, clock.frame(NANOS + Launch.SETTLE_MS.toLong() * 1_000_000L).phase)
        assertEquals(Launch.SETTLE_PROMPT_MS, promptAt!!, 4f)
        // ready mid-breath: from where the breath is, continuously, to rest
        val before = Launch.at(1500f, null, false)
        val first = Launch.at(1500f, 1500f, false)
        assertEquals(before.dotScale, first.dotScale, 1e-6f)
        assertEquals(before.dotAlpha, first.dotAlpha, 1e-6f)
        assertEquals(Launch.Phase.DONE, Launch.at(1500f + Launch.SETTLE_MS, 1500f, false).phase)
    }

    /** Slow loading: the dot breathes calmly for as long as it takes; never done, never a prompt. */
    // decision: PHN-631
    @Test
    fun slowLoadingLoops() {
        val clock = LaunchClock(reduced = false)
        clock.elapsed(NANOS)
        val scales = (0..20_000 step 50).map { clock.frame(NANOS + it * 1_000_000L) }
        assertTrue(scales.none { it.phase == Launch.Phase.DONE || it.promptOk || it.handover > 0f })
        val breathing = scales.filter { it.phase == Launch.Phase.BREATHING }
        assertTrue(breathing.isNotEmpty())
        assertEquals(Launch.BREATH_SCALE, breathing.minOf { it.dotScale }, 0.005f)
        assertEquals(1f, breathing.maxOf { it.dotScale }, 0.005f)
        // the same moment one period later looks the same: a loop, not a drift
        val a = Launch.at(3000f, null, false)
        val b = Launch.at(3000f + Launch.BREATH_PERIOD_MS, null, false)
        assertEquals(a.dotScale, b.dotScale, 1e-4f)
        assertEquals(a.dotAlpha, b.dotAlpha, 1e-4f)
        // and when it is finally ready, it settles from wherever the breath was
        clock.ready(NANOS + 7_777_000_000L)
        assertEquals(Launch.Phase.DONE, clock.frame(NANOS + 7_777_000_000L + Launch.SETTLE_MS.toLong() * 1_000_000L).phase)
    }

    /** Reduced motion: nothing moves at any moment; the whole mark is still, then the overlay fades off. */
    // decision: PHN-634
    @Test
    fun reducedMotionIsStill() {
        for (ms in 0..5000 step 25) {
            for (ready in listOf(null, 0f, 1000f)) {
                val f = Launch.at(ms.toFloat(), ready, reduced = true)
                assertEquals(0f, f.dotY, 0f)
                assertEquals(1f, f.dotAlpha, 0f)
                assertEquals(1f, f.dotScale, 0f)
                assertEquals(1f, f.fScale, 0f)
            }
        }
        assertFalse(Launch.at(100f, 0f, true).promptOk)
        assertTrue(Launch.at(Launch.REDUCED_MS, 0f, true).promptOk)
    }

    /** The system's splash already played for a while: the clock continues from there, not from zero. */
    // decision: PHN-632
    @Test
    fun theClockContinuesTheSplash() {
        val clock = LaunchClock(reduced = false, startOffsetMs = Launch.startOffset(250))
        assertEquals(250f, clock.elapsed(NANOS), 0.01f)
        clock.syncTo(420f, NANOS + 16_000_000L)
        assertEquals(436f, clock.elapsed(NANOS + 32_000_000L), 0.01f)
        assertEquals(0f, Launch.startOffset(null), 0f)
        assertEquals(0f, Launch.startOffset(-5), 0f)
    }

    /**
     * Android 12+: the settle never plays under the system's splash (it would end unseen and the page would jump in):
     * it starts when the splash says it went, or after 1 s if it never says; and it continues the splash's own clock.
     */
    // decision: PHN-632
    @Test
    fun theSettleWaitsForTheSplashToGo() {
        val s = LaunchState(reduced = false, now = { NANOS })
        s.awaitSplash()
        s.tick(NANOS, ready = true)
        s.tick(NANOS + 300_000_000L, ready = true)
        assertEquals("still under the splash: no settle", Launch.Phase.ARRIVING, s.frame().phase)
        s.syncToSplash(600f, atNanos = NANOS + 300_000_000L)
        s.tick(NANOS + 316_000_000L, ready = true)
        assertEquals(Launch.Phase.SETTLING, s.frame().phase)
        s.tick(NANOS + 316_000_000L + Launch.SETTLE_MS.toLong() * 1_000_000L, ready = true)
        assertTrue(s.done)
        // a splash that never says it went holds the settle 1 s at most
        val t = LaunchState(reduced = false)
        t.awaitSplash()
        t.tick(NANOS, ready = true)
        t.tick(NANOS + 999_000_000L, ready = true)
        assertFalse(t.promptOk)
        t.tick(NANOS + 1_001_000_000L, ready = true)
        t.tick(NANOS + 1_001_000_000L + Launch.SETTLE_PROMPT_MS.toLong() * 1_000_000L, ready = true)
        assertTrue(t.promptOk)
    }

    private companion object {
        const val NANOS = 5_000_000_000L
    }
}
