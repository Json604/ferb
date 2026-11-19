package com.ferb.app

import android.app.Application
import com.ferb.app.Parity.answer
import com.ferb.app.Parity.entry
import com.ferb.app.Parity.message
import com.ferb.app.model.ChatModel
import com.ferb.app.model.ChatRow
import com.ferb.app.model.Present
import com.ferb.app.model.RunningModel
import com.ferb.app.model.Thumbnails
import com.ferb.design.components.ResultShown
import com.ferb.design.components.RichBlock
import com.ferb.mobile.Item
import com.ferb.mobile.LiveView
import com.ferb.mobile.PreviewFrame
import kotlinx.coroutines.CoroutineScope
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

/**
 * The chat drawn as FERB.app draws it (kb PHN-720..PHN-729), through the real phone library: the research answer the
 * owner showed, each result card by its kind (none drawn as a generic row), an unknown kind, the files once, the
 * question's words, a notice's box, and the work row of a one-part task with its live pictures' rules.
 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class ShownTest {
    @Before fun setUp() { Dispatchers.setMain(UnconfinedTestDispatcher()) }
    @After fun tearDown() { Dispatchers.resetMain() }

    private fun answers(rows: List<ChatRow>) = rows.filterIsInstance<ChatRow.Answer>()

    // decision: PHN-720, PHN-721, PHN-723
    @Test
    fun the_research_answer_is_rich_text_with_one_sites_row_as_on_the_mac() {
        val rows = ChatModel.rows(Parity.research())
        val a = answers(rows).single().ui
        assertEquals(Parity.HOSTS, a.sites)
        val words = ChatModel.words(answers(rows).single())
        assertFalse("the sites line is said nowhere: $words", words.contains("Sites contacted"))
        assertFalse("no address written out", words.contains("(https://"))
        assertTrue("no card stands for a row of its own", rows.none { it is ChatRow.Cards || it is ChatRow.Results })
        val runs = a.blocks.flatMap { b -> when (b) { is RichBlock.Paragraph -> b.runs; is RichBlock.Item -> b.runs; else -> emptyList() } }
        assertTrue("the lead sentence is bold", runs.first().bold)
        assertTrue("the list keeps its bold terms", a.blocks.filterIsInstance<RichBlock.Item>().all { it.runs.first().bold })
        assertEquals("https://www.linkedin.com/legal/user-agreement", runs.first { it.link != null }.link)
        assertTrue("code stays code", runs.any { it.code && it.text == "isTrusted" })
        assertTrue(a.bubble && a.lines.isEmpty() && a.files.isEmpty())
    }

    private fun cardsOf(vararg cards: Map<String, Any?>, text: String = "All set."): List<ResultShown> {
        val rows = ChatModel.rows(Parity.view(message("m1", "r1", "do it"), answer("a1", "r1", "m1", text, cards = cards.toList())))
        val a = answers(rows).single().ui
        return a.above + a.below
    }

    // decision: PHN-724
    @Test
    fun every_card_kind_is_drawn_as_the_mac_draws_it_never_as_a_generic_row() {
        val due = "2026-01-02T09:00:00+00:00"
        val reminder = cardsOf(mapOf("type" to "reminder", "title" to "Stretch", "timer_id" to "t9", "due" to due, "repeat" to "every day")).single()
        assertTrue(reminder is ResultShown.Reminder && reminder.title == "Stretch" && reminder.repeats && reminder.due!!.startsWith("Due "))
        val timer = cardsOf(mapOf("type" to "event", "kind" to "timer", "what" to "Tea", "id" to "x1", "when" to due)).single()
        assertEquals("Timer set", (timer as ResultShown.Event).meta)
        val set = cardsOf(mapOf("type" to "event", "kind" to "reminder", "what" to "Call mum", "id" to "x2", "when" to due)).single()
        assertEquals("Reminder set", (set as ResultShown.Event).meta)
        val changed = cardsOf(mapOf("type" to "event", "kind" to "schedule", "what" to "News", "id" to "x3", "changed" to true,
            "was_repeat" to "every day", "repeat" to "every Monday")).single() as ResultShown.Event
        assertEquals("Schedule changed", changed.meta)
        assertEquals(listOf("Was" to "every day", "Now" to "every Monday"), changed.changes)
        val note = cardsOf(mapOf("type" to "note", "item_id" to "m1", "text" to "Likes tea", "state" to "proposed")).single()
        assertEquals("Saved in my words · waits for your Keep", (note as ResultShown.Note).meta)
        val pay = cardsOf(mapOf("type" to "payment", "amount" to "$5.00", "merchant" to "Cafe", "site" to "cafe.example", "card" to "Visa (personal)")).single()
        assertEquals(ResultShown.Payment("Cafe", "cafe.example", "Visa (personal)", "$5.00"), pay)
        val send = cardsOf(mapOf("type" to "send", "recipient" to "Ana Lopez", "kind" to "mailbox_send", "site" to "mail.example", "account" to "me@x.example")).single()
        send as ResultShown.Send
        assertEquals("AL", send.initials); assertEquals("Emailed · mail.example", send.verb); assertEquals("me@x.example", send.account)
        val schedules = cardsOf(mapOf("type" to "schedules", "schedules" to listOf(mapOf("id" to "s", "name" to "News", "repeat" to "every day", "state" to "paused")))).single()
        schedules as ResultShown.Schedules
        assertEquals("1 schedule", schedules.meta); assertEquals("paused", schedules.rows.single().state); assertEquals("Every day", schedules.rows.single().rule)
    }

    // decision: PHN-724
    @Test
    fun a_kind_this_build_doesnt_know_draws_nothing_and_its_line_stays_in_the_words() {
        val rows = ChatModel.rows(Parity.view(message("m1", "r1", "pay it"),
            answer("a1", "r1", "m1", "Receipt saved\nAll done.", cards = listOf(mapOf("type" to "receipt", "line" to "Receipt saved", "total" to "5")))))
        val a = answers(rows).single()
        assertTrue(a.ui.above.isEmpty() && a.ui.below.isEmpty())
        assertTrue(ChatModel.words(a).contains("Receipt saved"))
        assertEquals(1, ChatModel.words(a).split("Receipt saved").size - 1)
    }

    // decision: PHN-535, PHN-724
    @Test
    fun a_file_card_and_the_tasks_files_show_each_file_once_tappable_by_its_place() {
        val rows = ChatModel.rows(Parity.view(message("m1", "r1", "make it"),
            answer("a1", "r1", "m1", "Made it.", cards = listOf(mapOf("type" to "file", "path" to "/U/Desktop/a.pdf")),
                artifacts = listOf("/U/Desktop/a.pdf", "/U/Desktop/b.txt"))))
        val files = answers(rows).single().ui.files
        assertEquals(listOf("a.pdf" to 0, "b.txt" to 1), files.map { it.title to it.index })
        assertTrue(files.all { it.taskId == "t-1" })
    }

    // decision: PHN-770
    @Test
    fun an_open_question_shows_the_words_the_macs_inline_question_leads_with() {
        val rows = ChatModel.rows(Parity.view(message("m1", "r1", "send it"),
            entry("q1", "question", "r1", mapOf("approval_id" to "ap-1", "kind" to "approval", "title" to "Send the email to Ana?",
                "reason" to "Ana is someone new.", "options" to listOf("No", "Yes")), task = "t-1", state = "open")))
        val q = rows.filterIsInstance<ChatRow.Asking>().single()
        // FERB.app's card: its header ("title") over its one question (the reason, with no summary): kb PHN-739
        assertEquals("Ana is someone new.", q.title)
        assertEquals("Send the email to Ana?", q.reason)
    }

    // decision: PHN-724
    @Test
    fun a_reminders_arrival_is_its_one_box_with_its_own_words() {
        val rows = ChatModel.rows(Parity.view(message("m1", "r1", "hello"),
            entry("n1", "post", "r1", mapOf("text" to "Stand up and stretch for two minutes", "timer_id" to "t5",
                "card" to mapOf("type" to "reminder", "title" to "Stretch", "kind" to "reminder", "timer_id" to "t5")))))
        val n = rows.filterIsInstance<ChatRow.Notice>().single()
        assertTrue("no bubble repeats it", n.ui.boxSays)
        val box = n.ui.above.single() as ResultShown.Reminder
        assertEquals("Stretch", box.title)
        assertEquals("Stand up and stretch for two minutes", box.words)
    }

    /** The phone's own shortcut for plain words gives exactly what the library would, for every fixture's words. */
    // decision: PHN-722
    @Test
    fun plain_words_laid_out_here_equal_the_librarys() {
        var checked = 0
        val texts = Fixtures.conversations().flatMap { c ->
            FixtureMac.transcript(FixtureMac(c)).items.flatMap { i ->
                when (i) {
                    is Item.Turn -> listOfNotNull(i.turn.answer?.text, i.turn.firstReply?.planLine, i.turn.question)
                    is Item.Notice -> listOf(i.notice.text)
                    else -> emptyList()
                }
            }
        } + listOf("One line", "Two\nlines\n\nand a paragraph", "  indented", "1 apple", "A sentence. Another one.")
        for (t in texts) {
            val fast = Present.plainOnly(t) ?: continue
            val lib = com.ferb.mobile.richText(t).map { b -> (b as com.ferb.mobile.RichBlock.Paragraph).spans.joinToString("") { it.text } }
            assertEquals(t, lib, fast.map { (it as RichBlock.Paragraph).runs.single().text })
            checked++
        }
        assertTrue("$checked plain texts", checked > 20)
    }

    // decision: PHN-725, PHN-730
    @Test
    fun a_one_part_task_runs_in_the_work_row_with_what_it_does_now() {
        val view = Parity.view(message("m1", "r1", "search online and then tell"),
            entry("p1", "plan", "r1", mapOf("text" to "I’ll research it.", "plan_line" to "I’ll research it.", "lines" to emptyList<String>(),
                "name" to "Research automation detection"), task = "t-1"))
        val turn = view.items.filterIsInstance<Item.Turn>().single().turn
        assertEquals("working", turn.status)
        val live = view.copy(live = listOf(LiveView(turn.entry, "r1", "running", false, null, null, null, null, null, emptyList(), emptyList(), emptyList(),
            current = "Searching for extension detection")))
        val r = RunningModel.of(live)!!
        assertEquals("1 active task", r.words.lead)
        assertTrue(r.running)
        assertEquals("Research automation detection", r.tasks.single().title)
        assertEquals("Searching for extension detection", r.tasks.single().line)
        assertEquals("t-1", r.tasks.single().taskId)
        assertTrue("its time counts from the plan", r.since != null)
        // before its first step line: FERB.app's "Starting task…"
        assertEquals("Starting task…", RunningModel.of(view)!!.tasks.single().line)
        val paused = RunningModel.of(view.copy(live = listOf(live.live.single().copy(state = "paused"))))!!
        assertEquals("1 paused task", paused.words.lead); assertFalse(paused.running)
    }

    // decision: PHN-726
    @Test
    fun the_pictures_come_at_their_pace_for_at_most_four_and_stop_when_they_go() {
        val phone = FakePhone()
        val pics = Thumbnails(CoroutineScope(Dispatchers.Main), phone, decode = { androidx.compose.ui.graphics.ImageBitmap(2, 2) })
        pics.show(listOf("t1", "t2", Thumbnails.key("t3", "t3.s1"), "t4", "t5"))
        assertEquals(listOf("thumbnail:t1:2000", "thumbnail:t2:2000", "step-thumbnail:t3:t3.s1:2000", "thumbnail:t4:2000"),
            phone.calls.filter { it.contains("thumbnail") })
        phone.thumbnails["t1"]!!.frame(PreviewFrame(byteArrayOf(1, 2, 3), 1, "working", null, true, false, false, null))
        assertEquals(setOf("t1"), pics.pictures.value.keys)
        pics.show(emptyList())
        assertEquals(4, phone.thumbnailsStopped)
        assertTrue("the pictures go with the row", pics.pictures.value.isEmpty())
    }

    private fun finishedCard(): List<ChatRow> {
        val second = (1..12).joinToString("\n") { "Line $it of the second part's answer, with a source: [Chrome docs](https://developer.chrome.com/docs) and **a bold term**." }
        val card = mapOf("request_id" to "r1", "task_id" to "t-1", "total" to 2.0, "done" to 2.0, "line" to "2 of 2 done",
            "lines" to listOf("Sites contacted: developer.chrome.com"),
            "rows" to listOf(
                mapOf("part" to "a", "label" to "Find the extension rules", "state" to "done", "step" to "s1",
                    "text" to "Extensions may automate pages only with the user's consent. See [the policy](https://developer.chrome.com/policy)."),
                mapOf("part" to "b", "label" to "Check what sites can see", "state" to "done", "step" to "s2", "text" to second, "note" to "the rate limits")))
        return ChatModel.rows(Parity.view(message("m1", "r1", "two parts"),
            entry("p1", "plan", "r1", mapOf("text" to "Two parts.", "plan_line" to "Two parts.", "lines" to emptyList<String>(), "card" to card), task = "t-1"),
            answer("a1", "r1", "m1", "1. Extensions…\n2. Sites…\n\nSites contacted: developer.chrome.com", lines = listOf("Sites contacted: developer.chrome.com"),
                cards = listOf(mapOf("type" to "link", "host" to "developer.chrome.com", "line" to "Sites contacted: developer.chrome.com")))))
    }

    /** The owner's finished two-part task: each part's answer under its title, not only its tick (FERB.app's CardRowView). */
    // decision: PHN-733
    @Test
    fun a_finished_card_shows_each_parts_answer_as_the_mac_does() {
        val rows = finishedCard()
        val t = rows.filterIsInstance<ChatRow.Task>().single()
        assertTrue("a done part shows its answer", t.parts[0].rich != null && t.parts[0].detail == null)
        assertEquals("Couldn't confirm: ", t.parts[1].notes.first().label)
        assertEquals("2 of 2 done", t.footer)
        assertTrue("the sites line is the chip under the card", t.footerLines.isEmpty())
        assertTrue(rows.filterIsInstance<ChatRow.Answer>().any { it.ui.sites == listOf("developer.chrome.com") })
    }

    /** A running part shows its own step's picture; the work row then leaves that task's circle out (APP-891). */
    // decision: PHN-726, PHN-731
    @Test
    fun a_running_part_shows_its_steps_picture_and_the_work_row_leaves_its_circle_out() {
        val card = mapOf("request_id" to "r1", "task_id" to "t-1", "total" to 2.0,
            "rows" to listOf(mapOf("part" to "a", "label" to "Review source materials", "state" to "working", "step" to "s1", "step_task_id" to "t-1.s1"),
                mapOf("part" to "b", "label" to "Write the summary", "state" to "working", "step" to "s2", "step_task_id" to "t-1.s2")))
        val v = Parity.view(message("m1", "r1", "two parts"),
            entry("p1", "plan", "r1", mapOf("text" to "Two parts.", "plan_line" to "Two parts.", "lines" to emptyList<String>(), "card" to card), task = "t-1"))
        val lv = v.live.single()
        val live = v.copy(live = listOf(lv.copy(rows = listOf(
            com.ferb.mobile.LiveRow("a", "retrying", "Trying again (attempt 2) · reading", true, null),
            com.ferb.mobile.LiveRow("b", "waiting", "Waiting for Review source materials", false, null)), cardLine = "0 of 2 done · 1 running · 1 waiting")))
        val t = ChatModel.rows(live).filterIsInstance<ChatRow.Task>().single()
        assertEquals(listOf("t-1.s1", null), t.watch)
        assertEquals("0 of 2 done · 1 running · 1 waiting", t.status)
    }

    private fun vm(phone: FakePhone) = FerbViewModel(phone, Session { 0L }, Prefs(androidx.test.core.app.ApplicationProvider.getApplicationContext()), { true }, null,
        androidx.lifecycle.SavedStateHandle(), com.ferb.app.model.LocalFiles(java.nio.file.Files.createTempDirectory("ferb-shown").toFile()) { null })

    /** His choice B: the small circles follow his switch; the short view and Watch live always block. */
    // decision: PHN-727, PHN-732
    @Test
    fun small_pictures_follow_the_switch_and_the_big_views_always_block() {
        val phone = FakePhone()
        val vm = vm(phone)
        vm.thumbnails.show(listOf("t-1"))
        phone.thumbnails["t-1"]?.frame(PreviewFrame(byteArrayOf(1), 1, "working", null, true, false, false, null))
        assertFalse("a small picture on screen doesn't force the block", vm.mustSecure.value)
        vm.showTask("t-1", "t-1.s1")
        assertTrue("a step's short view forces it", vm.mustSecure.value)
        vm.show(null)
        assertFalse(vm.mustSecure.value)
        vm.rowPictures("card", listOf(com.ferb.app.model.Thumbnails.key("t-1", "t-1.s1")))
        assertEquals(setOf("t-1"), vm.tasksWithRowPictures.value)
        vm.rowPictures("card", emptyList())
        assertTrue(vm.tasksWithRowPictures.value.isEmpty())
    }

    /** FERB.app's CollapsePolicy on the phone: an answer past the chat's height to half of it; a row past 8 lines to 3. */
    // decision: PHN-728
    @Test
    fun long_texts_fold_as_the_macs_do() {
        val fold = com.ferb.design.components.FoldRule
        val ANSWER = com.ferb.design.components.Fold.ANSWER
        val ROW = com.ferb.design.components.Fold.ROW
        assertEquals(null, fold.lines(ANSWER, 20, 20f, 800f))
        assertEquals(20, fold.lines(ANSWER, 60, 20f, 800f))
        assertEquals("never to hide under 3 lines", null, fold.lines(ANSWER, 3, 20f, 40f))
        assertEquals(null, fold.lines(ROW, 8, 20f, 0f))
        assertEquals(3, fold.lines(ROW, 9, 20f, 0f))
    }

    /** FERB.app's card: amber while it asks, the steps a waiting row waits on under it, the connection line's time. */
    // decision: PHN-736, PHN-737
    @Test
    fun a_card_asks_in_amber_shows_its_waiting_rows_steps_and_the_line_ticks() {
        val card = mapOf("request_id" to "r1", "task_id" to "t-1", "total" to 1.0,
            "plan" to listOf(mapOf("id" to "s1", "label" to "Read the notes", "step_task_id" to "t-1.s1"),
                mapOf("id" to "s2", "label" to "Write the email", "step_task_id" to "t-1.s2", "depends_on" to listOf("s1"))),
            "rows" to listOf(mapOf("part" to "a", "label" to "Draft the email", "state" to "asks", "step" to "s2")))
        val v = Parity.view(message("m1", "r1", "draft it"),
            entry("p1", "plan", "r1", mapOf("text" to "On it.", "plan_line" to "On it.", "lines" to emptyList<String>(), "card" to card), task = "t-1"))
        val lv = v.live.single()
        val waiting = com.ferb.mobile.LiveRow("a", "waiting", "Waiting for Read the notes", false, null)
        val feeder = com.ferb.mobile.LiveFeeder("a", "s1", "Read the notes", "t-1.s1", com.ferb.mobile.LiveRow("s1", "running", "Reading", true, null))
        val live = v.copy(live = listOf(lv.copy(rows = listOf(waiting), feeders = listOf(feeder),
            line = com.ferb.mobile.LiveLine("Reconnected to the service", "2026-01-02T09:00:00+00:00"))))
        val rows = ChatModel.rows(live)
        val t = rows.filterIsInstance<ChatRow.Task>().single()
        assertEquals(com.ferb.design.components.CardTint.ASKING, t.tint)
        assertEquals(listOf("Read the notes"), t.parts.single().sub.map { it.title })
        val line = rows.filterIsInstance<ChatRow.Line>().first { it.key.endsWith(":line") }
        assertEquals(1_767_344_400_000L, line.since)
    }

    /** A reply still coming says so; the work row also counts the Mac's tasks started elsewhere. */
    // decision: PHN-736
    @Test
    fun a_streamed_reply_is_a_draft_and_the_work_row_counts_the_macs_other_tasks() {
        val v = Parity.view(message("m1", "r1", "hello", task = null))
        val lv = v.live.single()
        val rows = ChatModel.rows(v.copy(live = listOf(lv.copy(streamingReply = "Hel"))))
        assertTrue(rows.any { it is ChatRow.Line && it.text == "Draft · still responding" })
        val mac = listOf(com.ferb.mobile.TaskRow("t-9", "Morning news", "Read the news", "working", null, null, "", null, null))
        val r = com.ferb.app.model.RunningModel.of(v, mac)!!
        assertEquals("1 active task", r.words.lead)
        assertEquals("Morning news", r.tasks.single().title)
    }

    /** The owner's case: the Mac's question, its reason and the address; never the step line (kb PHN-739). */
    // decision: PHN-739
    @Test
    fun a_question_reads_as_the_macs_card_in_the_sheet_the_row_and_the_tasks_tab() {
        val body = """{"approval_id":"q1","capability":"ferb.confirm","kind":"owner_question","reason":"FERB: this types ps@microsoft.com. The step: browser.act action=type text=ps@microsoft.com. Go ahead?","options":["Yes","No"],"summary":"Type this email address on mail.google.com?","why":"Add Pratyay as the first main To recipient.","exact":"ps@microsoft.com","title":"Check before FERB goes on","labels":["Type it","Don't type it"]}"""
        val item = com.ferb.mobile.QuestionItem("e-q1", "q1", "r1", "t-1", "open", true, body, "end_of_chat", null)
        val q = com.ferb.app.model.Questions.parse(item)
        assertEquals("Type this email address on mail.google.com?", q.title)
        assertEquals("Check before FERB goes on", q.shown!!.header)
        assertEquals("The task's reason: Add Pratyay as the first main To recipient.", q.shown!!.why)
        assertEquals(listOf("Don't type it", "Type it"), q.choices.map { it.label })
        val v = Parity.view(message("m1", "r1", "write to Pratyay"))
        val rows = ChatModel.rows(v.copy(questions = listOf(item)))
        val row = rows.filterIsInstance<ChatRow.Asking>().single()
        assertEquals("Type this email address on mail.google.com?" to "Check before FERB goes on", row.title to row.reason)
        assertTrue(rows.none { r -> ChatModel.words(r).contains("The step:") })
    }
}
