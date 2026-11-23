package com.ferb.design.components

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.foundation.verticalScroll
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.Lamp
import com.ferb.design.LabColors
import com.ferb.design.Phone

// The owner's coding sessions (kb HAND-651..666): Claude Code and Codex on his Mac, in any app. A row of the Sessions tab, its
// state chip, and the pieces of one session's screen: the conversation's lines, the agent's question as a card, the
// composer. Stateless: the app passes the words and the taps. The agent's words are data: plain text, never links.

/** What a session is doing (`status`): working, waiting for the owner, or idle. */
enum class SessionState { WORKING, WAITING, IDLE }

/** One session as its row shows it: title (or folder), "<folder> · in <app>", its state, and FERB's badge when FERB keeps it going. */
data class SessionRowData(
    val title: String,
    val subtitle: String,
    val state: SessionState,
    val stateText: String,
    val ferb: String? = null,
)

/** The state as a small chip: a turning arc while it works, amber while it waits for the owner, quiet when idle. */
@Composable
fun SessionChip(state: SessionState, text: String, modifier: Modifier = Modifier) {
    val bg = when (state) {
        SessionState.WAITING -> Ink.amber
        else -> CardInk.current().fill
    }
    val ink = when (state) {
        SessionState.WAITING -> Ink.black
        SessionState.WORKING -> CardInk.current().primary
        SessionState.IDLE -> CardInk.current().secondary
    }
    Row(
        modifier.heightIn(min = 24.dp).background(bg, CircleShape).padding(start = 8.dp, end = 10.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        when (state) {
            SessionState.WORKING -> RunningArc(12.dp, ink, stroke = 1.8.dp)
            SessionState.WAITING -> Box(Modifier.size(7.dp).background(Ink.black, CircleShape))
            SessionState.IDLE -> Box(Modifier.size(7.dp).background(LabColors.waitingRing, CircleShape))
        }
        PhoneText(text, 12.5f, FerbType.Semibold, ink, maxLines = 1)
    }
}

/** "FERB is on it": FERB keeps this session going for the owner (a hand-over). */
@Composable
fun FerbBadge(text: String, modifier: Modifier = Modifier) {
    val dark = LocalOnFerb.current
    Row(
        modifier.heightIn(min = 24.dp).background(if (dark) Ink.white else Ink.black, CircleShape).padding(start = 8.dp, end = 10.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        LampDot(Ink.amber, 7.dp)
        PhoneText(text, 12.5f, FerbType.Semibold, if (dark) Ink.black else Ink.white, maxLines = 1)
    }
}

/** A session's row in the Sessions tab's card: the terminal mark, its title and place, its state, and it opens. */
@Composable
fun SessionRow(row: SessionRowData, onClick: (() -> Unit)? = null) {
    Row(
        Modifier.fillMaxWidth().pressable(onClick).semantics(mergeDescendants = true) {}
            .padding(horizontal = Phone.rowPaddingH, vertical = Phone.rowPaddingV),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(Phone.rowLeading), contentAlignment = Alignment.Center) { RowIcon(FerbIcons.terminal) }
        Spacer(Modifier.width(Phone.rowGap))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            val ink = CardInk.current()
            PhoneText(row.title, 16.5f, FerbType.Semibold, ink.primary, maxLines = 2)
            PhoneText(row.subtitle, 14f, color = ink.secondary, maxLines = 1)
            Row(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                SessionChip(row.state, row.stateText)
                row.ferb?.let { FerbBadge(it) }
            }
        }
        Spacer(Modifier.width(8.dp))
        Opens()
    }
}

/** What a line of a session's conversation is: the owner's words, the agent's, one tool it ran, a stop, an error. */
enum class SessionLineKind { OWNER, AGENT, TOOL, INTERRUPTED, ERROR }

/**
 * One line of the conversation ([key] stays the same while it is on screen: its sequence number). [day]: the words of
 * its day's start ("Today", "Yesterday", the date) when it is the first line of a day, as the main chat marks days.
 * An agent's reply (HAND-703): [parts] when the Mac sent them (one reply block holding them in order), else [blocks],
 * its words read as Markdown (Claude Code, Codex, an app's chat); neither: [text] as it is (a terminal's output).
 */
data class SessionLine(
    val key: String,
    val kind: SessionLineKind,
    val text: String,
    val day: String? = null,
    val blocks: List<RichBlock>? = null,
    val parts: List<SessionPartView> = emptyList(),
)

