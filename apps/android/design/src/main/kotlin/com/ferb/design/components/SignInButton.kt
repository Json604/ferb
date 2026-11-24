package com.ferb.design.components

import android.graphics.BlurMaskFilter
import android.graphics.RectF
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.Paint
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.graphics.vector.addPathNodes
import androidx.compose.ui.graphics.vector.group
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.ferb.design.FerbType
import com.ferb.design.Phone

/** Who a sign-in button signs in with (kb APP-1140, APP-1141). */
enum class SignInProvider { GOOGLE, APPLE }

/**
 * "Continue with Google" / "Continue with Apple" with the provider's own logo (owner kb APP-1140; how, kb APP-1141),
 * the same on Android, the iPhone (FerbDesign SignInButton) and FERB.app (FERBNotch SignInButton). Everything is in
 * proportion to the button's [height], from the two providers' own 44-point buttons:
 * - Google's iOS button (Sign in with Google branding guidelines): the "G" 20 of 44 high, 16 from the leading edge,
 *   12 to the words. The Apple logo is Apple's medium logo (Human Interface Guidelines, Sign in with Apple: the logo
 *   file as high as the button; its glyph is 19 of 44), centred on the same line as the G (Apple: "inset the logo if
 *   necessary" to line it up with other sign-in logos).
 * - The words stay centred in the whole button, kept clear of the logo on both sides (16 + 20 + 12 of 44), and wrap.
 * - On a light surface Google's dark theme for both buttons (fill #131314, a 1 px #8E918F line inside, words
 *   #E3E3E3), the Apple logo white; on FERB's dark surface ([LocalOnFerb], as [Pill] turns white there) Google's
 *   light theme (#FFFFFF, #747775, #1F1F1F), the Apple logo black: Apple's black and white styles, each on the
 *   background Apple allows it on.
 * The logos are decorative: the button says only its words ("Continue with Google"). Pressed and waiting ([onClick]
 * null) look like [Pill]'s, logo included.
 */
@Composable
fun SignInButton(
    provider: SignInProvider,
    title: String,
    modifier: Modifier = Modifier,
    height: Dp = Phone.pillHeight,
    onClick: (() -> Unit)? = {},
) {
    val form = if (LocalOnFerb.current) SignInForm.Light else SignInForm.Dark
    val shape = RoundedCornerShape(Phone.pillCorner)
    val unit = height / 44f
    Box(
        modifier.fillMaxWidth().pressable(onClick).heightIn(min = height)
            .background(form.fill, shape).border(1.dp, form.line, shape),
        contentAlignment = Alignment.Center,
    ) {
        // the logo's column: 16 from the leading edge, 20 wide; both logos centred in it
        Box(Modifier.align(Alignment.CenterStart).padding(start = unit * 16f).width(unit * 20f), contentAlignment = Alignment.Center) {
            when (provider) {
                SignInProvider.GOOGLE -> GoogleG(unit * 20f)
                SignInProvider.APPLE -> AppleLogo(unit * 19f, form.appleLogo)
            }
        }
        PhoneText(title, 16f, FerbType.Semibold, form.words, Modifier.padding(horizontal = unit * 48f, vertical = 6.dp),
            maxLines = 2, align = TextAlign.Center)
    }
}

private class SignInForm(val fill: Color, val line: Color, val words: Color, val appleLogo: Color) {
    companion object {
        val Dark = SignInForm(Color(0xFF131314), Color(0xFF8E918F), Color(0xFFE3E3E3), Color.White)
        val Light = SignInForm(Color.White, Color(0xFF747775), Color(0xFF1F1F1F), Color.Black)
    }
}

/**
 * Google's standard colour "G", [size] square, drawn from Google's own artwork (signin-assets.zip, "Theme=Dark,
 * Show text=No, Shape=Square, Platform=Android+Web.svg", the G in its 20-unit box at 10,10 of the 40-unit file):
 * the G path as the mask, under it the conic gradient (its colours and angles as the file has them, its centre and
 * its 45° turn from the file's transform) and the six soft ellipses (centre, radii, turn, colour, 1-unit blur).
 * Not drawn: the 1-unit blur the file also puts on the gradient layer, which only softens it where the blue ellipses
 * cover it (where the gradient's colours meet, at the bar's inner end) and at the circle's edge, 2.6 units outside
 * the G.
 */
