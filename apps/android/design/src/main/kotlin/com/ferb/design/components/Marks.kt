package com.ferb.design.components

import androidx.compose.foundation.Canvas
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.vector.rememberVectorPainter
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextMeasurer
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.LabColors
import com.ferb.design.Lamp
import com.ferb.design.Tone
import com.ferb.design.sky.blurredCircle
import kotlin.math.max

/** Theme.swift LampDot: a small dot of colour with a tight halo, the only thing in FERB that glows. */
@Composable
fun LampDot(color: Color, size: Dp = 6.dp, modifier: Modifier = Modifier) {
    Canvas(modifier.size(size)) { lampDot(color, Offset(this.size.width / 2, this.size.height / 2), this.size.width / 2) }
}

internal fun DrawScope.lampDot(color: Color, center: Offset, radius: Float) {
    blurredCircle(color.copy(alpha = 0.7f), center, radius, radius * 2 * 0.45f)
    drawCircle(color, radius, center)
}

/** Theme.swift FerbMark: the logo, an ivory "f" and its amber dot on a dark tile. Its dot is always amber. */
@Composable
fun FerbMark(size: Dp = 64.dp, modifier: Modifier = Modifier) {
    Canvas(modifier.size(size).semantics { contentDescription = "FERB" }) {
        val s = this.size.width
        ferbTile(s)
        drawPath(ferbLetter(s), Tone.text)
        ferbDot(s)
    }
}

/** FerbMark's dark tile with its light edge, in a square of side [s] (the launch draws the same, PHN-632). */
internal fun DrawScope.ferbTile(s: Float) {
    val corner = CornerRadius(s * 0.23f)
    drawRoundRect(Brush.verticalGradient(listOf(Tone.ink3, Tone.ink0)), cornerRadius = corner)
    val line = max(0.75f * density, s / 96)
    drawRoundRect(
        Brush.verticalGradient(listOf(Color.White.copy(alpha = 0.22f), Color.White.copy(alpha = 0.02f)), startY = 0f, endY = s / 2),
        topLeft = Offset(line / 2, line / 2), size = Size(s - line, s - line), cornerRadius = CornerRadius(s * 0.23f - line / 2),
        style = Stroke(line),
    )
}

/** FerbMark's amber dot: its centre as a fraction of the tile, and the dot itself in a square of side [s]. */
internal const val FERB_DOT_X = (82f - 16) / 96
internal const val FERB_DOT_Y = (85f - 16) / 96

internal fun DrawScope.ferbDot(s: Float) = lampDot(Lamp.amber, Offset(s * FERB_DOT_X, s * FERB_DOT_Y), s / 12)

/** The "f" of ferb-mark.svg, in that file's 96-point tile (its 128 box less the 16 margin): Theme.swift FerbMark.Letter. */
internal fun ferbLetter(width: Float): Path {
    val k = width / 96
    fun x(v: Double) = ((v - 16) * k).toFloat()
    return Path().apply {
        moveTo(x(49.969), x(58.772))
        lineTo(x(49.969), x(90.0)); lineTo(x(60.477), x(90.0)); lineTo(x(60.477), x(58.772))
        lineTo(x(67.729), x(58.772)); lineTo(x(67.729), x(51.742)); lineTo(x(60.477), x(51.742))
        lineTo(x(60.477), x(49.448))
        quadraticTo(x(60.477), x(47.08), x(61.402), x(46.081))
        quadraticTo(x(62.327), x(45.082), x(64.473), x(45.082))
        quadraticTo(x(66.471), x(45.082), x(68.321), x(45.304))
        lineTo(x(68.321), x(37.46))
        quadraticTo(x(66.989), x(37.386), x(65.583), x(37.275))
        quadraticTo(x(64.177), x(37.164), x(62.771), x(37.164))
        quadraticTo(x(56.333), x(37.164), x(53.151), x(40.42))
        quadraticTo(x(49.969), x(43.676), x(49.969), x(48.782))
        lineTo(x(49.969), x(51.742)); lineTo(x(43.679), x(51.742)); lineTo(x(43.679), x(58.772))
        close()
    }
}

