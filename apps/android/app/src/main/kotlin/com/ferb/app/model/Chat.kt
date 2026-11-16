package com.ferb.app.model

import com.ferb.design.components.MarkKind
import com.ferb.design.components.PlanPart
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

/** One row of the chat, top to bottom. [key] is stable across reads (an entry id and a part). */
sealed interface ChatRow {
    val key: String

    /** The owner's words (from this phone, the Mac or another device: drawn alike, as the Mac does, APP-916). */
    data class Owner(override val key: String, val text: String, val files: List<String>, val note: String?) : ChatRow

    /** FERB's words in a black bubble. */
    data class Ferb(override val key: String, val text: String) : ChatRow

    /** A short line on the sky: a notice, the Mac's note under a message, a live activity line. */
    data class Line(override val key: String, val text: String, val quotes: List<String> = emptyList()) : ChatRow

    /** A task's parts in one white card; [live] while a part runs or asks (Watch live and Stop show then). */
    data class Task(
        override val key: String,
        val taskId: String?,
        val headline: String,
        val done: Int,
        val total: Int,
        val parts: List<PlanPart>,
    ) : ChatRow {
        val live: Boolean get() = parts.any { it.kind == MarkKind.RUNNING || it.kind == MarkKind.ASKING }
    }

    /** Typed result cards and the files a task made. */
    data class Cards(override val key: String, val cards: List<CardLine>) : ChatRow

    /** FERB is writing. */
    data class Typing(override val key: String) : ChatRow

    /** An open question, drawn where the chat reaches it; tapping opens its sheet. */
    data class Asking(override val key: String, val approvalId: String, val title: String?) : ChatRow

    /** A message in the outbox (PHN-401): waiting, sending or failed, cancellable. */
    data class Outbox(override val key: String, val item: OutboxItem) : ChatRow

    /** The top of the chat: older messages can be fetched, or wait for a better path. */
    data class Older(override val key: String, val state: OlderState) : ChatRow
}

/**
 * One line of a result card or a file. A file a finished task made carries its [taskId] and [index] (its place among
 * the outcome's sorted files): a tap fetches it from the Mac by those (kb PHN-535).
 */
data class CardLine(val title: String, val detail: String?, val file: Boolean = false, val taskId: String? = null, val index: Int? = null)

object ChatModel {
    /** The rows of [view] with the outbox at the end. */
    fun rows(view: TranscriptView?): List<ChatRow> {
        if (view == null) return emptyList()
        val out = ArrayList<ChatRow>()
        when (val older = view.older) {
            is OlderState.Available, is OlderState.NeedsBetterPath -> out.add(ChatRow.Older("older", older))
            else -> Unit
        }
        val live = view.live.associateBy { it.entry }
        val turns = view.items.mapNotNull { (it as? Item.Turn)?.turn }.associateBy { it.entry }
        val open = view.questions.filter { it.open }
        val placed = HashSet<String>()
        for (item in view.items) {
            when (item) {
                is Item.Turn -> {
                    turn(item.turn, live[item.turn.entry], out)
                    for (q in open) {
                        val id = q.approvalId ?: continue
                        val mine = (q.requestId != null && q.requestId == item.turn.requestId) || (q.taskId != null && q.taskId == item.turn.taskId)
                        if (mine && placed.add(id)) out.add(ChatRow.Asking("q:$id", id, Questions.parse(q).title))
                    }
                }
                is Item.Reply -> {
                    val r = item.reply
                    val turn = turns[r.answers]
                    if (r.quotesRequest) turn?.words?.text?.let { out.add(ChatRow.Line("${r.entry}:quote", "“${oneLine(it)}”")) }
                    val said = r.said
                    when {
                        said != null -> answer(r.entry, said, out, turn?.taskId)
                        r.part == "question" -> turn?.question?.let { out.add(ChatRow.Ferb("${r.entry}:q", plain(it))) }
                        else -> turn?.answer?.let { answer(r.entry, it, out, turn?.taskId) }
                    }
                }
                is Item.Notice -> notice(item.notice, out)
                is Item.Reminder -> notice(item.notice, out)
            }
        }
        for (q in open) {
            val id = q.approvalId ?: continue
            if (placed.add(id)) out.add(ChatRow.Asking("q:$id", id, Questions.parse(q).title))
        }
        for (o in view.outbox) out.add(ChatRow.Outbox("o:${o.id}", o))
        return out
    }

    private fun notice(n: com.ferb.mobile.NoticeItem, out: MutableList<ChatRow>) {
        out.add(ChatRow.Line(n.entry, n.text, n.quotes.map { "“${it.text}” (${it.origin})" }))
        n.cards?.takeIf { it.isNotEmpty() }?.let { out.add(ChatRow.Cards("${n.entry}:cards", it.map(::cardLine))) }
    }

