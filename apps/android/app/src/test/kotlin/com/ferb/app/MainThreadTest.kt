package com.ferb.app

import android.app.Application
import android.os.Looper
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.Why
import com.ferb.core.KeyKind
import com.ferb.mobile.Change
import com.ferb.mobile.Item
import com.ferb.mobile.NoticeItem
import com.ferb.mobile.OlderState
import com.ferb.mobile.TranscriptView
import java.nio.file.Files
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/**
 * The main thread after a pairing and its unlock (kb PHN-657): the Mac's whole chat arrives page by page right then.
 * What the screens are built from is made on the main thread; it must stay inside a frame budget even for a long
 * chat, and the main thread never waits on the phone's slow calls (they are the library's, off the main thread).
 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class MainThreadTest {
    private val phone = FakePhone()
    private val root = Files.createTempDirectory("ferb-main").toFile()

    @Before fun setUp() { Dispatchers.setMain(UnconfinedTestDispatcher()) }
    @After fun tearDown() { Dispatchers.resetMain(); root.deleteRecursively() }

    private fun notice(i: Int) = Item.Notice(NoticeItem("e$i", 1_700_000_000.0 + i * 60, false, "e$i", null, "A line from FERB number $i", emptyList(), null))

    // decision: PHN-657
    @Test
    fun a_long_chat_arriving_after_pairing_never_holds_the_main_thread_past_a_frame_budget() {
        val session = Session { 0L }
        session.unlocked()
        val vm = FerbViewModel(phone, session, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
            LocalFiles(root) { null }, wait = {}, io = Dispatchers.Main, steady = { 0L })
        vm.prompts = object : Prompts {
            override fun signer(kind: KeyKind, why: Why) = FakeSigner(kind, false)
            override suspend fun local() = true
        }
        // the history arrives in pages: each page is one change, and each change is read again
        var worst = 0L
        for (page in 1..20) {
            phone.view = TranscriptView("c1", (0 until page * 250).map(::notice), emptyList(), emptyList(), emptyList(), OlderState.Complete)
            val t0 = System.nanoTime()
            phone.changes.tryEmit(Change.Transcript("c1"))
            shadowOf(Looper.getMainLooper()).idle()
            worst = maxOf(worst, (System.nanoTime() - t0) / 1_000_000)
        }
        println("main-thread worst per change: $worst ms")
        assertTrue("5 000 entries: one change took $worst ms on the main thread", worst < 100)
    }
}
