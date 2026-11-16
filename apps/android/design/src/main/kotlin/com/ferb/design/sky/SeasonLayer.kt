package com.ferb.design.sky

import android.os.Build
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableDoubleStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlurEffect
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.TileMode
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.clearAndSetSemantics
import com.ferb.design.LabColors
import com.ferb.design.rememberReducedMotion
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.cos
import kotlin.math.max
import kotlin.math.pow
import kotlin.math.sin

// The season's light layer over the home screen (Weather.swift WeatherLayer with the weather clear: PHN-412 pauses
// every other weather). Autumn drops leaves, spring petals, a summer or monsoon night has fireflies; winter and a
// summer day have nothing. Slow and sparse, drawn at three depths as a camera sees them, and still when the owner
// turned animations off (the lab draws a still frame at t = 1000 then).

/** The lab's still moment, used when motion is off and for screenshots. */
const val SEASON_STILL_T = 1000.0

/**
 * The season's layer. [time] fixes the moment in seconds (screenshots, previews); null runs it on the frame clock
 * unless animations are off.
 */
@Composable
fun SeasonLayer(season: Season, night: Boolean, modifier: Modifier = Modifier, time: Double? = null) {
    if (!hasLayer(season, night)) return
    val reduced = rememberReducedMotion()
    var t by remember { mutableDoubleStateOf(time ?: SEASON_STILL_T) }
    if (time == null && !reduced) {
        LaunchedEffect(Unit) {
            val start = withFrameNanos { it }
            while (true) withFrameNanos { now -> t = SEASON_STILL_T + (now - start) / 1e9 }
        }
    } else {
        t = time ?: SEASON_STILL_T
    }
    Box(modifier.clearAndSetSemantics { }) {
        for (depth in Depth.entries) {
            Canvas(
                Modifier.fillMaxSize().graphicsLayer {
                    alpha = depth.alpha
                    // Only the far layer is softened, and barely; blur needs Android 12, below it the far layer is sharp.
                    if (depth.blur > 0 && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                        renderEffect = BlurEffect(depth.blur * density, depth.blur * density, TileMode.Decal)
                    }
                },
            ) { drawSeason(season, night, depth, t) }
        }
    }
}

/** Whether the season draws anything (Weather.swift seasonal()'s cases). */
fun hasLayer(season: Season, night: Boolean): Boolean = when (season) {
    Season.AUTUMN, Season.SPRING -> true
    Season.SUMMER, Season.MONSOON -> night
    Season.WINTER -> false
}

/** Weather.swift Depth: far (small, slow, a little soft), the plane in focus, and near (large, fast). */
internal enum class Depth(val scale: Double, val blur: Float, val alpha: Float) {
    FAR(0.5, 0.6f, 0.65f), MID(0.85, 0f, 1f), NEAR(1.35, 0f, 0.8f);
}

private fun r(i: Int, k: Int) = seasonRandom(i, k)

private enum class Shape { OVAL, MAPLE, PETAL }

internal fun DrawScope.drawSeason(season: Season, night: Boolean, depth: Depth, t: Double) {
    val w = size.width / density.toDouble()
    val h = size.height / density.toDouble()
    val k = depth.ordinal
    // The lab works in points; draw in points and scale once.
    val px = density
    withTransform({ scale(px, px, pivot = Offset.Zero) }) {
        when (season) {
            Season.AUTUMN -> for (j in 0 until intArrayOf(5, 6, 1)[k]) {
                val i = j + k * 50
                fall(i, w, h, t, 1.0, depth, if (i % 3 == 0) Shape.MAPLE else Shape.OVAL,
                    LabColors.autumnLeaves[i % LabColors.autumnLeaves.size], 26 + 8 * r(i, 3))
            }
            Season.SPRING -> for (j in 0 until intArrayOf(7, 8, 1)[k]) {
                val i = j + k * 50
                fall(i, w, h, t, 1.0, depth, Shape.PETAL, LabColors.springPetal, 15 + 4 * r(i, 3))
            }
            Season.SUMMER, Season.MONSOON -> if (night) for (j in 0 until intArrayOf(7, 6, 2)[k]) firefly(j + k * 50, w, h, t, depth.scale)
            Season.WINTER -> Unit
        }
    }
}

