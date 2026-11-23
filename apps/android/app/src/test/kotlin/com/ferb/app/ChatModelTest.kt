package com.ferb.app

import com.ferb.app.model.ChatModel
import com.ferb.app.model.ChatRow
import com.ferb.app.model.Questions
import com.ferb.app.model.WorkModel
import com.ferb.design.components.MarkKind
import com.ferb.mobile.Item
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Every conversation of the engine's chat fixtures (the Mac's own scenarios: gaps, sealed IDs, legacy rows, deleted
 * turns, another device's turn, questions answered and left open), synced by the real phone library from a fixture
 * Mac and turned into the app's rows: nothing is lost on the way to the screen.
 */
class ChatModelTest {
    private val all = Fixtures.conversations()

    // decision: PHN-473, PHN-416, PHN-401
    @Test
    fun every_fixture_conversation_reaches_the_screen_whole() {
        var turns = 0
        var answers = 0
        var asks = 0
        for (c in all) {
            val view = FixtureMac.transcript(FixtureMac(c))
            val rows = ChatModel.rows(view)
            val keys = rows.map { it.key }
            assertEquals("${c.scenario}: row keys are unique", keys.size, keys.toSet().size)
            val text = rows.joinToString("\n") { ChatModel.words(it) }
            for (item in view.items) {
                if (item !is Item.Turn) continue
                val t = item.turn
                turns++
                t.words?.text?.takeIf { it.isNotBlank() && t.from == "owner" }?.let {
                    assertTrue("${c.scenario}: the owner's words '$it' are drawn", text.contains(it))
                }
                val a = t.answer
                if (a != null && !t.answeredBelow && t.card == null && (a.displayText ?: a.text).isNotBlank()) {
                    answers++
                    // drawn as FERB.app draws it (its words, cards, sites and files laid out by the library: kb PHN-723)
                    assertTrue("${c.scenario}: the answer of ${t.entry} is drawn", rows.any { it is ChatRow.Answer && it.key == "${t.entry}:answer" })
                }
            }
            val open = Questions.open(view.questions)
            asks += open.size
            for (q in open) assertTrue("${c.scenario}: open question ${q.approvalId} is in the chat", rows.any { it is ChatRow.Asking && it.approvalId == q.approvalId })
            // the Tasks tab reads the same transcript without failing
            WorkModel.of(view, "")
        }
        assertTrue("the fixtures hold turns ($turns), answers ($answers) and open questions ($asks)", turns > 100 && answers > 50 && asks > 0)
    }

    // decision: ORCH-651, PHN-473
    @Test
    fun a_delivered_with_gaps_answer_keeps_its_not_done_lines() {
        val c = all.first { it.scenario == "gaps_single_task" }
        val rows = ChatModel.rows(FixtureMac.transcript(FixtureMac(c)))
        assertTrue(rows.filterIsInstance<ChatRow.Answer>().any { ChatModel.words(it).contains("Not done:") })
    }

    // decision: PHN-535
    @Test
    fun a_finished_tasks_files_on_its_card_carry_the_task_and_their_place() {
        var files = 0
        for (c in all) {
            val view = FixtureMac.transcript(FixtureMac(c))
            val lines = ChatModel.rows(view).filterIsInstance<ChatRow.Answer>().flatMap { it.ui.files }.filter { it.index != null }
            for (t in view.items.mapNotNull { (it as? com.ferb.mobile.Item.Turn)?.turn }) {
                val a = t.answer ?: continue
                if (t.answeredBelow || t.taskId == null) continue
                a.artifacts.forEachIndexed { i, path ->
                    files++
                    assertTrue("${c.scenario}: $path is tappable by its task and place",
                        lines.any { it.taskId == t.taskId && it.index == i && it.title == ChatModel.fileName(path) })
                }
            }
        }
        assertTrue("the fixtures hold task files ($files)", files > 0)
    }

    // decision: ENG-12
    @Test
    fun legacy_rows_and_a_turn_from_another_device_are_drawn_like_any_turn() {
        for (name in listOf("legacy_then_new", "turn_from_another_client")) {
            val c = all.first { it.scenario == name }
            val rows = ChatModel.rows(FixtureMac.transcript(FixtureMac(c)))
            assertTrue("$name has owner rows", rows.any { it is ChatRow.Owner })
            assertTrue("$name has FERB's rows", rows.any { it is ChatRow.Answer || it is ChatRow.Rich || it is ChatRow.Ferb })
        }
    }

    // The owner's live case (2026-11-23): FERB's questions sat above his newest message on the phone, under the first
    // turn of their task, while FERB.app draws them at the end of the chat (ConversationTranscript: the entries, then
    // the open questions), a card row's question in its row. `question_left_open` as the Mac stores it: "Which colour?"
    // is the paint card's row p2, "Move the screenshots?" is asked at the end; with its row taken away, "Which
    // colour?" is a loose question asked before "tidy my desktop", and goes to the end too.
    // decision: PHN-824
    @Test
    fun an_open_question_is_where_ferb_app_draws_it_never_above_newer_words() {
        val c = all.first { it.scenario == "question_left_open" }
        fun asked(rows: List<ChatRow>) = rows.withIndex().filter { it.value is ChatRow.Asking }.map { (it.value as ChatRow.Asking).approvalId to it.index }
        val tidy = { rows: List<ChatRow> -> rows.indexOfFirst { it is ChatRow.Owner && it.text == "tidy my desktop" } }

        val card = ChatModel.rows(FixtureMac.transcript(FixtureMac(c)))
        val inCard = asked(card).toMap()
        assertTrue("the card row's question is in its turn, before the next message", inCard.getValue("ap-1") < tidy(card))
        assertTrue("the other at the end", inCard.getValue("ap-2") > tidy(card))

        @Suppress("UNCHECKED_CAST")
        val loose = c.copy(entries = c.entries.map { e ->
            if (e["approval_id"] == "ap-1") e + ("body" to ((e["body"] as Map<String, Any?>) + ("part" to null))) else e
        })
        val rows = ChatModel.rows(FixtureMac.transcript(FixtureMac(loose)))
        assertEquals(listOf("ap-1", "ap-2"), asked(rows).map { it.first })
        assertTrue("every open question comes after his newer message", asked(rows).all { it.second > tidy(rows) })
        assertTrue("and after everything said: the questions close the chat", rows.takeLast(2).all { it is ChatRow.Asking })
    }

    @Test
    fun marks_follow_the_macs_states_and_never_colour_alone() {
        assertEquals(MarkKind.DONE, ChatModel.markOf("done_with_gaps"))
        assertEquals(MarkKind.ASKING, ChatModel.markOf("needsYou"))
        assertEquals(MarkKind.FAILED, ChatModel.markOf("wontRun"))
        assertEquals(MarkKind.WAITING, ChatModel.markOf("waiting"))
        assertEquals(MarkKind.RUNNING, ChatModel.markOf("retrying"))
    }

    @Test
    fun markdown_marks_that_only_style_are_dropped() {
        // a one-line summary (the Tasks tab) says a link's words, never its address (kb PHN-721)
        assertEquals("Done: the report", ChatModel.plain("**Done:** [the report](https://x.example)"))
        assertEquals("Title\n• one", ChatModel.plain("## Title\n- one"))
    }
}
