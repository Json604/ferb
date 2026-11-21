package com.ferb.app.ui

import android.view.HapticFeedbackConstants
import androidx.compose.ui.platform.LocalView
import com.ferb.design.components.MicState
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
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
import androidx.compose.runtime.SideEffect
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
import com.ferb.app.VoicePress
import com.ferb.app.FerbViewModel
import com.ferb.app.Note
import com.ferb.app.R
import com.ferb.app.Sheet
import com.ferb.app.UiState
import com.ferb.app.model.CardLine
import com.ferb.app.model.ChatRow
import com.ferb.app.model.Phase
import com.ferb.app.model.WorkItem
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.Phone
import com.ferb.design.MotionKind
import com.ferb.design.MotionSwitch
import com.ferb.design.components.TabBarHost
import com.ferb.design.motionItem
import com.ferb.design.components.ChatWaiting
import com.ferb.design.components.DayDivider
import com.ferb.design.components.ChatJump
import com.ferb.design.components.JumpPill
import com.ferb.design.components.StatusLine
import com.ferb.design.components.StatusTone
import com.ferb.design.components.StickyDay
import com.ferb.design.components.chatJump
import com.ferb.design.MotionShow
import com.ferb.design.components.RunningArc
import com.ferb.design.components.rememberChatFollow
import com.ferb.app.model.ChatModel
import com.ferb.app.model.Sending
import com.ferb.app.model.SendPhase
import com.ferb.design.components.AttachmentChip
import com.ferb.design.components.AttachmentRow
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.runtime.rememberCoroutineScope
import kotlinx.coroutines.launch
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
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.draw.drawBehind
import com.ferb.design.components.Fold
import com.ferb.design.components.FerbRichBubble
import com.ferb.design.components.LocalCardsLive
import com.ferb.design.components.LocalChatViewport
import com.ferb.design.components.ResultCard
import com.ferb.design.components.RichWords
import com.ferb.design.components.SitesChip
import com.ferb.design.components.ActiveWorkRow
import com.ferb.design.components.WorkTask
import com.ferb.design.components.onBlack
import com.ferb.design.components.richText
import com.ferb.design.components.TopBar
import com.ferb.design.components.TypingDots
import com.ferb.design.components.pressable
import com.ferb.design.screens.HomePage
import com.ferb.design.screens.HomeTexts
import com.ferb.design.screens.MacPage
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.paddingOverBottom
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

/** The chat tab's pages (kb PHN-696): today with nothing said yet, today's chat, the earlier days. */
private enum class ChatPage { HOME, TODAY, EARLIER }

