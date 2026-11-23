package com.ferb.design.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.components.Card
import com.ferb.design.components.CloseButton
import com.ferb.design.components.Divide
import com.ferb.design.components.FerbBadge
import com.ferb.design.components.FerbSurface
import com.ferb.design.components.LocalSkyInk
import com.ferb.design.components.PhoneTab
import com.ferb.design.components.PhoneText
import com.ferb.design.components.Pill
import com.ferb.design.components.PillKind
import com.ferb.design.components.SessionAction
import com.ferb.design.components.SessionAsk
import com.ferb.design.components.SessionAskCard
import com.ferb.design.components.SessionChip
import com.ferb.design.components.SessionLine
import com.ferb.design.components.SessionLineView
import com.ferb.design.components.SessionRow
import com.ferb.design.components.SessionRowData
import com.ferb.design.components.SkyLabel
import com.ferb.design.components.TopBar
import androidx.compose.ui.platform.testTag
import com.ferb.design.components.pressable
import com.ferb.design.components.MoreButton
import com.ferb.design.components.SessionMenu
import com.ferb.design.components.SessionOutgoingView
import com.ferb.design.components.SessionOutgoing
import com.ferb.design.components.ChatJump
import com.ferb.design.components.ChatWaiting
import com.ferb.design.components.DayDivider
import com.ferb.design.components.JumpPill
import com.ferb.design.components.chatJump
import com.ferb.design.components.rememberChatFollow
import com.ferb.design.MotionShow
import com.ferb.design.motionItem
import com.ferb.design.presenceFade
import androidx.compose.foundation.layout.height
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import kotlinx.coroutines.launch

// The Sessions tab and one session's screen (kb HAND-691, HAND-651..666): every terminal on the owner's Mac, his Claude Code
// and Codex sessions among them, and the chat apps' chats. The phones show what FERB.app's Sessions place shows, in the
// phone's own look.

/** Sessions are off on the Mac: what FERB would do, and the one button that turns them on. */
data class SessionsOff(val title: String, val text: String, val turnOn: String)

/**
 * The Sessions tab: its label on the sky, the sessions in one white card (title or folder, "<folder> · in <app>", the
 * state, "FERB is on it"), and New session at the foot. Off: what turning it on does and Turn on. None: [empty].
 */
@Composable
fun SessionsPage(
    label: String,
    rows: List<SessionRowData>,
    newLabel: String,
    insets: PhoneInsets = PhoneInsets.Lab,
    off: SessionsOff? = null,
    empty: String? = null,
    /** HAND-697: one quiet line per chat app open with no chat FERB could find. */
    notes: List<String> = emptyList(),
    waiting: PhoneTab? = null,
    onTab: ((PhoneTab) -> Unit)? = null,
    onProfile: (() -> Unit)? = null,
    onRow: ((Int) -> Unit)? = null,
    onNew: (() -> Unit)? = null,
    onTurnOn: (() -> Unit)? = null,
) {
    val ink = LocalSkyInk.current
    Column(Modifier.fillMaxSize()) {
        TopBar(PhoneTab.SESSIONS, waiting = waiting, onTab = onTab, onProfile = onProfile)
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).offset(y = (-6).dp)) {
            SkyLabel(label)
            when {
                off != null -> Card(padding = 18.dp) {
                    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                        PhoneText(off.title, 17f, FerbType.Semibold, Ink.black)
                        PhoneText(off.text, 15f, color = Ink.detail, lineSpacing = 2f)
                        Pill(off.turnOn, Modifier.padding(top = 6.dp), onClick = onTurnOn)
                    }
                }
                rows.isEmpty() -> if (empty != null) {
                    PhoneText(empty, 15.5f, FerbType.Medium, ink.low, Modifier.padding(start = 8.dp, end = 8.dp, top = 4.dp))
                }
                else -> Card {
                    rows.forEachIndexed { i, row ->
                        if (i > 0) Divide()
                        SessionRow(row, onRow?.let { { it(i) } })
                    }
                }
            }
            if (off == null) {
                for (note in notes) {
                    PhoneText(note, 14.5f, FerbType.Medium, ink.low, Modifier.padding(start = 8.dp, end = 8.dp, top = 10.dp).testTag("no-chat"))
                }
            }
        }
        if (off == null) {
            Pill(newLabel, Modifier.paddingOverBottom(insets, 8.dp, start = 16.dp, end = 16.dp, top = 8.dp), PillKind.BLACK, FerbIcons.plus, onClick = onNew)
        }
    }
}

