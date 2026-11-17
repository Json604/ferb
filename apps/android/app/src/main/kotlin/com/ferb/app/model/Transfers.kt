package com.ferb.app.model

import com.ferb.app.FerbViewModel
import com.ferb.app.Note
import com.ferb.app.R
import com.ferb.core.Outcome
import com.ferb.core.linkCode
import com.ferb.mobile.Attachment
import com.ferb.mobile.FerbException
import com.ferb.mobile.TransferListener
import com.ferb.mobile.Tree
import java.io.File
import java.io.InputStream
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

/** A finished task's own file: its task and its place among the outcome's sorted files (the result card's). */
data class TaskFile(val taskId: String, val index: Long)

/**
 * One file moving either way (PHN-13, PHN-115): its bytes so far of the whole, where it is on the phone, and how it
 * stands. A file sent belongs to a [batch] (the files picked together: one progress card, "Sending 2 photos to your
 * Mac"); [forChat] batches go to the composer when they arrive. A file of a whole folder carries its [rel] path inside
 * it; a task's own file its [task].
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
    val rel: String? = null,
    val task: TaskFile? = null,
    /** A file sent: where it is on the Mac in the Mac's words, when the library says (UploadPlace); null: the default place. */
    val macPlace: String? = null,
    /** A file of a message he sent with files (kb PHN-691): its batch is the message; shown with it, never on the Files tab. */
    val message: Boolean = false,
) {
    val fraction: Float get() = if (total > 0) (done.toFloat() / total).coerceIn(0f, 1f) else 0f
}

/**
 * Files that move together. A whole [folder] either way (PHN-13: "a folder goes either way, with its progress"): fetched
 * from the Mac's listing of every file under it ([skipped] key-place files left out, [cut] when the listing stopped at
 * its cap), or sent as one folder in the Mac's upload place ([group], the id its files carry and the message's
 * attachment). [unreadable]: files of a picked folder that couldn't be read, so they weren't sent.
 */
data class Batch(
    val key: String,
    val way: Way,
    val folder: String? = null,
    val macPath: String? = null,
    val skipped: Long = 0,
    val cut: Boolean = false,
    val group: String? = null,
    val unreadable: Int = 0,
)

/**
 * The phone's transfers: fetches from the Mac into [LocalFiles.fetched] and uploads from [LocalFiles.outgoing], each
 * with progress, pause (the library's cancelTransfer: what arrived stays) and resume where it stopped (a fetch from the
 * file's length, an upload by its upload id), refused on the public relay before anything is asked and tried once more
 * by itself when the path carries files again ([bulkAllowed]). A batch's files go one after another. Never
 * auto-opened: the screen opens a finished file only on the owner's tap.
 */
