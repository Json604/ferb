package com.ferb.design.components

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.core.animateFloat
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.ui.input.pointer.isOutOfBounds
import androidx.compose.runtime.setValue
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.LabColors
import com.ferb.design.Motion
import com.ferb.design.Phone
import com.ferb.design.rememberReducedMotion
import com.ferb.design.sky.SkyInk

// The phone's pieces (Mobile.swift "Pieces"): black, white and neutral greys over the sky.

/** The sky's ink for what sits straight on it (Mobile.swift `\.skyInk`); the screen frame provides it. */
val LocalSkyInk = androidx.compose.runtime.staticCompositionLocalOf { SkyInk.WHITE }

/**
 * A press that sinks (DESIGN.md: "Touch-down sinks"): scale 0.97 and 85 % the moment a finger lands, even inside a
 * scrolling list (PairIt's GlassUI.pressFeedback; Compose's own press waits out a tap timeout there), on PairIt's
 * springs (Motion.pressDown, Motion.pressUp); a scroll that takes the touch lets it rise again. Nothing is consumed:
 * the click, scrolling and TalkBack are Compose's. With motion reduced it only dims. Small targets keep their drawn
 * size; Compose widens their touch area to the 48 dp minimum by itself.
 */
@Composable
fun Modifier.pressable(onClick: (() -> Unit)?, role: Role = Role.Button, label: String? = null): Modifier {
    if (onClick == null) return this
    val source = remember { MutableInteractionSource() }
    val pressedByCompose by source.collectIsPressedAsState()
    var touched by remember { androidx.compose.runtime.mutableStateOf(false) }
    val pressed = pressedByCompose || touched
    val reduced = com.ferb.design.rememberReducedMotion()
    // read in the layer, never in composition: a press's frames recompose nothing around it (kb PHN-711)
    val scale = animateFloatAsState(
        if (pressed && !reduced) Motion.PRESS_SCALE else 1f,
        if (pressed) Motion.pressDown.spec() else Motion.pressUp.spec(),
        label = "press",
    )
    return this
        .pointerInput(Unit) {
            awaitEachGesture {
                val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
                touched = true
                try {
                    // until it lifts, leaves, or a scroll takes it (a move consumed by the Final pass)
                    while (true) {
                        val c = awaitPointerEvent(PointerEventPass.Final).changes.firstOrNull { it.id == down.id } ?: break
                        if (!c.pressed || c.isOutOfBounds(size, extendedTouchPadding)) break
                        if (c.positionChanged() && c.isConsumed) break
                    }
                } finally {
                    touched = false
                }
            }
        }
        .graphicsLayer { scaleX = scale.value; scaleY = scale.value; alpha = if (pressed) Motion.PRESS_ALPHA else 1f }
        .clickable(interactionSource = source, indication = null, role = role, onClickLabel = label, onClick = onClick)
}

/** A text in the lab's phone type (Mobile.swift `.type`). */
@Composable
fun PhoneText(
    text: String,
    size: Float,
    weight: FontWeight = FontWeight.Normal,
    color: Color = Ink.black,
    modifier: Modifier = Modifier,
    maxLines: Int = Int.MAX_VALUE,
    lineSpacing: Float = 0f,
    overflow: TextOverflow = TextOverflow.Ellipsis,
    align: androidx.compose.ui.text.style.TextAlign? = null,
) {
    val style = FerbType.style(size, weight, color, lineSpacing)
    BasicText(text, modifier, if (align != null) style.copy(textAlign = align) else style, maxLines = maxLines, overflow = overflow)
}

/** Mobile.swift Card: a white card of rows, PairIt's settings card. */
@Composable
fun Card(modifier: Modifier = Modifier, padding: Dp = Phone.cardPadding, content: @Composable ColumnScope.() -> Unit) {
    // SwiftUI's VStack centres what is narrower than the card; rows fill it.
    Column(
        modifier.fillMaxWidth().background(CardInk.current().card, RoundedCornerShape(Phone.cardCorner)).padding(padding),
        horizontalAlignment = Alignment.CenterHorizontally,
        content = content,
    )
}

/** Mobile.swift Divide: a line between rows, inset 14. */
@Composable
fun Divide() {
    Box(Modifier.fillMaxWidth().padding(horizontal = 14.dp).height(1.dp).background(CardInk.current().line))
}

