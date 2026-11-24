package com.ferb.app

import android.app.Application
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.Why
import com.ferb.core.KeyKind
import com.ferb.core.Outcome
import com.ferb.mobile.AnswerInput
import com.ferb.mobile.FerbException
import com.ferb.mobile.OlderState
import com.ferb.mobile.PhoneStatus
import com.ferb.mobile.QuestionNeeds
import com.ferb.mobile.ScreenUse
import com.ferb.mobile.StatusQuestion
import com.ferb.mobile.TranscriptView
import java.nio.file.Files
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.UnconfinedTestDispatcher
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

/** "Until I'm back at my Mac" from the phone (kb PHN-610, PHN-611): offered only when the Mac offers it, sent as its own answer. */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class UntilBackTest {
    private val phone = FakePhone()
    private lateinit var session: Session
    private val signers = ArrayList<FakeSigner>()
    private val root = Files.createTempDirectory("ferb-until-back").toFile()

    @Before
    fun setUp() {
        Dispatchers.setMain(UnconfinedTestDispatcher())
        session = Session { 0L }
        session.unlocked()
    }

    @After
    fun tearDown() { Dispatchers.resetMain(); root.deleteRecursively() }

    private fun vm() = FerbViewModel(phone, session, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
        LocalFiles(root) { null }, account = testAccount()).also {
        it.prompts = object : Prompts {
            override fun signer(kind: KeyKind, why: Why) = FakeSigner(kind, false).also(signers::add)
            override suspend fun local() = true
        }
    }

    private fun screenQuestion(untilBack: Boolean) = StatusQuestion("ap-s", "approve", "screen.foreground", "t-1", "Use your screen in Keynote?",
        listOf("Yes", "No"), false, QuestionNeeds(true, false, false, false, "ch-1", null, untilBack))

    private fun status(q: StatusQuestion?, screenUse: ScreenUse? = null) =
        PhoneStatus(false, true, false, listOfNotNull(q), emptyList(), emptyList(), screenUse)

    // decision: PHN-610, PHN-611
    @Test
    fun offered_only_when_the_mac_offers_it() {
        phone.view = TranscriptView("c1", emptyList(), emptyList(), emptyList(), emptyList(), OlderState.Complete)
        phone.status = status(screenQuestion(untilBack = true))
        val vm = vm()
        assertTrue(vm.ui.value.questions.single().untilBack)
        phone.status = status(screenQuestion(untilBack = false))
        vm.refresh()
        assertFalse(vm.ui.value.questions.single().untilBack)
        // a question that isn't the gateway's Yes/No never gets it, whatever the flag
        val other = screenQuestion(true).copy(options = listOf("Small", "Large"))
        phone.status = status(other)
        vm.refresh()
        assertFalse(vm.ui.value.questions.single().untilBack)
    }

    // decision: PHN-611, PHN-13
    @Test
    fun chosen_while_the_macs_window_is_closed_it_unlocks_and_is_sent_once_more_without_a_fingerprint() {
        phone.view = TranscriptView("c1", emptyList(), emptyList(), emptyList(), emptyList(), OlderState.Complete)
        phone.status = status(screenQuestion(untilBack = true))
        val vm = vm()
        phone.answerScript.add(FakePhone.refused("locked"))
        vm.answer("ap-s", AnswerInput.UntilBack)
        assertEquals(listOf(AnswerInput.UntilBack, AnswerInput.UntilBack), phone.answers.map { it.first })
        assertTrue("unlock" in phone.calls)
        assertEquals("the app's lock key only, for the unlock", listOf(KeyKind.LOCK), signers.map { it.kind })
        assertEquals("never signed itself", listOf(null, null), phone.answers.map { it.second })
    }

    // decision: PHN-611
    @Test
    fun a_refusal_says_the_macs_reason_and_the_question_stays_for_yes_or_no() {
        phone.view = TranscriptView("c1", emptyList(), emptyList(), emptyList(), emptyList(), OlderState.Complete)
        phone.status = status(screenQuestion(untilBack = true))
        val vm = vm()
        vm.refresh()
        phone.answerScript.add(Outcome.Failed(FerbException.Refused("Input Monitoring is off for FERB on the Mac.", "screen_use_unavailable")))
        vm.answer("ap-s", AnswerInput.UntilBack)
        assertEquals(Note(R.string.err_mac_said, "Input Monitoring is off for FERB on the Mac."), vm.ui.value.note)
        assertTrue("the question stays open", vm.ui.value.questions.any { it.approvalId == "ap-s" })
        vm.answer("ap-s", AnswerInput.Approve(true, 1))
        assertEquals("answered with a Yes", true, (phone.answers.last().first as? AnswerInput.Approve)?.granted)
    }

    // decision: PHN-611
    @Test
    fun while_it_holds_the_line_shows_and_ask_me_again_ends_it() {
        phone.view = TranscriptView("c1", emptyList(), emptyList(), emptyList(), emptyList(), OlderState.Complete)
        val active = ScreenUse(true, "2026-11-17T10:00:00+05:30", "phone-1", "Pixel", true)
        phone.screenUse = active
        phone.status = status(null, active)
        val vm = vm()
        assertTrue(vm.ui.value.status!!.screenUse!!.active)
        vm.endScreenUse()
        assertTrue("end-screen-use" in phone.calls)
        assertFalse(vm.ui.value.status!!.screenUse!!.active)
        assertEquals(Note(R.string.screen_use_ended), vm.ui.value.note)
        // ended on the Mac (he came back): the status says so and the line goes, nothing is posted
        phone.status = status(null, active.copy(active = false))
        vm.refresh()
        assertFalse(vm.ui.value.status!!.screenUse!!.active)
    }

    /** The notification's third action is "Yes, until I'm back at my Mac" when the Mac offers it. */
    // decision: PHN-611
    @Test
    fun the_notification_offers_it_when_the_mac_does() {
        val context = ApplicationProvider.getApplicationContext<Application>()
        org.robolectric.Shadows.shadowOf(context).grantPermissions(android.Manifest.permission.POST_NOTIFICATIONS)
        val notifier = Notifier(context, phone, Prefs(context), session)
        notifier.channels()
        val q = com.ferb.app.model.Questions.fromStatus(screenQuestion(untilBack = true))
        notifier.question(q, lockedNow = false)
        val nm = context.getSystemService(android.app.NotificationManager::class.java)
        val n = org.robolectric.Shadows.shadowOf(nm).allNotifications.single()
        assertEquals(listOf("Yes", "No", context.getString(R.string.until_back_yes)), n.actions.map { it.title.toString() })
        val intent = org.robolectric.Shadows.shadowOf(n.actions[2].actionIntent).savedIntent
        assertEquals(AnswerReceiver.UNTIL_BACK, intent.getIntExtra(AnswerReceiver.CHOICE, 0))
    }
}
