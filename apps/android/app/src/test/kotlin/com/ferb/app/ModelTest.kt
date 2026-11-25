package com.ferb.app

import com.ferb.app.model.AnswerFlow
import com.ferb.app.model.Answered
import com.ferb.app.model.AppLock
import com.ferb.app.model.Corpus
import com.ferb.app.model.Moment
import com.ferb.app.model.Question
import com.ferb.core.KeyHealth
import com.ferb.core.KeyKind
import com.ferb.core.KeyState
import com.ferb.core.Outcome
import com.ferb.design.sky.SkyMoment
import com.ferb.design.sky.SkyPlace
import com.ferb.mobile.AnswerInput
import com.ferb.mobile.FerbException
import java.io.File
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AppLockTest {
    private var t = 0L
    private val lock = AppLock { t }

    // decision: PHN-835, PHN-15, PHN-794
    @Test
    fun locked_on_a_cold_start_and_again_after_five_minutes_away() {
        assertTrue("a cold start is locked", lock.locked)
        lock.unlocked()
        lock.wentToBackground()
        t += AppLock.LOCK_AFTER_MS // exactly five minutes: still open
        lock.cameToFront()
        assertFalse(lock.locked)
        lock.wentToBackground()
        t += AppLock.LOCK_AFTER_MS + 1
        lock.cameToFront()
        assertTrue("away longer than five minutes", lock.locked)
    }

    // decision: PHN-615, PHN-616
    @Test
    fun one_prompt_is_due_per_arrival_in_front_while_locked() {
        assertFalse("a new process not yet in front", lock.promptDue)
        lock.cameToFront()
        assertTrue(lock.takePrompt())
        assertFalse("taken once", lock.takePrompt())
        lock.cameToFront() // told again while in front (a system sheet closed)
        assertFalse(lock.takePrompt())
        lock.promptAgain() // the taken prompt could not be shown
        assertTrue(lock.takePrompt())
        lock.unlocked()
        lock.lock() // the owner's own lock from inside
        assertFalse("his own lock waits for his tap", lock.takePrompt())
        lock.wentToBackground()
        lock.promptAgain()
        assertFalse("nothing is due while away", lock.promptDue)
        lock.cameToFront()
        assertTrue("locked and back: due again", lock.takePrompt())
        lock.unlocked()
        lock.wentToBackground()
        t += 60_000
        lock.cameToFront()
        assertFalse("open again inside five minutes: nothing due", lock.promptDue)
        lock.wentToBackground()
        t += AppLock.LOCK_AFTER_MS + 1
        assertTrue(lock.backgroundTimer())
        assertFalse("locked in the background: nothing shows while away", lock.promptDue)
        lock.cameToFront()
        assertTrue(lock.takePrompt())
    }

    // decision: PHN-15, PHN-794
    @Test
    fun the_background_timer_locks_only_when_the_time_has_passed_and_the_app_is_still_away() {
        lock.unlocked()
        assertFalse("in front: the timer does nothing", lock.backgroundTimer())
        lock.wentToBackground()
        t += 60_000
        assertFalse("a minute away: not yet", lock.backgroundTimer())
        t += AppLock.LOCK_AFTER_MS
        assertTrue("five minutes away: locks now", lock.backgroundTimer())
        assertTrue(lock.locked)
        assertFalse("already locked: once", lock.backgroundTimer())
    }
}

class QuestionTest {
    private fun q(options: List<String>, labels: List<String> = emptyList(), kind: String = "owner_question", capability: String? = "ferb.confirm") =
        Question("ap-1", "t-1", null, "Go ahead?", null, kind, capability, options, labels, kind in Question.SECRET_KINDS)

    @Test
    fun yes_no_is_a_plain_no_and_yes_as_option_one_like_the_mac() {
        val c = q(listOf("Yes", "No"), listOf("Change it once", "Don't")).choices
        assertEquals(listOf("Don't", "Change it once"), c.map { it.label })
        assertEquals(AnswerInput.Approve(false, null), c[0].input)
        assertEquals(AnswerInput.Approve(true, 1L), c[1].input)
    }

    @Test
    fun other_options_are_numbered_from_one_and_no_options_take_words() {
        assertEquals(listOf(1L, 2L, 3L), q(listOf("Use Codex", "Use Claude", "Stop")).choices.map { (it.input as AnswerInput.Approve).choice })
        assertTrue(q(emptyList()).choices.isEmpty())
        assertTrue("a private answer has no buttons", q(listOf("Yes", "No"), kind = "owner_login_code").choices.isEmpty())
    }

