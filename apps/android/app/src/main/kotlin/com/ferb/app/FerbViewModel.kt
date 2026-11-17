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
import com.ferb.app.model.MacSettings
import com.ferb.app.model.Moment
import com.ferb.app.model.Phase
import com.ferb.app.model.PhoneApi
import com.ferb.app.model.Question
import com.ferb.app.model.Questions
import com.ferb.app.model.SettingRow
import com.ferb.app.model.Transfers
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
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

/** What fills the screen: one at a time, nothing private before [MAIN] (PHN-8). */
enum class Gate { STARTING, FAILED, NO_SCREEN_LOCK, LOCKED, PAIR, CODES, REMOVED, MAIN }

/** Why the pairing screen shows again (or null on a first run). */
enum class PairReason { WIPED, REMOVED, KEYS }

/** What slides up over the page. Only ids are kept (saved state holds no question words and never a private value). */
sealed interface Sheet {
    data class Asking(val approvalId: String) : Sheet
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

    /** The words before the owner's first use of speaking to FERB, for this phone's kind of recognition (PHN-604). */
    data class Voice(val kind: SpeechKind) : Sheet

    /** The Mac asks for this phone's exact location once, for a task (GATE-613); the owner says yes or no here. */
    data class Location(val requestId: String) : Sheet

    /** The words before Android's own question, when the owner turns on sharing the city (SENSE-420). */
    data object City : Sheet
}

/** What pressing the microphone needs from the screen: nothing more, or Android's microphone question. */
enum class VoicePress { DONE, ASK_MICROPHONE }