/**
 * Weather.swift fall(): a leaf swings like a pendulum, gliding down fastest through the middle of each swing, tilting
 * into it, and turning over now and then.
 */
private fun DrawScope.fall(
    i: Int, w: Double, h: Double, t: Double, gust: Double, d: Depth, kind: Shape, tone: Triple<Color, Color, Color>, size: Double,
) {
    val petal = kind == Shape.PETAL
    val period = (if (petal) 3.4 else 2.9) + 1.4 * r(i, 11)
    val s = t / period * 2 * PI + r(i, 12) * 6
    val amp = (if (petal) 46.0 else 38.0) * d.scale * (0.7 + 0.6 * r(i, 13))
    val v = (if (petal) 22.0 else 30.0) * d.scale * (0.8 + 0.4 * r(i, 2))
    val span = h + 120
    val yRaw = r(i, 4) * span + v * t + v * period / (2 * PI) * 0.45 * sin(2 * s)
    val y = yRaw % span - 60
    val xRaw = r(i, 1) * w + amp * sin(s) + t * (if (petal) 9.0 else 3.0) * gust * d.scale
    val x = ((xRaw % (w + 80)) + w + 80) % (w + 80) - 40
    val tilt = 0.75 * cos(s) + r(i, 7) * 6.28 + sin(t * 0.13 + r(i, 14) * 6) * 0.6
    val turn = cos(t * (0.55 + 0.5 * r(i, 8)) + r(i, 9) * 6)
    val lit = if (turn > 0) 1.0 else 0.78
    val sz = size * d.scale
    withTransform({
        translate(x.toFloat(), y.toFloat())
        rotate((tilt * 180 / PI).toFloat(), pivot = Offset.Zero)
        scale(1f, (0.55 + 0.45 * abs(turn)).toFloat(), pivot = Offset.Zero)
    }) {
        when (kind) {
            Shape.OVAL -> ovalLeaf(sz, tone, lit, i)
            Shape.MAPLE -> mapleLeaf(sz, tone, lit, i)
            Shape.PETAL -> blossom(sz, tone, lit)
        }
    }
}

private fun pt(x: Double, y: Double) = Offset(x.toFloat(), y.toFloat())
private fun Path.moveTo(p: Offset) = moveTo(p.x, p.y)
private fun Path.lineTo(p: Offset) = lineTo(p.x, p.y)
private fun Path.quadTo(to: Offset, control: Offset) = quadraticTo(control.x, control.y, to.x, to.y)
private fun Path.curveTo(to: Offset, c1: Offset, c2: Offset) = cubicTo(c1.x, c1.y, c2.x, c2.y, to.x, to.y)

/** Weather.swift ovalLeaf: rounded base, widest a third up, a soft point, toothed, bent along its midrib, a stalk. */
private fun DrawScope.ovalLeaf(len: Double, tone: Triple<Color, Color, Color>, lit: Double, seed: Int) {
    val w = len * (0.3 + 0.06 * r(seed, 15))
    val bend = len * 0.06 * (r(seed, 16) - 0.5) * 2
    val n = 40
    fun k(u: Double) = (u * n).toInt()
    fun edge(u: Double, side: Double): Offset {
        val profile = sin(PI * u.pow(0.62)).pow(0.9) * (1 - 0.18 * u)
        val half = w * profile * (if (side > 0) 1.0 else 0.9)
        val tooth = 1 + 0.035 * (if (k(u) % 2 == 0) 1 else -1) * minOf(1.0, u * 3) * (1 - u)
        return pt(-len / 2 + len * u, side * half * tooth + bend * sin(PI * u))
    }
    val leaf = Path().apply {
        moveTo(pt(-len / 2, 0.0))
        for (i in 1..n) lineTo(edge(i / n.toDouble(), -1.0))
        for (i in n - 1 downTo 0) lineTo(edge(i / n.toDouble(), 1.0))
        close()
    }
    val stalk = Path().apply {
        moveTo(pt(-len / 2 + 1, -1.1))
        quadTo(pt(-len * 0.64, bend * 0.6), pt(-len * 0.58, -0.6))
        quadTo(pt(-len / 2 + 1, 1.1), pt(-len * 0.58, 1.4))
        close()
    }
    drawPath(stalk, tone.third)
    paintLeaf(leaf, tone, lit, len, seed)
    val veins = Path().apply {
        moveTo(pt(-len / 2, 0.0))
        quadTo(pt(len * 0.46, bend * 0.2), pt(0.0, bend * 1.2))
        for (f in doubleArrayOf(-0.32, -0.16, 0.0, 0.16, 0.3)) for (side in doubleArrayOf(-1.0, 1.0)) {
            val from = pt(len * f, bend * sin(PI * (f + 0.5)))
            moveTo(from)
            quadTo(pt(len * (f + 0.14), from.y + side * w * 0.75), pt(len * (f + 0.03), from.y + side * w * 0.45))
        }
    }
    clipPath(leaf) { drawPath(veins, tone.first.copy(alpha = (0.5 * lit).toFloat()), style = Stroke(0.55f, cap = StrokeCap.Round)) }
}

