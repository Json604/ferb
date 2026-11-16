package com.ferb.app.model

import com.ferb.design.components.MarkKind
import com.ferb.mobile.Item
import com.ferb.mobile.TranscriptView
import com.ferb.mobile.TurnItem

// The Tasks tab and the home screen's list (MobileScreens.swift PhoneTasksPage, HomePage), from the same transcript
// the chat draws: what waits for the owner, what runs, what finished (PHN-13: "Tasks is a tab: what runs, how far,
// Stop per task, what ran before"). The Mac's tasks.list would add tasks no message here started (FFI list).

/** One task as the Tasks tab and the home screen list it. [entry] opens it in the chat; [approvalId] opens its question. */
data class WorkItem(
    val kind: MarkKind,
    val title: String,
    val detail: String,
    val progress: Pair<Int, Int>? = null,
    val entry: String? = null,
    val approvalId: String? = null,
    val taskId: String? = null,
)

data class Work(val waiting: List<WorkItem>, val running: List<WorkItem>, val earlier: List<WorkItem>) {
    val isEmpty: Boolean get() = waiting.isEmpty() && running.isEmpty() && earlier.isEmpty()
}

object WorkModel {
    /** [asking] is the app's line for a question with no title of its own. */
    fun of(view: TranscriptView?, asking: String): Work {
        if (view == null) return Work(emptyList(), emptyList(), emptyList())
        val live = view.live.associateBy { it.entry }
        val turns = view.items.mapNotNull { (it as? Item.Turn)?.turn }
        val byTask = turns.filter { it.taskId != null }.associateBy { it.taskId }
        val waiting = Questions.open(view.questions).map { q ->
            val turn = q.taskId?.let(byTask::get)
            WorkItem(MarkKind.ASKING, turn?.let(::title) ?: (q.title ?: asking), q.title ?: oneLine(q.reason), approvalId = q.approvalId, entry = turn?.entry, taskId = q.taskId)
        }
        val running = ArrayList<WorkItem>()
        val earlier = ArrayList<WorkItem>()
        for (t in turns.asReversed()) {
            if (t.taskId == null && t.card == null && t.checklist == null) continue
            val l = live[t.entry]
            val prog = progress(t)
            if (l != null || t.status == "working" || t.status == "needs_user") {
                val detail = l?.line?.text ?: l?.activity ?: l?.rows?.firstOrNull { it.spinner }?.words ?: t.firstReply?.planLine ?: ""
                running.add(WorkItem(if (l?.state == "needs_you" || t.status == "needs_user") MarkKind.ASKING else MarkKind.RUNNING, title(t), detail, prog, t.entry, taskId = t.taskId))
            } else if (t.status != "merged") {
                val said = t.answer?.let { ChatModel.plain(it.displayText ?: it.text) }?.let(::oneLine).orEmpty()
                earlier.add(WorkItem(ChatModel.markOf(t.status), title(t), said, null, t.entry, taskId = t.taskId))
            }
        }
        return Work(waiting, running, earlier)
    }

    private fun progress(t: TurnItem): Pair<Int, Int>? =
        t.card?.let { it.done.toInt() to it.total.toInt() } ?: t.checklist?.let { it.ended.size to it.steps.size }

    fun title(t: TurnItem): String = oneLine(t.taskName ?: t.words?.text ?: t.goal ?: "")

    fun oneLine(s: String): String = s.lineSequence().firstOrNull { it.isNotBlank() }.orEmpty().trim().let { if (it.length > 90) it.take(89) + "…" else it }
}
