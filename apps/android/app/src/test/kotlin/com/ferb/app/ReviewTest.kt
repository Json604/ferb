package com.ferb.app

import android.app.Application
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.test.assertIsNotSelected
import androidx.compose.ui.test.assertIsSelected
import androidx.compose.ui.test.assertCountEquals
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.Question
import com.ferb.app.ui.LocalBusy
import com.ferb.app.ui.LocalQuestions
import com.ferb.app.ui.MemoryReviewList
import com.ferb.app.ui.Host
import com.ferb.app.ui.QuestionBox
import com.ferb.app.ui.Sheets
import com.ferb.core.KeyHealth
import com.ferb.core.KeyState
import com.ferb.design.screens.PhoneInsets
import com.ferb.core.Outcome
import com.ferb.design.components.CardInk
import com.ferb.mobile.FerbException
import com.ferb.mobile.MemoryReview
import com.ferb.mobile.PhoneStatus
import com.ferb.mobile.QuestionNeeds
import com.ferb.mobile.QuestionShown
import com.ferb.mobile.ReviewButton
import com.ferb.mobile.ReviewGroup
import com.ferb.mobile.ReviewItem
import com.ferb.mobile.ReviewPart
import com.ferb.mobile.StatusQuestion
import java.nio.file.Files
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * FERB's review of what it learned (kb MEM-460..457, MEM-485, MEM-486): the Mac's parts, groups, lines and notes laid
 * out as they come, each line ticked until the owner unticks it, Approve all and Decline all on top answering the whole
 * list in one call; in its question's box and in the sheet the day's note opens; the Mac told when the owner opens FERB
 * here; a failure leaves the list as it was; the typed answer stays.
 */
@RunWith(RobolectricTestRunner::class)
// tall enough that every button is on screen, where a tap lands
@Config(application = Application::class, sdk = [35], qualifiers = "w412dp-h2400dp-port-mdpi")
class ReviewTest {
    @get:Rule val compose = createComposeRule()

    private val root = Files.createTempDirectory("ferb-review").toFile()

    @After
    fun tearDown() { root.deleteRecursively() }

    private fun vm(phone: FakePhone, session: Session = Session { 0L }) =
        FerbViewModel(phone, session, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
            LocalFiles(root) { null }, account = testAccount())

    // decision: MEM-460, MEM-488
    @Test
    fun the_list_lays_out_the_macs_parts_groups_and_lines_with_a_tick_each() {
        val toggled = ArrayList<String>()
        val all = ArrayList<Boolean>()
        var unticked by mutableStateOf(emptySet<String>())
        compose.setContent {
            MemoryReviewList(ReviewData.review, unticked, emptySet(), CardInk.white, showHint = true,
                onToggle = { toggled += it; unticked = if (it in unticked) unticked - it else unticked + it }, onAll = { all += it })
        }
        for (t in listOf("Keep these in mind all the time?", ReviewData.HINT, "How you like me to work", "Your setup",
            "You prefer short, concise text in chat.", "You want to talk things through before coding starts.", "You like replies in bullet points.",
            "You test on a Motorola Edge 60 Fusion over wireless debugging.", "Are these still true?", "You told me these a while ago.",
            "You value sound feedback in apps.", "Last said 20 Sep 2026", "Still want help with these?", "Book the dentist for March",
            ReviewData.UNTICK)) {
            compose.onNodeWithText(t).assertExists()
        }
        // no button per line: one tick each, and the list's two buttons on top
        compose.onAllNodesWithText("Approve").assertCountEquals(0)
        compose.onAllNodesWithText("Keep").assertCountEquals(0)
        compose.onAllNodesWithText("Yes to all").assertCountEquals(0)
        compose.onNodeWithTag("review-item:a2").assertIsSelected()
        compose.onNodeWithText("Approve all").assertExists()
        // a tap on a line unticks it; the main button then names how many it approves
        compose.onNodeWithTag("review-item:a2").performClick()
        compose.onNodeWithTag("review-item:a2").assertIsNotSelected()
        compose.onNodeWithText("Approve 5").performClick()
        compose.onNodeWithText("Decline all").performClick()
        assertEquals(listOf("a2"), toggled)
        assertEquals(listOf(true, false), all)
    }

