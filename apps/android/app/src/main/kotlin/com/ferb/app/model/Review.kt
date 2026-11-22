package com.ferb.app.model

import com.ferb.mobile.ReviewPart

/**
 * FERB's review of what an import brought in (kb MEM-460..457; ferb-mobile `review`): its question's card is the
 * review's list ([com.ferb.mobile.QuestionShown.review]), read from the Mac and answered item by item or all at once.
 * Nothing of it is stored on the phone.
 */
object Review {
    /** A group shows this many items until opened (FERB.app's `MemoryReview.shownAtFirst`). */
    const val SHOWN_AT_FIRST = 3

    /** The read of the review on its way (`ui.busy`). */
    const val LOAD = "review"

    /** Every item's id of [part], for an answer to all of them at once. */
    fun ids(part: ReviewPart): List<String> = part.groups.flatMap { g -> g.items.map { it.id } }

    /** An answer on its way (`ui.busy`): one item's ([id]), or all of a part's (null). */
    fun key(part: String, id: String?): String = "review:$part:${id ?: "*"}"

    /** A group's answer to all of its items on its way (`ui.busy`). */
    fun groupKey(part: String, group: String): String = "review:$part:group:$group"
}
