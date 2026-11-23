package com.ferb.app.model

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import com.ferb.core.Outcome
import com.ferb.design.components.WorkWords
import com.ferb.mobile.Item
import com.ferb.mobile.PhoneStatus
import com.ferb.mobile.PreviewFrame
import com.ferb.mobile.PreviewSink
import com.ferb.mobile.TranscriptView
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

// The work row above the message box (kb PHN-725), FERB.app's "N active tasks" row (ChatBody.swift WorkRowView,
// TaskActivityBubble.swift): what runs, from the same live state the chat draws, and its round live pictures.

/** One running task as the work row and its short view show it. */
data class RunTask(
    val entry: String,
    val taskId: String?,
    val title: String,
    /** What it is doing now (FERB.app's TaskItem.currentLine): the library's `LiveView.current`. */
    val line: String,
    val waiting: Boolean,
    val paused: Boolean,
)

/** The work row: its words, whether anything runs (its spinner), since when (epoch ms), and the tasks with pictures. */
data class RunningUi(val words: WorkWords, val running: Boolean, val since: Long?, val tasks: List<RunTask>)

object RunningModel {
    private const val REFERENCE_UNIX = 978_307_200.0

    /**
     * FERB.app's work row from the chat's live state: a turn that became a task and has no answer yet runs; one that
     * waits on the owner's answer to go on is paused, one that only waits for a time is kept going (L31), one with an
     * open question waits for him (its line then amber, "waiting for you"). Null when nothing runs, waits or is kept.
     *
     * The tasks this chat doesn't show (a schedule's run) come from the Mac's live status ([status]'s `running`, pushed by
     * the link whenever it changes), never from the Tasks tab's last `tasks.list` ([mac]): that list is read again only
     * while the Tasks tab shows, so a task that was running when it was read stayed "1 active task" after it finished
     * (kb PHN-792, the owner's report of 2026-11-21). [mac] only says whether such a task is paused or waits for him.
     */
    fun of(view: TranscriptView?, status: PhoneStatus? = null, mac: List<com.ferb.mobile.TaskRow>? = null): RunningUi? {
        if (view == null && status == null) return null
        val turns = view?.items.orEmpty().mapNotNull { (it as? Item.Turn)?.turn }.associateBy { it.entry }
        val tasks = ArrayList<RunTask>()
        var active = 0
        var paused = 0
        var kept = 0
        var since: Double? = null
        for (l in view?.live.orEmpty()) {
            val line = l.current ?: continue
            val t = turns[l.entry] ?: continue
            when (l.state) {
                "paused" -> paused++
                "sleeping" -> kept++
                else -> {
                    active++
                    t.planAt?.let { since = minOf(since ?: it, it) }
                }
            }
            val title = WorkModel.title(t).ifEmpty { "A task from earlier" }
            if (l.state != "sleeping") tasks.add(RunTask(l.entry, t.taskId ?: t.card?.taskId ?: t.checklist?.taskId, title, line, l.state == "needs_you", l.state == "paused"))
        }
        // FERB.app counts every running task, not only this chat's: the Mac's live status adds the ones started elsewhere
        val here = tasks.mapNotNullTo(HashSet()) { it.taskId }
        val asking = status?.questions.orEmpty().mapNotNullTo(HashSet()) { it.taskId }
        for (r in status?.running.orEmpty()) {
            if (r.taskId.isEmpty() || r.taskId in here) continue
            val listed = mac?.firstOrNull { it.taskId == r.taskId }?.status
            val isPaused = listed == "paused"
            val waiting = listed == "needs_user" || r.taskId in asking
            if (isPaused) paused++ else active++
            val title = WorkModel.oneLine(r.name.ifEmpty { r.goal }).ifEmpty { "A task from earlier" }
            tasks.add(RunTask("mac:${r.taskId}", r.taskId, title, "", waiting, isPaused))
        }
        if (active + paused + kept == 0) return null
        return RunningUi(WorkWords.of(active, paused, kept), active > 0, since?.let { ((it + REFERENCE_UNIX) * 1000).toLong() }, tasks)
    }
}

