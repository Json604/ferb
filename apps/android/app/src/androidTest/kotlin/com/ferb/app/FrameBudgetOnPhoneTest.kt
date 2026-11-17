package com.ferb.app

import android.os.ParcelFileDescriptor
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.statusBars
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalDensity
import androidx.activity.compose.setContent
import androidx.test.core.app.ActivityScenario
import androidx.compose.ui.unit.max
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.UiDevice
import com.ferb.app.model.ChatRow
import com.ferb.app.model.LocalFiles
import com.ferb.app.ui.Host
import com.ferb.app.ui.Main
import com.ferb.core.KeyHealth
import com.ferb.core.KeyState
import com.ferb.design.components.PhoneTab
import com.ferb.design.preview.labMoment
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import java.io.File
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The app's own chat screen on a running emulator or phone, with the JVM tests' stand-in phone (no Mac), measured by
 * Android's own frame counter (`dumpsys gfxinfo`, kb PHN-713): an idle chat of 500 rows (no frame may be drawn while
 * nothing on it changes), scrolling it, a reply streaming 20 words a second into it, the keyboard opening and closing,
 * and the tabs. Each scene's frames and their 50/90/99th percentile times are logged under `ferb-frames`
 * (`adb logcat -s ferb-frames`). An emulator's GPU is not a phone's: the counts hold everywhere, the times only there.
 * Alone: adb shell am instrument -w -e class com.ferb.app.FrameBudgetOnPhoneTest
 * com.ferb.phone.test/androidx.test.runner.AndroidJUnitRunner
 */
