package com.ferb.design.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
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
import com.ferb.design.Phone
import com.ferb.design.launchTarget
import com.ferb.design.components.Card
import com.ferb.design.components.CardRow
import com.ferb.design.components.Divide
import com.ferb.design.components.FerbBubble
import com.ferb.design.components.FerbMark
import com.ferb.design.components.FileIcon
import com.ferb.design.components.LinkKind
import com.ferb.design.components.LinkPicture
import com.ferb.design.components.LocalSkyInk
import com.ferb.design.components.Mark
import com.ferb.design.components.MarkKind
import com.ferb.design.components.Opens
import com.ferb.design.components.OwnerBubble
import com.ferb.design.components.PairAction
import com.ferb.design.components.PairStep
import com.ferb.design.components.PairingCode
import com.ferb.design.components.PhoneComposer
import com.ferb.design.components.PhoneTab
import com.ferb.design.components.PhoneText
import com.ferb.design.components.Pill
import com.ferb.design.components.PillKind
import com.ferb.design.components.PlanPart
import com.ferb.design.components.RowIcon
import com.ferb.design.components.SkyLabel
import com.ferb.design.components.TaskCard
import com.ferb.design.components.TopBar
import com.ferb.design.components.TypingDots
import com.ferb.design.components.pressable

// The pages (MobileScreens.swift "Pages"). Stateless: the app passes what to show; the lab's own wording stays as
// the defaults where it is the design's and not data.

/** A row of work: its mark, title and detail. */
data class WorkRow(val kind: MarkKind, val title: String, val detail: String, val value: String? = null)

/** The home screen's words (Words.swift HomeTexts); the app chooses them. */
data class HomeTexts(
    val greeting: String,
    val subline: String,
    val away: String = "While you were away",
    val placeholder: String = "Ask FERB",
    val chat: String? = null,
)

/**
 * MobileScreens.swift HomePage: the sky, a greeting, and what happened while the owner was away: questions first,
 * then failures, then what finished, at most three. Nothing happened: no list and no box, only the sky and the
 * message box (the empty state).
 */
