package com.ferb.design.sky

import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min

// The skies themselves: the lab's own (Sky.swift SkyLook), Astra's eight presets (AstraSky.swift FerbSkyPalettes,
// from sky/astra/sky-presets.json version 1) and the owner's picks between them (Sky.swift ChosenSky, from
// sky/choices.json of 2026-11-14), all reduced to one form (Sky.swift SkyFrameLook).

/** Sky.swift SkyLook: four colours from the top to the horizon and the light a low sun throws. */
class LabSky private constructor(val colors: List<OkLab>, val glow: OkLab) {
    /** The nine stops SwiftUI's gradient gets, as (location, sRGB). */
    val rgbStops: List<Pair<Double, Triple<Double, Double, Double>>>
        get() {
            val at = doubleArrayOf(0.0, 0.38, 0.74, 1.0)
            val out = ArrayList<Pair<Double, Triple<Double, Double, Double>>>(10)
            for (k in 0 until 3) for (j in 0 until 3) {
                val t = j / 3.0
                out += (at[k] + (at[k + 1] - at[k]) * t) to colors[k].mixed(colors[k + 1], t).rgb
            }
            out += 1.0 to colors[3].rgb
            return out
        }

    companion object {
        /** Sky.swift SkyLook.skies: name to (top ... horizon, glow). */
        val SKIES: Map<String, Pair<IntArray, Int>> = mapOf(
            "night" to (intArrayOf(0x02040A, 0x060A18, 0x0B1226, 0x121A33) to 0x000000),
            "astro" to (intArrayOf(0x040917, 0x0A1331, 0x18204A, 0x262A57) to 0x2A1E4A),
            "blueM" to (intArrayOf(0x081332, 0x18295E, 0x3C4E8E, 0x7C7CAE) to 0x8E7FB8),
            "blueE" to (intArrayOf(0x0A1131, 0x1E2560, 0x4D3F88, 0x9A6A9E) to 0xB0628E),
            "dawn" to (intArrayOf(0x0F2250, 0x2C4787, 0x8492C0, 0xEBC3B6) to 0xF6C6B4),
            "dusk" to (intArrayOf(0x14204F, 0x3B4489, 0xA07AAE, 0xF2AE8E) to 0xF79A72),
            "riseM" to (intArrayOf(0x1B3B78, 0x4C70AE, 0xAFBBD8, 0xF7D9C2) to 0xFBD7BC),
            "riseE" to (intArrayOf(0x1C3674, 0x55639F, 0xC49BB5, 0xF6BE95) to 0xFFB37A),
            "goldM" to (intArrayOf(0x21529C, 0x5A88C6, 0xB9CDE6, 0xF3E2CF) to 0xFBE3C8),
            "goldE" to (intArrayOf(0x204E96, 0x5B7FBE, 0xBDB7D6, 0xF4D3AE) to 0xFFD19A),
            "day" to (intArrayOf(0x1D5DC6, 0x4A8BE1, 0x9CC2F1, 0xE2ECFB) to 0xFFFFFF),
            "high" to (intArrayOf(0x1957C6, 0x3E84E4, 0x8DB9F2, 0xDAE7FB) to 0xFFFFFF),
        )

        /** Sky.swift SkyLook.init(elevation:evening:): by the sun's height, with a smooth step between named skies. */
        fun at(elevation: Double, evening: Boolean): LabSky {
            val stops = listOf(
                -90.0 to "night", -18.0 to "night", -12.0 to "astro", -6.0 to (if (evening) "blueE" else "blueM"),
                -2.5 to (if (evening) "dusk" else "dawn"), 1.0 to (if (evening) "riseE" else "riseM"),
                7.0 to (if (evening) "goldE" else "goldM"), 20.0 to "day", 90.0 to "high",
            )
            var i = 0
            while (i < stops.size - 2 && elevation > stops[i + 1].first) i++
            val (ea, a) = stops[i]
            val (eb, b) = stops[i + 1]
            val raw = max(0.0, min(1.0, (elevation - ea) / (eb - ea)))
            val t = raw * raw * (3 - 2 * raw)
            val sa = SKIES.getValue(a)
            val sb = SKIES.getValue(b)
            return LabSky(
                (0 until 4).map { OkLab.hex(sa.first[it]).mixed(OkLab.hex(sb.first[it]), t) },
                OkLab.hex(sa.second).mixed(OkLab.hex(sb.second), t),
            )
        }
    }
}

