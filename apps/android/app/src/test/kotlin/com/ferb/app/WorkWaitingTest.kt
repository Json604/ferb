package com.ferb.app

import com.ferb.app.model.Work
import com.ferb.app.model.WorkItem
import com.ferb.app.model.WorkModel
import com.ferb.design.components.MarkKind
import com.ferb.mobile.TaskRow
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The Tasks tab: a task that waits for the owner is under "Waiting for you", never under Running, and listed once when
 * its open question is there already (owner 2026-11-23: two ChatGPT watches asking him were under Running).
 */
class WorkWaitingTest {
    // decision: PHN-802
    @Test
    fun a_task_waiting_for_the_owner_is_under_waiting_once() {
        val asked = WorkItem(MarkKind.ASKING, "Renew the domain", "Pay for another year?", approvalId = "ap-1", taskId = "t3")
        val rows = listOf(
            TaskRow("t1", "Bring in what ChatGPT knows", "Check the export request", "needs_user", null, null, "", null, null),
            TaskRow("t2", "Compare desks", "", "working", null, null, "", null, null),
            TaskRow("t3", "Renew the domain", "", "needs_user", null, null, "", null, null),
        )
        val w = WorkModel.fromMac(rows, listOf(asked), Work(emptyList(), emptyList(), emptyList()))
        assertEquals(listOf("Renew the domain", "Bring in what ChatGPT knows"), w.waiting.map { it.title })
        assertEquals(listOf("ap-1", null), w.waiting.map { it.approvalId })
        assertEquals(listOf("Compare desks"), w.running.map { it.title })
    }
}
