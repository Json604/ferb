package com.ferb.app.model

import com.ferb.app.R
import com.ferb.mobile.Insomnia
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.LinkStatus
import com.ferb.mobile.PhoneStatus

/**
 * Insomnia mode's words that depend on the Mac's state (APP-981): the warning's closed-lid sentence, and what the
 * phone says when the Mac can't be reached while the mode was on (it may be asleep: closed on battery without FERB's
 * helper). One place for the rule, so the notification, the chat's line and the waiting messages agree.
 */
object MacAsleep {
    /**
     * The Mac may be asleep: the link has been lost for the can't-reach time ([lost]; never just connecting), the last
     * status said insomnia mode was on, and the link hasn't stopped for good (removed, pair again: those say their own).
     */
    fun shown(status: PhoneStatus?, link: LinkStatus?, lost: Boolean): Boolean =
        lost && status?.insomnia?.on == true && link?.phase != LinkPhase.ONLINE && link?.phase != LinkPhase.STOPPED

    /**
     * The Mac's own words while it is asleep and only briefly awake (one of macOS's Power Nap wakes; kb SENSE-430):
     * replies are slow and screen work waits. From the last status, mode on or off, until the link stops for good.
     */
    fun line(status: PhoneStatus?, link: LinkStatus?): String? =
        status?.insomnia?.takeIf { it.asleep && link?.phase != LinkPhase.STOPPED }?.asleepLine?.takeIf { it.isNotBlank() }

    /** A message waiting for the Mac waits for it to wake: the may-be-asleep rule above, or the Mac said it is asleep. */
    fun waiting(status: PhoneStatus?, link: LinkStatus?, lost: Boolean): Boolean = shown(status, link, lost) || line(status, link) != null

    /** The warning's words: the lid-open advice, then the Mac's own closed-lid sentence ([fallback] from an older Mac). */
    fun body(open: String, fallback: String, i: Insomnia?): String = "$open ${i?.lidLine?.takeIf { it.isNotBlank() } ?: fallback}"

    /** Why FERB's lid helper isn't on, in the Mac's plain words (none when nothing is wrong, or an older Mac). */
    fun helperProblem(i: Insomnia?): String? = i?.helper?.problem?.takeIf { it.isNotBlank() }

    /** What a message waiting for the Mac says: the phone's unlock, the Mac asleep, or today's "Waiting for your Mac". */
    fun waitingLine(waitingFor: String?, asleep: Boolean): Int = when {
        waitingFor == "unlock" -> R.string.outbox_waiting_unlock
        asleep -> R.string.outbox_mac_asleep
        else -> R.string.outbox_waiting
    }
}
