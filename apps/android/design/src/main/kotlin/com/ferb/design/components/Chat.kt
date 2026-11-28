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
import androidx.compose.ui.focus.focusRequester
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
import androidx.compose.foundation.border
import androidx.compose.ui.draw.clip
import androidx.compose.foundation.layout.fillMaxSize
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

/**
 * The owner's bubble: white, its tail corner at the bottom right. [bubble] goes on the bubble itself, before its
 * background (the reply swipe, kb APP-1146).
 */
@Composable
fun OwnerBubble(text: String, modifier: Modifier = Modifier, bubble: Modifier = Modifier) {
    Box(modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) {
        PhoneText(
            text, 16f, color = Ink.black, lineSpacing = 2f,
            modifier = bubble.widthIn(max = Phone.bubbleMaxOwner)
                .background(Ink.white, OwnerBubbleShape)
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

/** The owner's bubble's shape (its tail corner at the bottom right). */
val OwnerBubbleShape = RoundedCornerShape(Phone.bubbleCorner, Phone.bubbleCorner, Phone.bubbleTail, Phone.bubbleCorner)

private val ferbBubbleShape = RoundedCornerShape(Phone.bubbleCorner, Phone.bubbleCorner, Phone.bubbleCorner, Phone.bubbleTail)

/** Conversation.swift TypingDots: FERB writing, three dots rising one after another in a black bubble. */
@Composable
fun TypingDots(modifier: Modifier = Modifier, timeMillis: Long? = null) {
    val reduced = rememberReducedMotion()
    var now by remember { mutableLongStateOf(timeMillis ?: 0L) }
    if (timeMillis == null && !reduced) LaunchedEffect(Unit) { while (true) withFrameMillis { now = it } }
    Row(
        modifier.background(Ink.black, ferbBubbleShape).padding(horizontal = 16.dp, vertical = 15.dp)
            .semantics { contentDescription = "FERB is writing"; liveRegion = LiveRegionMode.Polite },
        horizontalArrangement = Arrangement.spacedBy(5.dp),
    ) {
        for (i in 0 until 3) {
            // the clock is read while placing the dot (the same whole pixels offset(y) gives): the dots rise and fall
            // without recomposing or measuring anything on any frame (kb PHN-711)
            Box(
                Modifier.offset {
                    val t = (timeMillis ?: now) / 1000.0
                    val lift = if (reduced) 0.0 else -3 * max(0.0, sin(t * 6 - i * 0.7))
                    androidx.compose.ui.unit.IntOffset(0, lift.dp.roundToPx())
                }.size(7.dp).background(Color.White.copy(alpha = 0.85f), CircleShape),
            )
        }
    }
}

/**
 * One part of a task: its mark, its title and what it is doing. Not from the lab (kb PHN-733; FERB.app's CardRowView):
 * its own answer in rich text ([rich], folded past 8 lines to 3 with "Show more"), the colour its plain words take, its
 * lines under them ("Couldn't confirm", "Updated", "You chose"), whether it was updated, and [key] (where its fold is kept).
 */
@androidx.compose.runtime.Immutable
data class PlanPart(
    val kind: MarkKind,
    val title: String,
    val detail: String? = null,
    val rich: List<RichBlock>? = null,
    val tone: PartTone = PartTone.DIM,
    val notes: List<PartNote> = emptyList(),
    val updated: Boolean = false,
    val key: String = "",
    /** The steps a waiting part waits on that no part answers (APP-895), each its own line under it. */
    val sub: List<PlanPart> = emptyList(),
)

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
    /** While it runs, FERB.app's one status line ("1 of 4 done · 2 running · 1 waiting", CardStatusLine). */
    status: String? = null,
    /** Once it ended, FERB.app's footer: the done line in semibold and code's lines after it. */
    footer: String? = null,
    footerLines: List<String> = emptyList(),
    /** Not from the lab (kb PHN-726): which parts show their own live picture (FERB.app's RowPreview), its picture once
     *  one came, and what a tap on it does. */
    watching: List<Boolean> = emptyList(),
    pictures: List<androidx.compose.ui.graphics.ImageBitmap?> = emptyList(),
    onPicture: ((Int) -> Unit)? = null,
    more: String = "Show more",
    less: String = "Show less",
) {
    val ink = CardInk.current()
    Column(modifier.fillMaxWidth().background(ink.card, RoundedCornerShape(Phone.taskCorner)).padding(8.dp)) {
        Row(Modifier.fillMaxWidth().padding(start = 10.dp, end = 10.dp, top = 8.dp, bottom = 4.dp)) {
            PhoneText(headline, 14f, FerbType.Medium, ink.secondary, Modifier.weight(1f))
            PhoneText(progress, 14f, FerbType.Medium, ink.secondary, Modifier.semantics { liveRegion = LiveRegionMode.Polite })
        }
        parts.forEachIndexed { i, part ->
            Row(
                Modifier.fillMaxWidth()
                    .then(if (part.kind == MarkKind.RUNNING) Modifier.background(ink.fill, RoundedCornerShape(Phone.partCorner)) else Modifier)
                    .semantics(mergeDescendants = true) {}
                    .padding(10.dp),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Mark(part.kind)
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(1.dp)) {
                    val waiting = part.kind == MarkKind.WAITING
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                        PhoneText(part.title, 15.5f, if (waiting) FerbType.Regular else FerbType.Semibold, if (waiting) ink.faint else ink.primary)
                        if (part.updated) PhoneText("UPDATED", 10f, FerbType.Bold, Ink.white,
                            Modifier.background(ink.link, RoundedCornerShape(4.dp)).padding(horizontal = 5.dp))
                    }
                    if (part.detail != null) PhoneText(part.detail, 14f, color = part.tone.color(ink))
                    part.rich?.let { rich ->
                        // the part's own answer as FERB.app's row shows it: rich text, past 8 lines folded to 3
                        val look = RichLook(14.5f, ink.primary, ink.secondary, ink.link, ink.fill)
                        val text = androidx.compose.runtime.remember(rich, ink.onFerb) { richText(rich, look) }
                        RichWords(text, look, part.key, Fold.ROW, more = more, less = less, modifier = Modifier.padding(top = 2.dp))
                    }
                    for (n in part.notes) {
                        androidx.compose.foundation.text.BasicText(androidx.compose.ui.text.buildAnnotatedString {
                            n.label?.let { pushStyle(androidx.compose.ui.text.SpanStyle(color = n.tone.color(ink), fontWeight = FerbType.Semibold)); append(it); pop() }
                            pushStyle(androidx.compose.ui.text.SpanStyle(color = if (n.label == null) n.tone.color(ink) else ink.secondary)); append(n.text); pop()
                        }, style = FerbType.style(13.5f, color = ink.secondary), modifier = Modifier.padding(top = 2.dp))
                    }
                }
                if (watching.getOrElse(i) { false }) {
                    // the part's own live picture (FERB.app's RowPreview: a small circle with the amber ring, no dot)
                    val picture = pictures.getOrNull(i)
                    Box(Modifier.size(26.dp).clip(CircleShape).background(Ink.darkFill)
                        .border(1.dp, Ink.amber.copy(alpha = 0.65f), CircleShape)
                        .pressable(onPicture?.let { { it(i) } }, label = "Live preview of ${part.title}")) {
                        picture?.let { androidx.compose.foundation.Image(it, null, Modifier.fillMaxSize(), contentScale = androidx.compose.ui.layout.ContentScale.Crop) }
                    }
                }
            }
        }
        if (status != null) {
            PhoneText(status, 13.5f, FerbType.Medium, ink.secondary, Modifier.fillMaxWidth().padding(start = 10.dp, end = 10.dp, top = 6.dp)
                .semantics { liveRegion = LiveRegionMode.Polite }, maxLines = 1)
        }
        if (footer != null || footerLines.isNotEmpty()) {
            Column(Modifier.fillMaxWidth().padding(start = 10.dp, end = 10.dp, top = 8.dp, bottom = 4.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                footer?.let { PhoneText(it, 14.5f, FerbType.Semibold, ink.primary) }
                for (l in footerLines) PhoneText(l, 13.5f, color = ink.secondary)
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
    /** Files waiting in the box (kb PHN-690): drawn inside it, above the words; the box's capsule becomes a card. */
    attachments: (@Composable () -> Unit)? = null,
    /** Lets the app put the cursor in the box (a tap on FERB's question's notification, kb PHN-739). */
    focus: androidx.compose.ui.focus.FocusRequester? = null,
    /** The message he replies to (kb APP-1146, [ComposerQuote]): inside the box at its top, above its files. */
    quote: (@Composable () -> Unit)? = null,
) {
    if (attachments != null || quote != null) {
        Column(modifier.fillMaxWidth().background(Ink.black, RoundedCornerShape(Phone.composerHeight / 2)).padding(top = 8.dp)) {
            if (quote != null) Box(Modifier.padding(horizontal = 8.dp).padding(bottom = if (attachments != null) 6.dp else 0.dp)) { quote() }
            if (attachments != null) Box(Modifier.padding(horizontal = 8.dp)) { attachments() }
            ComposerRow(Modifier, placeholder, typed, onValueChange, onSend, onAttach, onMic, onMicDown, onMicUp, listening, listeningText, micLabel, mic, micLevel, focus)
        }
    } else {
        ComposerRow(modifier.background(Ink.black, CircleShape), placeholder, typed, onValueChange, onSend, onAttach, onMic, onMicDown, onMicUp,
            listening, listeningText, micLabel, mic, micLevel, focus)
    }
}

@Composable
private fun ComposerRow(
    modifier: Modifier,
    placeholder: String,
    typed: String?,
    onValueChange: ((String) -> Unit)?,
    onSend: (() -> Unit)?,
    onAttach: (() -> Unit)?,
    onMic: (() -> Unit)?,
    onMicDown: (() -> Unit)?,
    onMicUp: (() -> Unit)?,
    listening: Boolean,
    listeningText: String,
    micLabel: String,
    mic: MicState,
    micLevel: Float,
    focus: androidx.compose.ui.focus.FocusRequester? = null,
) {
    Row(
        modifier.fillMaxWidth().heightIn(min = Phone.composerHeight).padding(horizontal = 8.dp, vertical = 8.dp),
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
                    typed.orEmpty(), onValueChange,
                    Modifier.fillMaxWidth().then(if (focus != null) Modifier.focusRequester(focus) else Modifier).semantics { contentDescription = placeholder },
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
