package com.ferb.app.model

import android.graphics.Bitmap
import com.ferb.app.FerbViewModel
import com.ferb.app.Note
import com.ferb.app.R
import com.ferb.core.Outcome
import com.ferb.mobile.CameraFrame
import com.ferb.mobile.CameraSink
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/** What the camera view ("Your Mac's camera") shows: the last frame (decoded in memory), the Mac's state and words, or why it ended. */
data class CameraView(
    val frame: Bitmap? = null,
    /** starting, live, off, denied, failed (the Mac's). */
    val state: String = "starting",
    /** The Mac's words for the state; none: the app's "Starting your Mac's camera…" while there is no frame. */
    val text: String? = null,
    val ended: Note? = null,
    /** The app locked under it: a tap unlocks and looks again. */
    val needsUnlock: Boolean = false,
)

/**
 * Show me (APP-983): the Mac's camera while the camera view is on screen, and only then. Each frame is decoded in
 * memory and never written anywhere (no file, no cache, no notification); the view follows the owner's screenshot switch as every
 * page does (kb PHN-810; MainActivity's FLAG_SECURE). It stops when the view goes, the app goes to the
 * background or locks; the Mac turns the camera off a few seconds after the last ask. On the public relay the library
 * refuses it before anything is asked, and the view says the library's words.
 */
class CameraWatch(
    private val scope: CoroutineScope,
    private val api: PhoneApi,
    private val decode: (ByteArray) -> Bitmap? = ::atScreenSize,
) {
    private val _view = MutableStateFlow<CameraView?>(null)
    val view: StateFlow<CameraView?> = _view.asStateFlow()
    @Volatile private var live: Live? = null
    @Volatile private var generation = 0

    fun start() {
        stop(keepView = true)
        val gen = ++generation
        _view.value = CameraView()
        scope.launch {
            val sink = object : CameraSink {
                override fun frame(frame: CameraFrame) {
                    if (gen != generation) return
                    val picture = frame.jpeg?.let { bytes -> decode(bytes).also { bytes.fill(0) } }
                    _view.update { v ->
                        v?.copy(
                            // the Mac turned it off, was denied or failed: the last frame goes with it
                            frame = picture ?: v.frame.takeIf { frame.state == "live" || frame.state == "starting" },
                            state = frame.state, text = frame.text,
                        )
                    }
                }

                override fun ended(code: String?, detail: String?) {
                    if (gen != generation || code == null) return
                    _view.update { v ->
                        v?.copy(
                            frame = null,
                            ended = when (code) {
                                // the library's own words for the public relay (the link's rule)
                                "needs_better_path" -> detail?.let { Note(R.string.err_mac_said, it) } ?: Note(R.string.not_on_shared_relay)
                                "needs_unlock" -> Note(R.string.camera_needs_unlock)
                                "closed" -> Note(R.string.camera_ended)
                                else -> Note(R.string.err_mac_said, detail.orEmpty())
                            },
                            needsUnlock = code == "needs_unlock",
                        )
                    }
                }
            }
            when (val r = api.watchCamera(sink)) {
                is Outcome.Ok -> if (gen == generation) live = r.value else r.value.stop()
                is Outcome.Failed -> if (gen == generation) _view.update { it?.copy(ended = FerbViewModel.errorNote(r.error)) }
            }
        }
    }

    /** Ends the pulling; the view goes too unless [keepView]. The last frame is dropped with it. */
    fun stop(keepView: Boolean = false) {
        generation++
        live?.stop()
        live = null
        if (!keepView) _view.value = null else _view.update { it?.copy(frame = null) }
    }

    val showing: Boolean get() = _view.value != null
}