    private fun turn(t: TurnItem, live: LiveView?, out: MutableList<ChatRow>) {
        val e = t.entry
        val words = t.words
        if (t.from == "owner" && words != null) {
            out.add(ChatRow.Owner(e, words.text, words.files.map(::fileName), words.hiddenNotice))
        } else if (t.goal != null) {
            out.add(ChatRow.Line("$e:goal", t.goal!!))
        }
        t.notice?.let { out.add(ChatRow.Line("$e:notice", it)) }
        val first = t.firstReply
        val hasCard = t.card != null || t.checklist != null
        if (first != null && first.says) {
            val lines = first.direct + first.receipts + if (!hasCard) listOfNotNull(first.planLine ?: first.plan) + first.planLines else emptyList()
            val text = lines.filter { it.isNotBlank() }.joinToString("\n")
            if (text.isNotEmpty()) out.add(ChatRow.Ferb("$e:first", plain(text)))
            first.receiptCards?.takeIf { it.isNotEmpty() }?.let { out.add(ChatRow.Cards("$e:receipts", it.map(::cardLine))) }
        }
        t.card?.let { out.add(card(e, t, it, first?.planLine, live)) }
        t.checklist?.let { out.add(checklist(e, t, it, first?.planLine, live)) }
        if (t.parts.isNotEmpty()) out.add(ChatRow.Ferb("$e:parts", t.parts.joinToString("\n")))
        if (t.answer != null && t.question != null) out.add(ChatRow.Ferb("$e:asked", plain(t.question!!)))
        if (live != null) {
            live.streamingPlan?.takeIf { it.isNotBlank() && !hasCard }?.let { out.add(ChatRow.Ferb("$e:plan-live", plain(it))) }
            live.streamingReply?.takeIf { it.isNotBlank() }?.let { out.add(ChatRow.Ferb("$e:reply-live", plain(it))) }
            live.replyNotice?.let { out.add(ChatRow.Line("$e:reply-notice", it)) }
            if (!hasCard) live.line?.text?.let { out.add(ChatRow.Line("$e:line", it)) }
            live.activity?.let { out.add(ChatRow.Line("$e:activity", it)) }
            if (live.typing && live.streamingReply.isNullOrBlank()) out.add(ChatRow.Typing("$e:typing"))
        }
        if (!t.answeredBelow) t.answer?.let { answer(e, it, out, t.taskId) }
        else if (t.answer == null && t.question != null) out.add(ChatRow.Ferb("$e:asked", plain(t.question!!)))
    }

    private fun answer(key: String, a: AnswerView, out: MutableList<ChatRow>, taskId: String?) {
        val text = plain(a.displayText ?: a.text)
        val extra = a.lines.orEmpty().filter { it.isNotBlank() }
        val body = (listOf(text) + extra).filter { it.isNotBlank() }.joinToString("\n")
        if (body.isNotEmpty()) out.add(ChatRow.Ferb("$key:answer", body))
        val cards = a.cards.orEmpty().map(::cardLine) + a.artifacts.mapIndexed { i, p -> CardLine(fileName(p), folderOf(p), file = true, taskId = taskId, index = i) }
        if (cards.isNotEmpty()) out.add(ChatRow.Cards("$key:cards", cards))
        a.note?.takeIf { it.isNotBlank() }?.let { out.add(ChatRow.Line("$key:note", it)) }
    }

    private fun card(e: String, t: TurnItem, c: CardView, planLine: String?, live: LiveView?): ChatRow.Task {
        val liveRows = live?.rows.orEmpty().associateBy { it.id }
        val parts = c.rows.map { r ->
            val l = liveRows[r.row.part]
            val kind = l?.let { markOf(it.kind) } ?: markOf(r.shows.kind)
            PlanPart(kind, r.row.label, l?.words ?: r.shows.words ?: r.row.note)
        }
        return ChatRow.Task("$e:card", c.taskId.ifEmpty { t.taskId }, c.headline ?: c.line ?: planLine ?: t.taskName.orEmpty(),
            c.done.toInt(), c.total.toInt(), parts)
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
        return ChatRow.Task("$e:checklist", c.taskId.ifEmpty { t.taskId }, planLine ?: c.collapsed ?: t.taskName.orEmpty(),
            ended.size, c.steps.size, parts)
    }

    /** A row's, a line's or a turn's state as the phone's marks draw it (each shape its own, never colour alone). */
    fun markOf(kind: String): MarkKind = when (kind) {
        "done", "updated", "gaps", "done_with_gaps", "done_unverified" -> MarkKind.DONE
        "needsYou", "needs_you", "needs_user", "asks" -> MarkKind.ASKING
        "failed", "notFinished", "wontRun", "stopped", "cancelled", "interrupted" -> MarkKind.FAILED
        "waiting", "paused", "sleeping" -> MarkKind.WAITING
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
            .replace(Regex("\\[([^\\]]+)]\\(([^)]+)\\)"), "$1 ($2)")
    }.trim()
}
