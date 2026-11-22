package com.ferb.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.ferb.app.R
import com.ferb.app.model.Review
import com.ferb.design.FerbType
import com.ferb.design.components.CardInk
import com.ferb.design.components.PhoneText
import com.ferb.design.components.pressable
import com.ferb.mobile.MemoryReview
import com.ferb.mobile.ReviewButton
import com.ferb.mobile.ReviewGroup
import com.ferb.mobile.ReviewPart

/**
 * FERB's review of what an import brought in (kb MEM-460..457), inside its question's box as FERB.app draws it
 * (MemoryReviewList.swift): each part's question and the line under it, its groups, each item with its note and its
 * buttons (its own when it has them, as an item already always on does: Keep / Remove; else the part's), a group's own
 * buttons for all of its items when it has them (MEM-464), and the part's buttons for all of them at once (shown when
 * the part has more than one item). Every word is the Mac's; the app adds only "Show all N" / "Show less" for a group
 * of more than [Review.SHOWN_AT_FIRST].
 * An answered item leaves the list when the Mac's reply comes back ([onAnswer]'s caller shows it).
 */
@Composable
fun MemoryReviewList(
    review: MemoryReview,
    busy: Set<String>,
    ink: CardInk,
    /** An answer to [ids] of [part]; [key] is its `ui.busy` key ([Review.key], [Review.groupKey]). */
    onAnswer: (part: String, ids: List<String>, answer: String, key: String) -> Unit,
) {
    Column(Modifier.fillMaxWidth().padding(top = 4.dp).testTag("review"), verticalArrangement = Arrangement.spacedBy(14.dp)) {
        for (part in review.parts) ReviewPartView(part, busy, ink, onAnswer)
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ReviewPartView(part: ReviewPart, busy: Set<String>, ink: CardInk, onAnswer: (String, List<String>, String, String) -> Unit) {
    val ids = Review.ids(part)
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            PhoneText(part.title, 15f, FerbType.Semibold, ink.primary)
            if (part.hint.isNotEmpty()) PhoneText(part.hint, 13f, color = ink.secondary)
        }
        for (group in part.groups) ReviewGroupView(part, group, busy, ink, onAnswer)
        if (ids.size > 1 && part.all.isNotEmpty()) {
            val key = Review.key(part.part, null)
            AllButtons(part.all, ink, "review:${part.part}:all", sending = key in busy, part = true) { b -> onAnswer(part.part, ids, b.answer, key) }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ReviewGroupView(part: ReviewPart, group: ReviewGroup, busy: Set<String>, ink: CardInk, onAnswer: (String, List<String>, String, String) -> Unit) {
    var open by remember(part.part, group.title) { mutableStateOf(false) }
    val shown = if (open) group.items else group.items.take(Review.SHOWN_AT_FIRST)
    val groupKey = Review.groupKey(part.part, group.title)
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        if (group.title.isNotEmpty()) PhoneText(group.title, 12.5f, FerbType.Medium, ink.secondary, Modifier.padding(top = 2.dp))
        for (item in shown) {
            val key = Review.key(part.part, item.id)
            val sending = key in busy || Review.key(part.part, null) in busy || groupKey in busy
            Column(
                Modifier.fillMaxWidth().background(ink.fill, RoundedCornerShape(12.dp)).padding(horizontal = 12.dp, vertical = 10.dp)
                    .testTag("review-item:${item.id}"),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                PhoneText(item.text, 14.5f, color = ink.primary)
                if (item.note.isNotEmpty()) PhoneText(item.note, 12.5f, color = ink.secondary)
                FlowRow(Modifier.padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    for (b in item.answers.ifEmpty { part.answers }) {
                        SmallButton(b, ink, primary = false, tag = "review:${part.part}:${item.id}:${b.answer}",
                            onClick = if (sending) null else ({ onAnswer(part.part, listOf(item.id), b.answer, key) }))
                    }
                }
            }
        }
        if (group.items.size > Review.SHOWN_AT_FIRST) {
            PhoneText(
                if (open) stringResource(R.string.show_less) else stringResource(R.string.review_show_all, group.items.size),
                13.5f, FerbType.Medium, ink.primary,
                Modifier.pressable({ open = !open }).padding(vertical = 4.dp),
            )
        }
        // the group's own buttons for all of its items (Keep all / Remove all, MEM-464); usually none
        if (group.all.isNotEmpty()) {
            val ids = group.items.map { it.id }
            AllButtons(group.all, ink, "review:${part.part}:group:${group.title}", sending = groupKey in busy || Review.key(part.part, null) in busy) { b ->
                onAnswer(part.part, ids, b.answer, groupKey)
            }
        }
    }
}

/**
 * Buttons for all of a part's or a group's items at once. A part's sit a little apart with the first in the card's
 * inverse; a group's are quiet, so they read as that group's, not the whole part's.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun AllButtons(buttons: List<ReviewButton>, ink: CardInk, tag: String, sending: Boolean, part: Boolean = false, onClick: (ReviewButton) -> Unit) {
    FlowRow(Modifier.padding(top = if (part) 6.dp else 0.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        buttons.forEachIndexed { i, b ->
            SmallButton(b, ink, primary = part && i == 0, tag = "$tag:${b.answer}", onClick = if (sending) null else ({ onClick(b) }))
        }
    }
}

/** A small button of the review: the first of a part's "all" buttons in the card's inverse, the rest on its fill. */
@Composable
private fun SmallButton(b: ReviewButton, ink: CardInk, primary: Boolean, tag: String, onClick: (() -> Unit)?) {
    val bg = if (primary) ink.primary else if (ink.onFerb) Color.White.copy(alpha = 0.14f) else ink.line
    val fg = if (primary) ink.card else ink.primary
    androidx.compose.foundation.layout.Box(
        Modifier.pressable(onClick, label = b.label).heightIn(min = 32.dp).background(bg, RoundedCornerShape(50))
            .padding(horizontal = 12.dp, vertical = 6.dp).testTag(tag),
        contentAlignment = Alignment.Center,
    ) {
        PhoneText(b.label, 13.5f, FerbType.Medium, if (onClick == null) fg.copy(alpha = 0.5f) else fg)
    }
}
