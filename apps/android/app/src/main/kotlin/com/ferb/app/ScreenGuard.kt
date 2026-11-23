package com.ferb.app

import android.view.Window
import android.view.WindowManager

/**
 * The one place that says which of FERB's screens screenshots and screen recordings may show (FLAG_SECURE on
 * MainActivity's window, kb PHN-811). FERB draws every page, sheet and overlay inside that one window, so each follows
 * it; a dialog or popup window opened from it inherits the flag when it opens (Compose's SecureFlagPolicy.Inherit),
 * and the scanner's camera is a TextureView drawn inside the window. The owner's switch decides for every page (PHN-420;
 * owner 2026-11-23, kb PHN-810): the lock page (PHN-793), Watch live, a task's short view and the Mac's camera
 * included. The one exception: a hidden answer's box (the view model's mustSecure, PHN-110) is kept out whatever the
 * switch says. The recent-apps picture is covered always and separately (setRecentsScreenshotEnabled, PHN-15).
 */
object ScreenGuard {
    fun secure(blockScreenshots: Boolean, mustSecure: Boolean): Boolean = blockScreenshots || mustSecure

    fun apply(window: Window, on: Boolean) {
        if (on) window.setFlags(WindowManager.LayoutParams.FLAG_SECURE, WindowManager.LayoutParams.FLAG_SECURE)
        else window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
    }
}
