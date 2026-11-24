package com.ferb.app

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.unit.dp
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.ChatRow
import com.ferb.app.model.LocalFiles
import com.ferb.app.ui.Host
import com.ferb.app.ui.LocalQuestions
import com.ferb.app.ui.Main
import com.ferb.app.ui.QuestionBox
import com.ferb.app.ui.Sheets
import com.ferb.core.KeyHealth
import com.ferb.core.KeyState
import com.ferb.design.Phone
import com.ferb.design.components.PhoneTab
import com.ferb.design.preview.labMoment
import com.ferb.design.screens.LabAndroidChrome
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.github.takahirom.roborazzi.RoborazziOptions
import com.github.takahirom.roborazzi.captureRoboImage
import java.io.File
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * FERB's review of what an import brought in (kb MEM-460..457) as the app draws it: in the chat, at the lab's Android
 * size, and its whole box on its own. `:app:recordRoborazziDebug` rewrites them (look at them first).
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(qualifiers = "w412dp-h1000dp-port-mdpi")
class ReviewShotsTest {
    private companion object {
        const val PHONE = "phone"
        val options = RoborazziOptions(compareOptions = RoborazziOptions.CompareOptions(changeThreshold = 0.001f))
    }

    @get:Rule val compose = createComposeRule()

    private val phone = FakePhone().also { it.memoryReview = ReviewData.review }
    private val vm by lazy {
        FerbViewModel(phone, Session { 0L }, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
            LocalFiles(java.nio.file.Files.createTempDirectory("ferb-review-shots").toFile()) { null }, account = testAccount())
    }
    private val host = object : Host {
        override fun stayConnected(on: Boolean) {}
        override fun blockScreenshots(on: Boolean) {}
        override fun askNotifications() {}
        override fun pickFiles(forChat: Boolean) {}
        override fun handOut(key: String, file: File, name: String, share: Boolean) {}
    }
    private val ui = UiState(gate = Gate.MAIN, link = FakePhone.status(), keys = KeyHealth(KeyState.OK, KeyState.OK, KeyState.OK),
        rows = listOf(
            ChatRow.Owner("a", "Bring in what ChatGPT remembers about me", emptyList(), null),
            ChatRow.Ferb("b", "Done: I brought in 42 things from ChatGPT. A few need your word."),
        ),
        questions = listOf(ReviewData.question))

    private fun shoot(path: String) {
        compose.waitForIdle()
        compose.mainClock.autoAdvance = false
        compose.mainClock.advanceTimeByFrame()
        compose.onNodeWithTag(PHONE).captureRoboImage("src/test/screenshots/$path", roborazziOptions = options)
    }

    // decision: MEM-460, PHN-770
    @Test
    fun the_review_in_the_chat() {
        compose.setContent {
            Box(Modifier.size(Phone.frameWidth, Phone.frameHeight).testTag(PHONE)) {
                PhoneScreen(labMoment(21.0), insets = PhoneInsets.Lab, glide = false, sheet = {
                    Sheets(vm, ui, null, PhoneInsets.Lab, host)
                    LabAndroidChrome()
                }) { Main(vm, ui, PhoneTab.CHAT, home = false, PhoneInsets.Lab, labMoment(21.0), host) }
            }
        }
        shoot("review-chat.png")
    }

    /** The whole box, every part, at the chat's width (the chat shows it as the owner scrolls). */
    // decision: MEM-460
    @Test
    @Config(qualifiers = "w412dp-h1800dp-port-mdpi")
    fun the_whole_review_box() {
        compose.setContent {
            Box(Modifier.testTag(PHONE).width(Phone.frameWidth).background(Color(0xFF0B1230)).padding(16.dp)) {
                CompositionLocalProvider(LocalQuestions provides ui.questions) { QuestionBox(vm, "ap-r", null, null) }
            }
        }
        shoot("review-box.png")
    }

    /** A group whose items are all already always on, with its own buttons for all of them (MEM-464). */
    // decision: MEM-464
    @Test
    @Config(qualifiers = "w412dp-h1800dp-port-mdpi")
    fun the_whole_review_box_with_a_group_already_on() {
        phone.memoryReview = ReviewData.setupGroup
        compose.setContent {
            Box(Modifier.testTag(PHONE).width(Phone.frameWidth).background(Color(0xFF0B1230)).padding(16.dp)) {
                CompositionLocalProvider(LocalQuestions provides ui.questions) { QuestionBox(vm, "ap-r", null, null) }
            }
        }
        shoot("review-box-group-kept.png")
    }
}
