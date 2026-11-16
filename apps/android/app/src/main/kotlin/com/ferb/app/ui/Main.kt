package com.ferb.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.ferb.app.Feature
import com.ferb.app.FerbViewModel
import com.ferb.app.Note
import com.ferb.app.R
import com.ferb.app.Sheet
import com.ferb.app.UiState
import com.ferb.app.model.ChatRow
import com.ferb.app.model.WorkItem
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.Phone
import com.ferb.design.components.Card
import com.ferb.design.components.CardRow
import com.ferb.design.components.Divide
import com.ferb.design.components.FerbBubble
import com.ferb.design.components.FileIcon
import com.ferb.design.components.LinkKind
import com.ferb.design.components.LocalSkyInk
import com.ferb.design.components.Mark
import com.ferb.design.components.MarkKind
import com.ferb.design.components.Opens
import com.ferb.design.components.OwnerBubble
import com.ferb.design.components.PhoneComposer
import com.ferb.design.components.PhoneTab
import com.ferb.design.components.PhoneText
import com.ferb.design.components.Pill
import com.ferb.design.components.PillKind
import com.ferb.design.components.RowIcon
import com.ferb.design.components.SkyLabel
import com.ferb.design.components.TaskCard
import com.ferb.design.components.TopBar
import com.ferb.design.components.TypingDots
import com.ferb.design.components.pressable
import com.ferb.design.screens.HomePage
import com.ferb.design.screens.HomeTexts
import com.ferb.design.screens.MacPage
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.TasksPage
import com.ferb.design.screens.WorkGroup
import com.ferb.design.screens.WorkRow
import com.ferb.design.sky.SkyMoment
import com.ferb.mobile.LinkPath
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.OlderState
import com.ferb.mobile.RelayStatus

@Composable
fun noteText(n: Note): String = stringResource(n.res, n.words.orEmpty())

/** The four tabs (Mobile.swift TopBar): chat (the home screen until a chat starts), tasks, files, the Mac. */
@Composable
fun Main(vm: FerbViewModel, ui: UiState, tab: PhoneTab, home: Boolean, insets: PhoneInsets, moment: SkyMoment, host: Host) {
    val waiting = if (ui.questions.isNotEmpty() && tab != PhoneTab.TASKS) PhoneTab.TASKS else null
    val draft by vm.draft.collectAsState()
    val onTab: (PhoneTab) -> Unit = vm::choose
    val onProfile = { vm.show(Sheet.Profile) }
    Box(Modifier.fillMaxSize()) {
        when (tab) {
            PhoneTab.CHAT -> if (home) Home(vm, ui, draft, insets, moment, onTab, onProfile, host)
            else Chat(vm, ui, draft, insets, waiting, onTab, onProfile, host)
            PhoneTab.TASKS -> Tasks(vm, ui, onTab, onProfile)
            PhoneTab.FILES -> Files(vm, ui, insets, waiting, onTab, onProfile, host)
            PhoneTab.MAC -> Mac(vm, ui, waiting, onTab, onProfile)
        }
        ui.note?.let { n ->
            // a short line over the page; it goes by itself after a few seconds (and on a tap)
            val text = noteText(n)
            LaunchedEffect(n) { kotlinx.coroutines.delay(5_000); vm.note(null) }
            Card(Modifier.align(Alignment.TopCenter).padding(start = 16.dp, end = 16.dp, top = 64.dp).pressable({ vm.note(null) }), padding = 14.dp) {
                PhoneText(text, 15f, color = Ink.black, modifier = Modifier.fillMaxWidth().semantics { liveRegion = LiveRegionMode.Polite })
            }
        }
    }
}

/**
 * The composer every page shares: the owner's draft and send; + picks files that go to the Mac and ride with the
 * next message (PHN-127), shown above it while they travel and once they arrived; the mic isn't on the phone yet.
 */
