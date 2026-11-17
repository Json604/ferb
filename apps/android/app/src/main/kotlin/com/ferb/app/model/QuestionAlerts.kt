package com.ferb.app.model

/**
 * The one rule for FERB's questions in the phone's notifications (the owner, kb PHN-760: "i shoudl get it like nothc
 * comes down for all questions from worker or palnner no matter what. no notifications only when im in the app"):
 *
 * - every question that waits (the Mac's status: a worker's question or check, a hidden input, the planner's question)
 *   is posted once, at once, unless he is in the app;
 * - in the app, unlocked, he sees it there: nothing is posted, and it is remembered as seen; if he leaves with it still
 *   waiting, it goes to the shade quietly (no sound, no heads-up), so it can still be answered from there (PHN-764);
 * - in the app but on its lock page he has not seen it: nothing yet, and a heads-up when he leaves without unlocking;
 * - a question already posted is updated quietly when its words or needs change, and never posted again; a question
 *   FERB asks again is a new question (a new id) and is posted again;
 * - what no longer waits leaves the shade (answered on the phone, the Mac or another phone; withdrawn; expired).
 *
 * Pure: the caller reads the questions and the app's state, posts what [decide] says and keeps [announced] (persisted:
 * a question announced before a restart is not announced again) and the seen set (this process only).
 */
class QuestionAlerts {
    /** What to do with the shade now. */
    data class Plan(
        /** New in the shade: with sound and heads-up unless [Post.quiet]. */
        val post: List<Post>,
        /** Already in the shade, its words or needs changed: updated without sound. */
        val update: List<Question>,
        /** No longer waiting: removed. */
        val cancel: List<String>,
        /** The announced ids to keep (`q:<id>`). */
        val announced: Set<String>,
    )

    data class Post(val question: Question, val quiet: Boolean)

    private val seen = HashSet<String>()

    /**
     * [waiting]: every question that waits now, null when nothing says yet (no status and no transcript): then nothing
     * is posted or removed. [front]: the app is on screen; [unlocked]: past its lock page. [posted]: what is in the shade.
     */
    fun decide(waiting: List<Question>?, front: Boolean, unlocked: Boolean, posted: Map<String, Question>, announced: Set<String>): Plan {
        if (waiting == null) return Plan(emptyList(), emptyList(), emptyList(), announced)
        val ids = waiting.map { it.approvalId }.toSet()
        seen.retainAll(ids)
        val post = ArrayList<Post>()
        val update = ArrayList<Question>()
        val known = announced.filter { !it.startsWith(Q) || it.removePrefix(Q) in ids }.toMutableSet()
        for (q in waiting) {
            val key = Q + q.approvalId
            if (key in known) {
                val shown = posted[q.approvalId]
                if (shown != null && shown != q) update.add(q)
                continue
            }
            if (front) {
                // in the app: shown there, not in the shade; seen only past the lock page
                if (unlocked) seen.add(q.approvalId)
                continue
            }
            known.add(key)
            post.add(Post(q, quiet = q.approvalId in seen))
        }
        val cancel = posted.keys.filter { it !in ids }
        return Plan(post, update, cancel, known)
    }

    companion object {
        const val Q = "q:"
    }
}
