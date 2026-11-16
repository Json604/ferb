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

    /** Mobile.swift Mark(.running): one turn in 0.8 s. Theme.swift Spinner: 0.9 s. */
    const val MARK_TURN_MS = 800
    const val SPINNER_TURN_MS = 900

    /** SwiftUI's easeInOut and easeOut curves. */
    val EaseInOut = CubicBezierEasing(0.42f, 0f, 0.58f, 1f)
    val EaseOut = CubicBezierEasing(0f, 0f, 0.58f, 1f)

    fun <T> skyGlide(): FiniteAnimationSpec<T> = tween(SKY_GLIDE_MS, easing = EaseInOut)
    fun <T> seasonFade(): FiniteAnimationSpec<T> = tween(SEASON_FADE_MS, easing = EaseInOut)
}

/**
 * True when the owner turned animations off (Settings > Accessibility > Remove animations, or the animator duration
 * scale at 0). Compose's own transitions already follow the scale; the season layer and other running motion read
 * this to stop, as Weather.swift stops under Reduce Motion.
 */
@Composable
fun rememberReducedMotion(): Boolean {
    val context = LocalContext.current
    return remember(context) {
        val scale = runCatching {
            Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f)
        }.getOrDefault(1f)
        scale == 0f || !ValueAnimator.areAnimatorsEnabled()
    }
}