@RunWith(AndroidJUnit4::class)
class FrameBudgetOnPhoneTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val device = UiDevice.getInstance(instrumentation)
    private val host = object : Host {
        override fun stayConnected(on: Boolean) {}
        override fun blockScreenshots(on: Boolean) {}
        override fun askNotifications() {}
        override fun pickFiles(forChat: Boolean) {}
        override fun handOut(key: String, file: File, name: String, share: Boolean) {}
    }
    private var ui by mutableStateOf(UiState())
    private var tab by mutableStateOf(PhoneTab.CHAT)
    private lateinit var scenario: ActivityScenario<ComponentActivity>

    private fun rows(n: Int, last: String) = (1..n).map {
        if (it % 2 == 0) ChatRow.Ferb("f$it", "FERB's line number $it, long enough to wrap onto a second line in its bubble on a phone.")
        else ChatRow.Owner("o$it", "The owner's message $it", emptyList(), null)
    } + ChatRow.Ferb("reply", last)

    private fun shell(command: String): String {
        val pfd = instrumentation.uiAutomation.executeShellCommand(command)
        return ParcelFileDescriptor.AutoCloseInputStream(pfd).bufferedReader().use { it.readText() }
    }

    /** On the main thread, as the app's own state changes arrive. */
    private fun onMain(block: () -> Unit) = scenario.onActivity { block() }

    /** The frames this process drew while [block] ran, as gfxinfo counts them. */
    private fun frames(scene: String, block: () -> Unit): Int {
        device.waitForIdle()
        Thread.sleep(1_500)
        shell("dumpsys gfxinfo ${instrumentation.targetContext.packageName} reset")
        block()
        val out = shell("dumpsys gfxinfo ${instrumentation.targetContext.packageName} framestats")
        fun line(prefix: String) = out.lineSequence().firstOrNull { it.startsWith(prefix) }?.substringAfter(':')?.trim().orEmpty()
        val total = line("Total frames rendered").toIntOrNull() ?: -1
        Log.i("ferb-frames", "$scene: frames $total, janky ${line("Janky frames")}, p50 ${line("50th percentile")}, " +
            "p90 ${line("90th percentile")}, p99 ${line("99th percentile")}, main thread per frame ${mainThread(out)}")
        return total
    }

    /**
     * The main thread's share of each frame (input, animation, measure, layout and recording the drawing: from
     * HandleInputStart to SyncQueued) over the last frames gfxinfo keeps (120): the app's own work, apart from the GPU.
     */
    private fun mainThread(out: String): String {
        val lines = out.lineSequence().dropWhile { !it.startsWith("Flags,") }.toList()
        if (lines.isEmpty()) return "-"
        val head = lines.first().split(',')
        val input = head.indexOf("HandleInputStart")
        val queued = head.indexOf("SyncQueued")
        val ms = lines.drop(1).takeWhile { it.isNotBlank() && !it.startsWith("---") }.mapNotNull { l ->
            val c = l.split(',')
            if (c.size <= queued || c[0] != "0") null else (c[queued].toLong() - c[input].toLong()) / 1e6
        }.filter { it >= 0 }.sorted()
        if (ms.isEmpty()) return "-"
        fun p(q: Double) = "%.1f".format(ms[((ms.size - 1) * q).toInt()])
        return "p50 ${p(0.5)} ms, p90 ${p(0.9)} ms, max ${p(1.0)} ms (${ms.size} frames)"
    }

    /** The longest single message the main looper ran while [block] ran, in ms (the looper's own dispatch log). */
    private fun longestMain(block: () -> Unit): Long {
        if (InstrumentationRegistry.getArguments().getString("looper_log") == "off") { block(); return -1 }
        val longest = java.util.concurrent.atomic.AtomicLong()
        var began = 0L
        val log = android.util.Printer { line ->
            if (line.startsWith(">>>>>")) began = System.nanoTime()
            else if (line.startsWith("<<<<<") && began != 0L) longest.accumulateAndGet(System.nanoTime() - began, ::maxOf)
        }
        onMain { android.os.Looper.getMainLooper().setMessageLogging(log) }
        try { block() } finally { onMain { android.os.Looper.getMainLooper().setMessageLogging(null) } }
        return longest.get() / 1_000_000
    }

    /**
     * The chat in an activity of its own on the real frame clock (a compose test rule's clock only moves when the test
     * waits on it, so nothing it draws by itself would be counted).
     */
    private fun open(n: Int) {
        ui = UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK),
            conversation = "c1", rows = rows(n, "The reply"))
        val vm = FerbViewModel(FakePhone(), Session { 0L }, Prefs(ApplicationProvider.getApplicationContext()), { true }, null,
            SavedStateHandle(), LocalFiles(File(instrumentation.targetContext.cacheDir, "frames-${System.nanoTime()}").apply { mkdirs() }) { null })
        scenario = ActivityScenario.launch(ComponentActivity::class.java)
        val longest = longestMain { onMain {
            scenario.onActivity { a ->
                a.enableEdgeToEdge()
                a.setContent {
                    // the insets as FerbRoot makes them: the bottom read where it is used (kb PHN-712)
                    val density = LocalDensity.current
                    val top = with(density) { WindowInsets.statusBars.getTop(this).toDp() }
                    val nav = WindowInsets.navigationBars
                    val ime = WindowInsets.ime
                    // -e insets read_in_root: as FerbRoot read them before kb PHN-712, for a comparison
                    val insets = if (InstrumentationRegistry.getArguments().getString("insets") == "read_in_root") {
                        PhoneInsets(top, with(density) { max(nav.getBottom(this).toDp(), ime.getBottom(this).toDp()) })
                    } else {
                        remember(top, density) { PhoneInsets.live(top) { with(density) { max(nav.getBottom(this).toDp(), ime.getBottom(this).toDp()) } } }
                    }
                    val moment = remember { labMoment(12.5) }
                    PhoneScreen(moment, insets = insets) { Main(vm, ui, tab, home = false, insets, moment, host) }
                }
            }
        }
        Thread.sleep(3_000) }
        Log.i("ferb-frames", "opening a chat of $n rows: the longest main-thread message $longest ms")
    }

    // decision: PHN-710, PHN-711, PHN-713
    @Test
    fun the_chat_draws_only_when_something_on_it_changes() {
        open(500)
        val w = device.displayWidth
        val h = device.displayHeight
        val idle = frames("idle chat of 500 rows, 30 s") { Thread.sleep(30_000) }
        frames("a reply streaming 20 words a second for 10 s into 500 rows") {
            var text = "The reply"
            repeat(200) { i ->
                text += " word$i"
                val next = rows(500, text)
                onMain { ui = ui.copy(rows = next) }
                Thread.sleep(50)
            }
        }
        frames("the keyboard opening and closing, 3 times") {
            repeat(3) {
                // the message box, just above the gesture bar
                device.click(w / 2, h - (60 * instrumentation.targetContext.resources.displayMetrics.density).toInt())
                Thread.sleep(1_500)
                onMain { scenario.onActivity { it.window.insetsController?.hide(android.view.WindowInsets.Type.ime()) } }
                Thread.sleep(1_500)
            }
        }
        frames("the tabs, 12 switches") {
            val order = listOf(PhoneTab.TASKS, PhoneTab.FILES, PhoneTab.MAC, PhoneTab.CHAT)
            repeat(12) { i -> onMain { tab = order[i % 4] }; Thread.sleep(800) }
        }
        // each tab at rest draws nothing
        val still = PhoneTab.entries.associateWith { t ->
            onMain { tab = t }
            Thread.sleep(2_000)
            frames("the ${t.title} tab at rest, 5 s") { Thread.sleep(5_000) }
        }
        // a folder of 500 files (one page, PHN-609) opened on the Files tab
        onMain { tab = PhoneTab.FILES }
        Thread.sleep(2_000)
        val entries = (1..500).map { com.ferb.mobile.FileEntry("Report $it.pdf", "~/Downloads/Report $it.pdf", false, false, 120_000u, 1_700_000_000L + it, false) }
        val longest = longestMain {
            frames("opening a folder of 500 files") {
                onMain { ui = ui.copy(files = FilesUi(folder = com.ferb.mobile.Folder("~/Downloads", "~", false, entries, 500u, false, 0u, null))) }
                Thread.sleep(4_000)
            }
        }
        Log.i("ferb-frames", "opening a folder of 500 files: the longest main-thread message ${longest} ms")
        onMain { ui = ui.copy(files = FilesUi()); tab = PhoneTab.CHAT }
        Thread.sleep(2_000)
        frames("scrolling the 500-row chat, 12 flings") {
            repeat(6) { device.swipe(w / 2, h / 3, w / 2, h * 3 / 4, 8); Thread.sleep(700) }
            repeat(6) { device.swipe(w / 2, h * 3 / 4, w / 2, h / 3, 8); Thread.sleep(700) }
        }
        scenario.close()
        assertTrue("an idle chat drew $idle frames in 30 s", idle <= 2)
        for ((t, n) in still) assertTrue("the ${t.title} tab at rest drew $n frames in 5 s", n <= 1)
    }

    /**
     * A reply streaming into a long chat through the real view model (the stand-in phone's transcript, read whole on
     * every change as the library hands it): the longest single piece of main-thread work per word, from the main
     * looper's own dispatch log, for 500 and 5 000 entries. Logged only: the bound is the next decision's.
     */
    // decision: PHN-713
    @Test
    fun a_streamed_word_on_the_main_thread_for_500_and_5000_entries() {
        val sizes = InstrumentationRegistry.getArguments().getString("entries")?.split(',')?.map { it.trim().toInt() } ?: listOf(500, 5_000)
        for (n in sizes) {
            val phone = FakePhone()
            val session = Session { 0L }
            session.unlocked()
            fun notice(i: Int, text: String) = com.ferb.mobile.Item.Notice(com.ferb.mobile.NoticeItem("e$i", 1_700_000_000.0 + i * 60, false, "e$i", null, text, emptyList(), null))
            val older = (0 until n).map { notice(it, "A line from FERB number $it, long enough to wrap in its bubble.") }
            phone.view = com.ferb.mobile.TranscriptView("c1", older + notice(n, "The reply"), emptyList(), emptyList(), emptyList(), com.ferb.mobile.OlderState.Complete)
            lateinit var vm: FerbViewModel
            scenario = ActivityScenario.launch(ComponentActivity::class.java)
            scenario.onActivity { a ->
                vm = FerbViewModel(phone, session, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
                    LocalFiles(File(instrumentation.targetContext.cacheDir, "frames-${System.nanoTime()}").apply { mkdirs() }) { null })
                a.enableEdgeToEdge()
                a.setContent {
                    val u by vm.ui.collectAsState()
                    val moment = remember { labMoment(12.5) }
                    PhoneScreen(moment, insets = PhoneInsets.Lab) { Main(vm, u.copy(gate = Gate.MAIN), PhoneTab.CHAT, home = false, PhoneInsets.Lab, moment, host) }
                }
                phone.changes.tryEmit(com.ferb.mobile.Change.Transcript("c1"))
            }
            Thread.sleep(4_000)
            var text = "The reply"
            val longest = longestMain { frames("$n entries through the view model, 10 words a second for 6 s") {
                repeat(60) { i ->
                    text += " word$i"
                    phone.view = phone.view!!.copy(items = older + notice(n, text))
                    phone.changes.tryEmit(com.ferb.mobile.Change.Transcript("c1"))
                    Thread.sleep(100)
                }
            } }
            Log.i("ferb-frames", "$n entries: the longest main-thread message while words streamed: $longest ms")
            scenario.close()
        }
    }
}
