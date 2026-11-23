package com.ferb.design

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.AnimationVector1D
import androidx.compose.animation.core.spring
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.LazyItemScope
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch

// FERB's one motion system on Android (kb PHN-621..624; the owner wants PairIt's motion, PHN-620). Every screen moves
// through these, with the numbers of Motion.kt and nothing of its own:
// - MotionSwitch: one thing shown at a time (the gate's pages, the tabs, a folder, the sheet over the page). What comes
//   in rises (or slides the tab's way) and fades in; what goes fades out and takes no touch while it goes; the new one
//   takes touches at once. A target `instant` says shows at once, the old one gone in the same frame (the lock).
// - The pieces read where they are (LocalMotionPresence): the sheet's panel comes up from below over a fading dim and
//   can be dragged down; the tab bar stays and its white pill slides (TopBar.kt).
// - Lists: motionItem() on a lazy row, motionAppear() on a row of a plain column.
// Only alpha and translation in graphics layers (no layout, no recomposition per frame), never a blur. With the system's
// animations off or removed nothing moves and every change is instant (Compose also follows the duration scale).

/** Where something shown by a [MotionSwitch] is in its coming (0 to 1) and going (back to 0). */
@Stable
class MotionPresence internal constructor(start: Float, val direction: Int) {
    internal val progress: Animatable<Float, AnimationVector1D> = Animatable(start)

    /** Its share of being shown, for reading while drawing (never in composition: it changes every frame). */
    val value: Float get() = progress.value

    /** Going: it takes no touch and its content is held as it was (heldWhileLeaving). */
    var leaving by mutableStateOf(false)
        internal set

    /** A dismissible sheet dragged down, in pixels. */
    internal val drag: Animatable<Float, AnimationVector1D> = Animatable(0f)

    /** Moving now (or not yet started): only then do the hosts add a layer. */
    internal val moving: Boolean get() = progress.isRunning || progress.targetValue != 1f
}

/** The presence of the [MotionSwitch] entry this is inside, or null (screenshots, previews: drawn at rest). */
val LocalMotionPresence = staticCompositionLocalOf<MotionPresence?> { null }

/** How a [MotionSwitch] moves its entries. */
enum class MotionKind {
    /** A page: rises 24 dp and fades in; fades out. */
    SCREEN,

    /** A tab's page: slides 24 dp from the side of the tab it comes from and fades in; fades out. */
    TAB,

    /** A sheet: the entry itself stays; its dim and panel move (Sheet), on the sheet spring both ways. */
    SHEET,
}

private class Entry<K>(val key: K, val presence: MotionPresence)

private class Switch<K>(var target: K?) {
    val entries = ArrayList<Entry<K>>()
    val pending = ArrayList<Pair<Entry<K>, Boolean>>()
    var version by mutableIntStateOf(0)
}

/**
 * Shows [target] (nothing for null) and moves between targets as [kind] says. [content] draws a target: it must draw
 * the target it is given (a going one is drawn as it was), never the current state. [direction] says from which side a
 * new target comes (-1 left, 1 right, 0 none); [instant] targets replace everything at once (the lock: nothing of the
 * page before may show for a frame).
 */
@Composable
fun <K : Any> MotionSwitch(
    target: K?,
    modifier: Modifier = Modifier,
    kind: MotionKind = MotionKind.SCREEN,
    instant: (K) -> Boolean = { false },
    direction: (from: K, to: K) -> Int = { _, _ -> 0 },
    content: @Composable (K) -> Unit,
) {
    val reduced = rememberReducedMotion()
    val scope = rememberCoroutineScope()
    val switch = remember { Switch<K>(target).also { s -> if (target != null) s.entries += Entry(target, MotionPresence(1f, 0)) } }
    switch.version // a finished exit removes its entry and recomposes
    if (target != switch.target) {
        val from = switch.target
        switch.target = target
        if (reduced || (target != null && instant(target))) {
            switch.entries.clear()
            switch.pending.clear()
            if (target != null) switch.entries += Entry(target, MotionPresence(1f, 0))
        } else {
            for (e in switch.entries) {
                if (e.key != target && !e.presence.leaving) {
                    e.presence.leaving = true
                    switch.pending += e to false
                }
            }
            if (target != null) {
                val back = switch.entries.firstOrNull { it.key == target }
                if (back != null) {
                    back.presence.leaving = false
                    switch.pending += back to true
                } else {
                    val e = Entry(target, MotionPresence(0f, if (from != null) direction(from, target) else 0))
                    switch.entries += e
                    switch.pending += e to true
                }
            }
        }
    }
    SideEffect {
        if (switch.pending.isEmpty()) return@SideEffect
        val start = switch.pending.toList()
        switch.pending.clear()
        for ((e, show) in start) {
            scope.launch {
                if (show && e.presence.drag.value != 0f) launch { e.presence.drag.animateTo(0f, Motion.sheet.spec()) }
                val spec = when {
                    kind == MotionKind.SHEET -> Motion.sheet.spec()
                    show -> Motion.enter()
                    else -> Motion.exit<Float>()
                }
                e.presence.progress.animateTo(if (show) 1f else 0f, spec)
                if (!show && e.presence.leaving) {
                    switch.entries.remove(e)
                    switch.version++
                }
            }
        }
    }
    Box(modifier) {
        for (e in switch.entries.toList()) {
            key(e.key) {
                val p = e.presence
                val layer = if (kind != MotionKind.SHEET && p.moving) Modifier.switchLayer(kind, p) else Modifier
                Box(Modifier.fillMaxSize().then(layer).then(if (p.leaving && kind != MotionKind.SHEET) Modifier.inert() else Modifier)) {
                    androidx.compose.runtime.CompositionLocalProvider(LocalMotionPresence provides p) { content(e.key) }
                }
            }
        }
    }
}

