package com.ferb.app.model

import com.ferb.design.components.Fold
import com.ferb.design.components.MarkKind
import com.ferb.design.components.PlanPart
import com.ferb.design.components.ResultShown
import com.ferb.design.components.RichBlock
import com.ferb.design.components.plainBlocks
import com.ferb.mobile.AnswerView
import com.ferb.mobile.ChecklistView
import com.ferb.mobile.CardView
import com.ferb.mobile.Item
import com.ferb.mobile.LiveRow
import com.ferb.mobile.LiveView
import com.ferb.mobile.OlderState
import com.ferb.mobile.OutboxItem
import com.ferb.mobile.ResultCard
import com.ferb.mobile.TranscriptView
import com.ferb.mobile.TurnItem

// The chat as the phone draws it: the core's transcript (the Mac's own fold, transcripts.json's shape) turned into
// rows the :design pieces draw. Pure: every scenario of the engine's fixtures goes through it in the JVM tests.

/**
 * One row of the chat, top to bottom. [key] is stable across reads (an entry id and a part). Immutable values: a row
 * equal to the one drawn is not drawn again (kb PHN-714).
 */
@androidx.compose.runtime.Immutable
sealed interface ChatRow {
    val key: String

    /** The owner's words (from this phone, the Mac or another device: drawn alike, as the Mac does, APP-916). */
    data class Owner(override val key: String, val text: String, val files: List<String>, val note: String?) : ChatRow

    /** FERB's words in a black bubble. */
    data class Ferb(override val key: String, val text: String) : ChatRow

    /** A short line on the sky: a notice, the Mac's note under a message, a live activity line. */
    data class Line(override val key: String, val text: String, val quotes: List<String> = emptyList(), val since: Long? = null) : ChatRow

    /**
     * FERB's words as FERB.app draws them (kb PHN-721): Markdown in a black bubble, [tail] lines in the quieter colour
     * under them, folded past the chat's height when [fold] says so (APP-911: a long plan folds as an answer does).
     */
    data class Rich(
        override val key: String,
        val blocks: List<RichBlock>,
        val copy: String,
        val tail: List<String> = emptyList(),
        val fold: Fold = Fold.NONE,
    ) : ChatRow

    /** A turn's answer as FERB.app draws it (kb PHN-723, PHN-724): its cards, words, sites row, files and ending. */
    data class Answer(override val key: String, val ui: AnswerUi) : ChatRow

    /** FERB's own message (a notice) in its bubble, with its reminder boxes, quotes, facts and pages (NoticeView). */
    data class Notice(override val key: String, val text: String, val quotes: List<Pair<String, String>>, val ui: NoticeUi) : ChatRow

    /** Result cards by kind (a first reply's receipts). */
    data class Results(override val key: String, val cards: List<ResultShown>) : ChatRow

    /** A task's parts in one white card; [live] while a part runs or asks (Watch live and Stop show then). */
    data class Task(
        override val key: String,
        val taskId: String?,
        val headline: String,
        val done: Int,
        val total: Int,
        val parts: List<PlanPart>,
        /** FERB.app's running status line ("1 of 4 done · 2 running"), and once ended its footer and code's lines. */
        val status: String? = null,
        val footer: String? = null,
        val footerLines: List<String> = emptyList(),
        /** Each part's own live picture, by its step task id while its step works (FERB.app's RowPreview.watch). */
        val watch: List<String?> = emptyList(),
        /** The card's colour as a whole (FERB.app's surfaceTint). */
        val tint: com.ferb.design.components.CardTint = com.ferb.design.components.CardTint.NONE,
    ) : ChatRow {
        val live: Boolean get() = parts.any { it.kind == MarkKind.RUNNING || it.kind == MarkKind.ASKING }
    }

    /** Typed result cards and the files a task made. */
    data class Cards(override val key: String, val cards: List<CardLine>) : ChatRow

    /** FERB is writing. */
    data class Typing(override val key: String) : ChatRow

