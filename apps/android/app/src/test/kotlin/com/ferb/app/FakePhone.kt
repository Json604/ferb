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
    /** The account statement each pairing carried (kb PHN-828), in order; [endpoint] is what it must name. */
    val pairAccounts = ArrayList<String>()
    var endpoint: String? = com.ferb.core.FAKE_ENDPOINT
    override suspend fun pair(code: String, account: String): Outcome<Unit> { calls += "pair:$code"; pairAccounts += account; return Outcome.Ok(Unit) }
    override suspend fun endpointId(): String? = endpoint

    /** The statements each bind carried (kb PHN-828), and what each bind answers in turn (then Ok(true)). */
    val binds = ArrayList<String>()
    val bindScript = ArrayDeque<Outcome<Boolean>>()
    override suspend fun bindAccount(assertion: String): Outcome<Boolean> {
        calls += "bind"; binds += assertion
        return bindScript.removeFirstOrNull() ?: Outcome.Ok(true)
    }
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
    override suspend fun lockByHand(): Outcome<Boolean> { calls += "lock:hand"; return Outcome.Ok(true) }
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
    /** The Mac's record of this phone's app lock (kb PHN-846), and a refusal to give instead when set. */
    var lockOff = false
    var lockOffRefusal: Outcome.Failed? = null
    override suspend fun setLockOff(off: Boolean, signer: Signer?): Outcome<Boolean> {
        calls += "lockoff:$off"
        lockOffRefusal?.let { return it }
        if (off && signer?.sign("challenge") == null) return Outcome.Failed(FerbException.NotSigned("Nothing was sent.", null))
        lockOff = off
        mac = mac?.copy(lockOff = off)
        return Outcome.Ok(off)
    }
    override suspend fun unregisteredKeys(): PublicKeys? = newKeys
    override suspend fun registerKeys(signer: (KeyKind) -> Signer): Outcome<Boolean> {
        val k = newKeys ?: return Outcome.Ok(false)
        signer(KeyKind.LOCK).sign("link.keys")?.let(signed::add) ?: return Outcome.Failed(FerbException.NotSigned("Nothing was sent.", null))
        calls += "keys:${k.strong}"
        newKeys = null
        return Outcome.Ok(true)
    }

    // ---- FERB's review of what an import brought in (kb MEM-460..457): the Mac's list, the answers it got, a refusal ----
    var memoryReview = com.ferb.mobile.MemoryReview(emptyList(), 0uL, emptyList(), "", "")
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
        memoryReview = memoryReview.copy(parts = parts, count = parts.sumOf { p -> p.groups.sumOf { it.items.size } }.toULong())
        return Outcome.Ok(memoryReview)
    }
    /** Each Approve all / Decline all as (approved ids, declined ids) (MEM-486). */
    val reviewAll = ArrayList<Pair<List<String>, List<String>>>()
    /** As the Mac does: every line named leaves the list. */
    override suspend fun answerMemoryReviewAll(approve: List<String>, decline: List<String>): Outcome<com.ferb.mobile.MemoryReview> {
        calls += "memory-review-all"
        reviewRefusal?.let { return it }
        reviewAll += approve to decline
        val gone = (approve + decline).toSet()
        val parts = memoryReview.parts.map { p ->
            p.copy(groups = p.groups.map { g -> g.copy(items = g.items.filter { it.id !in gone }) }.filter { it.items.isNotEmpty() })
        }.filter { it.groups.isNotEmpty() }
        memoryReview = memoryReview.copy(parts = parts, count = parts.sumOf { p -> p.groups.sumOf { it.items.size } }.toULong())
        return Outcome.Ok(memoryReview)
    }
    override suspend fun reviewOpened(): Outcome<Unit> {
        calls += "review-opened"
        return Outcome.Ok(Unit)
    }

    /** The Mac's temporary chat (memory only on the Mac); [temporaryItems] are its turns as the library folds them. */
    var temporaryOn = false
    var temporaryItems: List<com.ferb.mobile.Item> = emptyList()
    /** Its whole transcript as the library folds the Mac's entries (its questions too), in place of [temporaryItems]. */
    var temporaryView: TranscriptView? = null
    var temporaryRefusal: Outcome.Failed? = null
    val temporarySent = ArrayList<String>()
    /** A temporary send waits here (its message on its way) until it is let go. */
    var temporaryGate: CompletableDeferred<Unit>? = null
    fun temporaryNow() = com.ferb.mobile.TemporaryChat(
        temporaryOn, if (temporaryOn) "T1" else null, if (temporaryOn) "2026-01-02T10:05:00+00:00" else null, if (temporaryOn) 1L else 0L,
        temporaryView?.takeIf { temporaryOn }
            ?: TranscriptView(if (temporaryOn) "temporary:T1" else "", if (temporaryOn) temporaryItems else emptyList(), emptyList(), emptyList(), emptyList(), OlderState.Complete),
    )
    override suspend fun temporaryChat(): Outcome<com.ferb.mobile.TemporaryChat> { calls += "temporary"; return Outcome.Ok(temporaryNow()) }
    override suspend fun setTemporaryChat(on: Boolean): Outcome<com.ferb.mobile.TemporaryChat> {
        calls += "temporary:$on"
        temporaryRefusal?.let { return it }
        temporaryOn = on
        if (!on) temporaryItems = emptyList()
        return Outcome.Ok(temporaryNow())
    }
    override suspend fun sendTemporary(text: String): Outcome<com.ferb.mobile.TemporaryChat> = sendTemporaryWith(text, emptyList())
    /** The files each temporary message carried, by upload id. */
    val temporaryFiles = ArrayList<List<String>>()
    override suspend fun sendTemporaryWith(text: String, attachments: List<Attachment>): Outcome<com.ferb.mobile.TemporaryChat> {
        calls += "send-temporary:$text"
        temporaryGate?.await()
        temporaryRefusal?.let { return it }
        temporarySent += text
        temporaryFiles += attachments.map { it.uploadId }
        temporaryOn = true
        return Outcome.Ok(temporaryNow())
    }
    /** The real library on the host, when a test drives the temporary chat's watch through it. */
    var realTemporary: com.ferb.mobile.Mobile? = null
    /** What the library's watch holds of the temporary chat (memory): read on each `Change.Temporary`, no call made. */
    override suspend fun lastTemporaryChat(): Outcome<com.ferb.mobile.TemporaryChat?> {
        calls += "last-temporary"
        realTemporary?.let { return Outcome.Ok(it.lastTemporaryChat()) }
        return Outcome.Ok(temporaryNow().takeIf { it.on })
    }

    companion object {
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
