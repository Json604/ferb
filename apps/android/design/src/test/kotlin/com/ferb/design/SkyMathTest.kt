package com.ferb.design

import com.ferb.design.sky.AstraSky
import com.ferb.design.sky.ChosenSky
import com.ferb.design.sky.LabSky
import com.ferb.design.sky.MoonPhase
import com.ferb.design.sky.Season
import com.ferb.design.sky.SkyField
import com.ferb.design.sky.SkyFrameLook
import com.ferb.design.sky.SkyInk
import com.ferb.design.sky.SkyStars
import com.ferb.design.sky.SunDay
import com.ferb.design.sky.SunPosition
import com.ferb.design.sky.seasonRandom
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.time.ZoneId
import kotlin.math.abs

/**
 * The sky's arithmetic against tools/sky_golden.swift's numbers: the glass lab's own formulas (Sky.swift,
 * AstraSky.swift, Weather.swift, Mobile.swift), re-evaluated by Swift for fixed times and places.
 */
class SkyMathTest {
    private val golden = MiniJson.resource("lab/sky-golden.json").obj()

    private fun close(want: Double, got: Double, what: String, tol: Double = 1e-9) {
        if (abs(want - got) > tol * maxOf(1.0, abs(want))) throw AssertionError("$what: want $want, got $got")
    }

    private fun closeAll(want: List<Double>, got: List<Double>, what: String, tol: Double = 1e-9) {
        assertEquals("$what: length", want.size, got.size)
        want.indices.forEach { close(want[it], got[it], "$what[$it]", tol) }
    }

    // decision: PHN-450
    @Test
    fun sunPositionMatchesTheLab() {
        for (c in golden["sun"].list().map { it.obj() }) {
            val s = SunPosition.at(c["t"].num(), c["lat"].num(), c["lon"].num())
            val what = "${c["place"]} at ${c["t"].num().toLong()}"
            // A degree of the sun's height is a large change; 1e-9 of it is far below anything drawn.
            close(c["elevation"].num(), s.elevation, "$what elevation", 1e-8)
            close(c["azimuth"].num(), s.azimuth, "$what azimuth", 1e-8)
            close(c["across"].num(), SunPosition.across(s.azimuth, c["lat"].num()), "$what across", 1e-8)
        }
    }

    // decision: PHN-450
    @Test
    fun sunriseAndSunsetMatchTheLab() {
        for (c in golden["sunDays"].list().map { it.obj() }) {
            val d = SunDay.around(c["t"].num(), c["lat"].num(), c["lon"].num(), ZoneId.of(c["zone"] as String))
            val what = "${c["place"]} around ${c["t"].num().toLong()}"
            val rise = c["sunrise"]
            val set = c["sunset"]
            if (rise == null) assertNull("$what sunrise", d.sunrise) else assertEquals("$what sunrise", rise.num(), d.sunrise!!, 0.0)
            if (set == null) assertNull("$what sunset", d.sunset) else assertEquals("$what sunset", set.num(), d.sunset!!, 0.0)
        }
    }

    // decision: PHN-450
    @Test
    fun moonPhaseMatchesTheLab() {
        for (c in golden["moons"].list().map { it.obj() }) {
            val m = MoonPhase.at(c["t"].num())
            close(c["age"].num(), m.age, "moon age at ${c["t"]}")
            close(c["lit"].num(), m.lit, "moon lit at ${c["t"]}")
            assertEquals(c["waxing"], m.waxing)
        }
    }

    @Test
    fun theLabsOwnSkiesMatch() {
        for (c in golden["labSkies"].list().map { it.obj() }) {
            val sky = LabSky.at(c["elevation"].num(), c["evening"] as Boolean)
            val what = "lab sky ${c["elevation"]} ${c["evening"]}"
            closeAll(c["colors"].list().flatMap { it.nums() }, sky.colors.flatMap { listOf(it.l, it.a, it.b) }, "$what colours")
            closeAll(c["glow"].nums(), listOf(sky.glow.l, sky.glow.a, sky.glow.b), "$what glow")
            closeAll(c["rgbStops"].list().flatMap { it.nums() }, sky.rgbStops.flatMap { listOf(it.first, it.second.first, it.second.second, it.second.third) }, "$what stops")
        }
    }

    private fun fieldNumbers(f: SkyField) = listOf(f.x, f.y, f.rx, f.ry, f.colour.r, f.colour.g, f.colour.b, f.alpha)

    // decision: PHN-450, PHN-416
    @Test
    fun theOwnersChosenSkyMatchesTheLab() {
        for (c in golden["chosen"].list().map { it.obj() }) {
            val look: SkyFrameLook = ChosenSky.look(c["elevation"].num(), c["across"].num())
            val what = "chosen sky e=${c["elevation"]} across=${c["across"]}"
            closeAll(c["colors"].list().flatMap { it.nums() }, look.colors.flatMap { listOf(it.l, it.a, it.b) }, "$what colours")
            closeAll(c["fields"].list().flatMap { it.nums() }, look.fields.flatMap(::fieldNumbers), "$what fields")
            closeAll(c["sunGlow"].nums(), listOf(look.sunGlow.l, look.sunGlow.a, look.sunGlow.b), "$what glow")
            close(c["sunGlowStrength"].num(), look.sunGlowStrength, "$what glow strength")
            close(c["lightTop"].num(), look.lightTop, "$what lightTop")
            close(c["lightLow"].num(), look.lightLow, "$what lightLow")
            val ink = SkyInk.of(look)
            assertEquals("$what top ink", c["inkTopBlack"], ink.top == androidx.compose.ui.graphics.Color.Black)
            assertEquals("$what low ink", c["inkLowBlack"], ink.low == androidx.compose.ui.graphics.Color.Black)
        }
    }

    @Test
    fun astrasClockScheduleMatches() {
        for (c in golden["astraAt"].list().map { it.obj() }) {
            val p = AstraSky.atHour(c["hour"].num())
            val what = "Astra at ${c["hour"]}"
            closeAll(c["colours"].list().flatMap { it.nums() }, p.colours.flatMap { listOf(it.r, it.g, it.b) }, "$what colours")
            closeAll(c["haze"].nums(), fieldNumbers(p.haze), "$what haze")
            closeAll(c["glow"].nums(), fieldNumbers(p.glow), "$what glow")
            closeAll(c["shade"].nums(), fieldNumbers(p.shade), "$what shade")
        }
    }

    // decision: PHN-412
    @Test
    fun seasonsAndTheMonsoonBoxMatch() {
        for (c in golden["seasons"].list().map { it.obj() }) {
            val got = Season.at(c["lat"].num(), c["lon"].num(), c["month"].num().toInt())
            assertEquals("season at ${c["lat"]}, ${c["lon"]} in month ${c["month"]}", c["season"], got.name.lowercase())
        }
    }

    @Test
    fun theSeasonLayersNumbersAndTheStarsMatch() {
        for (r in golden["randoms"].list().map { it.nums() }) close(r[2], seasonRandom(r[0].toInt(), r[1].toInt()), "r(${r[0]}, ${r[1]})", 1e-6)
        val stars = SkyStars.generate()
        val want = golden["stars"].list().map { it.nums() }
        assertEquals(want.size, stars.size)
        want.indices.forEach { closeAll(want[it], stars[it].toList(), "star $it", 0.0) }
    }

    // decision: PHN-457
    @Test
    fun theGoldensComeFromTheLabCommitRecordedInLabNumbers() {
        val numbers = MiniJson.resource("lab/lab-numbers.json").obj()
        assertEquals(numbers["labCommit"], golden["labCommit"])
    }
}