/** Data.swift FileKind: what a file is, read from its name; the four office kinds keep their known colours. */
enum class FileKind(val badge: Color) {
    PDF(LabColors.badgePdf), DOCUMENT(LabColors.badgeDocument), SHEET(LabColors.badgeSheet), SLIDES(LabColors.badgeSlides),
    IMAGE(LabColors.badgeOther), VIDEO(LabColors.badgeOther), AUDIO(LabColors.badgeOther), CODE(LabColors.badgeOther),
    ARCHIVE(LabColors.badgeOther), TEXT(LabColors.badgeOther), OTHER(LabColors.badgeOther);

    companion object {
        fun of(name: String): FileKind = when (name.substringAfterLast('.', "").lowercase()) {
            "pdf" -> PDF
            "doc", "docx", "pages", "rtf" -> DOCUMENT
            "xls", "xlsx", "csv", "numbers", "tsv" -> SHEET
            "ppt", "pptx", "key" -> SLIDES
            "png", "jpg", "jpeg", "heic", "gif", "webp", "svg", "tiff" -> IMAGE
            "mp4", "mov", "m4v", "webm" -> VIDEO
            "mp3", "m4a", "wav", "aiff", "caf" -> AUDIO
            "swift", "py", "js", "ts", "json", "html", "css", "sh", "toml", "yaml", "yml", "rs", "go", "c", "h" -> CODE
            "zip", "dmg", "tar", "gz", "pkg" -> ARCHIVE
            "txt", "md" -> TEXT
            else -> OTHER
        }

        /** Data.swift FileIcon.label: the extension in capitals when it is 1 to 5 letters, else FILE. */
        fun label(name: String): String {
            val ext = if (name.contains('.')) name.substringAfterLast('.').uppercase() else ""
            return if (ext.isNotEmpty() && ext.length <= 5) ext else "FILE"
        }
    }
}

/** Data.swift FileIcon: paper with a folded corner, a hint of its contents and a badge naming its type. */
@Composable
fun FileIcon(name: String, width: Dp = 34.dp, modifier: Modifier = Modifier) {
    val kind = FileKind.of(name)
    val label = FileKind.label(name)
    val measurer = rememberTextMeasurer()
    val glyph = when (kind) {
        FileKind.IMAGE -> FerbIcons.photo
        FileKind.VIDEO -> FerbIcons.play
        FileKind.AUDIO -> FerbIcons.waveform
        FileKind.CODE -> FerbIcons.code
        FileKind.ARCHIVE -> FerbIcons.archive
        else -> null
    }
    val glyphPainter = glyph?.let { rememberVectorPainter(it) }
    // the paper, its fold, the shadow's paint and the badge's words are made once per size, not on every draw: a row
    // redrawn as the list scrolls only draws them (kb PHN-716)
    androidx.compose.foundation.layout.Spacer(modifier.size(width, width * 1.26f).semantics { contentDescription = "$label file" }.drawWithCache {
        val w = size.width
        val h = size.height
        val fold = w * 0.3f
        val shadow = shadowPaint(w)
        val paperPath = paper(w, h, fold)
        val foldPath = Path().apply {
            moveTo(w - fold, 0f)
            lineTo(w - fold, fold - w * 0.06f)
            quadraticTo(w - fold, fold, w - fold + w * 0.06f, fold)
            lineTo(w, fold)
            close()
        }
        val badge = badgeWords(measurer, label, w)
        onDrawBehind {
        drawContext.canvas.nativeCanvas.drawRoundRect(0f, w * 0.04f, w, h + w * 0.04f, w * 0.11f, w * 0.11f, shadow)
        drawPath(paperPath, LabColors.paper)
        drawPath(foldPath, LabColors.fold)
        val ink = LabColors.paperInk
        translate(0f, h * 0.1f) {
            val ch = h * 0.62f
            if (glyphPainter != null) {
                val g = w * 0.34f * 1.2f
                translate((w - g) / 2, (ch - g) / 2) {
                    with(glyphPainter) { draw(Size(g, g), colorFilter = ColorFilter.tint(ink.copy(alpha = 0.4f))) }
                }
            } else when (kind) {
                FileKind.SHEET -> {
                    val cw = w * 0.17f; val chh = w * 0.1f; val gap = w * 0.05f
                    val gw = 3 * cw + 2 * gap; val gh = 3 * chh + 2 * gap
                    for (r in 0 until 3) for (c in 0 until 3) drawRoundRect(ink.copy(alpha = 0.2f),
                        Offset((w - gw) / 2 + c * (cw + gap), (ch - gh) / 2 + r * (chh + gap)), Size(cw, chh), CornerRadius(1f * density))
                }
                FileKind.SLIDES -> {
                    val sw = w * 0.6f; val sh = w * 0.4f; val lw = max(1f * density, w * 0.035f)
                    drawRoundRect(ink.copy(alpha = 0.3f), Offset((w - sw) / 2 + lw / 2, (ch - sh) / 2 + lw / 2), Size(sw - lw, sh - lw),
                        CornerRadius(w * 0.05f), style = Stroke(lw))
                }
                else -> {
                    val gap = w * 0.065f
                    val heights = listOf(max(1.5f * density, w * 0.06f), max(1f * density, w * 0.04f), max(1f * density, w * 0.04f), max(1f * density, w * 0.04f))
                    val widths = listOf(w * 0.38f, w * 0.56f, w * 0.56f, w * 0.34f)
                    val total = heights.sum() + gap * 3
                    var y = (ch - total) / 2
                    for (i in 0 until 4) {
                        drawRoundRect(ink.copy(alpha = if (i == 0) 0.75f else 0.22f), Offset(w * 0.18f, y), Size(widths[i], heights[i]), CornerRadius(1f * density))
                        y += heights[i] + gap
                    }
                }
            }
        }
        drawBadge(badge, kind.badge, w, h)
        }
    })
}

