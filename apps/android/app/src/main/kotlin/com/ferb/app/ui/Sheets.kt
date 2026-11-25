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
        Sheet.Insomnia -> InsomniaSheet(vm, ui, insets)
        Sheet.Camera -> CameraScreen(vm, ui, insets)
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
        Sheet.Account -> AccountSheet(vm, ui, insets, close)
        Sheet.SignOut -> SignOutSheet(vm, ui, insets)
        Sheet.DeleteAccount -> DeleteAccountSheet(vm, ui, insets)
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

/** MobileScreens.swift ProfileSheet with this phone's switches (PHN-835, PHN-420, PHN-421, PHN-422) working. */
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
        // kb PHN-835, PHN-845: Fingerprint to open, on by default; turning it off takes the prompt
        lockOn = s.lockOn,
        onLock = if ("app-lock" in ui.busy) null else ({ on -> vm.setAppLock(on) }),
        // kb PHN-847: locks at once, whatever the switch says; the sheet closes into the lock page
        lockNow = {
            CardRow(stringResource(R.string.profile_lock_now), stringResource(R.string.profile_lock_now_detail),
                onClick = vm::lockNow, leading = { RowIcon(FerbIcons.lockFill) })
        },
        stayConnected = s.stayConnected,
        onClose = close,
        onStayConnected = host::stayConnected,
        onForget = { vm.show(Sheet.Forget) },
        // PHN-801: the connection's row (the top bar's badge); its sheet keeps Try again, the relay and the Mac tab
        connection = {
            val mark = linkMark(ui.link, ui.linkLost)
            com.ferb.design.screens.ConnectionRow(mark, linkStateWords(mark), linkPathWords(mark)) { vm.show(Sheet.Connection) }
        },
        more = {
            // kb PHN-827: FERB's account, its page on a tap (Sign out, Delete account)
            if (ui.account.account != null) {
                Divide()
                AccountRow(vm, ui)
            }
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
