package com.ferb.design.components

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.ui.draw.clip
import androidx.compose.foundation.interaction.collectIsDraggedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import kotlinx.coroutines.flow.first
import androidx.compose.foundation.gestures.animateScrollBy
import androidx.compose.foundation.gestures.scrollBy
import com.ferb.design.Motion
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.motionAppear

// The chat as a list (kb PHN-627, PHN-628): one continuous chat, a day's start between days, its end followed while
// the owner is at it, a "new message" pill when he scrolled up, and a waiting look while the first page comes.

/** A day's start in the chat: "Today", "Yesterday" or the date, small and centred in the sky's ink. */
@Composable
fun DayDivider(text: String, modifier: Modifier = Modifier) {
    val ink = LocalSkyInk.current
    Box(modifier.fillMaxWidth().padding(top = 6.dp, bottom = 2.dp).semantics { heading() }, contentAlignment = Alignment.Center) {
        PhoneText(text, 13f, FerbType.Semibold, ink.low)
    }
}

/** Over the chat's end when the owner scrolled up and something new arrived: a tap goes down to it. */
@Composable
fun NewMessagePill(text: String, modifier: Modifier = Modifier, onClick: () -> Unit) =
    JumpPill(text, text, modifier.motionAppear(), onClick)

/**
 * The way back to the chat's latest messages whenever the owner is not at its end (kb PHN-662), on the dark fill so it
 * stands apart from FERB's black messages under it: a down arrow, with
 * [text] beside it when there is something to say ("New message", or the latest messages' day, "Today", when he is in
 * an earlier day), the arrow alone when he is in the same day scrolled up. [label] is what a screen reader says.
 */
@Composable
fun JumpPill(text: String?, label: String, modifier: Modifier = Modifier, onClick: () -> Unit) {
    Row(
        modifier.pressable(onClick, label = label).background(Ink.darkFill, CircleShape)
            .padding(horizontal = if (text == null) 11.dp else 16.dp, vertical = 9.dp)
            .semantics(mergeDescendants = true) {
                liveRegion = LiveRegionMode.Polite
                if (text == null) contentDescription = label
            },
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        FerbIcon(FerbIcons.arrowDown, if (text == null) 16f else 13f, Ink.white)
        if (text != null) PhoneText(text, 14f, FerbType.Semibold, Ink.white, maxLines = 1)
    }
}

/** What the chat's jump control shows (kb PHN-662, PHN-698): see [chatJump]. */
enum class ChatJump {
    NONE, NEW_MESSAGE, DAY, LATEST,

    /** In the earlier days' chat: "Today" takes him back to today, from anywhere in it, its end too. */
    TODAY,
}

/**
 * In the earlier days' chat ([earlier]) always the way back to today, wherever he is in it, its end too (kb PHN-698).
 * In today's: nothing at the end ([atEnd]); "New message" when something arrived while he was away ([unseen]); the
 * latest messages' day when the day he is looking at ([shownDay], epoch days) is before it ([endDay]); else the arrow
 * alone.
 */
fun chatJump(atEnd: Boolean, unseen: Boolean, shownDay: Long?, endDay: Long?, earlier: Boolean = false): ChatJump = when {
    earlier -> ChatJump.TODAY
    atEnd -> ChatJump.NONE
    unseen -> ChatJump.NEW_MESSAGE
    shownDay != null && endDay != null && shownDay < endDay -> ChatJump.DAY
    else -> ChatJump.LATEST
}

/**
 * The day he is looking at, held at the top of the chat while he scrolls through it (kb PHN-663): the day divider's
 * words, on a small capsule of the dark fill so it reads over the messages under it and apart from them.
 */
@Composable
fun StickyDay(text: String, modifier: Modifier = Modifier) {
    Box(modifier.fillMaxWidth().semantics(mergeDescendants = true) { heading() }, contentAlignment = Alignment.Center) {
        Box(Modifier.background(Ink.darkFill, CircleShape).padding(horizontal = 12.dp, vertical = 4.dp)) {
            PhoneText(text, 13f, FerbType.Semibold, Ink.white, maxLines = 1)
        }
    }
}