@Composable
private fun Composer(vm: FerbViewModel, draft: String, placeholder: String, modifier: Modifier, host: Host) {
    val ready by vm.transfers.ready.collectAsState()
    val moving by vm.transfers.list.collectAsState()
    val ink = LocalSkyInk.current
    Column(modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        val forChat = moving.filter { it.forChat }
        if (forChat.isNotEmpty()) TransferCard(vm, forChat, host, Modifier)
        if (ready.isNotEmpty()) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 6.dp), horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
                for (a in ready.take(4)) {
                    Row(Modifier.pressable({ vm.transfers.unattach(a.uploadId) }, label = stringResource(R.string.attach_remove, a.name)),
                        horizontalArrangement = Arrangement.spacedBy(4.dp), verticalAlignment = Alignment.CenterVertically) {
                        FileIcon(a.name, 18.dp)
                        PhoneText(a.name, 13.5f, FerbType.Medium, ink.low, Modifier.widthIn(max = 120.dp), maxLines = 1)
                        FerbIcon(FerbIcons.close, 11f, ink.low)
                    }
                }
                if (ready.size > 4) PhoneText("+${ready.size - 4}", 13.5f, FerbType.Medium, ink.low)
            }
        }
        PhoneComposer(
            Modifier, placeholder = placeholder, typed = draft, onValueChange = vm::typed, onSend = { vm.send() },
            onAttach = { host.pickFiles(true) }, onMic = { vm.show(Sheet.NotYet(Feature.VOICE)) },
        )
    }
}

@Composable
private fun Home(vm: FerbViewModel, ui: UiState, draft: String, insets: PhoneInsets, moment: SkyMoment, onTab: (PhoneTab) -> Unit, onProfile: () -> Unit, host: Host) {
    val words = remember(moment.epochSeconds.toLong() / 60, ui.work, ui.link?.path) { vm.homeWords(moment) }
    val texts = words?.let { HomeTexts(it.greeting, it.subline, it.away, it.placeholder, it.chat) } ?: HomeTexts("Hello", "")
    val away = awayRows(ui)
    val asking = stringResource(R.string.question_default_title)
    HomePage(
        texts, away.map { WorkRow(it.kind, it.title.ifEmpty { asking }, it.detail) }, insets,
        onTab = onTab, onProfile = onProfile,
        onRow = { i -> away.getOrNull(i)?.let { open(vm, it) } },
        onMore = { vm.choose(PhoneTab.TASKS) },
        composer = { m -> Composer(vm, draft, texts.placeholder, m, host) },
    )
}

/** What happened while the owner was away: questions first, then failures, then what finished (HomePage). */
private fun awayRows(ui: UiState): List<WorkItem> =
    ui.work.waiting + ui.away.filter { it.kind == MarkKind.FAILED } + ui.away.filter { it.kind != MarkKind.FAILED }

private fun open(vm: FerbViewModel, item: WorkItem) {
    if (item.approvalId != null) vm.show(Sheet.Asking(item.approvalId)) else vm.openChat()
}

@Composable
private fun Chat(vm: FerbViewModel, ui: UiState, draft: String, insets: PhoneInsets, waiting: PhoneTab?, onTab: (PhoneTab) -> Unit, onProfile: () -> Unit, host: Host) {
    val list = rememberLazyListState()
    val rows = ui.rows
    // new rows: follow the chat down; reaching the top asks for the page before it
    LaunchedEffect(rows.size) { if (rows.isNotEmpty()) list.animateScrollToItem(rows.size - 1) }
    val atTop by remember { derivedStateOf { list.firstVisibleItemIndex == 0 } }
    LaunchedEffect(atTop, ui.older) { if (atTop && ui.older is OlderState.Available) vm.loadOlder() }
    val ink = LocalSkyInk.current
    Column(Modifier.fillMaxSize()) {
        TopBar(PhoneTab.CHAT, waiting = waiting, onTab = onTab, onProfile = onProfile)
        statusLine(ui)?.let { (text, tap) ->
            PhoneText(stringResource(text), 14f, FerbType.Medium, ink.low,
                Modifier.fillMaxWidth().pressable(if (tap) ({ vm.unlockMac() }) else null).padding(start = 24.dp, end = 24.dp, top = 10.dp))
        }
        LazyColumn(
            Modifier.weight(1f).fillMaxWidth(),
            state = list,
            contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 12.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            items(rows, key = { it.key }) { row -> ChatRowView(vm, ui, row) }
        }
        Composer(vm, draft, stringResource(R.string.chat_placeholder), Modifier.padding(start = 16.dp, end = 16.dp, bottom = insets.bottom + 8.dp), host)
    }
}

