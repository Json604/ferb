package com.ferb.app

import com.ferb.app.model.ChatModel
import com.ferb.app.model.ChatRow
import com.ferb.mobile.ImportCard
import com.ferb.mobile.ImportRow
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The memory import's live card (kb MEM-456; the owner, MEM-455): while the Mac's status has one it is the last row of
 * today's chat, after the open questions; without one there is no row; the earlier days never show it. Its words are
 * the Mac's, carried as they are.
 */
class ImportCardTest {
    private val today = 20_000L

    private fun row(key: String, name: String, line: String, done: Int = 0, total: Int = 0, count: String = "", found: String = "",
                    detail: String = "", note: String = "", stage: String = "reading") =
        ImportRow(key, key.substringAfter(':'), name, stage, line, detail, done.toUInt(), total.toUInt(), count, 0u, found, note)

    private val reading = ImportCard("Bringing in what Claude Code knows about you",
        listOf(row("run:r1", "Claude Code", "Reading what you've typed", 33, 40, "33 of 40", "221 things found so far")))

    private val chat = listOf(
        ChatRow.Owner("y1", "Compare the three standing desks", emptyList(), null),
        ChatRow.Ferb("y2:answer", "The Flexispot E7 is cheapest."),
        ChatRow.Owner("t1", "Bring in what Claude Code knows about me", emptyList(), null),
        ChatRow.Ferb("t2:answer", "Starting now."),
    )
    private val days = mapOf("y1" to today - 1, "y2" to today - 1, "t1" to today, "t2" to today)
    private val places = ChatModel.split(ChatModel.withDays(chat, days, today), today)
    private val loose = listOf<ChatRow>(ChatRow.Asking("q:a1", "a1", "Pay $5?"))

    // decision: MEM-456
    @Test
    fun with_an_import_under_way_its_card_is_the_last_row_of_todays_chat() {
        val card = ChatModel.importRow(reading)!!
        val rows = ChatModel.shown(places, earlier = false, loose = loose, card = card)
        assertEquals(listOf("earlier-link", "t1", "t2:answer", "q:a1", ChatModel.IMPORT_KEY), rows.map { it.key })
        assertTrue(rows.last() is ChatRow.ImportProgress)
        // with nothing else at the end it is still last
        assertEquals(ChatModel.IMPORT_KEY, ChatModel.shown(places, earlier = false, loose = emptyList(), card = card).last().key)
    }

    // decision: MEM-456
    @Test
    fun without_an_import_there_is_no_card() {
        assertNull(ChatModel.importRow(null))
        assertNull("a card with no row is none", ChatModel.importRow(ImportCard("Bringing in", emptyList())))
        val rows = ChatModel.shown(places, earlier = false, loose = loose, card = ChatModel.importRow(null))
        assertEquals(listOf("earlier-link", "t1", "t2:answer", "q:a1"), rows.map { it.key })
        assertTrue(rows.none { it is ChatRow.ImportProgress })
    }

    // decision: MEM-456, PHN-696
    @Test
    fun the_earlier_days_never_show_it() {
        val rows = ChatModel.shown(places, earlier = true, loose = loose, card = ChatModel.importRow(reading))
        assertTrue(rows.none { it is ChatRow.ImportProgress })
        assertEquals(places.earlier, rows)
    }

    // decision: MEM-456
    @Test
    fun the_cards_words_are_the_macs_as_they_are() {
        val mail = "I look in your email for it every 12 hours. It can take a day or two."
        val sites = ImportCard("Bringing in what ChatGPT and Claude know about you", listOf(
            row("site:chatgpt", "ChatGPT", "Waiting for ChatGPT's email — asked at 23:40", detail = mail, stage = "waiting_mail"),
            row("site:claude", "Claude", "Waiting for Claude's email — asked yesterday at 23:41", detail = mail, stage = "waiting_mail",
                note = "The last try didn't go through. I'll try again at 00:25."),
        ))
        val card = ChatModel.importRow(sites)!!
        assertEquals(sites.title, card.title)
        assertEquals(listOf("site:chatgpt", "site:claude"), card.rows.map { it.key })
        assertEquals(listOf("ChatGPT", "Claude"), card.rows.map { it.name })
        assertEquals(listOf(null, null), card.rows.map { it.fraction })
        assertEquals("The last try didn't go through. I'll try again at 00:25.", card.rows[1].note)
        val one = ChatModel.importRow(reading)!!.rows.single()
        assertEquals(33, one.done)
        assertEquals(40, one.total)
        assertEquals(33f / 40f, one.fraction!!, 1e-6f)
        assertEquals("33 of 40", one.count)
        assertEquals("221 things found so far", one.foundLine)
        // what reading the chat aloud would say: the Mac's words, the names only when there are several sources
        assertEquals("Bringing in what Claude Code knows about you\nReading what you've typed\n33 of 40\n221 things found so far",
            ChatModel.words(ChatModel.importRow(reading)!!))
        assertTrue(ChatModel.words(card).contains("ChatGPT\nWaiting for ChatGPT's email"))
    }
}
