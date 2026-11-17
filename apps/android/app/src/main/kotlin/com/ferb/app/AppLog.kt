package com.ferb.app

import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import java.io.File
import java.util.concurrent.Executors

/**
 * The app's own lines in logcat, tag `ferb-app` (kb PHN-641, PHN-657): what the lock and the Mac's unlock did and how
 * long each took, the microphone's states, and any stall of the main thread with where it stood, so a report from a
 * real phone can be read from `adb logcat -s ferb-app`. Never a secret, a path, a message's text or the words spoken:
 * only states, codes, counts and milliseconds. A phone's logcat drops lines of a busy app, so on a debuggable build the
 * lines also go to a small file of their own (`files/ferb-app.log`, 256 KB, the newest half kept), read with
 * `adb shell run-as com.ferb.phone cat files/ferb-app.log`.
 */
object AppLog {
    const val TAG = "ferb-app"
    private const val MAX = 256 * 1024L
    @Volatile private var file: File? = null
    private val writer by lazy { Executors.newSingleThreadExecutor { r -> Thread(r, "ferb-app-log").apply { isDaemon = true } } }

    /** Debuggable builds: the lines are kept in [f] too. */
    fun keepIn(f: File) { file = f }

    fun i(line: String) {
        runCatching { Log.i(TAG, line) }
        val f = file ?: return
        val stamped = "${System.currentTimeMillis()} ${Thread.currentThread().name}: $line\n"
        runCatching {
            writer.execute {
                runCatching {
                    if (f.length() > MAX) {
                        val keep = f.readText().takeLast((MAX / 2).toInt())
                        f.writeText(keep)
                    }
                    f.appendText(stamped)
                }
            }
        }
    }

    /**
     * A watchdog on its own thread (debuggable builds, kb PHN-657): the main thread is asked to answer every 500 ms;
     * when it hasn't for [stallMs], one line says how long and where the main thread stands (its top frames, the app's
     * own code first), and again each further [stallMs] while it stays stuck.
     */
    fun watchMain(stallMs: Long = 1_000) {
        val main = Handler(Looper.getMainLooper())
        Thread({
            val answered = java.util.concurrent.atomic.AtomicLong(SystemClock.uptimeMillis())
            var told = 0L
            while (true) {
                main.post { answered.set(SystemClock.uptimeMillis()) }
                runCatching { Thread.sleep(500) }
                val stuck = SystemClock.uptimeMillis() - answered.get()
                if (stuck >= stallMs && stuck - told >= stallMs) {
                    told = stuck
                    val frames = Looper.getMainLooper().thread.stackTrace
                    val ours = frames.filter { it.className.startsWith("com.ferb") }.take(8)
                    val top = frames.take(6)
                    i("main thread stalled $stuck ms at ${(ours + top).distinct().joinToString(" < ") { "${it.className.substringAfterLast('.')}.${it.methodName}:${it.lineNumber}" }}")
                } else if (stuck < stallMs) told = 0
            }
        }, "ferb-main-watch").apply { isDaemon = true; priority = Thread.MIN_PRIORITY }.start()
    }
}