    /**
     * An open question, drawn where the chat reaches it; tapping opens its sheet. [title] and [reason] are the words
     * FERB.app's inline question leads with (its title, else "FERB needs your answer"; the question itself under it).
     */
    data class Asking(override val key: String, val approvalId: String, val title: String?, val reason: String? = null, val hidden: Boolean = false) : ChatRow

    /** A message in the outbox (PHN-401): waiting, sending or failed, cancellable. */
    data class Outbox(override val key: String, val item: OutboxItem) : ChatRow

    /** A message he sent with files, held on the phone until its files are on the Mac (kb PHN-691). */
    data class WithFiles(override val key: String, val item: Sending) : ChatRow

    /** At the top of today's chat: the way to earlier days, as the home page has it (kb PHN-698). */
    data class EarlierLink(override val key: String) : ChatRow

    /** The top of the chat: older messages can be fetched, or wait for a better path. */
    data class Older(override val key: String, val state: OlderState) : ChatRow

    /** A day's start in the one continuous chat (kb PHN-627): "Today", "Yesterday", or the date; [day] in epoch days. */
    data class Day(override val key: String, val day: Long) : ChatRow
}

/**
 * One line of a result card or a file. A file a finished task made carries its [taskId] and [index] (its place among
 * the outcome's sorted files): a tap fetches it from the Mac by those (kb PHN-535).
 */
data class CardLine(val title: String, val detail: String?, val file: Boolean = false, val taskId: String? = null, val index: Int? = null)

object ChatModel {
    /** 1970 to 2001 in seconds: the library's item times (`at`) count from Foundation's reference date. */
    private const val REFERENCE_UNIX = 978_307_200.0

    /** When [item]'s entry was made, as epoch milliseconds. */
    fun millis(item: Item): Long = ((when (item) {
        is Item.Turn -> item.turn.at
        is Item.Reply -> item.reply.at
        is Item.Notice -> item.notice.at
        is Item.Reminder -> item.notice.at
    } + REFERENCE_UNIX) * 1000).toLong()

    /** The calendar day (epoch days) of [millis] in [zone]. */
    fun day(millis: Long, zone: java.time.ZoneId): Long = java.time.Instant.ofEpochMilli(millis).atZone(zone).toLocalDate().toEpochDay()

    /**
     * What counts as "today" for the chat (kb PHN-697): the phone's local calendar day at [millis]. The one place this
     * rule lives (the owner will decide what midnight does mid-conversation); the view model holds its answer and moves
     * it only at a natural moment (the app opened or unlocked, the Chat tab chosen), never under him.
     */
    fun chatDay(millis: Long, zone: java.time.ZoneId): Long = day(millis, zone)

    /** The chat's two places (kb PHN-696): today's rows, and the earlier days'. */
    data class Places(val today: List<ChatRow>, val earlier: List<ChatRow>)

    /**
     * Splits [rows] (as [withDays] made them) at [day] (kb PHN-696, FERB.app's day separation): today has the rows of
     * that day and after, his outbox and messages with files, and the open questions of earlier days at its end (a
     * question is live: FERB.app shows them only in today's chat), with no day starts; earlier has every row before
     * it with their day starts and the "earlier messages" row at its top, and no questions. Rows with no day (no times
     * known) are today's.
     */
    fun split(rows: List<ChatRow>, day: Long): Places {
        val today = ArrayList<ChatRow>()
        val earlier = ArrayList<ChatRow>()
        val asked = ArrayList<ChatRow>()
        var current: Long? = null
        for (r in rows) {
            when (r) {
                is ChatRow.Day -> { current = r.day; if (r.day < day) earlier += r }
                is ChatRow.Older -> earlier += r
                is ChatRow.Outbox, is ChatRow.WithFiles -> today += r
                is ChatRow.Asking -> if (current != null && current < day) asked += r else today += r
                else -> if (current != null && current < day) earlier += r else today += r
            }
        }
        return Places(today + asked, earlier)
    }