    // decision: MEM-488 — every line shows: Approve all answers only what he can see
    @Test
    fun every_line_shows_however_many() {
        val items = (1..8).map { ReviewItem("x$it", "Line $it", "", emptyList()) }
        val r = ReviewData.review.copy(parts = listOf(ReviewData.review.parts[0].copy(groups = listOf(ReviewGroup("Many", items, emptyList())))), count = 8uL)
        compose.setContent { MemoryReviewList(r, emptySet(), emptySet(), CardInk.ferb, onToggle = {}, onAll = {}) }
        for (n in 1..8) compose.onNodeWithText("Line $n").assertExists()
        compose.onAllNodesWithText("Show all 8").assertCountEquals(0)
    }

    // decision: MEM-460, MEM-488
    @Test
    fun in_the_question_box_approve_all_sends_the_ticked_and_the_unticked_and_the_reply_replaces_it() {
        val phone = FakePhone()
        phone.memoryReview = ReviewData.review
        phone.status = PhoneStatus(false, true, false, listOf(ReviewData.statusQuestion), emptyList(), emptyList(), null)
        val vm = vm(phone)
        compose.setContent {
            val ui by vm.ui.collectAsState()
            CompositionLocalProvider(LocalQuestions provides ui.questions, LocalBusy provides ui.busy) { QuestionBox(vm, "ap-r", null, null) }
        }
        compose.waitForIdle()
        assertTrue("read when the box shows", "memory-review" in phone.calls)
        compose.onNodeWithText(ReviewData.QUESTION).assertExists()
        compose.onNodeWithText("You like replies in bullet points.").assertExists()
        // the typed answer's box stays, as for any question
        compose.onNodeWithText("Or type your answer…").assertExists()
        compose.onNodeWithText("Skip").assertExists()

        // a refusal leaves the list as it was and says why
        phone.reviewRefusal = Outcome.Failed(FerbException.Offline("No link.", null))
        compose.onNodeWithText("Approve all").performClick()
        compose.waitForIdle()
        compose.onNodeWithText("Book the dentist for March").assertExists()
        assertEquals(Note(R.string.err_offline), vm.ui.value.note)
        phone.reviewRefusal = null

        compose.onNodeWithTag("review-item:a3").performClick()
        compose.onNodeWithTag("review-item:s1").performClick()
        compose.onNodeWithText("Approve 4").performClick()
        compose.waitForIdle()
        assertEquals(listOf("a1", "a2", "a4", "o1") to listOf("a3", "s1"), phone.reviewAll.last())
        // everything answered: the card goes
        compose.onAllNodesWithText("Keep these in mind all the time?").assertCountEquals(0)
        assertTrue(vm.unticked.value.isEmpty())
    }

    // decision: MEM-485 — the day's note's Review opens the list in a sheet (the phone has no Memory page)
    @Test
    fun the_notes_review_opens_the_list_in_a_sheet() {
        val phone = FakePhone()
        phone.memoryReview = ReviewData.review
        val vm = vm(phone)
        val ui = UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK))
        vm.openReview()
        assertEquals(Sheet.Review, vm.sheet.value)
        compose.setContent {
            val sheet by vm.sheet.collectAsState()
            Sheets(vm, ui, sheet, PhoneInsets.Lab, NoHost)
        }
        compose.waitForIdle()
        assertTrue("memory-review" in phone.calls)
        compose.onNodeWithText("To check").assertExists()
        compose.onNodeWithText(ReviewData.UNTICK).assertExists()
        compose.onNodeWithText("Decline all").performClick()
        compose.waitForIdle()
        assertEquals(emptyList<String>() to listOf("a1", "a2", "a3", "a4", "s1", "o1"), phone.reviewAll.last())
        compose.onNodeWithText("Nothing to check right now.").assertExists()
    }

    // decision: MEM-485 — opening FERB here (in front, unlocked, the Mac reachable) tells the Mac; locked or away, nothing
    @Test
    fun opening_the_app_tells_the_mac_once_each_time() {
        val phone = FakePhone()
        val session = Session { 0L }
        vm(phone, session)
        compose.waitForIdle()
        assertEquals(0, phone.calls.count { it == "review-opened" })
        session.foreground(true)
        session.unlocked()
        compose.waitForIdle()
        assertEquals(1, phone.calls.count { it == "review-opened" })
        session.foreground(false)
        session.foreground(true)
        session.unlocked()
        compose.waitForIdle()
        assertEquals(2, phone.calls.count { it == "review-opened" })
    }
}

