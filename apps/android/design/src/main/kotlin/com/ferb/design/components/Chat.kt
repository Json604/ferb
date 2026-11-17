package com.ferb.design.components

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.withFrameMillis
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.draw.drawBehind
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.Phone
import com.ferb.design.rememberReducedMotion
import kotlin.math.max
import kotlin.math.sin

// The conversation (MobileScreens.swift ChatPage, Conversation.swift): the owner's words on white, FERB's on black,
// a task's parts in one white card with Watch live and Stop.

/** The owner's bubble: white, its tail corner at the bottom right. */
@Composable
fun OwnerBubble(text: String, modifier: Modifier = Modifier) {
    Box(modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) {
        PhoneText(
            text, 16f, color = Ink.black, lineSpacing = 2f,
            modifier = Modifier.widthIn(max = Phone.bubbleMaxOwner)
                .background(Ink.white, RoundedCornerShape(Phone.bubbleCorner, Phone.bubbleCorner, Phone.bubbleTail, Phone.bubbleCorner))
                .padding(horizontal = 16.dp, vertical = 11.dp)
                .semantics { contentDescription = "You: $text" },
        )
    }
}

/** FERB's bubble: black, its tail corner at the bottom left. */
@Composable
fun FerbBubble(text: String, modifier: Modifier = Modifier, maxWidth: Dp = Phone.bubbleMaxFerb) {
    Box(modifier.fillMaxWidth(), contentAlignment = Alignment.CenterStart) {
        PhoneText(
            text, 16f, color = Ink.white, lineSpacing = 2f,
            modifier = Modifier.widthIn(max = maxWidth).background(Ink.black, ferbBubbleShape)
                .padding(horizontal = 16.dp, vertical = 11.dp)
                .semantics { contentDescription = "FERB: $text" },
        )
    }
}

private val ferbBubbleShape = RoundedCornerShape(Phone.bubbleCorner, Phone.bubbleCorner, Phone.bubbleCorner, Phone.bubbleTail)

/** Conversation.swift TypingDots: FERB writing, three dots rising one after another in a black bubble. */
@Composable
fun TypingDots(modifier: Modifier = Modifier, timeMillis: Long? = null) {
    val reduced = rememberReducedMotion()
    var now by remember { mutableLongStateOf(timeMillis ?: 0L) }
    if (timeMillis == null && !reduced) LaunchedEffect(Unit) { while (true) withFrameMillis { now = it } }
    val t = (timeMillis ?: now) / 1000.0
    Row(
        modifier.background(Ink.black, ferbBubbleShape).padding(horizontal = 16.dp, vertical = 15.dp)
            .semantics { contentDescription = "FERB is writing"; liveRegion = LiveRegionMode.Polite },
        horizontalArrangement = Arrangement.spacedBy(5.dp),
    ) {
        for (i in 0 until 3) {
            val lift = if (reduced) 0.0 else -3 * max(0.0, sin(t * 6 - i * 0.7))
            Box(Modifier.offset(y = lift.dp).size(7.dp).background(Color.White.copy(alpha = 0.85f), CircleShape))
        }
    }
}

/** One part of a task: its mark, its title and what it is doing. */
data class PlanPart(val kind: MarkKind, val title: String, val detail: String? = null)

/**
 * MobileScreens.swift ChatPage.task: the plan line and its count, the parts (a running one on a grey fill), and Watch
 * live and Stop while anything runs or asks.
 */
@Composable
fun TaskCard(
    headline: String,
    progress: String,
    parts: List<PlanPart>,
    modifier: Modifier = Modifier,
    onWatch: (() -> Unit)? = {},
    onStop: (() -> Unit)? = {},
) {
    Column(modifier.fillMaxWidth().background(Ink.white, RoundedCornerShape(Phone.taskCorner)).padding(8.dp)) {
        Row(Modifier.fillMaxWidth().padding(start = 10.dp, end = 10.dp, top = 8.dp, bottom = 4.dp)) {
            PhoneText(headline, 14f, FerbType.Medium, Ink.detail, Modifier.weight(1f))
            PhoneText(progress, 14f, FerbType.Medium, Ink.detail, Modifier.semantics { liveRegion = LiveRegionMode.Polite })
        }
        for (part in parts) {
            Row(
                Modifier.fillMaxWidth()
                    .then(if (part.kind == MarkKind.RUNNING) Modifier.background(Ink.fill, RoundedCornerShape(Phone.partCorner)) else Modifier)
                    .semantics(mergeDescendants = true) {}
                    .padding(10.dp),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Mark(part.kind)
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(1.dp)) {
                    val waiting = part.kind == MarkKind.WAITING
                    PhoneText(part.title, 15.5f, if (waiting) FerbType.Regular else FerbType.Semibold, if (waiting) Ink.faint else Ink.black)
                    if (part.detail != null) PhoneText(part.detail, 14f, color = Ink.detail)
                }
            }
        }
        if (parts.any { it.kind == MarkKind.RUNNING || it.kind == MarkKind.ASKING }) {
            Row(Modifier.fillMaxWidth().padding(start = 2.dp, end = 2.dp, top = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Pill("Watch live", Modifier.weight(1f), PillKind.GREY, FerbIcons.eye, 46.dp, onClick = onWatch)
                Pill("Stop", Modifier.weight(1f), PillKind.BLACK, FerbIcons.stop, 46.dp, onClick = onStop)
            }
        }
    }
}

/** The microphone's state as the composer draws it (kb PHN-643). */
enum class MicState { IDLE, READYING, LISTENING, FINISHING }

