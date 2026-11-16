package com.ferb.design.glass

import android.graphics.BlurMaskFilter
import android.os.Build
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.runtime.Composable
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlurEffect
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.TileMode
import androidx.compose.ui.graphics.addOutline
import androidx.compose.ui.graphics.asAndroidPath
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.toSize
import com.ferb.design.FerbIcon
import com.ferb.design.FerbType
import com.ferb.design.Radius
import com.ferb.design.Tone
import com.ferb.design.components.LampDot
import com.ferb.design.sky.MoonPhase
import com.ferb.design.sky.drawSky
import com.ferb.design.sky.skiaBlurRadius

// Glass (Theme.swift GlassLevel and GlassBackground; DESIGN.md "Glass"; tokens.json `glass`): four depths, and only a
// panel is real frost. On the phone the lab draws no glass panel of its own (its sheets and cards are white, PHN-416),
// so these are here for the pieces that float over the sky in the Mac's language; the gallery shows them.

/** Theme.swift GlassLevel. */
enum class GlassLevel { PANEL, CARD, CONTROL, INSET }

/** The sky behind a screen, so a glass panel can draw exactly the part of it that lies behind the panel. */
class SkyBackdrop(val elevation: Double, val across: Double, val moon: MoonPhase) {
    var origin by mutableStateOf(Offset.Zero)
    var size by mutableStateOf(Size.Zero)
}

val LocalSkyBackdrop = compositionLocalOf<SkyBackdrop?> { null }

/** Put on the composable that draws the sky, so panels know where the sky is. */
fun Modifier.skyBackdropHost(backdrop: SkyBackdrop): Modifier =
    onGloballyPositioned { backdrop.origin = it.positionInRoot(); backdrop.size = it.size.toSize() }

/** True where the platform can blur what is drawn under a panel (RenderEffect, Android 12 and later). */
val canBlur: Boolean get() = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S

/** tokens.json glass.panel: a backdrop blur of "about 30". */
const val PANEL_BLUR_DP = 30f

/** Theme.swift GlassBackground(.panel): the ink1 tint over the frost, 56 %. */
const val PANEL_TINT = 0.56f

/**
 * A panel: the sky behind it blurred by 30 (Android 12 and later), ink1 at 56 % over it, a highlight on its top lip
 * (white 26 % to 5 % a third of the way down, 3 % at the bottom) and the two panel shadows. On Android 11, where
 * nothing can blur what lies behind, the panel draws the sky's smooth colours without the sun, moon and stars: a
 * blurred gradient is the gradient itself, so the panel looks the same except that small lights do not glow through.
 */
@Composable
fun GlassPanel(
    modifier: Modifier = Modifier,
    shape: Shape = RoundedCornerShape(Radius.phoneSheet),
    content: @Composable BoxScope.() -> Unit,
) {
    val backdrop = LocalSkyBackdrop.current
    var origin by remember { mutableStateOf(Offset.Zero) }
    Box(modifier.onGloballyPositioned { origin = it.positionInRoot() }.panelShadow(shape).clip(shape)) {
        Canvas(
            Modifier.matchParentSize().graphicsLayer {
                if (canBlur) renderEffect = BlurEffect(PANEL_BLUR_DP * density, PANEL_BLUR_DP * density, TileMode.Clamp)
            },
        ) {
            if (backdrop != null) {
                drawSky(backdrop.elevation, backdrop.across, backdrop.moon, showSun = canBlur, stars = canBlur,
                    origin = origin - backdrop.origin, skySize = backdrop.size)
            }
        }
        Box(Modifier.matchParentSize().background(Tone.ink1.copy(alpha = PANEL_TINT)))
        Box(Modifier.matchParentSize().border(1.dp, topLip, shape))
        content()
    }
}

private val topLip = Brush.verticalGradient(
    0f to Color.White.copy(alpha = 0.26f), 0.3f to Color.White.copy(alpha = 0.05f), 1f to Color.White.copy(alpha = 0.03f),
)

/** Theme.swift `.glass(shape, level)` for the three depths inside a panel: a step of light, never glass on glass. */
fun Modifier.glass(shape: Shape, level: GlassLevel): Modifier = when (level) {
    GlassLevel.CARD -> background(Tone.fill1, shape)
    GlassLevel.CONTROL -> background(Tone.fill2, shape)
    GlassLevel.INSET -> background(Tone.well, shape)
        .border(1.dp, Brush.verticalGradient(0f to Color.Black.copy(alpha = 0.4f), 0.5f to Color.Transparent), shape)
    GlassLevel.PANEL -> this
}

/**
 * Theme.swift panelShadow(): a wide soft shadow for height (black 34 %, radius 36, down 20) and a tight one where
 * the panel meets the page (black 30 %, radius 3, down 1). Shadows are black; nothing casts a coloured glow.
 */
fun Modifier.panelShadow(shape: Shape): Modifier = drawBehind {
    shapeShadow(shape, Color.Black.copy(alpha = 0.34f), 36f, 20f)
    shapeShadow(shape, Color.Black.copy(alpha = 0.30f), 3f, 1f)
}

private fun DrawScope.shapeShadow(shape: Shape, color: Color, radius: Float, y: Float) {
    val path = Path().apply { addOutline(shape.createOutline(size, layoutDirection, this@shapeShadow)) }
    translate(0f, y * density) {
        drawIntoCanvas { canvas ->
            val paint = android.graphics.Paint(android.graphics.Paint.ANTI_ALIAS_FLAG).apply {
                this.color = color.toArgb()
                maskFilter = BlurMaskFilter(skiaBlurRadius(radius * density), BlurMaskFilter.Blur.NORMAL)
            }
            canvas.nativeCanvas.drawPath(path.asAndroidPath(), paint)
        }
    }
}

/** Theme.swift Chip: a lamp, a symbol and a few words on the control step, 21 high with 6 corners. */
@Composable
fun Chip(text: String, modifier: Modifier = Modifier, lamp: Color? = null, icon: androidx.compose.ui.graphics.vector.ImageVector? = null) {
    Row(
        modifier.heightIn(min = 21.dp).glass(RoundedCornerShape(Radius.chip), GlassLevel.CONTROL).padding(horizontal = 7.dp),
        horizontalArrangement = Arrangement.spacedBy(5.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (lamp != null) LampDot(lamp, 5.dp)
        if (icon != null) FerbIcon(icon, 9.5f, Tone.textDim)
        BasicText(text, style = FerbType.ink(11f, FerbType.Medium, Tone.textDim))
    }
}
