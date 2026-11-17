package com.ferb.app

import android.content.Context
import android.content.Intent
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Build
import android.os.Bundle
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import com.ferb.app.model.RecognizedSpeech
import com.ferb.app.model.SpeechKind
import com.ferb.app.model.VoiceEvent
import com.ferb.app.model.VoiceInput

/**
 * Android's speech recogniser behind [VoiceInput] (kb PHN-604). Where the phone offers recognition on the phone itself
 * (Android 12+, `SpeechRecognizer.isOnDeviceRecognitionAvailable`) that one is used and nothing leaves the phone. Where
 * it doesn't, the phone's own speech service is used (on most phones Google's), asked to prefer working offline
 * (`EXTRA_PREFER_OFFLINE`, a hint the service may ignore): it may send the sound of the voice to that service. The
 * owner is told which before his first use of each (Sheet.Voice). FERB never records: no audio source of its own,
 * nothing written, nothing to the Mac. Main thread only, as SpeechRecognizer requires.
 */
class AndroidSpeech(private val context: Context) : VoiceInput {
    private var recognizer: SpeechRecognizer? = null
    private val audio = context.getSystemService(AudioManager::class.java)

    /** Music or a video pauses while FERB listens (kb PHN-643), and plays on after. */
    private val focus = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_EXCLUSIVE)
        .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_ASSISTANT).setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
        .setOnAudioFocusChangeListener { }
        .build()

    /** The phone said on-device speech has no pack for this language: the service is used from now on. */
    var onDeviceMissing = false

    override fun kind(): SpeechKind = when {
        !onDeviceMissing && Build.VERSION.SDK_INT >= 31 && SpeechRecognizer.isOnDeviceRecognitionAvailable(context) -> SpeechKind.ON_DEVICE
        SpeechRecognizer.isRecognitionAvailable(context) -> SpeechKind.SERVICE
        else -> SpeechKind.NONE
    }

    override fun startTask(kind: SpeechKind, task: Int, events: (VoiceEvent) -> Unit) = startTask(kind, task, events, retried = false)

    private fun startTask(kind: SpeechKind, task: Int, events: (VoiceEvent) -> Unit, retried: Boolean) {
        release()
        runCatching { audio?.requestAudioFocus(focus) }
        var ready = false
        val r = if (kind == SpeechKind.ON_DEVICE && Build.VERSION.SDK_INT >= 31) SpeechRecognizer.createOnDeviceSpeechRecognizer(context)
        else SpeechRecognizer.createSpeechRecognizer(context)
        recognizer = r
        r.setRecognitionListener(object : RecognitionListener {
            override fun onPartialResults(partialResults: Bundle?) { heard(partialResults, false)?.let(events) }
            override fun onResults(results: Bundle?) {
                events(heard(results, true) ?: VoiceEvent.Heard(task, RecognizedSpeech("", isFinal = true)))
            }
            override fun onError(error: Int) {
                // the recogniser still busy with the last task, or its client not yet let go: one quiet new try
                // (kb PHN-643), never a failure the owner sees for that alone
                if (!ready && !retried && (error == SpeechRecognizer.ERROR_RECOGNIZER_BUSY || error == SpeechRecognizer.ERROR_CLIENT)) {
                    com.ferb.app.AppLog.i("voice error $error before ready: one new try")
                    android.os.Handler(android.os.Looper.getMainLooper()).postDelayed({ if (recognizer === r) startTask(kind, task, events, retried = true) }, RETRY_MS)
                    return
                }
                val missing = error == SpeechRecognizer.ERROR_LANGUAGE_NOT_SUPPORTED || error == SpeechRecognizer.ERROR_LANGUAGE_UNAVAILABLE
                if (missing && kind == SpeechKind.ON_DEVICE) onDeviceMissing = true
                events(VoiceEvent.Failed(task, noSpeech = error == SpeechRecognizer.ERROR_NO_MATCH || error == SpeechRecognizer.ERROR_SPEECH_TIMEOUT,
                    languageMissing = missing && kind == SpeechKind.ON_DEVICE, code = error))
            }
            override fun onReadyForSpeech(params: Bundle?) { ready = true; events(VoiceEvent.Ready(task)) }
            override fun onBeginningOfSpeech() = Unit
            // rmsdB runs about -2 (quiet) to 10 (loud speech)
            override fun onRmsChanged(rmsdB: Float) = events(VoiceEvent.Level(task, ((rmsdB + 2f) / 12f).coerceIn(0f, 1f)))
            override fun onBufferReceived(buffer: ByteArray?) = Unit
            override fun onEndOfSpeech() = Unit
            override fun onEvent(eventType: Int, params: Bundle?) = Unit

            private fun heard(b: Bundle?, final: Boolean): VoiceEvent.Heard? {
                val text = b?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull() ?: return null
                return VoiceEvent.Heard(task, RecognizedSpeech(text, isFinal = final))
            }
        })
        val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH)
            .putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
            .putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
            .putExtra(RecognizerIntent.EXTRA_PREFER_OFFLINE, true)
            .putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 1)
            .putExtra(RecognizerIntent.EXTRA_CALLING_PACKAGE, context.packageName)
        r.startListening(intent)
    }

    override fun finishAudio() { recognizer?.stopListening() }


    override fun stop() {
        release()
        runCatching { audio?.abandonAudioFocusRequest(focus) }
    }

    private fun release() {
        recognizer?.let { runCatching { it.cancel() }; runCatching { it.destroy() } }
        recognizer = null
    }

    private companion object {
        const val RETRY_MS = 300L
    }
}
