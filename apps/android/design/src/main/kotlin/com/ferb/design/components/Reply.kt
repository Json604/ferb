package com.ferb.design.components

import android.os.Build
import android.view.HapticFeedbackConstants
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.drawOutline
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.vector.rememberVectorPainter
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.Phone
import com.ferb.design.rememberReducedMotion
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.min
import kotlin.math.roundToInt
import kotlinx.coroutines.launch

// Reply to a message (the owner, 2026-11-27, kb APP-1146: "quote any ferb's reply message, either slide or an on hover
// icon maybe. many apps have that"; "yeah my messages too"). Not from the lab: FERB.app's ReplyViews.swift on the phone.
// A bubble swiped to the right follows the finger a short way with the reply arrow coming out from under it; let go
// past [ReplySwipe.reach] and its quote goes above the message box. The quote above the box and above a sent message
// are one line, "You: …" or "FERB: …", as FERB.app draws them.

object ReplySwipe {
    /** How far the bubble goes before letting go replies (FERB.app's `QuotableBubble.reach` is 44 pt of trackpad). */
    val reach: Dp = 64.dp

    /** The most it moves: past [reach] it follows the finger at a third of its speed, up to here. */
    val most: Dp = 96.dp

    /** The arrow's circle. */
    internal val arrow: Dp = 30.dp

    /**
     * Where the bubble is for a finger [moved] past the slop: 1:1 up to [reach], then a third of the way, never past
     * [most] (pixels; pure, for the tests).
     */
    fun pulled(moved: Float, reach: Float, most: Float): Float = when {
        moved <= 0f -> 0f
        moved <= reach -> moved
        else -> min(most, reach + (moved - reach) / 3f)
    }

    /** A move is the swipe once it passed the slop going right, clearly more sideways than up or down. */
    fun sideways(dx: Float, dy: Float): Boolean = dx > 0f && abs(dx) > abs(dy) * 1.5f
}

/**
 * A message bubble the owner can reply to (kb APP-1146): swiped right it follows his finger, the reply arrow grows in
 * the space it leaves, a light tick says when letting go will reply, and it springs back. A swipe that starts up or
 * down is the chat's scroll and is left alone, as is one a text selection or a sideways table took. TalkBack offers
 * [label] ("Reply") as the bubble's own action. [lit]: the quoted message lights up briefly after a tap on a quote of
 * it, over [shape] (the bubble's own). With [onReply] null nothing changes.
 *
 * Put it first on the bubble itself (before its background), so the arrow sits beside the bubble, not the row.
 * [pullForShot] holds the bubble that far out (the screenshots of a swipe under way).
 */
