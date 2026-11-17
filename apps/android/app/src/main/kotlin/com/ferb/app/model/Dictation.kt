package com.ferb.app.model

/**
 * Speaking to FERB from the phone (kb PHN-603..PHN-605): hold the microphone in the composer and speak; the words appear
 * in the message box as they are heard and stay there when the owner lets go, for him to check and send. Never sent by
 * itself. This file is the part with no Android in it: FERB.app's own dictation model (NotchCore Dictation.swift,
 * APP-673 and APP-675) ported line for line, and the controller that runs one dictation over the phone's recogniser.
 * The iPhone's twin is FerbAppModel Dictation.swift; both run test resources dictation-cases.json.
 */

/** One answer of the speech recogniser, as plain facts. */
data class RecognizedSpeech(
    val text: String,
    /** The recognition task is done: nothing more comes from it. */
    val isFinal: Boolean = false,
    /** The recogniser marked the end of an utterance: its later answers can start again without these words. */
    val utteranceEnded: Boolean = false,
    /** Where the first word starts and the last ends, in seconds of the task's audio; null without timing. */
    val start: Double? = null,
    val end: Double? = null,
)

/**
 * Everything the owner said in one dictation (APP-673). Each recognition task has its own slot, an ended utterance is
 * kept, and the live answer only ever replaces the words of the utterance still being heard. When it can't be told
 * whether an answer revises or starts over, the words are kept: a repeated word in the draft the owner reviews costs
 * less than a lost one.
 */
class DictationTranscript {
    private class Slot {
        val kept = ArrayList<String>()
        var live = ""
        var liveEnd: Double? = null

        fun receive(answer: RecognizedSpeech) {
            var incoming = answer.text.trim()
            // a recogniser that repeats the utterances it already ended at the front of later answers: only the rest is new
            if (kept.isNotEmpty()) remainder(incoming, kept.joinToString(" "))?.let { incoming = it }
            if (incoming.isNotEmpty()) {
                if (live.isNotEmpty() && startedOver(live, liveEnd, incoming, answer.start)) kept.add(live)
                live = incoming
                liveEnd = answer.end
            }
            if (answer.isFinal || answer.utteranceEnded) {
                if (live.isNotEmpty()) kept.add(live)
                live = ""
                liveEnd = null
            }
        }
    }

    private val slots = ArrayList<Slot>()

    val text: String get() = slots.flatMap { it.kept + it.live }.filter { it.isNotEmpty() }.joinToString(" ")

    /** A new recognition task: its words go after every earlier task's. */
    fun beginTask(): Int { slots.add(Slot()); return slots.size - 1 }

    fun receive(answer: RecognizedSpeech, task: Int) { slots.getOrNull(task)?.receive(answer) }

    companion object {
        private fun words(s: String) = s.split(Regex("\\s+")).filter { it.isNotEmpty() }

        /** Lowercased letters and digits only (Swift's CharacterSet.alphanumerics, the Mac's normalisation). */
        fun normalized(word: String): String {
            val out = StringBuilder()
            var i = 0
            while (i < word.length) {
                val cp = word.codePointAt(i)
                if (Character.isLetterOrDigit(cp) || Character.getType(cp).let { it == Character.NON_SPACING_MARK.toInt() ||
                        it == Character.COMBINING_SPACING_MARK.toInt() || it == Character.ENCLOSING_MARK.toInt() || it == Character.LETTER_NUMBER.toInt() ||
                        it == Character.OTHER_NUMBER.toInt() }) out.appendCodePoint(cp)
                i += Character.charCount(cp)
            }
            return out.toString().lowercase()
        }

        /** The words of [text] after [prefix], when [text] begins with [prefix]'s words (case and punctuation aside). */
        fun remainder(text: String, prefix: String): String? {
            val tokens = words(text)
            val head = words(prefix).map(::normalized)
            if (tokens.size < head.size || !tokens.zip(head).all { (t, h) -> normalized(t) == h }) return null
            return tokens.drop(head.size).joinToString(" ")
        }

        /**
         * The answer no longer holds the live words: it starts on audio after they ended, or it is a fraction of them
         * with a different first word (a revision rewrites the last words, not the whole utterance at once).
         */
        fun startedOver(live: String, end: Double?, incoming: String, start: Double?): Boolean {
            if (start != null && end != null && end > 0 && start >= end) return true
            val old = words(live).map(::normalized)
            val new = words(incoming).map(::normalized)
            return new.size * 2 < old.size && new.firstOrNull() != old.firstOrNull()
        }
    }
}

