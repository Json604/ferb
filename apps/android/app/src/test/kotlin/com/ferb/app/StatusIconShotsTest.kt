package com.ferb.app

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Battery5Bar
import androidx.compose.material.icons.rounded.Headset
import androidx.compose.material.icons.rounded.Notifications
import androidx.compose.material.icons.rounded.SignalCellularAlt
import androidx.compose.material.icons.rounded.Wifi
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.rememberVectorPainter
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.unit.dp
import com.github.takahirom.roborazzi.captureRoboImage
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * The status bar icon (kb PHN-666) beside system glyphs of the same 24 dp canvas (the cellular bars, wifi, headphones,
 * a typical app's bell, the battery), white on a dark bar and black on a light one, as Android tints it, at the moto
 * g85's density bucket (xxhdpi) and at 1x: the f. must be as big and as heavy as its neighbours.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
class StatusIconShotsTest {
    @get:Rule val compose = createComposeRule()

    @Composable
    private fun Bar(fg: Color, bg: Color) {
        Row(Modifier.background(bg).padding(horizontal = 8.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(4.dp),
            verticalAlignment = Alignment.CenterVertically) {
            val tint = ColorFilter.tint(fg)
            fun icon(v: ImageVector): @Composable () -> Unit = { Image(rememberVectorPainter(v), null, Modifier.size(STATUS_DP.dp), colorFilter = tint) }
            icon(Icons.Rounded.Notifications)()
            Image(painterResource(R.drawable.ic_stat_ferb), null, Modifier.size(STATUS_DP.dp), colorFilter = tint)
            icon(Icons.Rounded.Headset)()
            icon(Icons.Rounded.SignalCellularAlt)()
            icon(Icons.Rounded.Wifi)()
            icon(Icons.Rounded.Battery5Bar)()
        }
    }

    private fun shoot(name: String) {
        compose.setContent {
            Column(Modifier.testTag("bar")) {
                Bar(Color.White, Color.Black)
                Bar(Color(0xFF1F1F1F), Color(0xFFF2F2F2))
            }
        }
        compose.onNodeWithTag("bar").captureRoboImage("src/test/screenshots/status-icon-$name.png", roborazziOptions = ScreenShotsTest.options)
    }

    // decision: PHN-666
    @Test
    @Config(qualifiers = "xxhdpi")
    fun beside_the_system_icons_at_the_phones_density() = shoot("xxhdpi")

    // decision: PHN-666
    @Test
    @Config(qualifiers = "mdpi")
    fun beside_the_system_icons_at_1x() = shoot("mdpi")

    private companion object {
        /** Android draws status bar icons in a box of about 17 dp (status_bar_icon_size); the same for every glyph. */
        const val STATUS_DP = 17
    }
}
