package com.ferb.design.screenshots

import androidx.compose.ui.test.junit4.ComposeContentTestRule
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.onNodeWithTag
import com.ferb.design.Phone
import com.github.takahirom.roborazzi.RoborazziOptions
import com.github.takahirom.roborazzi.captureRoboImage

/** Where the committed goldens live, by kind. */
const val GOLDENS = "src/test/screenshots"

/**
 * Compared at a 0.1 % changed-pixel threshold: Robolectric's native renderer is deterministic on one machine, and a
 * different host's anti-aliasing stays under it.
 */
val shotOptions = RoborazziOptions(compareOptions = RoborazziOptions.CompareOptions(changeThreshold = 0.001f))

/** The lab's Android screen, 412 by 915 dp (Mobile.swift OS.android), whatever the test window's decor takes. */
@Composable
fun PhoneSized(content: @Composable () -> Unit) {
    Box(Modifier.size(Phone.frameWidth, Phone.frameHeight).testTag(PHONE)) { content() }
}

const val PHONE = "phone"

/** Stops the clock (running marks and other motion would never let Compose go idle) and captures the phone. */
fun ComposeContentTestRule.shoot(path: String) {
    mainClock.autoAdvance = false
    mainClock.advanceTimeByFrame()
    onNodeWithTag(PHONE).captureRoboImage("$GOLDENS/$path", roborazziOptions = shotOptions)
}
