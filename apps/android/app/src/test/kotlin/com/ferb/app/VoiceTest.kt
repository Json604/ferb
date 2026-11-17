package com.ferb.app

import android.app.Application
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.RecognizedSpeech
import com.ferb.app.model.SpeechKind
import com.ferb.app.model.VoiceEvent
import com.ferb.app.model.VoiceInput
import com.ferb.app.model.Why
import com.ferb.core.KeyKind
import java.nio.file.Files
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestCoroutineScheduler
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** A recogniser driven by the test: what it was asked, and the events it gives. */
class FakeVoice(var offers: SpeechKind = SpeechKind.ON_DEVICE) : VoiceInput {
    val tasks = ArrayList<Pair<SpeechKind, Int>>()
    var events: ((VoiceEvent) -> Unit)? = null
    var finished = 0
    var stopped = 0
    override fun kind() = offers
    override fun startTask(kind: SpeechKind, task: Int, events: (VoiceEvent) -> Unit) { tasks += kind to task; this.events = events }
    override fun finishAudio() { finished++ }
    override fun stop() { stopped++; events = null }
    fun heard(text: String, final: Boolean = false) = events!!(VoiceEvent.Heard(tasks.last().second, RecognizedSpeech(text, isFinal = final)))
    fun failed(noSpeech: Boolean = false, languageMissing: Boolean = false) =
        events!!(VoiceEvent.Failed(tasks.last().second, noSpeech, languageMissing))
}

