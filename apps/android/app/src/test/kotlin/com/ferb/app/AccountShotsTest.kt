package com.ferb.app

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.AccountUi
import com.ferb.app.model.LocalFiles
import com.ferb.app.ui.MacFirst
import com.ferb.app.ui.Pair
import com.ferb.app.ui.SignIn
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
 * FERB's account pages (kb PHN-839, PHN-841) over the noon sky, at the smallest common Android width (360 dp) and the
 * lab's (412 dp): the sign-in page (with a line after a cancelled try) and the phone-first page; at 412 dp also the
 * phone-first page signing out, the sign-in page after a sign-out the site didn't hear, and the Mac's refusal of this
 * phone's account with another account under it (kb PHN-842).
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
class AccountShotsTest {
    @get:Rule val compose = createComposeRule()

    private val vm by lazy {
        FerbViewModel(FakePhone(), Session { 0L }, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(),
            LocalFiles(java.nio.file.Files.createTempDirectory("ferb-account-shots").toFile()) { null }, account = signedOutAccount())
    }

    private fun shoot(name: String, page: @androidx.compose.runtime.Composable () -> Unit) {
        compose.setContent {
            Box(Modifier.fillMaxSize().testTag(ScreenShotsTest.PHONE)) {
                PhoneScreen(labMoment(12.5), insets = PhoneInsets.Lab, glide = false) { page() }
            }
        }
        compose.mainClock.autoAdvance = false
        repeat(40) { compose.mainClock.advanceTimeByFrame() }
        compose.onNodeWithTag(ScreenShotsTest.PHONE).captureRoboImage("src/test/screenshots/account/$name", roborazziOptions = ScreenShotsTest.options)
    }

    private val signedOut = UiState(gate = Gate.SIGN_IN, account = AccountUi(known = true, note = Note(R.string.signin_cancelled)))
    private val macFirst = UiState(gate = Gate.PAIR, account = AccountUi(known = true, account = INVENTED_ACCOUNT))
    private val signingOut = macFirst.copy(busy = setOf("account"))
    private val signedOutUnheard = UiState(gate = Gate.SIGN_IN, account = AccountUi(known = true, note = Note(R.string.signout_unreachable)))
    private val refusal = "This phone is signed in as someone@example.com, but FERB on this Mac is signed in as other@example.com. Sign in with the same account on both."

    // decision: PHN-839, PHN-416, APP-1140
    @Test @Config(qualifiers = "w360dp-h780dp-port-mdpi") fun sign_in_at_360() = shoot("sign-in-360.png") { SignIn(vm, signedOut, PhoneInsets.Lab) }

    // decision: PHN-839, PHN-416, APP-1140
    @Test @Config(qualifiers = "w412dp-h915dp-port-mdpi") fun sign_in_at_412() = shoot("sign-in-412.png") { SignIn(vm, signedOut, PhoneInsets.Lab) }

    // decision: PHN-841, PHN-416
    @Test @Config(qualifiers = "w360dp-h780dp-port-mdpi") fun phone_first_at_360() = shoot("phone-first-360.png") { MacFirst(vm, macFirst, PhoneInsets.Lab) }

    // decision: PHN-841, PHN-416
    @Test @Config(qualifiers = "w412dp-h915dp-port-mdpi") fun phone_first_at_412() = shoot("phone-first-412.png") { MacFirst(vm, macFirst, PhoneInsets.Lab) }

    // decision: PHN-842, PHN-416
    @Test @Config(qualifiers = "w412dp-h915dp-port-mdpi") fun phone_first_signing_out_at_412() =
        shoot("phone-first-signing-out-412.png") { MacFirst(vm, signingOut, PhoneInsets.Lab) }

    // decision: PHN-843, PHN-416
    @Test @Config(qualifiers = "w412dp-h915dp-port-mdpi") fun sign_in_after_an_unheard_sign_out_at_412() =
        shoot("sign-in-signed-out-unheard-412.png") { SignIn(vm, signedOutUnheard, PhoneInsets.Lab) }

    // decision: PHN-842, PHN-828, PHN-416
    @Test @Config(qualifiers = "w412dp-h915dp-port-mdpi") fun pair_refused_over_the_account_at_412() =
        shoot("pair-account-refused-412.png") { Pair(vm, false, null, refusal, PhoneInsets.Lab, other = true) }
}