/** Weather.swift mapleLeaf: five toothed lobes, veins from the stem to each lobe. */
private fun DrawScope.mapleLeaf(s: Double, tone: Triple<Color, Color, Color>, lit: Double, seed: Int) {
    val rr = s * 0.55
    val lobes = listOf(0.0 to 1.0, 58.0 to 0.92, -58.0 to 0.92, 112.0 to 0.66, -112.0 to 0.66)
    val n = 90
    val leaf = Path()
    for (k in 0 until n) {
        val deg = -180 + 360.0 * k / n
        val p = if (abs(deg) > 168) pt(0.0, rr * 0.12) else {
            var reach = 0.46
            for ((c, l) in lobes) reach = max(reach, l * max(0.0, 1 - (abs(deg - c) / 38).pow(2.2)))
            reach *= 1 + 0.06 * (if (k % 2 == 0) 1 else -1)
            val a = (deg - 90) * PI / 180
            pt(cos(a) * rr * reach, sin(a) * rr * reach)
        }
        if (k == 0) leaf.moveTo(p) else leaf.lineTo(p)
    }
    leaf.close()
    paintLeaf(leaf, tone, lit, s, seed)
    val veins = Path().apply {
        for ((deg, l) in lobes) {
            val a = (deg - 90) * PI / 180
            moveTo(pt(0.0, rr * 0.1))
            lineTo(pt(cos(a) * rr * l * 0.85, sin(a) * rr * l * 0.85))
        }
    }
    clipPath(leaf) { drawPath(veins, tone.first.copy(alpha = (0.55 * lit).toFloat()), style = Stroke(0.6f, cap = StrokeCap.Round)) }
    val stalk = Path().apply {
        moveTo(pt(-1.1, rr * 0.08))
        quadTo(pt(rr * 0.05, rr * 0.42), pt(-0.6, rr * 0.3))
        quadTo(pt(1.1, rr * 0.08), pt(1.4, rr * 0.3))
        close()
    }
    drawPath(stalk, tone.third)
}

/** Weather.swift paintLeaf: a light-to-dark fade across it, two soft patches, a browned rim, the underside duller. */
private fun DrawScope.paintLeaf(leaf: Path, tone: Triple<Color, Color, Color>, lit: Double, span: Double, seed: Int) {
    val (light, mid, dark) = tone
    drawPath(leaf, Brush.linearGradient(listOf(light, mid, dark), start = pt(-span / 2, -span / 3), end = pt(span / 2, span / 3)))
    clipPath(leaf) {
        for (k in 0 until 2) {
            val c = pt((r(seed, 40 + k) - 0.5) * span * 0.7, (r(seed, 42 + k) - 0.5) * span * 0.3)
            val rad = (span * (0.12 + 0.12 * r(seed, 44 + k))).toFloat()
            val col = if (k == 0) dark else light
            drawOval(
                Brush.radialGradient(listOf(col.copy(alpha = 0.45f), Color.Transparent), center = c, radius = rad),
                topLeft = Offset(c.x - rad, c.y - rad), size = Size(rad * 2, rad * 2),
            )
        }
        drawPath(leaf, dark.copy(alpha = 0.6f), style = Stroke((span * 0.06).toFloat()))
        if (lit < 1) drawPath(leaf, Color.Black.copy(alpha = (1 - lit).toFloat()))
    }
}

