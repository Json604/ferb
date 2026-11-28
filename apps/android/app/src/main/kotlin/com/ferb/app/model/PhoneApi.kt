package com.ferb.app.model

import com.ferb.core.FerbPhone
import com.ferb.core.KeyHealth
import com.ferb.core.KeyKind
import com.ferb.core.Outcome
import com.ferb.core.PhoneState
import com.ferb.core.PublicKeys
import com.ferb.mobile.AnswerInput
import com.ferb.mobile.Attachment
import com.ferb.mobile.CameraSink
import com.ferb.mobile.Insomnia
import com.ferb.mobile.InsomniaEnded
import com.ferb.mobile.InsomniaStarted
import com.ferb.mobile.MacLocked
import com.ferb.mobile.Cancelled
import com.ferb.mobile.Change
import com.ferb.mobile.DeviceReport
import com.ferb.mobile.Fetched
import com.ferb.mobile.Folder
import com.ferb.mobile.FoundFiles
import com.ferb.mobile.LocationFix
import com.ferb.mobile.LinkStatus
import com.ferb.mobile.MacInfo
import com.ferb.mobile.MemoryReview
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
import com.ferb.mobile.TemporaryChat
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
    /**
     * Pairs from a code, carrying [account]: the server's signed statement of this phone's FERB account for
     * [endpointId] (kb PHN-828); the Mac's daemon refuses a phone of another account before it asks.
     */
    suspend fun pair(code: String, account: String): Outcome<Unit>

    /** This phone's link id, which the account statement names; null before the link started. */
    suspend fun endpointId(): String?

    /**
     * A phone paired before FERB's account existed binds its pairing to the account it is signed in to now
     * (`phone.bind_account` with the server's statement, kb PHN-828): Ok(true) when the Mac bound it. The Mac refuses
     * another account with the link code `account_mismatch` (and removes this phone).
     */
    suspend fun bindAccount(assertion: String): Outcome<Boolean>
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

    /** kb PHN-847, PHN-848: the owner's "Lock FERB now": [lock], and the Mac is told it was by his hand. */
    suspend fun lockByHand(): Outcome<Boolean> = lock()

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

    /**
     * Insomnia mode (APP-980..APP-985): read it, turn it on after the app's warning (the sensitive class: [signer] is
     * the fingerprint-only key where the phone has one), end it, Lock my Mac and It's fine (the last three plain,
     * allowed while locked), and Show me: the Mac's camera to [sink] in memory only, never on the public relay.
     */
    suspend fun insomniaStatus(): Outcome<Insomnia>
    suspend fun startInsomnia(variant: String, screenYes: Boolean, signer: Signer): Outcome<InsomniaStarted>
    suspend fun endInsomnia(): Outcome<InsomniaEnded>
    suspend fun lockMac(): Outcome<MacLocked>
    suspend fun dismissInsomniaNotice(noticeId: String): Outcome<Insomnia>
    suspend fun watchCamera(sink: CameraSink): Outcome<Live>

    suspend fun markReadLatest(conversation: String): Outcome<Unit>
    /** A message with files, or quoting one of the chat's messages ([replyTo], kb APP-1146: `chat.send`'s `reply_to`). */
    suspend fun sendWith(text: String, attachments: List<Attachment>, replyTo: com.ferb.mobile.ReplyQuote? = null): Outcome<OutboxItem>

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
    /** Puts one of FERB's alerts away on every device (`attention.ack`, kb APP-1106, PHN-791). */
    suspend fun dismissAlert(id: String): Outcome<Unit>
    suspend fun macInfo(): Outcome<MacInfo>

    /**
     * kb PHN-846: this phone's app lock, kept on the Mac: off is `phone.lock_off`, signed by [signer] (the fingerprint-only
     * key where the Mac holds one); on is `phone.lock_on`, no proof. The Mac's `lock_off`.
     */
    suspend fun setLockOff(off: Boolean, signer: Signer?): Outcome<Boolean> =
        Outcome.Failed(com.ferb.mobile.FerbException.Offline("No link.", null))
    suspend fun cachedMacInfo(): Outcome<MacInfo?>

    /** Keys made after pairing the Mac doesn't hold (a fingerprint enrolled later), and registering them (PHN-133). */
    suspend fun unregisteredKeys(): PublicKeys?
    suspend fun registerKeys(signer: (KeyKind) -> Signer): Outcome<Boolean>

    /**
     * Update fingerprint: a new fingerprint-only key for the paired Mac, with the code FERB on the Mac shows after
     * Update fingerprint (scanned or typed). Returns once the link took it; [link]'s `fingerprintUpdate` says the rest.
     */
    suspend fun updateFingerprint(code: String): Outcome<Unit>

    /**
     * FERB's review of what an import brought in (kb MEM-460..457; ferb-mobile `review.rs`): the list as it stands, and
     * the owner's answer to items of one part (one or all at once), which gives the list after it. Both in the unlock window.
     */
    suspend fun memoryReview(): Outcome<MemoryReview>
    suspend fun answerMemoryReview(part: String, itemIds: List<String>, answer: String): Outcome<MemoryReview>

    /** Approve all (MEM-486): [approve] the ticked lines, [decline] the unticked ones (Decline all: every line). */
    suspend fun answerMemoryReviewAll(approve: List<String>, decline: List<String>): Outcome<MemoryReview>

    /** The owner opened FERB on this phone (MEM-485): the Mac's first open of the day shows what waits to check. */
    suspend fun reviewOpened(): Outcome<Unit>

    /**
     * The temporary chat (the owner, 2026-11-23): read from the Mac when the app opens (`chat.temporary.get`), turned on
     * and off by the one dashed bubble (`chat.temporary.set`), and a message sent now with `fresh: true` (with files as
     * the chat sends them), never through the outbox (a dropped link is a failure; the words stay with the app). After
     * that the library's one watch keeps it as it keeps the chat (the owner, 2026-11-24, ORCH-715): each
     * `Change.Temporary` is read from memory with [lastTemporaryChat], never from the Mac. Memory only: nothing of it is
     * stored on the phone.
     */
    suspend fun temporaryChat(): Outcome<TemporaryChat> = Outcome.Failed(com.ferb.mobile.FerbException.Offline("No link.", null))
    suspend fun setTemporaryChat(on: Boolean): Outcome<TemporaryChat> = Outcome.Failed(com.ferb.mobile.FerbException.Offline("No link.", null))
    suspend fun sendTemporary(text: String): Outcome<TemporaryChat> = Outcome.Failed(com.ferb.mobile.FerbException.Offline("No link.", null))
    /** With files and, replying to one of the temporary chat's messages, its quote (`replyTo`, kb APP-1146). */
    suspend fun sendTemporaryWith(text: String, attachments: List<Attachment>, replyTo: com.ferb.mobile.ReplyQuote? = null): Outcome<TemporaryChat> =
        Outcome.Failed(com.ferb.mobile.FerbException.Offline("No link.", null))
    suspend fun lastTemporaryChat(): Outcome<TemporaryChat?> = Outcome.Ok(null)
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
    override suspend fun pair(code: String, account: String) = phone.pair(code, account)
    override suspend fun endpointId() = phone.endpointId()
    override suspend fun bindAccount(assertion: String) = phone.call { it.bindAccount(assertion) }
    override suspend fun forget() = phone.forgetThisMac()
    override suspend fun statementReady() = phone.statementReady()
    override suspend fun unlockWithStatement(payload: String, sig: String, ageMs: Long) = phone.unlockWithStatement(payload, sig, ageMs)
    override fun setOpen(on: Boolean) = phone.setOpen(on)
    override fun retryLink() = phone.retry()
    override suspend fun lock() = phone.lock()
    override suspend fun lockByHand() = phone.lockByHand()
    override suspend fun unlocked() = phone.unlocked()
    override suspend fun stopEverything() = phone.stopEverything()
    override suspend fun status() = phone.call { it.status() }
    override suspend fun screenUse() = phone.call { it.screenUse() }
    override suspend fun endScreenUse() = phone.call { it.endScreenUse() }
    override suspend fun lastStatus(): PhoneStatus? = phone.lastStatus()
    override suspend fun insomniaStatus() = phone.call { it.insomniaStatus() }
    override suspend fun startInsomnia(variant: String, screenYes: Boolean, signer: Signer) =
        phone.call { it.startInsomnia(variant, screenYes, signer) }
    override suspend fun endInsomnia() = phone.call { it.endInsomnia() }
    override suspend fun lockMac() = phone.call { it.lockMac() }
    override suspend fun dismissInsomniaNotice(noticeId: String) = phone.call { it.dismissInsomniaNotice(noticeId) }
    override suspend fun watchCamera(sink: CameraSink): Outcome<Live> =
        phone.call { m -> m.watchCamera(sink).let { p -> Live { runCatching { p.stop() } } } }
    override suspend fun markReadLatest(conversation: String) = phone.call { it.markReadLatest(conversation) }
    override suspend fun sendWith(text: String, attachments: List<Attachment>, replyTo: com.ferb.mobile.ReplyQuote?) =
        phone.call { it.sendWith(text, attachments, replyTo) }
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
    override suspend fun dismissAlert(id: String) = phone.call { it.dismissAlert(id) }
    override suspend fun macInfo() = phone.call { it.macInfo() }
    override suspend fun setLockOff(off: Boolean, signer: Signer?) = phone.call { it.setLockOff(off, signer) }
    override suspend fun cachedMacInfo() = phone.call { it.cachedMacInfo() }
    override suspend fun unregisteredKeys() = phone.unregisteredKeys()
    override suspend fun registerKeys(signer: (KeyKind) -> Signer) = phone.registerKeys(signer)
    override suspend fun updateFingerprint(code: String) = phone.updateFingerprint(code)

    override suspend fun memoryReview() = phone.call { it.memoryReview() }
    override suspend fun answerMemoryReview(part: String, itemIds: List<String>, answer: String) =
        phone.call { it.answerMemoryReview(part, itemIds, answer) }
    override suspend fun answerMemoryReviewAll(approve: List<String>, decline: List<String>) =
        phone.call { it.answerMemoryReviewAll(approve, decline) }
    override suspend fun reviewOpened() = phone.call { it.reviewOpened("android") }

    // FERB's words laid out off the main thread, as the chat's (ChatModel.prepare)
    override suspend fun temporaryChat() = phone.call { it.temporaryChat().also { t -> ChatModel.prepare(t.transcript) } }
    override suspend fun setTemporaryChat(on: Boolean) = phone.call { it.setTemporaryChat(on).also { t -> ChatModel.prepare(t.transcript) } }
    override suspend fun sendTemporary(text: String) = phone.call { it.sendTemporary(text).also { t -> ChatModel.prepare(t.transcript) } }
    override suspend fun sendTemporaryWith(text: String, attachments: List<Attachment>, replyTo: com.ferb.mobile.ReplyQuote?) =
        phone.call { it.sendTemporaryWith(text, attachments, replyTo).also { t -> ChatModel.prepare(t.transcript) } }
    override suspend fun lastTemporaryChat() = phone.call { it.lastTemporaryChat()?.also { t -> ChatModel.prepare(t.transcript) } }
}