/** Mobile.swift CardRow: a mark or icon, a title and its detail, and what is on the right. One TalkBack stop. */
@Composable
fun CardRow(
    title: String,
    detail: String? = null,
    danger: Boolean = false,
    onClick: (() -> Unit)? = null,
    leading: @Composable () -> Unit = {},
    trailing: @Composable RowScope.() -> Unit = {},
) {
    Row(
        Modifier.fillMaxWidth().pressable(onClick).semantics(mergeDescendants = true) {}
            .padding(horizontal = Phone.rowPaddingH, vertical = Phone.rowPaddingV),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(Phone.rowLeading), contentAlignment = Alignment.Center) { leading() }
        Spacer(Modifier.width(Phone.rowGap))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            val ink = CardInk.current()
            PhoneText(title, 16.5f, FerbType.Semibold, if (danger) ink.red else ink.primary, maxLines = 2)
            if (detail != null) PhoneText(detail, 14f, color = ink.secondary, maxLines = 1)
        }
        Spacer(Modifier.width(8.dp))
        trailing()
    }
}

/** Mobile.swift RowIcon: a 20-point symbol, red for danger. */
@Composable
fun RowIcon(icon: ImageVector, danger: Boolean = false) {
    val ink = CardInk.current()
    FerbIcon(icon, 20f, if (danger) ink.red else ink.primary)
}

/** Mobile.swift Opens: an optional value and a chevron. */
@Composable
fun Opens(value: String? = null) {
    Row(horizontalArrangement = Arrangement.spacedBy(5.dp), verticalAlignment = Alignment.CenterVertically) {
        val ink = CardInk.current()
        if (value != null) PhoneText(value, 15f, FerbType.Medium, ink.faint)
        FerbIcon(FerbIcons.chevronRight, 14f, ink.faint)
    }
}

/** The state of a task or a part: each has its own shape, so colour is never the only sign. */
enum class MarkKind(val spoken: String) { DONE("Done"), ASKING("Waiting for you"), FAILED("Couldn't finish"), RUNNING("Running"), WAITING("Not started") }

/** Mobile.swift Mark: solid shapes; running turns once in 0.8 s. */
@Composable
fun Mark(kind: MarkKind, size: Dp = Phone.mark, modifier: Modifier = Modifier) {
    val glyph = with(LocalDensity.current) { (size * 0.55f).toSp() }
    val check = size.value * 0.45f
    Box(modifier.size(size).semantics { contentDescription = kind.spoken }, contentAlignment = Alignment.Center) {
        when (kind) {
            MarkKind.DONE -> {
                val dark = LocalOnFerb.current
                Box(Modifier.size(size).background(if (dark) Color.White else Ink.black, CircleShape))
                FerbIcon(FerbIcons.check, check, if (dark) Ink.black else Color.White)
            }
            MarkKind.ASKING -> {
                Box(Modifier.size(size).background(Ink.amber, CircleShape))
                BasicText("?", style = FerbType.style(0f, FerbType.Bold, Color.Black).copy(fontSize = glyph, letterSpacing = 0.sp))
            }
            MarkKind.FAILED -> {
                Box(Modifier.size(size).background(Ink.red, CircleShape))
                BasicText("!", style = FerbType.style(0f, FerbType.Bold, Color.White).copy(fontSize = glyph, letterSpacing = 0.sp))
            }
            MarkKind.RUNNING -> RunningArc(size, CardInk.current().primary)
            MarkKind.WAITING -> Canvas(Modifier.size(size)) {
                val w = 2.dp.toPx()
                drawCircle(LabColors.waitingRing, radius = this.size.minDimension / 2 - w / 2,
                    style = Stroke(w, pathEffect = PathEffect.dashPathEffect(floatArrayOf(3.dp.toPx(), 3.dp.toPx()))))
            }
        }
    }
}

/** Mobile.swift Mark(.running): a 30 % arc, 2.5 wide, round ends, inset 2, one turn in 0.8 s. */
@Composable
fun RunningArc(size: Dp, color: Color, modifier: Modifier = Modifier, stroke: Dp = 2.5.dp) {
    val reduced = rememberReducedMotion()
    val turn: androidx.compose.runtime.State<Float>? = if (reduced) null else {
        val transition = rememberInfiniteTransition(label = "running")
        transition.animateFloat(0f, 360f, infiniteRepeatable(tween(Motion.MARK_TURN_MS, easing = LinearEasing), RepeatMode.Restart), label = "turn")
    }
    // The turn is read while drawing, in a layer of its own: a turning mark redraws itself alone and recomposes
    // nothing, wherever it sits (kb PHN-711).
    Canvas(modifier.size(size).graphicsLayer()) {
        val w = stroke.toPx()
        val inset = (if (stroke < 2.5.dp) 1.dp else 2.dp).toPx() + w / 2
        drawArc(
            color, startAngle = turn?.value ?: 0f, sweepAngle = 0.3f * 360f, useCenter = false,
            topLeft = Offset(inset, inset), size = Size(this.size.width - 2 * inset, this.size.height - 2 * inset),
            style = Stroke(w, cap = StrokeCap.Round),
        )
    }
}