/**
 * One dictation's capture (APP-675): which recognition task takes the audio, and what happens when one ends. Only the
 * owner ends a dictation (letting go, leaving the app, the lock); a task the recogniser ends while he still holds (its
 * final answer after a pause, "no speech", an error after it heard words) is replaced by a new one, and its words stay.
 */
class DictationCapture {
    sealed interface Step {
        data object KeepListening : Step
        /** Start a new recognition task. */
        data object Restart : Step
        /** The dictation is over, with everything said. */
        data class Finished(val text: String) : Step
        /** A fresh task failed before hearing anything, with nothing said yet. */
        data object Failed : Step
    }

    val transcript = DictationTranscript()

    /** The recognition task taking the audio now; null between tasks. */
    var task: Int? = null
        private set
    var finishing = false
        private set
    private var taskHeard = false

    val text: String get() = transcript.text

    fun beginTask(): Int = transcript.beginTask().also { task = it; taskHeard = false }

    /** The task stops being fed; its words stay and any it still sends are kept. */
    fun leaveTask() { task = null }

    fun receive(answer: RecognizedSpeech, task: Int): Step {
        transcript.receive(answer, task)
        if (task != this.task) return Step.KeepListening
        taskHeard = true
        return if (answer.isFinal) ended() else Step.KeepListening
    }

    /** The task ended with an error; [noSpeech]: the recogniser heard no speech in its audio (the owner paused). */
    fun fail(task: Int, noSpeech: Boolean): Step {
        if (task != this.task) return Step.KeepListening
        if (finishing || noSpeech || taskHeard) return ended()
        // a new task that fails at once would fail again: stop, keeping what was said
        this.task = null
        return if (text.isEmpty()) Step.Failed else Step.Finished(text)
    }

    /** The owner let go: the current task's last answer completes the dictation. */
    fun finish(): Step {
        finishing = true
        return if (task == null) Step.Finished(text) else Step.KeepListening
    }

    private fun ended(): Step {
        task = null
        return if (finishing) Step.Finished(text) else Step.Restart
    }
}

/**
 * Where this phone turns speech into words: on the phone itself, through the phone's own speech service (on Android
 * usually Google's, on iPhone Apple's: it may send the sound of the voice to that service), or nowhere. kb PHN-604.
 */
enum class SpeechKind { ON_DEVICE, SERVICE, NONE }

/** One event from the phone's recogniser for one recognition task. */
sealed interface VoiceEvent {
    val task: Int
    data class Heard(override val task: Int, val speech: RecognizedSpeech) : VoiceEvent
    /**
     * The task ended with an error. [noSpeech]: it heard nothing (a pause); [languageMissing]: the phone has no on-device
     * speech for the language (the next dictation uses the phone's service, after its words); [busy]: the recogniser
     * is in use elsewhere.
     */
    data class Failed(override val task: Int, val noSpeech: Boolean, val languageMissing: Boolean = false, val code: Int = 0) : VoiceEvent

    /** The recogniser is ready for speech (kb PHN-643): until then the owner sees "Getting ready…". */
    data class Ready(override val task: Int) : VoiceEvent

    /** How loud the microphone hears him now, 0..1 (kb PHN-643): drawn as the ring around the microphone. */
    data class Level(override val task: Int, val level: Float) : VoiceEvent
}

