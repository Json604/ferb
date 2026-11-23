package com.ferb.design.components

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.Phone
import kotlin.math.min

// NOT FROM THE LAB. The temporary chat (the owner, 2026-11-23: "the same small icon that turned it on turns it off", as
// ChatGPT's and Claude's apps): one chat the Mac holds in memory only, with no prior context. The same dashed bubble on
// FERB.app and both phones, in the chat tab's top bar beside the profile button. Words are the app's.

/** The temporary chat's switch for the top bar: [label] is "Temporary chat" while off, "End temporary chat" while on. */
data class TemporaryToggle(
    val on: Boolean,
    val label: String,
    val busy: Boolean = false,
    /** The folded bar's menu button's words (PHN-820): the app's "More". */
    val more: String = "More",
    val onClick: () -> Unit,
)

/**
 * The chat bubble as a path in a 22 by 22 box scaled to [box] (the iPhone's DashedBubbleShape, the same numbers): a body
 * 18 by 14 at (2, 2) with round corners of 6 (4 at the tail's side), and a tail from (11, 16) down to (5, 20.5) and back
 * to (6, 16).
 */
fun dashedBubblePath(box: Rect): Path {
    val s = min(box.width, box.height) / 22f
    val ox = box.left + (box.width - 22f * s) / 2f
    val oy = box.top + (box.height - 22f * s) / 2f
    fun x(v: Float) = ox + v * s
    fun y(v: Float) = oy + v * s
    val r = 6f * s
    val rt = 4f * s
    return Path().apply {
        moveTo(x(8f), y(2f))
        lineTo(x(14f), y(2f))
        // top right, bottom right
        arcTo(Rect(x(20f) - 2 * r, y(2f), x(20f), y(2f) + 2 * r), -90f, 90f, false)
        lineTo(x(20f), y(16f) - r)
        arcTo(Rect(x(20f) - 2 * r, y(16f) - 2 * r, x(20f), y(16f)), 0f, 90f, false)
        // the bottom edge to the tail, the tail, then on to the bottom left corner
        lineTo(x(11f), y(16f))
        lineTo(x(5f), y(20.5f))
        lineTo(x(6f), y(16f))
        arcTo(Rect(x(2f), y(16f) - 2 * rt, x(2f) + 2 * rt, y(16f)), 90f, 90f, false)
        lineTo(x(2f), y(2f) + r)
        arcTo(Rect(x(2f), y(2f), x(2f) + 2 * r, y(2f) + 2 * r), 180f, 90f, false)
        close()
    }
}

/** The bubble's dashes, in units of the 22 box (the iPhone's 2.6 and 2.2 at a 22 frame). */
private val DASH = floatArrayOf(2.6f, 2.2f)

private fun DrawScope.dashedBubble(ink: Color, fill: Color?, width: Float, dash: FloatArray = DASH) {
    val path = dashedBubblePath(Rect(Offset.Zero, size))
    val unit = min(size.width, size.height) / 22f
    if (fill != null) drawPath(path, fill)
    drawPath(path, ink, style = Stroke(width * unit, cap = StrokeCap.Round, join = StrokeJoin.Round,
        pathEffect = PathEffect.dashPathEffect(floatArrayOf(dash[0] * unit, dash[1] * unit))))
}

/** The dashed bubble at [size]: off, a white dashed outline; [on], black dashes over a faint black fill (on white). */
@Composable
fun DashedBubble(on: Boolean, size: Dp = 22.dp, modifier: Modifier = Modifier) {
    Canvas(modifier.size(size)) {
        dashedBubble(if (on) Ink.black else Ink.white, if (on) Ink.black.copy(alpha = 0.14f) else null, 1.8f)
    }
}

/**
 * The switch itself, the profile button's round (42): off, the dashed outline on the bar's dark circle; on, the circle
 * takes the bar's accent (white, as the chosen tab's pill) and the bubble is filled faintly with its dashes still drawn.
 */
@Composable
fun TemporaryButton(t: TemporaryToggle, modifier: Modifier = Modifier) {
    Box(
        modifier
            .pressable(if (t.busy) null else t.onClick, role = Role.Button, label = t.label)
            .semantics { contentDescription = t.label; selected = t.on }
            .alpha(if (t.busy) 0.6f else 1f)
            .size(Phone.profileButton)
            .background(if (t.on) Ink.white else Ink.darkFill, CircleShape),
        contentAlignment = Alignment.Center,
    ) { DashedBubble(t.on) }
}

/** The line at the top of a temporary chat: what it is, on the black capsule of the app's status line. */
@Composable
fun TemporaryBanner(text: String, modifier: Modifier = Modifier) {
    Box(modifier.fillMaxWidth().padding(horizontal = 16.dp), contentAlignment = Alignment.Center) {
        Row(
            Modifier.background(Ink.black, RoundedCornerShape(14.dp)).padding(start = 11.dp, end = 13.dp, top = 7.dp, bottom = 7.dp)
                .semantics(mergeDescendants = true) { },
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Canvas(Modifier.size(15.dp)) { dashedBubble(Ink.white, null, 1.4f * 22f / 15f, floatArrayOf(2f * 22f / 15f, 1.8f * 22f / 15f)) }
            PhoneText(text, 13f, FerbType.Medium, Ink.white, lineSpacing = 1f)
        }
    }
}

/** An empty temporary chat: one quiet line where the conversation will be. */
@Composable
fun TemporaryEmpty(text: String, modifier: Modifier = Modifier) {
    val ink = LocalSkyInk.current
    Box(modifier.fillMaxWidth().padding(horizontal = 40.dp), contentAlignment = Alignment.Center) {
        PhoneText(text, 15f, FerbType.Regular, ink.low, align = TextAlign.Center)
    }
}
