package com.ferb.app.model

/**
 * The app's own lock (kb PHN-8, PHN-15): locked on a cold start; back from the background after more than
 * [LOCK_AFTER_MS] it is locked again. When that time passes while still in the background it locks then too (the
 * moment the Mac should also be told and fetched files go, PHN-15/PHN-11), through [onLockedInBackground]. Time is
 * injected (a monotonic clock), so the rule is tested without waiting. Pure: no Android here.
 */
class AppLock(private val now: () -> Long) {
    var locked: Boolean = true
        private set

    /** When the app last went to the background (monotonic ms), or null while it is in front. */
    private var leftAt: Long? = null

    /** The owner unlocked (the prompt succeeded). */
    fun unlocked() {
        locked = false
    }

    /** The app went to the background. Returns when (monotonic ms) the background lock is due. */
    fun wentToBackground(): Long {
        val t = now()
        leftAt = t
        return t + LOCK_AFTER_MS
    }

    /** The app is in front again: locked if it was away longer than the decided time. */
    fun cameToFront() {
        val left = leftAt
        leftAt = null
        if (left != null && now() - left > LOCK_AFTER_MS) locked = true
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