/** The line under the tabs: the Mac can't be reached, or its unlock is needed (a tap gives it). */
private fun statusLine(ui: UiState): Pair<Int, Boolean>? = when {
    ui.link?.phase != LinkPhase.ONLINE -> R.string.chat_offline to false
    ui.needsMacUnlock -> R.string.chat_unlock_mac to true
    else -> null
}

@Composable
private fun ChatRowView(vm: FerbViewModel, ui: UiState, row: ChatRow) {
    val ink = LocalSkyInk.current
    when (row) {
        is ChatRow.Owner -> Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(4.dp)) {
            OwnerBubble(row.text)
            for (f in row.files) Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                FileIcon(f, 18.dp)
                PhoneText(f, 13.5f, FerbType.Medium, ink.low, maxLines = 1)
            }
            row.note?.let { PhoneText(it, 13.5f, FerbType.Medium, ink.low, Modifier.padding(horizontal = 6.dp)) }
        }
        is ChatRow.Ferb -> FerbBubble(row.text)
        is ChatRow.Line -> Column(Modifier.fillMaxWidth().padding(horizontal = 6.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            PhoneText(row.text, 14.5f, FerbType.Medium, ink.low)
            for (q in row.quotes) PhoneText(q, 14f, color = ink.low)
        }
        is ChatRow.Task -> TaskCard(
            row.headline, stringResource(R.string.progress, row.done, row.total), row.parts,
            onWatch = { vm.watchLive(row.taskId) }, onStop = { vm.stop(row.taskId) },
        )
        is ChatRow.Cards -> Card {
            row.cards.forEachIndexed { i, c ->
                if (i > 0) Divide()
                CardRow(c.title, c.detail, leading = { if (c.file) FileIcon(c.title, 26.dp) else RowIcon(FerbIcons.doc) })
            }
        }
        is ChatRow.Typing -> TypingDots()
        is ChatRow.Asking -> Card {
            CardRow(row.title ?: stringResource(R.string.question_default_title), stringResource(R.string.question_tap),
                onClick = { vm.show(Sheet.Asking(row.approvalId)) }, leading = { Mark(MarkKind.ASKING) }, trailing = { Opens() })
        }
        is ChatRow.Outbox -> OutboxRow(vm, row)
        is ChatRow.Older -> when (row.state) {
            is OlderState.NeedsBetterPath -> PhoneText(stringResource(R.string.older_needs_better_path), 14f, FerbType.Medium, ink.low,
                Modifier.fillMaxWidth().padding(horizontal = 6.dp))
            else -> PhoneText(stringResource(R.string.older_available), 14f, FerbType.Medium, ink.low,
                Modifier.fillMaxWidth().pressable({ vm.loadOlder() }).padding(horizontal = 6.dp, vertical = 8.dp))
        }
    }
}

/** A message in the outbox (PHN-401): the owner's bubble, faded until it is sent, with what it waits for and Cancel. */
@Composable
private fun OutboxRow(vm: FerbViewModel, row: ChatRow.Outbox) {
    val o = row.item
    val ink = LocalSkyInk.current
    val failed = o.state == "failed"
    val status = when {
        failed -> stringResource(R.string.outbox_failed, o.reason.orEmpty())
        o.state == "sending" -> stringResource(R.string.outbox_sending)
        o.waitingFor == "unlock" -> stringResource(R.string.outbox_waiting_unlock)
        else -> stringResource(R.string.outbox_waiting)
    }
    Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Box(Modifier.alpha(if (failed) 1f else 0.7f)) { OwnerBubble(o.text) }
        Row(Modifier.padding(horizontal = 6.dp).semantics(mergeDescendants = true) { liveRegion = LiveRegionMode.Polite },
            horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
            PhoneText(status, 13.5f, FerbType.Medium, if (failed) Ink.red else ink.low, Modifier.widthIn(max = 240.dp))
            if (o.state != "sending") {
                Box(Modifier.pressable({ vm.cancelOutbox(o.id) }).heightIn(min = Phone.touchTarget), contentAlignment = Alignment.Center) {
                    PhoneText(stringResource(if (failed) R.string.outbox_remove else R.string.outbox_cancel), 13.5f, FerbType.Semibold, ink.top)
                }
            }
        }
    }
}

