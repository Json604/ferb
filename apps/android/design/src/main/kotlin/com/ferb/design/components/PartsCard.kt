package com.ferb.design.components

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.unit.dp
import com.ferb.design.FerbType
import com.ferb.design.Ink

// The card of a request in parts as FERB.app draws it (kb PHN-737, the owner: "see these box cut outs for rows, see hte
// mac ones … so clean while running adn after too. please make it clean liek mac"; TurnCardView.swift, CardRowState,
// StepChecklistView): one card on FERB's surface, no fill behind any row and no buttons. While a part runs or waits it
// is one line: a thin spinner or a small hollow circle, its label, its state words in the quieter ink (cut with an
// ellipsis; each of the two takes at most half the line when both are long), and its own round picture at the far right.
// A part that ended is its label as a small heading with its answer under it, hairlines between parts. The footer is
// FERB.app's: the done line in bold with code's lines after it. A tap on a running part with no picture opens its task's
// short view, where Watch live and Stop are.

/** What colours the card as a whole (FERB.app's surfaceTint): amber while it asks, red when a part failed, blue when updated or paused. */
enum class CardTint(val color: Color?) { NONE(null), ASKING(Color(0xFFFFB020)), FAILED(Color(0xFFE5281B)), UPDATED(Color(0xFF3B82F6)) }

@Composable
fun PartsCard(
    headline: String,
    progress: String,
    parts: List<PlanPart>,
    modifier: Modifier = Modifier,
    tint: CardTint = CardTint.NONE,
    status: String? = null,
    footer: String? = null,
    footerLines: List<String> = emptyList(),
    watching: List<Boolean> = emptyList(),
    pictures: List<ImageBitmap?> = emptyList(),
    onPicture: ((Int) -> Unit)? = null,
    onPart: ((Int) -> Unit)? = null,
    more: String = "Show more",
    less: String = "Show less",
) {
    val ink = CardInk.current()
    val shape = RoundedCornerShape(22.dp)
    Column(
        modifier.fillMaxWidth().background(ink.card, shape)
            .then(if (tint.color != null) Modifier.background(tint.color.copy(alpha = 0.14f), shape) else Modifier)
            .padding(vertical = 6.dp),
    ) {
        Row(Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, top = 6.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            PhoneText(headline, 13f, FerbType.Semibold, ink.secondary, Modifier.weight(1f), maxLines = 1)
            PhoneText(progress, 13f, FerbType.Medium, ink.secondary, Modifier.semantics { liveRegion = LiveRegionMode.Polite })
        }
        parts.forEachIndexed { i, part ->
            val ended = part.kind != MarkKind.RUNNING && part.kind != MarkKind.WAITING
            if (i > 0 && ended && parts[i - 1].kind.let { it != MarkKind.RUNNING && it != MarkKind.WAITING }) Hairline(ink)
            val watch = watching.getOrElse(i) { false }
            if (!ended) {
                LiveLine(part, ink, picture = if (watch) Pic(pictures.getOrNull(i)) else null,
                    onPicture = onPicture?.let { { it(i) } }, onTap = if (!watch) onPart?.let { { it(i) } } else null)
                for (s in part.sub) LiveLine(s, ink, indent = true, picture = null, onPicture = null, onTap = null)
            } else {
                EndedPart(part, ink, more, less)
            }
        }
        if (status != null) {
            Hairline(ink)
            PhoneText(status, 13f, FerbType.Medium, ink.secondary, Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, top = 6.dp, bottom = 2.dp)
                .semantics { liveRegion = LiveRegionMode.Polite }, maxLines = 1)
        }
        if (footer != null || footerLines.isNotEmpty()) {
            Hairline(ink)
            androidx.compose.foundation.text.BasicText(buildAnnotatedString {
                footer?.let { pushStyle(SpanStyle(color = ink.primary, fontWeight = FerbType.Semibold)); append(it); pop() }
                if (footerLines.isNotEmpty()) {
                    pushStyle(SpanStyle(color = ink.secondary)); append((if (footer != null) "   " else "") + footerLines.joinToString("   ")); pop()
                }
            }, style = FerbType.style(14f, color = ink.primary), modifier = Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, top = 8.dp, bottom = 4.dp))
        }
    }
}