    /** An option the Mac adds after Yes and No (its own number) is offered under them; No stays a plain No. */
    // decision: PHN-607
    @Test
    fun an_extra_option_after_yes_and_no_keeps_the_sheet_and_its_own_number() {
        val q = q(listOf("Yes", "No", "Until I'm back"), listOf("Allow", "Don't", "Until I'm back at my Mac"))
        assertTrue(q.yesNo)
        val c = q.choices
        assertEquals(listOf("Don't", "Allow", "Until I'm back at my Mac"), c.map { it.label })
        assertEquals(listOf(AnswerInput.Approve(false, null), AnswerInput.Approve(true, 1L), AnswerInput.Approve(true, 3L)), c.map { it.input })
        assertFalse(q(listOf("Use Codex", "Use Claude", "Stop")).yesNo)
    }
}

class AnswerFlowTest {
    private val phone = FakePhone()
    private var keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK)
    private val signers = ArrayList<FakeSigner>()
    private fun flow(cancel: Boolean = false) = AnswerFlow(phone, { keys }) { k -> FakeSigner(k, cancel).also(signers::add) }

    // decision: PHN-417, PHN-470, PHN-503
    @Test
    fun a_routine_answer_goes_with_no_prompt() = runBlocking {
        assertEquals(Answered.Sent, flow().answer("ap-1", AnswerInput.Approve(true, 1)))
        assertEquals(listOf("answer:ap-1:false"), phone.calls)
        assertTrue(signers.isEmpty())
    }

    // decision: PHN-137, PHN-129, PHN-13, PHN-503
    @Test
    fun a_yes_the_mac_wants_proved_is_signed_once_by_the_fingerprint_key() = runBlocking {
        phone.answerScript.add(FakePhone.refused("unlock_required"))
        assertEquals(Answered.Sent, flow().answer("ap-1", AnswerInput.Approve(true, 1)))
        assertEquals(listOf("answer:ap-1:false", "answer:ap-1:true"), phone.calls)
        assertEquals(listOf(KeyKind.STRONG), signers.map { it.kind })
    }

    // decision: PHN-129
    @Test
    fun a_phone_without_a_fingerprint_signs_with_its_lock_key() = runBlocking {
        keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.NONE)
        phone.answerScript.add(FakePhone.refused("unlock_required"))
        assertEquals(Answered.Sent, flow().answer("ap-1", AnswerInput.Approve(true, 1)))
        assertEquals(listOf(KeyKind.LOCK), signers.map { it.kind })
    }

    // decision: PHN-133, PHN-503
    @Test
    fun changed_fingerprints_mean_pair_again_not_a_pin() = runBlocking {
        keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.GONE)
        phone.answerScript.add(FakePhone.refused("unlock_required"))
        assertEquals(Answered.NeedsPairingAgain, flow().answer("ap-1", AnswerInput.Approve(true, 1)))
        assertEquals(1, phone.calls.size)
    }

    // decision: PHN-125, PHN-13, PHN-503
    @Test
    fun a_closed_mac_window_unlocks_and_asks_once_more() = runBlocking {
        phone.answerScript.add(FakePhone.refused("locked"))
        assertEquals(Answered.Sent, flow().answer("ap-1", AnswerInput.Words("the blue one")))
        assertEquals(listOf("answer:ap-1:false", "unlock", "answer:ap-1:false"), phone.calls)
        // and never more than once
        phone.calls.clear()
        phone.answerScript.add(FakePhone.refused("locked")); phone.answerScript.add(FakePhone.refused("locked"))
        assertTrue(flow().answer("ap-2", AnswerInput.Words("x")) is Answered.Failed)
        assertEquals(3, phone.calls.size)
    }

    // decision: PHN-110, PHN-109
    // decision: PHN-536
    @Test
    fun a_private_answer_always_goes_signed_and_a_cancelled_prompt_sends_nothing() = runBlocking {
        val secret = com.ferb.mobile.SecretInput().apply { "123456".forEach { push(it.code.toUShort()) } }
        assertEquals(Answered.Sent, flow().answerHidden("ap-1", secret))
        assertEquals("hidden:ap-1", phone.calls.single())
        assertEquals(listOf(6), phone.hiddenUnits)
        val again = com.ferb.mobile.SecretInput().apply { push('1'.code.toUShort()) }
        assertEquals(Answered.NotSent, flow(cancel = true).answerHidden("ap-1", again))
        assertTrue("emptied whatever happened", secret.isEmpty() && again.isEmpty())
        secret.destroy(); again.destroy()
    }

    // decision: PHN-302
    @Test
    fun a_refusal_comes_back_with_the_macs_words() = runBlocking {
        phone.answerScript.add(Outcome.Failed(FerbException.Refused("That question isn't waiting any more, so nothing was sent.", "question_not_waiting")))
        val r = flow().answer("ap-1", AnswerInput.Approve(true, 1)) as Answered.Failed
        assertEquals("question_not_waiting", (r.error as FerbException.Refused).linkCode)
    }
}