@Composable
fun Modifier.replySwipe(
    onReply: (() -> Unit)?,
    label: String,
    shape: Shape,
    lit: Boolean = false,
    litTint: Color = Color.White.copy(alpha = 0.16f),
    pullForShot: Dp? = null,
): Modifier {
    val glow by animateFloatAsState(if (lit) 1f else 0f, tween(if (lit) 180 else 600), label = "quoted")
    val lighting = Modifier.drawWithContent {
        if (glow > 0f) {
            // a soft ring around the bubble and a light over it: a white bubble on a bright sky still shows it
            val ring = 5.dp.toPx()
            translate(-ring, -ring) {
                drawOutline(shape.createOutline(Size(size.width + 2 * ring, size.height + 2 * ring), layoutDirection, this),
                    Color.White.copy(alpha = 0.45f * glow))
            }
            drawContent()
            drawOutline(shape.createOutline(size, layoutDirection, this), litTint.copy(alpha = litTint.alpha * glow))
        } else drawContent()
    }
    if (onReply == null) return this.then(lighting)
    val reply by rememberUpdatedState(onReply)
    val view = LocalView.current
    val reduced = rememberReducedMotion()
    val scope = rememberCoroutineScope()
    val pull = remember { Animatable(0f) }
    val painter = rememberVectorPainter(FerbIcons.reply)
    val shot = pullForShot
    return this
        .semantics { customActions = listOf(CustomAccessibilityAction(label) { reply(); true }) }
        .pointerInput(Unit) {
            val reach = ReplySwipe.reach.toPx()
            val most = ReplySwipe.most.toPx()
            val slop = viewConfiguration.touchSlop
            awaitEachGesture {
                val down = awaitFirstDown(requireUnconsumed = false)
                var dx = 0f
                var dy = 0f
                var from = 0f
                var taken = false
                var ready = false
                var lifted = false
                try {
                    while (true) {
                        val c = awaitPointerEvent().changes.firstOrNull { it.id == down.id } ?: break
                        if (!c.pressed) { lifted = true; break }
                        val d = c.positionChange()
                        if (!taken) {
                            // the chat's scroll, a selection or a table took it first: theirs
                            if (c.isConsumed) return@awaitEachGesture
                            dx += d.x
                            dy += d.y
                            if (hypot(dx, dy) <= slop) continue
                            if (!ReplySwipe.sideways(dx, dy)) return@awaitEachGesture
                            taken = true
                            from = dx
                        } else dx += d.x
                        c.consume()
                        val at = ReplySwipe.pulled(dx - from, reach, most)
                        scope.launch { pull.snapTo(at) }
                        val now = at >= reach
                        if (now && !ready) view.performHapticFeedback(
                            if (Build.VERSION.SDK_INT >= 34) HapticFeedbackConstants.GESTURE_THRESHOLD_ACTIVATE else HapticFeedbackConstants.CLOCK_TICK)
                        ready = now
                    }
                } finally {
                    if (taken) {
                        scope.launch { if (reduced) pull.snapTo(0f) else pull.animateTo(0f, spring(dampingRatio = 0.8f, stiffness = Spring.StiffnessMediumLow)) }
                        if (lifted && ready) reply()
                    }
                }
            }
        }
        .drawBehind {
            val p = shot?.toPx() ?: pull.value
            if (p <= 0f) return@drawBehind
            val reach = ReplySwipe.reach.toPx()
            val progress = (p / reach).coerceIn(0f, 1f)
            val r = ReplySwipe.arrow.toPx() / 2 * (0.6f + 0.4f * progress)
            // in the space the bubble left, from under it; near the foot of a tall bubble, mid-height of a short one
            val centre = Offset(min(p, reach) / 2, size.height - min(size.height / 2, 24.dp.toPx()))
            drawCircle(Color.Black.copy(alpha = (if (p >= reach) 0.85f else 0.55f) * progress), r, centre)
            val icon = r * 1.1f
            translate(centre.x - icon / 2, centre.y - icon / 2) {
                with(painter) { draw(Size(icon, icon), alpha = progress, colorFilter = ColorFilter.tint(Color.White)) }
            }
        }
        .then(lighting)
        .offset { IntOffset((shot?.toPx() ?: pull.value).roundToInt(), 0) }
}

/** "You: …" or "FERB: …": who said the quoted words and their opening, as FERB.app's quote line reads. */
fun quoteLine(byOwner: Boolean, excerpt: String, you: String = "You", ferb: String = "FERB"): String =
    (if (byOwner) you else ferb) + ": " + excerpt

/**
 * The quote above the message box (FERB.app's ComposerQuoteLine): the reply arrow, [line] on one line, and ✕ that
 * drops it ([dropLabel], "Don't reply to this message"). Tapping the words shows the quoted message in the chat. Drawn
 * inside the black box, over its words, as its files are.
 */
@Composable
fun ComposerQuote(
    line: String,
    modifier: Modifier = Modifier,
    description: String = line,
    onShow: (() -> Unit)? = null,
    onDrop: (() -> Unit)? = null,
    dropLabel: String = "Don't reply to this message",
) {
    Row(
        modifier.fillMaxWidth().heightIn(min = 40.dp).background(Ink.darkFill, RoundedCornerShape(20.dp)).padding(start = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(
            Modifier.weight(1f).pressable(onShow).semantics(mergeDescendants = true) { contentDescription = description }.padding(vertical = 9.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(7.dp),
        ) {
            FerbIcon(FerbIcons.reply, 13f, Ink.onDark)
            PhoneText(line, 14f, FerbType.Medium, Ink.white, Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        Box(
            Modifier.size(Phone.touchTarget - 4.dp).pressable(onDrop, label = dropLabel).semantics { contentDescription = dropLabel },
            contentAlignment = Alignment.Center,
        ) { FerbIcon(FerbIcons.close, 14f, Ink.onDark) }
    }
}

/**
 * The quote above a sent message (FERB.app's SentQuoteLine): one compact line on FERB's dark surface, in the shape of
 * the chat's small lines, so it reads on any sky; a tap shows the quoted message. [onShow] null: it only shows.
 */
@Composable
fun SentQuote(line: String, modifier: Modifier = Modifier, description: String = line, onShow: (() -> Unit)? = null) {
    Box(modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) {
        Row(
            Modifier.widthIn(max = Phone.bubbleMaxOwner).pressable(onShow)
                .semantics(mergeDescendants = true) { contentDescription = description }
                .background(Ink.black.copy(alpha = 0.82f), RoundedCornerShape(12.dp))
                .padding(horizontal = 10.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            FerbIcon(FerbIcons.reply, 12f, Ink.onDark)
            PhoneText(line, 13.5f, FerbType.Medium, Ink.onDark, Modifier.weight(1f, fill = false), maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}