    /**
     * Something was said today (kb PHN-626): an entry of today, or a message of his still in the outbox. False for an
     * empty chat: then the chat tab shows the home screen (a first-time user, or a new day with nothing said yet).
     */
    fun saidToday(view: TranscriptView?, today: Long, zone: java.time.ZoneId): Boolean =
        view != null && (view.outbox.isNotEmpty() || view.items.any { day(millis(it), zone) >= today })

    /**
     * [rows] with a day's start before the first row of each day (kb PHN-627): the one continuous chat, older days above
     * today's. A row's day is its entry's; the outbox is today; a question shares the day of what is above it.
     */
    fun withDays(rows: List<ChatRow>, days: Map<String, Long>, today: Long): List<ChatRow> {
        if (days.isEmpty() || rows.isEmpty()) return rows
        val out = ArrayList<ChatRow>(rows.size + 4)
        var current: Long? = null
        for (r in rows) {
            val d = when {
                r is ChatRow.Older -> null
                r is ChatRow.Outbox || r is ChatRow.WithFiles -> today
                r is ChatRow.Asking -> current
                else -> days[r.key.substringBefore(':')]
            }
            if (d != null && d != current) {
                out.add(ChatRow.Day("day:$d", d))
                current = d
            }
            out.add(r)
        }
        return out
    }

    /**
     * The day row [index] of [rows] (as [withDays] made them) belongs to: the day of the nearest day's start at or
     * above it, or below it for a row before the first (the "earlier messages" row). Null when the rows have no days.
     */
    fun dayAt(rows: List<ChatRow>, index: Int): Long? {
        if (index !in rows.indices) return null
        for (i in index downTo 0) (rows[i] as? ChatRow.Day)?.let { return it.day }
        for (i in index + 1 until rows.size) (rows[i] as? ChatRow.Day)?.let { return it.day }
        return null
    }

    /** Each entry's day (epoch days in [zone]), for [withDays]. */
    fun days(view: TranscriptView?, zone: java.time.ZoneId): Map<String, Long> =
        view?.items?.associate { it.entry() to day(millis(it), zone) }.orEmpty()

    private fun Item.entry(): String = when (this) {
        is Item.Turn -> turn.entry
        is Item.Reply -> reply.entry
        is Item.Notice -> notice.entry
        is Item.Reminder -> notice.entry
    }

    /**
     * Lays out every answer, notice and Markdown bubble of [view] through the phone library and keeps them (Present),
     * so [rows] on the main thread finds them all. Off the main thread: the library is called here.
     */
    fun prepare(view: TranscriptView) { rows(view) }

