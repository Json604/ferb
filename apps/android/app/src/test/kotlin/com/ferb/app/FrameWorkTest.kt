package com.ferb.app

import android.app.Application
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composer
import androidx.compose.runtime.CompositionTracer
import androidx.compose.runtime.InternalComposeTracingApi
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.Snapshot
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.unit.dp
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.ChatRow
import com.ferb.app.model.LocalFiles
import com.ferb.app.ui.Host
import com.ferb.app.ui.Main
import com.ferb.core.KeyHealth
import com.ferb.core.KeyState
import com.ferb.design.Phone
import com.ferb.design.components.PhoneTab
import com.ferb.design.preview.labMoment
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * The work the chat screen does per frame (kb PHN-710..713), counted by the compiler's trace markers: every composable
 * body that runs is one (re)composition of it. An idle chat does nothing; a streamed word costs the same in a long
 * chat as in a short one; the keyboard's slide recomposes nothing above the page (the root, the sky, the hosts, the
 * tab bar) and none of the chat's rows. The frames themselves are counted on a phone (FrameBudgetOnPhoneTest).
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(application = Application::class, sdk = [35], qualifiers = "w412dp-h915dp-port-mdpi")
class FrameWorkTest {
    @get:Rule val compose = createAndroidComposeRule<androidx.activity.ComponentActivity>()

    private val host = object : Host {
        override fun stayConnected(on: Boolean) {}
        override fun blockScreenshots(on: Boolean) {}
        override fun askNotifications() {}
        override fun pickFiles(forChat: Boolean) {}
        override fun handOut(key: String, file: File, name: String, share: Boolean) {}
    }
    private var bottom by mutableStateOf(Phone.gestureArea)
    private var ui by mutableStateOf(UiState())
    private var shown = false

    @After fun off() = Runs.uninstall()

    private fun rows(n: Int, last: String = "The reply") = (1..n).map {
        if (it % 2 == 0) ChatRow.Ferb("f$it", "FERB's line number $it, long enough to fill a bubble.")
        else ChatRow.Owner("o$it", "The owner's message $it", emptyList(), null)
    } + ChatRow.Ferb("reply", last)