/** What a part of an agent's reply is (HAND-703): its words, a thought, a step it ran, the turn's small meta line. */
enum class SessionPartKind { TEXT, THINKING, TOOL, META }

/**
 * One part of a reply as drawn: a text part's [blocks] (its Markdown, read by the phone library); a thought's or a
 * step's [label] ("Thought 1s", "List zip sizes and contents") and a step's [detail] (what it ran); [text] what opens
 * under a thought or a step (nothing opens when it is empty), or the meta line's words.
 */
@Immutable
data class SessionPartView(
    val kind: SessionPartKind,
    val blocks: List<RichBlock> = emptyList(),
    val text: String = "",
    val label: String = "",
    val detail: String = "",
)

/** The words of a reply on the session's dark fill: white, links light blue, code on black. */
val onSessionFill = RichLook(15.5f, Ink.white, Ink.onDark, androidx.compose.ui.graphics.Color(0xFF9CC7FF), Ink.black)

/**
 * One line on the session screen's black page: the owner's words on white at the right, the agent's on the dark fill
 * at the left, a tool it ran as one small fixed-width line, its turn stopped and an error as quiet lines.
 */
@Composable
fun SessionLineView(line: SessionLine, modifier: Modifier = Modifier) {
    when (line.kind) {
        SessionLineKind.OWNER -> OwnerBubble(line.text, modifier)
        SessionLineKind.AGENT -> if (line.parts.isNotEmpty()) SessionReply(line.key, line.parts, modifier)
        else if (line.blocks != null) Box(modifier.fillMaxWidth(), contentAlignment = Alignment.CenterStart) {
            // its Markdown as the main chat draws FERB's (Rich.kt): selectable, a wide table scrolls inside the bubble
            Column(
                Modifier.widthIn(max = Phone.bubbleMaxFerb)
                    .background(Ink.darkFill, RoundedCornerShape(Phone.bubbleCorner, Phone.bubbleCorner, Phone.bubbleCorner, Phone.bubbleTail))
                    .padding(horizontal = 16.dp, vertical = 11.dp).testTag("session-rich"),
            ) { RichBlocks(line.blocks, onSessionFill, "session:" + line.key) }
        }
        else Box(modifier.fillMaxWidth(), contentAlignment = Alignment.CenterStart) {
            // its words can be selected and copied, as FERB's answers in the main chat (Rich.kt)
            androidx.compose.foundation.text.selection.SelectionContainer {
                PhoneText(
                    line.text, 15.5f, color = Ink.white, lineSpacing = 2f,
                    modifier = Modifier.widthIn(max = Phone.bubbleMaxFerb)
                        .background(Ink.darkFill, RoundedCornerShape(Phone.bubbleCorner, Phone.bubbleCorner, Phone.bubbleCorner, Phone.bubbleTail))
                        .padding(horizontal = 16.dp, vertical = 11.dp),
                )
            }
        }
        SessionLineKind.TOOL -> BasicText(
            line.text, modifier.fillMaxWidth().padding(horizontal = 6.dp),
            style = FerbType.style(13f, color = Ink.onDark).copy(fontFamily = FontFamily.Monospace, letterSpacing = 0.sp),
            maxLines = 3, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
        )
        SessionLineKind.INTERRUPTED -> PhoneText(line.text, 13.5f, FerbType.Medium, Ink.onDark, modifier.fillMaxWidth().padding(horizontal = 6.dp),
            align = TextAlign.Center)
        SessionLineKind.ERROR -> Row(modifier.fillMaxWidth().padding(horizontal = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically) {
            LampDot(Lamp.red, 7.dp)
            PhoneText(line.text, 14f, color = Lamp.red, modifier = Modifier.weight(1f))
        }
    }
}

/**
 * An agent's reply with its parts (HAND-703), as the main chat draws one reply: one block on the dark fill holding its
 * words (Markdown: tables as grids that scroll sideways inside it, code in a fixed-width face), its thoughts and the
 * steps it ran as small rows that open on a tap (only when there is something to open), in order; the turn's meta
 * line ("Just now", "Working…") small and dim under the block, never in it.
 */
@Composable
fun SessionReply(key: String, parts: List<SessionPartView>, modifier: Modifier = Modifier) {
    val inside = parts.filter { it.kind != SessionPartKind.META }
    val meta = parts.filter { it.kind == SessionPartKind.META && it.text.isNotBlank() }
    Column(modifier.fillMaxWidth().testTag("session-reply"), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        if (inside.isNotEmpty()) {
            Column(
                Modifier.fillMaxWidth()
                    .background(Ink.darkFill, RoundedCornerShape(Phone.bubbleCorner, Phone.bubbleCorner, Phone.bubbleCorner, Phone.bubbleTail))
                    .padding(horizontal = 16.dp, vertical = 12.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                inside.forEachIndexed { i, p ->
                    when (p.kind) {
                        SessionPartKind.TEXT -> if (p.blocks.isNotEmpty()) RichBlocks(p.blocks, onSessionFill, "session:$key:$i")
                        SessionPartKind.THINKING, SessionPartKind.TOOL -> SessionStepRow(p, "session:$key:$i")
                        SessionPartKind.META -> Unit
                    }
                }
            }
        }
        for (m in meta) PhoneText(m.text, 12.5f, FerbType.Medium, Ink.onDark, Modifier.padding(horizontal = 6.dp).testTag("session-meta"), maxLines = 1)
    }
}

/**
 * A thought or a step it ran, one small row: a thought's label dim ("Thought 1s"); a step's label then what it ran
 * dimmer on the same line ("List zip sizes and contents · ls, echo, unzip, head"). Closed at first; a chevron and a
 * tap only when there is something to open: a step's output in a fixed-width block, a thought's words.
 */
@Composable
fun SessionStepRow(part: SessionPartView, key: String, modifier: Modifier = Modifier) {
    val tool = part.kind == SessionPartKind.TOOL
    val opens = part.text.isNotBlank()
    var open by androidx.compose.runtime.saveable.rememberSaveable(key) { androidx.compose.runtime.mutableStateOf(false) }
    Column(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(
            Modifier.fillMaxWidth().pressable(if (opens) ({ open = !open }) else null, label = part.label)
                .heightIn(min = 24.dp).testTag(if (tool) "session-step" else "session-thought"),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (tool) FerbIcon(FerbIcons.terminal, 13f, Ink.onDark)
            val words = androidx.compose.ui.text.buildAnnotatedString {
                pushStyle(androidx.compose.ui.text.SpanStyle(color = if (tool) Ink.white else Ink.onDark))
                append(part.label)
                pop()
                if (tool && part.detail.isNotBlank()) {
                    pushStyle(androidx.compose.ui.text.SpanStyle(color = Ink.onDark))
                    append(" · ")
                    append(part.detail)
                    pop()
                }
            }
            BasicText(words, Modifier.weight(1f, fill = false), style = FerbType.style(13.5f, FerbType.Medium, Ink.onDark),
                maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis)
            if (opens) FerbIcon(if (open) FerbIcons.chevronUp else FerbIcons.chevronDown, 15f, Ink.onDark)
        }
        if (opens && open) {
            if (tool) {
                Box(
                    Modifier.fillMaxWidth().heightIn(max = 320.dp).background(Ink.black, RoundedCornerShape(10.dp))
                        .verticalScroll(rememberScrollState()).horizontalScroll(rememberScrollState()).padding(10.dp)
                        .testTag("session-step-output"),
                ) {
                    androidx.compose.foundation.text.selection.SelectionContainer {
                        BasicText(part.text, style = FerbType.style(12.5f, color = Ink.onDark).copy(fontFamily = FontFamily.Monospace, letterSpacing = 0.sp))
                    }
                }
            } else {
                androidx.compose.foundation.text.selection.SelectionContainer {
                    PhoneText(part.text, 13.5f, color = Ink.onDark, lineSpacing = 2f, modifier = Modifier.testTag("session-thought-text"))
                }
            }
        }
    }
}

/**
 * FERB's latest word on a hand-over (HAND-702), exactly as the Mac says it: one quiet line with FERB's mark, pinned at
 * the end of the conversation above the message box (or under the header when there is none).
 */
@Composable
fun FerbNoteLine(text: String, modifier: Modifier = Modifier) {
    Row(
        modifier.fillMaxWidth().semantics(mergeDescendants = true) { liveRegion = LiveRegionMode.Polite }.testTag("session-ferb-note"),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        FerbMark(18.dp)
        PhoneText(text, 13.5f, FerbType.Medium, Ink.onDark, Modifier.weight(1f), maxLines = 3, lineSpacing = 1f)
    }
}

/**
 * The agent waits for the owner (a permission prompt or a question): what it asks, the exact thing in a fixed-width
 * face, and one button per answer it offers. No answers: [onMac] says to answer it on the Mac.
 */
data class SessionAsk(val title: String, val question: String, val detail: String, val choices: List<String>, val onMac: String, val key: String = title)

@Composable
fun SessionAskCard(ask: SessionAsk, modifier: Modifier = Modifier, onChoice: ((Int) -> Unit)? = null) {
    Card(modifier, padding = 16.dp) {
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
                SheetIcon(FerbIcons.terminal)
                PhoneText(ask.title, 17f, FerbType.Semibold, Ink.black, Modifier.weight(1f))
            }
            if (ask.question.isNotBlank()) PhoneText(ask.question, 15.5f, color = Ink.black, lineSpacing = 2f)
            if (ask.detail.isNotBlank()) ExactCommand(ask.detail)
            if (ask.choices.isEmpty()) {
                PhoneText(ask.onMac, 14.5f, color = Ink.detail)
            } else {
                ask.choices.forEachIndexed { i, label ->
                    Pill(label, kind = if (i == 0) PillKind.BLACK else PillKind.GREY, height = 46.dp, onClick = onChoice?.let { { it(i) } })
                }
            }
        }
    }
}