private fun Modifier.switchLayer(kind: MotionKind, p: MotionPresence): Modifier = graphicsLayer {
    val v = p.value
    alpha = v.coerceIn(0f, 1f)
    if (!p.leaving) {
        val shift = (1f - v) * Motion.ENTER_SHIFT_DP.dp.toPx()
        if (kind == MotionKind.TAB) translationX = shift * p.direction else translationY = shift
    }
}

/** Takes every touch and gives none to what is inside: a thing on its way out never acts. */
fun Modifier.inert(): Modifier = pointerInput(Unit) {
    awaitPointerEventScope {
        while (true) awaitPointerEvent(PointerEventPass.Initial).changes.forEach { it.consume() }
    }
}

/**
 * A page drawn over another page (a full-screen sheet: a coding session, Watch live, the Mac's camera): every touch on
 * it stays on it, on its empty places and margins too, and none reaches the page under it. Its own controls still take
 * theirs. Without it a tap beside a session's message box went through to the Sessions tab's New session under it
 * (owner 2026-11-23, kb PHN-815).
 */
fun Modifier.coversPage(): Modifier = pointerInput(Unit) {
    awaitPointerEventScope { while (true) awaitPointerEvent(PointerEventPass.Final) }
}

/**
 * For a page shown by a [MotionKind.SHEET] switch that is not a [com.ferb.design.components.Sheet] (Watch live, a
 * coding session): it rises and fades in, fades out, takes no touch while going, and while shown keeps every touch on
 * it from the page under it ([coversPage]).
 */
@Composable
fun Modifier.presenceFade(): Modifier {
    val p = LocalMotionPresence.current ?: return this.coversPage()
    return this.then(Modifier.switchLayer(MotionKind.SCREEN, p)).then(if (p.leaving) Modifier.inert() else Modifier.coversPage())
}

/** [value] as it was while this was shown: a going page or sheet keeps drawing what it showed (a question answered). */
@Composable
fun <T> heldWhileLeaving(value: T): T {
    val leaving = LocalMotionPresence.current?.leaving == true
    val held = remember { arrayOf<Any?>(value) }
    if (!leaving) held[0] = value
    @Suppress("UNCHECKED_CAST")
    return held[0] as T
}

/**
 * The sheet's panel (Sheet): up from below by its own height, down again; a dismissible one follows a drag down and
 * closes past a quarter of its height or on a fling, else springs back. Going, it takes no touch.
 */
@Composable
internal fun Modifier.sheetPanelMotion(p: MotionPresence, onDismiss: (() -> Unit)?): Modifier {
    val scope = rememberCoroutineScope()
    val height = remember { floatArrayOf(0f) }
    val fling = with(LocalDensity.current) { Motion.DISMISS_VELOCITY_DP.dp.toPx() }
    val drag = rememberDraggableState { delta -> scope.launch { p.drag.snapTo((p.drag.value + delta).coerceAtLeast(0f)) } }
    val moved = this
        .onSizeChanged { height[0] = it.height.toFloat() }
        .graphicsLayer { translationY = (1f - p.value) * size.height + p.drag.value }
    return when {
        p.leaving -> moved.inert()
        onDismiss == null -> moved
        else -> moved.draggable(drag, Orientation.Vertical, onDragStopped = { velocity ->
            if (p.drag.value > height[0] * Motion.DISMISS_FRACTION || velocity > fling) onDismiss()
            else p.drag.animateTo(0f, Motion.sheet.spec())
        })
    }
}

