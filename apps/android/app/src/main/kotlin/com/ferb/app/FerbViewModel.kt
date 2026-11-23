package com.ferb.app

import androidx.annotation.StringRes
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.ferb.app.model.AnswerFlow
import com.ferb.app.model.Answered
import com.ferb.app.model.ChatModel
import com.ferb.app.model.ChatRow
import com.ferb.app.model.Coarse
import com.ferb.app.model.Corpus
import com.ferb.app.model.Dictation
import com.ferb.app.model.SpeechKind
import com.ferb.app.model.VoiceInput
import com.ferb.app.model.HomeWords
import com.ferb.app.model.LiveWatch
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.UnlockStatement
import com.ferb.app.model.MacSettings
import com.ferb.app.model.Moment
import com.ferb.app.model.FolderPickUi
import com.ferb.app.model.NewSessionUi
import com.ferb.app.model.SessionConfirm
import com.ferb.app.model.SessionSending
import com.ferb.app.model.SessionSendPhase
import com.ferb.app.model.TerminalPick
import com.ferb.app.model.SessionUi
import com.ferb.app.model.Sessions
import com.ferb.app.model.SessionsUi
import com.ferb.app.model.Phase
import com.ferb.app.model.PhoneApi
import com.ferb.app.model.Question
import com.ferb.app.model.Questions
import com.ferb.app.model.SettingRow
import com.ferb.app.model.Transfers
import com.ferb.app.model.Outgoing
import com.ferb.app.model.Unlocker
import com.ferb.app.model.Why
import com.ferb.app.model.Work
import com.ferb.app.model.WorkItem
import com.ferb.app.model.WorkModel
import com.ferb.app.model.keys
import com.ferb.core.KeyHealth
import com.ferb.core.KeyKind
import com.ferb.core.KeyState
import com.ferb.core.Outcome
import com.ferb.core.PhoneState
import com.ferb.core.detail
import com.ferb.core.linkCode
import com.ferb.design.components.MarkKind
import com.ferb.design.components.PhoneTab
import com.ferb.design.sky.SkyMoment
import com.ferb.design.sky.SkyPlace
import com.ferb.mobile.AnswerInput
import com.ferb.mobile.Change
import com.ferb.mobile.FerbException
import com.ferb.mobile.FileEntry
import com.ferb.mobile.Folder
import com.ferb.mobile.FoundFile
import com.ferb.mobile.FoundFiles
import com.ferb.mobile.LinkPath
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.LinkStatus
import com.ferb.mobile.MacInfo
import com.ferb.mobile.OlderState
import com.ferb.mobile.PhoneStatus
import com.ferb.mobile.RecentFiles
import com.ferb.mobile.SecretInput
import com.ferb.mobile.Signer
import com.ferb.mobile.TaskRow
import com.ferb.mobile.TranscriptView
import java.time.ZoneId
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.selects.select
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

/** What fills the screen: one at a time, nothing private before [MAIN] (PHN-8). */
enum class Gate { STARTING, FAILED, NO_SCREEN_LOCK, LOCKED, PAIR, CODES, REMOVED, MAIN }

/** Why the pairing screen shows again (or null on a first run). */
enum class PairReason { WIPED, REMOVED, KEYS, UNKNOWN }

/** Why the Mac's side needs the owner's fingerprint again while the app is open (kb PHN-649's card). */
enum class MacNeed { LAPSED, CANCELLED, OLD }

/** What slides up over the page. Only ids are kept (saved state holds no question words and never a private value). */
sealed interface Sheet {
    data object Relay : Sheet
    data object Profile : Sheet
    data object Forget : Sheet
    data object TypeCode : Sheet
    data object Scan : Sheet
    data object OwnRelay : Sheet
    data object Notifications : Sheet
    data object Help : Sheet
    data object StopAll : Sheet
    data object Settings : Sheet
    data class Days(val key: String) : Sheet
    data class Live(val taskId: String) : Sheet

    /** A running task's short view (kb PHN-725; FERB.app's TaskShortView): its picture, title and line, Watch live, Stop. */
    data class Task(val taskId: String, val worker: String? = null) : Sheet

    /** The sites a task contacted (FERB.app's "Sites visited" list), by the chat row that has them. */
    data class Sites(val row: String) : Sheet

    /** The words before the owner's first use of speaking to FERB, for this phone's kind of recognition (PHN-604). */
    data class Voice(val kind: SpeechKind) : Sheet

    /** The Mac asks for this phone's exact location once, for a task (GATE-613); the owner says yes or no here. */
    data class Location(val requestId: String) : Sheet

    /** The words before Android's own question, when the owner turns on sharing the city (SENSE-420). */
    data object City : Sheet

    /** What keeps FERB's questions from reaching this phone, with the tap that fixes it (kb PHN-765). */
    data class CantNotify(val blocker: NotifyBlocker) : Sheet

    /** The connection control's sheet (kb PHN-752): which Mac, how the link stands, its path, Try again. */
    data object Connection : Sheet

    /** Insomnia mode's warning before the phone turns it on (APP-980): the two ways, the one Yes, Keep it working. */
    data object Insomnia : Sheet

    /** Show me (APP-983): the Mac's camera while this shows; screenshots as the owner's switch says (PHN-810). */
    data object Camera : Sheet

    /** One of the owner's coding sessions (kb HAND-651..666), full screen: its conversation; screenshots as his switch says (PHN-800). */
    data class Session(val id: String) : Sheet

    /** A coding session's window live (Watch, kb HAND-651..666; PHN-454's rules); screenshots as the owner's switch says (PHN-810). */
    data class SessionLive(val id: String) : Sheet

    /** Start a new coding session on the Mac: the agent, the folder, the first words, then the exact command. */
    data object NewSession : Sheet
}

/** What pressing the microphone needs from the screen: nothing more, or Android's microphone question. */
enum class VoicePress { DONE, ASK_MICROPHONE }

/** A short line for the owner, in the app's words (string resources) or the Mac's own; [inner] a line said inside it. */
data class Note(@StringRes val res: Int, val words: String? = null, val inner: Note? = null)

/** This phone's own switches, as the profile sheet shows them. */
data class PhoneSettings(val stayConnected: Boolean, val blockScreenshots: Boolean, val hideChecks: Boolean, val pairedSince: Long, val shareCity: Boolean = false)

/** The prompts only the screen can show: the system's fingerprint prompt, with a key or without one. */
/** The calm line about the Mac's side once the app is open (PHN-641); never a control. */
enum class MacLine { OPENING, SLOW }

interface Prompts {
    fun signer(kind: KeyKind, why: Why = Why.OPEN): Signer

    /** The prompt alone (the app opens while the Mac can't be reached): true when the owner passed it. */
    suspend fun local(): Boolean
}

/** The Files tab: the places' counts, the folder open, the Mac's recent files and a search. */
data class FilesUi(
    val folder: Folder? = null,
    val loading: String? = null,
    val problem: Note? = null,
    val counts: Map<String, Long> = emptyMap(),
    /** The Mac's recent files for this phone (`recentFiles`), or null before (or when they couldn't be read: [recentNote]). */
    val recent: RecentFiles? = null,
    val recentNote: Note? = null,
    val search: SearchUi? = null,
)

/** A search of names on the Mac: what was asked, where, and what came back (or why nothing did). */
data class SearchUi(
    val query: String,
    val path: String?,
    val loading: Boolean = true,
    val found: FoundFiles? = null,
    val problem: Note? = null,
)

/**
 * The temporary chat while it is on (the owner, 2026-11-23; memory only, never the phone's store): its turns as the
 * chat's rows, its open questions and what of it runs (the status's rows marked `temporary`). Null: the normal chat.
 */
data class TemporaryUi(
    val id: String?,
    val rows: List<ChatRow>,
    val questions: List<Question> = emptyList(),
    val running: com.ferb.app.model.RunningUi? = null,
)

data class UiState(
    val gate: Gate = Gate.STARTING,
    val failure: String? = null,
    val pairReason: PairReason? = null,
    val pairing: Boolean = false,
    val pairError: Note? = null,
    val link: LinkStatus? = null,
    val keys: KeyHealth? = null,
    val conversation: String? = null,
    val rows: List<ChatRow> = emptyList(),
    /** The work row above the message box (kb PHN-725): what runs, from the chat's live state; null when nothing does. */
    val running: com.ferb.app.model.RunningUi? = null,
    /** Each entry's calendar day (epoch days, the phone's zone): the chat's day dividers (kb PHN-627). */
    val days: Map<String, Long> = emptyMap(),
    /**
     * The chat's "today" (epoch days; ChatModel.chatDay, kb PHN-697), held: it moves only when the app opens or unlocks,
     * comes back to the front, or the Chat tab is chosen, so a day changing never moves the page under him. 0: not set
     * (screens then ask ChatModel.chatDay themselves).
     */
    val chatDay: Long = 0,
    /**
     * The Mac has really not been reachable (kb PHN-753): stopped for good, or not online for [FerbViewModel.LINK_LOST_AFTER_MS]
     * while the app is open and unlocked. Until then a link that isn't online is connecting, never an error.
     */
    val linkLost: Boolean = false,
    /** What the chat holds (the Tasks tab falls back to it when the Mac's own list can't be read). */
    val chatWork: Work = Work(emptyList(), emptyList(), emptyList()),
    val work: Work = Work(emptyList(), emptyList(), emptyList()),
    /** What finished or failed since the app was last open (the home screen's list and the corpus's counts). */
    val away: List<WorkItem> = emptyList(),
    val questions: List<Question> = emptyList(),
    val older: OlderState = OlderState.Unknown,
    val needsMacUnlock: Boolean = false,
    /** Why the Mac's side needs the fingerprint again: the card with its one button (PHN-649); null: nothing to do. */
    val macNeed: MacNeed? = null,
    /** The Mac doesn't know this phone's keys any more (`unlock_invalid`): it needs pairing again (PHN-649). */
    val macForgotPhone: Boolean = false,
    /** The Mac's side while the app is already open (PHN-641): its unlock under way, or slow to answer. */
    val macLine: MacLine? = null,
    val busy: Set<String> = emptySet(),
    val note: Note? = null,
    /** What FERB has waiting, as the Mac last said it (PHN-125): its questions' needs, Stop everything. */
    val status: PhoneStatus? = null,
    /** The Mac's own name and this phone's pairing date (kept by the library). */
    val mac: MacInfo? = null,
    /** The Mac's task list (`tasks.list`), or null when it couldn't be read (the chat's list is used then). */
    val macTasks: List<TaskRow>? = null,
    /**
     * FERB's newest alert above the message box (kb APP-1105, PHN-791; FERB.app's alert line): the Mac getting hot,
     * insomnia's end, a database set aside… never in the chat; null when none waits. [alertCount]: how many wait.
     */
    val alert: com.ferb.mobile.StatusAlert? = null,
    val alertCount: Int = 0,
    /** Every alert waiting, oldest first (the toast's stack, kb APP-1109); [alertsOpen]: the stack is open. */
    val alerts: List<com.ferb.mobile.StatusAlert> = emptyList(),
    val alertsOpen: Boolean = false,
    val files: FilesUi = FilesUi(),
    /** FERB's settings as read from the Mac, or null before (or when they couldn't be read). */
    val settings: List<SettingRow>? = null,
    /** A fingerprint enrolled after pairing that the Mac doesn't know yet (PHN-133). */
    val newKeys: Boolean = false,
    /** The temporary chat while it is on: the chat tab shows only it (null: the normal chat). */
    val temporary: TemporaryUi? = null,
)

/**
 * The app's state over [PhoneApi]'s flows: the gate, the chat, tasks and questions, files, Watch live and FERB's
 * settings, and every action, each run off the main thread by the library. A screen turned or rebuilt keeps it; a new
 * process reads it from the phone's store.
 */
