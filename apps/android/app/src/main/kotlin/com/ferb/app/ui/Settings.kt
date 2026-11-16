package com.ferb.app.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.layout
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.ferb.app.FerbViewModel
import com.ferb.app.R
import com.ferb.app.Sheet
import com.ferb.app.UiState
import com.ferb.app.model.MacSettings
import com.ferb.app.model.SettingKind
import com.ferb.app.model.SettingRow
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.components.CardRow
import com.ferb.design.components.Divide
import com.ferb.design.components.Opens
import com.ferb.design.components.PhoneSwitch
import com.ferb.design.components.PhoneText
import com.ferb.design.components.Pill
import com.ferb.design.components.PillKind
import com.ferb.design.components.RowIcon
import com.ferb.design.components.Sheet as SheetFrame
import com.ferb.design.components.SheetTitle
import com.ferb.design.screens.PhoneInsets

/**
 * The lab's "FERB's settings" row (MacPage: "Each change asks for your fingerprint"), as a plain sheet of the lab's
 * rows (ProfileSheet's layout): the switch and the day counts change from here, one fingerprint per change; the two
 * lists are shown as they are, with a line saying they change in FERB on the Mac (PHN-404, PHN-133).
 */
@Composable
fun SettingsSheet(vm: FerbViewModel, ui: UiState, insets: PhoneInsets, close: () -> Unit) {
    val title = stringResource(R.string.settings_title)
    SheetFrame(title = title, bottomInset = insets.bottom, onDismiss = close) {
        SheetTitle(title, close)
        val rows = ui.settings
        if (rows == null) {
            PhoneText(stringResource(R.string.settings_loading), 15.5f, color = Ink.detail, modifier = Modifier.padding(top = 10.dp))
            return@SheetFrame
        }
        Column(Modifier.padding(top = 8.dp).heightIn(max = 560.dp).verticalScroll(rememberScrollState()).outset(14.dp)) {
            rows.filter { it.spec.kind != SettingKind.DAYS }.forEachIndexed { i, r ->
                if (i > 0) Divide()
                SettingLine(vm, ui, r)
            }
            val days = rows.filter { it.spec.kind == SettingKind.DAYS }
            if (days.isNotEmpty()) {
                PhoneText(stringResource(R.string.settings_how_long), 15f, FerbType.Semibold, Ink.detail,
                    Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, top = 18.dp, bottom = 4.dp).semantics { heading() })
                days.forEachIndexed { i, r ->
                    if (i > 0) Divide()
                    SettingLine(vm, ui, r)
                }
            }
        }
        PhoneText(stringResource(R.string.settings_note), 14f, color = Ink.detail, modifier = Modifier.padding(top = 12.dp))
    }
}

@Composable
private fun SettingLine(vm: FerbViewModel, ui: UiState, r: SettingRow) {
    val title = stringResource(r.spec.title)
    val busy = "setting:${r.spec.key}" in ui.busy
    when (r.spec.kind) {
        SettingKind.SWITCH -> CardRow(title, leading = { RowIcon(FerbIcons.eye) },
            trailing = { PhoneSwitch(r.on, if (busy) null else ({ on -> vm.setSwitch(r.spec.key, on) }), title) })
        SettingKind.LIST -> CardRow(title, r.items.joinToString(", ").ifEmpty { stringResource(R.string.set_none) },
            leading = { RowIcon(FerbIcons.lock) })
        SettingKind.DAYS -> CardRow(title, null, onClick = if (busy) null else ({ vm.show(Sheet.Days(r.spec.key)) }),
            trailing = { Opens(r.days?.let { stringResource(R.string.set_days_value, it) }) })
    }
}

/** One day count: typed, then saved after one fingerprint. */
@Composable
fun DaysSheet(vm: FerbViewModel, ui: UiState, key: String, insets: PhoneInsets) {
    val row = ui.settings?.firstOrNull { it.spec.key == key } ?: return
    val spec = MacSettings.spec(key) ?: return
    var text by remember(key) { mutableStateOf(row.days?.toString().orEmpty()) }
    val back = { vm.show(Sheet.Settings) }
    Plain(stringResource(spec.title), stringResource(R.string.set_days_text), insets, back) {
        Field(text, { t -> text = t.filter(Char::isDigit).take(5) }, stringResource(R.string.set_days_hint), Modifier.padding(top = 16.dp),
            KeyboardOptions(keyboardType = KeyboardType.Number, imeAction = ImeAction.Done))
        Pill(stringResource(R.string.set_days_save), Modifier.padding(top = 14.dp),
            onClick = if ("setting:$key" in ui.busy || text.isBlank()) null else ({ vm.setDays(key, text) }))
        Pill(stringResource(R.string.cancel), Modifier.padding(top = 4.dp), PillKind.PLAIN, height = 44.dp, onClick = back)
    }
}

/** ProfileSheet's rows reach the sheet's edges: the sheet's padding, given back. */
internal fun Modifier.outset(by: Dp): Modifier = layout { measurable, constraints ->
    val extra = by.roundToPx() * 2
    val p = measurable.measure(constraints.copy(minWidth = constraints.minWidth + extra, maxWidth = constraints.maxWidth + extra))
    layout(p.width - extra, p.height) { p.place(-by.roundToPx(), 0) }
}
