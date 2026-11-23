package com.ferb.design

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.ferb.design.components.BarFit
import com.ferb.design.components.PhoneTab
import com.ferb.design.components.barLayout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * The top bar's order of giving way (owner 2026-11-23: "pack them in a good looking hamburger icon if theres no space"):
 * normal while every unchosen tab keeps 36; the bubble and the profile button fold into one menu button; then the
 * chosen tab drops its name. The room is the window less its side insets and the bar's 16 margins; the chosen tab's name
 * is measured at the font scale, as the bar measures it.
 */
@RunWith(RobolectricTestRunner::class)
@Config(qualifiers = "w400dp-h800dp-port-mdpi")
class BarLayoutTest {
    @get:Rule val compose = createComposeRule()

    /** "Chat" as the bar measures it (15, semibold) at each font scale (one composition per test). */
    private fun chatNames(vararg fontScales: Float): List<Dp> {
        val out = ArrayList<Dp>()
        compose.setContent {
            for (scale in fontScales) {
                CompositionLocalProvider(LocalDensity provides Density(1f, scale)) {
                    val m = rememberTextMeasurer()
                    out += m.measure(PhoneTab.CHAT.title, FerbType.style(15f, FerbType.Semibold)).size.width.dp
                }
            }
        }
        compose.waitForIdle()
        return out
    }

    private fun chatName(fontScale: Float): Dp = chatNames(fontScale).first()

    /** The bar's room on a window [width] wide with these side insets. */
    private fun room(width: Int, start: Int = 0, end: Int = 0) = (width - start - end - 32).dp

    // decision: PHN-820
    @Test
    fun a_wide_phone_shows_everything_as_before() {
        val name = chatName(1f)
        val l = barLayout(room(412), name, buttons = 2)
        assertEquals(BarFit.NORMAL, l.fit)
        assertTrue(l.showName && !l.menu)
        assertEquals(Phone.tabMinWidth, l.tabWidth)
    }

    // decision: PHN-820
    @Test
    fun a_narrow_phone_folds_the_two_round_buttons_into_the_menu() {
        val name = chatName(1f)
        for (w in listOf(320, 360)) {
            val l = barLayout(room(w), name, buttons = 2)
            assertEquals("at $w", BarFit.FOLDED, l.fit)
            assertTrue(l.menu && l.showName)
            assertTrue(l.tabWidth >= Phone.tabNarrowest)
        }
        // another tab carries only the profile button: at 320 it still fits, nothing folds
        val other = barLayout(room(320), name, buttons = 1)
        assertEquals(BarFit.NORMAL, other.fit)
        assertFalse(other.menu)
    }

    // decision: PHN-820
    @Test
    fun a_big_side_cutout_folds() {
        val name = chatName(1f)
        assertEquals(BarFit.NORMAL, barLayout(room(393), name, 2).fit)
        assertEquals(BarFit.FOLDED, barLayout(room(393, start = 60), name, 2).fit)
    }

    // decision: PHN-820
    @Test
    fun too_tight_even_folded_drops_the_chosen_tabs_name() {
        val name = chatName(1f)
        val l = barLayout(room(300, start = 20), name, buttons = 2)
        assertEquals(BarFit.CONTRACTED, l.fit)
        assertTrue(l.menu && !l.showName)
        // the profile button alone never folds: it goes straight to the icon-only tab
        val alone = barLayout(room(260), name, buttons = 1)
        assertEquals(BarFit.CONTRACTED, alone.fit)
        assertFalse(alone.menu)
    }

    // decision: PHN-820
    @Test
    fun large_text_folds_or_contracts_as_the_measured_name_says() {
        val (plain, big, bigger) = chatNames(1f, 1.6f, 2f)
        assertTrue("the name grows with the font", bigger > big && big > plain)
        assertEquals(BarFit.NORMAL, barLayout(room(412), big, 2).fit)
        assertEquals(BarFit.FOLDED, barLayout(room(375), big, 2).fit)
        assertEquals(BarFit.CONTRACTED, barLayout(room(320), bigger, 2).fit)
    }
}