/** Speaking to FERB from the phone (kb PHN-603..PHN-605): told first, asked in context, held, never sent by itself. */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class VoiceTest {
    private val phone = FakePhone()
    private val voice = FakeVoice()
    private var t = 0L
    private var mic = false
    private val scheduler = TestCoroutineScheduler()
    private lateinit var session: Session
    private val root = Files.createTempDirectory("ferb-voice").toFile()
    private lateinit var prefs: Prefs

    @Before
    fun setUp() {
        Dispatchers.setMain(StandardTestDispatcher(scheduler))
        session = Session { t }
        session.unlocked()
        session.foreground(true)
        prefs = Prefs(ApplicationProvider.getApplicationContext())
        prefs.voiceTold = emptySet()
    }

    @After
    fun tearDown() { Dispatchers.resetMain(); root.deleteRecursively() }

    private fun vm(input: VoiceInput? = voice) = FerbViewModel(phone, session, prefs, { true }, null, SavedStateHandle(), LocalFiles(root) { null },
        now = { t }, voice = input, micAllowed = { mic }).also {
        it.prompts = object : Prompts {
            override fun signer(kind: KeyKind, why: Why) = FakeSigner(kind, false)
            override suspend fun local() = true
        }
        scheduler.advanceUntilIdle()
    }

    // decision: PHN-603, PHN-604
    @Test
    fun the_first_press_says_what_happens_to_the_voice_then_android_asks_then_it_listens() {
        val vm = vm()
        assertEquals(Gate.MAIN, vm.ui.value.gate)
        assertEquals(VoicePress.DONE, vm.voicePress())
        assertEquals(Sheet.Voice(SpeechKind.ON_DEVICE), vm.sheet.value)
        assertTrue("nothing listens before the owner has read it", voice.tasks.isEmpty())
        vm.voiceTold(SpeechKind.ON_DEVICE)
        assertEquals(null, vm.sheet.value)
        assertEquals("Android's question comes next", VoicePress.ASK_MICROPHONE, vm.voicePress())
        assertTrue(voice.tasks.isEmpty())
        mic = true
        vm.microphoneAnswered(true)
        assertEquals(VoicePress.DONE, vm.voicePress())
        assertEquals(listOf(SpeechKind.ON_DEVICE to 0), voice.tasks)
        assertTrue(vm.listening.value)
    }

    // decision: PHN-603, PHN-605, APP-675
    @Test
    fun held_words_land_after_the_draft_and_stay_and_nothing_is_sent() {
        prefs.voiceTold = setOf(SpeechKind.ON_DEVICE.name); mic = true
        val vm = vm()
        vm.typed("Remind me")
        vm.voicePress()
        voice.heard("to water")
        assertEquals("Remind me to water", vm.draft.value)
        voice.heard("to water the plants.", final = true)                 // the recogniser ended a sentence mid-hold
        assertEquals("a new task takes the microphone", 2, voice.tasks.size)
        t += 4_000
        voice.heard("At six")
        vm.voiceRelease()
        assertEquals(1, voice.finished)
        assertTrue("still waiting for the last words", vm.listening.value)
        voice.heard("At six.", final = true)
        assertFalse(vm.listening.value)
        assertEquals("Remind me to water the plants. At six.", vm.draft.value)
        scheduler.advanceUntilIdle()
        assertTrue("never sent by itself", phone.calls.none { it.startsWith("send") })
    }

    // decision: PHN-603, PHN-605
    @Test
    fun without_a_last_answer_what_was_said_stays_after_the_grace() {
        prefs.voiceTold = setOf(SpeechKind.ON_DEVICE.name); mic = true
        val vm = vm()
        vm.voicePress()
        voice.heard("Book the train")
        t += 2_000
        vm.voiceRelease()
        assertTrue(vm.listening.value)
        scheduler.advanceTimeBy(2_001); scheduler.runCurrent()
        assertFalse(vm.listening.value)
        assertEquals("Book the train", vm.draft.value)
        assertTrue(voice.stopped >= 1)
    }

    // decision: PHN-603, PHN-605
    @Test
    fun a_quick_tap_says_to_hold_and_a_failure_leaves_the_draft() {
        prefs.voiceTold = setOf(SpeechKind.ON_DEVICE.name); mic = true
        val vm = vm()
        vm.typed("Hi")
        vm.voicePress()
        t += 100
        vm.voiceRelease()
        assertEquals(Note(R.string.voice_hold), vm.ui.value.note)
        assertFalse(vm.listening.value)
        assertEquals("Hi", vm.draft.value)
        vm.voicePress()
        voice.failed()                                                   // a fresh task fails before hearing anything
        assertFalse(vm.listening.value)
        assertEquals("Hi", vm.draft.value)
        assertEquals(Note(R.string.voice_failed), vm.ui.value.note)
    }

    // decision: PHN-604
    @Test
    fun no_on_device_speech_for_the_language_tells_the_owner_before_the_phones_service_is_used() {
        prefs.voiceTold = setOf(SpeechKind.ON_DEVICE.name); mic = true
        val vm = vm()
        vm.voicePress()
        voice.offers = SpeechKind.SERVICE                                 // AndroidSpeech switches after this error
        voice.failed(languageMissing = true)
        assertEquals(Sheet.Voice(SpeechKind.SERVICE), vm.sheet.value)
        vm.voiceTold(SpeechKind.SERVICE)
        vm.voicePress()
        assertEquals(SpeechKind.SERVICE, voice.tasks.last().first)
    }

    // decision: PHN-603, PHN-605, PHN-15
    @Test
    fun leaving_the_app_or_the_lock_stops_the_microphone_and_keeps_the_words() {
        prefs.voiceTold = setOf(SpeechKind.ON_DEVICE.name); mic = true
        val vm = vm()
        vm.voicePress()
        voice.heard("Call the bank")
        session.foreground(false)
        scheduler.advanceUntilIdle()
        assertFalse(vm.listening.value)
        assertTrue(voice.stopped >= 1)
        assertEquals("Call the bank", vm.draft.value)
    }

    // decision: PHN-603, PHN-605
    @Test
    fun a_phone_without_speech_recognition_says_so() {
        val vm = vm(FakeVoice(SpeechKind.NONE))
        vm.voicePress()
        assertEquals(Note(R.string.voice_unavailable), vm.ui.value.note)
        assertEquals(null, vm.sheet.value)
    }
}
