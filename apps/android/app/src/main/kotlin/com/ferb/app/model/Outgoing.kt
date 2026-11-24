package com.ferb.app.model

import com.ferb.app.FerbViewModel
import com.ferb.app.Note
import com.ferb.app.R
import com.ferb.core.Outcome
import com.ferb.mobile.Attachment
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.OutboxItem
import java.io.File
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * A file in the message box (kb PHN-690): picked and copied into the phone's own storage ([path]), nothing sent yet.
 * Once it goes, [uploadId] is the Mac's name for it (given when its upload starts, so a send cut short goes on where
 * it stopped) and [onMac] says it arrived whole.
 */
data class Chip(
    val key: String,
    val name: String,
    val size: Long,
    val path: String,
    val uploadId: String? = null,
    val onMac: Boolean = false,
)

/** Where a message with files stands (kb PHN-691). */
enum class SendPhase {
    /** The Mac can't be reached: it starts by itself when it can. */
    WAITING_LINK,

    /** Only the shared relay: files never go that way; it starts by itself on a direct path or his own relay. */
    WAITING_PATH,

    /** Its files are going to the Mac. */
    UPLOADING,

    /** Its files are there: the message itself is going. */
    SENDING,

    /** He stopped it. */
    STOPPED,

    /** FERB locked while its files went: it goes on by itself once he unlocks. */
    LOCKED,

    /** FERB closed (or the phone ended it) before it was sent. */
    CLOSED,

    /** It failed, for [Sending.note]'s reason. */
    FAILED,
}

/**
 * A message he sent with files, held on the phone until its files are on the Mac and the message went with them
 * (kb PHN-690, PHN-691). [onMac]: files that were already on the Mac (the Files tab's "Add a message"). [done] and
 * [total]: the bytes of its files so far.
 */
data class Sending(
    val id: String,
    val text: String,
    val chips: List<Chip>,
    val onMac: List<Attachment>,
    val phase: SendPhase,
    val note: Note? = null,
    val done: Long = 0,
    val total: Long = 0,
    /**
     * Sent in the temporary chat (the owner, 2026-11-24, ORCH-715: the same + and files as the chat): its files go the
     * same way, then it goes with `fresh` (never the outbox); it shows only in the temporary chat and goes when it ends.
     */
    val temporary: Boolean = false,
) {
    val fraction: Float get() = if (total > 0) (done.toFloat() / total).coerceIn(0f, 1f) else 0f
}

/**
 * The message box's files and the messages that carry files (kb PHN-690, PHN-691). A pick only copies the files here
 * and shows them in the box; nothing leaves the phone. Send makes the message a [Sending]: its files go to the Mac
 * through [Transfers] (one batch named by the message, with progress, stop and going on where it stopped), and when
 * they are all there the message goes with their ids (`sendWith`), as FERB.app's composer sends attached files.
 * The box's files and the messages under way are kept in `messages.json` beside the copies, so a message survives
 * FERB being closed. Every library call and every write runs on [io], never the main thread.
 */
