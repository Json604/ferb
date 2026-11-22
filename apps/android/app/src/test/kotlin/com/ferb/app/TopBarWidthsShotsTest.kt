package com.ferb.app

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.unit.dp
import com.ferb.design.components.Connection
import com.ferb.design.components.LinkMark
import com.ferb.design.components.LocalConnection
import com.ferb.design.components.PhoneTab
import com.ferb.design.components.TopBar
import com.ferb.design.preview.labMoment
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.github.takahirom.roborazzi.captureRoboImage
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * The top bar (PHN-801) with every tab chosen in turn, at the smallest common Android width (360 dp) and the lab's
 * (412 dp): the chosen tab's name fits, the other icons keep their room, the link's state is the profile button's badge.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
class TopBarWidthsShotsTest {
    @get:Rule val compose = createComposeRule()

    private fun shoot(name: String) {
        compose.setContent {
            Box(Modifier.fillMaxSize().testTag("phone")) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false) {
                    CompositionLocalProvider(LocalConnection provides Connection(LinkMark.DIRECT, "Your Mac: Connected", {})) {
                        Column(Modifier.padding(top = 20.dp), verticalArrangement = Arrangement.spacedBy(18.dp)) {
                            for (t in PhoneTab.entries) TopBar(t, waiting = if (t == PhoneTab.TASKS) null else PhoneTab.TASKS, onTab = {}, onProfile = {})
                        }
                    }
                }
            }
        }
        compose.mainClock.autoAdvance = false
        repeat(40) { compose.mainClock.advanceTimeByFrame() }
        compose.onNodeWithTag("phone").captureRoboImage("src/test/screenshots/$name", roborazziOptions = ScreenShotsTest.options)
    }

    // decision: PHN-801
    @Test @Config(qualifiers = "w360dp-h560dp-port-mdpi") fun every_tab_at_360() = shoot("topbar-360.png")

    // decision: PHN-801
    @Test @Config(qualifiers = "w412dp-h560dp-port-mdpi") fun every_tab_at_412() = shoot("topbar-412.png")
}
