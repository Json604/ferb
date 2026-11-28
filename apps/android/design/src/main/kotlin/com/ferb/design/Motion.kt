package com.ferb.design

import android.animation.ValueAnimator
import android.provider.Settings
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.FiniteAnimationSpec
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext
import kotlin.math.PI

// Motion as the lab gives it (tokens.json `motion`, DESIGN.md "Motion", and the springs in the lab's code). SwiftUI's
// spring(duration:bounce:) is a mass-1 spring with stiffness (2 pi / duration)^2 and damping ratio 1 - bounce; Compose's
// spring(dampingRatio, stiffness) is the same equation, so the numbers carry over exactly.

/** A SwiftUI spring, as Compose's. */
data class LabSpring(val duration: Double, val bounce: Double) {
    val stiffness: Float get() = ((2 * PI / duration) * (2 * PI / duration)).toFloat()
    val dampingRatio: Float get() = (1 - bounce).toFloat()
    fun <T> spec(): FiniteAnimationSpec<T> = spring(dampingRatio = dampingRatio, stiffness = stiffness)
}

object Motion {
    /** tokens.json press: scale 0.97, spring 0.2 s bounce 0.15 (Theme.swift FerbButton; opacity 0.85 while pressed). */
    val press = LabSpring(0.2, 0.15)
    const val PRESS_SCALE = 0.97f
    const val PRESS_ALPHA = 0.85f

    /** tokens.json panel: spring 0.30 to 0.45 s, bounce 0.08 to 0.16, interruptible. The phone's sheets use these. */
    val panelQuick = LabSpring(0.30, 0.08)
    val panel = LabSpring(0.45, 0.12)

    /** Stage.swift: moving between phone screens, spring 0.4 s bounce 0.1. */
    val screen = LabSpring(0.4, 0.1)

    /** Conversation.swift: every change in a played conversation, spring 0.45 s bounce 0.12. */
    val conversation = LabSpring(0.45, 0.12)

    /** Theme.swift GlassSwitch: spring 0.25 s bounce 0.2. */
    val toggle = LabSpring(0.25, 0.2)

    /** Stage.swift setHour: the sky glides to a new time, ease-in-out 2.2 s. */
    const val SKY_GLIDE_MS = 2200

    /** Conversation.swift: the season's layer fades as a chat starts, ease-in-out 1.4 s. */
    const val SEASON_FADE_MS = 1400

    /** Conversation.swift ConvoPlayer.stop: ease-out 0.25 s. */
    const val CLOSE_MS = 250

    /**
     * Every running spinner turns as FERB.app's RowSpinner (TurnCardView.swift): a 70 % arc, one turn in 0.9 s. The
     * owner chose the Mac's over the lab's (Mobile.swift Mark(.running): a 30 % arc in 0.8 s, which a still frame
     * shows as "("), 2026-11-28. Theme.swift Spinner: 0.9 s.
     */
    const val RUNNING_TURN_MS = 900
    const val RUNNING_SWEEP = 0.7f
    const val SPINNER_TURN_MS = 900

    /** SwiftUI's easeInOut and easeOut curves. */
    val EaseInOut = CubicBezierEasing(0.42f, 0f, 0.58f, 1f)
    val EaseOut = CubicBezierEasing(0f, 0f, 0.58f, 1f)

    fun <T> skyGlide(): FiniteAnimationSpec<T> = tween(SKY_GLIDE_MS, easing = EaseInOut)
    fun <T> seasonFade(): FiniteAnimationSpec<T> = tween(SEASON_FADE_MS, easing = EaseInOut)

    // ---- PairIt's motion (owner, kb PHN-620; the system: PHN-621..624). One table for both phones:
    // src/test/resources/motion/motion.json (MotionTableTest; FerbDesign's Motion.swift reads the same). Every screen
    // moves through MotionHosts.kt, never with numbers of its own.

    /** The tab indicator: PairIt's 650 ms curve 1 - e^(-7.5t)(cos 10t + 0.75 sin 10t), which is this spring's step. */
    val tab = LabSpring(0.326726, 0.4)

    /** A press at touch-down and its release: PairIt's same curve over 220 ms and 550 ms (GlassUI.pressFeedback). */
    val pressDown = LabSpring(0.110584, 0.4)
    val pressUp = LabSpring(0.276460, 0.4)

    /** A sheet comes up from below and goes back down (the lab's panel: 0.30 s, bounce 0.08, interruptible). */
    val sheet = LabSpring(0.30, 0.08)

    /** What comes in: PairIt's panel, 24 dp and a fade over 220 ms on (.22, 1, .36, 1). */
    const val ENTER_MS = 220
    const val ENTER_SHIFT_DP = 24f
    val EnterEasing = CubicBezierEasing(0.22f, 1f, 0.36f, 1f)

    /** What goes: a 120 ms linear fade (PairIt's notice leaving; its iPhone's reduced motion). */
    const val EXIT_MS = 120

    /** A list item arriving: up 6 dp and in over 180 ms on (.23, 1, .32, 1) (PairIt's notice). */
    const val ITEM_MS = 180
    const val ITEM_SHIFT_DP = 6f
    val ItemEasing = CubicBezierEasing(0.23f, 1f, 0.32f, 1f)

    /** With the system's motion reduced nothing moves; a change is at most this linear fade (Android: instant). */
    const val REDUCED_MS = 120

    /** The dimmed page under a sheet (Mobile.swift Sheet). */
    const val SCRIM_ALPHA = 0.5f

    /** A dismissible sheet let go past this share of its height, or flung down this fast (dp/s), closes. */
    const val DISMISS_FRACTION = 0.25f
    const val DISMISS_VELOCITY_DP = 900f

    fun <T> enter(): FiniteAnimationSpec<T> = tween(ENTER_MS, easing = EnterEasing)
    fun <T> exit(): FiniteAnimationSpec<T> = tween(EXIT_MS, easing = androidx.compose.animation.core.LinearEasing)
    fun <T> item(): FiniteAnimationSpec<T> = tween(ITEM_MS, easing = ItemEasing)

    /**
     * A bar filling as work is counted (the memory import's card, kb MEM-456): to its new share over 650 ms, easing out
     * (SwiftUI's easeOut); with the system's motion off it is there at once.
     */
    const val PROGRESS_MS = 650

    fun <T> progress(): FiniteAnimationSpec<T> = tween(PROGRESS_MS, easing = EaseOut)
}

/**
 * True when the owner turned animations off (Settings > Accessibility > Remove animations, or the animator duration
 * scale at 0). Compose's own transitions already follow the scale; the season layer and other running motion read
 * this to stop, as Weather.swift stops under Reduce Motion.
 */
@Composable
fun rememberReducedMotion(): Boolean {
    val context = LocalContext.current
    return remember(context) { reducedMotion(context) }
}

/** [rememberReducedMotion] outside composition (the launch decides before the first frame, kb PHN-634). */
fun reducedMotion(context: android.content.Context): Boolean {
    val scale = runCatching {
        Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f)
    }.getOrDefault(1f)
    return scale == 0f || !ValueAnimator.areAnimatorsEnabled()
}