/** AstraSky.swift SkyField: one soft elliptical field, its centre and radii as fractions of the screen. */
data class SkyField(val x: Double, val y: Double, val rx: Double, val ry: Double, val colour: SkyRgb, val alpha: Double) {
    fun mixed(other: SkyField, t: Double) = SkyField(
        skyLerp(x, other.x, t), skyLerp(y, other.y, t), skyLerp(rx, other.rx, t), skyLerp(ry, other.ry, t),
        colour.mixed(other.colour, t), skyLerp(alpha, other.alpha, t),
    )
}

/** AstraSky.swift SkyPalette: seven colours down the screen and three fields. */
data class SkyPalette(val colours: List<SkyRgb>, val haze: SkyField, val glow: SkyField, val shade: SkyField) {
    fun mixed(other: SkyPalette, t: Double) = SkyPalette(
        colours.zip(other.colours) { a, b -> a.mixed(b, t) },
        haze.mixed(other.haze, t), glow.mixed(other.glow, t), shade.mixed(other.shade, t),
    )
}

/** Astra's eight skies (AstraSky.swift FerbSkyPhase without `now`). */
enum class AstraPhase(val key: String, val label: String) {
    DAWN("dawn", "Dawn"), SUNRISE("sunrise", "Sunrise"), MORNING("morning", "Morning"), NOON("noon", "Noon"),
    GOLDEN_HOUR("golden-hour", "Golden hour"), SUNSET("sunset", "Sunset"), DUSK("dusk", "Dusk"), NIGHT("night", "Night"),
}

/** AstraSky.swift FerbSkyPalettes. */
object AstraSky {
    val positions = doubleArrayOf(0.0, 0.24, 0.48, 0.67, 0.82, 0.93, 1.0)

    private fun f(x: Double, y: Double, rx: Double, ry: Double, hex: Int, alpha: Double) = SkyField(x, y, rx, ry, SkyRgb.hex(hex), alpha)
    private fun p(c: IntArray, haze: SkyField, glow: SkyField, shade: SkyField) = SkyPalette(c.map(SkyRgb::hex), haze, glow, shade)

    /** The frames with their illustrative clock hour (not solar events), in order. */
    val frames: List<Triple<AstraPhase, Double, SkyPalette>> = listOf(
        Triple(AstraPhase.DAWN, 5.4, p(intArrayOf(0x304359, 0x536A80, 0x8996A5, 0xB9ADB6, 0xDAB6B7, 0xEAC5B8, 0xEED1BC),
            f(0.18, 0.59, 1.0, 0.34, 0x99BBC7, 0.13), f(0.73, 1.04, 0.85, 0.56, 0xFFE2BF, 0.32), f(0.05, 0.06, 1.1, 0.98, 0x15233D, 0.15))),
        Triple(AstraPhase.SUNRISE, 6.5, p(intArrayOf(0x384F70, 0x5F7D9B, 0x9BAEBB, 0xC8C4C0, 0xE6C5AD, 0xF2C590, 0xF6D39A),
            f(0.12, 0.72, 0.95, 0.28, 0xE2AC96, 0.14), f(0.78, 0.97, 0.58, 0.47, 0xFFF0C5, 0.72), f(0.02, 0.04, 0.92, 0.92, 0x203855, 0.12))),
        Triple(AstraPhase.MORNING, 9.0, p(intArrayOf(0x397CB4, 0x5C9AC8, 0x8AB7D7, 0xB5D2E2, 0xD3E0E5, 0xE4E8E4, 0xEEECE0),
            f(0.22, 0.84, 1.1, 0.32, 0xE2F0EF, 0.14), f(0.93, 0.62, 0.88, 0.85, 0xFFF8DF, 0.39), f(0.05, 0.05, 1.05, 0.95, 0x23578B, 0.06))),
        Triple(AstraPhase.NOON, 12.5, p(intArrayOf(0x1765A4, 0x267DB8, 0x4398C8, 0x72B6D8, 0xA0CEDF, 0xC3E0E7, 0xD6E9EA),
            f(0.45, 1.06, 1.3, 0.4, 0xE6F8F4, 0.16), f(0.85, -0.16, 0.86, 1.05, 0xEAF9FF, 0.29), f(0.03, 0.03, 0.85, 1.0, 0x064078, 0.08))),
        Triple(AstraPhase.GOLDEN_HOUR, 16.7, p(intArrayOf(0x5C788F, 0x8A9FA9, 0xB6BAB5, 0xD8C8B0, 0xEAD0A7, 0xF1CF96, 0xEFC388),
            f(0.12, 0.82, 0.94, 0.34, 0xD89B7E, 0.2), f(0.88, 0.88, 0.7, 0.67, 0xFFE9B1, 0.68), f(0.02, 0.03, 0.98, 0.93, 0x364A65, 0.12))),
        Triple(AstraPhase.SUNSET, 18.1, p(intArrayOf(0x3D496B, 0x626484, 0x92839A, 0xBE969F, 0xDEA693, 0xEDA87B, 0xEFB477),
            f(0.85, 0.74, 0.88, 0.29, 0xD98B98, 0.2), f(0.2, 1.02, 0.68, 0.49, 0xFFD89B, 0.65), f(0.95, 0.01, 1.05, 0.94, 0x263653, 0.17))),
        Triple(AstraPhase.DUSK, 18.85, p(intArrayOf(0x0B2137, 0x1C3B53, 0x3D5D76, 0x687D8D, 0x96959D, 0xBF9B93, 0xD5A68E),
            f(0.74, 0.68, 1.05, 0.3, 0x7896B0, 0.1), f(0.15, 1.16, 0.82, 0.43, 0xF7BF96, 0.41), f(0.92, 0.04, 1.05, 1.08, 0x081B31, 0.18))),
        Triple(AstraPhase.NIGHT, 22.0, p(intArrayOf(0x070D19, 0x0C1728, 0x142438, 0x1C3044, 0x263B4D, 0x2D4454, 0x344B58),
            f(0.3, 1.06, 1.2, 0.41, 0x68899B, 0.07), f(0.79, 0.2, 0.61, 0.7, 0x839EB9, 0.1), f(0.02, 0.03, 1.0, 0.98, 0x020811, 0.23))),
    )

