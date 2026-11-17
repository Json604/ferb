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
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.LocationOn
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import com.ferb.design.components.pressable
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
import com.ferb.app.model.SpeechKind
import com.ferb.mobile.AnswerInput
import java.text.DateFormat
import java.util.Date

/** What the sheets ask of the activity: the switches that act on Android itself. */
interface Host {
    fun stayConnected(on: Boolean)
    fun blockScreenshots(on: Boolean)
    fun askNotifications()

    /** The one tap that lets FERB's questions reach this phone again (kb PHN-765): Android's page, or Stay connected. */
    fun fixNotifications(b: com.ferb.app.NotifyBlocker) {}

    /** The system's picker for files to send to the Mac ([forChat]: they go with the next message). */
    fun pickFiles(forChat: Boolean)

    /** The owner's tap on a fetched file: the system's Open with or Share sheet. */
    fun handOut(key: String, file: java.io.File, name: String, share: Boolean)

    /** The system's folder picker (ACTION_OPEN_DOCUMENT_TREE, no storage permission): a folder to send whole. */
    fun pickFolder(forChat: Boolean) {}

    /** A fetched folder's files, shared together (a folder can't be opened in another app). */
    fun shareMany(files: List<java.io.File>) {}

    /** The owner's switch for sharing this phone's city (SENSE-420): Android's own question when turned on. */
    fun shareCity(on: Boolean) {}

    /** The owner said yes to the Mac's request for the exact location: Android's question if needed, then the fix. */
    fun shareExact(requestId: String) {}

    /** Android allowed FERB the microphone (speaking to FERB, PHN-604). */
    fun microphoneAllowed(): Boolean = false