/**
 * One session, full screen on black: its title and state on one header with its menu (where it runs and its actions,
 * PHN-800) and Close; the conversation, then his message on its way or refused ([outgoing], PHN-812, PHN-800), then
 * the agent's open questions; the message box ([composer], or why nothing can be sent). The screenshot switch decides
 * here as on every page (PHN-810).
 *
 * The conversation is the main chat's list (owner 2026-11-23, kb PHN-813, PHN-814; ChatList.kt): its end followed only
 * while he is at it, his own message always taken to it, the keyboard keeping the end in view; scrolled up, the arrow
 * back to the latest ([toLatest]), with "[newMessage]" when something arrived meanwhile; arriving lines fade in; a day's
 * start marked; earlier lines asked for when he reaches the top ([older] stays as the tap for it); the agent's words
 * selectable. Lines start below the header with the main chat's gaps and fade out under it and above the message box
 * (owner, kb PHN-815), and the page keeps every touch on it from the Sessions tab under it ([coversPage]). While its
 * first read is on its way ([loading]) it shows the main chat's waiting look (PHN-816).
 */
@Composable
fun SessionPage(
    header: SessionRowData,
    lines: List<SessionLine>,
    asks: List<SessionAsk>,
    actions: List<SessionAction>,
    insets: PhoneInsets = PhoneInsets.Lab,
    older: String? = null,
    onOlder: (() -> Unit)? = null,
    onClose: (() -> Unit)? = null,
    onChoice: ((ask: Int, choice: Int) -> Unit)? = null,
    outgoing: SessionOutgoing? = null,
    onRetry: (() -> Unit)? = null,
    moreLabel: String = "More",
    menuOpen: Boolean = false,
    onMenu: ((Boolean) -> Unit)? = null,
    toLatest: String = "Go to the latest messages",
    newMessage: String = "New message",
    loading: String? = null,
    list: LazyListState = rememberLazyListState(),
    composer: @Composable (Modifier) -> Unit,
) {
    Box(Modifier.presenceFade().fillMaxSize().background(Ink.black)) {
        Column(Modifier.fillMaxSize().paddingOverBottom(insets, 8.dp, top = insets.top + 8.dp)) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp).padding(bottom = 4.dp).testTag("session-header"),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    PhoneText(header.title, 19f, FerbType.Semibold, Ink.white, Modifier.semantics { heading() }, maxLines = 1)
                    FerbSurface {
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            SessionChip(header.state, header.stateText)
                            header.ferb?.let { FerbBadge(it) }
                        }
                    }
                }
                if (actions.isNotEmpty()) MoreButton(moreLabel, menuOpen, Modifier.testTag("session-more"), onClick = onMenu?.let { { it(!menuOpen) } })
                CloseButton(onClose)
            }
            Box(Modifier.weight(1f).fillMaxWidth()) {
                // its first read on its way: the main chat's waiting look, never an empty page or an error (PHN-816)
                if (loading != null && lines.isEmpty() && asks.isEmpty() && outgoing == null) {
                    ChatWaiting(loading, Modifier.testTag("session-waiting"), ink = Ink.onDark)
                } else {
                    SessionList(lines, asks, older, onOlder, onChoice, outgoing, onRetry, toLatest, newMessage, list)
                }
            }
            composer(Modifier.padding(start = 16.dp, end = 16.dp, top = 8.dp))
        }
        if (menuOpen && actions.isNotEmpty()) {
            // a tap anywhere else closes the menu
            Box(Modifier.fillMaxSize().pressable(onMenu?.let { { it(false) } }, label = moreLabel))
            SessionMenu(
                header.subtitle, actions,
                Modifier.align(Alignment.TopEnd).padding(top = insets.top + 8.dp + 52.dp, end = 16.dp).testTag("session-menu"),
                onPicked = { onMenu?.invoke(false) },
            )
        }
    }
}