/**
 * One of the session screen's actions, in its menu (Watch, Hand to FERB or Take it back, Stop its turn, Stop
 * controlling). [busy]: under way (a small turning mark, no tap); [keepOpen]: the menu stays open after the tap, until
 * the Mac answers (Take it back, HAND-702).
 */
data class SessionAction(
    val label: String,
    val icon: ImageVector?,
    val onClick: (() -> Unit)?,
    val busy: Boolean = false,
    val keepOpen: Boolean = false,
    val tag: String? = null,
)

/**
 * His message to a session that its conversation doesn't hold yet, one bubble for each way it stands (PHN-812,
 * PHN-800): [sending] ("Sending…" under it, a turning mark) while FERB types it on the Mac; sent (neither: his plain
 * bubble) until the session's own record of it arrives; refused ([reason]: dimmed, "[notSent] [reason]", and [retry]).
 */
data class SessionOutgoing(
    val text: String,
    val sending: Boolean,
    val reason: String? = null,
    val sendingLabel: String = "",
    val retry: String = "",
    val notSent: String = "",
)

@Composable
fun SessionOutgoingView(u: SessionOutgoing, modifier: Modifier = Modifier, onRetry: (() -> Unit)? = null) {
    Column(modifier.fillMaxWidth(), horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(if (u.reason != null) 8.dp else 4.dp)) {
        // faded until the Mac typed it, as the main chat's outbox (Main.kt OutboxRow); more when refused
        OwnerBubble(u.text, Modifier.alpha(when { u.reason != null -> 0.5f; u.sending -> 0.7f; else -> 1f }))
        when {
            u.reason != null -> {
                Row(Modifier.padding(start = 40.dp, end = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Top) {
                    LampDot(Lamp.red, 7.dp, Modifier.padding(top = 6.dp))
                    PhoneText("${u.notSent} ${u.reason}", 14f, color = Ink.onDark, lineSpacing = 2f)
                }
                FerbSurface { Pill(u.retry, Modifier.padding(end = 2.dp), PillKind.GREY, height = 36.dp, fill = false, onClick = onRetry) }
            }
            u.sending -> Row(
                Modifier.padding(horizontal = 6.dp).semantics(mergeDescendants = true) { liveRegion = LiveRegionMode.Polite },
                horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically,
            ) {
                RunningArc(12.dp, Ink.onDark, stroke = 1.8.dp)
                PhoneText(u.sendingLabel, 13.5f, FerbType.Medium, Ink.onDark)
            }
        }
    }
}

/** The session's menu (PHN-800): where it runs, then its actions, one per row, in a white card. */
@Composable
fun SessionMenu(place: String, actions: List<SessionAction>, modifier: Modifier = Modifier, onPicked: () -> Unit = {}) {
    Card(modifier.widthIn(min = 220.dp, max = 280.dp), padding = 6.dp) {
        if (place.isNotBlank()) PhoneText(place, 13.5f, color = Ink.detail, maxLines = 2, modifier = Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, top = 10.dp, bottom = 6.dp))
        actions.forEachIndexed { i, a ->
            if (i > 0 || place.isNotBlank()) Divide()
            Row(
                Modifier.fillMaxWidth().pressable(a.onClick?.takeIf { !a.busy }?.let { f -> { if (!a.keepOpen) onPicked(); f() } }, label = a.label)
                    .heightIn(min = 48.dp).padding(horizontal = 14.dp).then(if (a.tag != null) Modifier.testTag(a.tag) else Modifier)
                    .then(if (a.busy) Modifier.semantics(mergeDescendants = true) { liveRegion = LiveRegionMode.Polite } else Modifier),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                val ink = if (a.onClick != null && !a.busy) Ink.black else Ink.detail
                Box(Modifier.size(20.dp), contentAlignment = Alignment.Center) {
                    if (a.busy) RunningArc(16.dp, Ink.detail, stroke = 2.dp) else a.icon?.let { FerbIcon(it, 17f, ink) }
                }
                PhoneText(a.label, 16f, FerbType.Medium, ink, maxLines = 1)
            }
        }
    }
}