    /** The owner read FERB's words about speaking ([kind]; null: he had already) and goes on: Android's question if needed. */
    fun allowMicrophone(kind: com.ferb.app.model.SpeechKind?) {}
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
        Sheet.Connection -> ConnectionSheetFor(vm, ui, insets, close)
        Sheet.Profile -> Profile(vm, ui, insets, host, close)
        Sheet.StopAll -> Plain(stringResource(R.string.stop_all_title), stringResource(R.string.stop_all_text), insets, close) {
            Pill(stringResource(R.string.stop_all_yes), Modifier.padding(top = 18.dp), icon = FerbIcons.stop,
                onClick = if ("stop-all" in ui.busy) null else ({ vm.stopEverything() }))
            Pill(stringResource(R.string.cancel), Modifier.padding(top = 4.dp), PillKind.PLAIN, height = 44.dp, onClick = close)
        }
        Sheet.Settings -> SettingsSheet(vm, ui, insets, close)
        is Sheet.Days -> DaysSheet(vm, ui, sheet.key, insets)
        is Sheet.Live -> LiveScreen(vm, insets)
        // a running task's short view (kb PHN-725; FERB.app's TaskShortView): its picture, title, line, Watch live, Stop
        is Sheet.Task -> {
            val r = ui.running?.tasks?.firstOrNull { it.taskId == sheet.taskId }
            val shown by vm.live.view.collectAsState()
            val picture = shown?.frame?.let { androidx.compose.runtime.remember(it) { it.asImageBitmap() } }
            SheetFrame(title = r?.title ?: "", bottomInset = insets.bottom, onDismiss = close) {
                SheetTitle(r?.title ?: stringResource(R.string.question_default_title), close)
                com.ferb.design.components.TaskShortView(
                    com.ferb.design.components.WorkTask(sheet.taskId, r?.title ?: "", shown?.line ?: r?.line ?: "", r?.waiting == true, r?.paused == true, picture),
                    empty = shown?.ended?.let { noteText(it) } ?: shown?.empty,
                    onWatch = { vm.watchLive(sheet.taskId) }, onStop = { vm.stop(sheet.taskId) },
                )
            }
        }
        is Sheet.Sites -> {
            val sites = ui.rows.firstNotNullOfOrNull { (it as? com.ferb.app.model.ChatRow.Answer)?.takeIf { a -> a.key == sheet.row }?.ui?.sites }.orEmpty()
            val uri = androidx.compose.ui.platform.LocalUriHandler.current
            SheetFrame(title = stringResource(R.string.sites_visited), bottomInset = insets.bottom, onDismiss = close) {
                SheetTitle(stringResource(R.string.sites_visited), close)
                com.ferb.design.components.Card {
                    sites.forEachIndexed { i, h ->
                        if (i > 0) com.ferb.design.components.Divide()
                        com.ferb.design.components.CardRow(h, onClick = { com.ferb.app.model.Present.siteLink(h)?.let { runCatching { uri.openUri(it) } } },
                            leading = { com.ferb.design.FerbIcon(com.ferb.design.components.CardIcons.globe, 20f, Ink.black) },
                            trailing = { com.ferb.design.components.Opens() })
                    }
                }
            }
        }
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
        is Sheet.CantNotify -> Plain(stringResource(R.string.cant_notify_title), stringResource(sheet.blocker.words), insets, close) {
            Pill(stringResource(sheet.blocker.fix), Modifier.padding(top = 18.dp), onClick = { close(); host.fixNotifications(sheet.blocker) })
            Pill(stringResource(R.string.first_run_not_now), Modifier.padding(top = 4.dp), PillKind.PLAIN, height = 44.dp, onClick = close)
        }
        Sheet.Notifications -> {
            Plain(stringResource(R.string.notify_title), stringResource(R.string.notify_text), insets, close) {
                Pill(stringResource(R.string.notify_allow), Modifier.padding(top = 18.dp), onClick = host::askNotifications)
                Pill(stringResource(R.string.cancel), Modifier.padding(top = 4.dp), PillKind.PLAIN, height = 44.dp, onClick = close)
            }
        }
        is Sheet.Voice -> {
            val text = stringResource(if (sheet.kind == SpeechKind.ON_DEVICE) R.string.voice_text_device else R.string.voice_text_service)
            val ask = if (host.microphoneAllowed()) "" else "\n\n" + stringResource(R.string.voice_ask_next)
            Plain(stringResource(R.string.voice_title), text + ask, insets, close) {
                Pill(stringResource(R.string.voice_continue), Modifier.padding(top = 18.dp), icon = FerbIcons.mic, onClick = { host.allowMicrophone(sheet.kind) })
                Pill(stringResource(R.string.cancel), Modifier.padding(top = 4.dp), PillKind.PLAIN, height = 44.dp, onClick = close)
            }
        }
        is Sheet.Location -> {
            val busy = "location" in ui.busy
            Plain(stringResource(R.string.exact_title), stringResource(if (busy) R.string.exact_reading else R.string.exact_text), insets,
                onClose = { vm.declineLocation(sheet.requestId) }) {
                Pill(stringResource(R.string.exact_share), Modifier.padding(top = 18.dp), onClick = if (busy) null else ({ host.shareExact(sheet.requestId) }))
                Pill(stringResource(R.string.exact_no), Modifier.padding(top = 4.dp), PillKind.PLAIN, height = 44.dp,
                    onClick = if (busy) null else ({ vm.declineLocation(sheet.requestId) }))
            }
        }
        Sheet.City -> Plain(stringResource(R.string.city_title), stringResource(R.string.city_text), insets, close) {
            Pill(stringResource(R.string.city_continue), Modifier.padding(top = 18.dp), onClick = { host.shareCity(true) })
            Pill(stringResource(R.string.cancel), Modifier.padding(top = 4.dp), PillKind.PLAIN, height = 44.dp, onClick = { vm.show(Sheet.Profile) })
        }
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
    // FERB.app's card (kb PHN-739; ApprovalView.swift): header, the one question, why it asks, the exact thing, the
    // task's reason and the evidence, the buttons' words and the note under them; the phone adds only its own lines
    val m = q.shown
    val title = m?.question ?: q.title ?: stringResource(R.string.question_default_title)
    val text = if (m != null) listOfNotNull(m.pictureProblem, m.detail).joinToString("\n\n") else q.reason
    val choices = q.choices
    val exact: @Composable () -> Unit = { (m?.exact ?: q.exact)?.let { ExactCommand(it) } }
    val below: @Composable () -> Unit = { if (m != null) MacBelow(m) }
    if (choices.size == 2 || q.yesNo) {
        val (no, yes) = choices[0] to choices[1]
        // the lab's own line, said only where the Mac's status says the Yes needs the fingerprint-only key
        val note = if (q.needsProof && q.needs?.fingerprint == true) stringResource(R.string.question_yes_fingerprint) else null
        QuestionSheet(
            FerbIcons.help, title, text, yes.label, insets, no = no.label, note = note, onDismiss = close,
            header = m?.header?.ifEmpty { null }, below = below, answerNote = m?.note,
            onNo = if (busy) null else ({ vm.answer(q.approvalId, no.input) }),
            onYes = if (busy) null else ({ vm.answer(q.approvalId, yes.input) }),
            more = choices.drop(2).map { c -> c.label to (if (busy) null else ({ vm.answer(q.approvalId, c.input); Unit })) } +
                // the Mac offers "until I'm back at my Mac" on its screen-use question (PHN-610, PHN-611)
                (if (q.untilBack) listOf(stringResource(R.string.until_back_yes) to (if (busy) null else ({ vm.answer(q.approvalId, AnswerInput.UntilBack); Unit })))
                else emptyList()),
            moreNote = if (q.untilBack) stringResource(R.string.until_back_note) else null,
            exact = exact,
        )
        return
    }
    SheetFrame(title = title, bottomInset = insets.bottom, onDismiss = close) {
        Row(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            SheetIcon(FerbIcons.help)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                m?.header?.ifEmpty { null }?.let { PhoneText(it, 13.5f, FerbType.Medium, Ink.detail) }
                PhoneText(title, 22f, FerbType.Semibold)
                if (text.isNotEmpty()) PhoneText(text, 15.5f, color = Ink.detail)
            }
        }
        Box(Modifier.padding(top = 14.dp)) { exact() }
        below()
        if (choices.isNotEmpty()) {
            Column(Modifier.padding(top = 18.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                choices.forEachIndexed { i, c ->
                    Pill(c.label, kind = if (i == 0) PillKind.BLACK else PillKind.GREY, onClick = if (busy) null else ({ vm.answer(q.approvalId, c.input) }))
                }
                m?.note?.let { PhoneText(it, 13.5f, color = Ink.detail, modifier = Modifier.padding(horizontal = 4.dp)) }
            }
            // FERB.app's box under a task's choices: his own words instead (DaemonApproval.answerBox "answer")
            if (m?.answerBox == "answer") {
                var words by remember(q.approvalId) { mutableStateOf("") }
                Field(words, { words = it }, m.field ?: stringResource(R.string.question_answer_hint), Modifier.padding(top = 14.dp))
                Pill(m.send ?: stringResource(R.string.question_send), Modifier.padding(top = 10.dp),
                    onClick = if (busy || words.isBlank()) null else ({ vm.answer(q.approvalId, AnswerInput.Words(words.trim())); words = "" }))
            }
        } else {
            var answer by remember(q.approvalId) { mutableStateOf("") }
            Field(answer, { answer = it }, m?.field ?: stringResource(R.string.question_answer_hint), Modifier.padding(top = 18.dp))
            Row(Modifier.fillMaxWidth().padding(top = 12.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Pill(m?.decline ?: stringResource(R.string.question_skip), kind = PillKind.GREY, fill = false,
                    onClick = if (busy) null else ({ vm.answer(q.approvalId, AnswerInput.Words("")) }))
                Pill(m?.send ?: stringResource(R.string.question_send), Modifier.weight(1f),
                    onClick = if (busy || answer.isBlank()) null else ({ vm.answer(q.approvalId, AnswerInput.Words(answer.trim())); answer = "" }))
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
    // FERB.app's words for a hidden input (kb PHN-739): its question, why, the field's label and its two buttons
    val m = q.shown
    val title = m?.question ?: q.title ?: stringResource(R.string.private_title_default)
    val detail = if (m != null) m.detail.orEmpty() else q.reason
    val decline = m?.decline ?: stringResource(R.string.private_decline)
    val sendWord = m?.send ?: stringResource(R.string.private_send)
    if (q.isCode) {
        val digits = remember(q.approvalId) { CharArray(MAX_CODE) }
        var count by remember(q.approvalId) { mutableIntStateOf(0) }
        DisposableEffect(q.approvalId) { onDispose { digits.fill('\u0000'); count = 0 } }
        val send: () -> Unit = {
            // the digits go into the library's SecretInput one by one and this array is zeroed at once (kb PHN-536)
            val n = count
            count = 0
            vm.answerHidden(q.approvalId, digits, n)
        }
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
            SheetFrame(title = title, bottomInset = insets.bottom, lift = (4 * 57 + 18).dp + insets.bottom, onDismiss = close) {
                PrivateHead(title, detail, m?.header?.ifEmpty { null })
        m?.let { MacBelow(it) }
                SecretCodeBoxes(count, Modifier.padding(top = 16.dp), length = maxOf(6, minOf(MAX_CODE, count + 1)))
                PrivateNote()
                Row(Modifier.fillMaxWidth().padding(top = 14.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Pill(decline, kind = PillKind.GREY, fill = false, height = 46.dp,
                        onClick = if (busy) null else ({ vm.answer(q.approvalId, AnswerInput.Words("")) }))
                    Pill(sendWord, Modifier.weight(1f), height = 46.dp, onClick = if (busy || count == 0) null else send)
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
        PrivateHead(title, detail, m?.header?.ifEmpty { null })
        m?.let { MacBelow(it) }
        Box(Modifier.padding(top = 16.dp).fillMaxWidth().heightIn(min = Phone.secretBoxHeight).background(Ink.fill, RoundedCornerShape(Phone.secretBoxCorner)).padding(horizontal = 16.dp),
            contentAlignment = Alignment.CenterStart) {
            BasicSecureTextField(
                field, Modifier.fillMaxWidth().semantics { contentDescription = "Private answer" },
                textStyle = FerbType.style(18f, color = Ink.black), textObfuscationMode = TextObfuscationMode.Hidden,
                keyboardOptions = KeyboardOptions(KeyboardCapitalization.None, autoCorrectEnabled = false, keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
                cursorBrush = SolidColor(Ink.black),
            )
            if (field.text.isEmpty()) PhoneText(m?.field ?: stringResource(R.string.private_field), 16f, color = Ink.faint)
        }
        PrivateNote()
        Row(Modifier.fillMaxWidth().padding(top = 14.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Pill(decline, kind = PillKind.GREY, fill = false,
                onClick = if (busy) null else ({ field.clearText(); vm.answer(q.approvalId, AnswerInput.Words("")) }))
            Pill(sendWord, Modifier.weight(1f), onClick = if (busy || field.text.isEmpty()) null else ({
                // read character by character (no String of it here), the field cleared, the copy zeroed by the view
                // model once its units are in the library's SecretInput (kb PHN-536)
                val t = field.text
                val chars = CharArray(t.length) { t[it] }
                field.clearText()
                vm.answerHidden(q.approvalId, chars, chars.size)
            }))
        }
    }
}

private const val MAX_CODE = 10

@Composable
private fun PrivateHead(title: String, detail: String, header: String? = null) {
    Row(horizontalArrangement = Arrangement.spacedBy(14.dp), verticalAlignment = Alignment.CenterVertically) {
        SheetIcon(FerbIcons.key)
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            if (header != null) PhoneText(header, 13.5f, FerbType.Medium, Ink.detail)
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
    // read again when the switches change and when he comes back from Android's settings (the app returns to front)
    val front by vm.front.collectAsState()
    val blocker = androidx.compose.runtime.remember(s, front) { vm.notifyBlocker() }
    // the Mac's own date for this phone's pairing (phone.list), else the day this phone first saw it paired
    val pairedAt = ui.mac?.pairedAt?.let { runCatching { java.time.OffsetDateTime.parse(it).toInstant().toEpochMilli() }.getOrNull() }
        ?: s.pairedSince.takeIf { it > 0 }
    val since = pairedAt?.let { DateFormat.getDateInstance(DateFormat.LONG).format(Date(it)) } ?: "—"
    ProfileSheet(
        ui.mac?.name ?: stringResource(R.string.profile_mac_name), since, insets,
        lockOn = true, // PHN-8: FERB always opens with the fingerprint or the screen lock; there is no off
        // PHN-656: no switch that does nothing: "Always on", and a tap says why
        lockAlways = stringResource(R.string.profile_lock_always),
        onLockInfo = { vm.note(com.ferb.app.Note(R.string.profile_lock_why)) },
        stayConnected = s.stayConnected,
        onClose = close,
        onStayConnected = host::stayConnected,
        onForget = { vm.show(Sheet.Forget) },
        more = {
            // kb PHN-765: what keeps FERB's questions from reaching this phone, said where he looks, with its fix
            blocker?.let { b ->
                Divide()
                CardRow(stringResource(R.string.cant_notify_title), stringResource(b.words), leading = { RowIcon(FerbIcons.bell) },
                    onClick = { host.fixNotifications(b) }, trailing = { com.ferb.design.components.Opens() })
            }
            Divide()
            CardRow(stringResource(R.string.profile_screenshots), stringResource(R.string.profile_screenshots_detail), leading = { RowIcon(FerbIcons.eye) },
                trailing = { PhoneSwitch(s.blockScreenshots, { host.blockScreenshots(it); vm.refreshProfile() }, stringResource(R.string.profile_screenshots)) })
            Divide()
            CardRow(stringResource(R.string.profile_lock_screen), stringResource(R.string.profile_lock_screen_detail), leading = { RowIcon(FerbIcons.bell) },
                trailing = { PhoneSwitch(s.hideChecks, vm::hideChecks, stringResource(R.string.profile_lock_screen)) })
            Divide()
            CardRow(stringResource(R.string.profile_city), stringResource(R.string.profile_city_detail), leading = { RowIcon(Icons.Rounded.LocationOn) },
                trailing = { PhoneSwitch(s.shareCity, host::shareCity, stringResource(R.string.profile_city)) })
            if (ui.newKeys) {
                Divide()
                CardRow(stringResource(R.string.keys_row), stringResource(R.string.keys_row_detail), leading = { RowIcon(FerbIcons.fingerprint) },
                    onClick = if ("keys" in ui.busy) null else ({ vm.registerKeys() }), trailing = { com.ferb.design.components.Opens() })
            }
        },
    )
}

/**
 * What FERB.app shows under a question's exact thing (ApprovalView.swift:60-73): the task's reason in its own words,
 * smaller, and the app-use evidence behind a disclosure.
 */
@Composable
private fun MacBelow(m: com.ferb.mobile.QuestionShown) {
    m.why?.let { PhoneText(it, 13.5f, color = Ink.detail, modifier = Modifier.fillMaxWidth().padding(top = 12.dp)) }
    if (m.evidence.isNotEmpty()) {
        var open by remember(m) { mutableStateOf(false) }
        PhoneText((if (open) "▾ " else "▸ ") + "App-use evidence", 13.5f, FerbType.Medium, Ink.detail,
            Modifier.padding(top = 10.dp).pressable({ open = !open }))
        if (open) for (line in m.evidence) PhoneText(line, 13.5f, color = Ink.detail, modifier = Modifier.padding(top = 4.dp, start = 12.dp))
    }
}