private fun paper(w: Float, h: Float, fold: Float): Path {
    val r = w * 0.11f
    return Path().apply {
        moveTo(r, 0f); lineTo(w - fold, 0f); lineTo(w, fold); lineTo(w, h - r)
        quadraticTo(w, h, w - r, h); lineTo(r, h); quadraticTo(0f, h, 0f, h - r); lineTo(0f, r); quadraticTo(0f, 0f, r, 0f)
        close()
    }
}

/** The file's shadow's paint: black 30 %, radius 0.08 x width (drawn down 0.04 x width). */
private fun shadowPaint(w: Float) = android.graphics.Paint(android.graphics.Paint.ANTI_ALIAS_FLAG).apply {
    color = android.graphics.Color.argb(77, 0, 0, 0)
    maskFilter = android.graphics.BlurMaskFilter(com.ferb.design.sky.skiaBlurRadius(w * 0.08f), android.graphics.BlurMaskFilter.Blur.NORMAL)
}


/** The type badge: white bold text on the kind's colour, hanging off the left edge (offset -0.1 w, 0.6 h). */
private fun androidx.compose.ui.unit.Density.badgeWords(measurer: TextMeasurer, label: String, w: Float): androidx.compose.ui.text.TextLayoutResult {
    val fontPx = max(6f * density, w * 0.21f)
    return measurer.measure(label, FerbType.style(0f, FontWeight.Bold, Color.White).copy(fontSize = (fontPx / density / fontScale).sp, letterSpacing = 0.sp))
}

private fun DrawScope.drawBadge(layout: androidx.compose.ui.text.TextLayoutResult, colour: Color, w: Float, h: Float) {
    val bw = layout.size.width + 2 * w * 0.09f
    val bh = w * 0.33f
    val x = -w * 0.1f
    val y = h * 0.6f
    drawRoundRect(colour, Offset(x, y), Size(bw, bh), CornerRadius(w * 0.07f))
    drawText(layout, topLeft = Offset(x + w * 0.09f, y + (bh - layout.size.height) / 2))
}
