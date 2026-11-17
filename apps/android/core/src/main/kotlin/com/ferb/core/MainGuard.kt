package com.ferb.core

import android.os.Looper
import android.util.Log

/**
 * Blocking work never runs on the main thread (kb PHN-657): every library (FFI) call, Keystore round and file read the
 * phone code makes is a call that can wait. [offMain] says so in logcat (tag `ferb-app`, the call's name) when one is
 * reached from the main thread; tests can make it throw ([strict]).
 */
object MainGuard {
    /** Tests: a call on the main thread throws instead of only saying so. */
    @Volatile var strict = false

    /** Told each call that was reached from the main thread (the app keeps the line in its own log; tests count them). */
    @Volatile var onMain: ((String) -> Unit)? = null

    fun offMain(what: String) {
        val main = runCatching { Looper.getMainLooper()?.thread === Thread.currentThread() }.getOrDefault(false)
        if (!main) return
        runCatching { Log.w("ferb-app", "blocking call on the main thread: $what") }
        runCatching { onMain?.invoke(what) }
        if (strict) throw IllegalStateException("$what on the main thread")
    }
}
