package com.ferb.design.sky

import android.graphics.BlurMaskFilter
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.Canvas
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.semantics.clearAndSetSemantics
import com.ferb.design.LabColors
import com.ferb.design.Lamp
import com.ferb.design.Motion
import com.ferb.design.rememberReducedMotion
import kotlinx.coroutines.delay
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.max
import kotlin.math.min
import kotlin.math.sin

// The sky over the owner right now (Sky.swift SkyView and SkyPaint): its colour from the sun's real height where the
// owner is, the sun as FERB's amber dot exactly where it is, at night tonight's moon over a few fixed stars. A change
// of time glides through every sky in between, as the lab's animatable elevation and across do.

/**
 * The sky for [moment]. [showSun] off hides the sun and the moon (the lab's `showSun`). When the moment changes the
 * sun's height and place glide over [Motion.SKY_GLIDE_MS], unless the owner turned animations off.
 */
@Composable
fun Sky(moment: SkyMoment, modifier: Modifier = Modifier, showSun: Boolean = true, glide: Boolean = true) {
    val reduced = rememberReducedMotion()
    val animate = glide && !reduced
    val elevation by animateFloatAsState(moment.sun.elevation.toFloat(), if (animate) Motion.skyGlide() else androidx.compose.animation.core.snap(), label = "sun elevation")
    val across by animateFloatAsState(moment.across.toFloat(), if (animate) Motion.skyGlide() else androidx.compose.animation.core.snap(), label = "sun across")
    val moon = moment.moon
    Canvas(modifier.clipToBounds().clearAndSetSemantics { }) {
        drawSky(elevation.toDouble(), across.toDouble(), moon, showSun)
    }
}

/** The live clock for the sky: the current moment at [place], renewed once a minute (the lab's `.periodic(by: 60)`). */
@Composable
fun rememberSkyNow(place: SkyPlace): SkyMoment {
    var now by remember(place) { mutableStateOf(SkyMoment(System.currentTimeMillis() / 1000.0, place)) }
    LaunchedEffect(place) {
        while (true) {
            delay(60_000)
            now = SkyMoment(System.currentTimeMillis() / 1000.0, place)
        }
    }
    return now
}

private val STARS = SkyStars.generate()

/** How far up the screen the stars reach before they fade out entirely (Sky.swift: the mask ends at 0.65). */
private const val STAR_MASK_END = 0.65

/**
 * Paints the sky into [skySize] (the whole screen) shifted by [origin], so a piece of the screen (a glass panel) can
 * draw exactly the sky behind it. [stars] and [showSun] are off for the flat copy a panel uses without blur.
 */
fun DrawScope.drawSky(
    elevation: Double,
    across: Double,
    moon: MoonPhase,
    showSun: Boolean,
    stars: Boolean = true,
    origin: Offset = Offset.Zero,
    skySize: Size = size,
) {
    val look = ChosenSky.look(elevation, across)
    val w = skySize.width
    val h = skySize.height
    val sunX = w * (0.5 + across * 0.42).toFloat()
    val sunY = h * (0.94f - (max(elevation, -3.0) / 70).toFloat() * 0.82f)
    translate(-origin.x, -origin.y) {
        drawSkyLook(look, skySize, sunX)
        if (stars) {
            val starsAlpha = min(1.0, max(0.0, (-elevation - 7) / 8))
            if (starsAlpha > 0) {
                for (star in STARS) {
                    val y = star[1] * h
                    val mask = max(0.0, 1 - y / (STAR_MASK_END * h))
                    if (mask <= 0) continue
                    val s = star[2].toFloat() * density
                    drawOval(
                        Color.White.copy(alpha = (star[3] * mask * starsAlpha).toFloat()),
                        topLeft = Offset((star[0] * w).toFloat(), y.toFloat()), size = Size(s, s),
                    )
                }
            }
        }
        if (showSun && elevation > -2) drawSun(elevation, Offset(sunX, sunY), min(1.0, (elevation + 2) / 3).toFloat())
        if (showSun && elevation < -5) drawMoon(moon, Offset(w * 0.8f, h * 0.17f), min(1.0, (-elevation - 5) / 6).toFloat())
    }
}

/**
 * Sky.swift SkyFrameLook.paint: the colours down the screen, Astra's soft fields, and the lab's glow on the sun's
 * side screened over the rest. [sunX] is where the sun is across.
 */