    /**
     * The rows of [view] with the outbox at the end. FERB's words come laid out by the phone library, kept per entry by
     * [Present]: after [prepare] for the same view this calls the library for nothing.
     */
    fun rows(view: TranscriptView?): List<ChatRow> {
        if (view == null) return emptyList()
        val out = ArrayList<ChatRow>()
        when (val older = view.older) {
            is OlderState.Available, is OlderState.NeedsBetterPath -> out.add(ChatRow.Older("older", older))
            else -> Unit
        }
        val live = view.live.associateBy { it.entry }
        val turnList = view.items.mapNotNull { (it as? Item.Turn)?.turn }
        val turns = turnList.associateBy { it.entry }
        val order = turnList.withIndex().associate { it.value.entry to it.index }
        val open = view.questions.filter { it.open }
        val placed = HashSet<String>()
        val questionBelow = view.items.mapNotNullTo(HashSet()) { (it as? Item.Reply)?.reply?.takeIf { r -> r.part == "question" }?.answers }
        fun ask(q: com.ferb.mobile.QuestionItem) {
            val id = q.approvalId ?: return
            if (!placed.add(id)) return
            val p = Questions.parse(q)
            // FERB.app's card (kb PHN-739): its one question, with its header ("Check before FERB goes on") under it
            val s = p.shown
            out.add(if (s != null) ChatRow.Asking("q:$id", id, s.question, s.header.ifEmpty { null }, p.hidden)
                else ChatRow.Asking("q:$id", id, p.title, p.reason.takeIf { it.isNotBlank() }, p.hidden))
        }
        for (item in view.items) {
            when (item) {
                is Item.Turn -> {
                    // ORCH-318: read together with another message, whose reply answers both (TurnCardView.swift MergedLabel)
                    val merged = item.turn.mergedInto?.let { lead ->
                        val at = turnList.indexOfFirst { it.requestId == lead }
                        if (at < 0) "Read together with your other message"
                        else "Read together with your message " + if (at < (order[item.turn.entry] ?: 0)) "above" else "below"
                    }
                    turn(item.turn, live[item.turn.entry], merged, item.turn.entry in questionBelow, out)
                    for (q in open) {
                        val mine = (q.requestId != null && q.requestId == item.turn.requestId) || (q.taskId != null && q.taskId == item.turn.taskId)
                        if (mine) ask(q)
                    }
                }
                is Item.Reply -> {
                    val r = item.reply
                    val turn = turns[r.answers]
                    if (r.quotesRequest) {
                        val quote = turn?.goal?.takeIf { it.isNotEmpty() } ?: turn?.words?.text?.takeIf { it.isNotBlank() }
                        out.add(ChatRow.Line("${r.entry}:quote", "↩ " + (quote?.let(::oneLine) ?: "A task from earlier")))
                    }
                    val said = r.said
                    when {
                        said != null -> answer(r.entry, said, false, r.hidden, out, turn?.taskId, null)
                        r.part == "question" -> turn?.question?.let { q ->
                            out.add(rich("${r.entry}:q", q))
                            // the final answer follows the question unless it went further below
                            val below = view.items.any { x -> x is Item.Reply && x.reply.answers == r.answers && x.reply.part == "answer" }
                            if (!below) turn.answer?.let { answer(r.entry, it, turn.card != null, turn.hidden, out, turn.taskId, turn.statusWords) }
                        }
                        else -> turn?.answer?.let { answer(r.entry, it, turn.card != null, turn.hidden, out, turn.taskId, turn.statusWords) }
                    }
                }
                is Item.Notice -> notice(item.notice, out)
                is Item.Reminder -> notice(item.notice, out)
            }
        }
        for (q in open) ask(q)
        for (o in view.outbox) out.add(ChatRow.Outbox("o:${o.id}", o))
        return out
    }

    /** FERB's Markdown in a bubble, laid out by the phone library (kept per text). */
    private fun rich(key: String, text: String, tail: List<String> = emptyList(), fold: Fold = Fold.NONE, keep: Boolean = true): ChatRow.Rich =
        ChatRow.Rich(key, if (keep) Present.rich(text) else Present.richNow(key, text), text, tail, fold)

    private fun notice(n: com.ferb.mobile.NoticeItem, out: MutableList<ChatRow>) {
        val ui = Present.notice(n.text, n.quotes.size, n.cards.orEmpty(), n.hidden)
        out.add(ChatRow.Notice(n.entry, n.text, n.quotes.map { it.text to it.origin }, ui))
    }

