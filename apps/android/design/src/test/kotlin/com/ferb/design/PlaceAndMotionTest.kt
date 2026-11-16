package com.ferb.design

import com.ferb.design.sky.SkyMoment
import com.ferb.design.sky.SkyPlace
import com.ferb.design.sky.Season
import com.ferb.design.sky.hasLayer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.ZoneId
import kotlin.math.PI

class PlaceAndMotionTest {
    /** Without a location the sky uses the time zone's principal place from the tz database, old names included. */
    // decision: PHN-420, PHN-459
    @Test
    fun aTimeZoneGivesACoarsePlace() {
        val kolkata = SkyPlace.fromZone(ZoneId.of("Asia/Kolkata"))
        assertEquals(22.53, kolkata.latitude, 0.01)
        assertEquals(88.37, kolkata.longitude, 0.01)
        assertEquals(kolkata.latitude, SkyPlace.fromZone(ZoneId.of("Asia/Calcutta")).latitude, 0.0)
        val sydney = SkyPlace.fromZone(ZoneId.of("Australia/Sydney"))
        assertTrue(sydney.latitude < -33 && sydney.longitude > 151)
        // A zone with no place (a fixed offset) falls back to its meridian on the equator.
        val fixed = SkyPlace.fromZone(ZoneId.of("Etc/GMT-5"))
        assertEquals(0.0, fixed.latitude, 0.0)
        assertEquals(75.0, fixed.longitude, 0.0)
    }

    /** SwiftUI's spring(duration:bounce:) as Compose's spring: stiffness (2 pi / d)^2, damping 1 - bounce. */
    @Test
    fun springsCarryOverExactly() {
        assertEquals(((2 * PI / 0.2) * (2 * PI / 0.2)).toFloat(), Motion.press.stiffness, 0.01f)
        assertEquals(0.85f, Motion.press.dampingRatio, 1e-6f)
        assertEquals(0.88f, Motion.panel.dampingRatio, 1e-6f)
        assertEquals(0.97f, Motion.PRESS_SCALE)
        assertEquals(2200, Motion.SKY_GLIDE_MS)
    }

    /** No weather: the layer is the season's alone, as the lab draws it under a clear sky. */
    // decision: PHN-412
    @Test
    fun theSeasonLayerDrawsOnlyTheSeasons() {
        assertTrue(hasLayer(Season.AUTUMN, night = false))
        assertTrue(hasLayer(Season.SPRING, night = true))
        assertFalse(hasLayer(Season.SUMMER, night = false))
        assertTrue(hasLayer(Season.MONSOON, night = true))
        assertFalse(hasLayer(Season.WINTER, night = true))
        val delhiNovember = SkyMoment.at(SkyPlace.DELHI, 2026, 11, 14, 12.5)
        assertEquals(Season.AUTUMN, delhiNovember.season)
        assertEquals(Season.SPRING, SkyMoment.at(SkyPlace.SYDNEY, 2026, 11, 14, 12.5).season)
    }
}