fun DrawScope.drawSkyLook(look: SkyFrameLook, skySize: Size = size, sunX: Float = skySize.width / 2) {
    val w = skySize.width
    val h = skySize.height
    drawRect(
        Brush.verticalGradient(
            *SkyFrameLook.GRID.mapIndexed { i, at -> at.toFloat() to look.colors[i].color() }.toTypedArray(),
            startY = 0f, endY = h,
        ),
        size = skySize,
    )
    for (field in look.fields) {
        if (field.alpha > 0.001) drawField(field, skySize)
    }
    if (look.sunGlowStrength > 0.001) {
        val s = look.sunGlowStrength
        drawRect(
            Brush.radialGradient(
                0f to look.sunGlow.color(s), 0.5f to look.sunGlow.color(s * 0.35), 1f to look.sunGlow.color(0.0),
                center = Offset(sunX, h * 1.05f), radius = w * 0.9f,
            ),
            size = skySize,
            blendMode = BlendMode.Screen,
        )
    }
}

/** AstraSky.swift paint(_:in:size:): a radial falloff in a unit circle stretched to the field's radii. */
private fun DrawScope.drawField(field: SkyField, skySize: Size) {
    val falloff = listOf(0.0 to 1.0, 0.30 to 0.65, 0.65 to 0.18, 1.0 to 0.0)
    withTransform({
        translate((field.x * skySize.width).toFloat(), (field.y * skySize.height).toFloat())
        scale((field.rx * skySize.width).toFloat(), (field.ry * skySize.height).toFloat(), pivot = Offset.Zero)
    }) {
        drawRect(
            Brush.radialGradient(
                *falloff.map { (at, strength) -> at.toFloat() to field.colour.color(field.alpha * strength) }.toTypedArray(),
                center = Offset.Zero, radius = 1f,
            ),
            topLeft = Offset((-field.x / field.rx).toFloat(), (-field.y / field.ry).toFloat()),
            size = Size((1 / field.rx).toFloat(), (1 / field.ry).toFloat()),
        )
    }
}

/**
 * Sky.swift SunDisc: FERB's amber dot, white high in the sky and amber below 10 degrees; a 60-point halo at 30 %
 * blurred by 16, and an 18-point core with a shadow of radius 7 in its own colour at 80 %.
 */
private fun DrawScope.drawSun(elevation: Double, center: Offset, alpha: Float) {
    val core = if (elevation < 10) Lamp.amber else LabColors.sunHigh
    blurredCircle(core.copy(alpha = 0.3f * alpha), center, 30f * density, 16f * density)
    blurredCircle(core.copy(alpha = 0.8f * alpha), center, 9f * density, 7f * density)
    drawCircle(core.copy(alpha = alpha), radius = 9f * density, center = center)
}

/** Sky.swift MoonDisc: 15 points, the dark side, tonight's lit part, and a glow as strong as it is lit. */
private fun DrawScope.drawMoon(moon: MoonPhase, center: Offset, alpha: Float) {
    val r = 7.5f * density
    blurredCircle(LabColors.moonLit.copy(alpha = (0.35 * moon.lit).toFloat() * alpha), center, r, 8f * density)
    drawCircle(LabColors.moonDark.copy(alpha = alpha), radius = r, center = center)
    drawPath(moonLight(moon.lit, moon.waxing, center, r), LabColors.moonLit.copy(alpha = alpha))
}

/** Sky.swift MoonLight: half a circle on the lit side plus an ellipse that grows or eats into it. */
internal fun moonLight(lit: Double, waxing: Boolean, c: Offset, r: Float): Path {
    val k = (1 - 2 * lit).toFloat()
    val side = if (waxing) 1f else -1f
    return Path().apply {
        moveTo(c.x, c.y - r)
        for (i in 0..40) {
            val a = -PI / 2 + PI * i / 40
            lineTo(c.x + side * r * cos(a).toFloat(), c.y + r * sin(a).toFloat())
        }
        for (i in 0..40) {
            val a = PI / 2 - PI * i / 40
            lineTo(c.x + side * k * r * cos(a).toFloat(), c.y + r * sin(a).toFloat())
        }
        close()
    }
}

/**
 * A circle softened the way SwiftUI's blur and shadow soften (their radius taken as the Gaussian's standard
 * deviation), drawn with the platform's blur mask, which works on every Android version FERB supports.
 */
internal fun DrawScope.blurredCircle(color: Color, center: Offset, radius: Float, swiftBlur: Float) {
    if (color.alpha <= 0f) return
    drawIntoCanvas { canvas ->
        val paint = android.graphics.Paint(android.graphics.Paint.ANTI_ALIAS_FLAG).apply {
            this.color = color.toArgb()
            maskFilter = BlurMaskFilter(skiaBlurRadius(swiftBlur), BlurMaskFilter.Blur.NORMAL)
        }
        canvas.nativeCanvas.drawCircle(center.x, center.y, radius, paint)
    }
}

/** Skia turns a blur radius into a standard deviation as 0.57735 r + 0.5; this is the inverse. */
internal fun skiaBlurRadius(sigma: Float): Float = max(0.5f, (sigma - 0.5f) / 0.57735f)
