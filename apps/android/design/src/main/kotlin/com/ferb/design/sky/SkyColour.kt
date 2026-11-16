package com.ferb.design.sky

import androidx.compose.ui.graphics.Color
import kotlin.math.cbrt
import kotlin.math.floor
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow

// Colour mixed in OKLab so a sky never goes muddy between two others. From Sky.swift (OKLab) and AstraSky.swift
// (SkyRGB). The two are kept apart because they differ: Astra's mix rounds each channel to 8 bits, as its JS did.

/** Sky.swift OKLab. */
data class OkLab(val l: Double, val a: Double, val b: Double) {
    fun mixed(o: OkLab, t: Double) = OkLab(l + (o.l - l) * t, a + (o.a - a) * t, b + (o.b - b) * t)

    /** sRGB, each 0 to 1, clamped (Sky.swift OKLab.rgb). */
    val rgb: Triple<Double, Double, Double>
        get() {
            val lc = (l + 0.3963377774 * a + 0.2158037573 * b).pow(3)
            val mc = (l - 0.1055613458 * a - 0.0638541728 * b).pow(3)
            val sc = (l - 0.0894841775 * a - 1.2914855480 * b).pow(3)
            fun gam(c: Double) = max(0.0, min(1.0, if (c <= 0.0031308) 12.92 * c else 1.055 * c.pow(1 / 2.4) - 0.055))
            return Triple(
                gam(4.0767416621 * lc - 3.3077115913 * mc + 0.2309699292 * sc),
                gam(-1.2684380046 * lc + 2.6097574011 * mc - 0.3413193965 * sc),
                gam(-0.0041960863 * lc - 0.7034186147 * mc + 1.7076147010 * sc),
            )
        }

    fun color(alpha: Double = 1.0): Color = rgb.let { (r, g, bl) -> Color(r.toFloat(), g.toFloat(), bl.toFloat(), alpha.toFloat()) }

    companion object {
        fun hex(hex: Int) = rgb(((hex shr 16) and 0xFF) / 255.0, ((hex shr 8) and 0xFF) / 255.0, (hex and 0xFF) / 255.0)

        /** From sRGB values, 0 to 1. */
        fun rgb(red: Double, green: Double, blue: Double): OkLab {
            fun lin(c: Double) = if (c <= 0.04045) c / 12.92 else ((c + 0.055) / 1.055).pow(2.4)
            val r = lin(red)
            val g = lin(green)
            val bl = lin(blue)
            val lc = cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * bl)
            val mc = cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * bl)
            val sc = cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * bl)
            return OkLab(
                0.2104542553 * lc + 0.7936177850 * mc - 0.0040720468 * sc,
                1.9779984951 * lc - 2.4285922050 * mc + 0.4505937099 * sc,
                0.0259040371 * lc + 0.7827717662 * mc - 0.8086757660 * sc,
            )
        }
    }
}

internal fun skyClamp(v: Double) = min(1.0, max(0.0, v))
internal fun skyLerp(a: Double, b: Double, t: Double) = a + (b - a) * t

/** AstraSky.swift SkyRGB: sRGB 0 to 1; mixing goes through OKLab and rounds to 8 bits. */
data class SkyRgb(val r: Double, val g: Double, val b: Double) {
    fun color(alpha: Double = 1.0) = Color(r.toFloat(), g.toFloat(), b.toFloat(), skyClamp(alpha).toFloat())

    fun mixed(other: SkyRgb, t: Double): SkyRgb {
        val p = lab()
        val q = other.lab()
        return fromLab(skyLerp(p[0], q[0], t), skyLerp(p[1], q[1], t), skyLerp(p[2], q[2], t))
    }

    private fun lab(): DoubleArray {
        fun linear(v: Double) = if (v <= 0.04045) v / 12.92 else ((v + 0.055) / 1.055).pow(2.4)
        val rl = linear(r)
        val gl = linear(g)
        val bl = linear(b)
        val l = cbrt(0.4122214708 * rl + 0.5363325363 * gl + 0.0514459929 * bl)
        val m = cbrt(0.2119034982 * rl + 0.6806995451 * gl + 0.1073969566 * bl)
        val s = cbrt(0.0883024619 * rl + 0.2817188376 * gl + 0.6299787005 * bl)
        return doubleArrayOf(
            0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
            1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
            0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
        )
    }

    companion object {
        fun hex(hex: Int) = SkyRgb(((hex shr 16) and 255) / 255.0, ((hex shr 8) and 255) / 255.0, (hex and 255) / 255.0)

        private fun fromLab(lightness: Double, a: Double, b: Double): SkyRgb {
            val l = (lightness + 0.3963377774 * a + 0.2158037573 * b).pow(3)
            val m = (lightness - 0.1055613458 * a - 0.0638541728 * b).pow(3)
            val s = (lightness - 0.0894841775 * a - 1.291485548 * b).pow(3)
            fun channel(value: Double): Double {
                val v = skyClamp(value)
                val encoded = if (v <= 0.0031308) 12.92 * v else 1.055 * v.pow(1 / 2.4) - 0.055
                // Swift's rounded() is half away from zero; the value is never negative, so floor(x + 0.5).
                return floor(skyClamp(encoded) * 255 + 0.5) / 255
            }
            return SkyRgb(
                channel(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
                channel(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
                channel(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s),
            )
        }
    }
}