@Composable
fun HomePage(
    texts: HomeTexts,
    away: List<WorkRow>,
    insets: PhoneInsets = PhoneInsets.Lab,
    typed: String? = null,
    onTab: ((PhoneTab) -> Unit)? = null,
    onProfile: (() -> Unit)? = null,
    onRow: ((Int) -> Unit)? = null,
    onMore: (() -> Unit)? = null,
    composer: (@Composable (Modifier) -> Unit)? = null,
    /** Not from the lab: the way back to earlier days' messages on a new day (kb PHN-626); none by default. */
    earlier: String? = null,
    onEarlier: (() -> Unit)? = null,
    /**
     * Not from the lab: a card FERB keeps live above the box (the memory import's progress while one runs, kb MEM-457),
     * drawn with the modifier given; none by default.
     */
    live: (@Composable (Modifier) -> Unit)? = null,
) {
    val ink = LocalSkyInk.current
    Column(Modifier.fillMaxSize()) {
        TopBar(PhoneTab.CHAT, waiting = if (away.any { it.kind == MarkKind.ASKING }) PhoneTab.TASKS else null, onTab = onTab, onProfile = onProfile)
        if (earlier != null) com.ferb.design.components.EarlierLink(earlier, Modifier.padding(top = 10.dp), onEarlier)
        Column(Modifier.padding(start = 22.dp, end = 22.dp, top = 70.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            PhoneText(texts.greeting, FerbType.Role.greeting, FerbType.Medium, ink.top, Modifier.semantics { heading() })
            PhoneText(texts.subline, FerbType.Role.subline, FerbType.Medium, ink.top)
        }
        if (texts.chat != null) {
            FerbBubble(texts.chat, Modifier.padding(start = 16.dp, end = 16.dp, top = 22.dp), maxWidth = 300.dp)
        }
        Spacer(Modifier.weight(1f))
        Column(Modifier.paddingOverBottom(insets, 8.dp, start = 16.dp, end = 16.dp)) {
            live?.invoke(Modifier.padding(bottom = 12.dp))
            if (away.isNotEmpty()) {
                SkyLabel(texts.away)
                Card {
                    away.take(3).forEachIndexed { i, row ->
                        if (i > 0) Divide()
                        CardRow(row.title, row.detail, onClick = onRow?.let { { it(i) } }, leading = { Mark(row.kind) }, trailing = { if (row.kind != MarkKind.DONE) Opens() })
                    }
                    if (away.size > 3) {
                        Divide()
                        PhoneText("${away.size - 3} more in Tasks", 14.5f, FerbType.Medium, Ink.detail,
                            Modifier.fillMaxWidth().pressable(onMore).padding(horizontal = 14.dp, vertical = 12.dp))
                    }
                }
            }
            if (composer != null) composer(Modifier.padding(top = 12.dp))
            else PhoneComposer(Modifier.padding(top = 12.dp), placeholder = texts.placeholder, typed = typed)
        }
    }
}

/** One thing in the chat. */
sealed interface ChatItem {
    data class Owner(val text: String) : ChatItem
    data class Ferb(val text: String) : ChatItem
    data class Task(val headline: String, val progress: String, val parts: List<PlanPart>) : ChatItem
    data object Typing : ChatItem
}

/** MobileScreens.swift ChatPage: tabs at the top, the conversation, and the composer to add to it. */
@Composable
fun ChatPage(items: List<ChatItem>, insets: PhoneInsets = PhoneInsets.Lab, waiting: PhoneTab? = PhoneTab.TASKS, typingTimeMillis: Long? = null) {
    Column(Modifier.fillMaxSize()) {
        TopBar(PhoneTab.CHAT, waiting = waiting)
        Column(
            Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(start = 16.dp, end = 16.dp, top = 16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            for (item in items) when (item) {
                is ChatItem.Owner -> OwnerBubble(item.text)
                is ChatItem.Ferb -> FerbBubble(item.text)
                is ChatItem.Task -> TaskCard(item.headline, item.progress, item.parts)
                ChatItem.Typing -> TypingDots(timeMillis = typingTimeMillis)
            }
        }
        PhoneComposer(Modifier.paddingOverBottom(insets, 8.dp, start = 16.dp, end = 16.dp), placeholder = "Add to this")
    }
}

/** A labelled group of rows on the sky. */
data class WorkGroup(val label: String, val rows: List<WorkRow>)

/** MobileScreens.swift PhoneTasksPage: what waits for you, what runs, what finished: one white card per group. */
@Composable
fun TasksPage(
    groups: List<WorkGroup>,
    waiting: PhoneTab? = PhoneTab.TASKS,
    onTab: ((PhoneTab) -> Unit)? = null,
    onProfile: (() -> Unit)? = null,
    onRow: ((group: Int, row: Int) -> Unit)? = null,
    empty: (@Composable () -> Unit)? = null,
) {
    Column(Modifier.fillMaxSize()) {
        TopBar(PhoneTab.TASKS, waiting = waiting, onTab = onTab, onProfile = onProfile)
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).offset(y = (-6).dp)) {
            if (groups.isEmpty()) empty?.invoke()
            groups.forEachIndexed { g, group ->
                SkyLabel(group.label)
                Card {
                    group.rows.forEachIndexed { i, row ->
                        if (i > 0) Divide()
                        CardRow(row.title, row.detail, onClick = onRow?.let { { it(g, i) } }, leading = { Mark(row.kind) }, trailing = { Opens(row.value) })
                    }
                }
            }
        }
    }
}

/** A place on the Mac: its icon, name and how many things are in it. */
data class PlaceRow(val icon: ImageVector, val title: String, val count: String)

/** A file: its name and what is known about it. */
data class FileRow(val name: String, val detail: String)

/** A transfer under way. */
data class Transfer(val title: String, val fraction: Float)

