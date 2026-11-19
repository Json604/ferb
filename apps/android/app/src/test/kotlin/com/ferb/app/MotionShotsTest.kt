package com.ferb.app

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.Snapshot
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.ChatRow
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.Question
import com.ferb.app.model.Work
import com.ferb.app.model.WorkItem
import com.ferb.app.ui.Host
import com.ferb.app.ui.Main
import com.ferb.app.ui.Sheets
import com.ferb.core.KeyHealth
import com.ferb.core.KeyState
import com.ferb.design.MotionKind
import com.ferb.design.MotionSwitch
import com.ferb.design.Phone
import com.ferb.design.heldWhileLeaving
import com.ferb.design.components.MarkKind
import com.ferb.design.components.PhoneTab
import com.ferb.design.preview.labMoment
import com.ferb.design.screens.LabAndroidChrome
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.ferb.design.sky.SkyMoment
import com.github.takahirom.roborazzi.captureRoboImage
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.io.File

/**
 * The app's motion settles on the very pixels of its still screens (kb PHN-622, PHN-623): a sheet that came up through
 * the sheet host and a tab reached through the tab host are compared with ScreenShotsTest's goldens, unchanged.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(qualifiers = "w412dp-h1000dp-port-mdpi")
class MotionShotsTest {
    @get:Rule val compose = createComposeRule()

    private val phone = FakePhone()
    private val vm by lazy {
        FerbViewModel(phone, Session { 0L }, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
            LocalFiles(java.nio.file.Files.createTempDirectory("ferb-motion").toFile()) { null })
    }
    private val host = object : Host {
        override fun stayConnected(on: Boolean) {}
        override fun blockScreenshots(on: Boolean) {}
        override fun askNotifications() {}
        override fun pickFiles(forChat: Boolean) {}
        override fun handOut(key: String, file: File, name: String, share: Boolean) {}
    }
    private val labRows = listOf(
        ChatRow.Owner("a", "What's left on my calendar today?", emptyList(), null),
        ChatRow.Ferb("b", "Two things: the design review at 4, and a call with the bank at 6:30."),
        ChatRow.Owner("c", "Compare the three standing desks I bookmarked and put the cheapest in my cart", emptyList(), null),
        ChatRow.Task("d", "t-1", "On it. Three parts.", 2, 3, listOf(
            com.ferb.design.components.PlanPart(MarkKind.DONE, "Open your three desks", "3 pages read"),
            com.ferb.design.components.PlanPart(MarkKind.RUNNING, "Compare price and delivery", "Reading delivery options"),
            com.ferb.design.components.PlanPart(MarkKind.WAITING, "Put the cheapest in your cart"),
        )),
    )
    private val asking = Question("ap-1", "t-1", "Run this in Terminal?", "To update Figma before opening the file you asked for.",
        "brew upgrade --cask figma", "owner_question", "ferb.confirm", listOf("Yes", "No"), listOf("Yes, run it", "No"), false)
    private val base = UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK))

    /** As Root draws it: the tab host in Main, the sheet host over it. */
    @Composable
    private fun Frame(moment: SkyMoment, ui: UiState, tab: PhoneTab, sheet: Sheet?) {
        Box(Modifier.size(Phone.frameWidth, Phone.frameHeight).testTag(PHONE)) {
            PhoneScreen(moment, insets = PhoneInsets.Lab, glide = false, sheet = {
                MotionSwitch(sheet, Modifier.fillMaxSize(), MotionKind.SHEET) { s -> Sheets(vm, heldWhileLeaving(ui), s, PhoneInsets.Lab, host) }
                LabAndroidChrome()
            }) { Main(vm, ui, tab, home = false, PhoneInsets.Lab, moment, host) }
        }
    }

    private fun settleAndCompare(golden: String) {
        compose.mainClock.autoAdvance = false
        repeat(90) {
            Snapshot.sendApplyNotifications()
            compose.mainClock.advanceTimeByFrame()
        }
        compose.onNodeWithTag(PHONE).captureRoboImage("src/test/screenshots/$golden", roborazziOptions = ScreenShotsTest.options)
    }

    /** A sheet that came up from below settles on the still golden (questions are boxes in the chat now, PHN-770). */
    // decision: PHN-623
    @Test
    fun a_sheet_that_came_up_from_below_rests_on_its_golden() {
        var sheet by mutableStateOf<Sheet?>(null)
        compose.setContent { Frame(labMoment(6.1), base.copy(rows = labRows), PhoneTab.CHAT, sheet) }
        compose.mainClock.autoAdvance = false
        sheet = Sheet.Profile
        settleAndCompare("profile.png")
    }

    // decision: PHN-622
    @Test
    fun tasks_reached_from_the_chat_rest_on_the_tasks_golden() {
        val work = Work(
            listOf(WorkItem(MarkKind.ASKING, "Renew the domain", "Asks before paying $12.99", approvalId = "ap-1")),
            listOf(WorkItem(MarkKind.RUNNING, "Compare standing desks", "Comparing price and delivery", 2 to 3)),
            listOf(WorkItem(MarkKind.DONE, "Back up the thesis folder", "Copied to the external drive"), WorkItem(MarkKind.FAILED, "Download the October invoice", "The site asked for a code that never came")),
        )
        var tab by mutableStateOf(PhoneTab.CHAT)
        compose.setContent { Frame(labMoment(12.5), base.copy(work = work), tab, null) }
        compose.mainClock.autoAdvance = false
        tab = PhoneTab.TASKS
        settleAndCompare("tasks.png")
    }

    private companion object {
        const val PHONE = "phone"
    }
}
