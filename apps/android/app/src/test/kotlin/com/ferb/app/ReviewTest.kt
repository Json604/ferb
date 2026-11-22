package com.ferb.app

import android.app.Application
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
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
import com.ferb.app.ui.QuestionBox
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
 * FERB's review of what an import brought in (kb MEM-460..457) in its question's box: the Mac's parts, groups, items,
 * notes and buttons laid out as they come; an item's button answers that item, a part's "all" button every item of the
 * part; the Mac's reply is the list from then on; a failure leaves the list as it was; the typed answer stays.
 */
@RunWith(RobolectricTestRunner::class)
// tall enough that every button is on screen, where a tap lands
@Config(application = Application::class, sdk = [35], qualifiers = "w412dp-h2400dp-port-mdpi")
class ReviewTest {
    @get:Rule val compose = createComposeRule()

    private val root = Files.createTempDirectory("ferb-review").toFile()

    @After
    fun tearDown() { root.deleteRecursively() }

    // decision: MEM-460
    @Test
    fun the_list_lays_out_the_macs_parts_groups_items_notes_and_buttons() {
        val sent = ArrayList<List<Any>>()
        compose.setContent { MemoryReviewList(ReviewData.review, emptySet(), CardInk.ferb) { p, ids, a, key -> sent += listOf(p, ids, a, key) } }
        for (t in listOf("Keep these in mind all the time?", ReviewData.HINT, "How you like me to work", "Your setup",
            "You prefer short, concise text in chat.", "You want to talk things through before coding starts.", "You like replies in bullet points.",
            "You test on a Motorola Edge 60 Fusion over wireless debugging.", "Are these still true?", "You told me these a while ago.",
            "You value sound feedback in apps.", "Last said 20 Sep 2026", "Still want help with these?", "Book the dentist for March")) {
            compose.onNodeWithText(t).assertExists()
        }
        compose.onAllNodesWithText("Already always on").assertCountEquals(2)
        // every item has its part's buttons, with the Mac's words, or its own (an item already always on: Keep / Remove)
        compose.onAllNodesWithText("Approve").assertCountEquals(2)
        compose.onAllNodesWithText("Decline").assertCountEquals(2)
        compose.onAllNodesWithText("Keep").assertCountEquals(2)
        compose.onAllNodesWithText("Remove").assertCountEquals(2)
        compose.onNodeWithTag("review:always_on:a1:approve").assertExists()
        compose.onNodeWithTag("review:always_on:a4:decline").assertExists()
        compose.onNodeWithTag("review:still_true:s1:not_sure").assertExists()
        compose.onNodeWithTag("review:offers:o1:done").assertExists()
        // a part's "all" buttons under it when it has more than one item (FERB.app's rule)
        compose.onNodeWithText("Approve all").assertExists()
        compose.onNodeWithText("Decline all").assertExists()
        compose.onAllNodesWithText("Yes to all").assertCountEquals(0)

        compose.onNodeWithTag("review:always_on:a2:decline").performClick()
        compose.onNodeWithTag("review:always_on:a1:decline").performClick()
        compose.onNodeWithText("Approve all").performClick()
        compose.onNodeWithTag("review:still_true:s1:no").performClick()
        assertEquals(listOf(
            listOf("always_on", listOf("a2"), "decline", "review:always_on:a2"),
            listOf("always_on", listOf("a1"), "decline", "review:always_on:a1"),
            listOf("always_on", listOf("a1", "a2", "a3", "a4"), "approve", "review:always_on:*"),
            listOf("still_true", listOf("s1"), "no", "review:still_true:s1"),
        ), sent)
    }

    // decision: MEM-460
    @Test
    fun a_group_of_more_than_three_shows_three_until_opened() {
        val items = (1..5).map { ReviewItem("x$it", "Line $it", "", emptyList()) }
        val r = MemoryReview(listOf(ReviewData.review.parts[0].copy(groups = listOf(ReviewGroup("Many", items, emptyList())))), 5uL)
        compose.setContent { MemoryReviewList(r, emptySet(), CardInk.ferb) { _, _, _, _ -> } }
        compose.onNodeWithText("Line 3").assertExists()
        compose.onAllNodesWithText("Line 4").assertCountEquals(0)
        compose.onNodeWithText("Show all 5").performClick()
        compose.onNodeWithText("Line 5").assertExists()
        compose.onNodeWithText("Show less").performClick()
        compose.onAllNodesWithText("Line 5").assertCountEquals(0)
    }

    // decision: MEM-464
    @Test
    fun a_group_of_items_already_on_has_its_own_buttons_for_all_of_them() {
        val sent = ArrayList<List<Any>>()
        compose.setContent { MemoryReviewList(ReviewData.setupGroup, emptySet(), CardInk.ferb) { p, ids, a, key -> sent += listOf(p, ids, a, key) } }
        compose.onNodeWithText("Keep all").assertExists()
        compose.onNodeWithText("Remove all").performClick()
        compose.onNodeWithText("Approve all").performClick()
        assertEquals(listOf(
            listOf("always_on", listOf("a4", "a5"), "decline", "review:always_on:group:Your setup"),
            listOf("always_on", listOf("a1", "a2", "a3", "a4", "a5"), "approve", "review:always_on:*"),
        ), sent)
    }

    // decision: MEM-460, MEM-462
    @Test
    fun in_the_question_box_it_is_read_from_the_mac_answered_in_place_and_the_reply_replaces_it() {
        val phone = FakePhone()
        phone.memoryReview = ReviewData.review
        phone.status = PhoneStatus(false, true, false, listOf(ReviewData.statusQuestion), emptyList(), emptyList(), null)
        val vm = FerbViewModel(phone, Session { 0L }, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
            LocalFiles(root) { null })
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

        compose.onNodeWithTag("review:always_on:a3:decline").performClick()
        compose.waitForIdle()
        assertEquals(Triple("always_on", listOf("a3"), "decline"), phone.reviewAnswers.last())
        compose.onAllNodesWithText("You like replies in bullet points.").assertCountEquals(0)
        compose.onNodeWithText("You prefer short, concise text in chat.").assertExists()

        compose.onNodeWithText("Approve all").performClick()
        compose.waitForIdle()
        assertEquals(Triple("always_on", listOf("a1", "a2", "a4"), "approve"), phone.reviewAnswers.last())
        compose.onAllNodesWithText("Keep these in mind all the time?").assertCountEquals(0)
        compose.onNodeWithText("Are these still true?").assertExists()

        // a refusal leaves the list as it was and says why
        phone.reviewRefusal = Outcome.Failed(FerbException.Offline("No link.", null))
        compose.onNodeWithTag("review:offers:o1:done").performClick()
        compose.waitForIdle()
        compose.onNodeWithText("Book the dentist for March").assertExists()
        assertEquals(Note(R.string.err_offline), vm.ui.value.note)
    }
}

/** The invented review the tests and the screenshots draw. */
object ReviewData {
    const val QUESTION = "Can you check a few things I brought in?"
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
    ), 6uL)

    /** The variant: "Your setup" holds two items already always on, with its own Keep all / Remove all (MEM-464). */
    val setupGroup = review.copy(parts = review.parts.mapIndexed { i, p ->
        if (i != 0) p else p.copy(groups = listOf(p.groups[0], ReviewGroup("Your setup", listOf(
            p.groups[1].items[0],
            ReviewItem("a5", "You use Arc as your main browser.", "Already always on", keep),
        ), listOf(ReviewButton("approve", "Keep all"), ReviewButton("decline", "Remove all")))))
    }, count = 7uL)

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