/** Mobile.swift Pill: black is the one thing to do, grey the other choice, plain a way out. */
enum class PillKind { BLACK, GREY, PLAIN }

@Composable
fun Pill(
    title: String,
    modifier: Modifier = Modifier,
    kind: PillKind = PillKind.BLACK,
    icon: ImageVector? = null,
    height: Dp = Phone.pillHeight,
    fill: Boolean = true,
    onClick: (() -> Unit)? = {},
) {
    // on FERB's surface the one thing to do is the black pill's inverse, the other choice the dark fill
    val dark = LocalOnFerb.current
    val fg = if ((kind == PillKind.BLACK) != dark) Ink.white else Ink.black
    val bg = when (kind) {
        PillKind.BLACK -> if (dark) Ink.white else Ink.black
        PillKind.GREY -> if (dark) Ink.darkFill else Ink.fill
        PillKind.PLAIN -> Color.Transparent
    }
    Row(
        modifier.then(if (fill) Modifier.fillMaxWidth() else Modifier).pressable(onClick).heightIn(min = height)
            .background(bg, RoundedCornerShape(Phone.pillCorner)).padding(horizontal = if (fill) 12.dp else 28.dp, vertical = 6.dp),
        horizontalArrangement = Arrangement.spacedBy(9.dp, Alignment.CenterHorizontally),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (icon != null) FerbIcon(icon, 17f, fg)
        PhoneText(title, 16f, FerbType.Semibold, fg, maxLines = 2)
    }
}

/** Mobile.swift SkyLabel: a label that sits straight on the sky, in the sky's ink. */
@Composable
fun SkyLabel(text: String, modifier: Modifier = Modifier) {
    PhoneText(
        text, 15f, FerbType.Semibold, LocalSkyInk.current.low,
        modifier.fillMaxWidth().padding(start = 8.dp, end = 8.dp, top = 22.dp, bottom = 8.dp).semantics { heading() },
    )
}

/** Mobile.swift CloseButton: an x in a 36 grey circle. */
@Composable
fun CloseButton(onClick: (() -> Unit)? = {}, modifier: Modifier = Modifier) {
    Box(
        modifier.pressable(onClick, label = "Close").size(Phone.closeButton).background(Ink.fill, CircleShape)
            .semantics { contentDescription = "Close" },
        contentAlignment = Alignment.Center,
    ) { FerbIcon(FerbIcons.close, 14f, Ink.black) }
}

/** Mobile.swift SheetIcon: what the question is about, in a 46 tile, with an amber dot while it asks. */
@Composable
fun SheetIcon(icon: ImageVector, asking: Boolean = true) {
    Box(Modifier.size(Phone.sheetIcon)) {
        Box(Modifier.size(Phone.sheetIcon).background(Ink.fill, RoundedCornerShape(Phone.sheetIconCorner)), contentAlignment = Alignment.Center) {
            FerbIcon(icon, 20f, Ink.black)
        }
        if (asking) {
            // A 12 dot offset (3, -3) with a 3-wide white ring centred on its edge: 15 across, 9 of it amber.
            Box(
                Modifier.align(Alignment.TopEnd).offset(4.5.dp, (-4.5).dp).size(15.dp)
                    .background(Color.White, CircleShape).padding(3.dp).background(Ink.amber, CircleShape)
                    .semantics { contentDescription = "Waiting for you" },
            )
        }
    }
}

/** Mobile.swift Switch: 52 by 32, black when on; the knob 24. */
@Composable
fun PhoneSwitch(on: Boolean, onChange: ((Boolean) -> Unit)? = null, label: String? = null) {
    // the knob's place is read while placing it: its slide recomposes nothing (kb PHN-711)
    val position = animateFloatAsState(if (on) 1f else 0f, Motion.toggle.spec(), label = "switch")
    Box(
        Modifier
            .then(if (onChange != null) Modifier.clickable(role = Role.Switch, onClickLabel = label) { onChange(!on) } else Modifier)
            .semantics { stateDescription = if (on) "On" else "Off" }
            .size(Phone.switchWidth, Phone.switchHeight)
            .background(if (on) Ink.black else LabColors.switchOff, CircleShape),
    ) {
        Box(Modifier.offset { androidx.compose.ui.unit.IntOffset((4 + 20 * position.value).dp.roundToPx(), 4.dp.roundToPx()) }.size(24.dp).background(Color.White, CircleShape))
    }
}