/** The four tabs (Mobile.swift TopBar): chat (the home screen until a chat starts), tasks, files, the Mac. */
@Composable
fun Main(vm: FerbViewModel, ui: UiState, tab: PhoneTab, home: Boolean, insets: PhoneInsets, moment: SkyMoment, host: Host) {
    val waiting = if (ui.questions.isNotEmpty() && tab != PhoneTab.TASKS) PhoneTab.TASKS else null
    val draft by vm.draft.collectAsState()
    val onTab: (PhoneTab) -> Unit = vm::choose
    val onProfile = { vm.show(Sheet.Profile) }
    Box(Modifier.fillMaxSize()) {
        // one tab bar whose pill slides; the page under it comes in from the side of its tab (kb PHN-622)
        TabBarHost(Modifier.fillMaxSize()) {
            // the chat tab is one of three pages (kb PHN-696): today's home page, today's chat, or the earlier days
            val earlier by vm.earlier.collectAsState()
            val page = when {
                tab != PhoneTab.CHAT -> ChatPage.TODAY
                earlier -> ChatPage.EARLIER
                home -> ChatPage.HOME
                else -> ChatPage.TODAY
            }
            MotionSwitch(tab to page, Modifier.fillMaxSize(), MotionKind.TAB, direction = { a, b -> b.first.ordinal.compareTo(a.first.ordinal) }) { (t, p) ->
                when (t) {
                    PhoneTab.CHAT -> if (p == ChatPage.HOME) Home(vm, ui, draft, insets, moment, onTab, onProfile, host)
                    else Chat(vm, ui, draft, insets, waiting, onTab, onProfile, host)
                    PhoneTab.TASKS -> Tasks(vm, ui, onTab, onProfile)
                    PhoneTab.FILES -> Files(vm, ui, insets, waiting, onTab, onProfile, host)
                    PhoneTab.MAC -> Mac(vm, ui, waiting, onTab, onProfile)
                }
            }
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
 * The composer every page shares: the owner's draft and send; + picks files that wait in the box as chips until he
 * sends (kb PHN-690: nothing leaves the phone before Send; the cross takes one out), with files already on the Mac
 * from the Files tab beside them; the microphone, held, puts the owner's spoken words in the box (PHN-603).
 */
@Composable
private fun Composer(vm: FerbViewModel, draft: String, placeholder: String, modifier: Modifier, host: Host, running: com.ferb.app.model.RunningUi? = null) {
    val chips by vm.outgoing.chips.collectAsState()
    val ready by vm.transfers.ready.collectAsState()
    val ink = LocalSkyInk.current
    val context = LocalContext.current
    val hasFiles = chips.isNotEmpty() || ready.isNotEmpty()
    Column(modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        val listening by vm.listening.collectAsState()
        val mic by vm.mic.collectAsState()
        val level by vm.micLevel.collectAsState()
        // PHN-643: what the microphone is doing, in words above the box, and a tick when it starts and stops hearing
        val view = LocalView.current
        var was by remember { mutableStateOf(MicState.IDLE) }
        LaunchedEffect(mic) {
            if (mic == MicState.LISTENING && was != MicState.LISTENING) view.performHapticFeedback(HapticFeedbackConstants.GESTURE_START)
            if (was == MicState.LISTENING && mic != MicState.LISTENING) view.performHapticFeedback(HapticFeedbackConstants.GESTURE_END)
            was = mic
        }
        val micText = when (mic) {
            MicState.READYING -> stringResource(R.string.voice_getting_ready)
            MicState.LISTENING -> stringResource(R.string.voice_listening)
            MicState.FINISHING -> stringResource(R.string.voice_finishing)
            MicState.IDLE -> null
        }
        if (micText != null) {
            PhoneText(micText, 13.5f, FerbType.Medium, ink.low, Modifier.padding(horizontal = 6.dp).semantics { liveRegion = LiveRegionMode.Polite })
        }
        running?.let { WorkRowFor(vm, it) }
        val onMacText = stringResource(R.string.attach_on_mac)
        val focus = remember { androidx.compose.ui.focus.FocusRequester() }
        val focusAsk by vm.focusAsk.collectAsState()
        LaunchedEffect(focusAsk) {
            if (focusAsk) { androidx.compose.runtime.withFrameNanos { }; runCatching { focus.requestFocus() }; vm.focusTaken() }
        }
        PhoneComposer(
            Modifier, placeholder = if (hasFiles) stringResource(R.string.chat_placeholder_files) else placeholder,
            typed = draft, onValueChange = vm::typed, onSend = { vm.send() },
            attachments = if (!hasFiles) null else {
                {
                    AttachmentRow(Modifier.testTag("attachments")) {
                        for (c in chips) {
                            AttachmentChip(c.name, android.text.format.Formatter.formatShortFileSize(context, c.size), thumbnail = rememberThumbnail(c.path, c.name),
                                removeLabel = stringResource(R.string.attach_remove, c.name), onRemove = { vm.unpick(c.key) })
                        }
                        for (a in ready) {
                            AttachmentChip(a.name, onMacText, removeLabel = stringResource(R.string.attach_remove, a.name), onRemove = { vm.transfers.unattach(a.uploadId) })
                        }
                    }
                }
            },
            onAttach = { host.pickFiles(true) },
            // hold to speak (PHN-603): the words land in the box while held and stay there; never sent by themselves
            onMicDown = { if (vm.voicePress() == VoicePress.ASK_MICROPHONE) host.allowMicrophone(null) },
            onMicUp = { vm.voiceRelease() },
            listening = listening, listeningText = micText ?: stringResource(R.string.voice_listening), micLabel = stringResource(R.string.voice_speak),
            mic = mic, micLevel = level, focus = focus,
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
        composer = { m -> Composer(vm, draft, texts.placeholder, m, host, ui.running) },
        // a new day with nothing said yet: earlier days are one tap away (kb PHN-626)
        earlier = if (ui.rows.any { it !is ChatRow.Older }) stringResource(R.string.older_available) else null,
        onEarlier = { vm.openChat() },
    )
}

/**
 * The work row above the message box (kb PHN-725; FERB.app ChatBody.swift WorkRowView): "1 active task · 0:42", the
 * round live pictures of the first two tasks, "+N" and "View tasks". A picture opens the task's short view; "+N" and
 * "View tasks" the Tasks tab. Its pictures are asked for only while it is on screen (the view model adds the rest of
 * the rules: unlocked, in front, not on the shared relay).
 */
@Composable
private fun WorkRowFor(vm: FerbViewModel, r: com.ferb.app.model.RunningUi) {
    val pictures by vm.thumbnails.pictures.collectAsState()
    // a task whose card rows show their own pictures has no circle here (APP-891: a picture shows once)
    val taken by vm.tasksWithRowPictures.collectAsState()
    val shown = remember(r.tasks, taken) { r.tasks.filter { t -> t.taskId == null || t.taskId !in taken } }
    val ids = remember(shown) { shown.mapNotNull { it.taskId }.take(2) }
    DisposableEffect(ids) {
        vm.workRowShown(ids)
        onDispose { vm.workRowShown(emptyList()) }
    }
    ActiveWorkRow(
        r.words, r.running, r.since,
        shown.map { t -> WorkTask(t.entry, t.title, t.line, t.waiting, t.paused, t.taskId?.let { pictures[it] }) },
        onPicture = { w -> vm.showTask(r.tasks.firstOrNull { it.entry == w.key }?.taskId) },
        onMore = { vm.choose(PhoneTab.TASKS) },
        onViewTasks = { vm.choose(PhoneTab.TASKS) },
        modifier = Modifier.padding(horizontal = 6.dp).testTag("work-row"),
        viewTasks = stringResource(R.string.work_view_tasks),
    )
}

/** What happened while the owner was away: questions first, then failures, then what finished (HomePage). */
private fun awayRows(ui: UiState): List<WorkItem> =
    ui.work.waiting + ui.away.filter { it.kind == MarkKind.FAILED } + ui.away.filter { it.kind != MarkKind.FAILED }

private fun open(vm: FerbViewModel, item: WorkItem) {
    if (item.approvalId != null) vm.goToQuestion(item.approvalId) else vm.openChat()
}

@Composable
private fun Chat(vm: FerbViewModel, ui: UiState, draft: String, insets: PhoneInsets, waiting: PhoneTab?, onTab: (PhoneTab) -> Unit, onProfile: () -> Unit, host: Host) {
    val list = rememberLazyListState()
    // two places, as FERB.app separates days (kb PHN-696): today's chat (only today's rows, the way to earlier days at
    // its top) or the earlier days (one continuous chat with a day's start between days, PHN-627), never both
    val earlier by vm.earlier.collectAsState()
    val today = chatDayOf(ui)
    // messages he sent with files go at the end, after the outbox, until their files are on the Mac (kb PHN-691)
    val withFiles by vm.outgoing.messages.collectAsState()
    val places = remember(ui.rows, ui.days, today, withFiles) {
        ChatModel.split(ChatModel.withDays(ui.rows + withFiles.map { ChatRow.WithFiles("send:${it.id}", it) }, ui.days, today), today)
    }
    // earlier days exist: rows of them, or older pages the Mac still has (or has when the path allows)
    val hasEarlier = places.earlier.any { it !is ChatRow.Day }
    // a question the chat doesn't hold yet (the Mac's status brought it first) is a box at the end of today's chat, as
    // FERB.app draws its open questions (ConversationTranscript.swift:205-208; kb PHN-770)
    val loose = remember(ui.questions, ui.rows) {
        val placed = ui.rows.mapNotNullTo(HashSet()) { (it as? ChatRow.Asking)?.approvalId }
        ui.questions.filter { it.approvalId !in placed }.map { ChatRow.Asking("q:${it.approvalId}", it.approvalId, it.title, null, it.hidden) }
    }
    val rows = remember(places, earlier, hasEarlier, loose) {
        when {
            earlier -> places.earlier
            hasEarlier && places.today.isNotEmpty() -> listOf<ChatRow>(ChatRow.EarlierLink("earlier-link")) + places.today + loose
            else -> places.today + loose
        }
    }
    // its end followed while he is at it: a new row, a reply growing as it streams, the keyboard; his own message
    // always; a pill when he scrolled up (kb PHN-628). Before, only a change in the number of rows scrolled, to the
    // top of the last row: a reply that replaced "typing" or grew never moved the list.
    val last = rows.lastOrNull()
    val follow = rememberChatFollow(list, last?.key, last, mine = last is ChatRow.Outbox || last is ChatRow.WithFiles)
    // the rows new to the chat since it last drew (none on its first draw): only they fade in (kb PHN-624, PHN-670)
    val drawn = remember { arrayOfNulls<Set<String>>(1) }
    val keys = remember(rows) { rows.mapTo(HashSet()) { it.key } }
    // the same empty set while nothing arrives: the rows' content is the same and nothing on screen recomposes (kb PHN-714)
    val arriving = drawn[0]?.let { before -> (keys - before).ifEmpty { emptySet() } }.orEmpty()
    SideEffect { drawn[0] = keys }
    // when the chat first draws entries, and how many (`ferb-app`, kb PHN-629)
    LaunchedEffect(rows.isNotEmpty()) { com.ferb.app.AppLog.i(if (rows.isEmpty()) "chat opened, nothing to draw yet" else "chat drew ${rows.size} rows") }
    // reaching the top of the earlier days asks for the page before it (today's chat never loads older pages)
    val atTop by remember { derivedStateOf { list.firstVisibleItemIndex == 0 } }
    LaunchedEffect(atTop, ui.older, earlier) { if (earlier && atTop && ui.older is OlderState.Available) vm.loadOlder() }
    // the Chat tab tapped on today's chat: to its end (kb PHN-662; in the earlier days it goes back to today, PHN-698)
    LaunchedEffect(follow) { vm.chatToEnd.collect { follow.toEnd(animate = true) } }
    // a notification's tap or the Tasks tab's row: the question's box in view (kb PHN-770)
    val rowsNow by androidx.compose.runtime.rememberUpdatedState(rows)
    LaunchedEffect(list) {
        vm.toQuestion.collect { id ->
            androidx.compose.runtime.withFrameNanos { }
            val at = rowsNow.indexOfFirst { it is ChatRow.Asking && it.approvalId == id }
            if (at >= 0) list.animateScrollToItem(at) else follow.toEnd(animate = true)
        }
    }
    Column(Modifier.fillMaxSize()) {
        TopBar(PhoneTab.CHAT, waiting = waiting, onTab = onTab, onProfile = onProfile)
        val status = statusLine(ui)
        MotionShow(status, push = true) { (text, tone) ->
            StatusLine(stringResource(text), tone, Modifier.padding(top = 10.dp).testTag("status-line"))
        }
        ui.macNeed?.let { need -> MacNeedCard(need, ui.macLine != null) { vm.unlockMac() } }
        // the chat's height as answers fold to it (FERB.app's CollapsePolicy): the tallest it has been, so the keyboard
        // coming up never folds or unfolds what is on screen
        var tallest by remember { mutableIntStateOf(0) }
        val density = androidx.compose.ui.platform.LocalDensity.current
        Box(Modifier.weight(1f).fillMaxWidth().onSizeChanged { if (it.height > tallest) tallest = it.height }) {
          CompositionLocalProvider(LocalChatViewport provides with(density) { tallest.toDp() }, LocalCardsLive provides !earlier,
              LocalQuestions provides ui.questions, LocalBusy provides ui.busy) {
            // nothing stored yet and the first page on its way: the waiting look, never a blank screen (kb PHN-628)
            if (rows.isEmpty() && ui.conversation == null && ui.link?.phase == LinkPhase.ONLINE) ChatWaiting(stringResource(R.string.chat_loading))
            LazyColumn(
                Modifier.fillMaxSize().testTag("chat-list"),
                state = list,
                // with the way to today always there, the end keeps room for it (kb PHN-698)
                contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 16.dp, bottom = if (earlier) 64.dp else 12.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                // a row that arrives fades in where it lands, the others glide (kb PHN-624); a row that was there already
                // is drawn at once, whatever the list does (kb PHN-670)
                // each row gets only what it draws, never the whole state: a streamed word recomposes its own row (kb PHN-714)
                val olderLoading = "older" in ui.busy
                // a row of a kind reuses the composition of one of its kind that scrolled away (kb PHN-716)
                items(rows, key = { it.key }, contentType = { it::class }) { row -> Box(motionItem(arriving = row.key in arriving)) { ChatRowView(vm, today, olderLoading, row, host) } }
            }
            ChatOverlays(vm, list, rows, follow, earlier, today)
          }
        }
        Composer(vm, draft, stringResource(R.string.chat_placeholder), Modifier.paddingOverBottom(insets, 8.dp, start = 16.dp, end = 16.dp), host, ui.running)
    }
}

/**
 * Over the chat: the day he is in, held at the top while he looks through it (kb PHN-663), and the way back to the
 * latest messages whenever he is not at the end (kb PHN-662). A composable of its own: it reads where the list is, which
 * changes as rows pass its top on every fling; only this recomposes then, never the chat or the message box (kb PHN-716).
 */
@Composable
private fun androidx.compose.foundation.layout.BoxScope.ChatOverlays(
    vm: FerbViewModel, list: androidx.compose.foundation.lazy.LazyListState, rows: List<ChatRow>,
    follow: com.ferb.design.components.ChatFollow, earlier: Boolean, today: Long,
) {
    val scope = rememberCoroutineScope()
    // the day he is looking at and the latest messages' day: the sticky label and the way back (kb PHN-662, PHN-663)
    val first by remember { derivedStateOf { list.firstVisibleItemIndex } }
    val firstHidden by remember { derivedStateOf { list.firstVisibleItemScrollOffset > 0 } }
    val shownDay = ChatModel.dayAt(rows, first)
    val endDay = ChatModel.dayAt(rows, rows.lastIndex)
    val jump = chatJump(follow.following, follow.unseen, shownDay, endDay, earlier)
    val sticky = shownDay?.takeIf { earlier && !follow.following && (rows.getOrNull(first) !is ChatRow.Day || firstHidden) }
    MotionShow(sticky?.let { dayLabel(it, today) }, Modifier.align(Alignment.TopCenter).padding(top = 8.dp)) { StickyDay(it, Modifier.testTag("sticky-day")) }
    val toLatest = stringResource(R.string.chat_to_latest)
    val toToday = stringResource(R.string.chat_to_today)
    val jumpText = when (jump) {
        ChatJump.NEW_MESSAGE -> stringResource(R.string.chat_new_message)
        ChatJump.DAY -> endDay?.let { dayLabel(it, today) }
        ChatJump.TODAY -> stringResource(R.string.day_today)
        else -> null
    }
    MotionShow(jump.takeIf { it != ChatJump.NONE }?.let { it to (jumpText ?: "") }, Modifier.align(Alignment.BottomCenter).padding(bottom = 10.dp)) { (kind, text) ->
        JumpPill(text.ifEmpty { null }, if (kind == ChatJump.TODAY) toToday else toLatest, Modifier.testTag("chat-jump")) {
            if (kind == ChatJump.TODAY) vm.backToToday() else scope.launch { follow.toEnd(animate = true) }
        }
    }
}

/** The chat's "today" as the view model holds it (kb PHN-697); ChatModel.chatDay itself where none is held yet. */
private fun chatDayOf(ui: UiState): Long =
    ui.chatDay.takeIf { it > 0 } ?: ChatModel.chatDay(System.currentTimeMillis(), java.time.ZoneId.systemDefault())

/** A day's start as words: "Today", "Yesterday", else the weekday and date in the phone's language (kb PHN-627). */
@Composable
private fun dayLabel(day: Long, today: Long = ChatModel.chatDay(System.currentTimeMillis(), java.time.ZoneId.systemDefault())): String {
    return when (day) {
        today -> stringResource(R.string.day_today)
        today - 1 -> stringResource(R.string.day_yesterday)
        else -> {
            val locale = java.util.Locale.getDefault()
            val pattern = android.text.format.DateFormat.getBestDateTimePattern(locale, if (today - day < 300) "EEEEdMMMM" else "EEEEdMMMMy")
            java.time.LocalDate.ofEpochDay(day).format(java.time.format.DateTimeFormatter.ofPattern(pattern, locale))
        }
    }
}

/**
 * The line under the tabs: only what the top bar's connection control cannot say (kb PHN-756). The link's state
 * (connecting, connected, its path, can't reach) is the control's (PHN-751), and each message that waits says so in its
 * own row ("Waiting for your Mac"), so the line never speaks of the link. Left here: the Mac's unlock on its way or
 * slow once connected (PHN-641), and FERB may use the Mac's screen until he is back (PHN-611; "Ask me again" is on the
 * Mac tab).
 */
private fun statusLine(ui: UiState): Pair<Int, StatusTone>? = when {
    // the one thing about the link the control can't say: insomnia mode was on, so the Mac may be asleep (APP-981)
    com.ferb.app.model.MacAsleep.shown(ui.status, ui.link, ui.linkLost) -> R.string.mac_asleep to StatusTone.INFO
    // not connected: the control in the top bar says it, the waiting rows say they wait
    ui.link?.phase != LinkPhase.ONLINE -> null
    // PHN-641: the Mac's side after the app opened: words only, it clears by itself
    ui.macLine == com.ferb.app.MacLine.OPENING -> R.string.mac_opening to StatusTone.WORKING
    ui.macLine == com.ferb.app.MacLine.SLOW -> R.string.mac_slow to StatusTone.WORKING
    ui.status?.screenUse?.active == true -> R.string.screen_use_active to StatusTone.INFO
    else -> null
}

/**
 * The Mac's side needs the owner's fingerprint once (kb PHN-649): the lab's white card with what happened in plain
 * words and one button, never a line to find. Only for what truly needs it: 12 hours, FERB restarted on the Mac, a
 * prompt he closed, or a link that came back too late for the opening prompt's unlock.
 */
@Composable
private fun MacNeedCard(need: com.ferb.app.MacNeed, busy: Boolean, onUnlock: () -> Unit) {
    Card(Modifier.padding(start = 16.dp, end = 16.dp, top = 12.dp), padding = 16.dp) {
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            PhoneText(stringResource(R.string.mac_needs_title), 17f, FerbType.Semibold)
            PhoneText(stringResource(when (need) {
                com.ferb.app.MacNeed.LAPSED -> R.string.mac_needs_lapsed
                com.ferb.app.MacNeed.CANCELLED -> R.string.mac_needs_cancelled
                com.ferb.app.MacNeed.OLD -> R.string.mac_needs_old
            }), 15f, color = Ink.detail, lineSpacing = 2f)
            Pill(stringResource(R.string.mac_needs_unlock), icon = FerbIcons.fingerprint, onClick = if (busy) null else onUnlock)
        }
    }
}

@Composable
private fun ChatRowView(vm: FerbViewModel, today: Long, olderLoading: Boolean, row: ChatRow, host: Host) {
    // whatever FERB says or did sits on FERB's dark surface; the owner's own messages stay white (kb PHN-734)
    val fromFerb = row !is ChatRow.Owner && row !is ChatRow.Outbox && row !is ChatRow.WithFiles && row !is ChatRow.Day &&
        row !is ChatRow.EarlierLink && row !is ChatRow.Older
    androidx.compose.runtime.CompositionLocalProvider(com.ferb.design.components.LocalOnFerb provides fromFerb) {
        ChatRowContent(vm, today, olderLoading, row, host)
    }
}

/**
 * A card of parts (FERB.app's TurnCardView): each part's answer, its own live picture while its step works (kb PHN-726,
 * asked for while the card is on screen), Watch live and Stop. A picture that arrives changes only this card.
 */
@Composable
private fun TaskRow(vm: FerbViewModel, row: ChatRow.Task) {
    val keys = remember(row.taskId, row.watch) {
        row.watch.map { step -> if (step != null && row.taskId != null) com.ferb.app.model.Thumbnails.key(row.taskId, step) else null }
    }
    val wanted = remember(keys) { keys.filterNotNull() }
    DisposableEffect(row.key, wanted) {
        vm.rowPictures(row.key, wanted)
        onDispose { vm.rowPictures(row.key, emptyList()) }
    }
    // only this card's pictures are read: another task's picture arriving never recomposes it
    val all = vm.thumbnails.pictures.collectAsState()
    val pictures by remember(keys) { androidx.compose.runtime.derivedStateOf { keys.map { k -> k?.let { all.value[it] } } } }
    // FERB.app's card (kb PHN-737): no buttons in it; a part's picture opens its short view, a running part with no
    // picture its task's (Watch live and Stop are there), and View tasks is under the chat
    com.ferb.design.components.PartsCard(
        row.headline, stringResource(R.string.progress, row.done, row.total), row.parts,
        tint = row.tint, status = row.status, footer = row.footer, footerLines = row.footerLines,
        watching = keys.map { it != null }, pictures = pictures,
        onPicture = { i -> vm.showTask(row.taskId, row.watch.getOrNull(i)) },
        onPart = { vm.showTask(row.taskId) },
        more = stringResource(R.string.show_more), less = stringResource(R.string.show_less),
    )
}

@Composable
private fun ChatRowContent(vm: FerbViewModel, today: Long, olderLoading: Boolean, row: ChatRow, host: Host) {
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
            // a connection line ticks its time as FERB.app's LiveLineView ("… · 0:42")
            if (row.since != null) com.ferb.design.components.TickingText(row.text, row.since, 14.5f, ink.low)
            // FERB.app's status words and notices under a reply: 10 points under its 12 (the phone's 16 scaled the same),
            // grey, "Delivered, with parts not done" amber, on FERB's dark surface so either reads on any sky (kb PHN-773)
            else if (row.small) PhoneText(row.text, 13.5f, FerbType.Medium, if (row.amber) com.ferb.design.Ink.amber else com.ferb.design.Ink.onDark,
                Modifier.background(com.ferb.design.Ink.black, androidx.compose.foundation.shape.RoundedCornerShape(50)).padding(horizontal = 10.dp, vertical = 4.dp))
            else PhoneText(row.text, 14.5f, FerbType.Medium, ink.low)
            for (q in row.quotes) PhoneText(q, 14f, color = ink.low)
        }
        is ChatRow.Task -> TaskRow(vm, row)
        is ChatRow.Rich -> FerbRichBubble(copy = row.copy) {
            val tail = remember(row.tail) { row.tail.map { it to false } }
            com.ferb.design.components.RichBlocks(row.blocks, onBlack, row.key, row.fold, more = stringResource(R.string.show_all),
                less = stringResource(R.string.show_less), tail = tail)
        }
        is ChatRow.Answer -> AnswerRow(vm, row, host)
        is ChatRow.Notice -> NoticeRow(vm, row)
        is ChatRow.Results -> Column(Modifier.fillMaxWidth().padding(end = 28.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            for (c in row.cards) ResultCard(c)
        }
        is ChatRow.Cards -> Card {
            row.cards.forEachIndexed { i, c ->
                if (i > 0) Divide()
                if (c.taskId != null && c.index != null) TaskFileRow(vm, c, host)
                else CardRow(c.title, c.detail, leading = { if (c.file) FileIcon(c.title, 26.dp) else RowIcon(FerbIcons.doc) })
            }
        }
        // FERB has his message and is on it: never a silent screen (kb PHN-628)
        is ChatRow.Typing -> Row(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
            TypingDots()
            PhoneText(stringResource(R.string.chat_thinking), 14f, FerbType.Medium, ink.low)
        }
        is ChatRow.Day -> DayDivider(dayLabel(row.day, today))
        // FERB.app's inline question leads with its title (else "FERB needs your answer") and the question under it;
        // the phone opens the answer sheet, since some answers need the fingerprint (kb PHN-729)
        // FERB.app's question card in the chat, answered in place (kb PHN-770)
        is ChatRow.Asking -> QuestionBox(vm, row.approvalId, row.title, row.reason)
        is ChatRow.Outbox -> OutboxRow(vm, row)
        is ChatRow.WithFiles -> WithFilesRow(vm, row.item)
        is ChatRow.EarlierLink -> com.ferb.design.components.EarlierLink(stringResource(R.string.older_available), Modifier.testTag("earlier-link")) { vm.openChat() }
        is ChatRow.Older -> when (row.state) {
            is OlderState.NeedsBetterPath -> PhoneText(stringResource(R.string.older_needs_better_path), 14f, FerbType.Medium, ink.low,
                Modifier.fillMaxWidth().padding(horizontal = 6.dp))
            // earlier messages on their way: a small turning mark at the top (kb PHN-628)
            else -> if (olderLoading) Row(Modifier.fillMaxWidth().padding(horizontal = 6.dp, vertical = 8.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally), verticalAlignment = Alignment.CenterVertically) {
                RunningArc(14.dp, ink.low)
                PhoneText(stringResource(R.string.chat_older_loading), 14f, FerbType.Medium, ink.low)
            } else PhoneText(stringResource(R.string.older_available), 14f, FerbType.Medium, ink.low,
                Modifier.fillMaxWidth().pressable({ vm.loadOlder() }).padding(horizontal = 6.dp, vertical = 8.dp))
        }
    }
}

/**
 * A turn's answer as FERB.app draws it (kb PHN-723, PHN-724; ConversationTranscript.swift AnswerView): a reminder that
 * came due above, the bubble with FERB's words (Markdown, folded past the chat's height with "Show all") and code's
 * own lines under them, "Details" with the whole answer when FERB shortened it, the cards by kind, the sites row, the
 * page buttons and the files, each once.
 */
@Composable
private fun AnswerRow(vm: FerbViewModel, row: ChatRow.Answer, host: Host) {
    val a = row.ui
    val uri = androidx.compose.ui.platform.LocalUriHandler.current
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        for (c in a.above) ResultCard(c, Modifier.padding(end = 28.dp))
        if (a.bubble) FerbRichBubble(copy = a.copy) {
            val tail = remember(a.gaps, a.note, a.lines) {
                a.gaps.map { "${com.ferb.app.model.GapLines.LABEL} $it" to true } +
                    listOfNotNull(a.note?.let { "Couldn't confirm: $it" to true }) + a.lines.map { it to false }
            }
            com.ferb.design.components.RichBlocks(a.blocks, onBlack, row.key, Fold.ANSWER, more = stringResource(R.string.show_all),
                less = stringResource(R.string.show_less), tail = tail)
            if (a.details.isNotEmpty()) {
                var open by androidx.compose.runtime.saveable.rememberSaveable(row.key + ":details") { mutableStateOf(false) }
                PhoneText(stringResource(R.string.answer_details), 14.5f, FerbType.Semibold, onBlack.link,
                    Modifier.pressable({ open = !open }).padding(vertical = 4.dp))
                if (open) {
                    com.ferb.design.components.RichBlocks(a.details, onBlack, row.key + ":full")
                }
            }
        }
        if (a.below.isNotEmpty()) Column(Modifier.padding(end = 28.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) { for (c in a.below) ResultCard(c) }
        if (a.sites.isNotEmpty()) SitesChip(a.sites, onOpen = { h -> com.ferb.app.model.Present.siteLink(h)?.let { runCatching { uri.openUri(it) } } },
            onAll = { vm.show(Sheet.Sites(row.key)) })
        // FERB.app's page buttons (Memory, Settings → Phone) open pages the phone doesn't have: not drawn, never a
        // control that does nothing (kb PHN-656)
        if (a.files.isNotEmpty()) Card(Modifier.padding(end = 28.dp)) {
            a.files.forEachIndexed { i, c ->
                if (i > 0) Divide()
                TaskFileRow(vm, c, host)
            }
        }
    }
}

/**
 * FERB's own message (FERB.app's NoticeView): a reminder's or timer's arrival is its one box (its own words inside it),
 * else the words in FERB's bubble with the facts FERB remembered, the quotes with where they came from, and the page
 * buttons.
 */
@Composable
private fun NoticeRow(vm: FerbViewModel, row: ChatRow.Notice) {
    val ink = LocalSkyInk.current
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        for (c in row.ui.above) ResultCard(c, Modifier.padding(end = 28.dp))
        if (!row.ui.boxSays) FerbRichBubble(copy = row.text) {
            val text = remember(row.text) { richText(com.ferb.design.components.plainBlocks(row.text), onBlack) }
            RichWords(text, onBlack, row.key)
            for (c in row.ui.notes) ResultCard(c)
            for ((quote, origin) in row.quotes) {
                Column(Modifier.drawBehind { drawRect(Ink.onDark, size = androidx.compose.ui.geometry.Size(2.dp.toPx(), size.height)) }.padding(start = 10.dp),
                    verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    PhoneText(quote, 14.5f, color = Ink.white, lineSpacing = 2f)
                    if (origin.isNotEmpty()) PhoneText(origin, 13f, color = Ink.onDark)
                }
            }
        }
        if (row.ui.boxSays && row.quotes.isNotEmpty()) for ((q, o) in row.quotes) PhoneText("“$q” ($o)", 14f, color = ink.low)
    }
}

/**
 * A file a finished task made, on its result card (kb PHN-535): a tap fetches it from the Mac (by the task and its
 * place, never by a path read in the chat), the row says how far it got, and once here a tap opens it in the app the
 * owner picks. It is a fetched file like any other: gone when FERB locks.
 */
@Composable
private fun TaskFileRow(vm: FerbViewModel, c: CardLine, host: Host) {
    if (c.taskId == null || c.index == null) {
        CardRow(c.title, c.detail, leading = { FileIcon(c.title, 26.dp) })
        return
    }
    val moving by vm.transfers.list.collectAsState()
    val t = moving.firstOrNull { it.task?.taskId == c.taskId && it.task?.index == c.index!!.toLong() }
    val detail = when {
        t == null -> c.detail
        t.phase == Phase.DONE -> stringResource(R.string.tr_card_ready)
        t.problem != null -> noteText(t.problem)
        t.phase == Phase.RUNNING -> stringResource(R.string.tr_card_getting, (t.fraction * 100).toInt())
        else -> stringResource(R.string.tr_card_paused)
    }
    CardRow(c.title, detail, leading = { FileIcon(c.title, 26.dp) }, trailing = { Opens() }, onClick = {
        when {
            t == null -> vm.fetchTaskFile(c.taskId!!, c.index!!, c.title)
            t.phase == Phase.DONE -> host.handOut(t.key, java.io.File(t.local), t.name, share = false)
            t.phase != Phase.RUNNING -> vm.transfers.resume(t.key)
        }
    })
}

/** A message in the outbox (PHN-401): the owner's bubble, faded until it is sent, with what it waits for and Cancel. */
@Composable
private fun OutboxRow(vm: FerbViewModel, row: ChatRow.Outbox) {
    val o = row.item
    val ink = LocalSkyInk.current
    val failed = o.state == "failed"
    val ui by vm.ui.collectAsState()
    val status = when {
        failed -> stringResource(R.string.outbox_failed, o.reason.orEmpty())
        o.state == "sending" -> stringResource(R.string.outbox_sending)
        // the Mac asleep while insomnia mode was on says so (APP-981); else what it waits for
        else -> stringResource(com.ferb.app.model.MacAsleep.waitingLine(o.waitingFor, com.ferb.app.model.MacAsleep.shown(ui.status, ui.link, ui.linkLost)))
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

/**
 * A message he sent with files (kb PHN-691): its files and words, as his message, and how it stands, with what he can
 * do: stop it while its files go; try again, send the words alone (when files can't go on this path) or remove it.
 */
@Composable
private fun WithFilesRow(vm: FerbViewModel, m: Sending) {
    val ink = LocalSkyInk.current
    val context = LocalContext.current
    val bad = m.phase == SendPhase.FAILED || m.phase == SendPhase.CLOSED
    val ui by vm.ui.collectAsState()
    val status = when (m.phase) {
        SendPhase.UPLOADING -> stringResource(R.string.send_progress, (m.fraction * 100).toInt())
        SendPhase.SENDING -> stringResource(R.string.outbox_sending)
        SendPhase.WAITING_LINK -> stringResource(com.ferb.app.model.MacAsleep.waitingLine(null, com.ferb.app.model.MacAsleep.shown(ui.status, ui.link, ui.linkLost)))
        SendPhase.WAITING_PATH -> m.note?.let { noteText(it) } ?: stringResource(R.string.send_waiting_path)
        SendPhase.STOPPED -> stringResource(R.string.send_stopped)
        SendPhase.LOCKED -> stringResource(R.string.send_locked)
        SendPhase.CLOSED -> stringResource(R.string.send_closed)
        SendPhase.FAILED -> stringResource(R.string.outbox_failed, m.note?.let { noteText(it) }.orEmpty())
    }
    val actions = buildList {
        when (m.phase) {
            SendPhase.UPLOADING -> add(R.string.outbox_cancel to { vm.stopSending(m.id) })
            SendPhase.SENDING -> Unit
            SendPhase.WAITING_PATH -> {
                if (m.text.isNotBlank()) add(R.string.send_text_only to { vm.sendTextOnly(m.id) })
                add(R.string.outbox_remove to { vm.removeSending(m.id) })
            }
            SendPhase.STOPPED, SendPhase.CLOSED -> {
                add(R.string.send_try_again to { vm.retrySending(m.id) })
                add(R.string.outbox_remove to { vm.removeSending(m.id) })
            }
            SendPhase.FAILED -> {
                if (m.note?.res != R.string.send_files_gone) add(R.string.send_try_again to { vm.retrySending(m.id) })
                add(R.string.outbox_remove to { vm.removeSending(m.id) })
            }
            SendPhase.WAITING_LINK, SendPhase.LOCKED -> add(R.string.outbox_remove to { vm.removeSending(m.id) })
        }
    }
    Column(Modifier.fillMaxWidth().testTag("sending"), horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Column(Modifier.alpha(if (bad) 1f else 0.85f), horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(6.dp)) {
            for (c in m.chips.take(4)) {
                AttachmentChip(c.name, android.text.format.Formatter.formatShortFileSize(context, c.size), thumbnail = rememberThumbnail(c.path, c.name))
            }
            if (m.chips.size > 4) PhoneText("+${m.chips.size - 4}", 13.5f, FerbType.Medium, ink.low, Modifier.padding(horizontal = 6.dp))
            if (m.text.isNotBlank()) OwnerBubble(m.text)
        }
        Row(Modifier.padding(horizontal = 6.dp).semantics(mergeDescendants = true) { liveRegion = LiveRegionMode.Polite },
            horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
            PhoneText(status, 13.5f, FerbType.Medium, if (bad) Ink.red else ink.low, Modifier.weight(1f, fill = false))
            for ((label, act) in actions) {
                Box(Modifier.pressable(act).heightIn(min = Phone.touchTarget), contentAlignment = Alignment.Center) {
                    PhoneText(stringResource(label), 13.5f, FerbType.Semibold, ink.top)
                }
            }
        }
    }
}

/** A small picture of a photo in the phone's own copy, made off the main thread; null for other files. */
@Composable
private fun rememberThumbnail(path: String, name: String): androidx.compose.ui.graphics.ImageBitmap? {
    val image = remember(name) { name.substringAfterLast('.', "").lowercase() in setOf("jpg", "jpeg", "png", "webp", "heic", "heif", "gif", "bmp") }
    val bitmap by androidx.compose.runtime.produceState<androidx.compose.ui.graphics.ImageBitmap?>(null, path, image) {
        if (!image) return@produceState
        value = kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) {
            runCatching {
                val bounds = android.graphics.BitmapFactory.Options().apply { inJustDecodeBounds = true }
                android.graphics.BitmapFactory.decodeFile(path, bounds)
                var sample = 1
                while (bounds.outWidth / (sample * 2) >= 96 && bounds.outHeight / (sample * 2) >= 96) sample *= 2
                android.graphics.BitmapFactory.decodeFile(path, android.graphics.BitmapFactory.Options().apply { inSampleSize = sample })?.asImageBitmap()
            }.getOrNull()
        }
    }
    return bitmap
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
        notice = macNotices(vm, ui),
    )
}

/** The cards above the Mac tab's rows: "until I'm back" (PHN-611) and "Keep my Mac working" (APP-980). */
private fun macNotices(vm: FerbViewModel, ui: UiState): (@Composable () -> Unit)? {
    val screenUse = ui.status?.screenUse?.active == true
    val insomnia = ui.status?.insomnia
    if (!screenUse && insomnia == null) return null
    return {
        Column {
            if (screenUse) ScreenUseCard(vm, ui)
            if (insomnia != null) InsomniaCard(vm, ui, insomnia)
        }
    }
}

/** FERB may use the Mac's screen until the owner is back (PHN-611): the line and "Ask me again". */
@Composable
private fun ScreenUseCard(vm: FerbViewModel, ui: UiState) {
    Card(Modifier.padding(start = 16.dp, end = 16.dp, bottom = 12.dp), padding = 16.dp) {
        PhoneText(stringResource(R.string.screen_use_active), 15.5f, FerbType.Medium, Ink.black,
            Modifier.fillMaxWidth().semantics { liveRegion = LiveRegionMode.Polite })
        Pill(stringResource(R.string.screen_use_end), Modifier.padding(top = 12.dp), PillKind.GREY, height = 44.dp,
            onClick = if ("screen-use" in ui.busy) null else ({ vm.endScreenUse() }))
    }
}

/** A relay's address as the row shows it: its host. */
fun host(url: String): String = url.substringAfter("://").substringBefore('/').substringBefore('?')
