package com.ferb.app

import android.util.Log

/**
 * The app's own lines in logcat, tag `ferb-app` (kb PHN-641): what the lock and the Mac's unlock did and how long each
 * took, and the microphone's states, so a report from a real phone can be read from `adb logcat -s ferb-app`. Never a
 * secret, a path, a message's text or the words spoken: only states, codes, counts and milliseconds.
 */
object AppLog {
    const val TAG = "ferb-app"

    fun i(line: String) {
        runCatching { Log.i(TAG, line) }
    }
}
