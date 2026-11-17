package com.ferb.app

import android.os.Process
import android.os.SystemClock

/**
 * The start-up's milestones under `ferb-app` (kb PHN-633), each in ms since the process began, so a start on the
 * owner's own phone can be read with `adb logcat -s ferb-app`: the Application, the activity, the first frame, the
 * first page the gate allows, the phone opened (library, keys, link and store, off the main thread) and the launch
 * settled (also Android's own "Fully drawn" line). Each milestone once per process (the activity's once per launch).
 */
object StartLog {
    private val seen = HashSet<String>()

    /** Ms since this process began. */
    fun sinceStart(): Long = SystemClock.uptimeMillis() - Process.getStartUptimeMillis()

    /** Whether this process began less than [ms] ago (a cold start's first activity). */
    fun fresh(ms: Long = 15_000): Boolean = sinceStart() < ms

    @Synchronized
    fun once(what: String, detail: String = "") {
        if (!seen.add(what)) return
        AppLog.i("start: $what at ${sinceStart()} ms${if (detail.isEmpty()) "" else " ($detail)"}")
    }

    /** A milestone of each launch of the activity (cold or warm): logged every time. */
    fun each(what: String, detail: String = "") {
        AppLog.i("start: $what at ${sinceStart()} ms${if (detail.isEmpty()) "" else " ($detail)"}")
    }
}
