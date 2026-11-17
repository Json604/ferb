package com.ferb.app

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.speech.RecognitionService
import android.util.Log
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.ferb.app.model.SpeechKind
import com.ferb.app.model.VoiceEvent
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Speaking to FERB on a running emulator or phone (kb PHN-604, PHN-605): the speech services FERB may see (the manifest's
 * queries), which kind this phone offers, and a recognition task started and stopped on the main thread with the
 * microphone allowed. It hears nothing (no voice in the test); what a real phone recognises is the owner's live check.
 */
@RunWith(AndroidJUnit4::class)
class SpeechOnPhoneTest {
    private val context: Context = ApplicationProvider.getApplicationContext()
    private val instrumentation = InstrumentationRegistry.getInstrumentation()

    // decision: PHN-604, PHN-605
    @Test
    fun the_phones_speech_service_is_visible_and_a_task_starts_and_stops() {
        val services = context.packageManager.queryIntentServices(Intent(RecognitionService.SERVICE_INTERFACE), 0)
        val speech = AndroidSpeech(context)
        val kind = speech.kind()
        Log.i("FERBTest", "speech services: ${services.map { it.serviceInfo.packageName }}; FERB would use: $kind")
        assertEquals("FERB sees a speech service exactly when the phone has one", services.isNotEmpty(), kind != SpeechKind.NONE)
        if (kind == SpeechKind.NONE) return
        // granted for the test (revoking it here would end the test's own process; the app is uninstalled after)
        instrumentation.uiAutomation.grantRuntimePermission(context.packageName, Manifest.permission.RECORD_AUDIO)
        run {
            assertEquals(PackageManager.PERMISSION_GRANTED, context.checkSelfPermission(Manifest.permission.RECORD_AUDIO))
            val events = ArrayList<VoiceEvent>()
            val ended = CountDownLatch(1)
            instrumentation.runOnMainSync {
                speech.startTask(kind, 0) { e -> events += e; if (e is VoiceEvent.Failed || (e is VoiceEvent.Heard && e.speech.isFinal)) ended.countDown() }
            }
            Thread.sleep(1_500)
            instrumentation.runOnMainSync { speech.finishAudio() }
            val answered = ended.await(10, TimeUnit.SECONDS)
            Log.i("FERBTest", "after letting go: answered=$answered events=$events")
            instrumentation.runOnMainSync { speech.stop() }
            val after = events.size
            Thread.sleep(1_000)
            assertEquals("nothing comes after stop", after, events.size)
        }
    }
}