    fun fixed(phase: AstraPhase): SkyPalette = frames.first { it.first == phase }.third

    /** FerbSkyPalettes.at: Astra's own clock schedule (art direction, not the sun). Kept for the gallery's comparison. */
    fun atHour(hour: Double): SkyPalette {
        val night = frames[7].third
        val anchors = listOf(0.0 to night, 4.1 to night) + frames.map { it.second to it.third } + listOf(24.0 to night)
        for (index in 0 until anchors.size - 1) {
            val (ha, pa) = anchors[index]
            val (hb, pb) = anchors[index + 1]
            if (hour >= ha && hour <= hb) {
                val t = skyClamp((hour - ha) / (hb - ha))
                return pa.mixed(pb, t * t * (3 - 2 * t))
            }
        }
        return night
    }
}

/**
 * Sky.swift SkyFrameLook: one sky in a form both kinds share: 25 colours down the screen, Astra's soft fields, and
 * the lab's glow on the sun's side.
 */
class SkyFrameLook(val colors: List<OkLab>, val fields: List<SkyField>, val sunGlow: OkLab, val sunGlowStrength: Double) {
    /** How light the top and the middle are (OKLab L): what decides black or white text over them. */
    val lightTop: Double get() = colors.subList(0, 7).sumOf { it.l } / 7
    val lightLow: Double get() = colors.subList(7, 16).sumOf { it.l } / 9

    fun mixed(o: SkyFrameLook, t: Double): SkyFrameLook {
        // A sky without fields borrows the other's shapes at no strength, so they fade in rather than jump.
        val mine = if (fields.isEmpty()) o.fields.map { it.copy(alpha = 0.0) } else fields
        val theirs = if (o.fields.isEmpty()) fields.map { it.copy(alpha = 0.0) } else o.fields
        return SkyFrameLook(
            colors.zip(o.colors) { a, b -> a.mixed(b, t) },
            mine.zip(theirs) { a, b -> a.mixed(b, t) },
            sunGlow.mixed(o.sunGlow, t),
            sunGlowStrength + (o.sunGlowStrength - sunGlowStrength) * t,
        )
    }