    private fun turn(t: TurnItem, live: LiveView?, merged: String?, questionBelow: Boolean, out: MutableList<ChatRow>) {
        val e = t.entry
        val words = t.words
        if (t.from == "owner" && words != null) {
            out.add(ChatRow.Owner(e, words.text, words.files.map(::fileName), words.hiddenNotice))
        } else if (t.goal != null) {
            out.add(ChatRow.Line("$e:goal", t.goal!!.ifEmpty { "A task from earlier" }))
        }
        merged?.let { out.add(ChatRow.Line("$e:merged", it)) }
        val first = t.firstReply
        if (first != null && first.says) {
            // FERB.app's FirstReplyView: the asks answered at once (Markdown) only without a card, the receipts no card
            // stands for, the plan line, and code's lines under it in the quieter colour; a long one folds (APP-911)
            val line = first.planLine ?: first.plan
            val said = (if (t.card == null) first.direct else emptyList()) + first.receiptsShown + listOfNotNull(line)
            val text = said.filter { it.isNotBlank() }.joinToString("\n\n")
            val tail = if (first.planLine != null) first.planLines.filter { it.isNotBlank() } else emptyList()
            if (text.isNotEmpty() || tail.isNotEmpty()) out.add(rich("$e:first", text, tail, Fold.ANSWER))
        } else if (first == null) {
            // the plan as it streams (plain: FERB.app's PlanLineBlock)
            live?.streamingPlan?.takeIf { it.isNotBlank() }?.let { out.add(ChatRow.Rich("$e:first", plainBlocks(it), it)) }
        }
        first?.receiptCards?.takeIf { it.isNotEmpty() }?.let { cards ->
            Present.cards(cards, t.hidden).takeIf { it.isNotEmpty() }?.let { out.add(ChatRow.Results("$e:receipts", it)) }
        }
        if (t.card == null) t.checklist?.let { out.add(checklist(e, t, it, first?.planLine, live)) }
        t.card?.let { c ->
            out.add(card(e, t, c, first?.planLine, live))
            // step 8e: the task's cards (payments, sends, links, files) under its card's footer
            t.answer?.let { a -> answer("$e:card", a, true, t.hidden, out, t.taskId, null) }
        }
        t.parts.forEachIndexed { i, p -> if (p.isNotBlank()) out.add(rich("$e:parts:$i", p)) }
        // the connection line with its time (FERB.app's LiveLineView, "… · 0:42")
        live?.line?.let { l -> out.add(ChatRow.Line("$e:line", l.text, since = Present.time(l.startedAt)?.toEpochMilli())) }
        if (live != null && live.typing && live.streamingReply.isNullOrBlank()) out.add(ChatRow.Typing("$e:typing"))
        // a question FERB asked stays here unless it was itself shown below
        t.question?.let { q -> if (!questionBelow) out.add(rich("$e:asked", q)) }
        if (t.answeredBelow) {
            out.add(ChatRow.Line("$e:below", "Answered below"))
        } else if (t.card == null) {
            val streaming = live?.streamingReply?.takeIf { it.isNotBlank() }
            when {
                // a reply as it streams: its words so far, read again for each piece (never kept: only the last one is)
                streaming != null && t.answer == null -> {
                    out.add(rich("$e:answer", streaming, keep = false))
                    // FERB.app's `isProvisional`: a reply that isn't a task's, still coming
                    if (t.taskId == null) out.add(ChatRow.Line("$e:draft", "Draft · still responding"))
                }
                else -> t.answer?.let { answer(e, it, false, t.hidden, out, t.taskId, null) }
            }
        }
        live?.replyNotice?.let { out.add(ChatRow.Line("$e:reply-notice", it)) }
        t.notice?.let { out.add(ChatRow.Line("$e:notice", it)) }
        if (t.notice == null) t.statusWords?.let { out.add(ChatRow.Line("$e:status", it)) }
    }

    private fun answer(key: String, a: AnswerView, card: Boolean, hidden: List<String>, out: MutableList<ChatRow>, taskId: String?, status: String?) {
        val ui = Present.answer(a, card, hidden, taskId)
        if (ui.bubble || ui.above.isNotEmpty() || ui.below.isNotEmpty() || ui.sites.isNotEmpty() || ui.files.isNotEmpty() || ui.pages.isNotEmpty()) {
            out.add(ChatRow.Answer("$key:answer", ui))
        }
        (status ?: ui.statusWords)?.let { out.add(ChatRow.Line("$key:status", it)) }
    }

