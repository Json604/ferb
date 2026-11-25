package com.ferb.app.model

import com.ferb.mobile.MemoryReview
import com.ferb.mobile.ReviewPart

/**
 * FERB's review of what it learned (kb MEM-460..457, MEM-485, MEM-486; ferb-mobile `review`): one list, each line
 * ticked until the owner unticks it, answered all at once; in its question's card ([com.ferb.mobile.QuestionShown.review],
 * five or fewer at the day's first open) or in the sheet the day's note opens ([com.ferb.app.Sheet.Review]). Nothing of
 * it is stored on the phone.
 */
object Review {
    /** Approve all or Decline all on its way (`ui.busy`). */
    const val ALL = "review:all"

    /** The day's note's page (MEM-485): its button opens the review's sheet. */
    const val PAGE = "review"

    /** Every line's id, in the order shown. */
    fun ids(r: MemoryReview): List<String> = r.parts.flatMap { ids(it) }

    /** The main button's words with the lines [unticked]: Approve all, "Approve 2", null when none is ticked (the Mac's words). */
    fun approveLabel(r: MemoryReview, unticked: Set<String>): String? =
        com.ferb.mobile.reviewApproveLabel(r, ids(r).count { it !in unticked }.toUInt())

    /** The read of the review on its way (`ui.busy`). */
    const val LOAD = "review"

    /** Every item's id of [part], for an answer to all of them at once. */
    fun ids(part: ReviewPart): List<String> = part.groups.flatMap { g -> g.items.map { it.id } }

    /** An answer on its way (`ui.busy`): one item's ([id]), or all of a part's (null). */
    fun key(part: String, id: String?): String = "review:$part:${id ?: "*"}"

    /** A group's answer to all of its items on its way (`ui.busy`). */
    fun groupKey(part: String, group: String): String = "review:$part:group:$group"
}
