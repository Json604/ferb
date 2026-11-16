package com.ferb.app

import androidx.annotation.StringRes
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.ferb.app.model.AnswerFlow
import com.ferb.app.model.Answered
import com.ferb.app.model.ChatModel
import com.ferb.app.model.ChatRow
import com.ferb.app.model.Corpus
import com.ferb.app.model.HomeWords
import com.ferb.app.model.Moment
import com.ferb.app.model.PhoneApi
import com.ferb.app.model.Question
import com.ferb.app.model.Questions
import com.ferb.app.model.Work
import com.ferb.app.model.WorkModel
import com.ferb.app.model.keys
import com.ferb.app.model.linkCode
import com.ferb.core.KeyHealth
import com.ferb.core.KeyKind
import com.ferb.core.detail
import com.ferb.core.Outcome
import com.ferb.core.PhoneState
import com.ferb.design.components.PhoneTab
import com.ferb.design.sky.SkyMoment
import com.ferb.design.sky.SkyPlace
import com.ferb.mobile.AnswerInput
import com.ferb.mobile.Change
import com.ferb.mobile.FerbException
import com.ferb.mobile.LinkPath
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.LinkStatus
import com.ferb.mobile.OlderState
import com.ferb.mobile.Signer
import com.ferb.mobile.TranscriptView
import java.time.ZoneId
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
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
    data class NotYet(val what: Feature) : Sheet
}

/** What the phone library does not carry yet (S11's FFI list): the screen says so plainly and fakes nothing. */
enum class Feature(@StringRes val words: Int) {
    FILES(R.string.feature_files), SEND_FILES(R.string.feature_send_files), WATCH_LIVE(R.string.feature_watch_live),
    STOP_ALL(R.string.feature_stop_all), SETTINGS(R.string.feature_settings), VOICE(R.string.feature_voice),
}

/** A short line for the owner, in the app's words (string resources) or the Mac's own. */
data class Note(@StringRes val res: Int, val words: String? = null)

/** This phone's own switches, as the profile sheet shows them. */
data class PhoneSettings(val stayConnected: Boolean, val blockScreenshots: Boolean, val hideChecks: Boolean, val pairedSince: Long)

/** The prompts only the screen can show: the system's fingerprint prompt, with a key or without one. */
interface Prompts {
    fun signer(kind: KeyKind): Signer

    /** The prompt alone (the app opens while the Mac can't be reached): true when the owner passed it. */
    suspend fun local(): Boolean
}

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
    val work: Work = Work(emptyList(), emptyList(), emptyList()),
    /** What finished or failed since the app was last open (the home screen's list and the corpus's counts). */
    val away: List<com.ferb.app.model.WorkItem> = emptyList(),
    val questions: List<Question> = emptyList(),
    val older: OlderState = OlderState.Unknown,
    val needsMacUnlock: Boolean = false,
    val busy: Set<String> = emptySet(),
    val note: Note? = null,
)

/**
 * The app's state over [PhoneApi]'s flows: the gate, the chat, tasks and questions, and every action, each run off the
 * main thread by the library. A screen turned or rebuilt keeps it; a new process reads it from the phone's store.
 */