    private fun card(e: String, t: TurnItem, c: CardView, planLine: String?, live: LiveView?): ChatRow.Task {
        val liveRows = live?.rows.orEmpty().associateBy { it.id }
        // APP-895: under a row still waiting, the steps it waits on that no row answers, each its own line
        val feeders = live?.feeders.orEmpty().groupBy { it.part }
        val parts = c.rows.map { r ->
            val l = liveRows[r.row.part]
            part(r.row, l?.kind ?: r.shows.kind, l?.words ?: r.shows.words, "$e:${r.row.part}").copy(
                sub = feeders[r.row.part].orEmpty().map { f -> PlanPart(markOf(f.line.kind), f.label, f.line.words) })
        }
        // FERB.app's surfaceTint: amber while it asks, red when a part failed, blue when one was updated or paused
        val states = c.rows.map { it.row.state }.toSet()
        val tint = when {
            !live?.openQuestions.isNullOrEmpty() || "asks" in states -> com.ferb.design.components.CardTint.ASKING
            "failed" in states -> com.ferb.design.components.CardTint.FAILED
            "paused" in states || "updated" in states -> com.ferb.design.components.CardTint.UPDATED
            else -> com.ferb.design.components.CardTint.NONE
        }
        // FERB.app's RowPreview.watch: a row whose own step works now (starting, running, retrying, checking) and that
        // isn't answered at once shows that step's picture
        val watch = c.rows.map { r ->
            val kind = liveRows[r.row.part]?.kind
            r.row.stepTaskId?.takeIf { it.isNotEmpty() && !r.row.direct && kind in setOf("starting", "running", "retrying", "checking", "working") }
        }
        return ChatRow.Task("$e:card", c.taskId.ifEmpty { t.taskId }, c.headline ?: planLine ?: t.taskName.orEmpty(),
            c.done.toInt(), c.total.toInt(), parts,
            status = live?.cardLine, footer = c.line, footerLines = c.linesShown, watch = watch, tint = tint)
    }

    /**
     * One part of a card as FERB.app's CardRowView shows it (kb PHN-733): by what the row shows now ([display], the
     * live RowDisplay or the stored Shows), its answer in rich text (done, updated, delivered with gaps, needs you), a
     * failure's or stop's words, else its state words; then "Couldn't confirm", "Updated: …" and "You chose …".
     */
    fun part(row: com.ferb.mobile.CardRow, display: String, words: String?, key: String): PlanPart {
        val text = row.text?.takeIf { it.isNotBlank() }
        var detail: String? = null
        var rich: List<com.ferb.design.components.RichBlock>? = null
        var tone = com.ferb.design.components.PartTone.DIM
        when (display) {
            "preview" -> detail = "Preview: I'd do this step"
            "previewRunning" -> detail = "Preview: as if working…"
            "failed" -> { detail = text ?: "Couldn't do it."; tone = com.ferb.design.components.PartTone.RED }
            "wontRun" -> { detail = words; tone = com.ferb.design.components.PartTone.RED }
            "stopped" -> detail = text
            "done", "updated" -> rich = text?.let(Present::rich)
            "gaps" -> { detail = words; tone = com.ferb.design.components.PartTone.AMBER; rich = text?.let(Present::rich) }
            "needsYou", "paused" -> if (text != null) rich = Present.rich(text) else if (row.question == null) { detail = words; tone = com.ferb.design.components.PartTone.AMBER }
            else -> detail = words
        }
        val notes = buildList {
            row.note?.let { add(com.ferb.design.components.PartNote(it, "Couldn't confirm: ", com.ferb.design.components.PartTone.AMBER)) }
            row.why?.takeIf { display == "updated" }?.let { add(com.ferb.design.components.PartNote(if (it.startsWith("Updated")) it else "Updated: $it", tone = com.ferb.design.components.PartTone.BLUE)) }
            row.chosen?.let { add(com.ferb.design.components.PartNote("You chose $it")) }
            row.question?.takeIf { display == "needsYou" || display == "paused" }?.let { add(com.ferb.design.components.PartNote(it.text)) }
        }
        return PlanPart(markOf(display), row.label, detail, rich, tone, notes, display == "updated", key)
    }

