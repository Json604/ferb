package com.ferb.app.model

import com.ferb.app.FerbViewModel
import com.ferb.app.Note
import com.ferb.app.R
import com.ferb.core.Outcome
import com.ferb.core.linkCode
import com.ferb.mobile.Attachment
import com.ferb.mobile.FerbException
import com.ferb.mobile.TransferListener
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

enum class Way { FETCH, SEND }

enum class Phase { RUNNING, PAUSED, DONE, FAILED, WAITING_PATH }

/**
 * One file moving either way (PHN-13, PHN-115): its bytes so far of the whole, where it is on the phone, and how it
 * stands. A file sent belongs to a [batch] (the files picked together: one progress card, "Sending 2 photos to your
 * Mac"); [forChat] batches go to the composer when they arrive.
 */
data class Transfer(
    val key: String,
    val way: Way,
    val name: String,
    val local: String,
    val macPath: String? = null,
    val done: Long = 0,
    val total: Long = 0,
    val phase: Phase = Phase.RUNNING,
    val keyPlace: Boolean = false,
    val uploadId: String? = null,
    val problem: Note? = null,
    val batch: String? = null,
    val forChat: Boolean = false,
) {
    val fraction: Float get() = if (total > 0) (done.toFloat() / total).coerceIn(0f, 1f) else 0f
}

/**
 * The phone's transfers: fetches from the Mac into [LocalFiles.fetched] and uploads from [LocalFiles.outgoing], each
 * with progress, pause (the library's cancelTransfer: what arrived stays) and resume where it stopped (a fetch from the
 * file's length, an upload by its upload id), refused on the public relay before anything is asked and tried once more
 * by itself when the path carries files again ([bulkAllowed]). Never auto-opened: the screen opens a finished file
 * only on the owner's tap.
 */
