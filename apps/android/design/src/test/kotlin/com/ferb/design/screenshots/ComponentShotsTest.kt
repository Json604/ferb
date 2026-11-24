package com.ferb.design.screenshots

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.wrapContentSize
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.Density
import androidx.compose.runtime.CompositionLocalProvider
import com.ferb.design.preview.LabScreen
import com.ferb.design.preview.LabScreenView
import com.ferb.design.preview.SHOWCASES
import com.ferb.design.preview.labHomeTexts
import com.ferb.design.preview.labMoment
import com.ferb.design.sky.SEASON_STILL_T
import com.github.takahirom.roborazzi.captureRoboImage
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.ParameterizedRobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/** Every token page and component showcase, at twice the lab's points (as the lab's own renders are). */
@RunWith(ParameterizedRobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(qualifiers = "w1250dp-h1400dp-land-xhdpi")
class ComponentShotsTest(private val name: String) {
    @get:Rule val compose = createComposeRule()

    // decision: PHN-416, PHN-455, PHN-456, PHN-458, APP-1140, APP-1141
    @Test
    fun component() {
        val show = SHOWCASES.first { it.first == name }.second
        compose.setContent { Box(Modifier.wrapContentSize(Alignment.TopStart).testTag("subject")) { show() } }
        compose.mainClock.autoAdvance = false
        compose.mainClock.advanceTimeByFrame()
        compose.onNodeWithTag("subject").captureRoboImage("$GOLDENS/components/$name.png", roborazziOptions = shotOptions)
    }

    companion object {
        @JvmStatic
        @ParameterizedRobolectricTestRunner.Parameters(name = "{0}")
        fun cases(): List<Array<Any>> = SHOWCASES.map { arrayOf<Any>(it.first) }
    }
}

/** Larger text (the owner's font size at 150 %): the screens with the most words must still hold together. */
@RunWith(ParameterizedRobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(qualifiers = "w412dp-h1000dp-port-mdpi")
class LargeTextShotsTest(private val screen: LabScreen) {
    @get:Rule val compose = createComposeRule()

    @Test
    fun screen() {
        val moment = labMoment(12.5)
        compose.setContent {
            val d = LocalDensity.current
            CompositionLocalProvider(LocalDensity provides Density(d.density, 1.5f)) {
                PhoneSized { LabScreenView(screen, moment, labHomeTexts(moment), seasonTime = SEASON_STILL_T, glide = false) }
            }
        }
        compose.shoot("large-text/${screen.name.lowercase()}.png")
    }

    companion object {
        @JvmStatic
        @ParameterizedRobolectricTestRunner.Parameters(name = "{0}")
        fun cases(): List<Array<Any>> = listOf(LabScreen.HOME, LabScreen.CHAT, LabScreen.QUESTION, LabScreen.TASKS, LabScreen.PAIR, LabScreen.CODES)
            .map { arrayOf<Any>(it) }
    }
}