@Composable
private fun Tasks(vm: FerbViewModel, ui: UiState, onTab: (PhoneTab) -> Unit, onProfile: () -> Unit) {
    val w = ui.work
    val asking = stringResource(R.string.question_default_title)
    val groups = listOf(
        stringResource(R.string.tasks_waiting) to w.waiting,
        stringResource(R.string.tasks_running) to w.running,
        stringResource(R.string.tasks_earlier) to w.earlier.take(30),
    ).filter { it.second.isNotEmpty() }
    val ink = LocalSkyInk.current
    TasksPage(
        groups.map { (label, items) ->
            WorkGroup(label, items.map { WorkRow(it.kind, it.title.ifEmpty { asking }, it.detail, it.progress?.let { (d, t) -> stringResource(R.string.progress, d, t) }) })
        },
        waiting = null, onTab = onTab, onProfile = onProfile,
        onRow = { g, r -> groups.getOrNull(g)?.second?.getOrNull(r)?.let { open(vm, it) } },
        empty = { PhoneText(stringResource(R.string.tasks_empty), 15.5f, FerbType.Medium, ink.low, Modifier.padding(start = 8.dp, end = 8.dp, top = 22.dp)) },
    )
}

@Composable
private fun Mac(vm: FerbViewModel, ui: UiState, waiting: PhoneTab?, onTab: (PhoneTab) -> Unit, onProfile: () -> Unit) {
    val l = ui.link
    val online = l?.phase == LinkPhase.ONLINE
    val kind = when {
        !online -> LinkKind.SEARCH
        l?.path == LinkPath.DIRECT -> LinkKind.DIRECT
        else -> LinkKind.RELAY
    }
    // the Mac's own name once it told it (phone.list, kept by the library), else the lab's "Your Mac"
    val name = ui.mac?.name ?: stringResource(R.string.mac_name)
    val status = when {
        online && l?.path == LinkPath.OWN_RELAY -> stringResource(R.string.mac_own_relay)
        online -> null // the lab's own words: "Connected directly", "Through the shared relay"
        l?.phase == LinkPhase.CONNECTING -> stringResource(R.string.mac_connecting)
        l?.retryInS != null -> stringResource(R.string.mac_retry_in, l.retryInS!!.toInt())
        else -> stringResource(R.string.mac_offline)
    }
    val note = when {
        ui.status?.stopped == true -> stringResource(R.string.mac_stopped)
        l?.relay is RelayStatus.OwnFallback -> (l.relay as RelayStatus.OwnFallback).problem?.detail
            ?: stringResource(R.string.mac_own_fallback)
        l?.macOlder == true -> stringResource(R.string.mac_mac_older)
        l?.macNewer == true -> stringResource(R.string.mac_mac_newer)
        !online -> l?.problem?.detail
        else -> null
    }
    MacPage(
        kind, macName = name, ownRelay = l?.ownRelayUrl?.let(::host), status = status, note = note, name = if (!online) name else null,
        waiting = waiting, onTab = onTab, onProfile = onProfile,
        onLink = { if (online && l?.path == LinkPath.PUBLIC_RELAY) vm.show(Sheet.Relay) },
        onOwnRelay = { vm.show(Sheet.OwnRelay) },
        onSettings = { vm.openSettings() },
        onStopAll = { vm.show(Sheet.StopAll) },
    )
}

/** A relay's address as the row shows it: its host. */
fun host(url: String): String = url.substringAfter("://").substringBefore('/').substringBefore('?')
