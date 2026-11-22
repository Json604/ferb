package com.ferb.app.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.ferb.app.FerbViewModel
import com.ferb.app.R
import com.ferb.app.Sheet
import com.ferb.app.model.NewSessionUi
import com.ferb.app.model.SessionUi
import com.ferb.app.model.Sessions
import com.ferb.app.model.SessionsUi
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.MotionKind
import com.ferb.design.MotionSwitch
import com.ferb.design.components.CardRow
import com.ferb.design.components.Divide
import com.ferb.design.components.ExactCommand
import com.ferb.design.components.PhoneTab
import com.ferb.design.components.PhoneText
import com.ferb.design.components.Pill
import com.ferb.design.components.PillKind
import com.ferb.design.components.RowIcon
import com.ferb.design.components.SessionAction
import com.ferb.design.components.SessionAsk
import com.ferb.design.components.SessionComposer
import com.ferb.design.components.SessionRowData
import com.ferb.design.components.Sheet as SheetFrame
import com.ferb.design.components.SheetTitle
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.SessionPage
import com.ferb.design.screens.SessionsOff
import com.ferb.design.screens.SessionsPage
import com.ferb.mobile.AgentSession
import com.ferb.mobile.SessionYes

// The owner's coding sessions (kb HAND-651..666): the Sessions tab, one session's screen, its window live, and starting one.
// The words are the app's (strings.xml, the same English as FERB.app); the agent's own words are shown as plain text.

/** A session's row and header: its title (or folder), "<folder> · in <app>" (or in the background), its state, FERB's badge. */
@Composable
internal fun sessionRow(s: AgentSession): SessionRowData {
    val where = if (s.host.isBlank()) stringResource(R.string.sessions_in_background) else stringResource(R.string.sessions_in_app, s.host)
    val state = Sessions.state(s.status)
    val stateText = stringResource(when (state) {
        com.ferb.design.components.SessionState.WORKING -> R.string.sessions_working
        com.ferb.design.components.SessionState.WAITING -> R.string.sessions_waiting
        com.ferb.design.components.SessionState.IDLE -> R.string.sessions_idle
    })
    val folder = s.folderName.ifBlank { s.folder }
    return SessionRowData(Sessions.title(s), if (folder.isBlank()) where else "$folder · $where", state, stateText,
        ferb = if (s.withFerb) stringResource(R.string.sessions_with_ferb) else null)
}

/** The Sessions tab: the list while it shows (the view model reads it every few seconds), off, empty, New session. */
@Composable
fun SessionsTab(vm: FerbViewModel, insets: PhoneInsets, waiting: PhoneTab?, onTab: (PhoneTab) -> Unit, onProfile: () -> Unit) {
    val ui by vm.sessions.collectAsState()
    SessionsTabPage(ui, insets, waiting, onTab, onProfile, onRow = vm::openSession, onNew = { vm.newSession() }, onTurnOn = { vm.turnOnSessions() })
}

/** The tab's page from its state (the screenshots draw it too). */
@Composable
fun SessionsTabPage(
    ui: SessionsUi, insets: PhoneInsets, waiting: PhoneTab?, onTab: ((PhoneTab) -> Unit)?, onProfile: (() -> Unit)?,
    onRow: (String) -> Unit = {}, onNew: () -> Unit = {}, onTurnOn: () -> Unit = {},
) {
    val rows = ui.sessions.map { sessionRow(it) }
    SessionsPage(
        stringResource(R.string.sessions_title), rows, stringResource(R.string.sessions_new), insets,
        off = if (ui.loaded && !ui.enabled) SessionsOff(stringResource(R.string.sessions_off), stringResource(R.string.sessions_off_more),
            stringResource(R.string.sessions_turn_on)) else null,
        empty = when {
            ui.problem != null -> noteText(ui.problem)
            ui.loaded -> stringResource(R.string.sessions_none)
            else -> null
        },
        waiting = waiting, onTab = onTab, onProfile = onProfile,
        onRow = { i -> ui.sessions.getOrNull(i)?.let { onRow(it.id) } },
        onNew = onNew, onTurnOn = onTurnOn,
    )
}