    private fun checklist(e: String, t: TurnItem, c: ChecklistView, planLine: String?, live: LiveView?): ChatRow.Task {
        val liveRows: Map<String, LiveRow> = live?.checklist.orEmpty().associateBy { it.id }
        val lines = c.lines.associateBy { it.step }
        val ended = c.ended
        val parts = c.steps.map { s ->
            val l = liveRows[s.id]
            val end = ended[s.id]
            val kind = when {
                l != null -> markOf(l.kind)
                end != null -> markOf(end)
                live != null -> MarkKind.WAITING
                t.status == "working" || t.status == "needs_user" -> MarkKind.WAITING
                else -> markOf(t.status)
            }
            PlanPart(kind, s.label, l?.words ?: lines[s.id]?.words)
        }
        // a checklist line whose step runs shows its picture (FERB.app's ChecklistLineView)
        val watch = c.steps.map { s -> s.stepTaskId.takeIf { it.isNotEmpty() && liveRows[s.id]?.kind == "running" } }
        // FERB.app's checklist folds once finished to "N steps done" (StepChecklistView)
        return ChatRow.Task("$e:checklist", c.taskId.ifEmpty { t.taskId }, c.collapsed ?: planLine ?: t.taskName.orEmpty(),
            ended.size, c.steps.size, parts, watch = watch)
    }

    /** A row's words without their look (tests, and what reading the chat aloud would say). */
    fun words(row: ChatRow): String = when (row) {
        is ChatRow.Owner -> row.text
        is ChatRow.Ferb -> row.text
        is ChatRow.Line -> row.text
        is ChatRow.Rich -> row.copy + row.tail.joinToString("") { "\n" + it }
        is ChatRow.Answer -> (listOf(row.ui.copy) + listOfNotNull(row.ui.note) + row.ui.lines).filter { it.isNotEmpty() }.joinToString("\n")
        is ChatRow.Notice -> row.text
        is ChatRow.Task -> row.headline + "\n" + row.parts.joinToString("\n") { it.title }
        else -> ""
    }

    /** A row's, a line's or a turn's state as the phone's marks draw it (each shape its own, never colour alone). */
    fun markOf(kind: String): MarkKind = when (kind) {
        "done", "updated", "gaps", "done_with_gaps", "done_unverified" -> MarkKind.DONE
        "needsYou", "needs_you", "needs_user", "asks" -> MarkKind.ASKING
        "failed", "notFinished", "wontRun", "stopped", "cancelled", "interrupted" -> MarkKind.FAILED
        "waiting", "paused", "sleeping", "upcoming" -> MarkKind.WAITING
        else -> MarkKind.RUNNING // working, running, starting, retrying, checking, previewRunning
    }

    fun cardLine(c: ResultCard): CardLine {
        val title = c.title ?: c.what ?: c.text ?: c.name ?: c.line ?: c.kindOf
        val detail = listOfNotNull(c.due ?: c.`when` ?: c.nextRun, c.repeatRule, c.path?.let(::folderOf)).firstOrNull()
            ?: c.line?.takeIf { it != title }
        return CardLine(title, detail, file = c.kindOf == "file")
    }

    fun fileName(path: String): String = path.trimEnd('/').substringAfterLast('/')

    fun folderOf(path: String): String? = path.trimEnd('/').substringBeforeLast('/', "").substringAfterLast('/').ifEmpty { null }

    private fun oneLine(s: String) = s.lineSequence().firstOrNull().orEmpty().let { if (it.length > 80) it.take(79) + "…" else it }

    /**
     * FERB's answers are Markdown; the phone's bubble draws plain text (the lab's bubble has no rich text), so the
     * marks that only style are taken out: bold and italic stars, code ticks, heading hashes, a link's brackets.
     */
    fun plain(md: String): String = md.lineSequence().joinToString("\n") { line ->
        line.replace(Regex("^#{1,6}\\s+"), "")
            .replace(Regex("^\\s*[-*]\\s+"), "• ")
            .replace("**", "").replace("__", "").replace("`", "")
            .replace(Regex("\\[([^\\]]+)]\\(([^)]+)\\)"), "$1")
    }.trim()
}