class Outgoing(
    private val scope: CoroutineScope,
    private val api: PhoneApi,
    private val transfers: Transfers,
    private val local: LocalFiles,
    private val io: CoroutineDispatcher,
    /** A temporary chat's message went: the temporary chat as the Mac answered it. */
    private val sentTemporary: (com.ferb.mobile.TemporaryChat) -> Unit = {},
    /** A message went: the chat reads again (and the Mac may want its unlock). */
    private val sent: (OutboxItem) -> Unit,
) {
    private val _chips = MutableStateFlow<List<Chip>>(emptyList())
    val chips: StateFlow<List<Chip>> = _chips.asStateFlow()

    private val _messages = MutableStateFlow<List<Sending>>(emptyList())
    val messages: StateFlow<List<Sending>> = _messages.asStateFlow()

    private val n = AtomicInteger()
    private val file: File get() = File(local.root, "messages.json")

    init {
        scope.launch { transfers.list.collect { moved(it) } }
    }

    private fun get(id: String) = _messages.value.firstOrNull { it.id == id }
    private fun set(id: String, f: (Sending) -> Sending) = _messages.update { l -> l.map { if (it.id == id) f(it) else it } }

    // ---- the message box --------------------------------------------------------------------------------------------

    /** Picked files, already copied here: they wait in the box. */
    fun pick(staged: List<Staged>) {
        if (staged.isEmpty()) return
        _chips.update { it + staged.map { s -> Chip("c${System.nanoTime()}-${n.incrementAndGet()}", s.name, s.size, s.path) } }
        save()
    }

    /** The cross on a file in the box: it goes, and so does its copy. Nothing was sent. */
    fun unpick(key: String) {
        val c = _chips.value.firstOrNull { it.key == key } ?: return
        _chips.update { l -> l.filterNot { it.key == key } }
        scope.launch(io) { gone(c.path) }
        save()
    }

    /**
     * Send with the box's files: false when the box has none (the message then goes as before). The box empties; the
     * message shows in the chat and goes as soon as its files are on the Mac.
     */
    fun send(text: String, onMac: List<Attachment>, temporary: Boolean = false): Boolean {
        val chips = _chips.value
        if (chips.isEmpty()) return false
        val id = "m${System.currentTimeMillis()}-${n.incrementAndGet()}"
        _chips.value = emptyList()
        _messages.update { it + Sending(id, text, chips, onMac, SendPhase.UPLOADING, total = chips.sumOf { c -> c.size }, temporary = temporary) }
        start(id)
        return true
    }

    /** The temporary chat ended (its icon, on any device): its messages still on their way go, with their copies. */
    fun dropTemporary() {
        for (m in _messages.value.filter { it.temporary }) remove(m.id)
    }

    // ---- a message on its way -----------------------------------------------------------------------------------------

    /** Starts (or goes on with) a message: its files first, as far as the path allows. */
    fun start(id: String) {
        val m = get(id) ?: return
        val l = api.link.value
        val left = m.chips.filterNot { it.onMac }
        when {
            left.isEmpty() -> deliver(id)
            l?.phase != LinkPhase.ONLINE -> set(id) { it.copy(phase = SendPhase.WAITING_LINK, note = null) }
            !l.bulkAllowed -> set(id) { it.copy(phase = SendPhase.WAITING_PATH, note = null) }
            else -> {
                set(id) { it.copy(phase = SendPhase.UPLOADING, note = null) }
                transfers.sendMessage(id, left)
            }
        }
        save()
    }

    /**
     * The link moved: what waited for the Mac, or for a path that carries files, starts. (Files the path refused part
     * way are tried again by [Transfers.bulkAllowed] itself.)
     */
    fun linkChanged() {
        val l = api.link.value
        val started = transfers.list.value.mapNotNullTo(HashSet()) { it.batch }
        for (m in _messages.value) {
            when (m.phase) {
                SendPhase.WAITING_LINK -> if (l?.phase == LinkPhase.ONLINE) start(m.id)
                SendPhase.WAITING_PATH -> if (l?.phase == LinkPhase.ONLINE && l.bulkAllowed && m.id !in started) start(m.id)
                else -> Unit
            }
        }
    }

    /** FERB locked (PHN-15): files on their way stop (Transfers.locked); their messages go on after the unlock. */
    fun locked() {
        _messages.update { l -> l.map { if (it.phase == SendPhase.UPLOADING) it.copy(phase = SendPhase.LOCKED) else it } }
        save()
    }

    /** He unlocked: what the lock paused goes on. */
    fun unlocked() {
        for (m in _messages.value) if (m.phase == SendPhase.LOCKED) start(m.id)
    }

    /** Cancel while its files go: they stop where they are; the message stays, not sent, to try again or remove. */
    fun cancel(id: String) {
        set(id) { it.copy(phase = SendPhase.STOPPED, note = null) }
        transfers.pauseBatch(id)
        save()
    }

    fun retry(id: String) = start(id)

    /** Files can't go on this path: the words go now, without them. */
    fun textOnly(id: String) {
        val m = get(id) ?: return
        if (m.text.isBlank()) return
        set(id) { it.copy(phase = SendPhase.SENDING, note = null) }
        scope.launch {
            if (m.temporary) {
                when (val r = withContext(io) { api.sendTemporaryWith(m.text, m.onMac) }) {
                    is Outcome.Ok -> { drop(id); sentTemporary(r.value) }
                    is Outcome.Failed -> { set(id) { it.copy(phase = SendPhase.WAITING_PATH, note = FerbViewModel.errorNote(r.error)) }; save() }
                }
                return@launch
            }
            when (val r = withContext(io) { if (m.onMac.isEmpty()) api.send(m.text) else api.sendWith(m.text, m.onMac) }) {
                is Outcome.Ok -> { drop(id); sent(r.value) }
                is Outcome.Failed -> { set(id) { it.copy(phase = SendPhase.WAITING_PATH, note = FerbViewModel.errorNote(r.error)) }; save() }
            }
        }
    }

    /** Remove: the message, its files' copies and anything of them on its way. Nothing of it is sent. */
    fun remove(id: String) {
        transfers.removeBatch(id)
        drop(id)
    }

    private fun drop(id: String) {
        val m = get(id) ?: return
        _messages.update { l -> l.filterNot { it.id == id } }
        scope.launch(io) { m.chips.forEach { gone(it.path) } }
        save()
    }

    /** A copy goes; one already gone (sent and removed by its transfer, or by the lock) is no matter. */
    private fun gone(path: String) {
        runCatching { local.remove(path) }
    }

    /** Every file is on the Mac: the message goes with their ids. */
    private fun deliver(id: String) {
        val m = get(id) ?: return
        if (m.phase == SendPhase.SENDING) return
        set(id) { it.copy(phase = SendPhase.SENDING, note = null, done = it.total) }
        save()
        val files = m.onMac + m.chips.map { Attachment(it.uploadId!!, it.name) }
        scope.launch {
            if (m.temporary) {
                // the temporary chat's message: with `fresh`, never the outbox (the Mac moves its files into the
                // temporary chat's own folder and deletes them when it ends)
                when (val r = withContext(io) { api.sendTemporaryWith(m.text, files) }) {
                    is Outcome.Ok -> {
                        transfers.forgetBatch(id)
                        drop(id)
                        sentTemporary(r.value)
                    }
                    is Outcome.Failed -> { set(id) { it.copy(phase = SendPhase.FAILED, note = FerbViewModel.errorNote(r.error)) }; save() }
                }
                return@launch
            }
            when (val r = withContext(io) { api.sendWith(m.text, files) }) {
                is Outcome.Ok -> {
                    transfers.forgetBatch(id)
                    drop(id)
                    sent(r.value)
                }
                is Outcome.Failed -> { set(id) { it.copy(phase = SendPhase.FAILED, note = FerbViewModel.errorNote(r.error)) }; save() }
            }
        }
    }

    /** The transfers moved: each message's progress, its files' ids, and how its files stand. */
    private fun moved(list: List<Transfer>) {
        for (m in _messages.value) {
            val mine = list.filter { it.batch == m.id }
            if (mine.isEmpty()) continue
            var changed = false
            val chips = m.chips.map { c ->
                val t = mine.firstOrNull { it.key == "${m.id}.${c.key}" } ?: return@map c
                val id = t.uploadId ?: c.uploadId
                val on = c.onMac || t.phase == Phase.DONE
                if (id != c.uploadId || on != c.onMac) changed = true
                c.copy(uploadId = id, onMac = on)
            }
            val done = chips.sumOf { c -> if (c.onMac) c.size else mine.firstOrNull { it.key == "${m.id}.${c.key}" }?.done ?: 0 }
            // only a message whose files are going (or waiting for a path that carries them) follows its transfers;
            // one he stopped, or the lock paused, keeps its phase
            val phase = when {
                m.phase != SendPhase.UPLOADING && m.phase != SendPhase.WAITING_PATH -> m.phase
                chips.all { it.onMac } -> SendPhase.UPLOADING // delivered below
                mine.any { it.phase == Phase.FAILED } -> SendPhase.FAILED
                mine.any { it.phase == Phase.WAITING_PATH } -> SendPhase.WAITING_PATH
                mine.any { it.phase == Phase.RUNNING } -> SendPhase.UPLOADING
                else -> m.phase
            }
            val note = if (phase == SendPhase.FAILED) mine.firstOrNull { it.phase == Phase.FAILED }?.problem else m.note
            set(m.id) { it.copy(chips = chips, done = done, phase = phase, note = note) }
            if (changed || phase != m.phase) save()
            if ((m.phase == SendPhase.UPLOADING || m.phase == SendPhase.WAITING_PATH) && chips.all { it.onMac }) deliver(m.id)
        }
    }

    // ---- kept on the phone --------------------------------------------------------------------------------------------

    /**
     * Reads what was kept: the box's files, and the messages that were on their way. A message that was going when
     * FERB closed says so and waits for Try again (it goes on where its files stopped); one that waited for the Mac or
     * a path still waits. Files whose copy is gone (a day old, PHN-16) are left out of the box; a message that lost
     * one fails and says so. Blocking: call it on [io].
     */
    fun load() {
        val text = runCatching { file.takeIf { it.isFile }?.readText() }.getOrNull() ?: return
        val root = runCatching { Json.parse(text) as? Map<*, *> }.getOrNull() ?: return
        fun chip(v: Any?): Chip? {
            val o = v as? Map<*, *> ?: return null
            return Chip(o["key"] as? String ?: return null, o["name"] as? String ?: return null, (o["size"] as? Double)?.toLong() ?: 0,
                o["path"] as? String ?: return null, o["uploadId"] as? String, o["onMac"] == true)
        }
        val box = (root["chips"] as? List<*>).orEmpty().mapNotNull(::chip).filter { File(it.path).isFile }
        val messages = (root["messages"] as? List<*>).orEmpty().mapNotNull { v ->
            val o = v as? Map<*, *> ?: return@mapNotNull null
            val chips = (o["chips"] as? List<*>).orEmpty().mapNotNull(::chip)
            val onMac = (o["onMac"] as? List<*>).orEmpty().mapNotNull { a ->
                val x = a as? Map<*, *> ?: return@mapNotNull null
                Attachment(x["uploadId"] as? String ?: return@mapNotNull null, x["name"] as? String ?: "")
            }
            val was = runCatching { SendPhase.valueOf(o["phase"] as String) }.getOrDefault(SendPhase.CLOSED)
            val lost = chips.any { !it.onMac && !File(it.path).isFile }
            val phase = when {
                lost -> SendPhase.FAILED
                was == SendPhase.WAITING_LINK || was == SendPhase.WAITING_PATH || was == SendPhase.STOPPED -> was
                else -> SendPhase.CLOSED
            }
            Sending(o["id"] as? String ?: return@mapNotNull null, o["text"] as? String ?: "", chips, onMac, phase,
                note = if (lost) Note(R.string.send_files_gone) else null,
                done = chips.filter { it.onMac }.sumOf { it.size }, total = chips.sumOf { it.size })
        }
        _chips.value = box
        _messages.value = messages
    }

    private val saves = java.util.concurrent.atomic.AtomicLong()
    @Volatile private var written = 0L

    /** Writes the box and the messages off the main thread; a later save is never overwritten by an earlier one. */
    private fun save() {
        val box = _chips.value
        // a temporary chat's message is never written on the phone (memory only, as all of that chat): closing FERB
        // while its files go loses it
        val messages = _messages.value.filterNot { it.temporary }
        val seq = saves.incrementAndGet()
        scope.launch(io) {
            val out = StringBuilder("{\"chips\":[")
            box.joinTo(out, ",") { chipJson(it) }
            out.append("],\"messages\":[")
            messages.joinTo(out, ",") { m ->
                "{\"id\":${q(m.id)},\"text\":${q(m.text)},\"phase\":${q(m.phase.name)},\"chips\":[${m.chips.joinToString(",") { chipJson(it) }}]," +
                    "\"onMac\":[${m.onMac.joinToString(",") { "{\"uploadId\":${q(it.uploadId)},\"name\":${q(it.name)}}" }}]}"
            }
            out.append("]}")
            synchronized(this@Outgoing) {
                if (seq < written) return@synchronized
                written = seq
                runCatching {
                    local.root.mkdirs()
                    val tmp = File(local.root, "messages.json.tmp")
                    tmp.writeText(out.toString())
                    tmp.renameTo(file)
                }
            }
        }
    }

    private fun chipJson(c: Chip) =
        "{\"key\":${q(c.key)},\"name\":${q(c.name)},\"size\":${c.size},\"path\":${q(c.path)},\"uploadId\":${c.uploadId?.let(::q) ?: "null"},\"onMac\":${c.onMac}}"

    private fun q(s: String): String {
        val b = StringBuilder("\"")
        for (ch in s) when {
            ch == '"' -> b.append("\\\"")
            ch == '\\' -> b.append("\\\\")
            ch == '\n' -> b.append("\\n")
            ch == '\r' -> b.append("\\r")
            ch == '\t' -> b.append("\\t")
            ch < ' ' -> b.append(String.format("\\u%04x", ch.code))
            else -> b.append(ch)
        }
        return b.append('"').toString()
    }

    /** Forget this Mac: everything. */
    fun clear() {
        _chips.value = emptyList()
        _messages.value = emptyList()
        file.delete()
    }

    /** The screenshots' invented box and messages (nothing runs). */
    internal fun setForTest(chips: List<Chip>, messages: List<Sending> = emptyList()) {
        _chips.value = chips
        _messages.value = messages
    }
}