class Transfers(
    private val scope: CoroutineScope,
    private val api: PhoneApi,
    private val local: LocalFiles,
    private val unlocker: Unlocker,
) {
    private val _list = MutableStateFlow<List<Transfer>>(emptyList())
    val list: StateFlow<List<Transfer>> = _list.asStateFlow()

    private val _batches = MutableStateFlow<Map<String, Batch>>(emptyMap())
    val batches: StateFlow<Map<String, Batch>> = _batches.asStateFlow()

    /** Where the composer's files are on the Mac, in the Mac's words when the library says (PHN-608); null: the default. */
    private val _readyPlace = MutableStateFlow<String?>(null)
    val readyPlace: StateFlow<String?> = _readyPlace.asStateFlow()

    /** Uploads that arrived for the composer (PHN-127: a message's attachments are this phone's finished uploads). */
    private val _ready = MutableStateFlow<List<Attachment>>(emptyList())
    val ready: StateFlow<List<Attachment>> = _ready.asStateFlow()

    private val attempts = ConcurrentHashMap<String, String>()

    /** Where a folder's file is read from, until it is copied just before it goes (memory only). */
    private val sources = ConcurrentHashMap<String, () -> InputStream?>()
    private val n = AtomicInteger()

    private fun update(key: String, f: (Transfer) -> Transfer) = _list.update { l -> l.map { if (it.key == key) f(it) else it } }
    private fun get(key: String) = _list.value.firstOrNull { it.key == key }
    private fun setBatch(key: String, f: (Batch) -> Batch) = _batches.update { m -> m[key]?.let { m + (key to f(it)) } ?: m }

    // ---- fetching ---------------------------------------------------------------------------------------------------

    /** Fetches a file of the Mac; a key place's file asks for the fingerprint up front (PHN-121). */
    fun fetch(macPath: String, name: String, size: Long, keyPlace: Boolean): String {
        _list.value.firstOrNull { it.way == Way.FETCH && it.batch == null && it.macPath == macPath && it.phase != Phase.FAILED }?.let { existing ->
            if (existing.phase == Phase.PAUSED || existing.phase == Phase.WAITING_PATH) resume(existing.key)
            return existing.key
        }
        val key = "f${n.incrementAndGet()}-${System.nanoTime()}"
        val file = local.fetchTarget(key, name)
        _list.update { it + Transfer(key, Way.FETCH, name, file.path, macPath, total = size, keyPlace = keyPlace) }
        runFetch(key)
        return key
    }

    /**
     * A finished task's own file (its result card): the Mac names it from the task's record by its place among the
     * outcome's files, so the phone never sends a path it read in the chat. Kb PHN-535.
     */
    fun fetchTaskFile(taskId: String, index: Long, name: String): String {
        val task = TaskFile(taskId, index)
        _list.value.firstOrNull { it.task == task && it.phase != Phase.FAILED }?.let { existing ->
            if (existing.phase == Phase.PAUSED || existing.phase == Phase.WAITING_PATH) resume(existing.key)
            return existing.key
        }
        val key = "t${n.incrementAndGet()}-${System.nanoTime()}"
        val file = local.fetchTarget(key, name)
        _list.update { it + Transfer(key, Way.FETCH, name, file.path, task = task) }
        runFetch(key)
        return key
    }

    /** The transfer of a task's file, if one was started. */
    fun ofTask(taskId: String, index: Long): Transfer? = _list.value.firstOrNull { it.task == TaskFile(taskId, index) }

    /**
     * A whole folder of the Mac, from its listing of every file under it ([Tree]): one card, its files fetched one
     * after another into `fetched/<batch>/<folder>/…` (removed at the lock, after a day and on Forget, as every fetched
     * file). Kb PHN-533.
     */
    fun fetchFolder(tree: Tree): String {
        val batch = "d${n.incrementAndGet()}-${System.nanoTime()}"
        val base = tree.path.trimEnd('/')
        val files = tree.files.mapIndexed { i, f ->
            val target = local.folderTarget(batch, tree.name, f.rel)
            Transfer("$batch.$i", Way.FETCH, f.rel.substringAfterLast('/'), target.path, "$base/${f.rel}", total = f.size.toLong(),
                phase = Phase.PAUSED, batch = batch, rel = f.rel)
        }
        _batches.update { it + (batch to Batch(batch, Way.FETCH, tree.name, tree.path, tree.skipped.toLong(), tree.cut)) }
        _list.update { it + files }
        runBatch(batch)
        return batch
    }

    private fun runFetch(key: String) = scope.launch { fetchOne(key) }

    private suspend fun fetchOne(key: String): Boolean {
        val t = get(key) ?: return false
        val attempt = "$key.${n.incrementAndGet()}"
        attempts[key] = attempt
        update(key) { it.copy(phase = Phase.RUNNING, problem = null) }
        val listener = listener(key, null)
        val task = t.task
        val r = if (task != null) {
            // the Mac takes no offset for a task's file: it starts over
            update(key) { it.copy(done = 0) }
            unlocker.run { api.fetchTaskFile(attempt, task.taskId, task.index, t.local, listener) }
        } else {
            unlocker.withProof(t.keyPlace, Why.FILE) { signer ->
                val offset = withContext(Dispatchers.IO) { File(t.local).let { if (it.isFile) it.length() else 0L } }
                api.fetch(attempt, t.macPath!!, t.local, offset, signer, listener)
            }
        }
        if (attempts[key] != attempt) return false
        attempts.remove(key)
        return when (r) {
            is Outcome.Ok -> {
                update(key) { it.copy(phase = Phase.DONE, done = r.value.total.toLong(), total = r.value.total.toLong()) }
                true
            }
            is Outcome.Failed -> { ended(key, r.error); false }
        }
    }

    // ---- sending ----------------------------------------------------------------------------------------------------

    /** Sends staged copies to the Mac, one after another, as one batch. */
    fun send(files: List<Staged>, forChat: Boolean): String {
        val batch = "b${n.incrementAndGet()}-${System.nanoTime()}"
        _batches.update { it + (batch to Batch(batch, Way.SEND)) }
        _list.update { l ->
            l + files.mapIndexed { i, s -> Transfer("$batch.$i", Way.SEND, s.name, s.path, total = s.size, phase = Phase.PAUSED, batch = batch, forChat = forChat) }
        }
        runBatch(batch)
        return batch
    }

    /**
     * The files of a message he sent (kb PHN-691): one batch named by the message, each file keyed by its chip, going on
     * from its upload id when it had one (a send cut short, even across FERB closing). Already there: it goes on.
     */
    fun sendMessage(id: String, chips: List<Chip>) {
        if (_list.value.any { it.batch == id }) {
            if (_list.value.none { it.batch == id && attempts.containsKey(it.key) }) runBatch(id)
            return
        }
        _batches.update { it + (id to Batch(id, Way.SEND)) }
        _list.update { l ->
            l + chips.map { c -> Transfer("$id.${c.key}", Way.SEND, c.name, c.path, total = c.size, phase = Phase.PAUSED, uploadId = c.uploadId, batch = id, message = true) }
        }
        runBatch(id)
    }

    /** Stops the file of [batch] that is going (the rest wait with it). */
    fun pauseBatch(batch: String) {
        _list.value.firstOrNull { it.batch == batch && it.phase == Phase.RUNNING }?.let { pause(it.key) }
    }

    /** Stops [batch] and forgets it and its copies. */
    fun removeBatch(batch: String) {
        _list.value.firstOrNull { it.batch == batch }?.let { remove(it.key) }
    }

    /** A message's files all arrived and it was sent: its transfers go from the list (their copies already went). */
    fun forgetBatch(batch: String) {
        _list.update { l -> l.filterNot { it.batch == batch } }
        _batches.update { it - batch }
    }

    /**
     * Sends a picked folder whole (PHN-127: one folder in the Mac's upload place, each file's place inside it kept):
     * the Mac starts the folder (`putGroup`), then each file goes with its group and path, copied here only just before
     * it goes (a large folder never doubles its space at once). The folder rides with a message as one attachment, the
     * group's id. Kb PHN-534.
     */
    fun sendFolder(folder: PickedFolder, forChat: Boolean): String {
        val batch = "s${n.incrementAndGet()}-${System.nanoTime()}"
        val files = folder.files.mapIndexed { i, f ->
            val key = "$batch.$i"
            sources[key] = f.open
            Transfer(key, Way.SEND, f.rel.substringAfterLast('/'), local.sendTarget(batch, f.rel).path, total = f.size ?: 0,
                phase = Phase.PAUSED, batch = batch, forChat = forChat, rel = f.rel)
        }
        _batches.update { it + (batch to Batch(batch, Way.SEND, folder.name)) }
        _list.update { it + files }
        runBatch(batch)
        return batch
    }

    private fun runBatch(batch: String) = scope.launch {
        for (t in _list.value.filter { it.batch == batch }) {
            val cur = get(t.key) ?: continue
            if (cur.phase == Phase.DONE) continue
            val ok = if (cur.way == Way.FETCH) fetchOne(cur.key) else sendOne(cur.key, batch)
            if (!ok && get(cur.key) != null) return@launch
        }
        val all = _list.value.filter { it.batch == batch }
        if (all.isNotEmpty() && all.all { it.phase == Phase.DONE } && all.first().forChat && !all.first().message) attach(batch)
    }

    private suspend fun sendOne(key: String, batch: String): Boolean {
        val b = _batches.value[batch]
        var group = b?.group
        if (b?.folder != null && group == null) {
            when (val g = unlocker.run { api.putGroup(b.folder) }) {
                is Outcome.Ok -> { group = g.value; setBatch(batch) { it.copy(group = g.value) } }
                is Outcome.Failed -> { ended(key, g.error); return false }
            }
        }
        var cur = get(key) ?: return false
        val source = sources[key]
        if (source != null && !withContext(Dispatchers.IO) { File(cur.local).isFile }) {
            // a folder's file is copied only now; one that can't be read is left out and counted
            if (!withContext(Dispatchers.IO) { local.stageTo(File(cur.local), source) }) {
                sources.remove(key)
                _list.update { l -> l.filterNot { it.key == key } }
                setBatch(batch) { it.copy(unreadable = it.unreadable + 1) }
                return true
            }
            update(key) { it.copy(uploadId = null, total = File(it.local).length()) }
            cur = get(key) ?: return false
        }
        val attempt = "$key.${n.incrementAndGet()}"
        attempts[key] = attempt
        update(key) { it.copy(phase = Phase.RUNNING, problem = null) }
        val r = unlocker.run { api.upload(attempt, cur.local, cur.name, get(key)?.uploadId, group, cur.rel, listener(key, attempt)) }
        if (attempts[key] != attempt) return false
        attempts.remove(key)
        return when (r) {
            is Outcome.Ok -> {
                update(key) { it.copy(phase = Phase.DONE, done = r.value.size.toLong(), total = r.value.size.toLong(), uploadId = r.value.uploadId,
                    macPlace = UploadPlace.shown(r.value)) }
                sources.remove(key)
                withContext(Dispatchers.IO) { local.remove(cur.local) }
                true
            }
            is Outcome.Failed -> { ended(key, r.error); false }
        }
    }

    /** The batch's files go to the composer, to send with a message (`sendWith`): a folder as one, by its group. */
    fun attach(batch: String) {
        val b = _batches.value[batch]
        val all = _list.value.filter { it.batch == batch && it.phase == Phase.DONE && it.uploadId != null }
        val add = if (b?.folder != null && b.group != null) listOf(Attachment(b.group, b.folder)) else all.map { Attachment(it.uploadId!!, it.name) }
        _ready.update { r -> r + add.filter { a -> r.none { it.uploadId == a.uploadId } } }
        all.firstNotNullOfOrNull { it.macPlace }?.let { _readyPlace.value = it }
        _list.update { l -> l.filterNot { it.batch == batch } }
        _batches.update { it - batch }
        local.removeBatch(batch)
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
            val have = File(t.local).let { if (t.way == Way.FETCH && t.task == null && it.isFile) it.length() else t.done }
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
        if (t.batch != null) {
            if (_list.value.any { it.batch == t.batch && attempts.containsKey(it.key) }) return
            runBatch(t.batch)
        } else {
            if (attempts.containsKey(key)) return
            runFetch(key)
        }
    }

    /** Stops and forgets a transfer (its whole batch, for files moving together) and its files on the phone. */
    fun remove(key: String) {
        val t = get(key) ?: return
        val gone = if (t.batch != null) _list.value.filter { it.batch == t.batch } else listOf(t)
        _list.update { l -> l - gone.toSet() }
        t.batch?.let { b -> _batches.update { it - b } }
        scope.launch {
            for (g in gone) {
                sources.remove(g.key)
                attempts.remove(g.key)?.let { api.cancelTransfer(it) }
                withContext(Dispatchers.IO) { local.remove(g.local) }
            }
            t.batch?.let { b -> withContext(Dispatchers.IO) { local.removeBatch(b) } }
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
        _batches.update { m -> m.filterValues { it.way == Way.SEND } }
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
    internal fun setForTest(list: List<Transfer>, batches: List<Batch> = emptyList()) {
        _list.value = list
        _batches.value = batches.associateBy { it.key }
    }

    /** Forget this Mac. */
    fun clear() {
        for (a in attempts.values) scope.launch { api.cancelTransfer(a) }
        attempts.clear()
        sources.clear()
        _list.value = emptyList()
        _batches.value = emptyMap()
        _ready.value = emptyList()
        local.wipeAll()
    }
}
