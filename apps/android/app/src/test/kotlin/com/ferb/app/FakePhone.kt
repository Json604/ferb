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
import com.ferb.mobile.FerbException
import com.ferb.mobile.Fetched
import com.ferb.mobile.Folder
import com.ferb.mobile.LinkPath
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.LinkStatus
import com.ferb.mobile.MacInfo
import com.ferb.mobile.OlderState
import com.ferb.mobile.OutboxItem
import com.ferb.mobile.PhoneStatus
import com.ferb.mobile.PreviewSink
import com.ferb.mobile.SettingValue
import com.ferb.mobile.Signer
import com.ferb.mobile.TaskPage
import com.ferb.mobile.TransferListener
import com.ferb.mobile.TranscriptView
import com.ferb.mobile.Uploaded
import java.io.File
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
    val sentWith = ArrayList<Pair<String, List<Attachment>>>()
    var preview: PreviewSink? = null
    var previewStopped = 0
    val settingsValues = LinkedHashMap<String, String>()
    var settingsRefusal: Outcome.Failed? = null
    var tasks: Outcome<TaskPage> = Outcome.Ok(TaskPage(emptyList(), false))
    var mac: MacInfo? = null
    var newKeys: PublicKeys? = null

    override suspend fun transcript(): Outcome<TranscriptView?> = Outcome.Ok(view?.copy(outbox = outbox.toList()))
    override suspend fun sync(): Outcome<Unit> { calls += "sync"; return Outcome.Ok(Unit) }
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
    override suspend fun stop(taskIds: List<String>): Outcome<Unit> { calls += "stop:${taskIds.joinToString()}"; return stopResult }
    override suspend fun loadOlder(conversation: String): Outcome<OlderState> { calls += "older"; return Outcome.Ok(OlderState.Complete) }
    override suspend fun unlock(signer: Signer): Outcome<Unit> {
        calls += "unlock"
        val r = unlockResult
        if (r is Outcome.Ok && signer.sign("challenge") == null) return Outcome.Failed(FerbException.NotSigned("Nothing was sent.", null))
        return r
    }
    override suspend fun pair(code: String): Outcome<Unit> { calls += "pair:$code"; return Outcome.Ok(Unit) }
    override suspend fun forget(): Outcome<Unit> { calls += "forget"; return Outcome.Ok(Unit) }
    override suspend fun lock(): Outcome<Boolean> { calls += "lock"; return Outcome.Ok(true) }
    override suspend fun unlocked(): Outcome<Unit> { calls += "unlocked"; return Outcome.Ok(Unit) }
    override suspend fun stopEverything(): Outcome<Unit> { calls += "stop-everything"; return Outcome.Ok(Unit) }
    override suspend fun status(): Outcome<PhoneStatus> { calls += "status"; return status?.let { Outcome.Ok(it) } ?: Outcome.Failed(FerbException.Offline("no", null)) }
    override fun lastStatus(): PhoneStatus? = status
    override suspend fun markReadLatest(conversation: String): Outcome<Unit> { calls += "mark:$conversation"; return Outcome.Ok(Unit) }
    override suspend fun sendWith(text: String, attachments: List<Attachment>): Outcome<OutboxItem> {
        sentWith += text to attachments
        val item = OutboxItem("o${outbox.size + 1}", text, "2026-01-02T10:00:00+00:00", "waiting", "link", null, false, attachments)
        outbox += item
        return Outcome.Ok(item)
    }
    override suspend fun files(path: String?, signer: Signer?): Outcome<Folder> {
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
    override suspend fun upload(id: String, localPath: String, name: String, resume: String?, listener: TransferListener): Outcome<Uploaded> {
        calls += "upload:$name:${resume ?: "-"}"
        uploadRefusal?.let { return it }
        val size = File(localPath).length()
        listener.accepted(resume ?: "up-$name", 0u)
        listener.progress(size.toULong(), size.toULong())
        return Outcome.Ok(Uploaded(resume ?: "up-$name", "~/Downloads/FERB/$name", size.toULong()))
    }
    override suspend fun cancelTransfer(id: String): Outcome<Unit> { cancelled += id; fetchGate?.complete(Unit); return Outcome.Ok(Unit) }
    override suspend fun watchLive(taskId: String, sink: PreviewSink): Outcome<Live> {
        calls += "live:$taskId"
        preview = sink
        return Outcome.Ok(Live { previewStopped++ })
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