@Composable
fun GoogleG(size: Dp, modifier: Modifier = Modifier) {
    Canvas(modifier.size(size).graphicsLayer { compositingStrategy = CompositingStrategy.Offscreen }) { drawGoogleG() }
}

private val googleGPath by lazy { PathParser().parsePathString(GoogleArtwork.G).toPath() }

private fun DrawScope.drawGoogleG() {
    val unit = size.width / 20f
    // The G first, then everything else only where it is (source-in, source-atop): the G's own edge, anti-aliased,
    // is the mask's, in this offscreen layer
    withTransform({ scale(unit, unit, Offset.Zero); translate(-10f, -10f) }) {
        drawPath(googleGPath, Color.Black)
        val c = GoogleArtwork.CONIC_CENTRE
        rotate(GoogleArtwork.CONIC_TURN, c) {
            drawOval(Brush.sweepGradient(*GoogleArtwork.CONIC, center = c), GoogleArtwork.CIRCLE_ORIGIN, GoogleArtwork.CIRCLE_SIZE,
                blendMode = BlendMode.SrcIn)
        }
    }
    // The ellipses in pixels, so the blur's radius is the file's 1 unit whatever the canvas's matrix
    drawIntoCanvas { canvas ->
        val native = canvas.nativeCanvas
        val sigma = unit * GoogleArtwork.BLUR
        val paint = Paint().asFrameworkPaint().apply {
            isAntiAlias = true
            blendMode = android.graphics.BlendMode.SRC_ATOP
            // Skia's radius to sigma is 0.57735 r + 0.5
            if (sigma > 0.5f) maskFilter = BlurMaskFilter((sigma - 0.5f) / 0.57735f, BlurMaskFilter.Blur.NORMAL)
        }
        for (e in GoogleArtwork.ELLIPSES) {
            paint.color = e.colour.toArgb()
            native.save()
            native.translate((e.cx - 10f) * unit, (e.cy - 10f) * unit)
            native.rotate(e.turn)
            native.drawOval(RectF(-e.rx * unit, -e.ry * unit, e.rx * unit, e.ry * unit), paint)
            native.restore()
        }
    }
}

/** Google's G artwork, the numbers exactly as Google's file has them (in its 40-unit coordinates). */
internal object GoogleArtwork {
    const val G = "M29.3987 18.1814H19.9849V22.0445H25.3598C25.1286 23.294 24.4294 24.3596 23.3676 25.0712C22.4746 25.6716 " +
        "21.3266 26.0211 19.9849 26.0211C17.3864 26.0211 15.1823 24.2666 14.3947 21.9004C14.1952 21.2989 14.0853 20.6599 " +
        "14.0853 19.9983C14.0853 19.3367 14.1952 18.6966 14.3947 18.0962C15.1823 15.7311 17.3864 13.9755 19.9849 13.9755C21.4524 " +
        "13.9755 22.767 14.4816 23.8039 15.4713L26.6653 12.6057C24.936 10.9908 22.6786 10 19.9849 10C16.0832 10 12.705 12.2414 " +
        "11.0618 15.5076C10.383 16.8592 10 18.3834 10 19.9994C10 21.6155 10.383 23.1396 11.0618 24.4913C12.705 27.7597 16.0832 " +
        "30 19.9849 30C22.6797 30 24.9485 29.1137 26.6018 27.5861C28.4887 25.8452 29.5732 23.2702 29.5732 20.2275C29.5732 " +
        "19.5182 29.5131 18.835 29.3987 18.1825V18.1814Z"

    /** The gradient's circle: M7.25922 19.7927 C7.25922 12.6759 13.0209 6.90668 20.1283 6.90668 … 32.9973, 32.6786. */
    val CIRCLE_ORIGIN = Offset(7.25922f, 6.90668f)
    val CIRCLE_SIZE = Size(32.9973f - 7.25922f, 32.6786f - 6.90668f)

    /** The file's matrix(0.00804129 -0.00805186 0.00804128 0.00805186 19.6819 19.7927): a quarter-turn's half back. */
    val CONIC_CENTRE = Offset(19.6819f, 19.7927f)
    const val CONIC_TURN = -45f