    /** The chat as FerbRoot draws it: the root reads the state, the insets' bottom is read where it is used. */
    private fun chat(n: Int) {
        ui = UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK),
            conversation = "c1", rows = rows(n))
        if (shown) {
            // the same screen, the chat changed under it (a second setContent is not allowed)
            repeat(60) { frame() }
            Runs.install()
            return
        }
        shown = true
        val vm = FerbViewModel(FakePhone(), Session { 0L }, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
            LocalFiles(java.nio.file.Files.createTempDirectory("ferb-frames").toFile()) { null }, account = testAccount())
        compose.setContent {
            val moment = remember { labMoment(12.5) }
            val insets = remember { PhoneInsets.live(Phone.statusBar) { bottom } }
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight)) {
                PhoneScreen(moment, insets = insets, glide = false) { Main(vm, ui, PhoneTab.CHAT, home = false, insets, moment, host) }
            }
        }
        compose.waitForIdle()
        compose.mainClock.autoAdvance = false
        repeat(60) { compose.mainClock.advanceTimeByFrame() }
        Runs.install()
    }

    private fun frame() {
        Snapshot.sendApplyNotifications()
        compose.mainClock.advanceTimeByFrame()
    }

    /** Every composable body that ran per word of a reply streamed into a chat of [n] rows. */
    private fun perWord(n: Int): Double {
        chat(n)
        var text = "The reply"
        repeat(20) {
            text += " word$it"
            ui = ui.copy(rows = rows(n, text))
            frame()
        }
        repeat(30) { frame() }
        return Runs.total() / 20.0
    }

    // decision: PHN-710, PHN-711, PHN-713
    @Test
    fun an_idle_chat_recomposes_nothing() {
        chat(60)
        repeat(30 * 60) { frame() }
        assertEquals("30 s of an idle chat ran:\n${Runs.top()}", 0, Runs.total())
    }

    // decision: PHN-710, PHN-711
    @Test
    fun a_streamed_word_costs_the_same_in_a_long_chat() {
        val short = perWord(50)
        val long = perWord(500)
        assertTrue("a word cost $short bodies in 50 rows and $long in 500", long <= short * 1.1 + 5)
    }

    // decision: PHN-710, PHN-714
    @Test
    fun a_chat_of_5000_rows_opens_at_its_end_with_only_a_screenful_composed() {
        ui = UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK),
            conversation = "c1", rows = rows(5_000, "The very last reply"))
        val vm = FerbViewModel(FakePhone(), Session { 0L }, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
            LocalFiles(java.nio.file.Files.createTempDirectory("ferb-frames").toFile()) { null }, account = testAccount())
        Runs.install()
        compose.setContent {
            val moment = remember { labMoment(12.5) }
            val insets = remember { PhoneInsets.live(Phone.statusBar) { bottom } }
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight)) {
                PhoneScreen(moment, insets = insets, glide = false) { Main(vm, ui, PhoneTab.CHAT, home = false, insets, moment, host) }
            }
        }
        compose.waitForIdle()
        val rowsComposed = Runs.of("com.ferb.app.ui.ChatRowView")
        assertTrue("opening 5 001 rows composed $rowsComposed of them", rowsComposed <= 60)
        compose.onNodeWithText("The very last reply", substring = true).assertIsDisplayed()
        // the keyboard at the end keeps the end in view and composes no more than its own rows
        Runs.install()
        compose.mainClock.autoAdvance = false
        for (i in 1..12) {
            bottom = (24f + 306f * i / 12).dp
            frame()
        }
        repeat(30) { frame() }
        compose.onNodeWithText("The very last reply", substring = true).assertIsDisplayed()
        assertTrue("the keyboard composed ${Runs.of("com.ferb.app.ui.ChatRowView")} rows", Runs.of("com.ferb.app.ui.ChatRowView") <= 60)
    }

    // decision: PHN-710, PHN-715
    @Test
    fun a_folder_of_500_files_never_composes_all_its_rows_in_one_frame() {
        var tab by mutableStateOf(PhoneTab.FILES)
        ui = UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK), conversation = "c1")
        val vm = FerbViewModel(FakePhone(), Session { 0L }, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
            LocalFiles(java.nio.file.Files.createTempDirectory("ferb-frames").toFile()) { null }, account = testAccount())
        compose.setContent {
            val moment = remember { labMoment(12.5) }
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight)) {
                PhoneScreen(moment, insets = PhoneInsets.Lab, glide = false) { Main(vm, ui, tab, home = false, PhoneInsets.Lab, moment, host) }
            }
        }
        compose.waitForIdle()
        compose.mainClock.autoAdvance = false
        Runs.install()
        val entries = (1..500).map { com.ferb.mobile.FileEntry("Report $it.pdf", "~/Downloads/Report $it.pdf", false, false, 120_000u, 1_700_000_000L + it, false) }
        ui = ui.copy(files = FilesUi(folder = com.ferb.mobile.Folder("~/Downloads", "~", false, entries, 500u, false, 0u, null)))
        frame()
        val first = Runs.of("com.ferb.design.components.CardRow")
        assertTrue("the folder's first frame composed $first rows", first in 1..60)
        repeat(40) { frame() }
        assertTrue("all 500 rows come within frames: ${Runs.of("com.ferb.design.components.CardRow")}", Runs.of("com.ferb.design.components.CardRow") >= 500)
        compose.onNodeWithText("Report 500.pdf").assertExists()
        // each row is made once: rows already there are skipped as the rest come
        assertTrue("rows' details were made ${Runs.of("com.ferb.app.ui.detail")} times for 500 rows", Runs.of("com.ferb.app.ui.detail") <= 520)
    }

    // decision: PHN-710, PHN-712
    @Test
    fun the_keyboard_recomposes_nothing_above_the_page_and_no_row() {
        chat(60)
        for (i in 1..12) {
            bottom = (24f + 306f * i / 12).dp
            frame()
        }
        for (above in listOf("com.ferb.app.ui.Main", "com.ferb.design.screens.PhoneScreen", "com.ferb.design.sky.Sky",
            "com.ferb.design.components.TabBarHost", "com.ferb.app.ui.ChatRowView")) {
            assertEquals("$above ran while the keyboard slid:\n${Runs.top()}", 0, Runs.of(above))
        }
    }
}

/** Counts the composable bodies that run, by function (the compiler's trace markers). */
@OptIn(InternalComposeTracingApi::class)
internal object Runs : CompositionTracer {
    private val counts = ConcurrentHashMap<String, Int>()
    override fun isTraceInProgress() = true
    override fun traceEventStart(key: Int, dirty1: Int, dirty2: Int, info: String) {
        counts.merge(info.substringBefore(" ("), 1, Int::plus)
    }
    override fun traceEventEnd() {}
    fun install() { counts.clear(); Composer.setTracer(this) }
    fun uninstall() = Composer.setTracer(Off)
    fun total() = counts.values.sum()
    fun of(name: String) = counts[name] ?: 0
    fun top() = counts.entries.filter { it.key.startsWith("com.ferb") }.sortedByDescending { it.value }.take(20)
        .joinToString("\n") { "  ${it.value}  ${it.key}" }

    private object Off : CompositionTracer {
        override fun isTraceInProgress() = false
        override fun traceEventStart(key: Int, dirty1: Int, dirty2: Int, info: String) {}
        override fun traceEventEnd() {}
    }
}
