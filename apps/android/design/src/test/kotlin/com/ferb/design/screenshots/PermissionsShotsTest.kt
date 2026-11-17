package com.ferb.design.screenshots

import androidx.compose.ui.test.junit4.createComposeRule
import com.ferb.design.FerbIcons
import com.ferb.design.preview.labMoment
import com.ferb.design.screens.AskRow
import com.ferb.design.screens.PermissionsPage
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.ferb.design.screens.LabAndroidChrome
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.ParameterizedRobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/** The first run's asks (kb PHN-636) over the sky at dawn, noon and night, with the app's words (strings.xml). */
@RunWith(ParameterizedRobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(qualifiers = "w412dp-h1000dp-port-mdpi")
class PermissionsShotsTest(private val time: String, private val hour: Double) {
    @get:Rule val compose = createComposeRule()

    // decision: PHN-636
    @Test
    fun page() {
        compose.setContent {
            PhoneSized {
                PhoneScreen(labMoment(hour), insets = PhoneInsets.Lab, glide = false, sheet = { LabAndroidChrome() }) {
                    PermissionsPage(
                        "Before you start",
                        "FERB asks your phone for a few things. You can say no to any of them: FERB still works, and says what is missing when you need it.",
                        listOf(
                            AskRow(FerbIcons.bell, "Notifications", "So FERB can tell you about its questions and finished tasks when you're outside the app."),
                            AskRow(FerbIcons.location, "Your city", "So local times and places come out right. Only your city goes to your Mac; FERB asks before using your exact spot."),
                            AskRow(FerbIcons.camera, "Camera", "To scan the code on your Mac when you pair. Nothing is recorded."),
                            AskRow(FerbIcons.mic, "Microphone", "So you can hold the microphone and speak to FERB. Nothing is recorded."),
                        ),
                        "Continue", "Not now", PhoneInsets.Lab,
                    )
                }
            }
        }
        compose.shoot("screens/first-run-$time.png")
    }

    companion object {
        @JvmStatic
        @ParameterizedRobolectricTestRunner.Parameters(name = "{0}")
        fun cases(): List<Array<Any>> = listOf(arrayOf<Any>("dawn", 6.1), arrayOf<Any>("noon", 12.5), arrayOf<Any>("night", 22.5))
    }
}