/**
 * One session's screen (always out of screenshots: it holds the owner's conversation). Over it, as sheets of their own:
 * the Mac's question before FERB types for him, and Hand to FERB. Back closes what is over it first.
 */
@Composable
fun SessionScreen(vm: FerbViewModel, busy: Set<String>, insets: PhoneInsets) {
    val o by vm.openSession.collectAsState()
    val ui = o ?: return
    BackHandler {
        when {
            ui.ask != null -> vm.sessionNo()
            ui.handOver -> vm.askHandOver(false)
            else -> vm.closeSession()
        }
    }
    SessionScreenPage(
        ui, busy, insets,
        onClose = vm::closeSession, onOlder = { vm.sessionOlder() }, onTyped = vm::sessionTyped, onSend = { vm.sessionSend() },
        onChoice = { card, choice -> vm.sessionAnswer(card, choice) },
        onWatch = vm::watchSession, onHand = { vm.askHandOver() }, onInterrupt = { vm.sessionInterrupt() }, onRelease = { vm.sessionRelease() },
    )
    // over the screen: one at a time, up from below like every sheet
    val over: Any? = ui.ask ?: if (ui.handOver) HandOver else null
    MotionSwitch(over, Modifier.fillMaxSize(), MotionKind.SHEET) { what ->
        when (what) {
            is com.ferb.app.model.SessionConfirm -> SessionYesSheet(
                what.yes, stringResource(R.string.sessions_yes_send), insets, busy = "session-yes" in busy,
                onYes = { vm.sessionYes() }, onCancel = vm::sessionNo,
            )
            HandOver -> HandOverSheet(insets, busy = "session-hand" in busy, onGo = { vm.handOver(it) }, onClose = { vm.askHandOver(false) })
        }
    }
}

private object HandOver

/** The session's page from its state (the screenshots draw it too). */
@Composable
fun SessionScreenPage(
    ui: SessionUi, busy: Set<String>, insets: PhoneInsets,
    onClose: () -> Unit = {}, onOlder: () -> Unit = {}, onTyped: ((String) -> Unit)? = null, onSend: () -> Unit = {},
    onChoice: (card: String, choice: String) -> Unit = { _, _ -> }, onWatch: () -> Unit = {}, onHand: () -> Unit = {},
    onInterrupt: () -> Unit = {}, onRelease: () -> Unit = {},
) {
    val s = ui.session
    val header = s?.let { sessionRow(it) } ?: SessionRowData("", "", com.ferb.design.components.SessionState.IDLE, "")
    val agent = s?.agentName.orEmpty()
    val stopped = stringResource(R.string.sessions_interrupted)
    val lines = remember(ui.items, stopped) { ui.items.map { Sessions.line(it, stopped) } }
    val onMac = stringResource(R.string.sessions_answer_on_mac)
    val asks = ui.cards.map { c ->
        val title = if (c.kind == "permission" && c.tool.isNotBlank()) stringResource(R.string.sessions_wants_tool, agent, c.tool)
            else stringResource(R.string.sessions_asks, agent)
        SessionAsk(title, c.question, c.detail, c.choices.map { it.label }, onMac, key = c.id)
    }
    val actions = buildList {
        add(SessionAction(stringResource(R.string.sessions_watch), FerbIcons.eye, onWatch))
        add(SessionAction(stringResource(R.string.sessions_hand_over), null, onHand))
        if (s != null && s.status != "idle") {
            add(SessionAction(stringResource(R.string.sessions_stop_turn), FerbIcons.stop, onInterrupt.takeIf { "session-interrupt" !in busy }))
        }
        if (s?.controlledHere == true) add(SessionAction(stringResource(R.string.sessions_let_go), null, onRelease.takeIf { "session-release" !in busy }))
    }
    SessionPage(
        header, lines, asks, actions, insets,
        older = if (ui.more) stringResource(R.string.sessions_older) else null,
        onOlder = onOlder.takeIf { "session-older" !in busy },
        onClose = onClose,
        onChoice = { a, c ->
            val card = ui.cards.getOrNull(a)
            val choice = card?.choices?.getOrNull(c)
            if (card != null && choice != null && "session-answer:${card.id}" !in busy) onChoice(card.id, choice.id)
        },
    ) { m ->
        Column(m.testTag("session-foot")) {
            ui.problem?.let { PhoneText(noteText(it), 13.5f, color = Ink.onDark, modifier = Modifier.padding(start = 6.dp, end = 6.dp, bottom = 8.dp)) }
            if (s == null || s.canSend) {
                SessionComposer(
                    ui.draft, stringResource(R.string.sessions_message_hint, agent.ifBlank { header.title }), stringResource(R.string.sessions_send),
                    Modifier.testTag("session-composer"), onValueChange = onTyped, onSend = onSend.takeIf { "session-send" !in busy },
                )
            } else {
                PhoneText(s.noSend, 14.5f, color = Ink.onDark, modifier = Modifier.padding(horizontal = 6.dp, vertical = 10.dp).testTag("session-no-send"))
            }
        }
    }
}

