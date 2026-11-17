package com.ferb.app.model

/**
 * The app's own lock (kb PHN-8, PHN-15): locked on a cold start; back from the background after more than
 * [LOCK_AFTER_MS] it is locked again. When that time passes while still in the background it locks then too (the
 * moment the Mac should also be told and fetched files go, PHN-15/PHN-11), through [onLockedInBackground]. Time is
 * injected (a monotonic clock), so the rule is tested without waiting. Pure: no Android here.
 *
 * The prompt by itself (kb PHN-615, PHN-616): each time the app arrives in front locked (a new process, or back from
 * the background, relocked or still locked) one prompt is due, taken once by [takePrompt]; leaving clears it. The
 * owner's own lock from inside ([lock]) makes none: the page waits for his tap.
 */
class AppLock(private val now: () -> Long) {
    var locked: Boolean = true
        private set

    /** When the app last went to the background (monotonic ms), or null while it is in front. */
    private var leftAt: Long? = null

    /** Not in front: a new process, or since [wentToBackground]. */
    private var away = true

    /** The unlock prompt should appear by itself (PHN-615): the app arrived in front locked and none was shown yet. */
    var promptDue: Boolean = false
        private set

    /** The owner unlocked (the prompt succeeded). */
    fun unlocked() {
        locked = false
        promptDue = false
    }

    /** The app went to the background. Returns when (monotonic ms) the background lock is due. */
    fun wentToBackground(): Long {
        val t = now()
        leftAt = t
        away = true
        promptDue = false
        return t + LOCK_AFTER_MS
    }

    /**
     * The app is in front again: locked if it was away longer than the decided time. Arriving locked makes one prompt
     * due; being told again while already in front (a system sheet closed) changes nothing.
     */
    fun cameToFront() {
        val left = leftAt
        leftAt = null
        if (left != null && now() - left > LOCK_AFTER_MS) locked = true
        if (away && locked) promptDue = true
        away = false
    }

    /** Takes the due prompt: true once per arrival, and only while still locked. */
    fun takePrompt(): Boolean {
        if (!promptDue || !locked) return false
        promptDue = false
        return true
    }

    /** A taken prompt could not be shown (the app lost the screen first): due again while in front and locked. */
    fun promptAgain() {
        if (!away && locked) promptDue = true
    }

    /** The background timer fired: locks if the app is still away and the time has passed. True when it locked now. */
    fun backgroundTimer(): Boolean {
        val left = leftAt ?: return false
        if (locked || now() - left < LOCK_AFTER_MS) return false
        locked = true
        return true
    }

    /** The owner asked to lock (or the phone's screen lock went away). */
    fun lock() {
        locked = true
    }

    companion object {
        /** PHN-15: five minutes after the app was left. */
        const val LOCK_AFTER_MS = 5 * 60_000L
    }
}
