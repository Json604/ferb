package com.ferb.app

import com.ferb.app.model.Live
import com.ferb.app.model.PhoneApi
import com.ferb.core.KeyHealth
import com.ferb.core.KeyKind
import com.ferb.core.KeyState
import com.ferb.core.Outcome
import com.ferb.core.PhoneState
import com.ferb.core.PublicKeys
import com.ferb.mobile.AnswerInput
import com.ferb.mobile.Attachment
import com.ferb.mobile.Cancelled
import com.ferb.mobile.Change
import com.ferb.mobile.DeviceReport
import com.ferb.mobile.FerbException
import com.ferb.mobile.Fetched
import com.ferb.mobile.Folder
import com.ferb.mobile.FoundFiles
import com.ferb.mobile.LocationFix
import com.ferb.mobile.LinkPath
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.LinkStatus
import com.ferb.mobile.MacInfo
import com.ferb.mobile.OlderState
import com.ferb.mobile.OutboxItem
import com.ferb.mobile.PhoneStatus
import com.ferb.mobile.PreviewSink
import com.ferb.mobile.RecentFiles
import com.ferb.mobile.SecretInput
import com.ferb.mobile.SettingValue
import com.ferb.mobile.Signer
import com.ferb.mobile.TaskPage
import com.ferb.mobile.TransferListener
import com.ferb.mobile.TranscriptView
import com.ferb.mobile.Tree
import com.ferb.mobile.Uploaded
import java.io.File
import java.lang.ref.WeakReference
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow

/** A phone for the view models' tests: flows the test sets, calls it records, answers it scripts. No library. */
class FakePhone : PhoneApi {
    override val state = MutableStateFlow<PhoneState>(PhoneState.Open(KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK)))
    override val link = MutableStateFlow<LinkStatus?>(Companion.status())
    override val changes = MutableSharedFlow<Change>(extraBufferCapacity = 16)

    var view: TranscriptView? = null
    val outbox = ArrayList<OutboxItem>()
    val calls = ArrayList<String>()
    val answers = ArrayList<Pair<AnswerInput, Signer?>>()

    /** What each answer call says, in turn (then Ok). */
    val answerScript = ArrayDeque<Outcome<String>>()
    var unlockResult: Outcome<Unit> = Outcome.Ok(Unit)
    var stopResult: Outcome<Unit> = Outcome.Ok(Unit)
    var sendWaitingFor: String? = null

    var status: PhoneStatus? = null
    val folders = HashMap<String, Folder>()
    val filesScript = ArrayDeque<Outcome<Folder>>()
    val signed = ArrayList<String>()

    /** A fetch writes [fetchBytes] (all at once) unless [fetchGate] holds it, then it is cancelled or let go. */
    var fetchBytes = ByteArray(10) { it.toByte() }
    var fetchGate: CompletableDeferred<Unit>? = null
    var fetchRefusal: Outcome.Failed? = null
    val cancelled = ArrayList<String>()
    var uploadRefusal: Outcome.Failed? = null
    /** An upload waits here half way until it completes (a cancel lets it go, refused as stopped). */
    var uploadGate: CompletableDeferred<Unit>? = null
    val sentWith = ArrayList<Pair<String, List<Attachment>>>()
    var preview: PreviewSink? = null
    var previewStopped = 0
    val settingsValues = LinkedHashMap<String, String>()
    var settingsRefusal: Outcome.Failed? = null
    var tasks: Outcome<TaskPage> = Outcome.Ok(TaskPage(emptyList(), false))
    var mac: MacInfo? = null
    var newKeys: PublicKeys? = null

    /** A hidden answer: how many units the library's input held when it arrived, and only a weak hold on it. */
    val hiddenUnits = ArrayList<Int>()
    var hiddenSecret: WeakReference<SecretInput>? = null
    val hiddenScript = ArrayDeque<Outcome<String>>()
    val uploads = ArrayList<Triple<String, String?, String?>>()
    val trees = HashMap<String, Tree>()
    var groups = 0
    val taskFiles = HashMap<Pair<String, Long>, ByteArray>()
    var recent: Outcome<RecentFiles> = Outcome.Ok(RecentFiles(emptyList(), 0u))
    var found: Outcome<FoundFiles>? = null
    val reports = ArrayList<DeviceReport>()
    var wanted: String? = null
    val located = ArrayList<Triple<String, LocationFix?, String?>>()

    override suspend fun transcript(): Outcome<TranscriptView?> = Outcome.Ok(view?.copy(outbox = outbox.toList()))
    /** What each sync says, in turn (then Ok). */
    val syncScript = ArrayDeque<Outcome<Unit>>()
    override suspend fun sync(): Outcome<Unit> { calls += "sync"; return syncScript.removeFirstOrNull() ?: Outcome.Ok(Unit) }
    override suspend fun send(text: String): Outcome<OutboxItem> {
        calls += "send:$text"
        val item = OutboxItem("o${outbox.size + 1}", text, "2026-01-02T10:00:00+00:00", "waiting", sendWaitingFor ?: "link", null, false, emptyList())
        outbox += item
        return Outcome.Ok(item)
    }
    override suspend fun cancel(outboxId: String): Outcome<Cancelled> { calls += "cancel:$outboxId"; outbox.removeAll { it.id == outboxId }; return Outcome.Ok(Cancelled(false)) }
    override suspend fun answer(approvalId: String, input: AnswerInput, signer: Signer?): Outcome<String> {
        calls += "answer:$approvalId:${signer != null}"
        answers += input to signer
        signer?.sign("payload")
        return answerScript.removeFirstOrNull() ?: Outcome.Ok("{}")
    }
    override suspend fun answerHidden(approvalId: String, secret: SecretInput, signer: Signer): Outcome<String> {
        calls += "hidden:$approvalId"
        hiddenUnits += secret.len().toInt()
        hiddenSecret = WeakReference(secret)
        val sig = signer.sign("hidden")
        // the library empties and overwrites its copy whatever happens (mobile.rs answer_hidden)
        secret.clear()
        if (sig == null) return Outcome.Failed(FerbException.NotSigned("Nothing was sent.", null))
        return hiddenScript.removeFirstOrNull() ?: Outcome.Ok("{}")
    }
    override suspend fun stop(taskIds: List<String>): Outcome<Unit> { calls += "stop:${taskIds.joinToString()}"; return stopResult }
    override suspend fun loadOlder(conversation: String): Outcome<OlderState> { calls += "older"; return Outcome.Ok(OlderState.Complete) }
    /** Holds the Mac's challenge (before the prompt) or its answer (after it) until the test lets it go (PHN-641). */
    var challengeGate: CompletableDeferred<Unit>? = null
    var answerGate: CompletableDeferred<Unit>? = null
    override suspend fun unlock(signer: Signer): Outcome<Unit> {
        calls += "unlock"
        challengeGate?.await()
        val r = unlockResult
        if (r is Outcome.Ok && signer.sign("challenge") == null) return Outcome.Failed(FerbException.NotSigned("Nothing was sent.", null))
        answerGate?.await()
        return r
    }
    override suspend fun pair(code: String): Outcome<Unit> { calls += "pair:$code"; return Outcome.Ok(Unit) }
    var macForgot = true
    override suspend fun forget(): Outcome<Boolean> { calls += "forget"; return Outcome.Ok(macForgot) }

    /** PHN-649: the statement path; [statementResults] answer in turn (then Ok); what was sent, with its age. */
    var statementOk = true
    val statementResults = ArrayDeque<Outcome<Unit>>()
    val statements = ArrayList<Pair<String, Long>>()
    override suspend fun statementReady() = statementOk && link.value?.paired != false
    override suspend fun unlockWithStatement(payload: String, sig: String, ageMs: Long): Outcome<Unit> {
        calls += "statement"
        statements += payload to ageMs
        return statementResults.removeFirstOrNull() ?: Outcome.Ok(Unit)
    }
    val opens = ArrayList<Boolean>()
    override fun setOpen(on: Boolean) { opens += on }
    override suspend fun lock(): Outcome<Boolean> { calls += "lock"; return Outcome.Ok(true) }
    override suspend fun unlocked(): Outcome<Unit> { calls += "unlocked"; return Outcome.Ok(Unit) }
    override suspend fun stopEverything(): Outcome<Unit> { calls += "stop-everything"; return Outcome.Ok(Unit) }
    /** "Until I'm back" (PHN-611): what the Mac says about screen use, and what "Ask me again" gets. */
    var screenUse = com.ferb.mobile.ScreenUse(false, null, null, null, true)
    override suspend fun screenUse(): Outcome<com.ferb.mobile.ScreenUse> { calls += "screen-use"; return Outcome.Ok(screenUse) }
    override suspend fun endScreenUse(): Outcome<com.ferb.mobile.ScreenUseEnd> {
        calls += "end-screen-use"
        val was = screenUse.active
        screenUse = screenUse.copy(active = false, since = null, phone = null, phoneName = null)
        return Outcome.Ok(com.ferb.mobile.ScreenUseEnd(was, "phone-1", screenUse))
    }
    /** Insomnia mode (APP-980..985): what the Mac says, what each call gets, the camera's sink and its stops. */
    var insomnia = com.ferb.mobile.Insomnia(false, null, null, null, false, null, 0u, null, false, false,
        com.ferb.mobile.InsomniaCamera(false, "unknown"), com.ferb.mobile.InsomniaPower(null, null, null, null),
        emptyList(), null, "", false, null)
    var insomniaRefusal: Outcome.Failed? = null
    var camera: com.ferb.mobile.CameraSink? = null
    var cameraStopped = 0
    var cameraRefusal: Outcome.Failed? = null
    private fun withInsomnia(i: com.ferb.mobile.Insomnia) { insomnia = i; status = status?.copy(insomnia = i) }
    override suspend fun insomniaStatus(): Outcome<com.ferb.mobile.Insomnia> { calls += "insomnia"; return Outcome.Ok(insomnia) }
    override suspend fun startInsomnia(variant: String, screenYes: Boolean, signer: Signer): Outcome<com.ferb.mobile.InsomniaStarted> {
        calls += "insomnia-start:$variant:$screenYes"
        val sig = signer.sign("insomnia.start") ?: return Outcome.Failed(FerbException.NotSigned("Nothing was sent.", null))
        signed += sig
        insomniaRefusal?.let { insomniaRefusal = null; return it }
        val already = insomnia.on
        if (!already) withInsomnia(insomnia.copy(on = true, variant = variant, screenYes = screenYes, by = "phone-1",
            line = "Keeping your Mac working · 1 task running · battery 64%"))
        return Outcome.Ok(com.ferb.mobile.InsomniaStarted(already, insomnia))
    }
    override suspend fun endInsomnia(): Outcome<com.ferb.mobile.InsomniaEnded> {
        calls += "insomnia-end"
        val was = insomnia.on
        withInsomnia(insomnia.copy(on = false, variant = null, notice = null, warnings = emptyList(), line = ""))
        return Outcome.Ok(com.ferb.mobile.InsomniaEnded(was, "phone-1", insomnia))
    }
    override suspend fun lockMac(): Outcome<com.ferb.mobile.MacLocked> {
        calls += "lock-mac"
        insomniaRefusal?.let { insomniaRefusal = null; return it }
        withInsomnia(insomnia.copy(macLocked = true, notice = null))
        return Outcome.Ok(com.ferb.mobile.MacLocked(true, "Your Mac is locked.", insomnia))
    }
    override suspend fun dismissInsomniaNotice(noticeId: String): Outcome<com.ferb.mobile.Insomnia> {
        calls += "insomnia-fine:$noticeId"
        withInsomnia(insomnia.copy(notice = null))
        return Outcome.Ok(insomnia)
    }
    override suspend fun watchCamera(sink: com.ferb.mobile.CameraSink): Outcome<Live> {
        calls += "camera"
        cameraRefusal?.let { return it }
        camera = sink
        return Outcome.Ok(Live { cameraStopped++ })
    }
    override suspend fun status(): Outcome<PhoneStatus> { calls += "status"; return status?.let { Outcome.Ok(it) } ?: Outcome.Failed(FerbException.Offline("no", null)) }
    override suspend fun lastStatus(): PhoneStatus? = status
    override suspend fun markReadLatest(conversation: String): Outcome<Unit> { calls += "mark:$conversation"; return Outcome.Ok(Unit) }
    override suspend fun sendWith(text: String, attachments: List<Attachment>): Outcome<OutboxItem> {
        sentWith += text to attachments
        val item = OutboxItem("o${outbox.size + 1}", text, "2026-01-02T10:00:00+00:00", "waiting", "link", null, false, attachments)
        outbox += item
        return Outcome.Ok(item)
    }
    override suspend fun files(path: String?, signer: Signer?, after: String?): Outcome<Folder> {
        if (after != null) {
            calls += "files-more:$path:$after:${signer != null}"
            return morePages.removeFirstOrNull()?.let { Outcome.Ok(it) } ?: Outcome.Failed(FerbException.NotFound("No more.", null))
        }
        calls += "files:$path:${signer != null}"
        signer?.sign("files:$path")?.let(signed::add)
        filesScript.removeFirstOrNull()?.let { return it }
        return folders[path]?.let { Outcome.Ok(it) } ?: Outcome.Failed(FerbException.NotFound("No such folder.", null))
    }
    override suspend fun fetch(id: String, path: String, saveTo: String, offset: Long, signer: Signer?, listener: TransferListener): Outcome<Fetched> {
        calls += "fetch:$path@$offset:${signer != null}"
        signer?.sign("get:$path")?.let(signed::add)
        fetchRefusal?.let { return it }
        val f = File(saveTo)
        f.parentFile?.mkdirs()
        val half = fetchBytes.size / 2
        if (offset == 0L) f.writeBytes(fetchBytes.copyOfRange(0, half))
        listener.progress(half.toULong(), fetchBytes.size.toULong())
        fetchGate?.let { g ->
            g.await()
            if (id in cancelled) return Outcome.Failed(FerbException.Cancelled("Stopped.", "cancelled"))
        }
        f.writeBytes(fetchBytes)
        listener.progress(fetchBytes.size.toULong(), fetchBytes.size.toULong())
        return Outcome.Ok(Fetched(path.substringAfterLast('/'), saveTo, (fetchBytes.size - offset).toULong(), fetchBytes.size.toULong()))
    }
    override suspend fun upload(id: String, localPath: String, name: String, resume: String?, group: String?, rel: String?, listener: TransferListener): Outcome<Uploaded> {
        calls += "upload:$name:${resume ?: "-"}"
        uploads += Triple(name, group, rel)
        uploadRefusal?.let { return it }
        val size = File(localPath).length()
        listener.accepted(resume ?: "up-$name", 0u)
        // held half way while [uploadGate] waits: stopped there when it was cancelled meanwhile
        uploadGate?.let { g ->
            listener.progress((size / 2).toULong(), size.toULong())
            g.await()
            if (id in cancelled) return Outcome.Failed(FerbException.Cancelled("Stopped.", "cancelled"))
        }
        listener.progress(size.toULong(), size.toULong())
        return Outcome.Ok(Uploaded(resume ?: "up-$name", "~/Downloads/FERB/$name", size.toULong(), null))
    }
    override suspend fun cancelTransfer(id: String): Outcome<Unit> { cancelled += id; fetchGate?.complete(Unit); uploadGate?.complete(Unit); return Outcome.Ok(Unit) }
    override suspend fun tree(path: String, signer: Signer?): Outcome<Tree> {
        calls += "tree:$path:${signer != null}"
        return trees[path]?.let { Outcome.Ok(it) } ?: Outcome.Failed(FerbException.NotFound("No such folder.", null))
    }
    override suspend fun putGroup(name: String): Outcome<String> { calls += "group:$name"; groups++; return Outcome.Ok("g-$groups") }
    override suspend fun fetchTaskFile(id: String, taskId: String, index: Long, saveTo: String, listener: TransferListener): Outcome<Fetched> {
        calls += "task-file:$taskId:$index"
        val bytes = taskFiles[taskId to index] ?: return Outcome.Failed(FerbException.NotFound("FERB has no such file.", "not_found"))
        File(saveTo).apply { parentFile?.mkdirs(); writeBytes(bytes) }
        listener.progress(bytes.size.toULong(), bytes.size.toULong())
        return Outcome.Ok(Fetched("f", saveTo, bytes.size.toULong(), bytes.size.toULong()))
    }
    override suspend fun recentFiles(limit: Int): Outcome<RecentFiles> { calls += "recent:$limit"; return recent }
    override suspend fun searchFiles(query: String, path: String?, after: String?): Outcome<FoundFiles> {
        if (after != null) {
            calls += "search-more:$query:$after"
            return moreFound.removeFirstOrNull()?.let { Outcome.Ok(it) } ?: Outcome.Failed(FerbException.NotFound("No more.", null))
        }
        calls += "search:$query:${path ?: "~"}"
        return found ?: Outcome.Ok(FoundFiles(path ?: "~", query, emptyList(), 0u, 0u, false, false, 0u, null))
    }
    /** Paging (PHN-606): the pages "Load more" gets in turn (each asked with the page before's `next`). */
    val morePages = ArrayDeque<Folder>()
    val moreFound = ArrayDeque<FoundFiles>()

    override suspend fun reportDevice(report: DeviceReport): Outcome<Boolean> { reports += report; return Outcome.Ok(true) }
    override suspend fun locationWanted(): Outcome<String?> = Outcome.Ok(wanted)
    override suspend fun answerLocation(requestId: String, fix: LocationFix?, whyNot: String?): Outcome<Unit> {
        located += Triple(requestId, fix, whyNot)
        if (wanted == requestId) wanted = null
        return Outcome.Ok(Unit)
    }
    override suspend fun watchLive(taskId: String, sink: PreviewSink): Outcome<Live> {
        calls += "live:$taskId"
        preview = sink
        return Outcome.Ok(Live { previewStopped++ })
    }
    /** The work row's pictures asked for, by task, and their sinks (kb PHN-725). */
    val thumbnails = LinkedHashMap<String, PreviewSink>()
    var thumbnailsStopped = 0
    override suspend fun watchStepThumbnail(taskId: String, worker: String, everyMs: Long, sink: PreviewSink): Outcome<Live> {
        calls += "step-thumbnail:$taskId:$worker:$everyMs"
        thumbnails[com.ferb.app.model.Thumbnails.key(taskId, worker)] = sink
        return Outcome.Ok(Live { thumbnailsStopped++ })
    }
    override suspend fun watchThumbnail(taskId: String, everyMs: Long, sink: PreviewSink): Outcome<Live> {
        calls += "thumbnail:$taskId:$everyMs"
        thumbnails[taskId] = sink
        return Outcome.Ok(Live { thumbnailsStopped++ })
    }
    override suspend fun settings(): Outcome<List<SettingValue>> {
        calls += "settings"
        settingsRefusal?.let { settingsRefusal = null; return it }
        return Outcome.Ok(settingsValues.map { (k, v) -> SettingValue(k, v) })
    }
    override suspend fun setSetting(key: String, valueJson: String, signer: Signer): Outcome<String> {
        calls += "set:$key=$valueJson"
        val sig = signer.sign("settings.set") ?: return Outcome.Failed(FerbException.NotSigned("Nothing was sent.", null))
        signed += sig
        settingsValues[key] = valueJson
        return Outcome.Ok(valueJson)
    }
    override suspend fun tasks(limit: Int, before: String?): Outcome<TaskPage> { calls += "tasks"; return tasks }
    /** The alerts put away through [dismissAlert], in order. */
    val dismissed = mutableListOf<String>()
    override suspend fun dismissAlert(id: String): Outcome<Unit> { calls += "dismissAlert"; dismissed += id; return Outcome.Ok(Unit) }
    override suspend fun macInfo(): Outcome<MacInfo> { calls += "mac"; return mac?.let { Outcome.Ok(it) } ?: Outcome.Failed(FerbException.Offline("no", null)) }
    override suspend fun cachedMacInfo(): Outcome<MacInfo?> = Outcome.Ok(mac)
    override suspend fun unregisteredKeys(): PublicKeys? = newKeys
    override suspend fun registerKeys(signer: (KeyKind) -> Signer): Outcome<Boolean> {
        val k = newKeys ?: return Outcome.Ok(false)
        signer(KeyKind.LOCK).sign("link.keys")?.let(signed::add) ?: return Outcome.Failed(FerbException.NotSigned("Nothing was sent.", null))
        calls += "keys:${k.strong}"
        newKeys = null
        return Outcome.Ok(true)
    }

    // ---- the owner's coding sessions (kb HAND-651..666): what the Mac says, what each call got, the refusals it scripts ----
    var sessionsEnabled = true
    val sessions = ArrayList<com.ferb.mobile.AgentSession>()
    /** HAND-697: the chat apps the Mac says are open with no chat. */
    var noChats = listOf<String>()
    var sessionsRefusal: Outcome.Failed? = null
    /** Each session's items on the Mac, oldest first, and its open cards. */
    val sessionItems = HashMap<String, MutableList<com.ferb.mobile.SessionItem>>()
    val sessionCards = HashMap<String, MutableList<com.ferb.mobile.SessionCard>>()
    /** Sessions this phone controls (its sends go without asking). */
    val controlled = HashSet<String>()
    var delivered = true
    /** The Mac's refusal of a send or a control with words (the foreground proof failing, PHN-800); null: delivered. */
    var sendRefusal: Outcome.Failed? = null
    var interruptRefusal: Outcome.Failed? = null
    /** While set, a send or a control waits here as FERB typing into the terminal would (PHN-812), then answers. */
    var sendGate: CompletableDeferred<Unit>? = null
    /** While set, a session's read waits here (the Mac slow to answer, PHN-816); [timelineRefusal] answers it with a refusal. */
    var timelineGate: CompletableDeferred<Unit>? = null
    var timelineRefusal: Outcome.Failed? = null
    var startFolders = listOf("~/code/web-app", "~/code/blog")
    val timelineAsks = ArrayList<Triple<String, Long, Long>>()

    private fun yes(text: String) = com.ferb.mobile.SessionYes("Send this to Claude Code in Terminal?", text,
        "Yes lets you send to this session from here until you stop controlling it.")

    override suspend fun sessionsList(): Outcome<com.ferb.mobile.SessionsList> {
        calls += "sessions"
        sessionsRefusal?.let { return it }
        return Outcome.Ok(com.ferb.mobile.SessionsList(sessionsEnabled, if (sessionsEnabled) sessions.map { it.copy(controlledHere = it.id in controlled) } else emptyList(),
            if (sessionsEnabled) noChats else emptyList()))
    }
    override suspend fun sessionTimeline(session: String, after: Long, before: Long, limit: Int): Outcome<com.ferb.mobile.SessionTimeline> {
        calls += "timeline:$session:$after:$before"
        timelineAsks += Triple(session, after, before)
        timelineGate?.await()
        timelineRefusal?.let { return it }
        val all = sessionItems[session].orEmpty()
        val (page, more) = when {
            after > 0 -> all.filter { it.seq.toLong() > after }.let { a -> a.take(limit) to (a.size > limit) }
            before > 0 -> all.filter { it.seq.toLong() < before }.let { b -> b.takeLast(limit) to (b.size > limit) }
            else -> all.takeLast(limit) to (all.size > limit)
        }
        val s = sessions.firstOrNull { it.id == session }?.copy(controlledHere = session in controlled)
        return Outcome.Ok(com.ferb.mobile.SessionTimeline(s, page, more, sessionCards[session].orEmpty().toList()))
    }
    override suspend fun sessionSend(session: String, text: String): Outcome<com.ferb.mobile.SessionSent> {
        calls += "session-send:$session:$text"
        if (session !in controlled) return Outcome.Ok(com.ferb.mobile.SessionSent(yes(text), false, false, ""))
        if (text.isBlank()) return Outcome.Failed(FerbException.Refused("There's nothing to send.", "ferb_error"))
        sendGate?.await()
        sendRefusal?.let { return it }
        return Outcome.Ok(com.ferb.mobile.SessionSent(null, true, delivered, "agent_api"))
    }
    override suspend fun sessionControl(session: String, text: String, terminal: String, signer: Signer): Outcome<com.ferb.mobile.SessionSent> {
        calls += if (terminal.isEmpty()) "session-control:$session:$text" else "session-control:$session:$text:terminal=$terminal"
        val sig = signer.sign("sessions.control") ?: return Outcome.Failed(FerbException.NotSigned("Nothing was sent.", null))
        signed += sig
        controlled += session
        sendGate?.await()
        // HAND-683: until he chooses a terminal the session offers, the Mac can't tell which is the session's
        if (text.isNotBlank() && terminal.isEmpty()) sendRefusal?.let { return it }
        return Outcome.Ok(com.ferb.mobile.SessionSent(null, text.isNotBlank(), text.isNotBlank() && delivered, "agent_api"))
    }
    override suspend fun sessionRelease(session: String): Outcome<Int> {
        calls += "session-release:$session"
        return Outcome.Ok(if (controlled.remove(session)) 1 else 0)
    }
    override suspend fun sessionAnswer(session: String, card: String, choice: String, signer: Signer): Outcome<Boolean> {
        calls += "session-answer:$session:$card:$choice"
        val sig = signer.sign("sessions.answer") ?: return Outcome.Failed(FerbException.NotSigned("Nothing was sent.", null))
        signed += sig
        sessionCards[session]?.removeAll { it.id == card }
        return Outcome.Ok(true)
    }
    override suspend fun sessionInterrupt(session: String): Outcome<Boolean> {
        calls += "session-interrupt:$session"
        interruptRefusal?.let { return it }
        if (session !in controlled) return Outcome.Failed(FerbException.Refused("Say Yes to sending to this session first.", "ferb_error"))
        return Outcome.Ok(true)
    }
    override suspend fun sessionPrepare(agent: String, folder: String, text: String): Outcome<com.ferb.mobile.SessionStartPlan> {
        calls += "session-prepare:$agent:$folder:$text"
        if (folder.isEmpty()) return Outcome.Ok(com.ferb.mobile.SessionStartPlan(startFolders, null))
        // HAND-695: a new terminal; only a command is asked about, shown exactly
        if (text.isEmpty()) return Outcome.Ok(com.ferb.mobile.SessionStartPlan(startFolders, null))
        return Outcome.Ok(com.ferb.mobile.SessionStartPlan(startFolders, com.ferb.mobile.SessionYes("Open a new Terminal window in ${folder.substringAfterLast('/')} and type this?", text,
            "FERB opens it in the background and types only this command, then Return. You can type more into it from here.")))
    }
    override suspend fun sessionStart(agent: String, folder: String, text: String, signer: Signer): Outcome<String> {
        calls += "session-start:$agent:$folder:$text"
        val sig = signer.sign("sessions.start") ?: return Outcome.Failed(FerbException.NotSigned("Nothing was sent.", null))
        signed += sig
        return Outcome.Ok(text)
    }
    override suspend fun sessionDelegate(session: String, goal: String, signer: Signer): Outcome<String> {
        calls += "session-delegate:$session:$goal"
        val sig = signer.sign("sessions.delegate") ?: return Outcome.Failed(FerbException.NotSigned("Nothing was sent.", null))
        signed += sig
        return Outcome.Ok("FERB is on it.")
    }
    var sessionLiveRefusal: Outcome.Failed? = null
    override suspend fun watchSession(session: String, sink: PreviewSink): Outcome<Live> {
        calls += "session-live:$session"
        sessionLiveRefusal?.let { return it }
        preview = sink
        return Outcome.Ok(Live { previewStopped++ })
    }

    // ---- FERB's review of what an import brought in (kb MEM-460..457): the Mac's list, the answers it got, a refusal ----
    var memoryReview = com.ferb.mobile.MemoryReview(emptyList(), 0uL)
    var reviewRefusal: Outcome.Failed? = null
    /** Each answer as (part, item ids, answer). */
    val reviewAnswers = ArrayList<Triple<String, List<String>, String>>()
    override suspend fun memoryReview(): Outcome<com.ferb.mobile.MemoryReview> {
        calls += "memory-review"
        return Outcome.Ok(memoryReview)
    }
    /** As the Mac does: the answered items leave the list, an emptied group and part with them. */
    override suspend fun answerMemoryReview(part: String, itemIds: List<String>, answer: String): Outcome<com.ferb.mobile.MemoryReview> {
        calls += "memory-review-answer:$part:${itemIds.joinToString(",")}:$answer"
        reviewRefusal?.let { return it }
        reviewAnswers += Triple(part, itemIds, answer)
        val parts = memoryReview.parts.map { p ->
            if (p.part != part) p else p.copy(groups = p.groups.map { g -> g.copy(items = g.items.filter { it.id !in itemIds }) }.filter { it.items.isNotEmpty() })
        }.filter { it.groups.isNotEmpty() }
        memoryReview = com.ferb.mobile.MemoryReview(parts, parts.sumOf { p -> p.groups.sumOf { it.items.size } }.toULong())
        return Outcome.Ok(memoryReview)
    }

    companion object {
        /** One session as the library reads it (kb HAND-651..666). */
        fun agentSession(id: String = "claude:s1", title: String = "Fix the flaky login test", status: String = "working", host: String = "Terminal",
                         canSend: Boolean = true, noSend: String = "", withFerb: Boolean = false, controlledHere: Boolean = false,
                         terminals: List<com.ferb.mobile.SessionChoice> = emptyList()) =
            com.ferb.mobile.AgentSession(id, if (id.startsWith("codex")) "codex" else "claude_code", if (id.startsWith("codex")) "Codex" else "Claude Code",
                title, "/Users/owner/code/web-app", "web-app", status, if (status == "waiting_for_you") "permission" else "none", host, canSend, noSend,
                false, "2026-11-22T10:00:00Z", "", controlledHere, if (controlledHere) "phone:p1" else "", withFerb, "", terminals,
                "", "", "")

        fun item(seq: Int, kind: String, text: String, tool: String = "") = com.ferb.mobile.SessionItem(seq.toULong(), "2026-11-22T10:00:00Z", kind, text, tool)

        fun status(phase: LinkPhase = LinkPhase.ONLINE, paired: Boolean = true, path: LinkPath = LinkPath.DIRECT, code: String? = null, problem: com.ferb.mobile.LinkProblem? = null) =
            LinkStatus(phase, path, null, code, paired, path != LinkPath.PUBLIC_RELAY, false, null, false, null, problem, null, false, false)

        fun refused(linkCode: String) = Outcome.Failed(FerbException.NeedsUnlock("The Mac wants the phone's proof.", linkCode))
    }
}

/** A signer standing in for the prompt: it says which key it was asked for and signs, or the owner cancels. */
class FakeSigner(val kind: KeyKind, private val cancel: Boolean = false) : Signer {
    var asked = 0
    override fun sign(payload: String): String? { asked++; return if (cancel) null else "sig-$kind" }
}
