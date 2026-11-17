package com.ferb.app.model

import com.ferb.core.FerbPhone
import com.ferb.core.KeyHealth
import com.ferb.core.KeyKind
import com.ferb.core.Outcome
import com.ferb.core.PhoneState
import com.ferb.core.PublicKeys
import com.ferb.mobile.AnswerInput
import com.ferb.mobile.Attachment
import com.ferb.mobile.Cancelled
import com.ferb.mobile.Change
import com.ferb.mobile.DeviceReport
import com.ferb.mobile.Fetched
import com.ferb.mobile.Folder
import com.ferb.mobile.FoundFiles
import com.ferb.mobile.LocationFix
import com.ferb.mobile.LinkStatus
import com.ferb.mobile.MacInfo
import com.ferb.mobile.OlderState
import com.ferb.mobile.OutboxItem
import com.ferb.mobile.PhoneStatus
import com.ferb.mobile.ScreenUse
import com.ferb.mobile.ScreenUseEnd
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
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * What the app's screens ask of the phone: one seam over [FerbPhone], so the view models are tested with a fake
 * and the real one is the library. Every call runs off the main thread (FerbPhone.call) and comes back typed.
 *
 * The rule (kb PHN-682; the owner's phone froze when the main thread asked the library for the last status itself):
 * everything here that reaches the library is a `suspend` function, so no caller on the main thread can wait inside
 * it. The only plain function is [setOpen], which hands its word to the link's own lane and returns
 * (`PhoneApiShapeTest` holds this).
 */
interface PhoneApi {
    val state: StateFlow<PhoneState>
    val link: StateFlow<LinkStatus?>
    val changes: SharedFlow<Change>

    /** The current conversation as the phone holds it, or null before anything was fetched. */
    suspend fun transcript(): Outcome<TranscriptView?>
    suspend fun sync(): Outcome<Unit>
    suspend fun send(text: String): Outcome<OutboxItem>
    suspend fun cancel(outboxId: String): Outcome<Cancelled>
    suspend fun answer(approvalId: String, input: AnswerInput, signer: Signer?): Outcome<String>

    /**
     * A hidden answer (PHN-109, PHN-110) from a [SecretInput] the caller filled one UTF-16 unit at a time: the library
     * empties and overwrites its copy whatever happens; the caller destroys [secret] after. Never `AnswerInput.Hidden`.
     */
    suspend fun answerHidden(approvalId: String, secret: SecretInput, signer: Signer): Outcome<String>
    suspend fun stop(taskIds: List<String>): Outcome<Unit>
    suspend fun loadOlder(conversation: String): Outcome<OlderState>
    suspend fun unlock(signer: Signer): Outcome<Unit>
    suspend fun pair(code: String): Outcome<Unit>
    /** "Forget this Mac": Ok(true) when the Mac removed this phone too, Ok(false) when it couldn't be told (PHN-646). */
    suspend fun forget(): Outcome<Boolean>

    /** Paired with its lock key there: the opening prompt can sign an unlock statement before the link is up. */
    suspend fun statementReady(): Boolean = false

    /** The statement the opening prompt signed, handed to the Mac with its age (PHN-649). */
    suspend fun unlockWithStatement(payload: String, sig: String, ageMs: Long): Outcome<Unit> =
        Outcome.Failed(com.ferb.mobile.FerbException.Offline("No link.", null))

    /** The app is unlocked and in front: the device key's presence keeps the Mac's window open (PHN-648). */
    fun setOpen(on: Boolean) {}

    /** "Try again" on the connection sheet (kb PHN-752): the link dials now instead of at its next try. Non-blocking. */
    fun retryLink() {}

    /** PHN-15: the app locked: the Mac is told and the conversation waits for the next unlock. */
    suspend fun lock(): Outcome<Boolean>

    /** The app opened by the prompt alone (the Mac out of reach): the next sync reads the conversation again. */
    suspend fun unlocked(): Outcome<Unit>

    /** "Stop everything on the Mac" (`stop.engage`): no prompt, works locked, never queued. */
    suspend fun stopEverything(): Outcome<Unit>

    /** What FERB has waiting, read now (network), and the last one pushed or read (memory). */
    suspend fun status(): Outcome<PhoneStatus>

    /**
     * "Until I'm back at my Mac" (PHN-610, PHN-611): whether FERB may use the Mac's screen without asking now, and the
     * owner's "Ask me again" from the phone (both plain, allowed while locked).
     */
    suspend fun screenUse(): Outcome<ScreenUse>
    suspend fun endScreenUse(): Outcome<ScreenUseEnd>
    suspend fun lastStatus(): PhoneStatus?

    suspend fun markReadLatest(conversation: String): Outcome<Unit>
    suspend fun sendWith(text: String, attachments: List<Attachment>): Outcome<OutboxItem>

    // files (PHN-113, PHN-115, PHN-127): the blocking ones run on IO and end with Cancelled when paused
    /** A folder's listing, a page at a time (PHN-606): [after] is the page before's `next` ("Load more"). */
    suspend fun files(path: String?, signer: Signer?, after: String? = null): Outcome<Folder>
    suspend fun fetch(id: String, path: String, saveTo: String, offset: Long, signer: Signer?, listener: TransferListener): Outcome<Fetched>
    suspend fun upload(id: String, localPath: String, name: String, resume: String?, group: String?, rel: String?, listener: TransferListener): Outcome<Uploaded>
    suspend fun cancelTransfer(id: String): Outcome<Unit>

    /** Every file under a folder of the Mac, to fetch it whole (direct or the owner's relay only). */
    suspend fun tree(path: String, signer: Signer?): Outcome<Tree>

    /** Starts a folder sent from the phone: its files carry the id as `group`; the id rides with a message. */
    suspend fun putGroup(name: String): Outcome<String>

    /** A finished task's [index]-th file (its result card's files, the outcome's sorted artifacts). */
    suspend fun fetchTaskFile(id: String, taskId: String, index: Long, saveTo: String, listener: TransferListener): Outcome<Fetched>

    /** The Mac's recent files for this phone and a search of names (both need the Mac's unlock). */
    suspend fun recentFiles(limit: Int): Outcome<RecentFiles>
    suspend fun searchFiles(query: String, path: String?, after: String? = null): Outcome<FoundFiles>


    /** The phone's own facts (T-1214, SENSE-420): sent when changed and on each new connection; memory only. */
    suspend fun reportDevice(report: DeviceReport): Outcome<Boolean>

    /** The Mac's open request for one exact fix (after the owner's Yes for a task), and its answer. */
    suspend fun locationWanted(): Outcome<String?>
    suspend fun answerLocation(requestId: String, fix: LocationFix?, whyNot: String?): Outcome<Unit>

    /** A task's live preview (PHN-454): frames to [sink] in memory only; [Live.stop] ends it. */
    suspend fun watchLive(taskId: String, sink: PreviewSink): Outcome<Live>

    /** The work row's small live picture (kb PHN-725): the same preview at most every [everyMs] (never under 2 s). */
    suspend fun watchThumbnail(taskId: String, everyMs: Long, sink: PreviewSink): Outcome<Live>

    /** A card row's own small live picture (kb PHN-726; FERB.app's RowPreview): one step's preview ([worker]). */
    suspend fun watchStepThumbnail(taskId: String, worker: String, everyMs: Long, sink: PreviewSink): Outcome<Live>

    suspend fun settings(): Outcome<List<SettingValue>>
    suspend fun setSetting(key: String, valueJson: String, signer: Signer): Outcome<String>
    suspend fun tasks(limit: Int, before: String?): Outcome<TaskPage>
    suspend fun macInfo(): Outcome<MacInfo>
    suspend fun cachedMacInfo(): Outcome<MacInfo?>

    /** Keys made after pairing the Mac doesn't hold (a fingerprint enrolled later), and registering them (PHN-133). */
    suspend fun unregisteredKeys(): PublicKeys?
    suspend fun registerKeys(signer: (KeyKind) -> Signer): Outcome<Boolean>
}

/** A live preview under way. */
fun interface Live {
    fun stop()
}

val PhoneState.keys: KeyHealth? get() = (this as? PhoneState.Open)?.keys

/** The real phone. */
class LibraryPhone(private val phone: FerbPhone) : PhoneApi {
    override val state get() = phone.state
    override val link get() = phone.link
    override val changes get() = phone.changes

    override suspend fun transcript(): Outcome<TranscriptView?> = phone.call { m ->
        val conv = m.currentConversation() ?: m.conversations().firstOrNull { it.current }?.id
        conv?.let(m::transcript)
    }.also { r ->
        // FERB's words laid out by the library (kb PHN-721..724) here, off the main thread, so the rows the screen
        // builds from this view find every layout kept (ChatModel.prepare)
        (r as? Outcome.Ok)?.value?.let { v -> kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.Default) { ChatModel.prepare(v) } }
    }

    override suspend fun sync() = phone.call { it.sync() }
    override suspend fun send(text: String) = phone.call { it.send(text) }
    override suspend fun cancel(outboxId: String) = phone.call { it.cancel(outboxId) }
    override suspend fun answer(approvalId: String, input: AnswerInput, signer: Signer?) = phone.call { it.answer(approvalId, input, signer) }
    override suspend fun answerHidden(approvalId: String, secret: SecretInput, signer: Signer) = phone.call { it.answerHidden(approvalId, secret, signer) }
    override suspend fun stop(taskIds: List<String>) = phone.call { it.stop(taskIds) }
    override suspend fun loadOlder(conversation: String) = phone.call { it.loadOlder(conversation) }
    override suspend fun unlock(signer: Signer) = phone.unlock(signer)
    override suspend fun pair(code: String) = phone.pair(code)
    override suspend fun forget() = phone.forgetThisMac()
    override suspend fun statementReady() = phone.statementReady()
    override suspend fun unlockWithStatement(payload: String, sig: String, ageMs: Long) = phone.unlockWithStatement(payload, sig, ageMs)
    override fun setOpen(on: Boolean) = phone.setOpen(on)
    override fun retryLink() = phone.retry()
    override suspend fun lock() = phone.lock()
    override suspend fun unlocked() = phone.unlocked()
    override suspend fun stopEverything() = phone.stopEverything()
    override suspend fun status() = phone.call { it.status() }
    override suspend fun screenUse() = phone.call { it.screenUse() }
    override suspend fun endScreenUse() = phone.call { it.endScreenUse() }
    override suspend fun lastStatus(): PhoneStatus? = phone.lastStatus()
    override suspend fun markReadLatest(conversation: String) = phone.call { it.markReadLatest(conversation) }
    override suspend fun sendWith(text: String, attachments: List<Attachment>) = phone.call { it.sendWith(text, attachments) }
    override suspend fun files(path: String?, signer: Signer?, after: String?) = phone.call { it.files(path, signer, after) }
    override suspend fun fetch(id: String, path: String, saveTo: String, offset: Long, signer: Signer?, listener: TransferListener) =
        phone.call { it.fetch(id, path, saveTo, offset.toULong(), signer, listener) }
    override suspend fun upload(id: String, localPath: String, name: String, resume: String?, group: String?, rel: String?, listener: TransferListener) =
        phone.call { it.upload(id, localPath, name, resume, group, rel, listener) }
    override suspend fun cancelTransfer(id: String) = phone.call { it.cancelTransfer(id) }
    override suspend fun tree(path: String, signer: Signer?) = phone.call { it.tree(path, signer) }
    override suspend fun putGroup(name: String) = phone.call { it.putGroup(name) }
    override suspend fun fetchTaskFile(id: String, taskId: String, index: Long, saveTo: String, listener: TransferListener) =
        phone.call { it.fetchTaskFile(id, taskId, index.toULong(), saveTo, listener) }
    override suspend fun recentFiles(limit: Int) = phone.call { it.recentFiles(limit.toUInt()) }
    override suspend fun searchFiles(query: String, path: String?, after: String?) = phone.call { it.searchFiles(query, path, after) }
    override suspend fun reportDevice(report: DeviceReport) = phone.call { it.reportDevice(report) }
    override suspend fun locationWanted() = phone.call { it.locationWanted() }
    override suspend fun answerLocation(requestId: String, fix: LocationFix?, whyNot: String?) = phone.call { it.answerLocation(requestId, fix, whyNot) }
    override suspend fun watchLive(taskId: String, sink: PreviewSink): Outcome<Live> =
        phone.call { m -> m.watchLive(taskId, sink).let { p -> Live { runCatching { p.stop() } } } }

    override suspend fun watchThumbnail(taskId: String, everyMs: Long, sink: PreviewSink): Outcome<Live> =
        phone.call { m -> m.watchThumbnail(taskId, everyMs.toULong(), sink).let { p -> Live { runCatching { p.stop() } } } }

    override suspend fun watchStepThumbnail(taskId: String, worker: String, everyMs: Long, sink: PreviewSink): Outcome<Live> =
        phone.call { m -> m.watchStepThumbnail(taskId, worker, everyMs.toULong(), sink).let { p -> Live { runCatching { p.stop() } } } }
    override suspend fun settings() = phone.call { it.settings() }
    override suspend fun setSetting(key: String, valueJson: String, signer: Signer) = phone.call { it.setSetting(key, valueJson, signer) }
    override suspend fun tasks(limit: Int, before: String?) = phone.call { it.tasks(limit.toUInt(), before) }
    override suspend fun macInfo() = phone.call { it.macInfo() }
    override suspend fun cachedMacInfo() = phone.call { it.cachedMacInfo() }
    override suspend fun unregisteredKeys() = phone.unregisteredKeys()
    override suspend fun registerKeys(signer: (KeyKind) -> Signer) = phone.registerKeys(signer)
}