/** How far the lines fade out under the header and above the message box. */
private val EdgeFade = 14.dp

/** The conversation as the main chat's list (ChatList.kt's follow and jump; PHN-813, PHN-814). */
@Composable
private fun androidx.compose.foundation.layout.BoxScope.SessionList(
    lines: List<SessionLine>, asks: List<SessionAsk>, older: String?, onOlder: (() -> Unit)?,
    onChoice: ((ask: Int, choice: Int) -> Unit)?, outgoing: SessionOutgoing?, onRetry: (() -> Unit)?,
    toLatest: String, newMessage: String, list: LazyListState,
) {
    // the last row as drawn: a new line, his message's state, a question; his own message always goes to the end
    val lastKey: Any? = asks.lastOrNull()?.let { "ask:" + it.key } ?: outgoing?.let { "out:" + it.text } ?: lines.lastOrNull()?.let { "l:" + it.key }
    val last: Any? = Triple(lastKey, outgoing, asks.size)
    val follow = rememberChatFollow(list, lastKey, last, mine = outgoing != null && outgoing.reason == null && lastKey == "out:" + outgoing.text)
    // only rows new since the list last drew fade in (none on its first draw: kb PHN-624, PHN-670)
    val keys = remember(lines, outgoing, asks) {
        buildSet { lines.forEach { add("l:" + it.key) }; outgoing?.let { add("out:" + it.text) }; asks.forEach { add("ask:" + it.key) } }
    }
    val drawn = remember { arrayOfNulls<Set<String>>(1) }
    val arriving = drawn[0]?.let { before -> (keys - before).ifEmpty { emptySet() } }.orEmpty()
    SideEffect { drawn[0] = keys }
    // reaching the top after he scrolled up asks for the earlier lines, as the main chat's earlier days do
    val atTop by remember(list) { derivedStateOf { list.layoutInfo.totalItemsCount > 0 && !list.canScrollBackward } }
    LaunchedEffect(atTop, older, follow.following) { if (atTop && older != null && !follow.following) onOlder?.invoke() }
    LazyColumn(
        Modifier.fillMaxSize().testTag("session-list"),
        state = list,
        contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 12.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        if (older != null) item(key = "older") {
            Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                FerbSurface { Pill(older, kind = PillKind.GREY, height = 36.dp, fill = false, onClick = onOlder) }
            }
        }
        items(lines, key = { "l:" + it.key }, contentType = { it.kind }) {
            Column(motionItem(arriving = "l:" + it.key in arriving), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                it.day?.let { d -> DayDivider(d, color = Ink.onDark) }
                SessionLineView(it)
            }
        }
        if (outgoing != null) item(key = "out:" + outgoing.text) {
            SessionOutgoingView(outgoing, motionItem(arriving = "out:" + outgoing.text in arriving)
                .testTag(if (outgoing.reason != null) "session-unsent" else "session-outgoing"), onRetry)
        }
        itemsIndexed(asks, key = { _, a -> "ask:" + a.key }) { i, a ->
            SessionAskCard(a, motionItem(arriving = "ask:" + a.key in arriving), onChoice = onChoice?.let { { c -> it(i, c) } })
        }
    }
    // the lines fade out under the header and above the message box instead of being cut against them (PHN-815)
    Box(Modifier.align(Alignment.TopCenter).fillMaxWidth().height(EdgeFade).background(Brush.verticalGradient(listOf(Ink.black, Color.Transparent))))
    Box(Modifier.align(Alignment.BottomCenter).fillMaxWidth().height(EdgeFade).background(Brush.verticalGradient(listOf(Color.Transparent, Ink.black))))
    // scrolled up: the way back to the latest lines, "New message" when one came meanwhile (ChatList.kt, kb PHN-662)
    val scope = rememberCoroutineScope()
    val jump = chatJump(follow.following, follow.unseen, null, null)
    MotionShow(jump.takeIf { it != ChatJump.NONE }, Modifier.align(Alignment.BottomCenter).padding(bottom = 10.dp)) { kind ->
        JumpPill(if (kind == ChatJump.NEW_MESSAGE) newMessage else null, toLatest, Modifier.testTag("session-jump")) {
            scope.launch { follow.toEnd(animate = true) }
        }
    }
}
