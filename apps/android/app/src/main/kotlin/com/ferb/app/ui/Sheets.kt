package com.ferb.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicSecureTextField
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.input.TextFieldState
import androidx.compose.foundation.text.input.TextObfuscationMode
import androidx.compose.foundation.text.input.clearText
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import com.ferb.app.FerbViewModel
import com.ferb.app.PhoneSettings
import com.ferb.app.R
import com.ferb.app.Sheet
import com.ferb.app.UiState
import com.ferb.app.model.Question
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.Phone
import com.ferb.design.components.CardRow
import com.ferb.design.components.Divide
import com.ferb.design.components.ExactCommand
import com.ferb.design.components.PhoneSwitch
import com.ferb.design.components.PhoneText
import com.ferb.design.components.Pill
import com.ferb.design.components.PillKind
import com.ferb.design.components.RowIcon
import com.ferb.design.components.SecretCodeBoxes
import com.ferb.design.components.SecretKeypad
import com.ferb.design.components.Sheet as SheetFrame
import com.ferb.design.components.SheetIcon
import com.ferb.design.components.SheetTitle
import com.ferb.design.preview.labRelay
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.ProfileSheet
import com.ferb.design.screens.QuestionSheet
import com.ferb.design.screens.RelaySheet
import com.ferb.mobile.AnswerInput
import java.text.DateFormat
import java.util.Date

/** What the sheets ask of the activity: the switches that act on Android itself. */
interface Host {
    fun stayConnected(on: Boolean)
    fun blockScreenshots(on: Boolean)
    fun askNotifications()

    /** The system's picker for files to send to the Mac ([forChat]: they go with the next message). */
    fun pickFiles(forChat: Boolean)

    /** The owner's tap on a fetched file: the system's Open with or Share sheet. */
    fun handOut(key: String, file: java.io.File, name: String, share: Boolean)
}

/** The sheet over the page (Mobile.swift Sheet; MobileScreens.swift's sheets), or nothing. */
@Composable
fun Sheets(vm: FerbViewModel, ui: UiState, sheet: Sheet?, insets: PhoneInsets, host: Host) {
    val close = { vm.show(null) }
    when (sheet) {
        null -> Unit
        is Sheet.Asking -> {
            val q = ui.questions.firstOrNull { it.approvalId == sheet.approvalId } ?: return
            val busy = "answer:${q.approvalId}" in ui.busy
            when {
                q.onlyOnMac || q.held -> Plain(q.title ?: stringResource(R.string.question_default_title),
                    listOfNotNull(q.reason.ifEmpty { null }, q.needs?.problem, stringResource(R.string.question_on_mac).takeIf { q.onlyOnMac })
                        .joinToString("\n\n").ifEmpty { stringResource(R.string.n_held_text) }, insets, close)
                q.hidden -> PrivateAnswer(vm, q, insets, busy, close)
                else -> Asking(vm, q, insets, busy, close)
            }
        }
        Sheet.Relay -> RelaySheet(labRelay, insets, onClose = close)
        Sheet.Profile -> Profile(vm, ui, insets, host, close)
        Sheet.StopAll -> Plain(stringResource(R.string.stop_all_title), stringResource(R.string.stop_all_text), insets, close) {
            Pill(stringResource(R.string.stop_all_yes), Modifier.padding(top = 18.dp), icon = FerbIcons.stop,
                onClick = if ("stop-all" in ui.busy) null else ({ vm.stopEverything() }))
            Pill(stringResource(R.string.cancel), Modifier.padding(top = 4.dp), PillKind.PLAIN, height = 44.dp, onClick = close)
        }
        Sheet.Settings -> SettingsSheet(vm, ui, insets, close)
        is Sheet.Days -> DaysSheet(vm, ui, sheet.key, insets)
        is Sheet.Live -> LiveScreen(vm, insets)
        Sheet.Forget -> Plain(stringResource(R.string.forget_title), stringResource(R.string.forget_text), insets, close) {
            Pill(stringResource(R.string.forget_yes), Modifier.padding(top = 18.dp), onClick = { vm.forget() })
            Pill(stringResource(R.string.cancel), Modifier.padding(top = 4.dp), PillKind.PLAIN, height = 44.dp, onClick = close)
        }
        Sheet.TypeCode -> TypeCode(vm, insets, close)
        Sheet.Scan -> Unit // a page of its own (Root's Pair), not a sheet
        Sheet.OwnRelay -> {
            val url = ui.link?.ownRelayUrl
            Plain(stringResource(R.string.mac_own_relay_title),
                if (url != null) stringResource(R.string.mac_own_relay_set, host(url)) else stringResource(R.string.mac_own_relay_none), insets, close)
        }
        Sheet.Notifications -> {
            Plain(stringResource(R.string.notify_title), stringResource(R.string.notify_text), insets, close) {
                Pill(stringResource(R.string.notify_allow), Modifier.padding(top = 18.dp), onClick = host::askNotifications)
                Pill(stringResource(R.string.cancel), Modifier.padding(top = 4.dp), PillKind.PLAIN, height = 44.dp, onClick = close)
            }
        }
        is Sheet.NotYet -> Plain(stringResource(sheet.what.words), stringResource(sheet.what.text), insets, close)
        Sheet.Help -> Plain(stringResource(R.string.help_title), stringResource(R.string.help_text), insets, close)
    }
}

