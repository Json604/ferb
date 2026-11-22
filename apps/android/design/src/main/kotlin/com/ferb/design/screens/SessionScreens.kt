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
import com.ferb.design.components.SessionActions
import com.ferb.design.components.SessionAsk
import com.ferb.design.components.SessionAskCard
import com.ferb.design.components.SessionChip
import com.ferb.design.components.SessionLine
import com.ferb.design.components.SessionLineView
import com.ferb.design.components.SessionRow
import com.ferb.design.components.SessionRowData
import com.ferb.design.components.SkyLabel
import com.ferb.design.components.TopBar

// The Sessions tab and one session's screen (kb HAND-651..666): the owner's Claude Code and Codex sessions on his Mac. The
// phones show what FERB.app's Sessions place shows, in the phone's own look.

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
        }
        if (off == null) {
            Pill(newLabel, Modifier.paddingOverBottom(insets, 8.dp, start = 16.dp, end = 16.dp, top = 8.dp), PillKind.BLACK, FerbIcons.plus, onClick = onNew)
        }
    }
}

/**
 * One session, full screen on black (it holds the owner's conversation: the app keeps it out of screenshots): its
 * title, place and state, its buttons, the conversation with "Show earlier" at its top and the agent's open questions
 * at its end, and the message box ([composer], or why nothing can be sent).
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
    list: LazyListState = rememberLazyListState(),
    composer: @Composable (Modifier) -> Unit,
) {
    Column(Modifier.fillMaxSize().background(Ink.black).paddingOverBottom(insets, 8.dp, top = insets.top + 8.dp)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.Top) {
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                PhoneText(header.title, 20f, FerbType.Semibold, Ink.white, Modifier.semantics { heading() }, maxLines = 2)
                PhoneText(header.subtitle, 14f, color = Ink.onDark, maxLines = 1)
                FerbSurface {
                    Row(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        SessionChip(header.state, header.stateText)
                        header.ferb?.let { FerbBadge(it) }
                    }
                }
            }
            CloseButton(onClose)
        }
        if (actions.isNotEmpty()) SessionActions(actions, Modifier.padding(top = 12.dp))
        // the end followed: a new line or question comes into view when he was at the end (or on the first page)
        val count = lines.size + asks.size + (if (older != null) 1 else 0)
        val atEnd by remember(list) {
            derivedStateOf { list.layoutInfo.visibleItemsInfo.lastOrNull()?.index?.let { it >= list.layoutInfo.totalItemsCount - 3 } ?: true }
        }
        val lastKey = lines.lastOrNull()?.key to asks.size
        LaunchedEffect(lastKey) { if (count > 0 && atEnd) list.scrollToItem(count - 1) }
        LazyColumn(
            Modifier.weight(1f).fillMaxWidth(),
            state = list,
            contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 14.dp, bottom = 12.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (older != null) item(key = "older") {
                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                    FerbSurface { Pill(older, kind = PillKind.GREY, height = 36.dp, fill = false, onClick = onOlder) }
                }
            }
            items(lines, key = { "l:" + it.key }) { SessionLineView(it) }
            itemsIndexed(asks, key = { _, a -> "ask:" + a.key }) { i, a -> SessionAskCard(a, onChoice = onChoice?.let { { c -> it(i, c) } }) }
        }
        composer(Modifier.padding(horizontal = 16.dp))
    }
}
