package com.ferb.app

import android.Manifest
import android.app.Application
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.os.Looper
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.Question
import com.ferb.app.model.QuestionAlerts
import com.ferb.app.model.Questions
import com.ferb.mobile.Change
import com.ferb.mobile.OlderState
import com.ferb.mobile.PhoneStatus
import com.ferb.mobile.QuestionNeeds
import com.ferb.mobile.StatusQuestion
import com.ferb.mobile.TranscriptView
import java.nio.file.Files
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/**
 * The owner's rule (kb PHN-760): every question FERB asks, from a worker or the planner, notifies unless he is in the
 * app. Each kind of question (a routine one, one of FERB's checks, a hidden input, the planner's question, one whose
 * words are held) through the real notifier, the Mac's status as the fake gives it: in the app, in the background, on
 * the app's lock page, answered elsewhere, asked again, after a restart, and with notifications off.
 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class NotifierTest {
    private val context: Context = ApplicationProvider.getApplicationContext()
    private val phone = FakePhone()
    private lateinit var session: Session
    private val prefs = Prefs(context)
    private val nm = context.getSystemService(NotificationManager::class.java)
    private val scopes = ArrayList<CoroutineScope>()

    @Before
    fun setUp() {
        Dispatchers.setMain(UnconfinedTestDispatcher())
        session = Session { 0L }
        prefs.forgetPairing()
        phone.view = TranscriptView("c1", emptyList(), emptyList(), emptyList(), emptyList(), OlderState.Complete)
    }

    @After
    fun tearDown() {
        scopes.forEach { it.cancel() }
        Dispatchers.resetMain()
    }

    private fun idle() = shadowOf(Looper.getMainLooper()).idle()

    private fun allow() = shadowOf(context as Application).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)

    private fun started(): Notifier = Notifier(context, phone, prefs, session).also { n -> MainScope().also { scopes += it; n.start(it) } }

    private fun macSays(vararg q: StatusQuestion) {
        phone.status = PhoneStatus(false, true, false, q.toList(), emptyList(), emptyList(), null)
        phone.changes.tryEmit(Change.Status)
        idle()
    }

    private val shade: List<Notification> get() = shadowOf(nm).allNotifications
    private fun posted(id: String): Notification? = shadowOf(nm).getNotification(Notifier.keyOf("q:$id"))
    private fun Notification.text() = extras.getCharSequence(Notification.EXTRA_TEXT).toString()
    private fun Notification.title() = extras.getCharSequence(Notification.EXTRA_TITLE).toString()
    private val Notification.quiet: Boolean
        get() = flags and Notification.FLAG_ONLY_ALERT_ONCE != 0 && group == Notifier.SILENT_GROUP && groupAlertBehavior == Notification.GROUP_ALERT_SUMMARY

    private val routine = StatusQuestion("ap-routine", "approve", "owner.ask", "t1", "Send the draft to Ajay?", listOf("Yes", "No"), false,
        QuestionNeeds(true, false, false, false, null, null, false))
    private val check = StatusQuestion("ap-check", "approve", "ferb.confirm", "t1", "Delete 3 files for good?", listOf("Yes", "No"), false,
        QuestionNeeds(true, true, false, true, "ch-1", null, false))
    private val hidden = StatusQuestion("ap-code", "owner_login_code", "owner.input", "t1", "The code from your bank", emptyList(), false,
        QuestionNeeds(true, true, true, false, "ch-2", null, false))
    private val clarify = StatusQuestion("clarify:r1", Question.CLARIFY, null, null, "Which email account should I use?", emptyList(), false,
        QuestionNeeds(true, false, false, false, null, null, false))
    private val held = StatusQuestion("ap-held", "approve", "owner.ask", "t1", "FERB has a question", emptyList(), true,
        QuestionNeeds(true, false, false, false, null, null, false))

    // decision: PHN-760, PHN-761, PHN-762, PHN-417, PHN-422, PHN-126
    @Test
    fun every_kind_of_question_notifies_at_once_when_he_is_not_in_the_app() {
        allow()
        session.unlocked()
        session.foreground(false)
        started()
        macSays(routine, check, hidden, clarify, held)
        assertEquals(5, shade.size)
        for (n in shade) {
            assertFalse("a new question is a heads-up, with sound", n.quiet)
            assertTrue(n.channelId in setOf(Notifier.CH_QUESTIONS, Notifier.CH_CHECKS))
        }
        // routine: its words and buttons (PHN-417)
        val r = posted("ap-routine")!!
        assertEquals(listOf("Yes", "No"), r.actions.map { it.title.toString() })
        // a check: shown, its Yes needs the fingerprint in the app
        val c = posted("ap-check")!!
        assertTrue(c.actions.isNullOrEmpty())
        assertTrue(c.text().contains(context.getString(R.string.n_needs_fingerprint)))
        // a hidden input: never worded, never typed in a notification
        val h = posted("ap-code")!!
        assertEquals(context.getString(R.string.n_private_title), h.title())
        assertFalse(h.text().contains("bank"))
        assertTrue(h.actions.isNullOrEmpty())
        // the planner's question: its words; no buttons; the tap opens the chat, where the answer is a message
        val q = posted("clarify:r1")!!
        assertTrue(q.text().startsWith("Which email account should I use?"))
        assertTrue(q.text().contains(context.getString(R.string.n_clarify_text)))
        assertTrue(q.actions.isNullOrEmpty())
        assertEquals(Notifier.CHAT, shadowOf(q.contentIntent).savedIntent.getStringExtra(MainActivity.EXTRA_ENTRY))
        assertEquals(Notifier.CH_QUESTIONS, q.channelId)
        assertEquals(context.getString(R.string.n_needs_you), q.title())
        // words held back (a connection that didn't prove the device key): only that FERB has a question
        val w = posted("ap-held")!!
        assertEquals(context.getString(R.string.n_held_text), w.text())
        // the questions channel pops: heads-up needs high importance
        assertEquals(NotificationManager.IMPORTANCE_HIGH, nm.getNotificationChannel(Notifier.CH_QUESTIONS).importance)
        assertEquals(NotificationManager.IMPORTANCE_HIGH, nm.getNotificationChannel(Notifier.CH_CHECKS).importance)
    }

    /** The Mac's banner words (ferb-daemon notify.rs approval_notice), from the library's presentation of the question. */
    // decision: PHN-760, PHN-767
    @Test
    fun a_question_reads_as_the_mac_s_banner_and_the_planner_s_with_its_own_words() {
        allow()
        session.unlocked()
        session.foreground(false)
        started()
        val shown = com.ferb.mobile.questionShown("""{"kind": "owner_question", "capability": "owner.ask", "reason": "FERB: this types into the page. The step: browser.act action=type", "summary": "Send the draft to Ajay?", "options": ["Yes", "No"]}""")
        val asked = routine.copy(words = shown)
        val planner = clarify.copy(words = com.ferb.mobile.questionShown("""{"kind": "clarify", "reason": "Which email account should I use?"}"""))
        macSays(asked, planner)
        val r = posted("ap-routine")!!
        assertEquals("FERB needs you", r.title())
        assertEquals("FERB needs your OK: Send the draft to Ajay?", r.text())
        assertFalse("never the raw reason or a tool call", r.text().contains("browser.act"))
        val q = posted("clarify:r1")!!
        assertEquals("FERB needs you", q.title())
        assertTrue(q.text().startsWith("Which email account should I use?"))
    }

    /** A turn that ended, as the library hands it. */
    private fun ended(entry: String, task: String, name: String?, status: String, answer: String, display: String? = null) =
        com.ferb.mobile.Item.Turn(com.ferb.mobile.TurnItem(
            entry = entry, at = 0.0, legacy = false, clientId = null, requestId = "r-$entry", taskId = task, from = "owner",
            taskName = name, status = status, statusInferred = false, notice = null, mergedInto = null, question = null,
            answeredBelow = false,
            answer = com.ferb.mobile.AnswerView(answer, display, null, false, emptyList(), null, null, null, null),
            card = null, checklist = null, firstReply = null, parts = emptyList(), goal = null, words = null))

    private fun finished(task: String): Notification? = shadowOf(nm).getNotification(Notifier.keyOf("t:$task"))

    /**
     * The owner, 2026-11-22: the phone said "Couldn't finish" / "Coursera BITS check" for a task he had stopped, where
     * FERB.app's chat said "Stopped, as you asked.", and "Done" / a short name where the Mac shows the answer. Each end
     * now reads as the Mac's banner: the task's short name and the answer in the chat's words; his own stop, which the
     * Mac posts no banner for, posts nothing.
     */
    // decision: PHN-795, PHN-796
    @Test
    fun a_task_s_end_reads_as_the_mac_says_it() {
        allow()
        session.unlocked()
        session.foreground(false)
        started()
        idle()
        phone.view = TranscriptView("c1", listOf(
            ended("e1", "t-stop", "Coursera BITS check", "cancelled", "Stopped, as you asked."),
            ended("e2", "t-done", "Scaler dashboard check", "done", "Your dashboard shows **2** new classes.\n\nSites contacted: scaler.com"),
            ended("e3", "t-fail", null, "failed", "I couldn't finish this task. The site kept refusing the card."),
            ended("e4", "t-gaps", "Table booking", "done_with_gaps", "I booked the table for two at 8.\n\nNot done: the window seat — they had none left."),
            ended("e5", "t-again", "Google AI Pro perks", "cancelled", "Stopped here to start again with your change."),
            ended("e6", "t-short", "IIT Madras Chrome profile", "done", "The whole long answer.", display = "Your IIT Madras profile is set up."),
        ), emptyList(), emptyList(), emptyList(), OlderState.Complete)
        phone.changes.tryEmit(Change.Transcript("c1"))
        idle()
        assertNull("his own stop: no banner on the Mac, nothing here (the chat says it)", finished("t-stop"))
        val done = finished("t-done")!!
        assertEquals("Scaler dashboard check", done.title())
        assertEquals("the answer in the chat's words, the sites row left to the chat", "Your dashboard shows 2 new classes.", done.text())
        val failed = finished("t-fail")!!
        assertEquals("no short name: the Mac's own title", "FERB", failed.title())
        assertEquals("I couldn't finish this task. The site kept refusing the card.", failed.text())
        assertEquals("I booked the table for two at 8.\n\nNot done: the window seat — they had none left.", finished("t-gaps")!!.text())
        assertEquals("Your IIT Madras profile is set up.", finished("t-short")!!.text())
        for (n in shade) assertFalse(n.title() in setOf("Done", "Couldn't finish"))
        // the run his follow-up started again is not the task's end: nothing, and the task's real end is still news
        assertNull(finished("t-again"))
        phone.view = phone.view!!.copy(items = phone.view!!.items +
            ended("e7", "t-again", "Google AI Pro perks", "done", "Your trial ends on 3 December."))
        phone.changes.tryEmit(Change.Transcript("c1"))
        idle()
        val again = finished("t-again")!!
        assertEquals("Google AI Pro perks", again.title())
        assertEquals("Your trial ends on 3 December.", again.text())
    }

    /** From the Mac's status (the words held while locked): the Mac's title and the answer, or only that it's there. */
    // decision: PHN-795, PHN-126
    @Test
    fun a_result_from_the_mac_s_status_reads_as_its_banner_and_held_words_stay_home() {
        allow()
        session.unlocked()
        session.foreground(false)
        phone.status = PhoneStatus(false, true, false, emptyList(), emptyList(), emptyList(), null)
        started()
        idle()
        phone.status = PhoneStatus(false, true, false, emptyList(), emptyList(), listOf(
            com.ferb.mobile.StatusResult("t-1", null, null, "failed", "Invoice", "I couldn't finish this task. The site was down.", false),
            com.ferb.mobile.StatusResult("t-2", null, null, "done", "", "", true),
        ), null)
        phone.changes.tryEmit(Change.Status)
        idle()
        val failed = finished("t-1")!!
        assertEquals("Invoice", failed.title())
        assertEquals("I couldn't finish this task. The site was down.", failed.text())
        val held = finished("t-2")!!
        assertEquals("FERB finished", held.title())
        assertEquals(context.getString(R.string.n_held_text), held.text())
    }

    // decision: PHN-760, PHN-764
    @Test
    fun in_the_app_nothing_is_posted_and_one_he_saw_goes_to_the_shade_quietly_when_he_leaves() {
        allow()
        session.unlocked()
        session.foreground(true)
        started()
        macSays(routine, clarify)
        assertTrue("in the app, the app shows it", shade.isEmpty())
        session.foreground(false)
        idle()
        assertEquals(2, shade.size)
        assertTrue("seen in the app: no second alert", posted("ap-routine")!!.quiet && posted("clarify:r1")!!.quiet)
        // a new one while he is away: a heads-up
        macSays(routine, clarify, check)
        assertFalse(posted("ap-check")!!.quiet)
        // answered in the app before he left: never in the shade
        session.foreground(true)
        macSays(routine, clarify, check, hidden)
        macSays(routine, clarify, check)
        session.foreground(false)
        idle()
        assertNull(posted("ap-code"))
    }

    // decision: PHN-760, PHN-8
    @Test
    fun on_the_app_s_lock_page_he_has_not_seen_it_so_leaving_brings_a_heads_up() {
        allow()
        session.foreground(true) // locked: the lock page
        started()
        macSays(clarify)
        assertTrue(shade.isEmpty())
        session.foreground(false)
        idle()
        assertFalse(posted("clarify:r1")!!.quiet)
    }

    // decision: PHN-760, PHN-108
    @Test
    fun once_per_question_gone_when_answered_anywhere_and_again_only_when_asked_again() {
        allow()
        session.unlocked()
        session.foreground(false)
        started()
        macSays(routine, clarify)
        val first = posted("clarify:r1")!!.`when`
        macSays(routine, clarify)
        macSays(routine, clarify)
        assertEquals(2, shade.size)
        assertEquals("not posted again", first, posted("clarify:r1")!!.`when`)
        // answered on the Mac, on another phone, or by his next message: it leaves the shade
        macSays(routine)
        assertNull(posted("clarify:r1"))
        macSays()
        assertTrue(shade.isEmpty())
        // FERB asks again: a new question, posted again
        macSays(clarify.copy(approvalId = "clarify:r2", reason = "Who goes in CC?"))
        assertTrue(posted("clarify:r2")!!.text().startsWith("Who goes in CC?"))
        // the same question again later (the status came back after a gap): not again
        prefs.announced.let { assertTrue("q:clarify:r2" in it) }
    }

    // decision: PHN-760
    @Test
    fun a_question_already_waiting_when_the_app_starts_again_still_notifies() {
        allow()
        session.unlocked()
        session.foreground(false)
        // the process starts before the first status has come
        phone.status = null
        started()
        idle()
        phone.status = PhoneStatus(false, true, false, listOf(check), emptyList(),
            listOf(com.ferb.mobile.StatusResult("t0", null, null, "done", "An old task", "Done.", false)), null)
        phone.changes.tryEmit(Change.Status)
        idle()
        assertTrue("a waiting question is news after a restart", posted("ap-check") != null)
        assertEquals("what had finished before is not", 1, shade.size)
        // and a process after that one, with it announced: not again
        nm.cancelAll()
        started()
        phone.changes.tryEmit(Change.Status)
        idle()
        assertTrue(shade.isEmpty())
    }

    // decision: PHN-765
    @Test
    fun what_he_must_change_is_named_never_silently_nothing() {
        session.unlocked()
        session.foreground(false)
        started()
        macSays(routine)
        assertTrue("Android's permission is off: nothing can be posted", shade.isEmpty())
        assertEquals(NotifyBlocker.NOTIFICATIONS_OFF, NotifyHealth.blocker(context, stayConnected = true))
        allow()
        assertEquals(NotifyBlocker.STAY_CONNECTED_OFF, NotifyHealth.blocker(context, stayConnected = false))
        assertNull(NotifyHealth.blocker(context, stayConnected = true))
        nm.createNotificationChannel(NotificationChannel(Notifier.CH_QUESTIONS, "Questions", NotificationManager.IMPORTANCE_NONE))
        assertEquals(NotifyBlocker.QUESTIONS_OFF, NotifyHealth.blocker(context, stayConnected = true))
        assertEquals(Notifier.CH_QUESTIONS, NotifyHealth.settings(context, NotifyBlocker.QUESTIONS_OFF)!!.getStringExtra(android.provider.Settings.EXTRA_CHANNEL_ID))
        assertNull("Stay connected is the app's own switch", NotifyHealth.settings(context, NotifyBlocker.STAY_CONNECTED_OFF))
    }

    // decision: PHN-765
    @Test
    fun after_a_send_he_is_told_once_what_keeps_questions_from_reaching_him() {
        val root = Files.createTempDirectory("ferb-notify").toFile()
        session.unlocked()
        val vm = FerbViewModel(phone, session, prefs, { true }, null, SavedStateHandle(), LocalFiles(root) { null }, account = testAccount())
        vm.notifyBlocker = { NotifyBlocker.STAY_CONNECTED_OFF }
        vm.typed("draft the mail")
        vm.send(); idle()
        assertEquals(Sheet.CantNotify(NotifyBlocker.STAY_CONNECTED_OFF), vm.sheet.value)
        vm.show(null)
        vm.typed("and another")
        vm.send(); idle()
        assertNull("once per kind", vm.sheet.value)
        root.deleteRecursively()
    }

    // decision: PHN-762
    @Test
    fun the_planner_s_question_is_ferb_s_bubble_in_the_app_not_a_question_sheet_or_a_task() {
        val root = Files.createTempDirectory("ferb-notify").toFile()
        session.unlocked()
        session.foreground(true)
        phone.status = PhoneStatus(false, true, false, listOf(clarify, routine), emptyList(), emptyList(), null)
        val vm = FerbViewModel(phone, session, prefs, { true }, null, SavedStateHandle(), LocalFiles(root) { null }, account = testAccount())
        vm.refresh(); idle()
        assertEquals(listOf("ap-routine"), vm.ui.value.questions.map { it.approvalId })
        assertTrue(vm.ui.value.work.waiting.none { it.approvalId == "clarify:r1" })
        assertTrue(vm.ui.value.rows.none { it is com.ferb.app.model.ChatRow.Asking && it.approvalId == "clarify:r1" })
        // the notifications' list has it; the routine-answer path never answers it from a button
        val all = Questions.waiting(emptyList(), phone.status)
        assertTrue(all.single { it.approvalId == "clarify:r1" }.let { it.clarify && !it.routine })
        root.deleteRecursively()
    }

    /** The rule alone (QuestionAlerts): no status yet means no change; seen ones are pruned once they stop waiting. */
    // decision: PHN-760
    @Test
    fun the_rule_alone() {
        val a = QuestionAlerts()
        val q = Questions.fromStatus(routine)
        assertEquals(QuestionAlerts.Plan(emptyList(), emptyList(), emptyList(), setOf("q:x")), a.decide(null, false, true, emptyMap(), setOf("q:x")))
        val p = a.decide(listOf(q), false, true, emptyMap(), setOf("q:gone", "t:task"))
        assertEquals(listOf(QuestionAlerts.Post(q, quiet = false)), p.post)
        assertEquals(setOf("q:ap-routine", "t:task"), p.announced)
        // updated quietly when its words change (the app unlocked: the transcript's words), never posted again
        val worded = q.copy(title = "Send the draft?")
        val u = a.decide(listOf(worded), false, true, mapOf(q.approvalId to q), p.announced)
        assertEquals(listOf(worded), u.update)
        assertTrue(u.post.isEmpty())
        assertEquals(listOf(q.approvalId), a.decide(emptyList(), false, true, mapOf(q.approvalId to q), u.announced).cancel)
    }
}