    /** conic-gradient(from 90deg, …): the file's "from 90deg" is 3 o'clock, where Android's sweep starts. */
    val CONIC: Array<Pair<Float, Color>> = arrayOf(
        0f to Color(255, 70, 65),
        4.14555f / 360f to Color(255, 70, 65),
        39.154f / 360f to Color(49, 134, 255),
        72.0044f / 360f to Color(49, 134, 255),
        96.7463f / 360f to Color(0, 165, 183),
        120.897f / 360f to Color(14, 188, 95),
        154.722f / 360f to Color(14, 188, 95),
        179.136f / 360f to Color(108, 196, 0),
        203.588f / 360f to Color(255, 204, 0),
        226.915f / 360f to Color(255, 211, 20),
        251.688f / 360f to Color(255, 204, 0),
        273.129f / 360f to Color(255, 106, 43),
        289.305f / 360f to Color(253, 70, 65),
        359.593f / 360f to Color(255, 70, 65),
        1f to Color(255, 70, 65),
    )

    class Ellipse(val cx: Float, val cy: Float, val rx: Float, val ry: Float, val turn: Float, val colour: Color)

    val ELLIPSES = listOf(
        Ellipse(20.0496f, 20.2413f, 5.39634f, 2.83537f, 24.4473f, Color(0xFF3186FF)),
        Ellipse(33.3538f, 18.2155f, 7.43918f, 3.09357f, 0f, Color(0xFF3186FF)),
        Ellipse(25.2744f, 16.2195f, 7.40854f, 2.37805f, 0f, Color(0xFFFF4641)),
        Ellipse(29.5427f, 12.9268f, 7.40854f, 2.37805f, 0f, Color(0xFFFF5B8B)),
        Ellipse(24.4817f, 19.878f, 8.5061f, 3.10976f, 0f, Color(0xFF3186FF)),
        Ellipse(25.1842f, 14.0197f, 4.53882f, 2.37805f, -28.6599f, Color(0xFFFF4641)),
    )

    /** feGaussianBlur stdDeviation="1". */
    const val BLUR = 1f
}

/**
 * The Apple logo [height] high in [colour]: the glyph of Apple's own Sign in with Apple button for the web and other
 * platforms (appleid.auth.js, the medium logo, 15 × 19 in its 44-high button), its path exactly as Apple's script has
 * it. The iPhone and FERB.app draw the system's own apple.logo instead.
 */
@Composable
fun AppleLogo(height: Dp, colour: Color, modifier: Modifier = Modifier) {
    Image(appleLogo, null, modifier.size(height * (15f / 19f), height), colorFilter = ColorFilter.tint(colour))
}

private val appleLogo: ImageVector by lazy {
    ImageVector.Builder("AppleLogo", 15.dp, 19.dp, 15f, 19f).apply {
        group(translationX = -20.5f, translationY = -16f) {
            addPath(addPathNodes(APPLE_GLYPH), fill = SolidColor(Color.White))
        }
    }.build()
}

private const val APPLE_GLYPH = "M28.2226562,20.3846154 C29.0546875,20.3846154 30.0976562,19.8048315 30.71875,19.0317864 " +
    "C31.28125,18.3312142 31.6914062,17.352829 31.6914062,16.3744437 C31.6914062,16.2415766 31.6796875,16.1087095 31.65625,16 " +
    "C30.7304687,16.0362365 29.6171875,16.640178 28.9492187,17.4494596 C28.421875,18.06548 27.9414062,19.0317864 27.9414062,20.0222505 " +
    "C27.9414062,20.1671964 27.9648438,20.3121424 27.9765625,20.3604577 C28.0351562,20.3725366 28.1289062,20.3846154 28.2226562,20.3846154 " +
    "Z M25.2929688,35 C26.4296875,35 26.9335938,34.214876 28.3515625,34.214876 C29.7929688,34.214876 30.109375,34.9758423 31.375,34.9758423 " +
    "C32.6171875,34.9758423 33.4492188,33.792117 34.234375,32.6325493 C35.1132812,31.3038779 35.4765625,29.9993643 35.5,29.9389701 " +
    "C35.4179688,29.9148125 33.0390625,28.9122695 33.0390625,26.0979021 C33.0390625,23.6579784 34.9140625,22.5588048 35.0195312,22.474253 " +
    "C33.7773438,20.6382708 31.890625,20.5899555 31.375,20.5899555 C29.9804688,20.5899555 28.84375,21.4596313 28.1289062,21.4596313 " +
    "C27.3554688,21.4596313 26.3359375,20.6382708 25.1289062,20.6382708 C22.8320312,20.6382708 20.5,22.5950413 20.5,26.2911634 " +
    "C20.5,28.5861411 21.3671875,31.013986 22.4335938,32.5842339 C23.3476562,33.9129053 24.1445312,35 25.2929688,35 Z"