class WordsTest {
    private val corpus = Corpus(File("src/main/assets/corpus.json").readText())

    private fun moment(hour: Double, firstRun: Boolean = false, waiting: Int = 0, link: String = "direct", justPaired: Boolean = false, lastOpen: Double? = null): Moment {
        val sky = SkyMoment.at(SkyPlace.DELHI, 2026, 11, 14, hour)
        return Moment(sky, waiting, 0, 0, 0, link, firstRun, justPaired, lastOpen, "Asia/Kolkata")
    }

    // decision: PHN-507
    @Test
    fun the_first_run_and_the_first_open_after_pairing_have_their_own_words() {
        val first = corpus.texts(moment(10.0, firstRun = true))
        assertTrue(first.greeting in listOf("Hello", "Hi, I'm FERB"))
        assertEquals("Pair with your Mac and ask for anything. It happens there.", first.subline)
        val paired = corpus.texts(moment(10.0, justPaired = true))
        assertTrue(paired.greeting in listOf("All set", "You're connected"))
    }

    // decision: PHN-507
    @Test
    fun a_mac_that_cant_be_reached_is_said_and_the_same_line_holds_all_day() {
        val a = corpus.texts(moment(9.0, link = "offline"))
        assertTrue(a.subline.contains("Mac"))
        assertEquals(a.subline, corpus.texts(moment(16.0, link = "offline")).subline)
    }

    // decision: PHN-507
    @Test
    fun conditions_the_phone_does_not_measure_never_hold() {
        // weather is paused (PHN-412) and the owner's habits are not kept: no rule on them can speak
        assertFalse(Corpus.holds("weather", "storm", null))
        assertTrue(Corpus.holds("hour", "22-4", 23.5))
        assertTrue(Corpus.holds("hour", "22-4", 3.0))
        assertFalse(Corpus.holds("hour", "22-4", 4.0))
        assertTrue(Corpus.holds("awayHours", ">=168", 200.0))
        assertTrue(Corpus.holds("phase", listOf("dawn", "sunrise"), "dawn"))
        assertNull(Corpus(File("src/main/assets/corpus.json").readText()).line("nothing", moment(10.0)))
        assertNotNull(corpus.texts(moment(2.0)).greeting)
    }
}

class MacSettingsTest {
    /** Jev is gone for good: an older Mac that still sends its day count gets no row, and no word of the app names it. */
    // decision: PHN-596
    @Test
    fun jev_is_never_shown_even_when_an_older_mac_sends_it() {
        val rows = com.ferb.app.model.MacSettings.rows(listOf(
            com.ferb.mobile.SettingValue("retention.trace_days", "14"),
            com.ferb.mobile.SettingValue("retention.decision_days", "30"),
        ))
        assertEquals(listOf("retention.trace_days"), rows.map { it.spec.key })
        assertNull(com.ferb.app.model.MacSettings.spec("retention.decision_days"))
        val words = File("src/main/res/values/strings.xml").readText()
        assertFalse("strings.xml names Jev", Regex("jev", RegexOption.IGNORE_CASE).containsMatchIn(words))
    }
}

class UploadPlaceTest {
    /** The Mac's words for where a sent file landed; nothing said, the screen's default place. Never the full path. */
    // decision: PHN-608
    @Test
    fun the_place_is_the_macs_words_or_nothing() {
        assertEquals("Documents › From my phone",
            com.ferb.app.model.UploadPlace.shown(com.ferb.mobile.Uploaded("u-1", "/Users/someone/Documents/From my phone/u-1-a.pdf", 3u, "Documents › From my phone")))
        assertNull(com.ferb.app.model.UploadPlace.shown(com.ferb.mobile.Uploaded("u-1", "/Users/someone/Downloads/FERB from phone/u-1-a.pdf", 3u, null)))
        assertNull(com.ferb.app.model.UploadPlace.shown(com.ferb.mobile.Uploaded("u-1", "/x", 3u, " ")))
    }
}