class FerbViewModel(
    private val api: PhoneApi,
    private val session: Session,
    private val prefs: Prefs,
    private val screenLock: () -> Boolean,
    private val corpus: Corpus?,
    private val saved: SavedStateHandle,
    private val now: () -> Long = System::currentTimeMillis,
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
    private var refreshing: Job? = null
    private var dirty = false

    init {
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
        viewModelScope.launch { api.changes.collect { if (it !is Change.Closed) refresh() } }
        viewModelScope.launch {
            // the link came online while the app is open and unlocked: fetch what changed (one call, no timer)
            api.link.collect { l -> if (l?.phase == LinkPhase.ONLINE && l.paired && !session.locked.value) syncNow() }
        }
        viewModelScope.launch { session.locked.collect { if (!it) { refresh(); saved["home"] = true } } }
        viewModelScope.launch {
            // leaving the app: what finished so far has been seen (the next open lists only what is new)
            session.front.collect { front -> if (!front) markSeen() }
        }
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

    private fun showPending() {
        val id = pending ?: return
        if (_ui.value.gate != Gate.MAIN) return
        pending = null
        show(Sheet.Asking(id))
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
        }
    }

    private fun markSeen() {
        val done = _ui.value.work.earlier.mapNotNull { it.entry }
        if (done.isNotEmpty()) prefs.seen = prefs.seen + done
    }

    private fun apply(view: TranscriptView?) {
        val questions = view?.let { Questions.open(it.questions) }.orEmpty()
        val work = WorkModel.of(view, "")
        if (prefs.lastOpen == 0L && prefs.seen.isEmpty()) prefs.seen = work.earlier.mapNotNull { it.entry }.toSet()
        val seen = prefs.seen
        val away = work.earlier.filter { it.entry !in seen }
        _ui.update {
            it.copy(conversation = view?.conversation, rows = ChatModel.rows(view), work = work, away = away,
                questions = questions, older = view?.older ?: OlderState.Unknown,
                needsMacUnlock = it.needsMacUnlock || view?.outbox.orEmpty().any { o -> o.waitingFor == "unlock" })
        }
        // a question slides up over whatever is open, once (MobileScreens.swift QuestionSheet)
        val fresh = questions.firstOrNull { it.approvalId !in presented }
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

    private suspend fun syncNow() {
        when (val r = api.sync()) {
            is Outcome.Failed -> if (r.error is FerbException.NeedsUnlock) _ui.update { it.copy(needsMacUnlock = true) }
            is Outcome.Ok -> _ui.update { it.copy(needsMacUnlock = false) }
        }
    }

    // ---- the app lock (PHN-8, PHN-13, PHN-125) ---------------------------------------------------------------------

    /**
     * One prompt opens the app and unlocks the Mac (PHN-13): with the Mac reachable the lock key signs the Mac's
     * challenge inside the prompt; otherwise the prompt alone opens the app and the Mac is unlocked on the next
     * action that needs it. A cold start waits a few seconds for the link so it stays one prompt.
     */
    fun unlockApp() = busy("unlock") {
        val p = prompts ?: return@busy
        if (api.link.value?.paired == true && api.link.value?.phase != LinkPhase.ONLINE) {
            withTimeoutOrNull(COLD_START_WAIT_MS) { api.link.first { it?.phase == LinkPhase.ONLINE } }
        }
        val online = api.link.value?.let { it.paired && it.phase == LinkPhase.ONLINE } == true
        if (online && api.state.value.keys?.lock == com.ferb.core.KeyState.OK) {
            val signed = Recording(p.signer(KeyKind.LOCK))
            val r = api.unlock(signed)
            if (signed.signed) {
                session.unlocked()
                opened()
                _ui.update { it.copy(needsMacUnlock = r is Outcome.Failed) }
                return@busy
            }
            if (r is Outcome.Failed && r.error is FerbException.NotSigned) return@busy // cancelled
        }
        if (p.local()) {
            session.unlocked()
            opened()
            if (api.link.value?.paired == true) _ui.update { it.copy(needsMacUnlock = true) }
        }
    }

    /** The Mac's unlock alone (its window closed while the app stayed open). */
    fun unlockMac() = busy("unlock-mac") {
        val p = prompts ?: return@busy
        when (val r = api.unlock(p.signer(KeyKind.LOCK))) {
            is Outcome.Ok -> { _ui.update { it.copy(needsMacUnlock = false) }; refresh() }
            is Outcome.Failed -> if (r.error !is FerbException.NotSigned) failed(r.error)
        }
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

    fun typed(text: String) { saved["draft"] = text; if (text.isNotEmpty()) saved["home"] = false }

    /** Sends through the outbox (PHN-401): with the Mac off it waits there, visible and cancellable. */
    fun send() = busy("send") {
        val text = draft.value.trim()
        if (text.isEmpty()) return@busy
        saved["draft"] = ""
        saved["home"] = false
        when (val r = api.send(text)) {
            is Outcome.Ok -> if (r.value.waitingFor == "unlock") _ui.update { it.copy(needsMacUnlock = true) }
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

    fun loadOlder() = busy("older") {
        val conv = _ui.value.conversation ?: return@busy
        if (_ui.value.older !is OlderState.Available) return@busy
        when (val r = api.loadOlder(conv)) {
            is Outcome.Failed -> if (r.error is FerbException.NeedsUnlock) _ui.update { it.copy(needsMacUnlock = true) }
            is Outcome.Ok -> Unit
        }
        refresh()
    }

    // ---- questions (PHN-415, PHN-417, PHN-470) ----------------------------------------------------------------------

    fun answer(approvalId: String, input: AnswerInput, hidden: Boolean = false) = busy("answer:$approvalId") {
        val p = prompts ?: return@busy
        val flow = AnswerFlow(api, { api.state.value.keys }, p::signer)
        when (val r = flow.answer(approvalId, input, hidden)) {
            Answered.Sent -> { if ((_sheet.value as? Sheet.Asking)?.approvalId == approvalId) _sheet.value = null; refresh() }
            Answered.NotSent -> Unit
            Answered.NeedsPairingAgain -> note(Note(R.string.answer_needs_pairing))
            is Answered.Failed -> failed(r.error)
        }
    }

    // ---- navigation and sheets ------------------------------------------------------------------------------------

    fun choose(t: PhoneTab) {
        if (t == PhoneTab.CHAT && tab.value == PhoneTab.CHAT) saved["home"] = !home.value
        saved["tab"] = t
    }

    fun openChat() { saved["tab"] = PhoneTab.CHAT; saved["home"] = false }

    fun show(s: Sheet?) {
        if (s is Sheet.Asking) { presented.add(s.approvalId); saved["presented"] = ArrayList(presented) }
        _sheet.value = s
    }

    fun note(n: Note?) = _ui.update { it.copy(note = n) }

    private fun readSettings() = PhoneSettings(prefs.stayConnected, prefs.blockScreenshots, prefs.hideChecksOnLockScreen, prefs.pairedSince)

    /** The profile's switches changed outside the view model (the service, the screenshot switch): read them again. */
    fun refreshProfile() { _settings.value = readSettings() }

    fun hideChecks(on: Boolean) { prefs.hideChecksOnLockScreen = on; refreshProfile() }

    /** Forget this Mac (PHN-408): the fetched chat, the Mac and the keys go; the pairing screen comes. */
    fun forget() = busy("forget") {
        _sheet.value = null
        when (val r = api.forget()) {
            is Outcome.Ok -> { prefs.forgetPairing(); saved["tab"] = PhoneTab.CHAT; saved["home"] = true; presented.clear() }
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
        val m = Moment(sky, u.work.waiting.size, u.away.count { it.kind == com.ferb.design.components.MarkKind.DONE },
            u.away.count { it.kind == com.ferb.design.components.MarkKind.FAILED }, u.work.running.size, link,
            firstRun = u.link?.paired != true, justPaired = prefs.justPaired, lastOpen = previousOpen, lastZone = previousZone)
        return c.texts(m)
    }

    fun skyPlace(): SkyPlace = SkyPlace.fromZone(ZoneId.systemDefault())

    // ---- helpers ---------------------------------------------------------------------------------------------------

    private fun failed(e: FerbException) = note(errorNote(e))

    private fun busy(what: String, block: suspend () -> Unit): Job = viewModelScope.launch {
        if (what in _ui.value.busy) return@launch
        _ui.update { it.copy(busy = it.busy + what) }
        try { block() } finally { _ui.update { it.copy(busy = it.busy - what) } }
    }

    /** Records whether the prompt signed: the app opens on the owner's fingerprint even if the Mac then says no. */
    private class Recording(private val inner: Signer) : Signer {
        @Volatile var signed = false
        override fun sign(payload: String): String? = inner.sign(payload)?.also { signed = true }
    }

    companion object {
        const val COLD_START_WAIT_MS = 6_000L

        /** The Mac no longer knows this phone: pair again. */
        val REMOVED_CODES = setOf("device_revoked", "not_paired", "unknown_phone")

        fun errorNote(e: FerbException): Note = when (e) {
            is FerbException.Offline -> Note(R.string.err_offline)
            is FerbException.NeedsBetterPath -> Note(R.string.err_needs_better_path)
            is FerbException.NeedsUnlock -> Note(R.string.err_needs_unlock)
            is FerbException.Refused, is FerbException.NotFound -> Note(R.string.err_mac_said, e.detail)
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
