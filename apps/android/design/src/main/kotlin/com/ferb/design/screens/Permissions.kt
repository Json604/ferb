package com.ferb.design.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.ferb.design.FerbIcon
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.components.Card
import com.ferb.design.components.PhoneText
import com.ferb.design.components.Pill
import com.ferb.design.components.pressable

/** One thing the first run asks the phone for: its icon, its name and why FERB wants it, in a line. */
data class AskRow(val icon: ImageVector, val title: String, val why: String)

/**
 * The first run's asks (kb PHN-636; not from the lab, built from its pair page's pieces: the white card at the bottom
 * over the sky, PairStep's grey circles, the black pill): what FERB will ask the phone for and why, a line each, one
 * Continue that brings the system's questions one after another, and Not now. Stateless: the app passes its words.
 */
@Composable
fun PermissionsPage(
    title: String,
    intro: String,
    rows: List<AskRow>,
    continueTitle: String,
    notNowTitle: String,
    insets: PhoneInsets = PhoneInsets.Lab,
    onContinue: (() -> Unit)? = {},
    onNotNow: (() -> Unit)? = {},
) {
    Column(Modifier.fillMaxSize()) {
        Spacer(Modifier.weight(1f))
        Card(Modifier.paddingOverBottom(insets, 8.dp, start = 16.dp, end = 16.dp), padding = 20.dp) {
            Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                PhoneText(title, 24f, FerbType.Semibold, modifier = Modifier.semantics { heading() })
                PhoneText(intro, 15.5f, color = Ink.detail, lineSpacing = 2f)
                for (row in rows) AskLine(row)
                Pill(continueTitle, Modifier.padding(top = 4.dp), onClick = onContinue)
                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                    PhoneText(notNowTitle, 15f, FerbType.Medium, Ink.detail,
                        Modifier.pressable(onNotNow, label = notNowTitle).padding(horizontal = 16.dp, vertical = 6.dp))
                }
            }
        }
    }
}

@Composable
private fun AskLine(row: AskRow) {
    Row(Modifier.semantics(mergeDescendants = true) {}, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Box(Modifier.size(34.dp).background(Ink.fill, CircleShape), contentAlignment = Alignment.Center) {
            FerbIcon(row.icon, 15f, Ink.black)
        }
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            PhoneText(row.title, 15.5f, FerbType.Semibold)
            PhoneText(row.why, 14f, color = Ink.detail, lineSpacing = 2f)
        }
    }
}