/**
 * The Mac's question before FERB types for the owner (GATE-622) or starts a session: its question as the title, the exact
 * words or command in a fixed-width face, what the Yes covers, and Yes / Cancel. Nothing was sent before his Yes.
 */
@Composable
internal fun SessionYesSheet(yes: SessionYes, yesLabel: String, insets: PhoneInsets, busy: Boolean, onYes: () -> Unit, onCancel: () -> Unit) {
    SheetFrame(title = yes.question, bottomInset = insets.bottom, onDismiss = onCancel) {
        SheetTitle(yes.question, onCancel)
        if (yes.detail.isNotEmpty()) {
            Box(Modifier.padding(top = 14.dp).heightIn(max = 260.dp).verticalScroll(rememberScrollState())) { ExactCommand(yes.detail) }
        }
        if (yes.covers.isNotBlank()) PhoneText(yes.covers, 14f, color = Ink.detail, lineSpacing = 2f, modifier = Modifier.padding(top = 12.dp))
        Pill(yesLabel, Modifier.padding(top = 18.dp).testTag("session-yes"), icon = FerbIcons.fingerprint, onClick = if (busy) null else onYes)
        Pill(stringResource(R.string.sessions_cancel), Modifier.padding(top = 4.dp), PillKind.PLAIN, height = 44.dp, onClick = onCancel)
    }
}

/** Hand this session to FERB: the owner's goal, what FERB will do, Hand it over (signed). */
@Composable
internal fun HandOverSheet(insets: PhoneInsets, busy: Boolean, onGo: (String) -> Unit, onClose: () -> Unit) {
    var goal by remember { mutableStateOf("") }
    SheetFrame(title = stringResource(R.string.sessions_hand_title), bottomInset = insets.bottom, onDismiss = onClose) {
        SheetTitle(stringResource(R.string.sessions_hand_title), onClose)
        PhoneText(stringResource(R.string.sessions_hand_label), 15.5f, FerbType.Semibold, Ink.black, Modifier.padding(top = 12.dp))
        Field(goal, { goal = it }, stringResource(R.string.sessions_hand_hint), Modifier.padding(top = 8.dp).testTag("hand-goal"))
        PhoneText(stringResource(R.string.sessions_hand_note), 14f, color = Ink.detail, lineSpacing = 2f, modifier = Modifier.padding(top = 12.dp))
        Pill(stringResource(R.string.sessions_hand_go), Modifier.padding(top = 18.dp).testTag("hand-go"),
            onClick = if (busy || goal.isBlank()) null else ({ onGo(goal) }))
    }
}

