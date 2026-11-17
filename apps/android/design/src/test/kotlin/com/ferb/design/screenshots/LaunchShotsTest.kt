package com.ferb.design.screenshots

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.test.junit4.createComposeRule
import com.ferb.design.Launch
import com.ferb.design.LaunchOverlay
import com.ferb.design.LaunchState
import com.ferb.design.LocalLaunch
import com.ferb.design.preview.LabScreen
import com.ferb.design.preview.LabScreenView
import com.ferb.design.preview.labHomeTexts
import com.ferb.design.preview.labMoment
import com.ferb.design.sky.SEASON_STILL_T
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * The launch over the lock page (kb PHN-631, PHN-632): its first frame (the splash's mark at the centre over ink0), a
 * moment of its settle, and the end of the settle, which must rest on the lock page's own golden: the launch's mark
 * lands exactly where the page draws its mark, at its size (the shared element), and ink0 is gone.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(qualifiers = "w412dp-h1000dp-port-mdpi")
class LaunchShotsTest {
    @get:Rule val compose = createComposeRule()

    private fun shootAt(frame: Launch.Frame, path: String) {
        val moment = labMoment(12.5)
        val state = LaunchState(reduced = false)
        compose.setContent {
            PhoneSized {
                CompositionLocalProvider(LocalLaunch provides state) {
                    LabScreenView(LabScreen.UNLOCK, moment, labHomeTexts(moment), seasonTime = SEASON_STILL_T, glide = false)
                    LaunchOverlay(state, ready = true, fixed = frame)
                }
            }
        }
        compose.shoot(path)
    }

    // decision: PHN-632
    @Test
    fun the_first_frame_is_the_mark_on_ink0() = shootAt(Launch.at(0f, null, reduced = false), "launch/first-frame-noon.png")

    // decision: PHN-631
    @Test
    fun a_moment_of_the_settle() = shootAt(Launch.at(200f, 0f, reduced = false), "launch/settling-noon.png")

    /** The last frame before the launch hands over draws the lock page's golden, unchanged. */
    // decision: PHN-631, PHN-632
    @Test
    fun the_settled_frame_rests_on_the_lock_page() = shootAt(Launch.at(Launch.SETTLE_MS - 0.01f, 0f, reduced = false), "screens/unlock-noon.png")
}