/**
 * How a status line reads (kb PHN-660, PHN-661): [WORKING] something on its way that clears by itself (a dashed ring,
 * the lab's waiting mark, never a spinner); [INFO] a standing fact (a quiet grey dot); [ATTENTION] something that needs
 * him to act on the Mac's side (an amber lamp and amber words). None is something to tap: what has an action is a card.
 */
enum class StatusTone { WORKING, INFO, ATTENTION }

/**
 * The app's one status line under the tabs (kb PHN-660): small grey words on a compact black capsule (the lab's
 * surface for what sits on the sky: the tabs, the message box), led by the tone's mark, so it reads as the app talking
 * about itself, never as a message, an error or a button, at every time of day.
 */
@Composable
fun StatusLine(text: String, tone: StatusTone, modifier: Modifier = Modifier) {
    val words = if (tone == StatusTone.ATTENTION) Ink.amber else Ink.onDark
    Box(modifier.fillMaxWidth().padding(horizontal = 16.dp), contentAlignment = Alignment.Center) {
        Row(
            Modifier.background(Ink.black, RoundedCornerShape(14.dp)).padding(start = 11.dp, end = 13.dp, top = 6.dp, bottom = 6.dp)
                .semantics(mergeDescendants = true) { liveRegion = LiveRegionMode.Polite },
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            StatusMark(tone)
            PhoneText(text, 13f, FerbType.Medium, words, lineSpacing = 1f)
        }
    }
}

@Composable
private fun StatusMark(tone: StatusTone) {
    when (tone) {
        StatusTone.ATTENTION -> LampDot(Ink.amber, 6.dp)
        StatusTone.INFO -> Box(Modifier.size(6.dp).background(Ink.onDark, CircleShape))
        StatusTone.WORKING -> androidx.compose.foundation.Canvas(Modifier.size(9.dp)) {
            val w = 1.2.dp.toPx()
            drawCircle(Ink.onDark, radius = (size.minDimension - w) / 2, style = androidx.compose.ui.graphics.drawscope.Stroke(
                w, pathEffect = androidx.compose.ui.graphics.PathEffect.dashPathEffect(floatArrayOf(2.dp.toPx(), 2.dp.toPx()))))
        }
    }
}

/**
 * While the first page of the chat comes from the Mac and nothing is stored yet: the shapes of a few messages and one
 * plain line ([text]), never a blank screen. Still shapes (no shimmer: the lab's only unasked motion is the spinner).
 */
@Composable
fun ChatWaiting(text: String, modifier: Modifier = Modifier) {
    val shade = Color.White.copy(alpha = 0.16f)
    Column(modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
            Box(Modifier.width(190.dp).height(40.dp).background(shade, RoundedCornerShape(16.dp)))
        }
        Box(Modifier.width(250.dp).height(64.dp).background(shade, RoundedCornerShape(16.dp)))
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
            Box(Modifier.width(140.dp).height(40.dp).background(shade, RoundedCornerShape(16.dp)))
        }
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier.padding(top = 6.dp).semantics(mergeDescendants = true) { liveRegion = LiveRegionMode.Polite }) {
            RunningArc(16.dp, LocalSkyInk.current.low)
            PhoneText(text, 14.5f, FerbType.Medium, LocalSkyInk.current.low)
        }
    }
}

/** Whether the chat follows its end, and whether something arrived he hasn't seen (scrolled up). */
@Stable
class ChatFollow internal constructor(private val list: LazyListState) {
    var following by mutableStateOf(true)
        internal set
    var unseen by mutableStateOf(false)
        internal set
    internal var moved = false
    /** True while [toEnd] scrolls: that scroll is not his. */
    internal var auto = false