/** Watch: the session's window live (Watch live's page and rules: memory only, out of screenshots, not on the shared relay). */
@Composable
fun SessionLiveScreen(vm: FerbViewModel, insets: PhoneInsets) {
    val view by vm.live.view.collectAsState()
    val o by vm.openSession.collectAsState()
    val v = view ?: return
    BackHandler { vm.closeLive() }
    val name = o?.session?.let { Sessions.title(it) }.orEmpty()
    LivePage(v, insets, onClose = vm::closeLive, onStop = null, onAgain = if (v.ended != null) ({ vm.watchAgain() }) else null,
        title = stringResource(R.string.sessions_watch_title, name))
}

/** New session: which agent, which folder, the first words; then the Mac's exact command and the owner's Yes. */
@Composable
fun NewSessionSheet(vm: FerbViewModel, busy: Set<String>, insets: PhoneInsets) {
    val n by vm.newSession.collectAsState()
    val ui = n ?: return
    NewSessionPage(ui, busy, insets, onAgent = vm::newSessionAgent, onFolder = vm::newSessionFolder, onText = vm::newSessionText,
        onNext = { vm.newSessionNext() }, onStart = { vm.newSessionStart() }, onBack = vm::newSessionBack, onClose = vm::closeNewSession)
}

@Composable
fun NewSessionPage(
    ui: NewSessionUi, busy: Set<String>, insets: PhoneInsets,
    onAgent: (String) -> Unit = {}, onFolder: (String) -> Unit = {}, onText: (String) -> Unit = {},
    onNext: () -> Unit = {}, onStart: () -> Unit = {}, onBack: () -> Unit = {}, onClose: () -> Unit = {},
) {
    val yes = ui.yes
    if (yes != null) {
        SessionYesSheet(yes, stringResource(R.string.sessions_start), insets, busy = "session-start" in busy, onYes = onStart, onCancel = onBack)
        return
    }
    val title = stringResource(R.string.sessions_new)
    SheetFrame(title = title, bottomInset = insets.bottom, onDismiss = onClose) {
        SheetTitle(title, onClose)
        Column(Modifier.heightIn(max = 520.dp).verticalScroll(rememberScrollState())) {
            PhoneText(stringResource(R.string.sessions_new_agent), 15.5f, FerbType.Semibold, Ink.black, Modifier.padding(top = 12.dp, bottom = 8.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                for ((id, name) in Sessions.AGENTS) {
                    Pill(name, Modifier.weight(1f).testTag("agent-$id"), if (ui.agent == id) PillKind.BLACK else PillKind.GREY, height = 44.dp,
                        onClick = { onAgent(id) })
                }
            }
            PhoneText(stringResource(R.string.sessions_new_folder), 15.5f, FerbType.Semibold, Ink.black, Modifier.padding(top = 16.dp, bottom = 8.dp))
            if (ui.folders.isNotEmpty()) {
                // the sheet's grey fill for the folders: a white card would vanish on the white sheet
                Column(Modifier.fillMaxWidth().background(Ink.fill, RoundedCornerShape(18.dp)).testTag("new-folders")) {
                    ui.folders.forEachIndexed { i, f ->
                        if (i > 0) Divide()
                        val name = f.trimEnd('/').substringAfterLast('/').ifEmpty { f }
                        CardRow(name, f, onClick = { onFolder(f) }, leading = { RowIcon(FerbIcons.folder) },
                            trailing = { if (f == ui.folder) com.ferb.design.FerbIcon(FerbIcons.check, 17f, Ink.black) })
                    }
                }
            } else if (ui.loading) {
                com.ferb.design.components.RunningArc(20.dp, Ink.black)
            }
            PhoneText(stringResource(R.string.sessions_new_first), 15.5f, FerbType.Semibold, Ink.black, Modifier.padding(top = 16.dp, bottom = 8.dp))
            Field(ui.text, onText, stringResource(R.string.sessions_message_hint, Sessions.AGENTS.firstOrNull { it.first == ui.agent }?.second.orEmpty()),
                Modifier.testTag("new-first"))
        }
        Pill(stringResource(R.string.sessions_start), Modifier.padding(top = 18.dp).testTag("new-start"),
            onClick = if (ui.folder == null || "session-prepare" in busy) null else onNext)
    }
}
