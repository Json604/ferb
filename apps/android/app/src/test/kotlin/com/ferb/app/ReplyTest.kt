package com.ferb.app

import android.app.Application
import android.os.Looper
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.ChatModel
import com.ferb.app.model.ChatRow
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.Picked
import com.ferb.app.model.SendPhase
import com.ferb.app.model.Why
import com.ferb.core.KeyKind
import com.ferb.core.Outcome
import com.ferb.design.components.ReplySwipe
import com.ferb.mobile.FerbException
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.ReplyQuote
import com.ferb.mobile.TranscriptView
import java.io.File
import java.nio.file.Files
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestCoroutineScheduler
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/**
 * Reply to a message on the phone (the owner, 2026-11-27, kb APP-1146; FERB.app's ReplyQuotes.swift): every bubble the
 * Mac lets him quote carries its quote from the real phone library, the quote waits above the message box, goes with
 * his next message as `reply_to` (or stays with it in the outbox and across a restart), comes back when the message
 * couldn't go, and a tap on a quote finds its message and lights it.
 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class ReplyTest {
    private val phone = FakePhone()
    private val scheduler = TestCoroutineScheduler()
    private lateinit var session: Session
    private val root: File = Files.createTempDirectory("ferb-reply").toFile()
    private val picked = HashMap<String, ByteArray>()
    private val local = LocalFiles(root) { uri -> picked[uri]?.let { b -> Picked(uri.substringAfterLast('/'), b.size.toLong()) { b.inputStream() } } }

    @Before
    fun setUp() {
        Dispatchers.setMain(UnconfinedTestDispatcher(scheduler))
        session = Session { 0L }
        session.unlocked()
        phone.view = chat()
    }

    @After
    fun tearDown() {
        Dispatchers.resetMain()
        runCatching { root.deleteRecursively() }
    }

    /** [io] Main: everything at once on the test's clock; else the phone's own IO (files are copied and written). */
    private fun vm(io: Boolean = false): FerbViewModel =
        FerbViewModel(phone, session, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(), local,
            io = if (io) Dispatchers.IO else Dispatchers.Main, account = testAccount()).also {
            it.prompts = object : Prompts {
                override fun signer(kind: KeyKind, why: Why) = FakeSigner(kind, false)
                override suspend fun local() = true
            }
        }

    private fun idle() = shadowOf(Looper.getMainLooper()).idle()

    private fun until(what: String, cond: () -> Boolean) {
        val end = System.currentTimeMillis() + 5_000
        while (!cond() && System.currentTimeMillis() < end) { idle(); Thread.sleep(10) }
        idle()
        assertTrue("waited for $what", cond())
    }

    private val ferbsAnswer get() = ChatModel.rows(phone.view).filterIsInstance<ChatRow.Answer>().first { it.quote != null }.quote!!

    // decision: APP-1146
    @Test
    fun every_bubble_offers_its_own_quote_and_his_message_shows_what_it_quotes() {
        val rows = ChatModel.rows(chat())
        val asked = rows.filterIsInstance<ChatRow.Owner>().first { it.key == "m1" }
        assertEquals("his own message: You", ReplyQuote("m1", "message", true, "Book a table for two at 8"), asked.quote)
        val first = rows.filterIsInstance<ChatRow.Rich>().single { it.key == "m1:first" }
        assertEquals("FERB's first reply is its plan entry", "p1", first.quote?.entryId)
        assertFalse(first.quote!!.byOwner)
        val answer = rows.filterIsInstance<ChatRow.Answer>().single { it.key == "m1:answer" }
        assertEquals("the answer, its words on one line without Markdown's marks", ReplyQuote("a1", "answer", false, "Booked at Nora's for 8 pm."), answer.quote)
        val reply = rows.filterIsInstance<ChatRow.Owner>().single { it.key == "m2" }
        assertEquals("his reply shows what it quotes, as the engine kept it", "a1", reply.replyTo?.entryId)
        assertEquals("Booked at Nora's for 8 pm.", reply.replyTo?.excerpt)
        assertNull("a message that quotes nothing shows nothing", asked.replyTo)
        // the tap's way back: the library's item for the answer is its turn, and the row is the answer's bubble
        val item = com.ferb.mobile.quotedItem(chat().items, "a1")
        assertEquals("m1", item)
        assertEquals(rows.indexOf(answer), ChatModel.quotedRow(rows, item!!, "a1"))
        assertEquals("a message no page holds is nowhere", null, com.ferb.mobile.quotedItem(chat().items, "gone"))
    }

    // decision: APP-1146
    @Test
    fun a_reply_waits_above_the_box_goes_with_the_next_message_and_shows_on_it_while_it_waits() {
        phone.link.value = FakePhone.status(LinkPhase.WAITING)
        val vm = vm()
        val q = ferbsAnswer
        vm.reply(q)
        assertEquals(q, vm.replyQuote.value)
        assertTrue("the box takes the cursor, so the keyboard opens", vm.focusAsk.value)
        vm.typed("Make it 8:30")
        vm.send()
        assertEquals("it went as reply_to", listOf<ReplyQuote?>(q), phone.sentQuotes)
        assertEquals("Make it 8:30", phone.sentWith.single().first)
        assertNull("the box's quote went with the words", vm.replyQuote.value)
        assertEquals("", vm.draft.value)
        val waiting = vm.ui.value.rows.filterIsInstance<ChatRow.Outbox>().single()
        assertEquals("the waiting message keeps what it quotes", q, waiting.item.replyTo)
        // a message without a quote goes as before
        vm.typed("And a cake")
        vm.send()
        assertEquals(listOf("send:And a cake"), phone.calls.filter { it.startsWith("send:") })
    }

    // decision: APP-1146
    @Test
    fun a_message_that_could_not_go_keeps_its_words_and_its_quote() {
        val vm = vm()
        val q = ferbsAnswer
        vm.reply(q)
        vm.typed("Make it 8:30")
        phone.sendWithRefusal = Outcome.Failed(FerbException.Store("disk full", null))
        vm.send()
        assertEquals("Make it 8:30", vm.draft.value)
        assertEquals("the quote is back above the box", q, vm.replyQuote.value)
    }

    // decision: APP-1146
    @Test
    fun the_cross_drops_the_quote_and_keeps_the_draft() {
        val vm = vm()
        vm.typed("Make it 8:30")
        assertFalse("nothing to drop: Esc does what it did", vm.dropReplyQuote())
        vm.reply(ferbsAnswer)
        assertTrue(vm.dropReplyQuote())
        assertNull(vm.replyQuote.value)
        assertEquals("Make it 8:30", vm.draft.value)
        vm.send()
        assertEquals("sent without a quote", listOf("send:Make it 8:30"), phone.calls.filter { it.startsWith("send:") })
        assertTrue(phone.sentQuotes.isEmpty())
    }

    // decision: APP-1146
    @Test
    fun the_quote_survives_a_rebuilt_screen() {
        val saved = SavedStateHandle()
        val first = FerbViewModel(phone, session, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, saved, local,
            io = Dispatchers.Main, account = testAccount())
        first.reply(ferbsAnswer)
        val again = FerbViewModel(phone, session, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, saved, local,
            io = Dispatchers.Main, account = testAccount())
        assertEquals(ferbsAnswer, again.replyQuote.value)
    }

    // decision: APP-1146, PHN-691
    @Test
    fun a_message_with_files_keeps_its_quote_while_it_waits_and_across_a_restart() {
        picked["content://picker/IMG_1.jpg"] = ByteArray(30) { 1 }
        phone.link.value = FakePhone.status(LinkPhase.WAITING)
        val first = vm(io = true)
        first.sendFiles(listOf("content://picker/IMG_1.jpg"), forChat = true)
        until("in the box") { first.outgoing.chips.value.size == 1 }
        val q = ferbsAnswer
        first.reply(q)
        first.typed("The menu")
        first.send()
        until("waiting for the Mac") { first.outgoing.messages.value.singleOrNull()?.phase == SendPhase.WAITING_LINK }
        assertEquals(q, first.outgoing.messages.value.single().replyTo)
        assertNull(first.replyQuote.value)
        until("kept") { File(root, "messages.json").takeIf { it.isFile }?.readText()?.contains("\"replyTo\"") == true }
        // FERB closes; a new process reads what was kept
        val vm = vm(io = true)
        until("the message is back") { vm.outgoing.messages.value.isNotEmpty() }
        assertEquals("its quote came back with it", q, vm.outgoing.messages.value.single().replyTo)
        phone.link.value = FakePhone.status()
        until("sent once the Mac is back") { phone.sentWith.isNotEmpty() }
        assertEquals(q, phone.sentQuotes.last())
    }

    // decision: APP-1146
    @Test
    fun a_tapped_quote_finds_its_message_in_its_day_and_lights_it_briefly() {
        val vm = vm()
        // the invented chat is of 2 January: its messages are in the earlier days
        vm.showQuoted(ferbsAnswer)
        val t = vm.quoted.value
        assertNotNull(t)
        assertEquals("the library's item: the answer's turn", "m1", t!!.item)
        assertEquals("a1", t.entryId)
        assertTrue("its day is an earlier one: the earlier days open", vm.earlier.value)
        vm.quotedReached(t)
        assertNull(vm.quoted.value)
        assertEquals("a1", vm.quoteLit.value)
        scheduler.advanceTimeBy(FerbViewModel.QUOTE_LIT_MS + 1)
        assertNull("only briefly", vm.quoteLit.value)
        // a message the phone holds no page of: nothing moves
        vm.backToToday()
        vm.showQuoted(ReplyQuote("not-here", "answer", false, "Something from last year"))
        assertNull(vm.quoted.value)
        assertFalse(vm.earlier.value)
    }

    // decision: APP-1146 — the temporary chat quotes as the chat does: its bubbles offer their quotes, the swipe puts one
    // above the box, it goes with the message as reply_to and comes back when the message couldn't go
    @Test
    fun the_temporary_chat_quotes_as_the_chat_does() {
        phone.temporaryOn = true
        phone.temporaryView = chat()
        val vm = vm()
        session.unlocked()
        assertNotNull("the temporary chat is on", vm.ui.value.temporary)
        val rows = vm.ui.value.temporary!!.rows
        val q = rows.filterIsInstance<ChatRow.Answer>().first { it.quote != null }.quote!!
        vm.reply(q)
        assertEquals("the swipe puts it above the box", q, vm.replyQuote.value)
        assertTrue("taken in the temporary chat", vm.quoteTemporary.value)
        assertTrue(vm.focusAsk.value)
        vm.typed("Make it 8:30")
        vm.send()
        assertEquals(listOf("Make it 8:30"), phone.temporarySent)
        assertEquals("it went as reply_to", listOf<ReplyQuote?>(q), phone.temporaryQuotes)
        assertNull("the box's quote went with the words", vm.replyQuote.value)
        // a message that couldn't go: its words and its quote are back in the box
        vm.reply(q)
        vm.typed("And a window table")
        phone.temporaryRefusal = Outcome.Failed(FerbException.Offline("FERB's Mac can't be reached now.", "mac_unreachable"))
        vm.send()
        assertEquals("And a window table", vm.draft.value)
        assertEquals(q, vm.replyQuote.value)
        assertTrue(vm.quoteTemporary.value)
        // the temporary chat ends: its quote goes with it (the Mac forgot what it quotes)
        phone.temporaryRefusal = null
        vm.toggleTemporary()
        assertNull(vm.ui.value.temporary)
        assertNull(vm.replyQuote.value)
    }

    // decision: APP-1146 — a quote shows only in the chat it was taken in: one of the normal chat waits while the
    // temporary chat is on, and a temporary message goes without it
    @Test
    fun a_quote_of_the_normal_chat_waits_while_the_temporary_chat_is_on() {
        val vm = vm()
        val q = ferbsAnswer
        vm.reply(q)
        assertFalse(vm.quoteTemporary.value)
        vm.toggleTemporary()
        assertNotNull(vm.ui.value.temporary)
        assertFalse("not shown here: Esc does what it did", vm.dropReplyQuote())
        vm.typed("what is 2+2")
        vm.send()
        assertEquals(listOf<ReplyQuote?>(null), phone.temporaryQuotes)
        assertEquals("it still waits for the normal chat", q, vm.replyQuote.value)
        vm.toggleTemporary()
        assertNull(vm.ui.value.temporary)
        vm.typed("Make it 8:30")
        vm.send()
        assertEquals("back in the normal chat it goes with the next message", listOf<ReplyQuote?>(q), phone.sentQuotes)
    }

    // decision: APP-1146
    @Test
    fun the_swipe_follows_the_finger_to_its_reach_then_slows_and_only_a_sideways_swipe_is_one() {
        val reach = 64f
        val most = 96f
        assertEquals(0f, ReplySwipe.pulled(-10f, reach, most))
        assertEquals(40f, ReplySwipe.pulled(40f, reach, most))
        assertEquals(64f, ReplySwipe.pulled(64f, reach, most))
        assertEquals(74f, ReplySwipe.pulled(94f, reach, most))
        assertEquals(most, ReplySwipe.pulled(400f, reach, most))
        assertTrue(ReplySwipe.sideways(20f, 4f))
        assertFalse("to the left is not a reply", ReplySwipe.sideways(-20f, 0f))
        assertFalse("mostly down is the chat's scroll", ReplySwipe.sideways(12f, 10f))
    }

    companion object {
        /** When the invented entries were made: 2 January (the earlier days), or [today]. */
        fun chat(at: String = "2026-01-02T09:00:00+00:00", fillers: Int = 0): TranscriptView {
            val asked = listOf(
                Parity.entry("m1", "message", "r1", mapOf("text" to "Book a table for two at 8", "attachments" to emptyList<String>(), "hidden" to emptyList<String>()),
                    task = "t-1", at = at),
                Parity.entry("p1", "plan", "r1", mapOf("text" to "I'll look for a table for two at 8 tonight.",
                    "plan_line" to "I'll look for a table for two at 8 tonight.", "lines" to emptyList<String>(), "name" to "Book a table"), task = "t-1", at = at),
                Parity.entry("a1", "answer", "r1", mapOf("from" to "task", "text" to "**Booked** at Nora's for 8 pm.", "status" to "done", "verified" to true,
                    "artifacts" to emptyList<String>(), "note" to null, "lines" to emptyList<String>(), "cards" to emptyList<Any>()), task = "t-1", answers = "m1", at = at),
            )
            // turns between the answer and his reply to it, so the chat scrolls
            val between = (1..fillers).flatMap { i ->
                listOf(
                    Parity.entry("f$i", "message", "rf$i", mapOf("text" to "And the next thing, number $i", "attachments" to emptyList<String>(),
                        "hidden" to emptyList<String>()), task = "tf$i", at = at),
                    Parity.entry("fa$i", "answer", "rf$i", mapOf("from" to "task", "text" to "Done: thing $i.", "status" to "done", "verified" to true,
                        "artifacts" to emptyList<String>(), "note" to null, "lines" to emptyList<String>(), "cards" to emptyList<Any>()), task = "tf$i",
                        answers = "f$i", at = at),
                )
            }
            val reply = Parity.entry("m2", "message", "r2", mapOf("text" to "Make it 8:30 and ask for a window table", "attachments" to emptyList<String>(),
                "hidden" to emptyList<String>(),
                "reply_to" to mapOf("entry_id" to "a1", "kind" to "answer", "who" to "ferb", "excerpt" to "Booked at Nora's for 8 pm.")), task = "t-2", at = at)
            val done = Parity.entry("a2", "answer", "r2", mapOf("from" to "task", "text" to "Moved to 8:30, and I asked for a window table.", "status" to "done",
                "verified" to true, "artifacts" to emptyList<String>(), "note" to null, "lines" to emptyList<String>(), "cards" to emptyList<Any>()),
                task = "t-2", answers = "m2", at = at)
            return Parity.view(*(asked + between + reply + done).toTypedArray())
        }

        /** [chat] as made just after this midnight on the phone: today's chat, whenever the test runs. */
        fun today(fillers: Int = 0): TranscriptView = chat(java.time.LocalDate.now().atStartOfDay(java.time.ZoneId.systemDefault()).plusSeconds(1)
            .toOffsetDateTime().toString(), fillers)
    }
}