/** A short line for the owner, in the app's words (string resources) or the Mac's own. */
data class Note(@StringRes val res: Int, val words: String? = null)

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
    /** What the chat holds (the Tasks tab falls back to it when the Mac's own list can't be read). */
    val chatWork: Work = Work(emptyList(), emptyList(), emptyList()),
    val work: Work = Work(emptyList(), emptyList(), emptyList()),
    /** What finished or failed since the app was last open (the home screen's list and the corpus's counts). */
    val away: List<WorkItem> = emptyList(),
    val questions: List<Question> = emptyList(),
    val older: OlderState = OlderState.Unknown,
    val needsMacUnlock: Boolean = false,
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
    val files: FilesUi = FilesUi(),
    /** FERB's settings as read from the Mac, or null before (or when they couldn't be read). */
    val settings: List<SettingRow>? = null,
    /** A fingerprint enrolled after pairing that the Mac doesn't know yet (PHN-133). */
    val newKeys: Boolean = false,
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
) : ViewModel() {
    private val _ui = MutableStateFlow(UiState())
    val ui: StateFlow<UiState> = _ui.asStateFlow()

    /** The chosen tab and whether the chat tab shows the home screen; saved across process death. */
    val tab: StateFlow<PhoneTab> = saved.getStateFlow("tab", PhoneTab.CHAT)
    val home: StateFlow<Boolean> = saved.getStateFlow("home", true)
    val draft: StateFlow<String> = saved.getStateFlow("draft", "")
    private val _settings = MutableStateFlow(readSettings())
    val settings: StateFlow<PhoneSettings> = _settings.asStateFlow()
    private val _sheet = MutableStateFlow<Sheet?>(null)
    val sheet: StateFlow<Sheet?> = _sheet.asStateFlow()

    /** Questions shown once already (presented or dismissed): a question slides up by itself only the first time. */
    private val presented = HashSet<String>(saved.get<ArrayList<String>>("presented").orEmpty())
    var prompts: Prompts? = null
    private val windowReady = MutableStateFlow(false)
    private var refreshing: Job? = null
    private var dirty = false

    private val unlocker = Unlocker(api, { api.state.value.keys }) { kind, why -> prompts?.signer(kind, why) }
    val transfers = Transfers(viewModelScope, api, local, unlocker)
    val live = LiveWatch(viewModelScope, api)

    /** Key places' files the owner opened in another app: they go when he is back in FERB (PHN-11). */
    private val openedKeyFiles = HashSet<String>()

    private val _mustSecure = MutableStateFlow(false)

    /**
     * A screen that must stay out of screenshots whatever the owner's switch says: a private answer being typed
     * (PHN-110) and Watch live (PHN-454). MainActivity adds FLAG_SECURE while it is true.
     */
    val mustSecure: StateFlow<Boolean> = _mustSecure.asStateFlow()

    init {
        viewModelScope.launch {
            combine(_sheet, _ui) { s, u ->
                s is Sheet.Live || (s is Sheet.Asking && u.questions.firstOrNull { it.approvalId == s.approvalId }?.hidden == true)
            }.collect { _mustSecure.value = it }
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
                    is Change.Status -> apply(lastView)
                    is Change.Mac -> readMac()
                    is Change.LocationWanted -> { pendingLocation = c.requestId; showPending() }
                    else -> refresh()
                }
            }
        }
        viewModelScope.launch {
            // PHN-615: the lock page that came with opening or returning asks by itself, once, when the window is the
            // owner's (resumed, focused: no system sheet or other prompt over it)
            combine(windowReady, session.promptDue, _ui.map { it.gate }.distinctUntilChanged()) { ready, due, gate ->
                ready && due && gate == Gate.LOCKED
            }.distinctUntilChanged().collect { if (it) autoUnlock() }
        }
        viewModelScope.launch {
            // the link came online while the app is open and unlocked: fetch what changed (one call, no timer)
            api.link.collect { l -> if (l?.phase == LinkPhase.ONLINE && l.paired && !session.locked.value) syncNow() }
        }
        viewModelScope.launch {
            // files cross the path again (PHN-444): what waited for it is tried once more, by itself
            api.link.map { it?.bulkAllowed == true && it.phase == LinkPhase.ONLINE }.distinctUntilChanged().collect { if (it) transfers.bulkAllowed() }
        }
        viewModelScope.launch {
            session.locked.collect { locked ->
                if (!locked) {
                    refresh(); saved["home"] = true
                    // a request for the exact location that came while the app was closed or locked
                    (api.locationWanted() as? Outcome.Ok)?.value?.let { pendingLocation = it; showPending() }
                } else {
                    // PHN-15: nothing private stays open; the preview stops, the fetched files go
                    AppLog.i("lock shown")
                    macJob?.cancel()
                    macPending = false
                    _ui.update { it.copy(macLine = null, needsMacUnlock = false) }
                    stopListening()
                    live.stop()
                    if (_sheet.value is Sheet.Live || _sheet.value is Sheet.Settings || _sheet.value is Sheet.Days || _sheet.value is Sheet.Location) _sheet.value = null
                    transfers.locked()
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
                } else {
                    transfers.dropKeyPlaceFiles(openedKeyFiles.toList())
                    openedKeyFiles.clear()
                    withContext(Dispatchers.IO) { local.dropOld(now()) }
                    markRead()
                    showPending()
                }
            }
        }
        readMac()
        refresh()
    }

    // ---- the gate -------------------------------------------------------------------------------------------------

    private fun gateOf(u: UiState, s: PhoneState, l: LinkStatus?, locked: Boolean): UiState {
        val wiped = (s as? PhoneState.Open)?.wiped
        val keys = s.keys
        val removed = l?.phase == LinkPhase.STOPPED && l.problem?.code in REMOVED_CODES
        val gate = when {
            s is PhoneState.Closed || s is PhoneState.Starting -> Gate.STARTING
            s is PhoneState.Failed -> Gate.FAILED
            !screenLock() -> Gate.NO_SCREEN_LOCK
            locked -> Gate.LOCKED
            l == null -> Gate.STARTING
            l.phase == LinkPhase.PAIRING_WAIT_FOR_MAC -> Gate.CODES
            removed -> Gate.REMOVED
            !l.paired -> Gate.PAIR
            keys?.pairAgain == true -> Gate.REMOVED
            else -> Gate.MAIN
        }
        val reason = when {
            removed -> PairReason.REMOVED
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
            show(Sheet.Asking(id))
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

    /** The transcript and the Mac's last status (each question's needs, Stop everything, PHN-125) as the screens draw them. */
    private fun apply(view: TranscriptView?) {
        lastView = view
        val status = api.lastStatus()
        val questions = Questions.merged(view?.let { Questions.open(it.questions) }.orEmpty(), status)
        val chatWork = WorkModel.of(view, "")
        if (prefs.lastOpen == 0L && prefs.seen.isEmpty()) prefs.seen = chatWork.earlier.mapNotNull { it.entry }.toSet()
        val seen = prefs.seen
        val away = chatWork.earlier.filter { it.entry !in seen }
        _ui.update {
            it.copy(conversation = view?.conversation, rows = ChatModel.rows(view), chatWork = chatWork, away = away,
                questions = questions, older = view?.older ?: OlderState.Unknown, status = status)
        }
        rebuildWork()
        // a question slides up over whatever is open, once (MobileScreens.swift QuestionSheet)
        val fresh = questions.firstOrNull { it.approvalId !in presented && !it.held }
        if (fresh != null && _ui.value.gate == Gate.MAIN && session.front.value && _sheet.value == null) {
            show(Sheet.Asking(fresh.approvalId))
        }
        val sheet = _sheet.value
        if (sheet is Sheet.Asking && questions.none { it.approvalId == sheet.approvalId }) {
            // answered on the Mac or elsewhere, or expired: the sheet goes and says so
            _sheet.value = null
            note(Note(R.string.question_gone))
        }
    }

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
                is Outcome.Ok -> _ui.update { it.copy(macTasks = r.value.tasks) }
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
        when (val r = api.sync()) {
            is Outcome.Failed -> if (r.error is FerbException.NeedsUnlock) macLocked()
            is Outcome.Ok -> { macPending = false; _ui.update { it.copy(needsMacUnlock = false) } }
        }
    }

    /** The chat on screen, the app open and unlocked: the Mac learns how far the owner has read (one mark, latest). */
    private fun markRead() {
        val conv = _ui.value.conversation ?: return
        if (_ui.value.gate != Gate.MAIN || session.locked.value || !session.front.value) return
        if (tab.value != PhoneTab.CHAT || home.value) return
        viewModelScope.launch { api.markReadLatest(conv) }
    }

    // ---- the app lock (PHN-8, PHN-13, PHN-125) ---------------------------------------------------------------------

    /**
     * One prompt opens the app and unlocks the Mac (PHN-13, PHN-641). With the Mac reachable the lock key signs the
     * Mac's challenge inside the prompt; the app opens the moment the prompt passes, and the Mac's answer (and the sync
     * the library makes after it) arrives in the background under a calm line. The prompt never waits on the network
     * longer than [PROMPT_WAIT_MS] (the link, then the Mac's challenge): past that, or on a second tap, the prompt alone
     * opens the app and the Mac is unlocked once it answers ([macLocked]).
     */
    fun unlockApp(auto: Boolean = false): Job {
        if ("unlock" in _ui.value.busy) {
            // a tap while the first try still waits for the Mac: the prompt now, never a silent no-op
            hurry?.complete(Unit)
            AppLog.i("unlock tap while waiting: prompt now")
            return unlockJob ?: Job()
        }
        return busy("unlock") {
            val p = prompts ?: return@busy
            session.takePrompt() // the owner's tap takes the due prompt too: after its cancel nothing asks by itself
            val started = now()
            val hurried = CompletableDeferred<Unit>().also { hurry = it }
            AppLog.i("unlock started auto=$auto link=${api.link.value?.phase}")
            if (api.link.value?.paired == true && api.link.value?.phase != LinkPhase.ONLINE) {
                withTimeoutOrNull(PROMPT_WAIT_MS) {
                    val up = async { api.link.first { it?.phase == LinkPhase.ONLINE } }
                    select<Unit> {
                        hurried.onAwait { }
                        up.onAwait { }
                    }
                    up.cancel()
                }
            }
            // the screen went elsewhere while the link was awaited: the prompt waits for the owner's return
            if (auto && !windowReady.value) { session.promptAgain(); AppLog.i("unlock: screen elsewhere, prompt later"); return@busy }
            val online = api.link.value?.let { it.paired && it.phase == LinkPhase.ONLINE } == true
            if (online && api.state.value.keys?.lock == KeyState.OK && !hurried.isCompleted) {
                val gate = PromptGate(p.signer(KeyKind.LOCK))
                val call = viewModelScope.async { api.unlock(gate) }
                val left = (PROMPT_WAIT_MS - (now() - started)).coerceAtLeast(0)
                withTimeoutOrNull(left) {
                    select<Unit> {
                        gate.asked.onAwait { }
                        call.onAwait { }
                        hurried.onAwait { }
                    }
                }
                if (gate.giveUp()) {
                    AppLog.i("unlock: the Mac's challenge didn't come in ${now() - started} ms; the prompt alone")
                } else {
                    AppLog.i("prompt shown (with the Mac's challenge) after ${now() - started} ms")
                    val shown = now()
                    val passed = gate.passed.await()
                    AppLog.i("prompt result ${if (passed) "passed" else "cancelled"} after ${now() - shown} ms")
                    if (!passed) return@busy
                    openApp()
                    AppLog.i("local unlock done")
                    macAnswer(call, now())
                    return@busy
                }
            }
            AppLog.i("prompt shown (alone)")
            val shown = now()
            val passed = p.local()
            AppLog.i("prompt result ${if (passed) "passed" else "cancelled"} after ${now() - shown} ms")
            if (passed) {
                openApp()
                api.unlocked()
                AppLog.i("local unlock done")
                if (api.link.value?.paired == true) {
                    AppLog.i("the Mac's unlock waits for the Mac")
                    viewModelScope.launch { _ui.first { "unlock" !in it.busy }; macLocked() }
                }
            }
        }.also { unlockJob = it }
    }

    private var hurry: CompletableDeferred<Unit>? = null
    private var unlockJob: Job? = null

    /** The app is open: the lock's state, the open's facts (PHN-13). */
    private fun openApp() {
        session.unlocked()
        opened()
        macTries = 0
    }

    /** The Mac's unlock that the opening prompt signed, answered in the background under a calm line (PHN-641). */
    private fun macAnswer(call: Deferred<Outcome<Unit>>, asked: Long) {
        macJob?.cancel()
        _ui.update { it.copy(macLine = MacLine.OPENING, needsMacUnlock = false) }
        macJob = viewModelScope.launch {
            val r = withTimeoutOrNull(MAC_SLOW_MS) { call.await() } ?: run {
                _ui.update { it.copy(macLine = MacLine.SLOW) }
                AppLog.i("mac unlock slow: no answer in $MAC_SLOW_MS ms")
                call.await()
            }
            macAnswered(r, asked)
        }
    }

    private suspend fun macAnswered(r: Outcome<Unit>, asked: Long) {
        when (r) {
            is Outcome.Ok -> {
                AppLog.i("mac unlock answered ok in ${now() - asked} ms")
                _ui.update { it.copy(macLine = null, needsMacUnlock = false) }
                refresh()
                afterMacUnlock()
            }
            is Outcome.Failed -> {
                AppLog.i("mac unlock failed ${r.error.linkCode ?: r.error.javaClass.simpleName} in ${now() - asked} ms")
                api.unlocked()
                _ui.update { it.copy(macLine = null) }
                when {
                    r.error is FerbException.NotSigned -> Unit
                    // the Mac didn't take this phone's proof: said plainly, with the one way that fixes it
                    r.error is FerbException.NeedsUnlock && r.error.linkCode in Unlocker.PROOF_CODES -> failed(r.error)
                    r.error is FerbException.NeedsSigner -> failed(r.error)
                    else -> macPending = true // can't be reached now: unlocked once it answers, by itself
                }
                if (macPending && api.link.value?.phase == LinkPhase.ONLINE) macLocked()
            }
        }
    }

    /** The Mac couldn't be unlocked yet (offline, or its answer was lost): it is, once the link is up again. */
    private var macPending = false
    private var macJob: Job? = null

    /** Automatic Mac unlocks since the app opened: one, never a loop (PHN-641). */
    private var macTries = 0

    /**
     * The Mac said its window is closed (a private call came back `locked`), or the app opened without it. First the
     * same read again ([RECHECKS] times, [RECHECK_MS] apart): right after a reconnect the phone proves itself on the new
     * connection and the window is open again. Still locked: one automatic unlock with the lock key (its one prompt) while the owner
     * is here; after that, or if he cancels, the line he can tap.
     */
    private fun macLocked() {
        if (macJob?.isActive == true || "unlock-mac" in _ui.value.busy || "unlock" in _ui.value.busy) return
        if (session.locked.value || api.link.value?.paired != true) return
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
                _ui.update { it.copy(needsMacUnlock = false) }
                refresh()
                return@launch
            }
            if (macTries >= 1 || !windowReady.value || prompts == null) {
                AppLog.i("mac locked: the owner's tap (tries=$macTries)")
                _ui.update { it.copy(needsMacUnlock = true) }
                return@launch
            }
            macTries++
            macPending = false
            AppLog.i("mac locked: one automatic unlock")
            unlockMacNow()
        }
    }

    /** The Mac's unlock alone (its window closed while the app stayed open): one prompt, the owner's tap. */
    fun unlockMac() = busy("unlock-mac") { unlockMacNow() }

    private suspend fun unlockMacNow() {
        val p = prompts ?: return
        val asked = now()
        _ui.update { it.copy(macLine = MacLine.OPENING, needsMacUnlock = false) }
        val gate = PromptGate(p.signer(KeyKind.LOCK))
        val call = viewModelScope.async { api.unlock(gate) }
        val r = withTimeoutOrNull(MAC_SLOW_MS) { call.await() } ?: run {
            _ui.update { it.copy(macLine = MacLine.SLOW) }
            call.await()
        }
        if (r is Outcome.Failed && r.error is FerbException.NotSigned) {
            AppLog.i("mac unlock prompt cancelled")
            _ui.update { it.copy(macLine = null, needsMacUnlock = true) }
            return
        }
        macAnswered(r, asked)
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
    fun registerKeys() = busy("keys") {
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
        if (text.isNotEmpty()) saved["home"] = false
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
        saved["home"] = false
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
        val text = draft.value.trim()
        val files = transfers.ready.value
        if (text.isEmpty() && files.isEmpty()) return@busy
        saved["draft"] = ""
        saved["home"] = false
        val r = if (files.isEmpty()) api.send(text) else api.sendWith(text, files)
        when (r) {
            is Outcome.Ok -> {
                if (files.isNotEmpty()) transfers.sent()
                if (r.value.waitingFor == "unlock") macLocked()
            }
            is Outcome.Failed -> { saved["draft"] = text; failed(r.error) }
        }
        refresh()
    }

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
    fun stopEverything() = busy("stop-all") {
        _sheet.value = null
        when (val r = api.stopEverything()) {
            is Outcome.Ok -> { note(Note(R.string.stop_all_done)); api.status(); apply(lastView) }
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
            Answered.Sent -> { if ((_sheet.value as? Sheet.Asking)?.approvalId == approvalId) _sheet.value = null; refresh() }
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
        if (staged.isNotEmpty()) transfers.send(staged, forChat)
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

    /** The Mac's window closed under the preview: unlock (one prompt) and watch again. */
    fun watchAgain() = busy("live") {
        val s = _sheet.value as? Sheet.Live ?: return@busy
        val p = prompts ?: return@busy
        if (live.view.value?.needsUnlock == true) {
            when (val r = api.unlock(p.signer(KeyKind.LOCK))) {
                is Outcome.Ok -> _ui.update { it.copy(needsMacUnlock = false) }
                is Outcome.Failed -> { if (r.error !is FerbException.NotSigned) failed(r.error); return@busy }
            }
        }
        live.start(s.taskId)
    }

    fun closeLive() {
        live.stop()
        if (_sheet.value is Sheet.Live) _sheet.value = null
    }

    // ---- FERB's settings (PHN-404, PHN-112, PHN-133) ----------------------------------------------------------------

    fun openSettings() = busy("settings") {
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
    fun setSetting(key: String, valueJson: String) = busy("setting:$key") {
        val spec = MacSettings.spec(key) ?: return@busy
        val kind = unlocker.freshKind() ?: run { note(Note(R.string.needs_pairing_again)); return@busy }
        val p = prompts ?: return@busy
        when (val r = unlocker.run { api.setSetting(key, valueJson, p.signer(kind, Why.SETTING)) }) {
            is Outcome.Ok -> {
                val row = MacSettings.row(spec, r.value)
                _ui.update { u -> u.copy(settings = u.settings?.map { if (it.spec.key == key) row else it }) }
                if (_sheet.value is Sheet.Days) _sheet.value = Sheet.Settings
            }
            is Outcome.Failed -> if (r.error !is FerbException.NotSigned) failed(r.error)
        }
    }

    fun setSwitch(key: String, on: Boolean) = setSetting(key, if (on) "true" else "false")

    fun setDays(key: String, text: String) {
        val days = text.trim().toIntOrNull()
        if (days == null || days < 1) { note(Note(R.string.set_days_invalid)); return }
        setSetting(key, days.toString())
    }

    // ---- navigation and sheets ------------------------------------------------------------------------------------

    fun choose(t: PhoneTab) {
        if (t == PhoneTab.CHAT && tab.value == PhoneTab.CHAT) saved["home"] = !home.value
        saved["tab"] = t
        when (t) {
            PhoneTab.TASKS -> loadTasks()
            PhoneTab.FILES -> filesOpened()
            PhoneTab.CHAT -> markRead()
            else -> Unit
        }
    }

    fun openChat() { saved["tab"] = PhoneTab.CHAT; saved["home"] = false; markRead() }

    fun show(s: Sheet?) {
        if (s is Sheet.Asking) { presented.add(s.approvalId); saved["presented"] = ArrayList(presented) }
        if (_sheet.value is Sheet.Live && s !is Sheet.Live) live.stop()
        _sheet.value = s
        if (s == null && pendingLocation != null) showPending()
    }

    fun note(n: Note?) = _ui.update { it.copy(note = n) }

    private fun readSettings() = PhoneSettings(prefs.stayConnected, prefs.blockScreenshots, prefs.hideChecksOnLockScreen, prefs.pairedSince, prefs.shareCity)

    /** The profile's switches changed outside the view model (the service, the screenshot switch): read them again. */
    fun refreshProfile() { _settings.value = readSettings() }

    fun hideChecks(on: Boolean) { prefs.hideChecksOnLockScreen = on; refreshProfile() }

    /** Forget this Mac (PHN-408): the fetched chat, the Mac, the keys and the files here go; the pairing screen comes. */
    fun forget() = busy("forget") {
        _sheet.value = null
        live.stop()
        when (val r = api.forget()) {
            is Outcome.Ok -> {
                prefs.forgetPairing(); saved["tab"] = PhoneTab.CHAT; saved["home"] = true; presented.clear()
                withContext(Dispatchers.IO) { transfers.clear() }
                _ui.update { it.copy(mac = null, macTasks = null, files = FilesUi(), settings = null, status = null) }
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

    private fun busy(what: String, block: suspend () -> Unit): Job = viewModelScope.launch {
        if (what in _ui.value.busy) return@launch
        _ui.update { it.copy(busy = it.busy + what) }
        try { block() } finally { _ui.update { it.copy(busy = it.busy - what) } }
    }

    /**
     * The opening prompt inside the Mac's unlock (PHN-641): says when the library asks for the signature (the Mac's
     * challenge came: the prompt shows) and whether the owner passed it, so the app opens then and not after the Mac's
     * answer. Given up before it was asked (the challenge was slow), it signs nothing and shows no prompt.
     */
    private class PromptGate(private val inner: Signer) : Signer {
        val asked = CompletableDeferred<Unit>()
        val passed = CompletableDeferred<Boolean>()
        private var state = 0 // 0 waiting, 1 asked, 2 given up

        override fun sign(payload: String): String? {
            synchronized(this) {
                if (state == 2) return null
                state = 1
            }
            asked.complete(Unit)
            val sig = runCatching { inner.sign(payload) }.getOrNull()
            passed.complete(sig != null)
            return sig
        }

        /** True when the prompt had not been asked and now never will be. */
        fun giveUp(): Boolean = synchronized(this) {
            if (state == 0) { state = 2; passed.complete(false); true } else false
        }
    }

    override fun onCleared() {
        dictation?.cancel()
        live.stop()
        super.onCleared()
    }

    companion object {
        /** The longest the opening prompt waits on the network (the link, then the Mac's challenge), PHN-641. */
        const val PROMPT_WAIT_MS = 2_000L

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

        /** How many recent files the Files tab asks for. */
        const val RECENT_FILES = 20

        /** The exact fix's bound: the Mac waits 60 s for the answer; the owner's yes and the fix share that. */
        const val EXACT_WAIT_MS = 20_000L

        /** The lab's three places (PhoneFilesPage), as the Mac's paths. */
        val PLACES = listOf("~/Desktop", "~/Documents", "~/Downloads")

        /** The Mac no longer knows this phone: pair again. */
        val REMOVED_CODES = setOf("device_revoked", "not_paired", "unknown_phone")

        fun errorNote(e: FerbException): Note = when {
            e.linkCode == Unlocker.PAIR_AGAIN -> Note(R.string.needs_pairing_again)
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
