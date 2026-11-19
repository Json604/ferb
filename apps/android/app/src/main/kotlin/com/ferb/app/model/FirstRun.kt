package com.ferb.app.model

/**
 * The first run's asks (owner kb PHN-635; PHN-636..639): once, after the first unlock, FERB asks the phone for what it
 * needs, from one page and one Continue: notifications, the city (coarse location, never fine, never in the
 * background), the camera (the pairing code moments later) and the microphone (hold to talk). An ask the phone already
 * allows, or one already answered or passed over, is never on the page; an update that adds an ask shows only it.
 * Pure: what the phone allows and what was asked come in, so the same rule runs on a phone and in the tests.
 */
class FirstRun(
    /** The asks this phone has (notifications only where Android asks for them: 13 and later). */
    private val wanted: List<Ask>,
    private val allowed: (Ask) -> Boolean,
    private val asked: () -> Set<String>,
    private val record: (Set<String>) -> Unit,
    /** A Yes to notifications on this page turns Stay connected on (owner kb PHN-771); nothing else does. */
    private val stayConnected: () -> Unit = {},
) {
    enum class Ask { NOTIFICATIONS, LOCATION, CAMERA, MICROPHONE }

    /** What the page asks for now, in order; empty: no page. */
    fun pending(): List<Ask> = asked().let { done -> wanted.filter { it.name !in done && !allowed(it) } }

    /** The page shows over the pairing page or the app itself, never before the unlock (PHN-8). */
    fun showsOver(unlockedPage: Boolean): Boolean = unlockedPage && pending().isNotEmpty()

    /** The phone's question for [ask] was answered (either way): it is never asked again. */
    fun answered(ask: Ask, granted: Boolean = false) {
        record(asked() + ask.name)
        if (ask == Ask.NOTIFICATIONS && granted) stayConnected()
    }

    /** Not now: the page's asks are passed over; each feature still explains itself when it is used. */
    fun notNow() = record(asked() + pending().map { it.name })

    companion object {
        /** This phone's asks: Android asks for notifications from 13 (API 33) on; before that they are on. */
        fun asksFor(sdk: Int): List<Ask> =
            listOfNotNull(Ask.NOTIFICATIONS.takeIf { sdk >= 33 }, Ask.LOCATION, Ask.CAMERA, Ask.MICROPHONE)
    }
}
