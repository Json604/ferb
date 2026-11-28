package com.ferb.app

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollToIndex
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.swipeUp
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.ChatModel
import com.ferb.app.model.ChatRow
import com.ferb.app.model.LocalFiles
import com.ferb.app.ui.Host
import com.ferb.app.ui.Main
import com.ferb.core.KeyHealth
import com.ferb.core.KeyState
import com.ferb.design.Phone
import com.ferb.design.components.PhoneTab
import com.ferb.design.preview.labMoment
import com.ferb.design.screens.LabAndroidChrome
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.ferb.mobile.OutboxItem
import com.ferb.mobile.ReplyQuote
import com.ferb.mobile.TranscriptView
import com.github.takahirom.roborazzi.captureRoboImage
import kotlinx.coroutines.launch
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.assertFalse
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsNotDisplayed
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.io.File

/**
 * Reply to a message as the owner sees and does it (kb APP-1146), goldens in app/src/test/screenshots/reply: a swipe
 * under way, the quote above the message box, his sent and his waiting message with their quotes. And the gesture on
 * the real chat: a swipe right far enough quotes a bubble, a short one or a scroll doesn't, TalkBack's Reply does, and a
 * tap on a quote brings its message into view and lights it.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(qualifiers = "w412dp-h1000dp-port-mdpi")
class ReplyShotsTest {
    @get:Rule val compose = createComposeRule()

    private val phone = FakePhone()
    private val session = Session { 0L }
    private val vm by lazy {
        FerbViewModel(phone, session, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
            LocalFiles(java.nio.file.Files.createTempDirectory("ferb-reply-shots").toFile()) { null }, account = testAccount())
    }
    private val host = object : Host {
        override fun stayConnected(on: Boolean) {}
        override fun blockScreenshots(on: Boolean) {}
        override fun askNotifications() {}
        override fun pickFiles(forChat: Boolean) {}
        override fun handOut(key: String, file: File, name: String, share: Boolean) {}
    }
    private val base = UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK), conversation = "c1")

    @Composable
    private fun Frame(ui: UiState, hour: Double = 12.5) {
        Box(Modifier.size(Phone.frameWidth, Phone.frameHeight).testTag("phone")) {
            PhoneScreen(labMoment(hour), insets = PhoneInsets.Lab, glide = false, sheet = { LabAndroidChrome() }) {
                Main(vm, ui, PhoneTab.CHAT, home = false, PhoneInsets.Lab, labMoment(hour), host)
            }
        }
    }

    private fun shoot(name: String) {
        compose.mainClock.autoAdvance = false
        repeat(40) { compose.mainClock.advanceTimeByFrame() }
        val path = "src/test/screenshots/reply/$name"
        compose.onNodeWithTag("phone").captureRoboImage(path, roborazziOptions = ScreenShotsTest.options)
    }

    private val answerQuote get() = ChatModel.rows(ReplyTest.chat()).filterIsInstance<ChatRow.Answer>().first().quote!!

    /** FERB's answer's bubble ("FERB: …Nora's…"), not the quote of it above his reply. */
    private fun answerBubble() = compose.onNode(androidx.compose.ui.test.SemanticsMatcher("FERB's answer") { n ->
        val said: List<String> = if (androidx.compose.ui.semantics.SemanticsProperties.ContentDescription in n.config)
            n.config[androidx.compose.ui.semantics.SemanticsProperties.ContentDescription] else emptyList()
        said.any { it.startsWith("FERB:") && "Nora" in it }
    }, useUnmergedTree = true)

    private fun toEnd(rows: List<ChatRow>) = compose.onNodeWithTag("chat-list").performScrollToIndex(rows.lastIndex)

    // decision: APP-1146
    @Test
    fun a_swipe_under_way_the_bubble_follows_and_the_arrow_comes_out() {
        val rows = ChatModel.rows(ReplyTest.chat())
        compose.setContent { Frame(base.copy(rows = rows)) }
        toEnd(rows)
        answerBubble().performTouchInput {
            down(centerLeft + Offset(24f, 0f))
            repeat(8) { moveBy(Offset(9f, 0f)) }
        }
        shoot("reply-swipe-under-way.png")
        answerBubble().performTouchInput { up() }
    }

    // decision: APP-1146
    @Test
    fun the_quote_waits_above_the_message_box() {
        val rows = ChatModel.rows(ReplyTest.chat())
        vm.reply(answerQuote)
        compose.setContent { Frame(base.copy(rows = rows)) }
        toEnd(rows)
        shoot("reply-composer-quote.png")
    }

    // decision: APP-1146
    @Test
    fun his_sent_and_his_waiting_message_show_what_they_quote() {
        val owner = ReplyQuote("m1", "message", true, "Book a table for two at 8")
        val rows = ChatModel.rows(ReplyTest.chat()) +
            ChatRow.Outbox("o:1", OutboxItem("1", "Two people, not three", "2026-01-02T10:00:00+00:00", "waiting", "link", null, false, emptyList(), owner))
        compose.setContent { Frame(base.copy(rows = rows)) }
        toEnd(rows)
        shoot("reply-sent-quote.png")
    }

    // decision: APP-1146
    @Test
    fun the_quoted_message_lights_up_for_a_moment() {
        val rows = ChatModel.rows(ReplyTest.chat())
        compose.setContent { Frame(base.copy(rows = rows)) }
        toEnd(rows)
        vm.quotedReached(FerbViewModel.Quoted("m1", "a1", 1))
        compose.mainClock.autoAdvance = false
        repeat(15) { compose.mainClock.advanceTimeByFrame() }
        compose.onNodeWithTag("phone").captureRoboImage("src/test/screenshots/reply/reply-lit.png", roborazziOptions = ScreenShotsTest.options)
    }

    // decision: APP-1146, APP-1147
    @Test
    fun a_swipe_far_enough_quotes_the_bubble_a_short_one_or_a_scroll_does_not() {
        val rows = ChatModel.rows(ReplyTest.chat())
        compose.setContent { Frame(base.copy(rows = rows)) }
        val bubble = compose.onNodeWithContentDescription("You: Book a table for two at 8", useUnmergedTree = true)
        bubble.performTouchInput {
            down(centerLeft + Offset(10f, 0f))
            repeat(4) { moveBy(Offset(8f, 0f)) }
            up()
        }
        compose.waitForIdle()
        assertNull("short of its reach: no reply", vm.replyQuote.value)
        bubble.performTouchInput { swipeUp() }
        compose.waitForIdle()
        assertNull("up and down is the chat's scroll", vm.replyQuote.value)
        compose.onNodeWithTag("chat-list").performScrollToIndex(0)
        val swiped = compose.onNodeWithContentDescription("You: Book a table for two at 8", useUnmergedTree = true)
        val top = swiped.fetchSemanticsNode().boundsInRoot.top
        swiped.performTouchInput {
            down(centerLeft + Offset(10f, 0f))
            repeat(12) { moveBy(Offset(10f, 1f)) }
            up()
        }
        compose.waitForIdle()
        assertEquals(ReplyQuote("m1", "message", true, "Book a table for two at 8"), vm.replyQuote.value)
        // the quote coming into the box never moves what he is reading (APP-1147's rule: the chat doesn't jump)
        assertEquals(top, swiped.fetchSemanticsNode().boundsInRoot.top)
        // the quote is in the box, in FERB.app's words
        compose.onNodeWithContentDescription("Replying to your message: Book a table for two at 8").assertExists()
        compose.onNodeWithContentDescription("Don't reply to this message").performClick()
        compose.waitForIdle()
        assertNull(vm.replyQuote.value)
    }

    // decision: APP-1146
    @Test
    fun talkback_offers_reply_on_each_bubble_it_can_quote() {
        val rows = ChatModel.rows(ReplyTest.chat())
        compose.setContent { Frame(base.copy(rows = rows)) }
        toEnd(rows)
        val node = answerBubble().fetchSemanticsNode()
        val actions = node.config[SemanticsActions.CustomActions]
        assertEquals(listOf("Reply"), actions.map { it.label })
        compose.runOnUiThread { actions.single().action() }
        compose.waitForIdle()
        assertEquals("a1", vm.replyQuote.value?.entryId)
    }

    // decision: APP-1146
    @Test
    fun a_tap_on_a_quote_brings_its_message_into_view_and_lights_it() {
        session.unlocked()
        phone.view = long()
        compose.waitUntil(5_000) { vm.ui.value.rows.isNotEmpty() }
        val rows = ChatModel.rows(long())
        compose.setContent { Frame(base.copy(rows = rows, days = vm.ui.value.days, chatDay = vm.ui.value.chatDay)) }
        toEnd(rows)
        val answer = rows.indexOfFirst { it is ChatRow.Answer && it.quote?.entryId == "a1" }
        // every value the light takes (it is on only briefly)
        val lit = java.util.Collections.synchronizedList(ArrayList<String?>())
        val watch = kotlinx.coroutines.CoroutineScope(kotlinx.coroutines.Dispatchers.Unconfined).launch { vm.quoteLit.collect { lit += it } }
        answerBubble().assertIsNotDisplayed()
        compose.onNodeWithContentDescription("Replying to FERB: Booked at Nora's for 8 pm.").performClick()
        // the library is asked on the IO threads, which the chat's idling doesn't count: look again until it answered
        val end = System.currentTimeMillis() + 5_000
        while ("a1" !in lit.toList() && System.currentTimeMillis() < end) { compose.waitForIdle(); Thread.sleep(10) }
        compose.waitForIdle()
        watch.cancel()
        assertTrue("the answer lit up", "a1" in lit.toList())
        assertNull("the tap was taken", vm.quoted.value)
        answerBubble().assertIsDisplayed()
        assertFalse("its day is today: today's chat stays", vm.earlier.value)
        assertTrue(answer >= 0)
    }

    /** [ReplyTest.today]'s chat with twelve more turns between the answer and his reply to it, so it scrolls. */
    private fun long(): TranscriptView = ReplyTest.today(fillers = 12)
}
