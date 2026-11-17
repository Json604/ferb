package com.ferb.app.model

import androidx.compose.ui.graphics.vector.ImageVector
import com.ferb.core.MainGuard
import com.ferb.design.components.CardIcons
import com.ferb.design.components.CountdownShown
import com.ferb.design.components.ResultShown
import com.ferb.design.components.RichBlock
import com.ferb.design.components.RichRun
import com.ferb.design.components.ScheduleShown
import com.ferb.mobile.AnswerView
import com.ferb.mobile.ResultCard
import java.time.Instant
import java.time.LocalDate
import java.time.OffsetDateTime
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.util.Locale

// FERB's words and answers as FERB.app draws them (kb PHN-720..): the phone library lays them out (its `rich_text`,
// `answer_shown`, `notice_shown`: FERB.app's MarkdownDocument, AnswerView and NoticeView in one place for both phones);
// this turns its records into the design's pieces, with the dates in the phone's own format. Every library call here
// runs off the main thread (the rows are built there, kb PHN-682) and each result is kept per entry and text, so a
// redraw, a streamed token or a live line never parses again.

/** An answer ready to draw: the design's blocks, the cards by kind, the sites, the files. */
data class AnswerUi(
    val bubble: Boolean,
    val blocks: List<RichBlock>,
    val copy: String,
    val note: String?,
    val lines: List<String>,
    val details: List<RichBlock>,
    val sites: List<String>,
    val above: List<ResultShown>,
    val below: List<ResultShown>,
    val pages: List<String>,
    val files: List<CardLine>,
    val statusWords: String?,
)

/** A notice's layout (FERB.app's NoticeView): its reminder boxes, whether one says it all, its facts and pages. */
data class NoticeUi(val boxSays: Boolean, val above: List<ResultShown>, val notes: List<ResultShown>, val pages: List<String>)

