package com.ferb.app

import com.ferb.app.model.DictationCapture
import com.ferb.app.model.RecognizedSpeech
import java.io.File
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * FERB.app's dictation model on the phone: every sequence of the shared table (test resources dictation-cases.json,
 * which the iPhone's DictationTests run too) through the Kotlin port. Robolectric only for org.json.
 */
@RunWith(RobolectricTestRunner::class)
@Config(application = android.app.Application::class, sdk = [35])
class DictationTest {
    // decision: PHN-605, APP-673, APP-675
    @Test
    fun every_shared_sequence_keeps_every_word_until_the_owner_lets_go() {
        val cases = JSONObject(File("src/test/resources/dictation-cases.json").readText()).getJSONArray("cases")
        assertTrue(cases.length() >= 8)
        for (i in 0 until cases.length()) {
            val case = cases.getJSONObject(i)
            val name = case.getString("name")
            val capture = DictationCapture()
            val steps = case.getJSONArray("steps")
            for (j in 0 until steps.length()) {
                val s = steps.getJSONObject(j)
                val step: DictationCapture.Step? = when {
                    s.has("begin") -> { assertEquals(name, s.getInt("begin"), capture.beginTask()); null }
                    s.has("heard") -> capture.receive(
                        RecognizedSpeech(s.getString("heard"), s.optBoolean("final"), s.optBoolean("ended"),
                            if (s.has("start")) s.getDouble("start") else null, if (s.has("end")) s.getDouble("end") else null),
                        s.getInt("task"))
                    s.has("fail") -> capture.fail(s.getInt("fail"), s.getBoolean("noSpeech"))
                    s.has("leave") -> { capture.leaveTask(); null }
                    s.has("finish") -> capture.finish()
                    s.has("text") -> { assertEquals(name, s.getString("text"), capture.text); null }
                    else -> error("unknown step in $name: $s")
                }
                if (s.has("step")) assertEquals("$name, step $j", s.getString("step"), words(step!!))
            }
        }
    }

    private fun words(step: DictationCapture.Step): String = when (step) {
        DictationCapture.Step.KeepListening -> "keep"
        DictationCapture.Step.Restart -> "restart"
        DictationCapture.Step.Failed -> "failed"
        is DictationCapture.Step.Finished -> "finished:${step.text}"
    }
}