/** The session header's round buttons: its menu (an ellipsis) beside Close, the same grey round. */
@Composable
fun MoreButton(label: String, open: Boolean, modifier: Modifier = Modifier, onClick: (() -> Unit)?) {
    Box(
        modifier.pressable(onClick, label = label).size(Phone.closeButton)
            .background(if (open) Ink.white else Ink.fill, CircleShape)
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) { FerbIcon(FerbIcons.more, 18f, Ink.black) }
}

/** The session screen's buttons: small dark pills in one row that scrolls sideways when they don't fit. */
@Composable
fun SessionActions(actions: List<SessionAction>, modifier: Modifier = Modifier) {
    Row(
        modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        for (a in actions) {
            Row(
                Modifier.pressable(a.onClick).heightIn(min = 38.dp).background(Ink.darkFill, RoundedCornerShape(Phone.pillCorner))
                    .padding(horizontal = 14.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                val ink = if (a.onClick != null) Ink.white else Ink.onDark
                a.icon?.let { FerbIcon(it, 14f, ink) }
                PhoneText(a.label, 14.5f, FerbType.Semibold, ink, maxLines = 1)
            }
        }
    }
}

/**
 * The session screen's message box (kb HAND-651..666): the owner's words to the agent, several lines, and Send. Never a
 * terminal's keyboard: what is sent is what the box holds. Without [onValueChange] it only shows [value] (screenshots).
 * While his last message is on its way ([sending], PHN-812) the send button turns, never a frozen arrow, and takes no tap.
 */
@Composable
fun SessionComposer(
    value: String,
    placeholder: String,
    sendLabel: String,
    modifier: Modifier = Modifier,
    onValueChange: ((String) -> Unit)? = null,
    onSend: (() -> Unit)? = null,
    focus: FocusRequester? = null,
    sending: Boolean = false,
) {
    Row(
        modifier.fillMaxWidth().heightIn(min = Phone.composerHeight).background(Ink.darkFill, RoundedCornerShape(Phone.composerHeight / 2))
            .padding(start = 18.dp, end = 8.dp, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.Bottom,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Box(Modifier.weight(1f).heightIn(min = Phone.composerButton), contentAlignment = Alignment.CenterStart) {
            val style = FerbType.style(16.5f, color = Ink.white)
            if (onValueChange != null) {
                BasicTextField(
                    value, onValueChange,
                    Modifier.fillMaxWidth().then(if (focus != null) Modifier.focusRequester(focus) else Modifier)
                        .semantics { contentDescription = placeholder },
                    textStyle = style, cursorBrush = SolidColor(Ink.onDark), maxLines = 6,
                )
            } else if (value.isNotEmpty()) {
                PhoneText(value, 16.5f, color = Ink.white, maxLines = 6)
            }
            if (value.isEmpty()) PhoneText(placeholder, 16.5f, color = Ink.onDark, maxLines = 1)
        }
        Box(
            Modifier.pressable(onSend?.takeIf { value.isNotBlank() && !sending }, label = sendLabel).semantics { contentDescription = sendLabel }
                .size(Phone.composerButton).background(if (value.isNotBlank() && !sending) Ink.white else Ink.onDark, CircleShape)
                .testTag(if (sending) "session-send-busy" else "session-send"),
            contentAlignment = Alignment.Center,
        ) { if (sending) RunningArc(18.dp, Ink.black, stroke = 2.dp) else FerbIcon(FerbIcons.arrowUp, 17f, Ink.black) }
    }
}
