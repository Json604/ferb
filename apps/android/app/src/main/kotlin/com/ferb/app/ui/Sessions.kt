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
import androidx.compose.ui.draw.alpha
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
import com.ferb.design.components.SessionOutgoing
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

// The owner's sessions (kb HAND-691, HAND-651..666): the Sessions tab, one session's screen, its window live, and opening a
// new terminal.
// The words are the app's (strings.xml, the same English as FERB.app); the agent's own words are shown as plain text.

/**
 * A session's row and header: its title (or its window, or folder), "<program> · <folder> · in <app>" (HAND-692; no
 * program: "<folder> · in <app>"; no app: in the background), its state, FERB's badge.
 */
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
    val subtitle = listOf(s.program, folder, where).filter { it.isNotBlank() }.joinToString(" · ")
    return SessionRowData(Sessions.title(s), subtitle, state, stateText,
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
        notes = ui.noChats.map { stringResource(R.string.sessions_no_chat, it) },
        coming = if (ui.coming != null) stringResource(R.string.sessions_coming) else null,
        waiting = waiting, onTab = onTab, onProfile = onProfile,
        onRow = { i -> ui.sessions.getOrNull(i)?.let { onRow(it.id) } },
        onNew = onNew, onTurnOn = onTurnOn,
    )
}

/**
 * One session's screen (screenshots as the owner's switch says, PHN-800). Over it, as sheets of their own:
 * the Mac's question before FERB types for him, and Hand to FERB. Back closes what is over it first.
 */