/**
 * Mobile.swift PhoneComposer: a black capsule with + and the send arrow. With [onValueChange] it is a working text
 * field; without it shows [typed] with the lab's caret, or the placeholder.
 *
 * Hold to speak (kb PHN-603): with [onMicDown] the microphone works while it is held: [onMicDown] when the finger goes
 * down, [onMicUp] when it lifts or the touch is taken away; [onMic] is then unused. While [listening] the microphone
 * sits in a white circle (the send button's) and an empty box reads [listeningText]. The defaults draw as before.
 *
 * What the owner sees of the microphone (kb PHN-643): a lighter circle the moment the finger goes down (before FERB
 * answers), [mic] while FERB gets ready, listens and finishes, and while it listens a ring around the circle that
 * grows with [micLevel] (0..1, how loud the phone hears him). A finger that slides off the button keeps the
 * microphone until it lifts.
 */
@Composable
fun PhoneComposer(
    modifier: Modifier = Modifier,
    placeholder: String = "Ask FERB",
    typed: String? = null,
    onValueChange: ((String) -> Unit)? = null,
    onSend: (() -> Unit)? = {},
    onAttach: (() -> Unit)? = {},
    onMic: (() -> Unit)? = {},
    onMicDown: (() -> Unit)? = null,
    onMicUp: (() -> Unit)? = null,
    listening: Boolean = false,
    listeningText: String = placeholder,
    micLabel: String = "Speak",
    mic: MicState = if (listening) MicState.LISTENING else MicState.IDLE,
    micLevel: Float = 0f,
) {
    Row(
        modifier.fillMaxWidth().heightIn(min = Phone.composerHeight).background(Ink.black, CircleShape).padding(horizontal = 8.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Box(
            Modifier.pressable(onAttach, label = "Attach").semantics { contentDescription = "Attach" }
                .size(Phone.composerButton).background(Ink.darkFill, CircleShape),
            contentAlignment = Alignment.Center,
        ) { FerbIcon(FerbIcons.plus, 18f, Ink.white) }
        Box(Modifier.weight(1f), contentAlignment = Alignment.CenterStart) {
            val style = FerbType.style(16.5f, color = Ink.white)
            if (onValueChange != null) {
                BasicTextField(
                    typed.orEmpty(), onValueChange, Modifier.fillMaxWidth().semantics { contentDescription = placeholder },
                    textStyle = style, cursorBrush = SolidColor(Ink.onDark), maxLines = 4,
                )
                if (typed.isNullOrEmpty()) PhoneText(if (listening) listeningText else placeholder, 16.5f, color = Ink.onDark, maxLines = 1)
            } else if (!typed.isNullOrEmpty()) {
                // The lab shows the end of what was typed, with a caret.
                PhoneText("$typed|", 16.5f, color = Ink.white, maxLines = 1, overflow = TextOverflow.Clip)
            } else {
                PhoneText(placeholder, 16.5f, color = Ink.onDark, maxLines = 1)
            }
        }
        if (onMicDown == null) {
            Box(Modifier.pressable(onMic, label = micLabel).semantics { contentDescription = micLabel }, contentAlignment = Alignment.Center) {
                FerbIcon(FerbIcons.mic, 19f, Ink.white)
            }
        } else {
            val down by rememberUpdatedState(onMicDown)
            val up by rememberUpdatedState(onMicUp)
            var pressed by remember { mutableStateOf(false) }
            val shown = if (mic == MicState.IDLE && pressed) MicState.READYING else mic
            val level by animateFloatAsState(if (shown == MicState.LISTENING) micLevel.coerceIn(0f, 1f) else 0f, tween(90), label = "mic level")
            val grow by animateFloatAsState(if (shown == MicState.IDLE) 1f else 1.12f, tween(120), label = "mic press")
            Box(
                // the circle and its ring are drawn behind the icon, so the composer's layout is the lab's in every state
                Modifier.drawBehind {
                    val r = Phone.composerButton.toPx() / 2
                    when (shown) {
                        MicState.IDLE -> Unit
                        MicState.READYING -> drawCircle(Ink.white.copy(alpha = 0.55f), radius = r * grow)
                        MicState.LISTENING -> {
                            drawCircle(Ink.white.copy(alpha = 0.22f + 0.2f * level), radius = r * (grow + 0.08f + 0.25f * level))
                            drawCircle(Ink.white, radius = r * grow)
                        }
                        MicState.FINISHING -> drawCircle(Ink.white.copy(alpha = 0.8f), radius = r)
                    }
                }
                    .semantics { contentDescription = micLabel }
                    .pointerInput(Unit) {
                        awaitEachGesture {
                            awaitFirstDown(requireUnconsumed = false)
                            pressed = true
                            down()
                            // held until the finger lifts, wherever it slides; a gesture the system takes ends it too
                            do {
                                val event = awaitPointerEvent()
                                event.changes.forEach { it.consume() }
                            } while (event.changes.any { it.pressed })
                            pressed = false
                            up?.invoke()
                        }
                    },
                contentAlignment = Alignment.Center,
            ) { FerbIcon(FerbIcons.mic, 19f, if (shown == MicState.IDLE) Ink.white else Ink.black) }
        }
        Box(
            Modifier.pressable(onSend, label = "Send").semantics { contentDescription = "Send" }
                .size(Phone.composerButton).background(Ink.white, CircleShape),
            contentAlignment = Alignment.Center,
        ) { FerbIcon(FerbIcons.arrowUp, 17f, Ink.black) }
    }
}
