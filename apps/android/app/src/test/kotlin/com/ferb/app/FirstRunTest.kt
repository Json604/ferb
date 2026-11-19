package com.ferb.app

import com.ferb.app.model.FirstRun
import com.ferb.app.model.FirstRun.Ask
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The first run's asks on fakes (kb PHN-635..639): all allowed, some declined, already allowed (no page), a relaunch
 * (never again), an update that adds an ask (only it), and the pairing page reachable with everything declined.
 */
class FirstRunTest {
    private val allowed = HashSet<Ask>()
    private var asked = emptySet<String>()
    private fun run(sdk: Int = 36) = FirstRun(FirstRun.asksFor(sdk), { it in allowed }, { asked }, { asked = it })

    /** Continue asks each in order; the phone's question for each is answered (here: yes) and the page goes. */
    // decision: PHN-635, PHN-636
    @Test
    fun all_allowed() {
        val r = run()
        assertEquals(listOf(Ask.NOTIFICATIONS, Ask.LOCATION, Ask.CAMERA, Ask.MICROPHONE), r.pending())
        assertTrue(r.showsOver(unlockedPage = true))
        assertFalse("never before the unlock", r.showsOver(unlockedPage = false))
        while (true) {
            val a = r.pending().firstOrNull() ?: break
            allowed += a
            r.answered(a)
        }
        assertTrue(r.pending().isEmpty())
        assertFalse(r.showsOver(unlockedPage = true))
    }

    /** A declined ask is never asked again by the phone's question; the app goes on to pairing. */
    // decision: PHN-636, PHN-639
    @Test
    fun some_declined_and_everything_declined_still_pairs() {
        val r = run()
        r.answered(Ask.NOTIFICATIONS)            // declined
        allowed += Ask.LOCATION; r.answered(Ask.LOCATION)
        r.answered(Ask.CAMERA)                   // declined
        r.answered(Ask.MICROPHONE)               // declined
        assertTrue(r.pending().isEmpty())
        assertFalse("the pairing page shows: nothing holds it", run().showsOver(unlockedPage = true))
        // Not now on a fresh phone passes over all of them at once
        asked = emptySet(); allowed.clear()
        val fresh = run()
        fresh.notNow()
        assertFalse(fresh.showsOver(unlockedPage = true))
        assertEquals(Ask.entries.map { it.name }.toSet(), asked)
    }

    /** A phone that already allows everything never sees the page; one that allows some sees only the rest. */
    // decision: PHN-636
    @Test
    fun already_allowed_shows_no_page() {
        allowed += Ask.entries
        assertTrue(run().pending().isEmpty())
        allowed -= Ask.CAMERA
        assertEquals(listOf(Ask.CAMERA), run().pending())
    }

    /** A relaunch never asks again, even if a permission was taken back in Android's settings meanwhile. */
    // decision: PHN-636
    @Test
    fun a_relaunch_never_asks_again() {
        run().notNow()
        allowed.clear()
        assertTrue(run().pending().isEmpty())
    }

    /** After an update that adds an ask, the page shows only the new one. */
    // decision: PHN-636
    @Test
    fun an_update_adding_an_ask_shows_only_it() {
        asked = setOf(Ask.LOCATION.name, Ask.CAMERA.name, Ask.MICROPHONE.name) // answered on an Android 12 build
        assertTrue(run(sdk = 32).pending().isEmpty())
        assertEquals("now on Android 13: only notifications", listOf(Ask.NOTIFICATIONS), run(sdk = 33).pending())
    }

    /** Coarse location only: never the exact spot (GATE-613 asks for it per task) and never the background. */
    // decision: PHN-637
    @Test
    fun only_these_asks_exist() {
        assertEquals(listOf(Ask.LOCATION, Ask.CAMERA, Ask.MICROPHONE), FirstRun.asksFor(30))
    }

    /** A Yes to notifications on the first-run page turns Stay connected on; a No or Not now leaves it off. */
    // decision: PHN-771
    @Test
    fun a_yes_to_notifications_keeps_ferb_connected_and_not_now_leaves_it_off() {
        var stay = 0
        val yes = FirstRun(FirstRun.asksFor(36), { it in allowed }, { asked }, { asked = it }) { stay++ }
        yes.answered(Ask.NOTIFICATIONS, granted = true)
        assertEquals(1, stay)
        asked = emptySet()
        val no = FirstRun(FirstRun.asksFor(36), { it in allowed }, { asked }, { asked = it }) { stay++ }
        no.answered(Ask.NOTIFICATIONS, granted = false)
        no.answered(Ask.LOCATION, granted = true)
        asked = emptySet()
        FirstRun(FirstRun.asksFor(36), { it in allowed }, { asked }, { asked = it }) { stay++ }.notNow()
        assertEquals("only his Yes to notifications turns it on", 1, stay)
    }
}
