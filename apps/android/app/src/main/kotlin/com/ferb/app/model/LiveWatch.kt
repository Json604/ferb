package com.ferb.app.model

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import com.ferb.app.FerbViewModel
import com.ferb.app.Note
import com.ferb.app.R
import com.ferb.core.Outcome
import com.ferb.mobile.PreviewFrame
import com.ferb.mobile.PreviewSink
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/** What the Watch live screen shows: the last frame (decoded in memory), FERB.app's state and line over it, or why it ended. */
data class LiveView(
    val taskId: String,
    val frame: Bitmap? = null,
    val state: String = "",
    val line: String? = null,
    val empty: String? = null,
    val ended: Note? = null,
    /** The Mac's window closed: a tap unlocks and watches again. */
    val needsUnlock: Boolean = false,
)

/**
 * Watch live (PHN-454, PHN-111): a task's live preview at the link's own pace, each frame decoded in memory and never
 * written anywhere (no file, no cache, no notification); the screen that shows it is always kept out of screenshots
 * (MainActivity, FLAG_SECURE whatever the owner's switch says). It stops when the screen goes, the app goes to the
 * background, or the app locks; on the public relay it is never asked for (the relay's rule is said instead).
 */
class LiveWatch(
    private val scope: CoroutineScope,
    private val api: PhoneApi,
    private val decode: (ByteArray) -> Bitmap? = { BitmapFactory.decodeByteArray(it, 0, it.size) },
) {
    private val _view = MutableStateFlow<LiveView?>(null)
    val view: StateFlow<LiveView?> = _view.asStateFlow()
    @Volatile private var live: Live? = null
    @Volatile private var generation = 0

    fun start(taskId: String) {
        stop(keepView = true)
        val gen = ++generation
        _view.value = LiveView(taskId)
        scope.launch {
            val sink = object : PreviewSink {
                override fun frame(frame: PreviewFrame) {
                    if (gen != generation) return
                    val jpeg = frame.jpeg
                    val picture = jpeg?.let { bytes -> decode(bytes).also { bytes.fill(0) } }
                    _view.update { v ->
                        v?.copy(
                            frame = when {
                                picture != null -> picture
                                frame.gone -> null
                                else -> v.frame
                            },
                            state = frame.state, line = frame.line, empty = frame.emptyText,
                        )
                    }
                }

                override fun ended(code: String?, detail: String?) {
                    if (gen != generation || code == null) return
                    _view.update { v ->
                        v?.copy(
                            ended = when (code) {
                                "needs_better_path" -> Note(R.string.not_on_shared_relay)
                                "needs_unlock" -> Note(R.string.live_needs_unlock)
                                "closed" -> Note(R.string.live_ended)
                                else -> Note(R.string.err_mac_said, detail.orEmpty())
                            },
                            needsUnlock = code == "needs_unlock",
                        )
                    }
                }
            }
            when (val r = api.watchLive(taskId, sink)) {
                is Outcome.Ok -> if (gen == generation) live = r.value else r.value.stop()
                is Outcome.Failed -> if (gen == generation) _view.update { it?.copy(ended = FerbViewModel.errorNote(r.error)) }
            }
        }
    }

    /** Ends the stream; the screen goes too unless [keepView]. The last frame is dropped with it. */
    fun stop(keepView: Boolean = false) {
        generation++
        live?.stop()
        live = null
        if (!keepView) _view.value = null
    }

    val showing: Boolean get() = _view.value != null
}
