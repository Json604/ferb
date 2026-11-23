package com.ferb.app

import android.app.Application
import android.view.Window
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.text.BasicText
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.Popup
import com.ferb.app.model.SpeechKind
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.shadows.ShadowDialog

/**
 * Which screens screenshots may show: one rule in one place (ScreenGuard on MainActivity's window, kb PHN-811). The
 * owner's switch decides for every page, sheet and window FERB opens (PHN-810); only a hidden answer's box keeps the
 * window out whatever the switch says (PHN-110).
 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class ScreenGuardTest {
    @Before fun setUp() { Dispatchers.setMain(UnconfinedTestDispatcher()) }
    @After fun tearDown() { Dispatchers.resetMain() }

    private fun Window.secure() = attributes.flags and WindowManager.LayoutParams.FLAG_SECURE != 0

    private fun secureAfter(block: Boolean, mustSecure: Boolean): Boolean {
        val window = Robolectric.buildActivity(ComponentActivity::class.java).setup().get().window
        ScreenGuard.apply(window, true) // as MainActivity's first frames
        ScreenGuard.apply(window, ScreenGuard.secure(block, mustSecure))
        return window.secure()
    }

    private fun vm() = FerbViewModel(FakePhone(), Session { 0L }, Prefs(androidx.test.core.app.ApplicationProvider.getApplicationContext()), { true }, null,
        androidx.lifecycle.SavedStateHandle(), com.ferb.app.model.LocalFiles(java.nio.file.Files.createTempDirectory("ferb-guard").toFile()) { null })

    /** Every kind of sheet FERB slides up (FerbViewModel's Sheet), the big live views among them. */
    private val everySheet: List<Sheet?> = listOf(
        null, Sheet.Relay, Sheet.Profile, Sheet.Forget, Sheet.TypeCode, Sheet.Scan, Sheet.OwnRelay, Sheet.Notifications,
        Sheet.Help, Sheet.StopAll, Sheet.Settings, Sheet.Days("2026-11-23"), Sheet.Live("t-1"), Sheet.Task("t-1", "t-1.s1"),
        Sheet.Sites("r1"), Sheet.Voice(SpeechKind.ON_DEVICE), Sheet.Location("loc-1"), Sheet.City,
        Sheet.CantNotify(NotifyBlocker.entries.first()), Sheet.Connection, Sheet.Insomnia, Sheet.Camera,
    )

    // decision: PHN-810, PHN-793, PHN-794, PHN-420, PHN-110
    @Test
    fun the_owner_s_switch_decides_every_page_and_only_a_hidden_answer_is_always_kept_out() {
        // the lock page and every other page: the switch (PHN-793, PHN-810)
        assertEquals("the switch off: screenshots show the app", false, secureAfter(block = false, mustSecure = false))
        assertEquals("the switch on: kept out", true, secureAfter(block = true, mustSecure = false))
        // a hidden answer's box: kept out whatever the switch says (PHN-110)
        assertEquals(true, secureAfter(block = false, mustSecure = true))
        assertEquals(true, secureAfter(block = true, mustSecure = true))
    }

    // decision: PHN-810, PHN-811, PHN-110
    @Test
    fun every_sheet_watch_live_the_short_view_and_the_camera_follow_the_switch() {
        val vm = vm()
        for (s in everySheet) {
            vm.show(s)
            assertFalse("$s doesn't force the block", vm.mustSecure.value)
            for (block in listOf(false, true)) {
                assertEquals("$s with the switch ${if (block) "on" else "off"}", block, secureAfter(block, vm.mustSecure.value))
            }
        }
        // a hidden answer's box over any of them keeps the window out with the switch off
        vm.show(Sheet.Live("t-1"))
        vm.hiddenShown("ap-1", true)
        assertEquals(true, secureAfter(block = false, mustSecure = vm.mustSecure.value))
        vm.hiddenShown("ap-1", false)
        assertEquals(false, secureAfter(block = false, mustSecure = vm.mustSecure.value))
        vm.show(null)
    }

    @get:org.junit.Rule val compose = androidx.compose.ui.test.junit4.createAndroidComposeRule<ComponentActivity>()

    /** The windows a dialog and a popup open from FERB's window: each takes FLAG_SECURE from the one window ScreenGuard set. */
    private fun dialogAndPopup(block: Boolean) {
        ScreenGuard.apply(compose.activity.window, ScreenGuard.secure(block, false))
        compose.setContent {
            BasicText("page")
            Popup { BasicText("popup") }
            Dialog(onDismissRequest = {}) { BasicText("dialog") }
        }
        compose.waitForIdle()
        assertEquals("the activity's window, switch $block", block, compose.activity.window.secure())
        val dialog = ShadowDialog.getLatestDialog()
        assertNotNull("the dialog opened", dialog)
        assertEquals("a dialog's own window, switch $block", block, dialog.window!!.secure())
        assertEquals("a popup's own window, switch $block", block, popupParams().flags and WindowManager.LayoutParams.FLAG_SECURE != 0)
    }

    // decision: PHN-811, PHN-810
    @Test
    fun a_dialog_and_a_popup_take_the_switch_on_from_ferb_s_window() = dialogAndPopup(true)

    // decision: PHN-811, PHN-810
    @Test
    fun a_dialog_and_a_popup_take_the_switch_off_from_ferb_s_window() = dialogAndPopup(false)

    /** The layout params of the newest panel window (Compose's Popup adds its view straight to the window manager). */
    private fun popupParams(): WindowManager.LayoutParams {
        val global = Class.forName("android.view.WindowManagerGlobal")
        val g = global.getMethod("getInstance").invoke(null)
        val f = global.getDeclaredField("mParams").apply { isAccessible = true }
        @Suppress("UNCHECKED_CAST")
        val all = f.get(g) as List<WindowManager.LayoutParams>
        return all.lastOrNull { it.type == WindowManager.LayoutParams.TYPE_APPLICATION_PANEL || it.type == WindowManager.LayoutParams.TYPE_APPLICATION_SUB_PANEL }
            ?: error("no popup window among " + all.map { "${it.type}:${it.title}" })
    }
}