@Composable
fun SessionScreen(vm: FerbViewModel, busy: Set<String>, insets: PhoneInsets) {
    val o by vm.openSession.collectAsState()
    val ui = o ?: return
    BackHandler {
        when {
            ui.menu -> vm.sessionMenu(false)
            ui.pick != null -> vm.sessionPickNo()
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
        onRetry = { vm.sessionRetry() }, onMenu = vm::sessionMenu, onTakeBack = { vm.takeBack() },
    )
    // over the screen: one at a time, up from below like every sheet
    val over: Any? = ui.pick ?: ui.ask ?: if (ui.handOver) HandOver else null
    MotionSwitch(over, Modifier.fillMaxSize(), MotionKind.SHEET) { what ->
        when (what) {
            is com.ferb.app.model.SessionConfirm -> SessionYesSheet(
                what.yes, stringResource(R.string.sessions_yes_send), insets, busy = "session-yes" in busy,
                onYes = { vm.sessionYes() }, onCancel = vm::sessionNo,
            )
            HandOver -> HandOverSheet(insets, busy = "session-hand" in busy, onGo = { vm.handOver(it) }, onClose = { vm.askHandOver(false) },
                problem = ui.handProblem?.let { noteText(it) })
            is com.ferb.app.model.TerminalPick -> PickTerminalSheet(what, insets, busy = "session-pick" in busy,
                onPick = { vm.sessionPickTerminal(it) }, onNo = vm::sessionPickNo)
        }
    }
}

private object HandOver

/** The day a session's lines are dated against ("Today", "Yesterday"): the clock's, or a fixed one (screenshots). */
val LocalSessionToday = androidx.compose.runtime.staticCompositionLocalOf<Long?> { null }

/** The session's page from its state (the screenshots draw it too). */
@Composable
fun SessionScreenPage(
    ui: SessionUi, busy: Set<String>, insets: PhoneInsets,
    onClose: () -> Unit = {}, onOlder: () -> Unit = {}, onTyped: ((String) -> Unit)? = null, onSend: () -> Unit = {},
    onChoice: (card: String, choice: String) -> Unit = { _, _ -> }, onWatch: () -> Unit = {}, onHand: () -> Unit = {},
    onInterrupt: () -> Unit = {}, onRelease: () -> Unit = {}, onRetry: () -> Unit = {}, onMenu: (Boolean) -> Unit = {},
    onTakeBack: () -> Unit = {},
) {
    val s = ui.session
    val header = s?.let { sessionRow(it) } ?: SessionRowData("", "", com.ferb.design.components.SessionState.IDLE, "")
    val agent = s?.agentName.orEmpty()
    val stopped = stringResource(R.string.sessions_interrupted)
    // each day's first line carries its day, as the main chat marks a day's start (PHN-813)
    val days = remember(ui.items) { Sessions.days(ui.items, java.time.ZoneId.systemDefault()) }
    val today = LocalSessionToday.current ?: com.ferb.app.model.ChatModel.chatDay(System.currentTimeMillis(), java.time.ZoneId.systemDefault())
    val labels = days.mapValues { (_, d) -> dayLabel(d, today) }
    // an agent's reply drawn by its parts or its Markdown (HAND-703); a terminal's output as it is
    val terminal = Sessions.terminal(s)
    val lines = remember(ui.items, stopped, labels, terminal) { ui.items.map { Sessions.line(it, stopped, terminal).copy(day = labels[it.seq]) } }
    val onMac = stringResource(R.string.sessions_answer_on_mac)
    val asks = ui.cards.map { c ->
        val title = if (c.kind == "permission" && c.tool.isNotBlank()) stringResource(R.string.sessions_wants_tool, agent, c.tool)
            else stringResource(R.string.sessions_asks, agent)
        SessionAsk(title, c.question, c.detail, c.choices.map { it.label }, onMac, key = c.id)
    }
    val actions = buildList {
        add(SessionAction(stringResource(R.string.sessions_watch), FerbIcons.eye, onWatch))
        // FERB has it (its task): Take it back stops that task; handed but not started yet: FERB is starting (HAND-702)
        when {
            s != null && s.ferbTask.isNotBlank() -> {
                val taking = "session-take-back" in busy
                add(SessionAction(stringResource(if (taking) R.string.sessions_taking_back else R.string.sessions_take_back), FerbIcons.takeBack,
                    onTakeBack, busy = taking, keepOpen = true, tag = "session-take-back"))
            }
            s != null && s.withFerb -> add(SessionAction(stringResource(R.string.sessions_ferb_starting), FerbIcons.handOver, null, tag = "session-ferb-starting"))
            else -> add(SessionAction(stringResource(R.string.sessions_hand_over), FerbIcons.handOver, onHand, tag = "session-hand-over"))
        }
        if (s != null && s.status != "idle") {
            add(SessionAction(stringResource(R.string.sessions_stop_turn), FerbIcons.stop, onInterrupt.takeIf { "session-interrupt" !in busy }))
        }
        if (s?.controlledHere == true) add(SessionAction(stringResource(R.string.sessions_let_go), FerbIcons.letGo, onRelease.takeIf { "session-release" !in busy }))
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
        outgoing = ui.outgoing?.let {
            SessionOutgoing(it.text, it.phase == com.ferb.app.model.SessionSendPhase.SENDING, it.reason?.let { r -> noteText(r) },
                stringResource(R.string.outbox_sending), stringResource(R.string.sessions_try_again), stringResource(R.string.sessions_not_sent))
        },
        onRetry = onRetry.takeIf { "session-send" !in busy },
        moreLabel = stringResource(R.string.sessions_more),
        menuOpen = ui.menu,
        onMenu = onMenu,
        toLatest = stringResource(R.string.chat_to_latest),
        newMessage = stringResource(R.string.chat_new_message),
        // its first read on its way and no answer yet: the waiting look; an error only once the Mac answered (PHN-816)
        loading = if (!ui.loaded && ui.problem == null) stringResource(R.string.sessions_loading) else null,
        // FERB's latest word on the hand-over, as the Mac says it (HAND-702): above the box, or under the header with none
        note = s?.ferbNote?.takeIf { it.isNotBlank() },
        noteOnTop = s != null && !s.canSend,
    ) { m ->
        Column(m.testTag("session-foot")) {
            ui.problem?.let { PhoneText(noteText(it), 13.5f, color = Ink.onDark, modifier = Modifier.padding(start = 6.dp, end = 6.dp, bottom = 8.dp)) }
            if (s == null || s.canSend) {
                SessionComposer(
                    ui.draft, stringResource(R.string.sessions_message_hint, agent.ifBlank { header.title }), stringResource(R.string.sessions_send),
                    Modifier.testTag("session-composer"), onValueChange = onTyped, onSend = onSend.takeIf { "session-send" !in busy },
                    sending = ui.outgoing?.phase == com.ferb.app.model.SessionSendPhase.SENDING,
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
internal fun SessionYesSheet(
    yes: SessionYes, yesLabel: String, insets: PhoneInsets, busy: Boolean, onYes: () -> Unit, onCancel: () -> Unit,
    busyLabel: String? = null, problem: String? = null,
) {
    SheetFrame(title = yes.question, bottomInset = insets.bottom, onDismiss = onCancel) {
        SheetTitle(yes.question, onCancel)
        if (yes.detail.isNotEmpty()) {
            Box(Modifier.padding(top = 14.dp).heightIn(max = 260.dp).verticalScroll(rememberScrollState())) { ExactCommand(yes.detail) }
        }
        if (yes.covers.isNotBlank()) PhoneText(yes.covers, 14f, color = Ink.detail, lineSpacing = 2f, modifier = Modifier.padding(top = 12.dp))
        Pill(if (busy && busyLabel != null) busyLabel else yesLabel, Modifier.padding(top = 18.dp).testTag("session-yes"), icon = FerbIcons.fingerprint,
            onClick = if (busy) null else onYes, busy = busy && busyLabel != null)
        problem?.let { Problem(it) }
        Pill(stringResource(R.string.sessions_cancel), Modifier.padding(top = 4.dp), PillKind.PLAIN, height = 44.dp, onClick = if (busy) null else onCancel)
    }
}

/**
 * HAND-683: FERB can't tell which terminal is the session's: the Mac's reason, the terminals it can see in the app by
 * the app's own words (the one in front now first), each a choice signed with the fingerprint, and Don't send.
 */
@Composable
internal fun PickTerminalSheet(pick: com.ferb.app.model.TerminalPick, insets: PhoneInsets, busy: Boolean, onPick: (String) -> Unit, onNo: () -> Unit) {
    val title = stringResource(R.string.sessions_pick_title)
    SheetFrame(title = title, bottomInset = insets.bottom, onDismiss = onNo) {
        SheetTitle(title, onNo)
        if (pick.why.isNotBlank()) PhoneText(pick.why, 14.5f, color = Ink.detail, lineSpacing = 2f, modifier = Modifier.padding(top = 10.dp))
        Column(Modifier.padding(top = 14.dp).fillMaxWidth().heightIn(max = 340.dp).verticalScroll(rememberScrollState())
            .background(Ink.fill, RoundedCornerShape(18.dp)).testTag("pick-terminals")) {
            pick.terminals.forEachIndexed { i, t ->
                if (i > 0) Divide()
                CardRow(t.label, onClick = if (busy) null else ({ onPick(t.id) }), leading = { RowIcon(FerbIcons.terminal) },
                    trailing = { com.ferb.design.FerbIcon(FerbIcons.fingerprint, 17f, Ink.detail) })
            }
        }
        PhoneText(stringResource(R.string.sessions_pick_note), 14f, color = Ink.detail, lineSpacing = 2f, modifier = Modifier.padding(top = 12.dp))
        Pill(stringResource(R.string.sessions_pick_no), Modifier.padding(top = 12.dp), PillKind.PLAIN, height = 44.dp, onClick = onNo)
    }
}

/**
 * Hand this session to FERB: the owner's goal, what FERB will do, Hand it over (signed). From the tap until the Mac
 * answers the button turns and says so (HAND-702); a refusal stays with his words and the Mac's reason ([problem]).
 */
@Composable
internal fun HandOverSheet(insets: PhoneInsets, busy: Boolean, onGo: (String) -> Unit, onClose: () -> Unit, problem: String? = null, initial: String = "") {
    var goal by remember { mutableStateOf(initial) }
    SheetFrame(title = stringResource(R.string.sessions_hand_title), bottomInset = insets.bottom, onDismiss = onClose) {
        SheetTitle(stringResource(R.string.sessions_hand_title), onClose)
        PhoneText(stringResource(R.string.sessions_hand_label), 15.5f, FerbType.Semibold, Ink.black, Modifier.padding(top = 12.dp))
        Field(goal, { goal = it }, stringResource(R.string.sessions_hand_hint), Modifier.padding(top = 8.dp).testTag("hand-goal"))
        PhoneText(stringResource(R.string.sessions_hand_note), 14f, color = Ink.detail, lineSpacing = 2f, modifier = Modifier.padding(top = 12.dp))
        Pill(stringResource(if (busy) R.string.sessions_hand_going else R.string.sessions_hand_go), Modifier.padding(top = 18.dp).testTag("hand-go"),
            onClick = if (busy || goal.isBlank()) null else ({ onGo(goal) }), busy = busy)
        problem?.let { Problem(it) }
    }
}

/** The Mac's reason under a sheet's button, after a refusal: a red dot and its words. */
@Composable
private fun Problem(text: String) {
    Row(Modifier.fillMaxWidth().padding(top = 10.dp).testTag("sheet-problem"), horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = androidx.compose.ui.Alignment.Top) {
        com.ferb.design.components.LampDot(com.ferb.design.Lamp.red, 7.dp, Modifier.padding(top = 6.dp))
        PhoneText(text, 14f, color = Ink.detail, lineSpacing = 2f, modifier = Modifier.weight(1f))
    }
}

/** Watch: the session's window live (Watch live's page and rules: memory only, screenshots as the owner's switch says, not on the shared relay). */
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

/**
 * New session (HAND-691): a terminal on the Mac. Only where (the folders the Mac offers, or another typed) and an optional
 * command; with a command, the Mac's exact command and the owner's Yes. No agent to pick: he types the one he wants.
 */
@Composable
fun NewSessionSheet(vm: FerbViewModel, busy: Set<String>, insets: PhoneInsets) {
    val n by vm.newSession.collectAsState()
    val ui = n ?: return
    // the Mac's folders over the sheet: Back goes up a folder, then to the sheet as it was (PHN-821)
    BackHandler(enabled = ui.browse != null) { vm.newSessionBrowseBack() }
    NewSessionPage(ui, busy, insets, onFolder = vm::newSessionFolder, onText = vm::newSessionText,
        onNext = { vm.newSessionNext() }, onStart = { vm.newSessionStart() }, onBack = vm::newSessionBack, onClose = vm::closeNewSession,
        onBrowse = vm::newSessionBrowse, onBrowseOpen = { path, key -> vm.newSessionBrowseOpen(path, key) }, onBrowseBack = vm::newSessionBrowseBack,
        onBrowseUse = vm::newSessionBrowseUse, onBrowseClose = vm::newSessionBrowseClose)
}

/**
 * The New session sheet from its state (the screenshots draw it too): the folders (the one he chose in the Mac's folders
 * first), "Another folder…" (the Mac's folders, folders only, PHN-821), an optional command, Start. From the tap until
 * the Mac answers Start turns and says it is opening a terminal, the form taking no taps (PHN-822); a refusal stays
 * with the Mac's reason under Start.
 */
@Composable
fun NewSessionPage(
    ui: NewSessionUi, busy: Set<String>, insets: PhoneInsets,
    onFolder: (String) -> Unit = {}, onText: (String) -> Unit = {},
    onNext: () -> Unit = {}, onStart: () -> Unit = {}, onBack: () -> Unit = {}, onClose: () -> Unit = {},
    onBrowse: () -> Unit = {}, onBrowseOpen: (String, Boolean) -> Unit = { _, _ -> }, onBrowseBack: () -> Unit = {},
    onBrowseUse: () -> Unit = {}, onBrowseClose: () -> Unit = {},
) {
    val opening = stringResource(R.string.sessions_opening)
    val problem = ui.problem?.let { noteText(it) }
    val browse = ui.browse
    if (browse != null) {
        MacFolderPicker(browse, "session-browse" in busy, insets, onOpen = onBrowseOpen, onBack = onBrowseBack, onUse = onBrowseUse, onClose = onBrowseClose)
        return
    }
    val yes = ui.yes
    if (yes != null) {
        SessionYesSheet(yes, stringResource(R.string.sessions_start), insets, busy = "session-start" in busy, onYes = onStart, onCancel = onBack,
            busyLabel = opening, problem = problem)
        return
    }
    val pending = "session-prepare" in busy || "session-start" in busy
    val title = stringResource(R.string.sessions_new)
    SheetFrame(title = title, bottomInset = insets.bottom, onDismiss = onClose) {
        SheetTitle(title, onClose)
        Column(Modifier.heightIn(max = 520.dp).verticalScroll(rememberScrollState()).then(if (pending) Modifier.alpha(0.5f) else Modifier).testTag("new-form")) {
            PhoneText(stringResource(R.string.sessions_new_folder), 15.5f, FerbType.Semibold, Ink.black, Modifier.padding(top = 12.dp, bottom = 8.dp))
            // the sheet's grey fill for the folders: a white card would vanish on the white sheet
            Column(Modifier.fillMaxWidth().background(Ink.fill, RoundedCornerShape(18.dp)).testTag("new-folders")) {
                ui.rows.forEachIndexed { i, f ->
                    if (i > 0) Divide()
                    val name = f.trimEnd('/').substringAfterLast('/').ifEmpty { f }
                    CardRow(name, f, onClick = if (pending) null else ({ onFolder(f) }), leading = { RowIcon(FerbIcons.folder) },
                        trailing = { if (f == ui.folder) com.ferb.design.FerbIcon(FerbIcons.check, 17f, Ink.black) })
                }
                if (ui.rows.isEmpty() && ui.loading) {
                    Box(Modifier.fillMaxWidth().padding(14.dp)) { com.ferb.design.components.RunningArc(20.dp, Ink.black) }
                } else if (ui.rows.isNotEmpty()) Divide()
                // the Mac's folders, as the Files tab browses them, to choose any other (PHN-821)
                Box(Modifier.testTag("new-browse")) {
                    CardRow(stringResource(R.string.sessions_new_browse), onClick = if (pending) null else onBrowse,
                        leading = { RowIcon(FerbIcons.folder) }, trailing = { com.ferb.design.components.Opens() })
                }
            }
            PhoneText(stringResource(R.string.sessions_new_command), 15.5f, FerbType.Semibold, Ink.black, Modifier.padding(top = 16.dp, bottom = 8.dp))
            Field(ui.text, if (pending) ({ _ -> }) else onText, stringResource(R.string.sessions_new_command_hint), Modifier.testTag("new-command"))
        }
        Pill(if (pending) opening else stringResource(R.string.sessions_start), Modifier.padding(top = 18.dp).testTag("new-start"),
            onClick = if (ui.chosen == null || pending) null else onNext, busy = pending)
        problem?.let { Problem(it) }
    }
}