/**
 * The work row's round live pictures (FERB.app's, APP-403, APP-900) under the preview's rules (PHN-454, PHN-444): only
 * while the row is on screen and the app unlocked and in front (the view model says when), never on the shared relay
 * (the library refuses it there: the row keeps its plain circles), at most one picture every [everyMs] per task, the
 * Mac leaving out a frame the phone already has. Each frame is decoded small, in memory only, and never written
 * anywhere; screenshots follow the owner's switch as every page does (kb PHN-810).
 */
class Thumbnails(
    private val scope: CoroutineScope,
    private val api: PhoneApi,
    private val everyMs: Long = EVERY_MS,
    private val decode: (ByteArray) -> ImageBitmap? = ::small,
) {
    private val _pictures = MutableStateFlow<Map<String, ImageBitmap>>(emptyMap())
    val pictures: StateFlow<Map<String, ImageBitmap>> = _pictures.asStateFlow()
    private val running = HashMap<String, Live>()
    @Volatile private var generation = 0

    /** Watches exactly [keys] ([key]: a task's or one step's picture), at most [MOST]; empty stops everything. */
    @Synchronized
    fun show(keys: List<String>) {
        val want = keys.take(MOST).toSet()
        for (id in running.keys - want) running.remove(id)?.stop()
        _pictures.update { it.filterKeys(want::contains) }
        if (want.isEmpty()) { generation++; return }
        val gen = generation
        for (id in want - running.keys) {
            running[id] = Live { }
            scope.launch {
                val sink = object : PreviewSink {
                    override fun frame(frame: PreviewFrame) {
                        if (gen != generation) return
                        val jpeg = frame.jpeg ?: run { if (frame.gone) _pictures.update { it - id }; return }
                        val picture = decode(jpeg)
                        jpeg.fill(0)
                        if (picture != null && synchronized(this@Thumbnails) { id in running }) _pictures.update { it + (id to picture) }
                    }

                    override fun ended(code: String?, detail: String?) {
                        synchronized(this@Thumbnails) { running.remove(id) }
                    }
                }
                val parts = id.split('\u001F', limit = 2)
                val asked = if (parts.size == 2) api.watchStepThumbnail(parts[0], parts[1], everyMs, sink) else api.watchThumbnail(id, everyMs, sink)
                when (val r = asked) {
                    is Outcome.Ok -> synchronized(this@Thumbnails) {
                        if (gen == generation && id in running) running[id] = r.value else r.value.stop()
                    }
                    is Outcome.Failed -> synchronized(this@Thumbnails) { running.remove(id) }
                }
            }
        }
    }

    companion object {
        /** At most one picture every 2 s per task (the library's least for the work row, `THUMBNAIL_LEAST_MS`). */
        const val EVERY_MS = 2_000L

        /** The most pictures asked for at once (the work row's two and the card rows on screen): their cost's bound. */
        const val MOST = 4

        /** A picture's key: the task's (the step that acted last), or one step's ([worker]: a card row's step task id). */
        fun key(taskId: String, worker: String? = null): String = if (worker == null) taskId else "$taskId\u001F$worker"

        /** The side a picture is decoded to at most: twice the round picture's 30 dp at the phone's density, more than enough. */
        private const val SIDE = 128

        fun small(jpeg: ByteArray): ImageBitmap? = runCatching {
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeByteArray(jpeg, 0, jpeg.size, bounds)
            var sample = 1
            while (bounds.outWidth / (sample * 2) >= SIDE && bounds.outHeight / (sample * 2) >= SIDE) sample *= 2
            BitmapFactory.decodeByteArray(jpeg, 0, jpeg.size, BitmapFactory.Options().apply { inSampleSize = sample; inPreferredConfig = Bitmap.Config.RGB_565 })?.asImageBitmap()
        }.getOrNull()
    }
}