    /**
     * The very end of the last row (a long reply's last line, not its first), by scrolling, never by a jump: a jump
     * (scrollToItem) restarts every row's animation, and done on every frame of a keyboard's slide it left the chat
     * blank until the keyboard stopped (kb PHN-670). Only from far away (more than [NEAR] rows) is there one jump to
     * the last row first. [animate]: a glide on the motion system's sheet spring, else at once.
     */
    suspend fun toEnd(animate: Boolean) {
        // the rows just given, not laid out yet (the chat's first draw): after their first layout
        if (list.layoutInfo.totalItemsCount == 0) {
            kotlinx.coroutines.withTimeoutOrNull(1_000) { snapshotFlow { list.layoutInfo.totalItemsCount }.first { it > 0 } }
        }
        val n = list.layoutInfo.totalItemsCount
        if (n == 0) return
        following = true
        unseen = false
        auto = true
        try {
            val shown = list.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: -1
            if (shown < n - 1 - NEAR) list.scrollToItem(n - 1)
            if (animate) distanceToEnd().takeIf { it > 0f }?.let { list.animateScrollBy(it, Motion.sheet.spec()) }
            // the rest exactly: a list stops at its end by itself. Never by more than the rows left and a screen: a
            // scroll through rows not laid out yet composes every one of them (a chat of 5 000 composed them all, kb
            // PHN-714); after the jump at most NEAR rows are left.
            list.scrollBy(beyondEnd())
        } finally {
            auto = false
        }
    }

    /** How far the end is from the bottom of the view: measured rows exactly, rows not yet measured by their average. */
    private fun distanceToEnd(): Float {
        val info = list.layoutInfo
        val seen = info.visibleItemsInfo
        val last = seen.lastOrNull() ?: return 0f
        val end = info.viewportEndOffset - info.afterContentPadding
        val missing = info.totalItemsCount - 1 - last.index
        val average = seen.sumOf { it.size }.toFloat() / seen.size + info.mainAxisItemSpacing
        return (last.offset + last.size - end).coerceAtLeast(0) + missing * average
    }

    /** A scroll that surely reaches the end from here: the distance to it and a screen more (the list stops at its end). */
    private fun beyondEnd(): Float = distanceToEnd() + list.layoutInfo.viewportSize.height

    /**
     * The end against the bottom of the view at once, in this frame's scroll (the keyboard, the message box). Only near
     * the end (at most [NEAR] rows away, as on every frame of a keyboard's slide): further away is [toEnd]'s jump, and a
     * scroll from there would compose every row on the way (kb PHN-714).
     */
    internal fun keepEnd() {
        if (!list.canScrollForward) return
        val info = list.layoutInfo
        val last = info.visibleItemsInfo.lastOrNull() ?: return
        if (info.totalItemsCount - 1 - last.index > NEAR) return
        list.dispatchRawDelta(beyondEnd())
    }

    internal companion object {
        /** Further than this many rows from the end, one jump first. */
        const val NEAR = 8
    }
}

/**
 * Keeps the chat's end in view while the owner is at it (kb PHN-628): a new last row, a reply that grows as it streams,
 * the keyboard or the message box changing the list's height. His own message always goes to the end. When he scrolled
 * up, a new last row only marks [ChatFollow.unseen] (the pill). [last] is the last row as drawn (its contents too);
 * [mine] says it is his own just-sent message.
 */
@Composable
fun rememberChatFollow(list: LazyListState, lastKey: Any?, last: Any?, mine: Boolean): ChatFollow {
    val follow = remember(list) { ChatFollow(list) }
    // a drag of his, seen without reading it in composition: every drag would recompose the whole chat (kb PHN-716)
    LaunchedEffect(list) {
        list.interactionSource.interactions.collect { if (it is androidx.compose.foundation.interaction.DragInteraction.Start) follow.moved = true }
    }
    // his own scrolling decides: let go at the end, it follows; anywhere above, it stays where he put it
    LaunchedEffect(list) {
        snapshotFlow { list.isScrollInProgress }.collect { moving ->
            // any scroll that is not the follow's own: a drag, a fling, a screen reader's scroll (kb PHN-662)
            if (moving && !follow.auto) follow.moved = true
            if (!moving && follow.moved) {
                follow.moved = false
                follow.following = !list.canScrollForward
                if (follow.following) follow.unseen = false
            }
        }
    }
    var lastSeenKey by remember { mutableStateOf<Any?>(null) }
    LaunchedEffect(lastKey, last, mine) {
        val first = lastSeenKey == null
        val newRow = lastKey != lastSeenKey
        lastSeenKey = lastKey
        when {
            lastKey == null -> Unit
            // the chat's first draw opens at its end, at once (no glide down from the top)
            first -> follow.toEnd(animate = false)
            mine && newRow -> follow.toEnd(animate = true)
            follow.following -> follow.toEnd(animate = newRow)
            newRow -> follow.unseen = true
        }
    }
    // the keyboard sliding in or out, the message box growing, a status line: the list's height changes on every frame
    // of it. At the end, the end stays against the message box by a scroll of the difference (never a jump: PHN-670);
    // scrolled up, nothing moves (the rows he reads stay where they are).
    LaunchedEffect(list) {
        snapshotFlow { list.layoutInfo.viewportSize.height }.collect { if (follow.following && lastSeenKey != null && !follow.auto) follow.keepEnd() }
    }
    return follow
}

