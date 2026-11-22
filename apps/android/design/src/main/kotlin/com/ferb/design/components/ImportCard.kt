package com.ferb.design.components

import androidx.compose.animation.core.Animatable
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.key
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.Motion
import com.ferb.design.rememberReducedMotion

// The memory import's card (kb MEM-456; the owner, MEM-455: "does ferb not show the progress on memory transfer it is
// gettigb??? i think it should show in a good ass UI"): live state at the end of today's chat while FERB brings in what
// the owner said in other assistants, gone when the import ends and FERB's notice arrives. The parts card's sibling
// (PartsCard.kt): the same surface, corner, padding and caption. Every word on it is the Mac's; the phone only draws
// them. Nothing turns or pulses: the one thing that moves is a row's bar, filling to its new share within that row.

/**
 * One source on the card, as the Mac words it (ferb-mobile's ImportRow). [key] stays the same while the row lives: its
 * bar moves only within one key. [name] is said only when the card has more than one row; a bar shows when [total] is
 * above 0, filled [done] of [total] with [count] ("33 of 40") beside it; [foundLine], [detail] and [note] show when not
 * empty, the note (a gap, or a try that didn't go through, and what happens next) set apart in amber.
 */
@Immutable
data class ImportCardRow(
    val key: String,
    val name: String,
    val line: String,
    val detail: String = "",
    val done: Int = 0,
    val total: Int = 0,
    val count: String = "",
    val foundLine: String = "",
    val note: String = "",
) {
    /** How full the bar is, 0 to 1; null: no bar (nothing counted yet, or a website's email still to come). */
    val fraction: Float? get() = if (total > 0) done.coerceIn(0, total).toFloat() / total else null
}

@Composable
fun ImportCard(title: String, rows: List<ImportCardRow>, modifier: Modifier = Modifier) {
    val ink = CardInk.current()
    val shape = RoundedCornerShape(22.dp)
    Column(modifier.fillMaxWidth().background(ink.card, shape).padding(vertical = 6.dp)) {
        PhoneText(title, 13f, FerbType.Semibold, ink.secondary,
            Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, top = 6.dp, bottom = 2.dp).semantics { heading() }, maxLines = 2)
        val named = rows.size > 1
        rows.forEachIndexed { i, row ->
            if (i > 0) Box(Modifier.fillMaxWidth().padding(horizontal = 14.dp).height(1.dp).background(ink.line))
            key(row.key) { ImportRowView(row, named, ink) }
        }
    }
}

@Composable
private fun ImportRowView(row: ImportCardRow, named: Boolean, ink: CardInk) {
    Column(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        if (named && row.name.isNotEmpty()) PhoneText(row.name, 13f, FerbType.Semibold, ink.secondary, maxLines = 1)
        PhoneText(row.line, 15f, FerbType.Semibold, ink.primary, Modifier.semantics { liveRegion = LiveRegionMode.Polite }, lineSpacing = 2f)
        row.fraction?.let { Bar(it, row.count, ink) }
        if (row.foundLine.isNotEmpty()) PhoneText(row.foundLine, 13.5f, FerbType.Medium, ink.secondary)
        if (row.detail.isNotEmpty()) PhoneText(row.detail, 13.5f, color = ink.faint, lineSpacing = 2f)
        if (row.note.isNotEmpty()) Note(row.note, ink)
    }
}

/**
 * A slim capsule track with a soft fill and the Mac's count beside it. The fill eases to a new share (Motion.progress)
 * and is drawn, never composed, on each frame; with the system's motion off it is there at once.
 */
@Composable
private fun Bar(fraction: Float, count: String, ink: CardInk) {
    val reduced = rememberReducedMotion()
    val shown = remember { Animatable(fraction) }
    LaunchedEffect(fraction, reduced) { if (reduced) shown.snapTo(fraction) else shown.animateTo(fraction, Motion.progress()) }
    val track = if (ink.onFerb) Ink.white.copy(alpha = 0.14f) else Ink.black.copy(alpha = 0.07f)
    val fill = if (ink.onFerb) Ink.white.copy(alpha = 0.9f) else Ink.black.copy(alpha = 0.82f)
    Row(Modifier.fillMaxWidth().padding(top = 6.dp, bottom = 2.dp), horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(
            Modifier.weight(1f).height(6.dp).clip(CircleShape).background(track)
                .semantics {
                    progressBarRangeInfo = ProgressBarRangeInfo(fraction, 0f..1f)
                    if (count.isNotEmpty()) stateDescription = count
                }
                .drawBehind {
                    val w = size.width * shown.value.coerceIn(0f, 1f)
                    if (w > 0f) drawRoundRect(fill, size = Size(maxOf(w, size.height), size.height), cornerRadius = CornerRadius(size.height / 2))
                },
        )
        if (count.isNotEmpty()) {
            // figures of one width: the count keeps its place as it climbs
            BasicText(count, Modifier.clearAndSetSemantics {},
                style = FerbType.style(13f, FerbType.Medium, ink.secondary).copy(fontFeatureSettings = "tnum"), maxLines = 1)
        }
    }
}

/** What didn't go through and what FERB does next, set apart in amber (the tone of "Not done" lines). */
@Composable
private fun Note(text: String, ink: CardInk) {
    Row(
        Modifier.padding(top = 4.dp).fillMaxWidth().background(Ink.amber.copy(alpha = if (ink.onFerb) 0.13f else 0.16f), RoundedCornerShape(12.dp))
            .padding(horizontal = 10.dp, vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Box(Modifier.padding(top = 6.dp).size(6.dp).background(ink.amberText, CircleShape))
        PhoneText(text, 13.5f, FerbType.Medium, ink.amberText, lineSpacing = 2f)
    }
}