    companion object {
        val GRID: List<Double> = (0..24).map { it / 24.0 }

        /** The lab's sky: its stops sampled the way SwiftUI blends between them (in sRGB). */
        fun of(look: LabSky, elevation: Double): SkyFrameLook = SkyFrameLook(
            GRID.map { sample(look.rgbStops, it) }, emptyList(), look.glow, max(0.0, 0.8 - abs(elevation - 1) / 14),
        )

        /** Astra's sky: its seven stops sampled the same way, and its fields. */
        fun of(p: SkyPalette): SkyFrameLook {
            val stops = AstraSky.positions.toList().zip(p.colours) { pos, c -> pos to Triple(c.r, c.g, c.b) }
            return SkyFrameLook(GRID.map { sample(stops, it) }, listOf(p.haze, p.glow, p.shade), OkLab.hex(0xFFFFFF), 0.0)
        }

        private fun sample(stops: List<Pair<Double, Triple<Double, Double, Double>>>, x: Double): OkLab {
            var i = 0
            while (i < stops.size - 2 && x > stops[i + 1].first) i++
            val (xa, a) = stops[i]
            val (xb, b) = stops[i + 1]
            val t = max(0.0, min(1.0, (x - xa) / max(0.0001, xb - xa)))
            return OkLab.rgb(a.first + (b.first - a.first) * t, a.second + (b.second - a.second) * t, a.third + (b.third - a.third) * t)
        }
    }
}

/**
 * Sky.swift ChosenSky: the owner picked, for each time of day, the lab's sky or Astra's (sky/choices.json,
 * 2026-11-14). The sky follows the real sun through those picks by its height, rising on one side of noon and
 * setting on the other; picks of different origin blend in OKLab with a smoother step.
 */
object ChosenSky {
    private sealed interface Pick {
        data class Mine(val elevation: Double) : Pick
        data class Astra(val phase: AstraPhase) : Pick
    }

    /** Morning: night, dawn, sunrise, morning, noon. */
    private val rising: List<Pair<Double, Pick>> = listOf(
        -90.0 to Pick.Mine(-90.0), -14.0 to Pick.Mine(-14.0), -4.5 to Pick.Mine(-4.5), 0.5 to Pick.Astra(AstraPhase.SUNRISE),
        15.0 to Pick.Mine(15.0), 40.0 to Pick.Astra(AstraPhase.NOON), 90.0 to Pick.Astra(AstraPhase.NOON),
    )

    /** Evening: noon, golden hour, sunset, dusk, night. */
    private val setting: List<Pair<Double, Pick>> = listOf(
        -90.0 to Pick.Mine(-90.0), -14.0 to Pick.Mine(-14.0), -5.0 to Pick.Astra(AstraPhase.DUSK), 0.0 to Pick.Astra(AstraPhase.SUNSET),
        7.0 to Pick.Astra(AstraPhase.GOLDEN_HOUR), 40.0 to Pick.Astra(AstraPhase.NOON), 90.0 to Pick.Astra(AstraPhase.NOON),
    )

    /** [across] below zero: rising; above: setting. Around midday the two sides are blended so there is no seam. */
    fun look(elevation: Double, across: Double): SkyFrameLook {
        val w = smoother((across + 0.14) / 0.28)
        if (w <= 0) return side(elevation, evening = false)
        if (w >= 1) return side(elevation, evening = true)
        return side(elevation, evening = false).mixed(side(elevation, evening = true), w)
    }

    /** A smoother step: no jolt in the speed of change at either end. */
    fun smoother(x: Double): Double {
        val t = max(0.0, min(1.0, x))
        return t * t * t * (t * (t * 6 - 15) + 10)
    }

    private fun side(e: Double, evening: Boolean): SkyFrameLook {
        val anchors = if (evening) setting else rising
        var i = 0
        while (i < anchors.size - 2 && e > anchors[i + 1].first) i++
        val (ea, a) = anchors[i]
        val (eb, b) = anchors[i + 1]
        if (a is Pick.Mine && b is Pick.Mine) return SkyFrameLook.of(LabSky.at(e, evening), e)
        return frame(a, evening).mixed(frame(b, evening), smoother((e - ea) / (eb - ea)))
    }

    private fun frame(pick: Pick, evening: Boolean): SkyFrameLook = when (pick) {
        is Pick.Mine -> SkyFrameLook.of(LabSky.at(pick.elevation, evening), pick.elevation)
        is Pick.Astra -> SkyFrameLook.of(AstraSky.fixed(pick.phase))
    }
}
