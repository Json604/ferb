package com.ferb.app

import android.view.Window
import android.view.WindowManager

/**
 * Which of FERB's screens screenshots and screen recordings may show (FLAG_SECURE). The owner's switch decides
 * (PHN-420), the lock page included (owner 2026-11-22, kb PHN-793): it shows only FERB's mark and fixed words, and
 * the unlock is the system's own prompt, so FERB never draws a PIN. A private screen (a hidden answer's box, Watch
 * live, the Mac's camera: the view model's mustSecure, PHN-110, PHN-454) is kept out whatever the switch says. The
 * recent-apps picture is covered always and separately (setRecentsScreenshotEnabled, PHN-15).
 */
object ScreenGuard {
    fun secure(blockScreenshots: Boolean, mustSecure: Boolean): Boolean = blockScreenshots || mustSecure

    fun apply(window: Window, on: Boolean) {
        if (on) window.setFlags(WindowManager.LayoutParams.FLAG_SECURE, WindowManager.LayoutParams.FLAG_SECURE)
        else window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
    }
}