class FerbViewModel(
    private val api: PhoneApi,
    private val session: Session,
    private val prefs: Prefs,
    private val screenLock: () -> Boolean,
    private val corpus: Corpus?,
    private val saved: SavedStateHandle,
    private val local: LocalFiles,
    private val now: () -> Long = System::currentTimeMillis,
    /** One exact fix for the Mac's request (GATE-613); null where the phone can't read one (tests without it). */
    private val locator: Locator? = null,
    /** The last coarse fix while the owner shares the city (SENSE-420): the sky's place; else the time zone's. */
    private val coarse: StateFlow<Coarse?> = MutableStateFlow(null),
    /** The app's words for what goes to the Mac (a string resource as text). */
    private val say: (Int) -> String = { "" },
    /** The location request was answered here: its notification goes. */
    private val locationAnswered: () -> Unit = {},
    /** The phone's speech recogniser (PHN-603); null where there is none (most tests). */
    private val voice: VoiceInput? = null,
    /** Android allowed FERB the microphone. */
    private val micAllowed: () -> Boolean = { false },
    /** How a short pause is waited (tests pass one that returns at once). */
    private val wait: suspend (Long) -> Unit = { delay(it) },
    /** Where the opening prompt's signature waits (its prompt blocks): never the main thread (tests: their own). */
    private val io: kotlinx.coroutines.CoroutineDispatcher = Dispatchers.IO,
    /** The phone's steady clock in ms (the unlock statement's age, PHN-649). */
    private val steady: () -> Long = { android.os.SystemClock.elapsedRealtime() },
) : ViewModel() {
    /** FERB's alerts put away on this phone this session (kb PHN-791): hidden at once, before the Mac's next status drops
     *  them. Declared first: the constructor's first read applies a status before the properties below are made. */
    private val dismissedAlerts: MutableSet<String> = java.util.Collections.synchronizedSet(HashSet<String>())

    /** The toast's wait before it leaves by itself (kb APP-1109): [AlertToast.SECONDS]; a test gives its own. */
    internal var alertDelay: suspend (Long) -> Unit = { kotlinx.coroutines.delay(it) }

    /** His finger is on the toast: it doesn't leave while held (kb APP-1109). */
    private val alertHeld = MutableStateFlow(false)

    private val _ui = MutableStateFlow(UiState())
    val ui: StateFlow<UiState> = _ui.asStateFlow()

    /** The chosen tab and whether the chat tab shows the home screen; saved across process death. */
    val tab: StateFlow<PhoneTab> = saved.getStateFlow("tab", PhoneTab.CHAT)
    /**
     * The chat tab shows today's home page (kb PHN-696): today, with nothing said on it yet (his send, FERB's post, a
     * message with files on its way). Typing, speaking or picking a file never changes it; something said today does.
     */
    private val _home = MutableStateFlow(true)
    val home: StateFlow<Boolean> = _home.asStateFlow()

    /** The chat tab shows the earlier days (kb PHN-696): "Earlier messages" opened them; Today and a send come back. */
    val earlier: StateFlow<Boolean> = saved.getStateFlow("earlier", false)
    val draft: StateFlow<String> = saved.getStateFlow("draft", "")
    private val _settings = MutableStateFlow(readSettings())
    val settings: StateFlow<PhoneSettings> = _settings.asStateFlow()
    private val _sheet = MutableStateFlow<Sheet?>(null)
    val sheet: StateFlow<Sheet?> = _sheet.asStateFlow()

    /** The question boxes with a hidden input's field on screen now: the screen stays out of screenshots (PHN-110). */
    private val hiddenBoxes = MutableStateFlow<Set<String>>(emptySet())
    fun hiddenShown(approvalId: String, on: Boolean) = hiddenBoxes.update { if (on) it + approvalId else it - approvalId }

    private val _toQuestion = kotlinx.coroutines.flow.MutableSharedFlow<String>(extraBufferCapacity = 1)
    /** A question's box to bring into view in today's chat (a notification's tap, the Tasks tab's row; kb PHN-770). */
    val toQuestion: kotlinx.coroutines.flow.SharedFlow<String> = _toQuestion

    /** Today's chat with this question's box in view: where it is answered (kb PHN-770). */
    fun goToQuestion(approvalId: String) {
        saved["tab"] = PhoneTab.CHAT
        saved["earlier"] = false
        _sheet.value = null
        _toQuestion.tryEmit(approvalId)
    }
    var prompts: Prompts? = null
    private val windowReady = MutableStateFlow(false)
    private var refreshing: Job? = null
    private var dirty = false

    // (declared before `init`, which already reads the chat: a property further down is still null then)
    private val applying = kotlinx.coroutines.sync.Mutex()
    private var statusJob: Job? = null
    private var statusAgain = false
    private var markJob: Job? = null
    private var markAgain = false

    private val unlocker = Unlocker(api, { api.state.value.keys }) { kind, why -> prompts?.signer(kind, why) }
    val transfers = Transfers(viewModelScope, api, local, unlocker)

    /** The message box's files and the messages that carry files (kb PHN-690, PHN-691); kept on the phone, off main. */
    val outgoing = Outgoing(viewModelScope, api, transfers, local, io) { item ->
        if (item.waitingFor == "unlock") macLocked()
        refresh()
    }
    val live = LiveWatch(viewModelScope, api)

    // the owner's coding sessions (kb HAND-651..666): read only while their screens show, memory only (declared before `init`,
    // whose lock collector clears them)
    private val _sessions = MutableStateFlow(SessionsUi())
    /** The Sessions tab's list, read every [Sessions.LIST_MS] while it shows. */
    val sessions: StateFlow<SessionsUi> = _sessions.asStateFlow()
    private val _openSession = MutableStateFlow<SessionUi?>(null)
    /** The open session's screen, its conversation read after its newest item every [Sessions.OPEN_MS] while it shows. */
    val openSession: StateFlow<SessionUi?> = _openSession.asStateFlow()
    private val _newSession = MutableStateFlow<NewSessionUi?>(null)
    val newSession: StateFlow<NewSessionUi?> = _newSession.asStateFlow()

    /** The pause between two reads of the sessions (tests give their own, to step the polling). */
    internal var sessionsWait: suspend (Long) -> Unit = { delay(it) }

    // FERB's review of what an import brought in (kb MEM-460..457): read when its question's card shows, memory only
    // (declared before `init`, whose lock collector clears it)
    private val _review = MutableStateFlow<com.ferb.mobile.MemoryReview?>(null)
    /** The review as the Mac last said it (null before the first read): its question's card draws it. */
    val review: StateFlow<com.ferb.mobile.MemoryReview?> = _review.asStateFlow()

    /** Show me (APP-983): the Mac's camera, pulled only while its view shows. */
    val camera = com.ferb.app.model.CameraWatch(viewModelScope, api)

    /** The work row's round live pictures (kb PHN-725), only while it shows, unlocked, in front and not on the shared relay. */
    val thumbnails = com.ferb.app.model.Thumbnails(viewModelScope, api)
    private val workRowShows = MutableStateFlow<List<String>>(emptyList())

    /** The card rows on screen with their own pictures, by row (kb PHN-726; FERB.app's RowPreview): their keys. */
    private val rowPictureKeys = MutableStateFlow<Map<String, List<String>>>(emptyMap())

    /** The tasks whose card rows show their pictures now: the work row leaves their circles out (APP-891). */
    val tasksWithRowPictures: StateFlow<Set<String>> = rowPictureKeys.map { m -> m.values.flatten().mapTo(HashSet()) { it.substringBefore('\u001F') } }
        .stateIn(viewModelScope, kotlinx.coroutines.flow.SharingStarted.Eagerly, emptySet())

    /** Key places' files the owner opened in another app: they go when he is back in FERB (PHN-11). */
    private val openedKeyFiles = HashSet<String>()

    private val _mustSecure = MutableStateFlow(false)

    /**
     * A screen that must stay out of screenshots whatever the owner's switch says: only a private answer being typed
     * (PHN-110). Every other page follows the switch (kb PHN-810, PHN-811). MainActivity adds FLAG_SECURE while it is true.
     */
    val mustSecure: StateFlow<Boolean> = _mustSecure.asStateFlow()

    init {
        _ui.update { it.copy(chatDay = today()) }
        // FERB's alerts leave by themselves (kb APP-1109, FERB.app's AlertToastView): the toast shown for six seconds while
        // the app is in front on the chat, unlocked, the stack closed and his finger off it, is seen: put away everywhere
        viewModelScope.launch {
            combine(_ui.map { it.alert?.id to it.alertsOpen }.distinctUntilChanged(), tab, session.front, session.locked, alertHeld) { (id, open), t, front, locked, held ->
                id.takeIf { it != null && !open && t == PhoneTab.CHAT && front && !locked && !held }
            }.distinctUntilChanged().collectLatest { id ->
                if (id == null) return@collectLatest
                alertDelay(com.ferb.app.model.AlertToast.SECONDS * 1000L)
                dismissAlert(id)
            }
        }
        viewModelScope.launch {
            // today's home page while nothing was said on the chat's day and the earlier days aren't open (kb PHN-696)
            combine(earlier, _ui, outgoing.messages) { e, u, m -> !e && m.isEmpty() && !ChatModel.saidToday(lastView, u.chatDay, zone) }
                .distinctUntilChanged().collect { _home.value = it }
        }
        viewModelScope.launch {
            // every page follows the owner's switch, Watch live, a short view, the Mac's camera and a session's Watch
            // included (owner 2026-11-23, kb PHN-810, PHN-811); only a hidden answer's box forces the block (PHN-110)
            hiddenBoxes.map { it.isNotEmpty() }.collect { _mustSecure.value = it }
        }
        viewModelScope.launch {
            // the coding sessions are read only while their screens show, the app unlocked and in front (kb HAND-651..666): the
            // list every 3 s on the Sessions tab, an open session's newest items every 2 s; nothing in the background
            combine(tab, _sheet, session.front, session.locked) { t, s, front, locked ->
                when {
                    locked || !front -> null
                    s is Sheet.Session -> s
                    s is Sheet.SessionLive -> null
                    t == PhoneTab.SESSIONS -> PhoneTab.SESSIONS
                    else -> null
                }
            }.distinctUntilChanged().collectLatest { what ->
                when (what) {
                    is Sheet.Session -> while (true) { readSession(what.id); sessionsWait(Sessions.OPEN_MS) }
                    PhoneTab.SESSIONS -> while (true) { readSessions(); sessionsWait(Sessions.LIST_MS) }
                    else -> Unit
                }
            }
        }
        viewModelScope.launch {
            // the work row's pictures: while it shows, the app unlocked and in front, and the path carries pictures
            // the rows' own pictures first, then the work row's for the tasks no row shows (APP-891); at most 4
            val wanted = combine(workRowShows, rowPictureKeys) { ids, rows ->
                val r = rows.values.flatten()
                val taken = r.mapTo(HashSet()) { it.substringBefore('\u001F') }
                r + ids.filter { it !in taken }
            }
            combine(wanted, session.locked, session.front, api.link) { keys, locked, front, l ->
                val bulk = l?.phase == LinkPhase.ONLINE && l.bulkAllowed && l.path != LinkPath.PUBLIC_RELAY
                if (!locked && front && bulk) keys else emptyList()
            }.distinctUntilChanged().collect { thumbnails.show(it) }
        }
        viewModelScope.launch {
            combine(api.state, api.link, session.locked) { s, l, locked -> Triple(s, l, locked) }.collect { (s, l, locked) ->
                _ui.update { gateOf(it.copy(link = l, keys = s.keys), s, l, locked) }
                showPending()
                if (s is PhoneState.Open && l?.paired == true && prefs.pairedSince == 0L) {
                    prefs.pairedSince = now()
                    prefs.justPaired = true
                }
            }
        }
        viewModelScope.launch {
            api.changes.collect { c ->
                when (c) {
                    is Change.Closed -> Unit
                    is Change.Status -> statusChanged()
                    is Change.Mac -> readMac()
                    is Change.LocationWanted -> { pendingLocation = c.requestId; showPending() }
                    is Change.Temporary -> readTemporary()
                    else -> refresh()
                }
            }
        }
        viewModelScope.launch {
            // PHN-648: while the app is unlocked and in front the device key's presence keeps the Mac's window open
            combine(session.locked, session.front) { locked, front -> !locked && front }.distinctUntilChanged()
                .collect { api.setOpen(it) }
        }
        viewModelScope.launch {
            // PHN-615: the lock page that came with opening or returning asks by itself, once, when the window is the
            // owner's (resumed, focused: no system sheet or other prompt over it)
            combine(windowReady, session.promptDue, _ui.map { it.gate }.distinctUntilChanged()) { ready, due, gate ->
                ready && due && gate == Gate.LOCKED
            }.distinctUntilChanged().collect { if (it) autoUnlock() }
        }
        viewModelScope.launch {
            // the link came online while the app is open and unlocked: fetch what changed. Once per coming online
            // (the link's status changes several times a minute on a path that moves, and each word of it started a
            // whole sync), and not while the opening prompt's statement is on its way: its unlock reads the chat itself
            api.link.map { it?.phase == LinkPhase.ONLINE && it.paired }.distinctUntilChanged()
                .collect { online -> if (online && !session.locked.value && statement == null) { syncNow(); readTemporary() } }
        }
        viewModelScope.launch {
            // can't reach the Mac (kb PHN-753): only once it has stopped for good, or hasn't been online for
            // LINK_LOST_AFTER_MS of the app being open and unlocked; every open starts the count again (connecting is
            // normal: about a second after the link fix)
            combine(api.link.map { it?.phase }, session.locked, session.front) { phase, locked, front ->
                when {
                    phase == LinkPhase.STOPPED -> 2
                    phase == LinkPhase.ONLINE || phase == null || locked || !front -> 0
                    else -> 1
                }
            }.distinctUntilChanged()
                .collectLatest { away ->
                    if (away == 1) wait(LINK_LOST_AFTER_MS)
                    val lost = away != 0
                    if (_ui.value.linkLost != lost) _ui.update { it.copy(linkLost = lost) }
                }
        }
        viewModelScope.launch {
            // files cross the path again (PHN-444): what waited for it is tried once more, by itself
            api.link.map { it?.bulkAllowed == true && it.phase == LinkPhase.ONLINE }.distinctUntilChanged().collect { if (it) transfers.bulkAllowed() }
        }
        viewModelScope.launch {
            // the box's files and the messages on their way, as FERB left them (read off the main thread)
            withContext(io) { outgoing.load() }
            // a message with files that waited for the Mac, or for a path that carries files, starts when it can
            api.link.map { it?.phase to it?.bulkAllowed }.distinctUntilChanged().collect { outgoing.linkChanged() }
        }
        viewModelScope.launch {
            session.locked.collect { locked ->
                if (!locked) {
                    // the home screen only for an empty chat: a first-time user, or a new day with nothing said yet
                    // (kb PHN-626); the chat as soon as something was said today
                    // opened or unlocked: today's chat, on the day it is now (kb PHN-696, PHN-697)
                    turnTheDay()
                    saved["earlier"] = false
                    refresh()
                    // a temporary chat on the Mac shows again (locking or closing the app never ends it)
                    readTemporary()
                    outgoing.unlocked()
                    // a request for the exact location that came while the app was closed or locked
                    (api.locationWanted() as? Outcome.Ok)?.value?.let { pendingLocation = it; showPending() }
                } else {
                    // PHN-15: nothing private stays open; the preview stops, the fetched files go
                    AppLog.i("lock shown")
                    macJob?.cancel()
                    statementJob?.cancel()
                    statement = null
                    macPending = false
                    _ui.update { it.copy(macLine = null, needsMacUnlock = false, macNeed = null) }
                    stopListening()
                    live.stop()
                    camera.stop()
                    if (_sheet.value is Sheet.Live || _sheet.value is Sheet.Task || _sheet.value is Sheet.Settings || _sheet.value is Sheet.Days ||
                        _sheet.value is Sheet.Location || _sheet.value is Sheet.Camera || _sheet.value is Sheet.Insomnia ||
                        _sheet.value is Sheet.Session || _sheet.value is Sheet.SessionLive || _sheet.value == Sheet.NewSession) _sheet.value = null
                    // the coding sessions' conversations and folders are memory only, gone with the lock (kb HAND-651..666)
                    _sessions.value = SessionsUi()
                    _openSession.value = null
                    _newSession.value = null
                    _review.value = null
                    transfers.locked()
                    outgoing.locked()
                    _ui.update { it.copy(files = FilesUi(), settings = null, macTasks = null) }
                }
            }
        }
        viewModelScope.launch {
            session.front.collect { front ->
                if (!front) {
                    // leaving the app: what finished so far has been seen; the preview stops (PHN-454)
                    markSeen()
                    stopListening()
                    if (live.showing) closeLive()
                    // the Mac's camera only while he looks at it (APP-983)
                    if (camera.showing) closeCamera()
                } else {
                    transfers.dropKeyPlaceFiles(openedKeyFiles.toList())
                    openedKeyFiles.clear()
                    // disk work on its own job: this collector never waits on it, so a leave that follows at once
                    // still closes the camera and the preview right away
                    launch(Dispatchers.IO) { local.dropOld(now()) }
                    // back in front: a natural moment for the chat's day to move (kb PHN-697)
                    turnTheDay()
                    markRead()
                    showPending()
                }
            }
        }
        readMac()
        refresh()
        readTemporary()
    }

    // ---- the gate -------------------------------------------------------------------------------------------------

    private fun gateOf(u: UiState, s: PhoneState, l: LinkStatus?, locked: Boolean): UiState {
        val wiped = (s as? PhoneState.Open)?.wiped
        val keys = s.keys
        val removed = l?.phase == LinkPhase.STOPPED && l.problem?.code in REMOVED_CODES
        val gate = when {
            s is PhoneState.Failed -> Gate.FAILED
            !screenLock() -> Gate.NO_SCREEN_LOCK
            // the lock page needs neither the library, the store nor the link: it shows at the first frame (PHN-633)
            locked -> Gate.LOCKED
            s is PhoneState.Closed || s is PhoneState.Starting -> Gate.STARTING
            l == null -> Gate.STARTING
            l.phase == LinkPhase.PAIRING_WAIT_FOR_MAC -> Gate.CODES
            removed -> Gate.REMOVED
            u.macForgotPhone && l.paired -> Gate.REMOVED
            !l.paired -> Gate.PAIR
            keys?.pairAgain == true -> Gate.REMOVED
            else -> Gate.MAIN
        }
        val reason = when {
            removed -> PairReason.REMOVED
            u.macForgotPhone && l?.paired == true -> PairReason.UNKNOWN
            keys?.pairAgain == true && l?.paired == true -> PairReason.KEYS
            wiped != null -> PairReason.WIPED
            else -> null
        }
        var pairing = u.pairing
        var pairError = u.pairError
        if (pairing && l != null) {
            if (l.paired && l.phase == LinkPhase.ONLINE) pairing = false
            else if (l.phase == LinkPhase.STOPPED || l.phase == LinkPhase.WAITING || l.phase == LinkPhase.IDLE) {
                l.problem?.let { pairing = false; pairError = pairNote(it.code, it.detail) }
            }
        }
        return u.copy(gate = gate, pairReason = reason, pairing = pairing, pairError = pairError,
            failure = (s as? PhoneState.Failed)?.error?.let { "${it.javaClass.simpleName}: ${it.message}" })
    }

    /** Reads the gate again (the screen lock may have been set in Android's settings meanwhile). */
    fun recheck() {
        _ui.update { gateOf(it, api.state.value, api.link.value, session.locked.value) }
        showPending()
    }

    /** A notification's tap: the question opens as soon as the app is unlocked. */
    fun openQuestion(approvalId: String) {
        pending = approvalId
        saved["tab"] = PhoneTab.CHAT
        showPending()
    }

    private var pending: String? = null
    private var pendingCamera = false

    /** The notice's Show me (APP-983): the camera view opens as soon as the app is unlocked. */
    fun openCamera() {
        pendingCamera = true
        showPending()
    }
    private var pendingLocation: String? = null

    /** A notification's tap: the Mac's request for the exact location opens as soon as the app is unlocked. */
    fun openLocation(requestId: String) {
        pendingLocation = requestId
        showPending()
    }

    private fun showPending() {
        if (_ui.value.gate != Gate.MAIN || session.locked.value) return
        pending?.let { id ->
            pending = null
            goToQuestion(id)
            return
        }
        if (pendingCamera && session.front.value) {
            pendingCamera = false
            saved["tab"] = PhoneTab.MAC
            showCamera()
            return
        }
        val loc = pendingLocation ?: return
        // in front: the sheet; in the background the Notifier posts the request instead (no answer without the app)
        if (!session.front.value || (_sheet.value != null && _sheet.value !is Sheet.Location)) return
        pendingLocation = null
        show(Sheet.Location(loc))
    }

    // ---- reading ---------------------------------------------------------------------------------------------------

    /** Reads the transcript again (coalesced: a burst of changes is one read after the one under way). */
    fun refresh() {
        if (refreshing?.isActive == true) { dirty = true; return }
        refreshing = viewModelScope.launch {
            do {
                dirty = false
                val view = (api.transcript() as? Outcome.Ok)?.value
                apply(view)
            } while (dirty)
            markRead()
            if (tab.value == PhoneTab.TASKS) loadTasks()
        }
    }

    private fun markSeen() {
        val done = _ui.value.chatWork.earlier.mapNotNull { it.entry }
        if (done.isNotEmpty()) prefs.seen = prefs.seen + done
    }

    private var lastView: TranscriptView? = null

    /** When this model started, and whether the chat's first entries were logged (`ferb-app`, kb PHN-629). */
    private val born = android.os.SystemClock.elapsedRealtime()
    private var storedShown = false

    /** The phone's own zone and today in it (epoch days): the chat's days and "said today" (kb PHN-626, PHN-627). */
    private val zone: java.time.ZoneId get() = java.time.ZoneId.systemDefault()
    /** The chat's day rule, asked now (kb PHN-697): only ChatModel.chatDay says what "today" is. */
    private fun today(): Long = ChatModel.chatDay(now(), zone)

    /** The chat's held day catches up with the clock: only at a natural moment (kb PHN-697). */
    private fun turnTheDay() {
        val d = today()
        if (_ui.value.chatDay != d) _ui.update { it.copy(chatDay = d) }
    }

    /** The Mac's status changed: the screens are drawn again from the transcript as last read (one at a time). */
    private fun statusChanged() {
        if (statusJob?.isActive == true) { statusAgain = true; return }
        statusJob = viewModelScope.launch {
            do {
                statusAgain = false
                apply(null, statusOnly = true)
            } while (statusAgain)
        }
    }

    /**
     * The transcript and the Mac's last status (each question's needs, Stop everything, PHN-125) as the screens draw
     * them; [statusOnly]: the transcript as last read. The status is read from the library off the main thread (kb
     * PHN-682), so one apply runs at a time and the last one in has the newest of both.
     */
    private suspend fun apply(read: TranscriptView?, statusOnly: Boolean = false) = applying.withLock {
        val view = if (statusOnly) lastView else read
        lastView = view
        val status = api.lastStatus()
        // a temporary chat's questions and tasks (the status's rows marked `temporary`) show only in its own view
        val normal = status?.let { s -> s.copy(questions = s.questions.filterNot { it.temporary }, running = s.running.filterNot { it.temporary }) }
        val temporary = lastTemporary?.takeIf { it.on }?.let { t ->
            val own = status?.let { s -> s.copy(questions = s.questions.filter { it.temporary }, running = s.running.filter { it.temporary }) }
            TemporaryUi(t.id, ChatModel.rows(t.transcript), Questions.merged(Questions.open(t.transcript.questions), own),
                com.ferb.app.model.RunningModel.of(t.transcript, own, null))
        }
        // everything made from the whole transcript is made off the main thread: on a long chat it is many
        // milliseconds per streamed word, more than a frame on a phone (kb PHN-714). The rows read the phone library's
        // layout of FERB's words, made when the transcript was read and kept per entry (ChatModel.prepare, PHN-682).
        // A short chat is made where it is (well under a millisecond; no hop, so a test's main looper sees it at once).
        val make = {
            val questions = Questions.merged(view?.let { Questions.open(it.questions) }.orEmpty(), normal)
            Made(questions, WorkModel.of(view, ""), ChatModel.rows(view), com.ferb.app.model.RunningModel.of(view, normal, _ui.value.macTasks), ChatModel.days(view, zone))
        }
        val made = if ((view?.items?.size ?: 0) > OFF_MAIN_ENTRIES) withContext(io) { make() } else make()
        val questions = made.questions
        val chatWork = made.chatWork
        if (prefs.lastOpen == 0L && prefs.seen.isEmpty()) prefs.seen = chatWork.earlier.mapNotNull { it.entry }.toSet()
        val seen = prefs.seen
        val away = chatWork.earlier.filter { it.entry !in seen }
        _ui.update {
            it.copy(conversation = view?.conversation, rows = made.rows, running = made.running, chatWork = chatWork, away = away,
                questions = questions, older = view?.older ?: OlderState.Unknown, status = status, days = made.days, temporary = temporary)
        }
        showAlerts(status)
        if (!storedShown && _ui.value.rows.isNotEmpty()) {
            storedShown = true
            AppLog.i("chat entries ready ${android.os.SystemClock.elapsedRealtime() - born} ms after the app started: ${_ui.value.rows.size} rows, link ${_ui.value.link?.phase}")
        }
        rebuildWork()
    }

    // ---- the temporary chat (the owner, 2026-11-23) ----------------------------------------------------------------

    /** The temporary chat as last read from the Mac (memory only; null or off: the normal chat). */
    private var lastTemporary: com.ferb.mobile.TemporaryChat? = null
    private var temporaryJob: Job? = null
    private var temporaryAgain = false

    /**
     * Reads the temporary chat from the Mac (on start, unlock, the link coming online, and each `Change.Temporary`: it
     * went on or off on any client, or its turns changed). A read that fails keeps what shows; one at a time.
     */
    fun readTemporary() {
        if (temporaryJob?.isActive == true) { temporaryAgain = true; return }
        temporaryJob = viewModelScope.launch {
            do {
                temporaryAgain = false
                if (session.locked.value) return@launch
                (api.temporaryChat() as? Outcome.Ok)?.value?.let { showTemporary(it) }
            } while (temporaryAgain)
        }
    }

    private suspend fun showTemporary(t: com.ferb.mobile.TemporaryChat) {
        // off drops the copy at once: the normal chat shows again
        lastTemporary = t.takeIf { it.on }
        apply(null, statusOnly = true)
    }

    /** The dashed bubble: the same tap turns the temporary chat on and off (never queued; a failure says so). */
    fun toggleTemporary() = busy("temporary", tap = true) {
        val on = _ui.value.temporary == null
        when (val r = api.setTemporaryChat(on)) {
            is Outcome.Ok -> showTemporary(r.value)
            is Outcome.Failed -> note(Note(if (on) R.string.temporary_start_failed else R.string.temporary_end_failed))
        }
    }

    /** A message in the temporary chat: sent now with `fresh: true`, never the outbox; a failure keeps the words. */
    private suspend fun sendTemporary(text: String) {
        if (text.isEmpty()) return
        saved["draft"] = ""
        when (val r = api.sendTemporary(text)) {
            is Outcome.Ok -> showTemporary(r.value)
            is Outcome.Failed -> { saved["draft"] = text; failed(r.error) }
        }
    }

    /** What [apply] makes from a transcript, off the main thread. */
    private class Made(
        val questions: List<Question>,
        val chatWork: Work,
        val rows: List<ChatRow>,
        val running: com.ferb.app.model.RunningUi?,
        val days: Map<String, Long>,
    )

    /** The Tasks tab: the Mac's own list where it answers, else the chat's (offline, or the Mac's window closed). */
    private fun rebuildWork() {
        val u = _ui.value
        val waiting = waitingOf(u.questions, u.chatWork)
        val work = u.macTasks?.let { WorkModel.fromMac(it, waiting, u.chatWork) } ?: u.chatWork.copy(waiting = waiting)
        _ui.update { it.copy(work = work) }
    }

    private fun waitingOf(questions: List<Question>, chat: Work): List<WorkItem> {
        val fromChat = chat.waiting.associateBy { it.approvalId }
        return questions.map { q ->
            fromChat[q.approvalId] ?: WorkItem(MarkKind.ASKING, q.title.orEmpty(), WorkModel.oneLine(q.reason), approvalId = q.approvalId, taskId = q.taskId)
        }
    }

    private var tasksJob: Job? = null

    /** Reads the Mac's task list (no prompt: while the Mac's window is closed the chat's list stays). */
    fun loadTasks() {
        if (tasksJob?.isActive == true) return
        tasksJob = viewModelScope.launch {
            if (session.locked.value || api.link.value?.phase != LinkPhase.ONLINE) return@launch
            when (val r = api.tasks(TASKS_PAGE, null)) {
                is Outcome.Ok -> _ui.update { it.copy(macTasks = r.value.tasks, running = com.ferb.app.model.RunningModel.of(lastView, api.lastStatus(), r.value.tasks)) }
                is Outcome.Failed -> if (r.error is FerbException.NeedsUnlock) macLocked()
            }
            rebuildWork()
        }
    }

    private fun readMac() {
        viewModelScope.launch { (api.cachedMacInfo() as? Outcome.Ok)?.value?.let { m -> _ui.update { it.copy(mac = m) } } }
    }

    /** After the Mac's unlock: its name and this phone's pairing date, and whether a key waits to be registered. */
    private fun afterMacUnlock() {
        viewModelScope.launch {
            (api.macInfo() as? Outcome.Ok)?.value?.let { m -> _ui.update { it.copy(mac = m) } }
            val fresh = api.unregisteredKeys()
            _ui.update { it.copy(newKeys = fresh?.strong != null) }
        }
    }

    private suspend fun syncNow() {
        val asked = android.os.SystemClock.elapsedRealtime()
        AppLog.i("chat sync asked at ${asked - born} ms")
        when (val r = api.sync()) {
            is Outcome.Failed -> {
                AppLog.i("chat sync failed after ${android.os.SystemClock.elapsedRealtime() - asked} ms: ${r.error::class.simpleName}")
                if (r.error is FerbException.NeedsUnlock) macLocked()
            }
            is Outcome.Ok -> {
                AppLog.i("chat sync answered after ${android.os.SystemClock.elapsedRealtime() - asked} ms")
                macPending = false; _ui.update { it.copy(needsMacUnlock = false, macNeed = null) }
            }
        }
    }

    /** The chat on screen, the app open and unlocked: the Mac learns how far the owner has read (one mark, latest). */
    private fun markRead() {
        val conv = _ui.value.conversation ?: return
        if (_ui.value.gate != Gate.MAIN || session.locked.value || !session.front.value) return
        if (tab.value != PhoneTab.CHAT || home.value || _ui.value.temporary != null) return
        // one mark on its way at a time, and at most one more behind it: every read of the chat asks for one, and on
        // a slow link each started its own call to the Mac before the first had answered (kb PHN-682)
        if (markJob?.isActive == true) { markAgain = true; return }
        markJob = viewModelScope.launch {
            do {
                markAgain = false
                api.markReadLatest(_ui.value.conversation ?: conv)
            } while (markAgain)
        }
    }

    // ---- the app lock (PHN-8, PHN-13, PHN-125, PHN-649) ----------------------------------------------------------------

    /**
     * One prompt opens the app, at once, whatever the link is doing (PHN-649). On a paired phone the lock key signs,
     * inside that prompt, an unlock statement the phone makes itself; the app opens the moment the prompt passes and
     * the statement goes to the Mac whenever the link is up ([handStatement]). Not paired: the prompt alone.
     */
    fun unlockApp(auto: Boolean = false): Job = busy("unlock") {
        val p = prompts ?: return@busy
        session.takePrompt() // the owner's tap takes the due prompt too: after its cancel nothing asks by itself
        if (auto && !windowReady.value) { session.promptAgain(); AppLog.i("unlock: screen elsewhere, prompt later"); return@busy }
        AppLog.i("unlock started auto=$auto")
        val shown = now()
        if (api.statementReady()) {
            val payload = UnlockStatement.make(now())
            AppLog.i("prompt shown (signs the unlock statement)")
            val signer = p.signer(KeyKind.LOCK)
            val sig = withContext(io) { runCatching { signer.sign(payload) }.getOrNull() }
            AppLog.i("prompt result ${if (sig != null) "passed" else "cancelled"} after ${now() - shown} ms")
            if (sig == null) return@busy
            openApp()
            AppLog.i("local unlock done; the statement waits for the link")
            statement = UnlockStatement(payload, sig, steady())
            handStatement()
            return@busy
        }
        AppLog.i("prompt shown (alone)")
        val passed = p.local()
        AppLog.i("prompt result ${if (passed) "passed" else "cancelled"} after ${now() - shown} ms")
        if (passed) {
            openApp()
            api.unlocked()
            AppLog.i("local unlock done")
        }
    }

    /** The app is open: the lock's state, the open's facts (PHN-13). */
    private fun openApp() {
        session.unlocked()
        opened()
        statementExpired = false
        _ui.update { it.copy(macNeed = null, needsMacUnlock = false) }
    }

    /** The statement signed in the opening prompt, until the Mac took it (PHN-649). */
    private var statement: UnlockStatement? = null
    private var statementJob: Job? = null

    /** The statement grew too old before the link was up: the Mac's side needs the fingerprint once more. */
    private var statementExpired = false

    /**
     * Hands the opening prompt's statement to the Mac once the link is up and paired, with its age by the phone's
     * steady clock: no prompt, no line. A connection that hasn't proved the device key yet is tried again in a moment;
     * an older Mac (one that doesn't know the statement) gets the Mac's own unlock, one prompt; a statement older than
     * [STATEMENT_AGE_MS] is dropped and, if the Mac then says it is locked, the card asks once.
     */
    private fun handStatement() {
        val st = statement ?: return
        if (statementJob?.isActive == true) return
        statementJob = viewModelScope.launch {
            val left = STATEMENT_AGE_MS - (steady() - st.signedAt)
            val l = withTimeoutOrNull(left.coerceAtLeast(0)) {
                api.link.first { it != null && it.paired && it.phase == LinkPhase.ONLINE }
            }
            if (statement !== st) return@launch
            if (l == null) {
                AppLog.i("statement dropped: the link wasn't up in time")
                statement = null
                statementExpired = true
                return@launch
            }
            if (l.macOlder) {
                AppLog.i("statement: the Mac is older; its own unlock")
                statement = null
                unlockMacNow()
                return@launch
            }
            for (attempt in 1..STATEMENT_TRIES) {
                val age = steady() - st.signedAt
                if (age > STATEMENT_AGE_MS) break
                val sent = steady()
                when (val r = api.unlockWithStatement(st.payload, st.sig, age)) {
                    is Outcome.Ok -> {
                        AppLog.i("mac unlock by statement ok in ${steady() - sent} ms (age $age ms)")
                        statement = null
                        macPending = false
                        _ui.update { it.copy(needsMacUnlock = false, macNeed = null, macLine = null) }
                        refresh()
                        afterMacUnlock()
                        return@launch
                    }
                    is Outcome.Failed -> {
                        val code = r.error.linkCode ?: r.error.javaClass.simpleName
                        AppLog.i("mac unlock by statement failed $code (try $attempt, age $age ms)")
                        when (code) {
                            // the connection hasn't proved the device key yet: a moment, then again
                            "locked" -> wait(STATEMENT_RETRY_MS)
                            "unlock_invalid" -> { statement = null; macForgot(); return@launch }
                            "unlock_required" -> break
                            else -> {
                                if (api.link.value?.phase == LinkPhase.ONLINE && r.error !is FerbException.Offline) break
                                // the link dropped under it: once it is back, again
                                wait(STATEMENT_RETRY_MS)
                                api.link.first { it != null && it.phase == LinkPhase.ONLINE }
                            }
                        }
                    }
                }
            }
            statement = null
            statementExpired = true
            needMac(MacNeed.OLD)
        }
    }

    /** The Mac doesn't know this phone's keys (`unlock_invalid`): the page that says to pair again (PHN-649). */
    private fun macForgot() {
        AppLog.i("mac doesn't know this phone's keys: pair again")
        _ui.update { gateOf(it.copy(macForgotPhone = true, macNeed = null, needsMacUnlock = false), api.state.value, api.link.value, session.locked.value) }
    }

    /** The card: the Mac's side needs the fingerprint once, and why (PHN-649). Never while a statement is on its way. */
    private fun needMac(why: MacNeed) {
        if (statement != null || session.locked.value) return
        AppLog.i("mac needs the fingerprint: $why")
        _ui.update { it.copy(macNeed = why, needsMacUnlock = true, macLine = null) }
    }

    /** The Mac couldn't be unlocked yet (offline, or its answer was lost): it is, once the link is up again. */
    private var macPending = false
    private var macJob: Job? = null

    /**
     * The Mac said its window is closed (a private call came back `locked`). First the same read again ([RECHECKS]
     * times, [RECHECK_MS] apart): right after a reconnect the phone proves itself on the new connection and the window
     * is open again; a statement on its way settles it too. Still locked: the card asks once, with its reason
     * (PHN-649). Nothing asks by itself.
     */
    private fun macLocked() {
        if (macJob?.isActive == true || "unlock-mac" in _ui.value.busy || "unlock" in _ui.value.busy) return
        if (session.locked.value || api.link.value?.paired != true) return
        if (statement != null) { handStatement(); return }
        macJob = viewModelScope.launch {
            if (api.link.value?.phase != LinkPhase.ONLINE) { macPending = true; return@launch }
            var open = false
            for (i in 1..RECHECKS) {
                wait(RECHECK_MS)
                if (session.locked.value) return@launch
                if (api.link.value?.phase != LinkPhase.ONLINE) { macPending = true; return@launch }
                if (api.sync() is Outcome.Ok) { open = true; break }
            }
            if (open) {
                AppLog.i("mac locked: open again after the recheck")
                macPending = false
                _ui.update { it.copy(needsMacUnlock = false, macNeed = null) }
                refresh()
                return@launch
            }
            needMac(if (statementExpired) MacNeed.OLD else MacNeed.LAPSED)
        }
    }

    /** The card's Unlock: the Mac's own challenge unlock, one prompt (PHN-649). */
    fun unlockMac() = busy("unlock-mac", tap = true) { unlockMacNow() }

    private suspend fun unlockMacNow() {
        val p = prompts ?: return
        val asked = now()
        _ui.update { it.copy(macLine = MacLine.OPENING, needsMacUnlock = false, macNeed = null) }
        val call = viewModelScope.async { api.unlock(p.signer(KeyKind.LOCK)) }
        val r = withTimeoutOrNull(MAC_SLOW_MS) { call.await() } ?: run {
            _ui.update { it.copy(macLine = MacLine.SLOW) }
            call.await()
        }
        _ui.update { it.copy(macLine = null) }
        when (r) {
            is Outcome.Ok -> {
                AppLog.i("mac unlock answered ok in ${now() - asked} ms")
                statementExpired = false
                macPending = false
                refresh()
                afterMacUnlock()
            }
            is Outcome.Failed -> {
                val code = r.error.linkCode ?: r.error.javaClass.simpleName
                AppLog.i("mac unlock failed $code in ${now() - asked} ms")
                when {
                    r.error is FerbException.NotSigned -> needMac(MacNeed.CANCELLED)
                    code == "unlock_invalid" -> macForgot()
                    r.error is FerbException.NeedsSigner -> failed(r.error)
                    else -> { macPending = true; needMac(MacNeed.LAPSED) }
                }
            }
        }
    }

    /**
     * MainActivity: the activity is resumed and its window has the focus (false while paused, or while a system
     * dialog, the shade or a prompt is over it). The prompt by itself is shown only then (PHN-615).
     */
    fun windowReady(ready: Boolean) {
        windowReady.value = ready
    }

    /** The due prompt (PHN-615), taken once; none while another unlock is already asking. */
    private fun autoUnlock() {
        if (prompts == null) return
        val take = session.takePrompt()
        if (!take || "unlock" in _ui.value.busy) return
        unlockApp(auto = true)
    }

    /** The open before this one (the corpus's awayHours, lastOpen, firstToday, newPlace). */
    private var previousOpen: Double? = null
    private var previousZone: String? = null

    private fun opened() {
        // the first open after pairing is the one in which the pairing happened; the next open is an ordinary one
        prefs.justPaired = false
        previousOpen = prefs.lastOpen.takeIf { it > 0 }?.toDouble()
        previousZone = prefs.lastZone
        prefs.lastOpen = now() / 1000
        prefs.lastZone = ZoneId.systemDefault().id
    }

    /** PHN-133: registers the fingerprint key made after pairing (one prompt), so hard Yeses can use it. */
    fun registerKeys() = busy("keys", tap = true) {
        val p = prompts ?: return@busy
        when (val r = api.registerKeys { kind -> p.signer(kind, Why.KEYS) }) {
            is Outcome.Ok -> { _ui.update { it.copy(newKeys = false) }; if (r.value) note(Note(R.string.keys_registered)) }
            is Outcome.Failed -> if (r.error !is FerbException.NotSigned) failed(r.error)
        }
    }

    // ---- pairing (PHN-442, PHN-447) ---------------------------------------------------------------------------------

    fun pair(code: String) = busy("pair") {
        val text = code.trim()
        if (text.isEmpty()) return@busy
        _ui.update { it.copy(pairing = true, pairError = null) }
        _sheet.value = null
        when (val r = api.pair(text)) {
            is Outcome.Ok -> Unit
            is Outcome.Failed -> _ui.update {
                it.copy(pairing = false, pairError = pairNote(r.error.linkCode ?: r.code, r.detail))
            }
        }
    }

    /** Cancel on the codes screen: the half-made pairing is forgotten, ready to start again. */
    fun cancelPairing() = busy("pair") {
        _ui.update { it.copy(pairing = false) }
        api.forget()
    }

    fun clearPairError() = _ui.update { it.copy(pairError = null) }

    // ---- chat -----------------------------------------------------------------------------------------------------

    fun typed(text: String) {
        dictation?.cancel()
        saved["draft"] = text
    }

    // ---- speaking to FERB (kb PHN-603..PHN-605) ---------------------------------------------------------------------

    private var dictation: Dictation? = null
    private var spokenOnto = ""
    private var pressedAt = 0L
    private val _listening = MutableStateFlow(false)

    /** The owner holds the microphone and FERB is listening. */
    val listening: StateFlow<Boolean> = _listening.asStateFlow()
    private val _mic = MutableStateFlow(com.ferb.design.components.MicState.IDLE)

    /** What the microphone shows (kb PHN-643): getting ready, listening, finishing, or nothing. */
    val mic: StateFlow<com.ferb.design.components.MicState> = _mic.asStateFlow()
    private val _micLevel = MutableStateFlow(0f)

    /** How loud the phone hears him while it listens, 0..1. */
    val micLevel: StateFlow<Float> = _micLevel.asStateFlow()
    private var partials = 0
    private var voiceStarted = 0L
    private var released = false

    /**
     * The owner pressed the microphone. Before the first use of this phone's kind of recognition FERB says what happens
     * to his voice (Sheet.Voice), then Android asks for the microphone; with both, FERB listens while he holds, and
     * the words go into the message box after what is there. Nothing is recorded, stored or sent to the Mac; the words
     * are never sent by themselves.
     */
    fun voicePress(): VoicePress {
        // a new press while the last words are still on their way: those words are kept now and a new one starts
        dictation?.takeIf { it.finishing }?.let { AppLog.i("voice: pressed while finishing; kept and started again"); it.cancel() }
        if (dictation != null || _ui.value.gate != Gate.MAIN || session.locked.value) return VoicePress.DONE
        val input = voice
        val kind = input?.kind() ?: SpeechKind.NONE
        if (input == null || kind == SpeechKind.NONE) { note(Note(R.string.voice_unavailable)); return VoicePress.DONE }
        if (kind.name !in prefs.voiceTold) { show(Sheet.Voice(kind)); return VoicePress.DONE }
        if (!micAllowed()) return VoicePress.ASK_MICROPHONE
        pressedAt = now()
        voiceStarted = pressedAt
        partials = 0
        released = false
        spokenOnto = draft.value
        _listening.value = true
        _mic.value = com.ferb.design.components.MicState.READYING
        AppLog.i("voice start kind=$kind")
        val d = Dictation(input, kind, words = { said -> partials++; saved["draft"] = joinSpoken(spokenOnto, said) }, ended = ::voiceEnded,
            later = { ms, block -> val job = viewModelScope.launch { kotlinx.coroutines.delay(ms); block() }; { job.cancel() } },
            ready = {
                if (_mic.value == com.ferb.design.components.MicState.READYING) {
                    _mic.value = com.ferb.design.components.MicState.LISTENING
                    AppLog.i("voice ready after ${now() - voiceStarted} ms")
                }
            },
            level = { if (_mic.value == com.ferb.design.components.MicState.LISTENING) _micLevel.value = it })
        dictation = d
        d.start()
        return VoicePress.DONE
    }

    /** The owner let go: the last words come, then they stay in the message box. A quick tap says to hold instead. */
    fun voiceRelease() {
        val d = dictation ?: return
        if (now() - pressedAt < TAP_MS && d.text.isEmpty()) {
            d.cancel()
            note(Note(R.string.voice_hold))
            return
        }
        _mic.value = com.ferb.design.components.MicState.FINISHING
        _micLevel.value = 0f
        released = true
        d.finish()
    }

    /** The owner read the words on Sheet.Voice and goes on: Android's microphone question next, if it isn't answered. */
    fun voiceTold(kind: SpeechKind) {
        prefs.voiceTold = prefs.voiceTold + kind.name
        if (_sheet.value is Sheet.Voice) _sheet.value = null
    }

    /** Android's answer to the microphone question. */
    fun microphoneAnswered(granted: Boolean) = note(Note(if (granted) R.string.voice_ready else R.string.voice_denied))

    private fun voiceEnded(end: Dictation.End) {
        dictation = null
        _listening.value = false
        _mic.value = com.ferb.design.components.MicState.IDLE
        _micLevel.value = 0f
        when (end) {
            is Dictation.End.Words -> {
                AppLog.i("voice end partials=$partials after ${now() - voiceStarted} ms")
                saved["draft"] = joinSpoken(spokenOnto, end.text)
                // held, and nothing was heard: said, never nothing (kb PHN-643)
                if (end.text.isBlank() && released) note(Note(R.string.voice_no_speech))
            }
            is Dictation.End.Failed -> {
                AppLog.i("voice error code=${end.code} languageMissing=${end.languageMissing} after ${now() - voiceStarted} ms")
                saved["draft"] = spokenOnto
                // the phone has no on-device speech for this language: its own service next time, after its words
                val next = voice?.kind()
                if (end.languageMissing && next == SpeechKind.SERVICE && next.name !in prefs.voiceTold) show(Sheet.Voice(next))
                else note(Note(voiceFailure(end)))
            }
        }
    }

    /** Each way the recogniser can fail, in plain words (kb PHN-643); never nothing. */
    private fun voiceFailure(end: Dictation.End.Failed): Int = if (end.languageMissing) R.string.voice_language else when (end.code) {
        android.speech.SpeechRecognizer.ERROR_NO_MATCH, android.speech.SpeechRecognizer.ERROR_SPEECH_TIMEOUT -> R.string.voice_no_speech
        android.speech.SpeechRecognizer.ERROR_NETWORK, android.speech.SpeechRecognizer.ERROR_NETWORK_TIMEOUT,
        android.speech.SpeechRecognizer.ERROR_SERVER, android.speech.SpeechRecognizer.ERROR_SERVER_DISCONNECTED -> R.string.voice_network
        android.speech.SpeechRecognizer.ERROR_AUDIO -> R.string.voice_mic_busy
        android.speech.SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS -> R.string.voice_denied
        android.speech.SpeechRecognizer.ERROR_RECOGNIZER_BUSY, android.speech.SpeechRecognizer.ERROR_CLIENT,
        android.speech.SpeechRecognizer.ERROR_TOO_MANY_REQUESTS -> R.string.voice_busy
        else -> R.string.voice_failed
    }

    /** Leaving the app or the lock: the microphone stops at once, and what was said stays in the box. */
    private fun stopListening() { dictation?.cancel() }

    /** Sends through the outbox (PHN-401): with the Mac off it waits there, visible and cancellable. Files go with it. */
    fun send() = busy("send") {
        if (_ui.value.temporary != null) { sendTemporary(draft.value.trim()); return@busy }
        val text = draft.value.trim()
        val files = transfers.ready.value
        // files in the box: the message shows at once and goes when its files are on the Mac (kb PHN-690, PHN-691)
        if (outgoing.chips.value.isNotEmpty()) {
            saved["draft"] = ""
            // a send from the earlier days goes to today, where his message is (kb PHN-698)
            saved["earlier"] = false
            outgoing.send(text, files)
            if (files.isNotEmpty()) transfers.sent()
            return@busy
        }
        if (text.isEmpty() && files.isEmpty()) return@busy
        saved["draft"] = ""
        saved["earlier"] = false
        val r = if (files.isEmpty()) api.send(text) else api.sendWith(text, files)
        when (r) {
            is Outcome.Ok -> {
                if (files.isNotEmpty()) transfers.sent()
                if (r.value.waitingFor == "unlock") macLocked()
                cantNotifyOnce()
            }
            is Outcome.Failed -> { saved["draft"] = text; failed(r.error) }
        }
        refresh()
    }

    /** A file's cross in the message box: it goes; nothing was sent. */
    fun unpick(key: String) = outgoing.unpick(key)

    /** A message with files: stop it while its files go, try it again, send its words without the files, remove it. */
    fun stopSending(id: String) = outgoing.cancel(id)
    fun retrySending(id: String) = outgoing.retry(id)
    fun sendTextOnly(id: String) = outgoing.textOnly(id)
    fun removeSending(id: String) = outgoing.remove(id)

    fun cancelOutbox(id: String) = busy("cancel:$id") {
        when (val r = api.cancel(id)) {
            is Outcome.Ok -> if (r.value.mayHaveArrived) note(Note(R.string.outbox_may_have_arrived))
            is Outcome.Failed -> failed(r.error)
        }
        refresh()
    }

    /** Stop needs no unlock (PHN-125: a locked connection can stop FERB) and is never queued. */
    fun stop(taskId: String?) = busy("stop:$taskId") {
        if (taskId == null) return@busy
        when (val r = api.stop(listOf(taskId))) {
            is Outcome.Ok -> note(Note(R.string.stopped))
            is Outcome.Failed -> failed(r.error)
        }
    }

    /** "Stop everything on the Mac" (DMN-1, PHN-125): after the owner's confirmation; no fingerprint, never queued. */
    fun stopEverything() = busy("stop-all", tap = true) {
        _sheet.value = null
        when (val r = api.stopEverything()) {
            is Outcome.Ok -> { note(Note(R.string.stop_all_done)); api.status(); apply(null, statusOnly = true) }
            is Outcome.Failed -> failed(r.error)
        }
    }

    /**
     * "Ask me again" (PHN-611): FERB stops using the Mac's screen without asking; the line goes when the Mac's status
     * says so. When it ends on the Mac (the owner came back, the Mac slept) the status says so too; no notification.
     */
    fun endScreenUse() = busy("screen-use") {
        when (val r = api.endScreenUse()) {
            is Outcome.Ok -> {
                _ui.update { u -> u.copy(status = u.status?.copy(screenUse = r.value.screenUse)) }
                note(Note(R.string.screen_use_ended))
            }
            is Outcome.Failed -> failed(r.error)
        }
    }

    // ---- insomnia mode (APP-980..APP-985) ------------------------------------------------------------------------

    /** The Mac's insomnia mode as its status last said it (none: an older Mac, which can't do it). */
    private fun keepInsomnia(i: com.ferb.mobile.Insomnia?) {
        if (i != null) _ui.update { u -> u.copy(status = u.status?.copy(insomnia = i)) }
    }

    /** "Keep my Mac working" turned on: its warning first (APP-980), never straight on. */
    fun askInsomnia() = show(Sheet.Insomnia)

    /**
     * Keep it working (APP-980, APP-984): the way he chose and his one Yes, signed with the fingerprint like a setting
     * (a sensitive change on the Mac, PHN-112). The sheet goes once the Mac took it.
     */
    fun startInsomnia(variant: String, screenYes: Boolean) = busy("insomnia", tap = true) {
        val signer = freshSigner(Why.INSOMNIA) ?: return@busy
        when (val r = unlocker.run { api.startInsomnia(variant, screenYes, signer()) }) {
            is Outcome.Ok -> {
                keepInsomnia(r.value.insomnia)
                if (_sheet.value == Sheet.Insomnia) _sheet.value = null
                if (r.value.already) note(Note(R.string.insomnia_already))
            }
            is Outcome.Failed -> if (r.error !is FerbException.NotSigned) failed(r.error)
        }
    }

    /** End (plain: it only takes something away; no prompt). How it ended is said once from the Mac's status. */
    fun endInsomnia() = busy("insomnia-end", tap = true) {
        when (val r = api.endInsomnia()) {
            is Outcome.Ok -> keepInsomnia(r.value.insomnia)
            is Outcome.Failed -> failed(r.error)
        }
    }

    /** Lock my Mac (APP-983): the owner's button only; FERB never locks by itself. */
    fun lockMac() = busy("lock-mac", tap = true) {
        when (val r = api.lockMac()) {
            is Outcome.Ok -> {
                keepInsomnia(r.value.insomnia)
                if (r.value.text.isNotEmpty()) note(Note(R.string.err_mac_said, r.value.text))
            }
            is Outcome.Failed -> failed(r.error)
        }
    }

    /** It's fine: the notice is answered; FERB goes on. */
    fun insomniaFine(noticeId: String) = busy("insomnia-fine", tap = true) {
        when (val r = api.dismissInsomniaNotice(noticeId)) {
            is Outcome.Ok -> keepInsomnia(r.value)
            is Outcome.Failed -> failed(r.error)
        }
    }

    /** Show me (APP-983): the camera view; the library refuses it on the public relay and the view says why. */
    fun showCamera() {
        _sheet.value = Sheet.Camera
        camera.start()
    }

    /** The app locked under the camera: unlock (one prompt) and look again. */
    fun cameraAgain() = busy("camera") {
        if (_sheet.value != Sheet.Camera) return@busy
        val p = prompts ?: return@busy
        if (camera.view.value?.needsUnlock == true) {
            when (val r = api.unlock(p.signer(KeyKind.LOCK, Why.OPEN))) {
                is Outcome.Ok -> _ui.update { it.copy(needsMacUnlock = false) }
                is Outcome.Failed -> { if (r.error !is FerbException.NotSigned) failed(r.error); return@busy }
            }
        }
        camera.start()
    }

    fun closeCamera() {
        camera.stop()
        if (_sheet.value == Sheet.Camera) _sheet.value = null
    }

    fun loadOlder() = busy("older") {
        val conv = _ui.value.conversation ?: return@busy
        if (_ui.value.older !is OlderState.Available) return@busy
        when (val r = api.loadOlder(conv)) {
            is Outcome.Failed -> if (r.error is FerbException.NeedsUnlock) macLocked()
            is Outcome.Ok -> Unit
        }
        refresh()
    }

    // ---- questions (PHN-415, PHN-417, PHN-470) ----------------------------------------------------------------------

    /**
     * Sends an answer. Where the Mac's status says the answer needs the phone's proof, the fingerprint (on a phone
     * without one, the lock key's prompt) is asked up front; a plain No and an empty answer never ask. A refusal still
     * asks once, as before (AnswerFlow).
     */
    fun answer(approvalId: String, input: AnswerInput) = busy("answer:$approvalId") {
        val p = prompts ?: return@busy
        val q = _ui.value.questions.firstOrNull { it.approvalId == approvalId }
        val declines = (input is AnswerInput.Approve && !input.granted) || (input is AnswerInput.Words && input.text.isEmpty())
        // "until I'm back" needs the app's unlock (the locked path below unlocks and tries once more), never a fingerprint
        val upfront = if (q != null && q.needsProof && !declines && input !is AnswerInput.UntilBack) {
            if (q.needs?.fingerprint == true) KeyKind.STRONG else KeyKind.LOCK
        } else {
            null
        }
        val flow = AnswerFlow(api, { api.state.value.keys }) { kind -> p.signer(kind, Why.YES) }
        answered(approvalId, flow.answer(approvalId, input, upfront))
    }

    private fun answered(approvalId: String, r: Answered) {
        when (r) {
            Answered.Sent -> refresh()
            Answered.NotSent -> Unit
            Answered.NeedsPairingAgain -> note(Note(R.string.answer_needs_pairing))
            is Answered.Failed -> failed(r.error)
        }
    }

    /**
     * A hidden answer (PHN-109, PHN-110; kb PHN-536): the [count] typed characters of [chars] go into the library's
     * [SecretInput] one UTF-16 unit at a time (each a plain number, so the secret never becomes a String or a byte
     * buffer here), [chars] is overwritten with zeros at once, before anything else happens, and the SecretInput is
     * emptied and destroyed when the answer ends, whether it was sent, refused or the prompt was cancelled (the owner
     * types it again then). Nothing the view model keeps holds the secret.
     */
    fun answerHidden(approvalId: String, chars: CharArray, count: Int): Job {
        val secret = SecretInput()
        try {
            for (i in 0 until count.coerceIn(0, chars.size)) secret.push(chars[i].code.toUShort())
        } finally {
            chars.fill('\u0000')
        }
        val what = "answer:$approvalId"
        return viewModelScope.launch {
            try {
                val p = prompts ?: return@launch
                if (what in _ui.value.busy) return@launch
                _ui.update { it.copy(busy = it.busy + what) }
                try {
                    val flow = AnswerFlow(api, { api.state.value.keys }) { kind -> p.signer(kind, Why.YES) }
                    answered(approvalId, flow.answerHidden(approvalId, secret))
                } finally {
                    _ui.update { it.copy(busy = it.busy - what) }
                }
            } finally {
                runCatching { secret.clear() }
                secret.destroy()
            }
        }
    }

    // ---- files (PHN-113, PHN-115, PHN-121, PHN-127) ----------------------------------------------------------------

    /**
     * The Files tab opened: the Mac's recent files for this phone (the Mac's unlock: asked once if its window closed)
     * and the places' counts, read only when cheap (no prompt; left out when they can't be read).
     */
    fun filesOpened() {
        if (session.locked.value || api.link.value?.phase != LinkPhase.ONLINE) return
        loadRecent()
        if (_ui.value.files.counts.isNotEmpty()) return
        viewModelScope.launch {
            val counts = HashMap<String, Long>()
            for (place in PLACES) {
                val r = api.files(place, null)
                if (r is Outcome.Ok) counts[place] = r.value.total.toLong() else break
            }
            _ui.update { it.copy(files = it.files.copy(counts = counts)) }
        }
    }

    /** The Mac's recent files: made by FERB, fetched or sent by this phone (T-1211). */
    fun loadRecent() = busy("recent") {
        when (val r = unlocker.run { api.recentFiles(RECENT_FILES) }) {
            is Outcome.Ok -> _ui.update { it.copy(files = it.files.copy(recent = r.value, recentNote = null)) }
            is Outcome.Failed -> if (r.error !is FerbException.NotSigned) _ui.update { it.copy(files = it.files.copy(recentNote = errorNote(r.error))) }
        }
    }

    /** Searches names on the Mac, inside the folder open (or the whole home folder). Never by contents. */
    fun search(query: String) {
        val q = query.trim()
        if (q.isEmpty()) return
        val path = _ui.value.files.folder?.path
        _ui.update { it.copy(files = it.files.copy(search = SearchUi(q, path))) }
        busy("search") {
            val r = unlocker.run { api.searchFiles(q, path) }
            _ui.update { u ->
                val s = u.files.search?.takeIf { it.query == q && it.path == path } ?: return@update u
                u.copy(files = u.files.copy(search = when (r) {
                    is Outcome.Ok -> s.copy(loading = false, found = r.value)
                    is Outcome.Failed -> s.copy(loading = false, problem = if (r.error is FerbException.NotSigned) Note(R.string.files_not_opened) else errorNote(r.error))
                }))
            }
        }
    }

    /** The next page of matches, added under the ones shown (PHN-609: 500 a page, as many pages as the owner asks for). */
    fun searchMore() {
        val s = _ui.value.files.search ?: return
        val next = s.found?.next ?: return
        busy("more") {
            val r = unlocker.run { api.searchFiles(s.query, s.path, next) }
            _ui.update { u ->
                val now = u.files.search?.takeIf { it.query == s.query && it.path == s.path && it.found?.next == next } ?: return@update u
                val had = now.found ?: return@update u
                u.copy(files = u.files.copy(search = when (r) {
                    is Outcome.Ok -> now.copy(found = r.value.copy(files = had.files + r.value.files))
                    is Outcome.Failed -> now.copy(problem = errorNote(r.error))
                }))
            }
        }
    }

    fun closeSearch() = _ui.update { it.copy(files = it.files.copy(search = null)) }

    /** A found or recent file: a folder opens in the browser (the search closes), a file is fetched as from a folder. */
    fun openFound(f: FoundFile) {
        if (f.dir) { closeSearch(); openFolder(f.path) } else transfers.fetch(f.path, f.name, f.size.toLong(), false)
    }

    /**
     * Gets the open folder whole (PHN-13): the Mac lists every file under it, then each is fetched into this phone's
     * fetched files. A key place's folder is not fetched whole (each of its files asks for the fingerprint by itself).
     */
    fun fetchFolder() = busy("tree") {
        val f = _ui.value.files.folder ?: return@busy
        if (f.locked) return@busy
        when (val r = unlocker.run { api.tree(f.path, null) }) {
            is Outcome.Ok -> if (r.value.files.isEmpty()) note(Note(R.string.files_tree_empty)) else transfers.fetchFolder(r.value)
            is Outcome.Failed -> if (r.error !is FerbException.NotSigned) _ui.update { it.copy(files = it.files.copy(problem = filesNote(r.error))) }
        }
    }

    /** A folder the owner picked in the system's folder picker (a tree Uri as text): sent whole to the Mac. */
    fun sendFolder(uri: String, forChat: Boolean) = busy("stage") {
        val folder = withContext(Dispatchers.IO) { local.folder(uri) }
        when {
            folder == null -> note(Note(R.string.files_folder_unreadable))
            folder.files.isEmpty() -> note(Note(R.string.files_folder_empty))
            else -> transfers.sendFolder(folder, forChat)
        }
    }

    /** A file on a finished task's result card (kb PHN-535): fetched by the task and its place; opened on a tap. */
    fun fetchTaskFile(taskId: String, index: Int, name: String) {
        transfers.fetchTaskFile(taskId, index.toLong(), name)
    }

    /** Opens a folder of the Mac; a key place asks for the fingerprint for that very path first (PHN-121). */
    fun openFolder(path: String, keyPlace: Boolean = false) = busy("folder") {
        _ui.update { it.copy(files = it.files.copy(loading = path, problem = null)) }
        val r = unlocker.withProof(keyPlace, Why.FILE) { signer -> api.files(path, signer) }
        _ui.update {
            when (r) {
                is Outcome.Ok -> it.copy(files = it.files.copy(folder = r.value, loading = null))
                is Outcome.Failed -> it.copy(files = it.files.copy(loading = null, problem = filesNote(r.error)))
            }
        }
    }

    /**
     * The open folder's next page, added under the entries shown (PHN-609: 500 a page, as many pages as the owner
     * asks for); a key place's folder asks for the fingerprint for its path again, as opening it did (PHN-121).
     */
    fun folderMore() {
        val f = _ui.value.files.folder ?: return
        val next = f.next ?: return
        busy("more") {
            val r = unlocker.withProof(f.locked, Why.FILE) { signer -> api.files(f.path, signer, next) }
            _ui.update {
                val now = it.files.folder?.takeIf { cur -> cur.path == f.path && cur.next == next } ?: return@update it
                when (r) {
                    is Outcome.Ok -> it.copy(files = it.files.copy(folder = r.value.copy(entries = now.entries + r.value.entries)))
                    is Outcome.Failed -> it.copy(files = it.files.copy(problem = filesNote(r.error)))
                }
            }
        }
    }

    /** Up one folder; from a place, back to the places. */
    fun folderUp() {
        val f = _ui.value.files.folder ?: return
        val parent = f.parent
        if (parent == null || f.path in PLACES || f.path == "~") _ui.update { it.copy(files = it.files.copy(folder = null, problem = null)) }
        else openFolder(parent)
    }

    fun openEntry(e: FileEntry) {
        if (e.dir) openFolder(e.path, e.locked) else transfers.fetch(e.path, e.name, e.size.toLong(), e.locked)
    }

    /** The owner opened or shared a fetched file in another app (his tap): a key place's copy goes when he is back. */
    fun handedOut(key: String) {
        if (transfers.list.value.firstOrNull { it.key == key }?.keyPlace == true) openedKeyFiles.add(key)
    }

    /** Files the owner picked in the system's picker (content Uris as text): copied here, then sent to the Mac. */
    fun sendFiles(uris: List<String>, forChat: Boolean) = busy("stage") {
        if (uris.isEmpty()) return@busy
        val staged = withContext(Dispatchers.IO) { uris.mapNotNull(local::stage) }
        if (staged.size < uris.size) note(Note(R.string.files_unreadable))
        // for the chat they wait in the message box until Send (kb PHN-690); from the Files tab the tap is the send
        if (staged.isNotEmpty()) { if (forChat) outgoing.pick(staged) else transfers.send(staged, forChat = false) }
    }

    /** The files of a finished batch go to the composer, to send with a message. */
    fun attachToChat(batch: String) {
        transfers.attach(batch)
        openChat()
    }

    /** Pauses a file, or the file under way in a batch. */
    fun pause(key: String) {
        val t = transfers.list.value.firstOrNull { it.key == key } ?: return
        val running = if (t.batch != null) transfers.list.value.firstOrNull { it.batch == t.batch && it.phase == Phase.RUNNING } else t
        running?.let { transfers.pause(it.key) }
    }

    private fun filesNote(e: FerbException): Note = when {
        e is FerbException.NotSigned -> Note(R.string.files_not_opened)
        else -> errorNote(e)
    }

    // ---- FERB's review of what an import brought in (kb MEM-460..457) -------------------------------------------------

    /** The review as the Mac has it now, read when its card shows (the Mac's unlock if its window closed). A failure keeps the list. */
    fun loadReview() = busy(com.ferb.app.model.Review.LOAD) {
        when (val r = unlocker.run { api.memoryReview() }) {
            is Outcome.Ok -> _review.value = r.value
            is Outcome.Failed -> AppLog.i("memory review not read: ${r.error.linkCode ?: r.error.javaClass.simpleName}")
        }
    }

    /**
     * The owner's answer to items of one part (one item, a group's or all of the part's), on its way under [key]
     * ([com.ferb.app.model.Review.key]): the Mac's reply is the list from then on (an answered item leaves it). A failure
     * leaves the list as it was and says why.
     */
    fun answerReview(part: String, ids: List<String>, answer: String, key: String = com.ferb.app.model.Review.key(part, ids.singleOrNull())) =
        busy(key, tap = true) {
            when (val r = unlocker.run { api.answerMemoryReview(part, ids, answer) }) {
                is Outcome.Ok -> {
                    _review.value = r.value
                    // the last answer closes FERB's question on the Mac: the chat follows
                    if (r.value.count == 0uL) refresh()
                }
                is Outcome.Failed -> if (r.error !is FerbException.NotSigned) failed(r.error)
            }
        }

    // ---- the owner's coding sessions (kb HAND-651..666) ------------------------------------------------------------------

    /** The list, as the Mac says it now. The first read may ask for the Mac's unlock; the later ones never prompt. */
    private suspend fun readSessions() {
        val first = !_sessions.value.loaded
        val r = if (first) unlocker.run { api.sessionsList() } else api.sessionsList()
        when (r) {
            is Outcome.Ok -> _sessions.update { u ->
                // a new terminal said to be on its way: its line stays until a session the list didn't hold shows
                val coming = u.coming?.takeIf { before -> r.value.sessions.all { it.id in before } }
                SessionsUi(true, r.value.enabled, r.value.sessions, null, r.value.noChats, coming)
            }
            is Outcome.Failed -> if (r.error !is FerbException.NotSigned) _sessions.update { it.copy(loaded = true, problem = errorNote(r.error)) }
        }
    }

    /** One session's screen, opened from its row: its conversation is read at once and while it shows. */
    fun openSession(id: String) {
        val known = _sessions.value.sessions.firstOrNull { it.id == id }
        if (_openSession.value?.id != id) _openSession.value = SessionUi(id, known)
        show(Sheet.Session(id))
    }

    fun closeSession() {
        if (_sheet.value is Sheet.Session || _sheet.value is Sheet.SessionLive) show(null)
        _openSession.value = null
    }

    /** Reads the open session: its newest page first, then what came after the newest item held. */
    private suspend fun readSession(id: String) {
        val o = _openSession.value?.takeIf { it.id == id } ?: return
        if (!o.loaded) {
            when (val r = unlocker.run { api.sessionTimeline(id, 0, 0, Sessions.PAGE) }) {
                is Outcome.Ok -> _openSession.update { u ->
                    u?.takeIf { it.id == id }?.copy(session = r.value.session ?: u.session, items = r.value.items.sortedBy { it.seq },
                        more = r.value.more, cards = r.value.cards, loaded = true, problem = null)?.settled()
                }
                is Outcome.Failed -> if (r.error !is FerbException.NotSigned) _openSession.update { it?.copy(problem = errorNote(r.error)) }
            }
            return
        }
        // a page may hold only part of what came: ask again at once while the Mac says there is more after it
        repeat(5) {
            val after = _openSession.value?.takeIf { it.id == id }?.last ?: return
            when (val r = api.sessionTimeline(id, after, 0, Sessions.AFTER_PAGE)) {
                is Outcome.Ok -> {
                    _openSession.update { u ->
                        u?.takeIf { it.id == id }?.copy(session = r.value.session ?: u.session, items = Sessions.appended(u.items, r.value.items),
                            cards = r.value.cards, problem = null)?.settled()
                    }
                    if (!r.value.more || r.value.items.isEmpty()) return
                }
                is Outcome.Failed -> { _openSession.update { it?.copy(problem = errorNote(r.error)) }; return }
            }
        }
    }

    /** "Show earlier": the page before the oldest item held. */
    fun sessionOlder() = busy("session-older") {
        val o = _openSession.value ?: return@busy
        if (!o.more || o.items.isEmpty()) return@busy
        when (val r = unlocker.run { api.sessionTimeline(o.id, 0, o.first, Sessions.PAGE) }) {
            is Outcome.Ok -> _openSession.update { u -> u?.takeIf { it.id == o.id }?.copy(items = Sessions.prepended(u.items, r.value.items), more = r.value.more) }
            is Outcome.Failed -> if (r.error !is FerbException.NotSigned) failed(r.error)
        }
    }

    /** The open session's message box. */
    fun sessionTyped(text: String) = _openSession.update { it?.copy(draft = text) }

    /** A sent message's bubble leaves once the session's own record of it is held (PHN-812). */
    private fun SessionUi.settled(): SessionUi = copy(outgoing = Sessions.settled(outgoing, items))

    /**
     * Send: his words leave the box at once and show as his message with "Sending…" while FERB types them on the Mac
     * (owner 2026-11-23, PHN-812). The Mac sends it when this phone controls the session; else it asks first (GATE-622)
     * and nothing was sent: the bubble goes, the question shows over the screen with the exact words, and his Yes is
     * [sessionYes] (Cancel puts the words back in the box).
     */
    fun sessionSend() = busy("session-send", tap = true) {
        val o = _openSession.value ?: return@busy
        val text = o.draft
        if (text.isBlank() || o.outgoing?.phase == SessionSendPhase.SENDING) return@busy
        _openSession.update { u -> if (u?.id == o.id) u.copy(draft = "") else u }
        sendWords(o.id, text)
    }

    /** Try again on a message the Mac refused (PHN-800): the same words, the same way (asked first if not controlled). */
    fun sessionRetry() = busy("session-send", tap = true) {
        val o = _openSession.value ?: return@busy
        val u = o.outgoing?.takeIf { it.phase == SessionSendPhase.NOT_SENT } ?: return@busy
        sendWords(o.id, u.text)
    }

    /** His words as sending (PHN-812): one bubble after the conversation's last line, in place of a refused one. */
    private fun sending(id: String, text: String) = _openSession.update { u ->
        if (u?.id == id) u.copy(outgoing = SessionSending(text, SessionSendPhase.SENDING, after = u.last)) else u
    }

    private suspend fun sendWords(id: String, text: String) {
        if (text.isBlank()) return
        sending(id, text)
        when (val r = unlocker.run { api.sessionSend(id, text) }) {
            is Outcome.Ok -> {
                val yes = r.value.needsYes
                if (yes != null) {
                    // nothing was sent: his Yes comes first, and only then is it on its way
                    _openSession.update { u -> if (u?.id == id) u.copy(outgoing = null, ask = SessionConfirm(text, yes)) else u }
                } else sessionSent(id, text, r.value)
            }
            is Outcome.Failed -> notSent(id, text, r.error)
        }
    }

    /**
     * PHN-800: a send the Mac refused (or that never reached it) never looks sent: his bubble dims with "Not sent." and
     * why, in the Mac's plain words, and Try again; the question, if one was open, closes. A prompt he cancelled sent
     * nothing: the words go back to the box.
     */
    private suspend fun notSent(id: String, text: String, e: FerbException) {
        if (e is FerbException.NotSigned) {
            _openSession.update { u ->
                if (u?.id != id || u.outgoing?.text != text) u
                else u.copy(outgoing = null, draft = u.draft.ifBlank { text })
            }
            return
        }
        _openSession.update { u ->
            if (u?.id != id) u
            else u.copy(ask = null, pick = null, outgoing = SessionSending(text, SessionSendPhase.NOT_SENT, errorNote(e), u.last),
                draft = if (u.draft == text) "" else u.draft)
        }
        // HAND-683: FERB can't tell which terminal is the session's: the ones it can see, for him to choose
        if (e is FerbException.Refused && e.linkCode == "choose_terminal") {
            readSession(id)
            val terminals = _openSession.value?.takeIf { it.id == id }?.session?.terminals.orEmpty()
            if (terminals.isNotEmpty()) _openSession.update { it?.copy(pick = TerminalPick(text, e.detail, terminals)) }
        }
    }

    /**
     * HAND-683: his choice of terminal for these words, signed with the fingerprint as control is (GATE-622): the Mac
     * types only there, keeps it for the session while he controls it, and asks again if it changes.
     */
    fun sessionPickTerminal(terminal: String) = busy("session-pick", tap = true) {
        val o = _openSession.value ?: return@busy
        val pick = o.pick ?: return@busy
        val signer = freshSigner(Why.YES) ?: return@busy
        // signed: the choice closes and his words are on their way (PHN-812)
        _openSession.update { it?.copy(pick = null) }
        sending(o.id, pick.text)
        when (val r = unlocker.run { api.sessionControl(o.id, pick.text, terminal, signer()) }) {
            is Outcome.Ok -> sessionSent(o.id, pick.text, r.value)
            is Outcome.Failed -> notSent(o.id, pick.text, r.error)
        }
    }

    /** Don't send: nothing is typed; his words stay as not sent, with Try again. */
    fun sessionPickNo() = _openSession.update { it?.copy(pick = null) }

    /**
     * The Mac typed it: his bubble shows as sent until the session's own record of it arrives (PHN-812); a send the Mac
     * couldn't see arrive yet is said.
     */
    private suspend fun sessionSent(id: String, text: String, sent: com.ferb.mobile.SessionSent) {
        if (!sent.sent) {
            _openSession.update { u -> if (u?.id == id && u.outgoing?.text == text) u.copy(outgoing = null) else u }
            return
        }
        _openSession.update { u ->
            if (u?.id != id) u
            else u.copy(draft = if (u.draft == text) "" else u.draft,
                outgoing = u.outgoing?.takeIf { it.text == text }?.copy(phase = SessionSendPhase.SENT, reason = null) ?: u.outgoing).settled()
        }
        if (!sent.delivered) note(Note(R.string.sessions_not_seen))
        readSession(id)
    }

    /**
     * The owner's Yes to the Mac's question (GATE-622): this phone controls the session from now on and the exact words
     * are sent, signed with the fingerprint (the fingerprint-only key where the phone has one). The question closes at
     * once and his words show as sending (PHN-812); a cancelled prompt sends nothing and the question opens again.
     */
    fun sessionYes() = busy("session-yes", tap = true) {
        val o = _openSession.value ?: return@busy
        val ask = o.ask ?: return@busy
        val signer = freshSigner(Why.YES) ?: return@busy
        if (!ask.thenInterrupt) {
            _openSession.update { it?.copy(ask = null) }
            sending(o.id, ask.text)
        }
        when (val r = unlocker.run { api.sessionControl(o.id, ask.text, "", signer()) }) {
            is Outcome.Ok -> {
                _openSession.update { it?.copy(ask = null) }
                if (ask.thenInterrupt) interruptNow(o.id, asked = true) else sessionSent(o.id, ask.text, r.value)
            }
            // PHN-800: refused after his Yes: the words show as not sent, never stuck open
            is Outcome.Failed -> when {
                ask.thenInterrupt -> if (r.error !is FerbException.NotSigned) { _openSession.update { it?.copy(ask = null) }; failed(r.error) }
                // the fingerprint cancelled: nothing went; the question is back as it was
                r.error is FerbException.NotSigned -> _openSession.update { u ->
                    if (u?.id == o.id) u.copy(ask = ask, outgoing = u.outgoing?.takeUnless { it.text == ask.text && it.phase == SessionSendPhase.SENDING }) else u
                }
                else -> notSent(o.id, ask.text, r.error)
            }
        }
    }

    /** Cancel on the Mac's question: nothing is sent; the words go back to the box. */
    fun sessionNo() = _openSession.update { u -> u?.copy(ask = null, draft = u.draft.ifBlank { u.ask?.text.orEmpty() }) }

    /** The session's menu (PHN-800). */
    fun sessionMenu(open: Boolean) = _openSession.update { it?.copy(menu = open) }

    /** Stop its turn (needs control of the session: the same question with no words first, then the stop). */
    fun sessionInterrupt() = busy("session-interrupt", tap = true) {
        val id = _openSession.value?.id ?: return@busy
        interruptNow(id, asked = false)
    }

    private suspend fun interruptNow(id: String, asked: Boolean) {
        when (val r = unlocker.run { api.sessionInterrupt(id) }) {
            is Outcome.Ok -> { note(Note(R.string.sessions_interrupted)); readSession(id) }
            is Outcome.Failed -> {
                // refused: when it is because this phone doesn't control the session, the Mac's send asks its question
                // (nothing is sent with no words), and his Yes takes control before the stop; else the Mac's words
                val yes = if (!asked && r.error is FerbException.Refused) (api.sessionSend(id, "") as? Outcome.Ok)?.value?.needsYes else null
                if (yes != null) _openSession.update { it?.copy(ask = SessionConfirm("", yes, thenInterrupt = true)) } else failed(r.error)
            }
        }
    }

    /** Stop controlling: this phone's sends ask again (allowed while locked: it only takes something away). */
    fun sessionRelease() = busy("session-release", tap = true) {
        val id = _openSession.value?.id ?: return@busy
        when (val r = api.sessionRelease(id)) {
            is Outcome.Ok -> {
                _openSession.update { u -> u?.copy(session = u.session?.copy(controlledHere = false)) }
                readSession(id)
            }
            is Outcome.Failed -> failed(r.error)
        }
    }

    /** One of the card's answers (its choice's id), signed each time (HAND-663). */
    fun sessionAnswer(cardId: String, choiceId: String) = busy("session-answer:$cardId", tap = true) {
        val id = _openSession.value?.id ?: return@busy
        val signer = freshSigner(Why.YES) ?: return@busy
        when (val r = unlocker.run { api.sessionAnswer(id, cardId, choiceId, signer()) }) {
            is Outcome.Ok -> {
                _openSession.update { u -> u?.copy(cards = u.cards.filter { it.id != cardId }) }
                readSession(id)
            }
            is Outcome.Failed -> if (r.error !is FerbException.NotSigned) failed(r.error)
        }
    }

    /** Hand to FERB: its sheet over the session's screen. */
    fun askHandOver(open: Boolean = true) = _openSession.update { it?.copy(handOver = open, handProblem = null) }

    /**
     * Hand it over (signed): the owner's goal becomes FERB's task in the chat; FERB says so. The sheet's button turns
     * at the tap and stays so until the Mac answers (HAND-702): done, the sheet closes and the session is read at once;
     * refused, the sheet stays with his words and the Mac's reason under the button.
     */
    fun handOver(goal: String) = busy("session-hand", tap = true) {
        val id = _openSession.value?.id ?: return@busy
        if (goal.isBlank()) return@busy
        _openSession.update { it?.copy(handProblem = null) }
        val signer = freshSigner(Why.YES) ?: return@busy
        when (val r = unlocker.run { api.sessionDelegate(id, goal.trim(), signer()) }) {
            is Outcome.Ok -> {
                _openSession.update { it?.copy(handOver = false, handProblem = null) }
                note(Note(R.string.sessions_handed))
                readSession(id)
            }
            is Outcome.Failed -> if (r.error !is FerbException.NotSigned) {
                // the sheet still open: the reason under its button; closed meanwhile: said as every refusal is
                if (_openSession.value?.let { it.id == id && it.handOver } == true) _openSession.update { it?.copy(handProblem = errorNote(r.error)) }
                else failed(r.error)
            }
        }
    }

    /**
     * Take it back (HAND-702): stops FERB's task that keeps the session going (the task stop every task has; no unlock,
     * never queued, as Stop). The menu's row turns at the tap; done, the menu closes and the session is read at once (its
     * chip goes when the Mac says so); refused, the Mac's reason.
     */
    fun takeBack() = busy("session-take-back", tap = true) {
        val o = _openSession.value ?: return@busy
        val task = o.session?.ferbTask?.takeIf { it.isNotBlank() } ?: return@busy
        when (val r = api.stop(listOf(task))) {
            is Outcome.Ok -> {
                _openSession.update { u -> if (u?.id == o.id) u.copy(menu = false) else u }
                readSession(o.id)
            }
            is Outcome.Failed -> note(Note(R.string.sessions_take_back_failed, inner = errorNote(r.error)))
        }
    }

    /** Watch: the session's window live, like Watch live (never asked for on the public relay: its rule is said). */
    fun watchSession() {
        val id = _openSession.value?.id ?: return
        val l = api.link.value
        if (l?.path == LinkPath.PUBLIC_RELAY || (l?.phase == LinkPhase.ONLINE && !l.bulkAllowed)) {
            note(Note(R.string.not_on_shared_relay))
            return
        }
        _sheet.value = Sheet.SessionLive(id)
        live.startSession(id)
    }

    /** Turn on (the off state's one button): the Mac's `sessions.enabled`, a signed setting like any other (PHN-133). */
    fun turnOnSessions() = busy("sessions-on", tap = true) {
        val signer = freshSigner(Why.SETTING) ?: return@busy
        when (val r = unlocker.run { api.setSetting(SESSIONS_SETTING, "true", signer()) }) {
            is Outcome.Ok -> readSessions()
            is Outcome.Failed -> if (r.error !is FerbException.NotSigned) failed(r.error)
        }
    }

    /** New session (HAND-691): a terminal; the sheet, with the folders the Mac offers (its sessions' first, then recent ones). */
    fun newSession() = busy("session-new", tap = true) {
        _newSession.value = NewSessionUi()
        show(Sheet.NewSession)
        when (val r = unlocker.run { api.sessionPrepare("", "", "") }) {
            is Outcome.Ok -> _newSession.update { it?.copy(loading = false, folders = r.value.folders, folder = it.folder ?: r.value.folders.firstOrNull()) }
            is Outcome.Failed -> {
                if (r.error !is FerbException.NotSigned) failed(r.error)
                if (_sheet.value == Sheet.NewSession) show(null)
                _newSession.value = null
            }
        }
    }

    fun newSessionFolder(folder: String) = _newSession.update { it?.copy(folder = folder, problem = null) }
    fun newSessionText(text: String) = _newSession.update { it?.copy(text = text, problem = null) }

    // ---- "Another folder…" (PHN-821): the Files tab's browser over phone.files, folders only, over the sheet --------------

    /** Opens the Mac's folders over the sheet: the places and the recent folders, read as the Files tab reads them. */
    fun newSessionBrowse() {
        if (_newSession.value == null) return
        _newSession.update { it?.copy(browse = FolderPickUi()) }
        viewModelScope.launch {
            val r = unlocker.run { api.recentFiles(RECENT_FILES) }
            if (r is Outcome.Ok) _newSession.update { n ->
                n?.browse?.let { b -> n.copy(browse = b.copy(recent = r.value.files.filter { it.dir }.map { it.path }.distinct())) } ?: n
            }
        }
    }

    /** A folder of the Mac opened in the picker (a key place asks for the fingerprint for that very path, PHN-121). */
    fun newSessionBrowseOpen(path: String, keyPlace: Boolean = false) = busy("session-browse") {
        _newSession.update { n -> n?.browse?.let { n.copy(browse = it.copy(loading = path, problem = null)) } ?: n }
        val r = unlocker.withProof(keyPlace, Why.FILE) { signer -> api.files(path, signer) }
        _newSession.update { n ->
            val b = n?.browse?.takeIf { it.loading == path } ?: return@update n
            n.copy(browse = when (r) {
                is Outcome.Ok -> b.copy(trail = b.trail + r.value, loading = null)
                is Outcome.Failed -> b.copy(loading = null, problem = filesNote(r.error))
            })
        }
    }

    /** Back in the picker: the folder before, then the places; from the places, back to the sheet as it was. */
    fun newSessionBrowseBack() = _newSession.update { n ->
        val b = n?.browse ?: return@update n
        n.copy(browse = if (b.trail.isEmpty()) null else b.copy(trail = b.trail.dropLast(1), loading = null, problem = null))
    }

    /** Closes the picker: the sheet as it was. */
    fun newSessionBrowseClose() = _newSession.update { it?.copy(browse = null) }

    /** "Use this folder": the folder showing becomes the chosen one, first in the sheet's list. */
    fun newSessionBrowseUse() = _newSession.update { n ->
        val path = n?.browse?.shown?.path ?: return@update n
        n.copy(browse = null, picked = path, folder = path, problem = null)
    }

    /** Start: with a command, the Mac's question showing exactly it first (nothing runs yet); with none, it opens. */
    fun newSessionNext() = busy("session-prepare", tap = true) {
        val n = _newSession.value ?: return@busy
        val folder = n.chosen ?: return@busy
        _newSession.update { it?.copy(problem = null) }
        when (val r = unlocker.run { api.sessionPrepare(Sessions.TERMINAL, folder, n.text.trim()) }) {
            is Outcome.Ok -> {
                val yes = r.value.needsYes
                if (yes != null) _newSession.update { it?.copy(yes = yes) } else startNow(n, folder)
            }
            is Outcome.Failed -> if (r.error !is FerbException.NotSigned) _newSession.update { it?.copy(problem = errorNote(r.error)) }
        }
    }

    /** The owner's Yes to the command: signed, it opens a terminal on the Mac in the folder and types exactly it. */
    fun newSessionStart() = busy("session-start", tap = true) {
        val n = _newSession.value ?: return@busy
        startNow(n, n.chosen ?: return@busy)
    }

    /**
     * Opens it (signed). Start shows its pending state until the Mac answers (PHN-822): opened, the sheet closes and the
     * new session opens when the Mac named it, else the list says it will show in a moment until it does; refused, the
     * sheet stays with the Mac's reason under Start.
     */
    private suspend fun startNow(n: NewSessionUi, folder: String) {
        _newSession.update { it?.copy(problem = null) }
        val signer = freshSigner(Why.YES) ?: return
        when (val r = unlocker.run { api.sessionStart(Sessions.TERMINAL, folder, n.text.trim(), signer()) }) {
            is Outcome.Ok -> {
                if (_sheet.value == Sheet.NewSession) show(null)
                _newSession.value = null
                note(Note(R.string.sessions_started))
                val id = r.value.session
                if (id.isNotBlank()) openSession(id)
                else {
                    _sessions.update { it.copy(coming = it.sessions.map { s -> s.id }.toSet()) }
                    readSessions()
                }
            }
            is Outcome.Failed -> if (r.error !is FerbException.NotSigned) _newSession.update { it?.copy(problem = errorNote(r.error)) }
        }
    }

    /** Back from the command to the choices. */
    fun newSessionBack() = _newSession.update { it?.copy(yes = null, problem = null) }

    fun closeNewSession() {
        if (_sheet.value == Sheet.NewSession) show(null)
        _newSession.value = null
    }

    // ---- Watch live (PHN-454) ----------------------------------------------------------------------------------------

    /** Watch live: never asked for on the public relay (its rule is said instead); elsewhere a full screen of its own. */
    fun watchLive(taskId: String?) {
        if (taskId == null) return
        val l = api.link.value
        if (l?.path == LinkPath.PUBLIC_RELAY || (l?.phase == LinkPhase.ONLINE && !l.bulkAllowed)) {
            note(Note(R.string.not_on_shared_relay))
            return
        }
        _sheet.value = Sheet.Live(taskId)
        live.start(taskId)
    }

    /** A running task's short view (kb PHN-725): its larger picture comes from the same live preview as Watch live. */
    fun showTask(taskId: String?, worker: String? = null) {
        if (taskId == null) return
        _sheet.value = Sheet.Task(taskId, worker)
        val l = api.link.value
        if (l?.path != LinkPath.PUBLIC_RELAY && !(l?.phase == LinkPhase.ONLINE && !l.bulkAllowed)) live.start(taskId, worker)
    }

    /** The work row is on screen with these tasks' pictures (empty: it went): kb PHN-725. */
    fun workRowShown(taskIds: List<String>) { workRowShows.value = taskIds }

    // ---- FERB's alerts (kb APP-1105, APP-1106, PHN-791) ---------------------------------------------------------------

    /** The newest alert the Mac's status lists that wasn't put away here (FERB.app shows the newest the same way). */
    private fun showAlerts(status: PhoneStatus?) {
        val waiting = status?.alerts.orEmpty().filter { it.id !in dismissedAlerts }
        _ui.update {
            it.copy(alert = waiting.lastOrNull(), alertCount = waiting.size, alerts = waiting,
                alertsOpen = it.alertsOpen && waiting.size > 1)
        }
    }

    /** "N alerts" tapped: the stack opens (every waiting alert, each with its ×) or closes (kb APP-1109). */
    fun toggleAlerts() = _ui.update { it.copy(alertsOpen = !it.alertsOpen && it.alertCount > 1) }

    /** His finger went down on the toast or came up (kb APP-1109). */
    fun alertHeld(held: Boolean) { alertHeld.value = held }

    /** The alert's ×: gone here at once, and on every device once the Mac has it (`attention.ack`). */
    fun dismissAlert(id: String) {
        dismissedAlerts.add(id)
        showAlerts(_ui.value.status)
        viewModelScope.launch { api.dismissAlert(id) }
    }

    /** A card on screen with its rows' pictures ([keys]: Thumbnails.key of task and step); empty: it went (kb PHN-726). */
    fun rowPictures(row: String, keys: List<String>) {
        rowPictureKeys.update { m -> if (keys.isEmpty()) m - row else m + (row to keys) }
    }

    /** The Mac's window closed under the preview: unlock (one prompt) and watch again. */
    fun watchAgain() = busy("live") {
        val s = _sheet.value
        if (s !is Sheet.Live && s !is Sheet.SessionLive) return@busy
        val p = prompts ?: return@busy
        if (live.view.value?.needsUnlock == true) {
            when (val r = api.unlock(p.signer(KeyKind.LOCK))) {
                is Outcome.Ok -> _ui.update { it.copy(needsMacUnlock = false) }
                is Outcome.Failed -> { if (r.error !is FerbException.NotSigned) failed(r.error); return@busy }
            }
        }
        if (s is Sheet.Live) live.start(s.taskId) else if (s is Sheet.SessionLive) live.startSession(s.id)
    }

    fun closeLive() {
        live.stop()
        when (val s = _sheet.value) {
            is Sheet.Live -> _sheet.value = null
            // a session's window: back to its conversation
            is Sheet.SessionLive -> _sheet.value = Sheet.Session(s.id)
            else -> Unit
        }
    }

    // ---- FERB's settings (PHN-404, PHN-112, PHN-133) ----------------------------------------------------------------

    /** "Try again" on the connection sheet (kb PHN-752): the link dials now; it never waits on the main thread. */
    fun retryLink() {
        viewModelScope.launch(io) { api.retryLink() }
    }

    fun openSettings() = busy("settings", tap = true) {
        show(Sheet.Settings)
        when (val r = unlocker.run { api.settings() }) {
            is Outcome.Ok -> _ui.update { it.copy(settings = MacSettings.rows(r.value)) }
            is Outcome.Failed -> {
                if (r.error !is FerbException.NotSigned) failed(r.error)
                if (_ui.value.settings == null && _sheet.value == Sheet.Settings) _sheet.value = null
            }
        }
    }

    /** One change, one prompt: the fingerprint-only key where the phone has one, else the lock key (PHN-133). */
    fun setSetting(key: String, valueJson: String) = busy("setting:$key", tap = true) {
        val spec = MacSettings.spec(key) ?: run { AppLog.i("setting $key unknown here"); note(Note(R.string.err_app_older)); return@busy }
        val signer = freshSigner(Why.SETTING) ?: return@busy
        when (val r = unlocker.run { api.setSetting(key, valueJson, signer()) }) {
            is Outcome.Ok -> {
                val row = MacSettings.row(spec, r.value)
                _ui.update { u -> u.copy(settings = u.settings?.map { if (it.spec.key == key) row else it }) }
                if (_sheet.value is Sheet.Days) _sheet.value = Sheet.Settings
            }
            is Outcome.Failed -> if (r.error !is FerbException.NotSigned) failed(r.error)
        }
    }

    /**
     * The signer for one sensitive call (PHN-112, PHN-133): the fingerprint-only key where the phone has one, else the
     * lock key. A fingerprint key made after the pairing that the Mac doesn't know yet is registered first (its one
     * prompt, kb PHN-656). Null when there is none (said) or the owner cancelled.
     */
    private suspend fun freshSigner(why: Why): (() -> com.ferb.mobile.Signer)? {
        val p = prompts ?: run { note(Note(R.string.err_try_again)); return null }
        if (api.unregisteredKeys()?.strong != null) {
            AppLog.i("$why: registering the fingerprint key first")
            when (val r = api.registerKeys { kind -> p.signer(kind, Why.KEYS) }) {
                is Outcome.Ok -> _ui.update { it.copy(newKeys = false) }
                is Outcome.Failed -> { if (r.error !is FerbException.NotSigned) failed(r.error); return null }
            }
        }
        val kind = unlocker.freshKind() ?: run { note(Note(R.string.needs_pairing_again)); return null }
        return { p.signer(kind, why) }
    }

    fun setSwitch(key: String, on: Boolean) = setSetting(key, if (on) "true" else "false")

    fun setDays(key: String, text: String) {
        val days = text.trim().toIntOrNull()
        if (days == null || days < 1) { note(Note(R.string.set_days_invalid)); return }
        setSetting(key, days.toString())
    }

    // ---- navigation and sheets ------------------------------------------------------------------------------------

    /** The Chat tab tapped while on Chat: the chat goes to its end (kb PHN-662). Heard only by a chat on screen. */
    private val _chatToEnd = kotlinx.coroutines.flow.MutableSharedFlow<Unit>(extraBufferCapacity = 1)
    val chatToEnd: kotlinx.coroutines.flow.SharedFlow<Unit> = _chatToEnd

    fun choose(t: PhoneTab) {
        if (t == PhoneTab.CHAT) {
            // choosing the Chat tab is a natural moment for the chat's day to move (kb PHN-697); on the chat already:
            // from the earlier days back to today, in today to its end (kb PHN-662, PHN-698)
            turnTheDay()
            if (tab.value == PhoneTab.CHAT) { if (earlier.value) saved["earlier"] = false else _chatToEnd.tryEmit(Unit) }
        }
        saved["tab"] = t
        when (t) {
            PhoneTab.TASKS -> loadTasks()
            PhoneTab.FILES -> filesOpened()
            PhoneTab.CHAT -> markRead()
            else -> Unit
        }
    }

    /** "Earlier messages" (the home page's, or the top of today's chat): the earlier days (kb PHN-696). */
    fun openChat(focus: Boolean = false) {
        saved["tab"] = PhoneTab.CHAT; saved["earlier"] = true; markRead()
        if (focus) _focusAsk.value = true
    }

    private val _focusAsk = MutableStateFlow(false)
    /** The message box takes the cursor once (a tap on FERB's question's notification: he answers it there, kb PHN-739). */
    val focusAsk: StateFlow<Boolean> = _focusAsk.asStateFlow()
    fun focusTaken() { _focusAsk.value = false }

    /** Back to today from the earlier days (kb PHN-698): its chat, or its home page while nothing was said on it. */
    fun backToToday() {
        saved["tab"] = PhoneTab.CHAT
        saved["earlier"] = false
        turnTheDay()
    }

    fun show(s: Sheet?) {
        val watching = { x: Sheet? -> x is Sheet.Live || x is Sheet.Task || x is Sheet.SessionLive }
        if (watching(_sheet.value) && !watching(s)) live.stop()
        if (_sheet.value is Sheet.Camera && s !is Sheet.Camera) camera.stop()
        _sheet.value = s
        if (s == null && pendingLocation != null) showPending()
    }

    fun note(n: Note?) = _ui.update { it.copy(note = n) }

    private fun readSettings() = PhoneSettings(prefs.stayConnected, prefs.blockScreenshots, prefs.hideChecksOnLockScreen, prefs.pairedSince, prefs.shareCity)

    /** The profile's switches changed outside the view model (the service, the screenshot switch): read them again. */
    fun refreshProfile() { _settings.value = readSettings() }

    /** What keeps FERB's questions from reaching this phone (NotifyHealth, set by the activity; none in tests). */
    var notifyBlocker: () -> NotifyBlocker? = { null }

    /** The app is on screen (read again by the profile sheet when he comes back from Android's settings). */
    val front: StateFlow<Boolean> get() = session.front

    /**
     * Once per kind, after he sends something from the phone: if FERB's questions couldn't reach him once he leaves,
     * he is told now, while he is still here, with the tap that fixes it (kb PHN-765).
     */
    private fun cantNotifyOnce() {
        val b = notifyBlocker() ?: return
        if (b.name in prefs.toldBlockers || _sheet.value != null) return
        prefs.toldBlockers = prefs.toldBlockers + b.name
        show(Sheet.CantNotify(b))
    }

    fun hideChecks(on: Boolean) { prefs.hideChecksOnLockScreen = on; refreshProfile() }

    /** Forget this Mac (PHN-408): the fetched chat, the Mac, the keys and the files here go; the pairing screen comes. */
    fun forget() = busy("forget", tap = true) {
        _sheet.value = null
        live.stop()
        when (val r = api.forget()) {
            is Outcome.Ok -> {
                prefs.forgetPairing(); saved["tab"] = PhoneTab.CHAT; saved["earlier"] = false
                withContext(Dispatchers.IO) { transfers.clear(); outgoing.clear() }
                statement = null
                _ui.update { it.copy(mac = null, macTasks = null, files = FilesUi(), settings = null, status = null,
                    macForgotPhone = false, macNeed = null, needsMacUnlock = false) }
                // PHN-646: the Mac removed this phone too, or it couldn't be told (then it still lists it until removed there)
                AppLog.i("forgot the Mac; the Mac was ${if (r.value) "told" else "not reachable"}")
                if (!r.value) note(Note(R.string.forget_mac_kept))
            }
            is Outcome.Failed -> failed(r.error)
        }
    }

    /** The home screen's words for this moment (the lab's corpus). */
    fun homeWords(sky: SkyMoment): HomeWords? {
        val c = corpus ?: return null
        val u = _ui.value
        val link = when (u.link?.path) {
            LinkPath.DIRECT -> "direct"; LinkPath.OWN_RELAY -> "own"; LinkPath.PUBLIC_RELAY -> "shared"; else -> "offline"
        }
        val m = Moment(sky, u.work.waiting.size, u.away.count { it.kind == MarkKind.DONE },
            u.away.count { it.kind == MarkKind.FAILED }, u.work.running.size, link,
            firstRun = u.link?.paired != true, justPaired = prefs.justPaired, lastOpen = previousOpen, lastZone = previousZone)
        return c.texts(m)
    }

    /**
     * The sky's place (PHN-420): the last coarse fix, rounded to 2 decimals, while the owner shares the city; else the
     * time zone's principal place.
     */
    fun skyPlace(): SkyPlace = coarse.value?.let { SkyPlace(it.latitude, it.longitude, ZoneId.systemDefault()) } ?: SkyPlace.fromZone(ZoneId.systemDefault())

    /** The coarse fix as it changes (the sky follows it). */
    val skyFix: StateFlow<Coarse?> get() = coarse

    // ---- the exact location for a task (GATE-613, SENSE-420; kb PHN-537) -----------------------------------------------

    /**
     * The owner said yes on the sheet: Android's permission was asked by the screen first ([granted]); then one fix
     * (satellites where allowed, else the network, at most [EXACT_WAIT_MS]) goes to the Mac, or the reason none does.
     */
    fun shareLocation(requestId: String, granted: Boolean) = busy("location") {
        val l = locator
        val fix = if (granted && l != null && l.coarse()) l.exact(EXACT_WAIT_MS) else null
        val why = when {
            fix != null -> null
            !granted || l == null || !l.coarse() -> say(R.string.loc_why_denied)
            else -> say(R.string.loc_why_none)
        }
        answerLocation(requestId, fix, why)
    }

    /** The owner said no: the Mac is told at once, plainly. */
    fun declineLocation(requestId: String) = busy("location") {
        answerLocation(requestId, null, say(R.string.loc_why_no))
    }

    private suspend fun answerLocation(requestId: String, fix: com.ferb.mobile.LocationFix?, why: String?) {
        if ((_sheet.value as? Sheet.Location)?.requestId == requestId) _sheet.value = null
        locationAnswered()
        when (val r = api.answerLocation(requestId, fix, why)) {
            is Outcome.Ok -> note(Note(if (fix != null) R.string.exact_sent else R.string.exact_not_sent))
            is Outcome.Failed -> failed(r.error)
        }
    }

    /** The owner's switch for sharing the city changed (the screen asked Android first): read the profile again. */
    fun cityChanged(granted: Boolean) {
        if (!granted) note(Note(R.string.city_denied))
        refreshProfile()
    }

    // ---- helpers ---------------------------------------------------------------------------------------------------

    private fun failed(e: FerbException) = note(errorNote(e))

    /**
     * One job of [what] at a time. A tap ([tap]) that comes while it still runs is never a silent no-op: it says FERB
     * is still on it (kb PHN-656); the control shows its working state from `ui.busy` meanwhile.
     */
    private fun busy(what: String, tap: Boolean = false, block: suspend () -> Unit): Job = viewModelScope.launch {
        if (what in _ui.value.busy) {
            if (tap) note(Note(R.string.still_working))
            AppLog.i("tap while $what is still working")
            return@launch
        }
        _ui.update { it.copy(busy = it.busy + what) }
        try { block() } finally { _ui.update { it.copy(busy = it.busy - what) } }
    }

    override fun onCleared() {
        dictation?.cancel()
        live.stop()
        camera.stop()
        super.onCleared()
    }

    companion object {
        /**
         * How long the link may be not online, with the app open and unlocked, before the app says it can't reach the
         * Mac: 10 s, the owner's (kb PHN-757); the connect itself takes about a second after the link fix.
         */
        const val LINK_LOST_AFTER_MS = 10_000L
        /**
         * The opening prompt's statement is handed to the Mac at most this long after it was signed (PHN-649): the
         * Mac's own bound is `statement_age_s`, 120 s; the phone stops a little sooner so a send in flight still fits.
         */
        const val STATEMENT_AGE_MS = 115_000L

        /** A statement refused because the connection hasn't proved the device key yet is sent again after this. */
        const val STATEMENT_RETRY_MS = 1_000L
        const val STATEMENT_TRIES = 6

        /** No answer from the Mac's unlock in this long: the line says the Mac is slow (the app is open already). */
        const val MAC_SLOW_MS = 6_000L

        /**
         * A `locked` refusal is read again [RECHECKS] times, this long apart, before anything is asked: right after a
         * reconnect the phone proves itself on the new connection and the Mac's window is open again.
         */
        const val RECHECK_MS = 1_500L
        const val RECHECKS = 2

        /** A press shorter than this that heard nothing is a tap: FERB says to hold the microphone instead. */
        const val TAP_MS = 350L

        /** What was in the message box, then the spoken words (one space between). */
        fun joinSpoken(before: String, said: String): String = when {
            said.isBlank() -> before
            before.isBlank() -> said
            else -> before.trimEnd() + " " + said
        }
        const val TASKS_PAGE = 30

        /** Above this many entries the chat's rows are made off the main thread (kb PHN-714). */
        const val OFF_MAIN_ENTRIES = 100

        /** How many recent files the Files tab asks for. */
        const val RECENT_FILES = 20

        /** The exact fix's bound: the Mac waits 60 s for the answer; the owner's yes and the fix share that. */
        const val EXACT_WAIT_MS = 20_000L

        /** The lab's three places (PhoneFilesPage), as the Mac's paths. */
        val PLACES = listOf("~/Desktop", "~/Documents", "~/Downloads")

        /** The Mac's switch for the coding sessions (kb HAND-651..666): turning it on also adds FERB's hook to Claude Code. */
        const val SESSIONS_SETTING = "sessions.enabled"

        /** The Mac no longer knows this phone: pair again. */
        val REMOVED_CODES = setOf("device_revoked", "not_paired", "unknown_phone")

        fun errorNote(e: FerbException): Note = when {
            e.linkCode == Unlocker.PAIR_AGAIN -> Note(R.string.needs_pairing_again)
            // a feature the paired Mac's FERB doesn't have yet: said, never a silent failure (kb PHN-656)
            e.linkCode == "mac_link_older" || e.linkCode == "method_not_allowed" -> Note(R.string.err_mac_older)
            e.linkCode == "app_older" -> Note(R.string.err_app_older)
            e is FerbException.Offline -> Note(R.string.err_offline)
            e is FerbException.NeedsBetterPath -> Note(R.string.err_needs_better_path)
            e is FerbException.NeedsUnlock -> Note(R.string.err_needs_unlock)
            e is FerbException.Refused || e is FerbException.NotFound -> Note(R.string.err_mac_said, e.detail)
            else -> Note(R.string.err_other, e.detail)
        }

        fun pairNote(code: String?, detail: String?): Note = when (code) {
            "pair_window_closed" -> Note(R.string.pair_expired)
            "pair_secret_wrong", "bad_request", "refused" -> Note(R.string.pair_wrong_code)
            "device_limit" -> Note(R.string.pair_limit)
            "pair_denied" -> Note(R.string.pair_denied)
            "pair_busy" -> Note(R.string.pair_busy)
            "already_paired" -> Note(R.string.pair_already)
            "mac_unreachable", "offline" -> Note(R.string.pair_unreachable)
            else -> Note(R.string.err_other, detail)
        }
    }
}
