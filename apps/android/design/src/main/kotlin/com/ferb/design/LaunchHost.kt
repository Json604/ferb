package com.ferb.design

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import com.ferb.design.components.FERB_DOT_X
import com.ferb.design.components.FERB_DOT_Y
import com.ferb.design.components.ferbDot
import com.ferb.design.components.ferbLetter
import com.ferb.design.components.ferbTile

/**
 * The launch on screen (kb PHN-631..634; the motion is [Launch]): one per new activity, drawn over everything from
 * the first frame. The page's own mark marked [launchTarget] is the shared element: hidden while the launch's mark
 * flies to it, shown in the same frame the launch ends. Only layer properties change per frame (translation, scale,
 * alpha) and one rectangle's alpha: no layout and no recomposition per frame, the halo's blur drawn once.
 */
@Stable
class LaunchState(
    val reduced: Boolean,
    /** When the launch's clock read 0, in System.nanoTime's time (the process's start on a cold start), or null. */
    private val originNanos: Long? = null,
    private val now: () -> Long = System::nanoTime,
) {
    internal val clock = LaunchClock(reduced)
    internal var nanos by mutableLongStateOf(-1L)
    private var splash: Pair<Float, Long>? = null

    /** The page's mark in root coordinates (null: the page has none; the mark then fades where it is). */
    internal var target: Rect? by mutableStateOf(null)

    /** The launch has ended: nothing of it is drawn and the page's own mark shows. */
    var done by mutableStateOf(false)
        private set

    /** The mark has settled far enough for the system prompt to come over it (PHN-633). */
    var promptOk by mutableStateOf(false)
        private set

    /** Android 12+: the system's splash still covers the app, so the settle waits for it to go (at most 1 s). */
    private var splashCovers = false
    private var firstTick = -1L

    /** The system's splash is over the app until [syncToSplash] (Android 12+). */
    fun awaitSplash() {
        splashCovers = true
    }

    /**
     * The system's splash had played [elapsedMs] at [atNanos] (Android 12+'s icon animation) and goes now: the launch
     * continues from that moment, and may settle from here on.
     */
    fun syncToSplash(elapsedMs: Float, atNanos: Long = now()) {
        splash = elapsedMs to atNanos
        splashCovers = false
    }

    /** The system's splash went without saying how far it played. */
    fun splashGone() {
        splashCovers = false
    }

    internal fun tick(frameNanos: Long, ready: Boolean) {
        if (firstTick < 0) firstTick = frameNanos
        if (nanos < 0 && originNanos != null) clock.syncTo(Launch.startOffset((frameNanos - originNanos) / 1_000_000), frameNanos)
        splash?.let { (ms, at) ->
            if (!clock.isReady) clock.syncTo(ms + (frameNanos - at) / 1_000_000f, frameNanos)
            splash = null
        }
        // a settle under the splash would play unseen and then jump: it starts when the app itself shows
        val covered = splashCovers && frameNanos - firstTick < SPLASH_WAIT_MAX_NS
        if (ready && !covered) clock.ready(frameNanos)
        nanos = frameNanos
        val f = clock.frame(frameNanos)
        if (f.promptOk && !promptOk) promptOk = true
        if (f.phase == Launch.Phase.DONE) done = true
    }

    /** The frame to draw now (read while drawing only); before the first tick, the splash's moment. */
    internal fun frame(): Launch.Frame =
        if (nanos >= 0) clock.frame(nanos)
        else Launch.at(originNanos?.let { Launch.startOffset((now() - it) / 1_000_000) } ?: 0f, null, reduced)
}

/** Longest the settle waits for a splash that never says it went (a system without the exit callback). */
private const val SPLASH_WAIT_MAX_NS = 1_000_000_000L

/** The launch running now, for the page's mark ([launchTarget]); null outside a launch. */
val LocalLaunch = staticCompositionLocalOf<LaunchState?> { null }

/** This is the page's mark the launch settles into: it reports where it is and stays hidden until the launch ends. */
fun Modifier.launchTarget(): Modifier = composed {
    val launch = LocalLaunch.current
    if (launch == null || launch.done || launch.reduced) {
        this
    } else {
        this
            .onGloballyPositioned { launch.target = it.boundsInRoot() }
            .graphicsLayer { alpha = if (launch.done) 1f else 0f }
    }
}

/**
 * The launch over the page: ink0 with the mark at the centre, the dot's arrival and breath, and the settle into the
 * page's mark once [ready] (the page under it can be shown). Draws nothing once [LaunchState.done]. Without [ground]
 * only the mark is drawn (over the sky).
 */
@Composable
fun LaunchOverlay(
    state: LaunchState,
    ready: Boolean,
    modifier: Modifier = Modifier,
    ground: Boolean = true,
    /** One moment drawn still instead of the frame clock (the screenshots). */
    fixed: Launch.Frame? = null,
) {
    if (state.done) return
    val isReady by rememberUpdatedState(ready)
    if (fixed == null) {
        LaunchedEffect(state) {
            while (!state.done) withFrameNanos { state.tick(it, isReady) }
        }
    }
    val frame = { fixed ?: state.frame() }
    val density = LocalDensity.current
    val side = with(density) { Launch.SIZE_DP.dp.toPx() }
    val back = if (ground) Modifier.drawBehind { drawRect(Tone.ink0, alpha = 1f - frame().handover) } else Modifier
    BoxWithConstraints(modifier.fillMaxSize().then(back)) {
        val w = constraints.maxWidth.toFloat()
        val h = constraints.maxHeight.toFloat()
        val home = Launch.Box((w - side) / 2, (h - side) / 2, side)
        Box(
            Modifier.size(Launch.SIZE_DP.dp).graphicsLayer {
                val f = frame()
                val to = state.target?.let { Launch.Box(it.left, it.top, it.width) }
                val at = if (state.reduced) home else home.toward(to, f.handover)
                transformOrigin = TransformOrigin(0f, 0f)
                translationX = at.x
                translationY = at.y
                scaleX = at.size / side
                scaleY = at.size / side
                alpha = if (to == null || state.reduced) 1f - f.handover else 1f
            },
        ) {
            Canvas(Modifier.fillMaxSize()) { ferbTile(size.width) }
            Canvas(
                Modifier.fillMaxSize().graphicsLayer {
                    scaleY = frame().fScale
                    transformOrigin = TransformOrigin(F_FOOT_X, F_FOOT_Y)
                },
            ) { drawPath(ferbLetter(size.width), Tone.text) }
            Canvas(
                Modifier.fillMaxSize().graphicsLayer {
                    val f = frame()
                    translationY = f.dotY * size.width
                    scaleX = f.dotScale
                    scaleY = f.dotScale
                    alpha = f.dotAlpha
                    transformOrigin = TransformOrigin(FERB_DOT_X, FERB_DOT_Y)
                },
            ) { ferbDot(size.width) }
        }
    }
}

/** The f's foot (the middle of its stem's bottom) in its tile: the pivot of Frame.fScale (1 since PHN-700). */
private const val F_FOOT_X = ((49.969f + 60.477f) / 2 - 16) / 96
private const val F_FOOT_Y = (90f - 16) / 96
