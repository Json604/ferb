package com.ferb.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.border
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import com.ferb.app.model.Review
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.components.CardInk
import com.ferb.design.components.PhoneText
import com.ferb.design.components.pressable
import com.ferb.mobile.MemoryReview
import com.ferb.mobile.ReviewItem
import com.ferb.mobile.ReviewPart

/**
 * FERB's review of what it learned (kb MEM-460..457, MEM-485, MEM-486), as FERB.app draws it (MemoryReviewList.swift):
 * the list's own buttons on top (Approve all, "Approve 2" while some lines are unticked, Decline all), the line above
 * the items ([showHint]: the sheet; the card's question already says it), then each part's question and the line under
 * it, its groups by topic, and every line with its tick. A tap on a line unticks it (the exception); Approve all gives
 * every ticked line its part's yes and every unticked one its no. Every line shows: Approve all answers only what he
 * can see. Every word is the Mac's. The Mac's reply is the new list ([onAll]'s caller shows it).
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun MemoryReviewList(
    review: MemoryReview,
    unticked: Set<String>,
    busy: Set<String>,
    ink: CardInk,
    showHint: Boolean = false,
    onToggle: (String) -> Unit,
    /** Approve all (true: the ticked lines approved, the unticked declined) or Decline all (false). */
    onAll: (approve: Boolean) -> Unit,
) {
    val sending = Review.ALL in busy
    Column(Modifier.fillMaxWidth().padding(top = 4.dp).testTag("review"), verticalArrangement = Arrangement.spacedBy(14.dp)) {
        if (review.parts.isNotEmpty()) {
            if (showHint && review.hint.isNotEmpty()) PhoneText(review.hint, 13.5f, color = ink.secondary)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Review.approveLabel(review, unticked)?.let { label ->
                    SmallButton(label, ink, primary = true, tag = "review:approve", onClick = if (sending) null else ({ onAll(true) }))
                }
                review.all.getOrNull(1)?.let { b ->
                    SmallButton(b.label, ink, primary = false, tag = "review:decline", onClick = if (sending) null else ({ onAll(false) }))
                }
            }
        }
        for (part in review.parts) ReviewPartView(part, unticked, sending, ink, onToggle)
    }
}

@Composable
private fun ReviewPartView(part: ReviewPart, unticked: Set<String>, sending: Boolean, ink: CardInk, onToggle: (String) -> Unit) {
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            PhoneText(part.title, 15f, FerbType.Semibold, ink.primary)
            if (part.hint.isNotEmpty()) PhoneText(part.hint, 13f, color = ink.secondary)
        }
        for (group in part.groups) {
            Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                if (group.title.isNotEmpty()) PhoneText(group.title, 12.5f, FerbType.Medium, ink.secondary, Modifier.padding(top = 2.dp))
                for (item in group.items) ItemRow(item, item.id !in unticked, sending, ink, onToggle)
            }
        }
    }
}

/** One line: its tick, its words and the small note under them; the whole row is the tick's button. */
@Composable
private fun ItemRow(item: ReviewItem, ticked: Boolean, sending: Boolean, ink: CardInk, onToggle: (String) -> Unit) {
    Row(
        Modifier.fillMaxWidth().background(if (ticked) ink.fill else Color.Transparent, RoundedCornerShape(12.dp))
            .border(1.dp, if (ticked) Color.Transparent else ink.line, RoundedCornerShape(12.dp))
            .pressable(if (sending) null else ({ onToggle(item.id) }), label = item.text)
            .semantics { role = Role.Checkbox; selected = ticked; stateDescription = if (ticked) "Ticked" else "Not ticked" }
            .padding(horizontal = 12.dp, vertical = 10.dp)
            .testTag("review-item:${item.id}"),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Tick(ticked, ink)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            PhoneText(item.text, 14.5f, color = if (ticked) ink.primary else ink.secondary)
            if (item.note.isNotEmpty()) PhoneText(item.note, 12.5f, color = ink.secondary)
        }
    }
}

/** A round tick: filled with a check while ticked, an empty ring once unticked. */
@Composable
private fun Tick(ticked: Boolean, ink: CardInk) {
    Box(
        Modifier.padding(top = 1.dp).size(20.dp)
            .background(if (ticked) ink.primary else Color.Transparent, CircleShape)
            .border(1.5.dp, if (ticked) ink.primary else ink.secondary, CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        if (ticked) FerbIcon(FerbIcons.check, 12f, ink.card)
    }
}

/** A small button of the review: the main one in the card's inverse, the other on its fill. */
@Composable
private fun SmallButton(label: String, ink: CardInk, primary: Boolean, tag: String, onClick: (() -> Unit)?) {
    val bg = if (primary) ink.primary else if (ink.onFerb) Color.White.copy(alpha = 0.14f) else ink.line
    val fg = if (primary) ink.card else ink.primary
    Box(
        Modifier.pressable(onClick, label = label).heightIn(min = 34.dp).background(bg, RoundedCornerShape(50))
            .padding(horizontal = 14.dp, vertical = 6.dp).testTag(tag),
        contentAlignment = Alignment.Center,
    ) {
        PhoneText(label, 13.5f, FerbType.Medium, if (onClick == null) fg.copy(alpha = 0.5f) else fg)
    }
}