/** A sheet with a title, a close button, its words and what else it holds (the relay and profile sheets' frame). */
@Composable
internal fun Plain(title: String, text: String, insets: PhoneInsets, onClose: () -> Unit, more: @Composable () -> Unit = {}) {
    SheetFrame(title = title, bottomInset = insets.bottom, onDismiss = onClose) {
        SheetTitle(title, onClose)
        PhoneText(text, 15.5f, color = Ink.detail, lineSpacing = 2f, modifier = Modifier.padding(top = 6.dp))
        more()
    }
}

/**
 * MobileScreens.swift QuestionSheet: the question, why, the exact thing, and its answers. Two answers are the lab's
 * No and Yes; more are a pill each; none is the owner's own words. "Yes asks for your fingerprint" is said only where
 * the phone knows it: the Mac decides (PHN-470), and the prompt shows when it asks.
 */
@Composable
private fun Asking(vm: FerbViewModel, q: Question, insets: PhoneInsets, busy: Boolean, close: () -> Unit) {
    val title = q.title ?: stringResource(R.string.question_default_title)
    val choices = q.choices
    val exact: @Composable () -> Unit = { q.exact?.let { ExactCommand(it) } }
    if (choices.size == 2) {
        val (no, yes) = choices[0] to choices[1]
        // the lab's own line, said only where the Mac's status says the Yes needs the fingerprint-only key
        val note = if (q.needsProof && q.needs?.fingerprint == true) stringResource(R.string.question_yes_fingerprint) else null
        QuestionSheet(
            FerbIcons.help, title, q.reason, yes.label, insets, no = no.label, note = note, onDismiss = close,
            onNo = if (busy) null else ({ vm.answer(q.approvalId, no.input) }),
            onYes = if (busy) null else ({ vm.answer(q.approvalId, yes.input) }),
            exact = exact,
        )
        return
    }
    SheetFrame(title = title, bottomInset = insets.bottom, onDismiss = close) {
        Row(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            SheetIcon(FerbIcons.help)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                PhoneText(title, 22f, FerbType.Semibold)
                PhoneText(q.reason, 15.5f, color = Ink.detail)
            }
        }
        Box(Modifier.padding(top = 14.dp)) { exact() }
        if (choices.isNotEmpty()) {
            Column(Modifier.padding(top = 18.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                choices.forEachIndexed { i, c ->
                    Pill(c.label, kind = if (i == 0) PillKind.BLACK else PillKind.GREY, onClick = if (busy) null else ({ vm.answer(q.approvalId, c.input) }))
                }
            }
        } else {
            var text by remember(q.approvalId) { mutableStateOf("") }
            Field(text, { text = it }, stringResource(R.string.question_answer_hint), Modifier.padding(top = 18.dp))
            Row(Modifier.fillMaxWidth().padding(top = 12.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Pill(stringResource(R.string.question_skip), kind = PillKind.GREY, fill = false,
                    onClick = if (busy) null else ({ vm.answer(q.approvalId, AnswerInput.Words("")) }))
                Pill(stringResource(R.string.question_send), Modifier.weight(1f),
                    onClick = if (busy || text.isBlank()) null else ({ vm.answer(q.approvalId, AnswerInput.Words(text.trim())); text = "" }))
            }
        }
    }
}

/**
 * MobileScreens.swift SecretSheet (PHN-109, PHN-110): the value goes straight to the Mac on the sealed path, signed
 * after the prompt, and is kept nowhere: not in saved state (plain remember, never rememberSaveable), not in a log,
 * and wiped from the field when sent or when the sheet goes; the screen stays out of screenshots while it is open
 * (FLAG_SECURE, set by the root). A code takes the lab's own keypad, so the system keyboard never sees it (a keyboard
 * app may learn or sync what is typed); a password or other secret needs letters, so it takes the system keyboard in
 * its password mode, which keyboards neither learn from nor suggest. kb PHN-502.
 */
@Composable
private fun PrivateAnswer(vm: FerbViewModel, q: Question, insets: PhoneInsets, busy: Boolean, close: () -> Unit) {
    val title = q.title ?: stringResource(R.string.private_title_default)
    if (q.isCode) {
        val digits = remember(q.approvalId) { CharArray(MAX_CODE) }
        var count by remember(q.approvalId) { mutableIntStateOf(0) }
        DisposableEffect(q.approvalId) { onDispose { digits.fill('\u0000'); count = 0 } }
        val send: () -> Unit = {
            val value = String(digits, 0, count)
            digits.fill('\u0000'); count = 0
            vm.answer(q.approvalId, AnswerInput.Hidden(value), hidden = true)
        }
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
            SheetFrame(title = title, bottomInset = insets.bottom, lift = (4 * 57 + 18).dp + insets.bottom, onDismiss = close) {
                PrivateHead(title, q.reason)
                SecretCodeBoxes(count, Modifier.padding(top = 16.dp), length = maxOf(6, minOf(MAX_CODE, count + 1)))
                PrivateNote()
                Row(Modifier.fillMaxWidth().padding(top = 14.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Pill(stringResource(R.string.private_decline), kind = PillKind.GREY, fill = false, height = 46.dp,
                        onClick = if (busy) null else ({ vm.answer(q.approvalId, AnswerInput.Words("")) }))
                    Pill(stringResource(R.string.private_send), Modifier.weight(1f), height = 46.dp, onClick = if (busy || count == 0) null else send)
                }
            }
            SecretKeypad(bottomInset = insets.bottom, onKey = { k ->
                if (k == "⌫") { if (count > 0) { count--; digits[count] = '\u0000' } } else if (count < MAX_CODE) { digits[count] = k[0]; count++ }
            })
        }
        return
    }
    val field = remember(q.approvalId) { TextFieldState() }
    DisposableEffect(q.approvalId) { onDispose { field.clearText() } }
    SheetFrame(title = title, bottomInset = insets.bottom, onDismiss = close) {
        PrivateHead(title, q.reason)
        Box(Modifier.padding(top = 16.dp).fillMaxWidth().heightIn(min = Phone.secretBoxHeight).background(Ink.fill, RoundedCornerShape(Phone.secretBoxCorner)).padding(horizontal = 16.dp),
            contentAlignment = Alignment.CenterStart) {
            BasicSecureTextField(
                field, Modifier.fillMaxWidth().semantics { contentDescription = "Private answer" },
                textStyle = FerbType.style(18f, color = Ink.black), textObfuscationMode = TextObfuscationMode.Hidden,
                keyboardOptions = KeyboardOptions(KeyboardCapitalization.None, autoCorrectEnabled = false, keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
                cursorBrush = SolidColor(Ink.black),
            )
            if (field.text.isEmpty()) PhoneText(stringResource(R.string.private_field), 16f, color = Ink.faint)
        }
        PrivateNote()
        Row(Modifier.fillMaxWidth().padding(top = 14.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Pill(stringResource(R.string.private_decline), kind = PillKind.GREY, fill = false,
                onClick = if (busy) null else ({ field.clearText(); vm.answer(q.approvalId, AnswerInput.Words("")) }))
            Pill(stringResource(R.string.private_send), Modifier.weight(1f), onClick = if (busy || field.text.isEmpty()) null else ({
                val value = field.text.toString()
                field.clearText()
                vm.answer(q.approvalId, AnswerInput.Hidden(value), hidden = true)
            }))
        }
    }
}

private const val MAX_CODE = 10

@Composable
private fun PrivateHead(title: String, detail: String) {
    Row(horizontalArrangement = Arrangement.spacedBy(14.dp), verticalAlignment = Alignment.CenterVertically) {
        SheetIcon(FerbIcons.key)
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            PhoneText(title, 19f, FerbType.Semibold)
            if (detail.isNotEmpty()) PhoneText(detail, 15f, color = Ink.detail)
        }
    }
}

@Composable
private fun PrivateNote() {
    Row(Modifier.padding(top = 12.dp).semantics(mergeDescendants = true) {}, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        FerbIcon(FerbIcons.lockFill, 14f, Ink.detail, Modifier.padding(top = 1.dp))
        PhoneText("Private: it goes straight to your Mac and is not kept on this phone.", 14f, color = Ink.detail)
    }
}

/** A grey field for typed words (the type-code sheet, a question's answer). */
@Composable
fun Field(value: String, onChange: (String) -> Unit, hint: String, modifier: Modifier = Modifier, keyboard: KeyboardOptions = KeyboardOptions.Default) {
    Box(modifier.fillMaxWidth().heightIn(min = Phone.secretBoxHeight).background(Ink.fill, RoundedCornerShape(Phone.secretBoxCorner)).padding(horizontal = 16.dp, vertical = 12.dp),
        contentAlignment = Alignment.CenterStart) {
        BasicTextField(value, onChange, Modifier.fillMaxWidth().semantics { contentDescription = hint },
            textStyle = FerbType.style(16f, color = Ink.black), cursorBrush = SolidColor(Ink.black), keyboardOptions = keyboard, maxLines = 4)
        if (value.isEmpty()) PhoneText(hint, 16f, color = Ink.faint)
    }
}

@Composable
private fun TypeCode(vm: FerbViewModel, insets: PhoneInsets, close: () -> Unit) {
    var code by remember { mutableStateOf("") }
    Plain(stringResource(R.string.type_code_title), stringResource(R.string.type_code_text), insets, close) {
        Field(code, { code = it }, stringResource(R.string.type_code_hint), Modifier.padding(top = 16.dp),
            KeyboardOptions(KeyboardCapitalization.Characters, autoCorrectEnabled = false, keyboardType = KeyboardType.Ascii, imeAction = ImeAction.Done))
        Pill(stringResource(R.string.type_code_pair), Modifier.padding(top = 14.dp), onClick = if (code.isBlank()) null else ({ vm.pair(code) }))
    }
}

/** MobileScreens.swift ProfileSheet with this phone's switches (PHN-8, PHN-420, PHN-421, PHN-422) working. */
@Composable
private fun Profile(vm: FerbViewModel, ui: UiState, insets: PhoneInsets, host: Host, close: () -> Unit) {
    val s: PhoneSettings by vm.settings.collectAsState()
    // the Mac's own date for this phone's pairing (phone.list), else the day this phone first saw it paired
    val pairedAt = ui.mac?.pairedAt?.let { runCatching { java.time.OffsetDateTime.parse(it).toInstant().toEpochMilli() }.getOrNull() }
        ?: s.pairedSince.takeIf { it > 0 }
    val since = pairedAt?.let { DateFormat.getDateInstance(DateFormat.LONG).format(Date(it)) } ?: "—"
    ProfileSheet(
        ui.mac?.name ?: stringResource(R.string.profile_mac_name), since, insets,
        lockOn = true, // PHN-8: FERB always opens with the fingerprint or the screen lock; there is no off
        stayConnected = s.stayConnected,
        onClose = close,
        onStayConnected = host::stayConnected,
        onForget = { vm.show(Sheet.Forget) },
        more = {
            Divide()
            CardRow(stringResource(R.string.profile_screenshots), stringResource(R.string.profile_screenshots_detail), leading = { RowIcon(FerbIcons.eye) },
                trailing = { PhoneSwitch(s.blockScreenshots, { host.blockScreenshots(it); vm.refreshProfile() }, stringResource(R.string.profile_screenshots)) })
            Divide()
            CardRow(stringResource(R.string.profile_lock_screen), stringResource(R.string.profile_lock_screen_detail), leading = { RowIcon(FerbIcons.bell) },
                trailing = { PhoneSwitch(s.hideChecks, vm::hideChecks, stringResource(R.string.profile_lock_screen)) })
            if (ui.newKeys) {
                Divide()
                CardRow(stringResource(R.string.keys_row), stringResource(R.string.keys_row_detail), leading = { RowIcon(FerbIcons.fingerprint) },
                    onClick = if ("keys" in ui.busy) null else ({ vm.registerKeys() }), trailing = { com.ferb.design.components.Opens() })
            }
        },
    )
}
