package com.ferb.design

import androidx.compose.animation.core.CubicBezierEasing
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.max
import kotlin.math.min

/**
 * The launch (owner kb PHN-630, PHN-700; PHN-631..634, PHN-701): FERB's whole mark, the f on its dark tile with its
 * amber dot in place, at the screen's centre over ink0 from the system's first frame (the splash icon); nothing moves
 * in or lands (the owner had the dot's drop removed, PHN-700), and while the app loads the dot breathes. The moment the
 * page under it is ready the mark settles into the lock page's mark (a shared element) while ink0 lifts off the sky.
 * Never longer than the loading: ready at the first frame, it settles at once. Every number is the shared table's
 * (src/test/resources/motion/motion.json `launch`, LaunchTest); FerbDesign's Launch.swift is the same function.
 */
object Launch {
    /** The mark's side at the screen's centre (dp): the splash icon's tile (res/drawable launch_mark). */
    const val SIZE_DP = 112f

    /** While loading takes longer, the dot breathes: (1 - cos 2 pi t / period) / 2 of the way to these. */
    const val BREATH_AFTER_MS = 900f
    const val BREATH_PERIOD_MS = 1800f
    const val BREATH_SCALE = 0.86f
    const val BREATH_ALPHA = 0.72f

    /**
     * Ready: everything goes to rest and the mark flies to the page's mark on the standard curve (it starts from rest:
     * the enter curve's sudden start read as a jump on the emulator).
     */
    const val SETTLE_MS = 450f
    val SettleEasing = CubicBezierEasing(0.4f, 0f, 0.2f, 1f)

    /** The system prompt may come this long after the settle began (the mark has done 94 % of its way). */
    const val SETTLE_PROMPT_MS = 315f

    /** Reduced motion: a still mark, and a linear fade off at ready (no breath). */
    const val REDUCED_MS = 200f

    /** What the launch draws at one moment. Lengths are fractions of the mark's side; [handover] 0..1. */
    data class Frame(
        val dotY: Float,
        val dotAlpha: Float,
        val dotScale: Float,
        val fScale: Float,
        val handover: Float,
        val phase: Phase,
        /** The system prompt may appear over it now. */
        val promptOk: Boolean,
    )

    enum class Phase { ARRIVING, BREATHING, SETTLING, DONE }

    private val rest = Frame(0f, 1f, 1f, 1f, 1f, Phase.DONE, true)

    /**
     * The launch at [t] ms after its start. [readyAt]: when the page under it became ready (null: not yet). [reduced]:
     * the system's motion is off or reduced.
     */
    fun at(t: Float, readyAt: Float?, reduced: Boolean): Frame {
        if (readyAt == null || t < readyAt) return free(t, reduced)
        val from = free(readyAt, reduced)
        val ms = if (reduced) REDUCED_MS else SETTLE_MS
        val u = ((t - readyAt) / ms).coerceIn(0f, 1f)
        val e = if (reduced) u else SettleEasing.transform(u)
        val prompt = t - readyAt >= if (reduced) REDUCED_MS else SETTLE_PROMPT_MS
        if (u >= 1f) return rest
        return Frame(
            dotY = lerp(from.dotY, rest.dotY, e),
            dotAlpha = lerp(from.dotAlpha, rest.dotAlpha, e),
            dotScale = lerp(from.dotScale, rest.dotScale, e),
            fScale = lerp(from.fScale, rest.fScale, e),
            handover = e,
            phase = Phase.SETTLING,
            promptOk = prompt,
        )
    }

    /** Before ready: the whole mark at rest, then the dot breathes. Reduced: the mark at rest, still. */
    fun free(t: Float, reduced: Boolean): Frame {
        if (reduced) return Frame(0f, 1f, 1f, 1f, 0f, Phase.ARRIVING, false)
        val k = if (t < BREATH_AFTER_MS) 0f else ((1 - cos(2 * PI * (t - BREATH_AFTER_MS) / BREATH_PERIOD_MS)) / 2).toFloat()
        return Frame(
            dotY = 0f,
            dotAlpha = 1 - (1 - BREATH_ALPHA) * k,
            dotScale = 1 - (1 - BREATH_SCALE) * k,
            fScale = 1f,
            handover = 0f,
            phase = if (t < BREATH_AFTER_MS) Phase.ARRIVING else Phase.BREATHING,
            promptOk = false,
        )
    }

    private fun lerp(a: Float, b: Float, f: Float) = a + (b - a) * f

    /** The mark's place on screen: centred at its size, then the page's mark it settles into, by [Frame.handover]. */
    data class Box(val x: Float, val y: Float, val size: Float) {
        fun toward(target: Box?, f: Float): Box =
            if (target == null) this else Box(lerp(x, target.x, f), lerp(y, target.y, f), lerp(size, target.size, f))
    }

    /** How long the clock has run when the launch's first frame shows, given the system's own start (or 0). */
    fun startOffset(splashStartedMsAgo: Long?): Float = min(max((splashStartedMsAgo ?: 0L).toFloat(), 0f), 60_000f)
}

/**
 * The launch's state over time: when it began, when the page became ready, whether motion is reduced. Pure (the time
 * is passed in), so the same machine runs in the app, the tests and the gallery.
 */
class LaunchClock(private val reduced: Boolean, startOffsetMs: Float = 0f) {
    private var origin: Long? = null
    private var offset = startOffsetMs
    private var readyAt: Float? = null

    /** Ms since the launch began at frame time [nanos] (the first call is the first frame). */
    fun elapsed(nanos: Long): Float {
        val o = origin ?: nanos.also { origin = it }
        return ((nanos - o) / 1_000_000.0 + offset).toFloat()
    }

    /** The system said how long its splash has played already: the clock continues from there. */
    fun syncTo(elapsedMs: Float, nanos: Long) {
        origin = nanos
        offset = elapsedMs
    }

    /** The page under it is ready at frame time [nanos] (the first call counts). */
    fun ready(nanos: Long) {
        if (readyAt == null) readyAt = elapsed(nanos)
    }

    val isReady: Boolean get() = readyAt != null

    fun frame(nanos: Long): Launch.Frame = Launch.at(elapsed(nanos), readyAt, reduced)
}