object Present {
    private const val KEEP = 4_000
    private val kept = object : LinkedHashMap<Any, Any>(64, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<Any, Any>?) = size > KEEP
    }

    @Suppress("UNCHECKED_CAST")
    private fun <T : Any> keep(key: Any, make: () -> T): T {
        synchronized(kept) { kept[key]?.let { return it as T } }
        val made = make()
        synchronized(kept) { kept[key] = made }
        return made
    }

    /**
     * The page a site row opens (FERB.app's VisitedSites.link): https:// and the host only; anything that isn't a plain
     * host (a scheme, a path, a user part, spaces) opens nothing.
     */
    fun siteLink(host: String): String? {
        if (host.isEmpty() || !host.all { it.isLetterOrDigit() || it in ".-:[]" }) return null
        return "https://$host"
    }

    /** FERB's Markdown as the design's blocks (FERB.app's MarkdownDocument). */
    fun rich(text: String): List<RichBlock> = plainOnly(text) ?: keep("r" to text) {
        MainGuard.offMain("rich_text")
        com.ferb.mobile.richText(text).map(::block)
    }

    /**
     * Words with nothing Markdown or a link could start (no mark, no list or quote start, no address): the library would
     * give one paragraph per run of lines, each one plain run, so they are made here without a call across (a call
     * costs more than reading them; the test holds the two equal over every fixture's words).
     */
    fun plainOnly(text: String): List<RichBlock>? {
        if (text.length > 4_000 || text.any { it in MARKS } || "://" in text || "www." in text) return null
        val paragraphs = ArrayList<RichBlock>()
        val run = ArrayList<String>()
        for (line in text.replace("\r\n", "\n").split('\n')) {
            val t = line.trim()
            if (line.isBlank()) { if (run.isNotEmpty()) { paragraphs += RichBlock.Paragraph(listOf(RichRun(run.joinToString("\n")))); run.clear() }; continue }
            if (t[0] == '-' || t[0] == '+' || t[0].isDigit() || line.firstOrNull()?.isWhitespace() == true) return null
            run += line
        }
        if (run.isNotEmpty()) paragraphs += RichBlock.Paragraph(listOf(RichRun(run.joinToString("\n"))))
        return paragraphs
    }

    private const val MARKS = "*_`[]<>#|~\\!@&"


    /** A reply as it streams: only its newest words are kept (one per key), so every piece is read once. */
    private val streaming = HashMap<String, Pair<String, List<RichBlock>>>()

    fun richNow(key: String, text: String): List<RichBlock> {
        synchronized(streaming) { streaming[key]?.let { if (it.first == text) return it.second } }
        MainGuard.offMain("rich_text")
        val blocks = com.ferb.mobile.richText(text).map(::block)
        synchronized(streaming) { if (streaming.size > 16) streaming.clear(); streaming[key] = text to blocks }
        return blocks
    }

    /** An answer as FERB.app draws it; [card]: its turn has a card of parts; [hidden]: boxes a later item draws. */
    fun answer(a: AnswerView, card: Boolean, hidden: List<String>, taskId: String?, zone: ZoneId = ZoneId.systemDefault()): AnswerUi =
        keep(listOf("a", a, card, hidden, taskId)) {
            MainGuard.offMain("answer_shown")
            val s = com.ferb.mobile.answerShown(a, card, hidden)
            AnswerUi(
                bubble = s.bubble,
                blocks = s.blocks.map(::block),
                copy = s.copy,
                note = s.note,
                lines = s.lines,
                details = s.details.map(::block),
                sites = s.sites,
                above = s.above.mapNotNull { card(it, zone, null) },
                below = s.below.mapNotNull { card(it, zone, null) },
                pages = s.pages.map(::page),
                files = s.files.map { f ->
                    CardLine(ChatModel.fileName(f.path), ChatModel.folderOf(f.path), file = true,
                        taskId = if (f.index != null) taskId else null, index = f.index?.toInt())
                },
                statusWords = s.statusWords,
            )
        }

    /** A notice's cards as FERB.app lays them out. */
    fun notice(text: String, quotes: Int, cards: List<ResultCard>, hidden: List<String>, zone: ZoneId = ZoneId.systemDefault()): NoticeUi =
        // no cards: nothing to lay out (the library's answer then is always this)
        if (cards.isEmpty()) NoticeUi(false, emptyList(), emptyList(), emptyList()) else keep(listOf("n", text, quotes, cards, hidden)) {
            MainGuard.offMain("notice_shown")
            val s = com.ferb.mobile.noticeShown(text, quotes.toUInt(), cards, hidden)
            NoticeUi(s.boxSays, s.above.mapNotNull { card(it, zone, s.boxWords) }, s.notes.mapNotNull { card(it, zone, null) }, s.pages.map(::page))
        }

    /** Receipt cards under the first reply (no layout of their own on the Mac but the same card views). */
    fun cards(cards: List<ResultCard>, hidden: List<String>, zone: ZoneId = ZoneId.systemDefault()): List<ResultShown> =
        cards.filter { box(it)?.let { b -> b !in hidden } ?: true }
            .filter { it.kindOf in setOf("reminder", "event", "note", "payment", "send", "schedules") }
            .mapNotNull { card(it, zone, null) }

    /** A card's box (FERB.app's ResultCard.box; the library's `shown::card_box`). */
    private fun box(c: ResultCard): String? = when (c.kindOf) {
        "event" -> c.id.takeIf { c.kind in setOf("timer", "reminder", "schedule") }
        "reminder" -> c.scheduleId ?: c.repeatOf ?: c.timerId
        else -> null
    }

    private fun page(c: ResultCard): String = c.line ?: when (c.page) {
        "memory" -> "Open the Memory page"
        else -> "Open Phone settings"
    }

    // ---- the blocks ------------------------------------------------------------------------------------------------

    private fun runs(s: List<com.ferb.mobile.RichSpan>): List<RichRun> = s.map { RichRun(it.text, it.bold, it.italic, it.code, it.strike, it.link) }

    private fun block(b: com.ferb.mobile.RichBlock): RichBlock = when (b) {
        is com.ferb.mobile.RichBlock.Paragraph -> RichBlock.Paragraph(runs(b.spans))
        is com.ferb.mobile.RichBlock.Heading -> RichBlock.Heading(b.level.toInt(), runs(b.spans))
        is com.ferb.mobile.RichBlock.ListItem -> RichBlock.Item(b.marker, b.level.toInt(), runs(b.spans))
        is com.ferb.mobile.RichBlock.Quote -> RichBlock.Quote(runs(b.spans))
        is com.ferb.mobile.RichBlock.Code -> RichBlock.Code(b.text, b.language)
        is com.ferb.mobile.RichBlock.Table -> RichBlock.Table(b.header.map { runs(it.spans) }, b.rows.map { r -> r.cells.map { runs(it.spans) } })
        com.ferb.mobile.RichBlock.Divider -> RichBlock.Divider
    }

    // ---- the cards: FERB.app's ResultCards.swift words -------------------------------------------------------------

    fun time(text: String?): Instant? = text?.let {
        runCatching { OffsetDateTime.parse(it).toInstant() }.getOrNull() ?: runCatching { Instant.parse(it) }.getOrNull()
    }

    /** CardTime.day: Today, Tomorrow, a weekday within the week, else a short date. */
    fun day(at: Instant, zone: ZoneId, today: LocalDate = LocalDate.now(zone), locale: Locale = Locale.getDefault()): String {
        val d = at.atZone(zone).toLocalDate()
        val days = java.time.temporal.ChronoUnit.DAYS.between(today, d)
        return when {
            days == 0L -> "Today"
            days == 1L -> "Tomorrow"
            days in 2..6 -> d.format(DateTimeFormatter.ofPattern("EEEE", locale))
            else -> d.format(DateTimeFormatter.ofPattern("EEE, MMM d", locale))
        }
    }

    fun clock(at: Instant, zone: ZoneId, locale: Locale = Locale.getDefault()): String =
        at.atZone(zone).format(DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT).withLocale(locale))

    private fun cap(s: String) = s.replaceFirstChar { it.titlecase() }

    /** A card's countdown (FERB.app's CountdownLine): to the schedule's end when it has one, else a reminder's next run. */
    private fun countdown(c: ResultCard, zone: ZoneId): CountdownShown? {
        if (c.kindOf != "reminder" && c.kindOf != "event") return null
        time(c.until)?.let { end ->
            val local = end.atZone(zone)
            val caption = if (local.hour == 0 && local.minute == 0 && local.second == 0) "until it ends, last day " + day(end.minusSeconds(1), zone)
            else "until it ends, " + day(end, zone) + " " + clock(end, zone)
            return CountdownShown(end.toEpochMilli(), caption, "ended")
        }
        if (c.kindOf == "reminder") time(c.nextRun)?.let { next ->
            return CountdownShown(next.toEpochMilli(), "until the next one, " + day(next, zone) + " " + clock(next, zone), "next run due")
        }
        return null
    }

    fun card(c: ResultCard, zone: ZoneId, words: String?): ResultShown? = when (c.kindOf) {
        "reminder" -> ResultShown.Reminder(
            timer = c.kind == "timer", title = c.title ?: "Scheduled reminder", words = words,
            due = time(c.due)?.let { "Due " + day(it, zone) + ", " + clock(it, zone) }, repeats = c.repeatRule != null,
            countdown = countdown(c, zone),
        )
        "event" -> {
            val timer = c.kind == "timer"
            val icon: ImageVector = if (timer) CardIcons.timer else if (c.kind == "reminder") CardIcons.reminder else CardIcons.calendar
            val meta = if (timer) "Timer set" else if (c.kind == "reminder") "Reminder set" else if (c.changed) "Schedule changed" else "Scheduled"
            val changes = if (!c.changed) emptyList() else buildList {
                c.wasRepeat?.let { add("Was" to it); add("Now" to (c.repeatRule ?: "")) }
                c.wasRequest?.let { add("Instructions were" to it); add("Instructions now" to (c.request ?: "")) }
            }
            val rule = c.repeatRule?.let { cap(it) + (c.watch?.let { w -> ", $w" } ?: "") } ?: c.watch?.let(::cap)
            val w = time(c.`when`)
            ResultShown.Event(meta, icon, c.what ?: "", w?.let { day(it, zone) }, w?.let { clock(it, zone) }, c.repeatRule != null, rule, changes, countdown(c, zone))
        }
        "note" -> ResultShown.Note(
            when (c.state) {
                "proposed" -> "Saved in my words · waits for your Keep"
                "corrected" -> "Remembered · corrected"
                "forgotten" -> "Forgotten"
                else -> "Remembered"
            },
            c.text ?: "", c.state == "forgotten",
        )
        "payment" -> ResultShown.Payment(c.merchant ?: c.site ?: "Payment", c.site?.takeIf { it != c.merchant }, c.savedCard, c.amount)
        "send" -> {
            val place = c.app ?: c.site ?: ""
            val verb = mapOf("call" to "Called", "mailbox_send" to "Emailed", "post" to "Posted", "reply" to "Replied")[c.kind] ?: "Sent"
            val icon = mapOf("call" to CardIcons.call, "mailbox_send" to CardIcons.mail, "post" to CardIcons.post, "reply" to CardIcons.reply)[c.kind] ?: CardIcons.send
            ResultShown.Send(initials(c.recipient ?: ""), c.recipient ?: place, verb + if (c.recipient == null) "" else " · $place", icon, c.account)
        }
        "schedules" -> {
            val rows = c.schedules.orEmpty()
            ResultShown.Schedules(
                when (rows.size) { 0 -> "No schedules"; 1 -> "1 schedule"; else -> "${rows.size} schedules" },
                rows.map { r ->
                    val next = time(r.nextRun)
                    ScheduleShown(r.name, cap(r.repeatRule) + (r.watch?.let { ", $it" } ?: ""), r.state.takeIf { it != "active" },
                        next?.let { day(it, zone) }, next?.let { clock(it, zone) })
                },
            )
        }
        // a kind this build doesn't draw: nothing (its line stays in FERB's words, as on the Mac)
        else -> null
    }

    /** FERB.app's ResultCard.initials: the first letters of the first two words of the name (an address's name part). */
    fun initials(name: String): String {
        val person = name.substringBefore('@')
        val letters = person.split(Regex("[^\\p{L}\\p{N}]+")).filter { it.isNotEmpty() }.take(2).joinToString("") { it.first().uppercase() }
        return letters.ifEmpty { "?" }
    }

}