/**
 * A file waiting in the message box, or riding with a message he sent (kb PHN-690): a small picture of a photo where
 * one is cheap to make ([thumbnail]), else the file's kind; its name and [detail] (its size, or where it is). With
 * [onRemove] a cross takes it out of the box ([removeLabel] for screen readers); nothing was sent.
 */
@Composable
fun AttachmentChip(
    name: String,
    detail: String,
    modifier: Modifier = Modifier,
    thumbnail: androidx.compose.ui.graphics.ImageBitmap? = null,
    removeLabel: String? = null,
    onRemove: (() -> Unit)? = null,
) {
    // compact, as FERB.app's composer keeps a file to its icon and name (ChatBody.swift:200-216,
    // ArtifactPreview.swift:214-250; owner kb PHN-772): a small icon or picture, the name, the cross; its size is said
    // to TalkBack, not drawn
    Row(
        modifier.height(34.dp).background(Ink.darkFill, RoundedCornerShape(10.dp))
            .padding(start = 5.dp, end = if (onRemove == null) 10.dp else 0.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        if (thumbnail != null) {
            androidx.compose.foundation.Image(thumbnail, null, Modifier.size(24.dp).clip(RoundedCornerShape(6.dp)), contentScale = androidx.compose.ui.layout.ContentScale.Crop)
        } else {
            Box(Modifier.size(24.dp), contentAlignment = Alignment.Center) { FileIcon(name, 18.dp) }
        }
        PhoneText(name, 13f, FerbType.Medium, Ink.white,
            Modifier.widthIn(max = 150.dp).semantics(mergeDescendants = true) { contentDescription = "$name, $detail" }, maxLines = 1)
        if (onRemove != null) {
            Box(Modifier.size(32.dp).pressable(onRemove, label = removeLabel).semantics { removeLabel?.let { contentDescription = it } },
                contentAlignment = Alignment.Center) { FerbIcon(FerbIcons.close, 11f, Ink.onDark) }
        }
    }
}

/**
 * The files in the message box: compact chips that wrap onto more lines as files are added (the box grows with them,
 * owner kb PHN-772), never big cards in a sideways strip; past four lines the chips scroll inside the box.
 */
@OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
fun AttachmentRow(modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    androidx.compose.foundation.layout.FlowRow(
        modifier.fillMaxWidth().heightIn(max = 160.dp).verticalScroll(rememberScrollState()),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) { content() }
}

/**
 * The way to earlier days (kb PHN-626, PHN-698): an up arrow and [text] ("Earlier messages"), small in the sky's ink,
 * centred. On the home page under the tabs, and at the top of today's chat.
 */
@Composable
fun EarlierLink(text: String, modifier: Modifier = Modifier, onClick: (() -> Unit)?) {
    val ink = LocalSkyInk.current
    Row(modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center) {
        Row(Modifier.pressable(onClick, label = text).padding(horizontal = 14.dp, vertical = 8.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
            FerbIcon(FerbIcons.arrowUp, 13f, ink.low)
            PhoneText(text, 14f, FerbType.Medium, ink.low)
        }
    }
}