/** Weather.swift blossom: a cupped cherry petal, notched at the tip, near white at the base, with fine lines. */
private fun DrawScope.blossom(s: Double, tone: Triple<Color, Color, Color>, lit: Double) {
    val p = Path().apply {
        moveTo(pt(0.0, s * 0.5))
        curveTo(pt(-s * 0.46, -s * 0.12), pt(-s * 0.3, s * 0.42), pt(-s * 0.5, s * 0.16))
        curveTo(pt(-s * 0.07, -s * 0.5), pt(-s * 0.44, -s * 0.42), pt(-s * 0.2, -s * 0.52))
        quadTo(pt(s * 0.07, -s * 0.5), pt(0.0, -s * 0.36))
        curveTo(pt(s * 0.46, -s * 0.12), pt(s * 0.2, -s * 0.52), pt(s * 0.44, -s * 0.42))
        curveTo(pt(0.0, s * 0.5), pt(s * 0.5, s * 0.16), pt(s * 0.3, s * 0.42))
        close()
    }
    drawPath(p, Brush.radialGradient(listOf(tone.first, tone.second, tone.third), center = pt(0.0, s * 0.55), radius = (s * 1.05).toFloat()))
    clipPath(p) {
        drawOval(tone.third.copy(alpha = 0.18f), topLeft = pt(-s * 0.6, -s * 0.5), size = Size((s * 0.5).toFloat(), s.toFloat()))
        drawOval(Color.White.copy(alpha = 0.35f), topLeft = pt(s * 0.05, -s * 0.35), size = Size((s * 0.22).toFloat(), (s * 0.6).toFloat()))
        val lines = Path().apply {
            for (f in doubleArrayOf(-0.18, 0.0, 0.18)) {
                moveTo(pt(0.0, s * 0.45))
                quadTo(pt(s * f * 1.6, -s * 0.3), pt(s * f, s * 0.05))
            }
        }
        drawPath(lines, tone.third.copy(alpha = 0.25f), style = Stroke(0.4f))
        if (lit < 1) drawPath(p, Color.Black.copy(alpha = ((1 - lit) * 0.6).toFloat()))
    }
}

/** Weather.swift firefly: a dark body and a glowing tail that brightens and fades, wandering slowly. */
private fun DrawScope.firefly(i: Int, w: Double, h: Double, t: Double, scale: Double) {
    val x = r(i, 1) * w + sin(t * 0.3 + r(i, 5) * 7) * 30 + sin(t * 1.1 + r(i, 8) * 4) * 4
    val y = h * (0.4 + 0.55 * r(i, 4)) + cos(t * 0.25 + r(i, 6) * 5) * 24
    val glow = ((sin(t * 1.3 + r(i, 7) * 9) + 1) / 2).pow(2)
    val warm = LabColors.fireflyGlow
    val halo = (14 * scale).toFloat()
    val c = pt(x, y)
    drawOval(
        Brush.radialGradient(listOf(warm.copy(alpha = (0.35 * glow).toFloat()), warm.copy(alpha = 0f)), center = c, radius = halo),
        topLeft = Offset(c.x - halo, c.y - halo), size = Size(halo * 2, halo * 2),
    )
    drawOval(warm.copy(alpha = (0.35 + 0.65 * glow).toFloat()), topLeft = pt(x - 2.2 * scale, y - 1.4 * scale),
        size = Size((4.4 * scale).toFloat(), (2.8 * scale).toFloat()))
    drawOval(LabColors.fireflyBody.copy(alpha = 0.8f), topLeft = pt(x - 4.2 * scale, y - 1.1 * scale),
        size = Size((2.6 * scale).toFloat(), (2.2 * scale).toFloat()))
}
