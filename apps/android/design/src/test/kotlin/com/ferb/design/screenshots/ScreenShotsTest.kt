package com.ferb.design.screenshots

import androidx.compose.ui.test.junit4.createComposeRule
import com.ferb.design.preview.LabScreen
import com.ferb.design.preview.LabScreenView
import com.ferb.design.preview.labHomeTexts
import com.ferb.design.preview.labMoment
import com.ferb.design.sky.SEASON_STILL_T
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.ParameterizedRobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * The lab's twelve phone screens at dawn, noon and night in Delhi on 2026-11-14, at the lab's Android size
 * (412 by 915 dp), against the committed goldens.
 */
@RunWith(ParameterizedRobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(qualifiers = "w412dp-h1000dp-port-mdpi")
class ScreenShotsTest(private val screen: LabScreen, private val time: String, private val hour: Double) {
    @get:Rule val compose = createComposeRule()

    // decision: PHN-416, PHN-449, PHN-455, PHN-410
    @Test
    fun screen() {
        val moment = labMoment(hour)
        compose.setContent { PhoneSized { LabScreenView(screen, moment, labHomeTexts(moment), seasonTime = SEASON_STILL_T, glide = false) } }
        compose.shoot("screens/${screen.name.lowercase()}-$time.png")
    }

    companion object {
        @JvmStatic
        @ParameterizedRobolectricTestRunner.Parameters(name = "{0} {1}")
        fun cases(): List<Array<Any>> = LabScreen.entries.flatMap { s ->
            listOf("dawn" to 6.1, "noon" to 12.5, "night" to 22.5).map { (name, h) -> arrayOf<Any>(s, name, h) }
        }
    }
}