private class Pic(val image: ImageBitmap?)

@Composable
private fun Hairline(ink: CardInk) {
    Box(Modifier.fillMaxWidth().padding(horizontal = 14.dp).height(1.dp).background(ink.line))
}

/** A running or waiting part: one line, its mark, label, state words and picture (FERB.app's RowStateLine). */
@Composable
private fun LiveLine(part: PlanPart, ink: CardInk, indent: Boolean = false, picture: Pic?, onPicture: (() -> Unit)?, onTap: (() -> Unit)?) {
    Row(
        Modifier.fillMaxWidth().heightIn(min = 34.dp).pressable(onTap, label = part.title).semantics(mergeDescendants = true) {}
            .padding(start = if (indent) 34.dp else 14.dp, end = 14.dp, top = 3.dp, bottom = 3.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(14.dp), contentAlignment = Alignment.Center) {
            if (part.kind == MarkKind.RUNNING) RunningArc(14.dp, ink.secondary, stroke = 1.6.dp)
            else Canvas(Modifier.size(10.dp).semantics { contentDescription = MarkKind.WAITING.spoken }) {
                drawCircle(ink.faint, radius = size.minDimension / 2 - 0.6.dp.toPx(), style = Stroke(1.2.dp.toPx()))
            }
        }
        val waiting = part.kind == MarkKind.WAITING
        Row(Modifier.weight(1f), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            PhoneText(part.title, 15f, if (waiting) FerbType.Regular else FerbType.Semibold, if (waiting) ink.secondary else ink.primary,
                Modifier.weight(1f, fill = false), maxLines = 1)
            val words = part.detail
            if (!words.isNullOrEmpty()) PhoneText(words, 14f, color = part.tone.color(ink), modifier = Modifier.weight(1f, fill = false), maxLines = 1)
        }
        if (picture != null) {
            // its own live picture (FERB.app's RowPreview: a small circle with the amber ring)
            Box(Modifier.size(24.dp).clip(CircleShape).background(Ink.darkFill).border(1.dp, Ink.amber.copy(alpha = 0.65f), CircleShape)
                .pressable(onPicture, label = "Live preview of ${part.title}")) {
                picture.image?.let { Image(it, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop) }
            }
        }
    }
}

/** A part that ended, asks or failed: its label as a small heading and what it says under it (FERB.app's CardRowView). */
@Composable
private fun EndedPart(part: PlanPart, ink: CardInk, more: String, less: String) {
    Column(Modifier.fillMaxWidth().semantics(mergeDescendants = false) {}.padding(horizontal = 14.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
            PhoneText(part.title, 13f, FerbType.Semibold, ink.secondary, maxLines = 2)
            if (part.updated) PhoneText("UPDATED", 10f, FerbType.Bold, Ink.white, Modifier.background(ink.link, RoundedCornerShape(4.dp)).padding(horizontal = 5.dp))
        }
        part.detail?.let { PhoneText(it, 15f, color = if (part.tone == PartTone.DIM) ink.primary else part.tone.color(ink), lineSpacing = 2f) }
        part.rich?.let { rich ->
            val look = remember(ink.onFerb) { RichLook(15f, ink.primary, ink.secondary, ink.link, ink.fill) }
            RichBlocks(rich, look, part.key, Fold.ROW, more = more, less = less)
        }
        for (n in part.notes) {
            androidx.compose.foundation.text.BasicText(buildAnnotatedString {
                n.label?.let { pushStyle(SpanStyle(color = n.tone.color(ink), fontWeight = FerbType.Semibold)); append(it); pop() }
                pushStyle(SpanStyle(color = if (n.label == null) n.tone.color(ink) else ink.secondary)); append(n.text); pop()
            }, style = FerbType.style(13.5f, color = ink.secondary))
        }
    }
}