class Transfers(
    private val scope: CoroutineScope,
    private val api: PhoneApi,
    private val local: LocalFiles,
    private val unlocker: Unlocker,
) {
    private val _list = MutableStateFlow<List<Transfer>>(emptyList())
    val list: StateFlow<List<Transfer>> = _list.asStateFlow()

    /** Uploads that arrived for the composer (PHN-127: a message's attachments are this phone's finished uploads). */
    private val _ready = MutableStateFlow<List<Attachment>>(emptyList())
    val ready: StateFlow<List<Attachment>> = _ready.asStateFlow()

    private val attempts = ConcurrentHashMap<String, String>()
    private val n = AtomicInteger()

    private fun update(key: String, f: (Transfer) -> Transfer) = _list.update { l -> l.map { if (it.key == key) f(it) else it } }
    private fun get(key: String) = _list.value.firstOrNull { it.key == key }

    // ---- fetching ---------------------------------------------------------------------------------------------------

    /** Fetches a file of the Mac; a key place's file asks for the fingerprint up front (PHN-121). */
    fun fetch(macPath: String, name: String, size: Long, keyPlace: Boolean): String {
        _list.value.firstOrNull { it.way == Way.FETCH && it.macPath == macPath && it.phase != Phase.FAILED }?.let { existing ->
            if (existing.phase == Phase.PAUSED || existing.phase == Phase.WAITING_PATH) resume(existing.key)
            return existing.key
        }
        val key = "f${n.incrementAndGet()}-${System.nanoTime()}"
        val file = local.fetchTarget(key, name)
        _list.update { it + Transfer(key, Way.FETCH, name, file.path, macPath, total = size, keyPlace = keyPlace) }
        runFetch(key)
        return key
    }

    private fun runFetch(key: String) = scope.launch {
        val t = get(key) ?: return@launch
        val attempt = "$key.${n.incrementAndGet()}"
        attempts[key] = attempt
        update(key) { it.copy(phase = Phase.RUNNING, problem = null) }
        val listener = listener(key, null)
        val r = unlocker.withProof(t.keyPlace, Why.FILE) { signer ->
            val offset = withContext(Dispatchers.IO) { File(t.local).let { if (it.isFile) it.length() else 0L } }
            api.fetch(attempt, t.macPath!!, t.local, offset, signer, listener)
        }
        if (attempts[key] != attempt) return@launch
        attempts.remove(key)
        when (r) {
            is Outcome.Ok -> update(key) { it.copy(phase = Phase.DONE, done = r.value.total.toLong(), total = r.value.total.toLong()) }
            is Outcome.Failed -> ended(key, r.error)
        }
    }

    // ---- sending ----------------------------------------------------------------------------------------------------

    /** Sends staged copies to the Mac, one after another, as one batch. */
    fun send(files: List<Staged>, forChat: Boolean): String {
        val batch = "b${n.incrementAndGet()}-${System.nanoTime()}"
        _list.update { l ->
            l + files.mapIndexed { i, s -> Transfer("$batch.$i", Way.SEND, s.name, s.path, total = s.size, phase = Phase.PAUSED, batch = batch, forChat = forChat) }
        }
        runBatch(batch)
        return batch
    }

    private fun runBatch(batch: String) = scope.launch {
        for (t in _list.value.filter { it.batch == batch }) {
            val cur = get(t.key) ?: return@launch
            if (cur.phase == Phase.DONE) continue
            val attempt = "${t.key}.${n.incrementAndGet()}"
            attempts[t.key] = attempt
            update(t.key) { it.copy(phase = Phase.RUNNING, problem = null) }
            val r = unlocker.run { api.upload(attempt, cur.local, cur.name, get(t.key)?.uploadId, listener(t.key, attempt)) }
            if (attempts[t.key] != attempt) return@launch
            attempts.remove(t.key)
            when (r) {
                is Outcome.Ok -> {
                    update(t.key) { it.copy(phase = Phase.DONE, done = r.value.size.toLong(), total = r.value.size.toLong(), uploadId = r.value.uploadId) }
                    withContext(Dispatchers.IO) { local.remove(cur.local) }
                }
                is Outcome.Failed -> {
                    ended(t.key, r.error)
                    return@launch
                }
            }
        }
        val all = _list.value.filter { it.batch == batch }
        if (all.isNotEmpty() && all.all { it.phase == Phase.DONE } && all.first().forChat) attach(batch)
    }

    /** The batch's files go to the composer, to send with a message (`sendWith`). */
    fun attach(batch: String) {
        val all = _list.value.filter { it.batch == batch && it.phase == Phase.DONE && it.uploadId != null }
        _ready.update { r -> r + all.map { Attachment(it.uploadId!!, it.name) }.filter { a -> r.none { it.uploadId == a.uploadId } } }
        _list.update { l -> l.filterNot { it.batch == batch } }
    }

    fun unattach(uploadId: String) = _ready.update { r -> r.filterNot { it.uploadId == uploadId } }

    /** The composer sent them. */
    fun sent() { _ready.value = emptyList() }

    // ---- either way -----------------------------------------------------------------------------------------------

    private fun listener(key: String, attempt: String?) = object : TransferListener {
        override fun progress(done: ULong, of: ULong) {
            if (attempt != null && attempts[key] != attempt) return
            update(key) { it.copy(done = done.toLong(), total = of.toLong().coerceAtLeast(it.total)) }
        }

        override fun accepted(uploadId: String, offset: ULong) {
            update(key) { it.copy(uploadId = uploadId, done = offset.toLong()) }
        }
    }

    private fun ended(key: String, e: FerbException) {
        val phase = when {
            e is FerbException.Cancelled || e is FerbException.NotSigned -> Phase.PAUSED
            e is FerbException.NeedsBetterPath -> Phase.WAITING_PATH
            else -> Phase.FAILED
        }
        val note = when {
            phase == Phase.PAUSED -> null
            e.linkCode == Unlocker.PAIR_AGAIN -> Note(R.string.needs_pairing_again)
            else -> FerbViewModel.errorNote(e)
        }
        update(key) { t ->
            val have = File(t.local).let { if (t.way == Way.FETCH && it.isFile) it.length() else t.done }
            t.copy(phase = phase, problem = note, done = have)
        }
    }

    /** Pauses a transfer (the one under way in a batch: the batch waits with it). */
    fun pause(key: String) {
        val attempt = attempts[key] ?: return
        scope.launch { api.cancelTransfer(attempt) }
    }

    fun resume(key: String) {
        val t = get(key) ?: return
        if (attempts.containsKey(key)) return
        if (t.batch != null) runBatch(t.batch) else runFetch(key)
    }

    /** Stops and forgets a transfer (its batch, for files sent) and its file on the phone. */
    fun remove(key: String) {
        val t = get(key) ?: return
        val gone = if (t.batch != null) _list.value.filter { it.batch == t.batch } else listOf(t)
        _list.update { l -> l - gone.toSet() }
        scope.launch {
            for (g in gone) {
                attempts.remove(g.key)?.let { api.cancelTransfer(it) }
                withContext(Dispatchers.IO) { local.remove(g.local) }
            }
        }
    }

    /** The path carries files again (PHN-444): each transfer that waited for it is tried once more. */
    fun bulkAllowed() {
        val waiting = _list.value.filter { it.phase == Phase.WAITING_PATH }
        for (b in waiting.mapNotNull { it.batch }.distinct()) runBatch(b)
        for (t in waiting.filter { it.batch == null }) runFetch(t.key)
    }

    /** PHN-11, PHN-15: the app locked: fetches stop and their files go. Files being sent stay paused. */
    fun locked() {
        for (t in _list.value) attempts.remove(t.key)?.let { a -> scope.launch { api.cancelTransfer(a) } }
        _list.update { l -> l.filter { it.way == Way.SEND }.map { if (it.phase == Phase.RUNNING) it.copy(phase = Phase.PAUSED) else it } }
        local.wipeFetched()
    }

    /** PHN-11: a key place's file goes the moment its viewer closes (the owner is back in FERB). */
    fun dropKeyPlaceFiles(keys: Collection<String>) {
        val gone = _list.value.filter { it.key in keys && it.keyPlace }
        if (gone.isEmpty()) return
        _list.update { l -> l - gone.toSet() }
        gone.forEach { local.remove(it.local) }
    }

    /** The screenshots' invented transfers (nothing runs). */
    internal fun setForTest(list: List<Transfer>) { _list.value = list }

    /** Forget this Mac. */
    fun clear() {
        for (a in attempts.values) scope.launch { api.cancelTransfer(a) }
        attempts.clear()
        _list.value = emptyList()
        _ready.value = emptyList()
        local.wipeAll()
    }
}