/** MobileScreens.swift PhoneFilesPage: search, a transfer, places, recent, and sending files to the Mac. */
@Composable
fun FilesPage(places: List<PlaceRow>, recent: List<FileRow>, transfer: Transfer? = null, insets: PhoneInsets = PhoneInsets.Lab) {
    Column(Modifier.fillMaxSize()) {
        TopBar(PhoneTab.FILES)
        Column(Modifier.weight(1f).padding(horizontal = 16.dp)) {
            Row(
                Modifier.padding(top = 14.dp).fillMaxWidth().heightIn(min = 50.dp).background(Ink.black, CircleShape).padding(horizontal = 16.dp)
                    .semantics(mergeDescendants = true) { contentDescription = "Search your Mac" },
                horizontalArrangement = Arrangement.spacedBy(10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                FerbIcon(FerbIcons.search, 17f, Ink.onDark)
                PhoneText("Search your Mac", 16.5f, color = Ink.onDark)
            }
            if (transfer != null) {
                Card(Modifier.padding(top = 12.dp), padding = 18.dp) {
                    Row(Modifier.fillMaxWidth().semantics(mergeDescendants = true) { liveRegion = LiveRegionMode.Polite }) {
                        PhoneText(transfer.title, 16f, FerbType.Semibold, Ink.black, Modifier.weight(1f))
                        PhoneText("${(transfer.fraction * 100).toInt()}%", 15f, FerbType.Medium, Ink.faint)
                    }
                    Box(Modifier.padding(top = 10.dp).fillMaxWidth().height(6.dp).background(Ink.line, CircleShape)) {
                        Box(Modifier.fillMaxHeight().fillMaxWidth(transfer.fraction).background(Ink.black, CircleShape))
                    }
                }
            }
            SkyLabel("Places")
            Card {
                places.forEachIndexed { i, p ->
                    if (i > 0) Divide()
                    CardRow(p.title, leading = { RowIcon(p.icon) }, trailing = { Opens(p.count) })
                }
            }
            SkyLabel("Recent")
            Card {
                recent.forEachIndexed { i, f ->
                    if (i > 0) Divide()
                    CardRow(f.name, f.detail, leading = { FileIcon(f.name, 26.dp) }, trailing = { Opens() })
                }
            }
            Spacer(Modifier.weight(1f))
            Pill("Send files to your Mac", Modifier.paddingOverBottom(insets, 8.dp), PillKind.BLACK, FerbIcons.sendFile)
        }
    }
}

/**
 * MobileScreens.swift MacPage: the phone and the Mac joined by a still line of dots, how they are connected, and
 * the Mac's own rows.
 */
@Composable
fun MacPage(
    link: LinkKind,
    macName: String = "MacBook Pro",
    ownRelay: String? = "relay.example.net",
    status: String? = null,
    note: String? = null,
    name: String? = null,
    waiting: PhoneTab? = PhoneTab.TASKS,
    onTab: ((PhoneTab) -> Unit)? = null,
    onProfile: (() -> Unit)? = null,
    onLink: (() -> Unit)? = null,
    onOwnRelay: (() -> Unit)? = null,
    onSettings: (() -> Unit)? = null,
    onStopAll: (() -> Unit)? = null,
    /** Not from the lab: a card above the rows (FERB may use the Mac's screen until the owner is back, PHN-611). */
    notice: (@Composable () -> Unit)? = null,
) {
    val ink = LocalSkyInk.current
    Column(Modifier.fillMaxSize()) {
        TopBar(PhoneTab.MAC, waiting = waiting, onTab = onTab, onProfile = onProfile)
        Spacer(Modifier.weight(1f))
        LinkPicture(link, macName, Modifier.pressable(onLink), status = status, name = name)
        if (note != null) {
            PhoneText(note, 14f, FerbType.Medium, ink.low, Modifier.fillMaxWidth().padding(start = 32.dp, end = 32.dp, top = 14.dp),
                align = androidx.compose.ui.text.style.TextAlign.Center)
        }
        Spacer(Modifier.weight(1f))
        notice?.invoke()
        Card(Modifier.padding(start = 16.dp, end = 16.dp, bottom = 40.dp)) {
            CardRow("Your own relay", ownRelay ?: "Not set", onClick = onOwnRelay, leading = { RowIcon(FerbIcons.server) }, trailing = { Opens() })
            Divide()
            CardRow("FERB's settings", "Each change asks for your fingerprint", onClick = onSettings, leading = { RowIcon(FerbIcons.sliders) }, trailing = { Opens() })
            Divide()
            CardRow("Stop everything on the Mac", danger = true, onClick = onStopAll, leading = { RowIcon(FerbIcons.stopCircle, danger = true) })
        }
    }
}

/** MobileScreens.swift UnlockPage: FERB opens with the fingerprint every time (PHN-8). */
@Composable
fun UnlockPage(insets: PhoneInsets = PhoneInsets.Lab, onUnlock: (() -> Unit)? = {}) {
    val ink = LocalSkyInk.current
    Column(Modifier.fillMaxSize(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(14.dp)) {
        Spacer(Modifier.height(150.dp))
        FerbMark(84.dp, Modifier.launchTarget())
        PhoneText("FERB is locked", FerbType.Role.lockTitle, FerbType.Semibold, ink.top, Modifier.padding(top = 6.dp).semantics { heading() })
        PhoneText("It opens with your fingerprint.", 16f, FerbType.Medium, ink.top)
        Spacer(Modifier.weight(1f))
        Card(Modifier.paddingOverBottom(insets, 8.dp, start = 16.dp, end = 16.dp), padding = 16.dp) {
            Pill("Unlock with fingerprint", icon = FerbIcons.fingerprint, onClick = onUnlock)
            PhoneText("Nothing in FERB can be read until it is unlocked.", 14f, color = Ink.detail, modifier = Modifier.padding(top = 12.dp))
        }
    }
}

/** MobileScreens.swift PairPage: first run, the Mac a dashed circle until it is paired, one button. */
@Composable
fun PairPage(
    insets: PhoneInsets = PhoneInsets.Lab,
    onScan: (() -> Unit)? = {},
    onTypeCode: (() -> Unit)? = {},
    onGetMacApp: (() -> Unit)? = {},
    onHelp: (() -> Unit)? = {},
) {
    Column(Modifier.fillMaxSize()) {
        Spacer(Modifier.weight(1f))
        LinkPicture(LinkKind.SEARCH)
        Spacer(Modifier.weight(1f))
        Card(Modifier.paddingOverBottom(insets, 8.dp, start = 16.dp, end = 16.dp), padding = 20.dp) {
            Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                PhoneText("Use FERB from your phone", 24f, FerbType.Semibold, modifier = Modifier.semantics { heading() })
                PairStep(1, "On your Mac, open FERB and choose Pair a phone.")
                PairStep(2, "Scan the code it shows.")
                PairStep(3, "Check both codes match, then say yes on the Mac.")
                Pill("Scan the code", Modifier.padding(top = 4.dp), icon = FerbIcons.qrScan, onClick = onScan)
                Row(Modifier.padding(top = 2.dp)) {
                    PairAction(FerbIcons.keyboard, "Type code", Modifier.weight(1f), onTypeCode)
                    PairAction(FerbIcons.laptop, "Get the Mac app", Modifier.weight(1f), onGetMacApp)
                    PairAction(FerbIcons.help, "Help", Modifier.weight(1f), onHelp)
                }
            }
        }
    }
}

/** MobileScreens.swift CodesPage: both screens show the same code; the owner says yes on the Mac only if they match. */
@Composable
fun CodesPage(code: String, insets: PhoneInsets = PhoneInsets.Lab, onCancel: (() -> Unit)? = {}) {
    Column(Modifier.fillMaxSize()) {
        Spacer(Modifier.weight(1f))
        Card(Modifier.paddingOverBottom(insets, 8.dp, start = 16.dp, end = 16.dp), padding = 20.dp) {
            Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Row(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.size(10.dp).background(Ink.amber, CircleShape))
                    PhoneText("Now say yes on your Mac", 22f, FerbType.Semibold, modifier = Modifier.semantics { heading() })
                }
                PhoneText("Your Mac shows this phone's name and a code. Say yes there only if it is this one:", 15.5f, color = Ink.detail, lineSpacing = 2f)
                PairingCode(code, Modifier.padding(vertical = 10.dp))
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp, Alignment.CenterHorizontally), verticalAlignment = Alignment.CenterVertically) {
                    Mark(MarkKind.RUNNING, 20.dp)
                    PhoneText("Waiting for your Mac", 15f, color = Ink.detail)
                }
                Pill("Cancel", kind = PillKind.PLAIN, height = 44.dp, onClick = onCancel)
            }
        }
    }
}