/**
 * The dim under a sheet, fading with it. It is one rectangle, so its alpha is applied to that rectangle as it is drawn
 * (the same pixels) instead of through a whole-screen offscreen buffer on every frame of the fade (kb PHN-711).
 */
internal fun Modifier.sheetScrimMotion(p: MotionPresence): Modifier = graphicsLayer {
    alpha = p.value.coerceIn(0f, 1f)
    compositingStrategy = androidx.compose.ui.graphics.CompositingStrategy.ModulateAlpha
}

/**
 * A row of a lazy list: fades in where it arrives, the others glide to their places, a removed one fades out.
 * [arriving] says this row is new to the list (it was not in it a moment ago): only then does it fade in. Compose
 * treats every row as appearing after a jump of the list's position (scrollToItem resets its item animations), so a
 * fade for every row turns a jump into a blank list for as long as the fade lasts (kb PHN-670); a row that was there
 * already, or one scrolled into view, is drawn at once.
 */
fun LazyItemScope.motionItem(arriving: Boolean = true): Modifier = Modifier.animateItem(
    fadeInSpec = if (arriving) Motion.item() else null,
    placementSpec = spring(Motion.sheet.dampingRatio, Motion.sheet.stiffness, IntOffset(1, 1)),
    fadeOutSpec = Motion.exit(),
)

/**
 * A row of a plain column that arrives after the column was first shown: up 6 dp and in (PairIt's notice). One already
 * there when the column first draws does not move.
 */
@Composable
fun Modifier.motionAppear(): Modifier {
    val reduced = rememberReducedMotion()
    val ready = LocalAppearReady.current
    val shown = remember { Animatable(if (reduced || !ready) 1f else 0f) }
    androidx.compose.runtime.LaunchedEffect(Unit) { if (shown.value < 1f) shown.animateTo(1f, Motion.item()) }
    if (!shown.isRunning && shown.targetValue == 1f) return this
    return graphicsLayer {
        alpha = shown.value
        translationY = (1f - shown.value) * Motion.ITEM_SHIFT_DP.dp.toPx()
    }
}

/** False while a column first draws (its rows are already there), true after: set by [AppearAfterFirst]. */
internal val LocalAppearReady = staticCompositionLocalOf { true }

/** Rows inside that use [motionAppear] stay still on the first draw and move in when they arrive later. */
@Composable
fun AppearAfterFirst(content: @Composable () -> Unit) {
    var ready by remember { mutableStateOf(false) }
    SideEffect { ready = true }
    androidx.compose.runtime.CompositionLocalProvider(LocalAppearReady provides ready, content = content)
}

/**
 * [value] shown while it is not null (a status line, the chat's jump control, its day label): it rises 6 dp and fades
 * in on the item curve, and fades out on the exit curve, still drawing what it showed while it goes. With [push] its
 * height opens and closes with it, so what is below moves by its own height and no more, smoothly. On the first draw
 * it is simply there; with the system's animations off every change is instant.
 */
@Composable
fun <T : Any> MotionShow(value: T?, modifier: Modifier = Modifier, push: Boolean = false, content: @Composable (T) -> Unit) {
    val reduced = rememberReducedMotion()
    val held = remember { arrayOfNulls<Any>(1) }
    if (value != null) held[0] = value
    val shift = with(LocalDensity.current) { Motion.ITEM_SHIFT_DP.dp.roundToPx() }
    val enter = if (reduced) androidx.compose.animation.EnterTransition.None else
        androidx.compose.animation.fadeIn(Motion.item()) + androidx.compose.animation.slideInVertically(Motion.item()) { shift } +
            (if (push) androidx.compose.animation.expandVertically(Motion.item(), expandFrom = androidx.compose.ui.Alignment.Top)
            else androidx.compose.animation.EnterTransition.None)
    val exit = if (reduced) androidx.compose.animation.ExitTransition.None else
        androidx.compose.animation.fadeOut(Motion.exit()) +
            (if (push) androidx.compose.animation.shrinkVertically(Motion.exit(), shrinkTowards = androidx.compose.ui.Alignment.Top)
            else androidx.compose.animation.ExitTransition.None)
    androidx.compose.animation.AnimatedVisibility(value != null, modifier, enter, exit) {
        @Suppress("UNCHECKED_CAST")
        (held[0] as T?)?.let { content(it) }
    }
}
