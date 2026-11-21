package com.ferb.app

import android.app.Activity
import android.app.Application
import android.view.WindowManager
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** Which screens screenshots may show: the window's FLAG_SECURE as MainActivity sets it through ScreenGuard. */
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class ScreenGuardTest {
    private fun secureAfter(block: Boolean, mustSecure: Boolean): Boolean {
        val window = Robolectric.buildActivity(Activity::class.java).setup().get().window
        ScreenGuard.apply(window, true) // as MainActivity's first frames
        ScreenGuard.apply(window, ScreenGuard.secure(block, mustSecure))
        return window.attributes.flags and WindowManager.LayoutParams.FLAG_SECURE != 0
    }

    // decision: PHN-793, PHN-794, PHN-420, PHN-110, PHN-454
    @Test
    fun the_owner_s_switch_decides_the_lock_page_too_and_a_private_screen_is_always_kept_out() {
        // the lock page has no private screen open: it follows the switch (owner 2026-11-22)
        assertEquals("the switch off: screenshots show the app, the lock page included", false, secureAfter(block = false, mustSecure = false))
        assertEquals("the switch on: kept out", true, secureAfter(block = true, mustSecure = false))
        // a hidden answer's box, Watch live or the Mac's camera: kept out whatever the switch says
        assertEquals(true, secureAfter(block = false, mustSecure = true))
        assertEquals(true, secureAfter(block = true, mustSecure = true))
    }
}