/**
 * The phone's recogniser (AndroidSpeech on a phone; a fake in tests). Called on the main thread; [startTask]'s events
 * come on the main thread. Nothing is recorded: the microphone's sound goes only to the recogniser, never to a file,
 * the store, a log or the Mac.
 */
interface VoiceInput {
    /** What this phone offers now. */
    fun kind(): SpeechKind

    /** A new recognition task (numbered by the capture) takes the microphone; an earlier one is let go. */
    fun startTask(kind: SpeechKind, task: Int, events: (VoiceEvent) -> Unit)

    /** The owner let go: the current task stops listening and gives its last answer. */
    fun finishAudio()

    /** Everything stops now; no more events. */
    fun stop()
}

/**
 * One dictation from press to the words left in the message box (the Mac's Microphone.swift, its recogniser behind
 * [VoiceInput]). [words] gets everything said so far after each answer; [ended] once, with the words or why there
 * are none. After the owner lets go the last answer completes it, or what was said so far after [graceMs].
 */
class Dictation(
    private val input: VoiceInput,
    private val kind: SpeechKind,
    private val words: (String) -> Unit,
    private val ended: (End) -> Unit,
    /** Runs a block after a delay (a coroutine on the main thread; tests run it by hand); returns its cancel. */
    private val later: (Long, () -> Unit) -> () -> Unit,
    private val graceMs: Long = GRACE_MS,
    /** The recogniser became ready for speech (each task; kb PHN-643). */
    private val ready: () -> Unit = {},
    /** How loud the microphone hears him, 0..1. */
    private val level: (Float) -> Unit = {},
) {
    sealed interface End {
        data class Words(val text: String) : End
        /** Nothing was heard and the recogniser failed: [languageMissing] when the phone lacks on-device speech for it. */
        data class Failed(val languageMissing: Boolean, val code: Int) : End
    }

    private val capture = DictationCapture()
    private var over = false
    private var grace: (() -> Unit)? = null

    val text: String get() = capture.text
    val running: Boolean get() = !over

    /** The owner let go and the last words are on their way (at most [graceMs]). */
    val finishing: Boolean get() = capture.finishing

    fun start() = startTask()

    private fun startTask() {
        val task = capture.beginTask()
        input.startTask(kind, task) { e -> if (!over) event(e) }
    }

    private fun event(e: VoiceEvent) {
        val step = when (e) {
            is VoiceEvent.Heard -> capture.receive(e.speech, e.task).also { words(capture.text) }
            is VoiceEvent.Failed -> capture.fail(e.task, e.noSpeech).also { lastFailure = e }
            is VoiceEvent.Ready -> { if (e.task == capture.task) ready(); return }
            is VoiceEvent.Level -> { if (e.task == capture.task) level(e.level); return }
        }
        when (step) {
            DictationCapture.Step.KeepListening -> Unit
            DictationCapture.Step.Restart -> startTask()
            is DictationCapture.Step.Finished -> done(End.Words(step.text))
            DictationCapture.Step.Failed -> done(End.Failed(lastFailure?.languageMissing == true, lastFailure?.code ?: 0))
        }
    }

    private var lastFailure: VoiceEvent.Failed? = null

    /** The owner let go. */
    fun finish() {
        if (over) return
        val step = capture.finish()
        if (step is DictationCapture.Step.Finished) { done(End.Words(step.text)); return }
        input.finishAudio()
        grace = later(graceMs) { if (!over) done(End.Words(capture.text)) }
    }

    /** Leaving the app, the lock, a new dictation: stops now, keeping what was said. */
    fun cancel() { if (!over) done(End.Words(capture.text)) }

    private fun done(end: End) {
        over = true
        grace?.invoke()
        grace = null
        input.stop()
        ended(end)
    }

    companion object {
        /** How long the last answer may take after the owner lets go (the Mac's 2 s). */
        const val GRACE_MS = 2_000L
    }
}