private object NoHost : Host {
    override fun stayConnected(on: Boolean) {}
    override fun blockScreenshots(on: Boolean) {}
    override fun askNotifications() {}
    override fun pickFiles(forChat: Boolean) {}
    override fun handOut(key: String, file: java.io.File, name: String, share: Boolean) {}
}

/** The invented review the tests and the screenshots draw. */
object ReviewData {
    const val QUESTION = "Can you check a few things I learned about you?"
    const val UNTICK = "Untick anything that isn't right."
    const val HINT = "I'll remember all of them either way. The ones you approve, I'll keep in mind in everything I do."

    private val keep = listOf(ReviewButton("approve", "Keep"), ReviewButton("decline", "Remove"))

    val review = MemoryReview(listOf(
        ReviewPart("always_on", "Keep these in mind all the time?", HINT, listOf(
            ReviewGroup("How you like me to work", listOf(
                ReviewItem("a1", "You prefer short, concise text in chat.", "Already always on", keep),
                ReviewItem("a2", "You want to talk things through before coding starts.", "", emptyList()),
                ReviewItem("a3", "You like replies in bullet points.", "", emptyList()),
            ), emptyList()),
            ReviewGroup("Your setup", listOf(ReviewItem("a4", "You test on a Motorola Edge 60 Fusion over wireless debugging.", "Already always on", keep)), emptyList()),
        ), listOf(ReviewButton("approve", "Approve"), ReviewButton("decline", "Decline")),
            listOf(ReviewButton("approve", "Approve all"), ReviewButton("decline", "Decline all"))),
        ReviewPart("still_true", "Are these still true?", "You told me these a while ago.", listOf(
            ReviewGroup("", listOf(ReviewItem("s1", "You value sound feedback in apps.", "Last said 20 Sep 2026", emptyList())), emptyList()),
        ), listOf(ReviewButton("yes", "Yes"), ReviewButton("no", "No, not any more"), ReviewButton("not_sure", "Not sure")),
            listOf(ReviewButton("yes", "Yes to all"), ReviewButton("no", "No to all"))),
        ReviewPart("offers", "Still want help with these?", "I won't start any of them on my own.", listOf(
            ReviewGroup("", listOf(ReviewItem("o1", "Book the dentist for March", "", emptyList())), emptyList()),
        ), listOf(ReviewButton("open", "Yes"), ReviewButton("done", "Done already"), ReviewButton("not_interested", "No thanks")), emptyList()),
    ), 6uL, listOf(ReviewButton("approve", "Approve all"), ReviewButton("decline", "Decline all")), "Approve {n}", UNTICK)

    /** The question as ferb-mobile shows it (`question.rs`: no options, the typed answer's box). */
    val shown = QuestionShown(
        header = "FERB needs your answer", secret = false, fromTask = false, question = QUESTION, plain = true, picture = false,
        pictureProblem = null, detail = null, exact = null, why = null, evidence = emptyList(), buttons = emptyList(), note = null,
        answerBox = "answer", field = "Or type your answer…", decline = "Skip", send = "Send", bannerTitle = null, bannerText = null, review = true,
    )

    val question = Question("ap-r", null, QUESTION, QUESTION, null, "memory_review", "memory.review", emptyList(), emptyList(), false, shown = shown)

    val statusQuestion = StatusQuestion("ap-r", "memory_review", "memory.review", null, QUESTION, emptyList(), false,
        QuestionNeeds(true, false, false, false, null, null, false), shown)
}
