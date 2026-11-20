package com.ferb.app.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import com.ferb.app.FerbViewModel
import com.ferb.app.R
import com.ferb.app.UiState
import com.ferb.app.model.CameraView
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.components.Card
import com.ferb.design.components.CloseButton
import com.ferb.design.components.Divide
import com.ferb.design.components.PhoneSwitch
import com.ferb.design.components.PhoneText
import com.ferb.design.components.Pill
import com.ferb.design.components.PillKind
import com.ferb.design.components.Sheet as SheetFrame
import com.ferb.design.components.SheetTitle
import com.ferb.design.components.pressable
import com.ferb.design.presenceFade
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.paddingOverBottom
import com.ferb.mobile.Insomnia

/** The insomnia ways the Mac takes (the contract's `variant`). */
const val SHIELD = "shield"
const val LOCKED = "locked"

/**
 * The Mac tab's "Keep my Mac working" (APP-980): the switch (on opens the warning, off ends it), the Mac's own line
 * and End while it is on, someone at the Mac with Show me, Lock my Mac and It's fine (APP-983), and the warnings
 * (APP-981). Shown only when the Mac says it can (an older Mac has no `insomnia`).
 */
@Composable
fun InsomniaCard(vm: FerbViewModel, ui: UiState, i: Insomnia) {
    val ending = "insomnia-end" in ui.busy
    Card(Modifier.padding(start = 16.dp, end = 16.dp, bottom = 12.dp), padding = 16.dp) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                PhoneText(stringResource(R.string.insomnia_row), 16.5f, FerbType.Semibold, Ink.black)
                if (i.on && i.line.isNotEmpty()) {
                    PhoneText(i.line, 14f, color = Ink.detail, modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
                }
            }
            Spacer(Modifier.width(8.dp))
            PhoneSwitch(i.on, onChange = if (ending || "insomnia" in ui.busy) null else ({ on -> if (on) vm.askInsomnia() else vm.endInsomnia() }),
                label = stringResource(R.string.insomnia_row))
        }
        val notice = i.notice?.takeIf { i.on }
        if (notice != null) {
            PhoneText(notice.text, 15.5f, FerbType.Medium, Ink.black,
                Modifier.fillMaxWidth().padding(top = 12.dp).semantics { liveRegion = LiveRegionMode.Assertive })
            Row(Modifier.fillMaxWidth().padding(top = 10.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Pill(stringResource(R.string.insomnia_show_me), Modifier.weight(1f), PillKind.BLACK, FerbIcons.camera, height = 44.dp,
                    onClick = { vm.showCamera() })
                Pill(stringResource(R.string.insomnia_lock_mac), Modifier.weight(1f), PillKind.GREY, FerbIcons.lock, height = 44.dp,
                    onClick = if ("lock-mac" in ui.busy) null else ({ vm.lockMac() }))
            }
            Pill(stringResource(R.string.insomnia_fine), Modifier.padding(top = 4.dp), PillKind.PLAIN, height = 40.dp,
                onClick = if ("insomnia-fine" in ui.busy) null else ({ vm.insomniaFine(notice.id) }))
        }
        if (i.on) {
            for (w in i.warnings) {
                PhoneText(w.text, 14.5f, color = Ink.detail, modifier = Modifier.fillMaxWidth().padding(top = 10.dp))
            }
            Pill(stringResource(R.string.insomnia_end), Modifier.padding(top = 12.dp), PillKind.GREY, height = 44.dp,
                onClick = if (ending) null else ({ vm.endInsomnia() }))
        }
    }
}

/**
 * The warning before the phone turns it on (APP-980): the same words as the Mac's dialog, the two ways (Cover the
 * screen by default), the one Yes under the first (on by default, APP-984), when it ends, Keep it working (the
 * fingerprint signs it) and Not now.
 */
@Composable
fun InsomniaSheet(vm: FerbViewModel, ui: UiState, insets: PhoneInsets) {
    val close = { vm.show(null) }
    var variant by rememberSaveable { mutableStateOf(SHIELD) }
    var screenYes by rememberSaveable { mutableStateOf(true) }
    val minutes = ui.status?.insomnia?.graceMinutes?.toInt() ?: 10
    val title = stringResource(R.string.insomnia_title)
    SheetFrame(title = title, bottomInset = insets.bottom, onDismiss = close) {
        SheetTitle(title, close)
        PhoneText(stringResource(R.string.insomnia_body), 15.5f, color = Ink.detail, lineSpacing = 2f, modifier = Modifier.padding(top = 6.dp))
        Column(Modifier.fillMaxWidth().padding(top = 14.dp).background(Ink.fill, RoundedCornerShape(16.dp))) {
            Choice(stringResource(R.string.insomnia_shield), stringResource(R.string.insomnia_shield_text), variant == SHIELD) { variant = SHIELD }
            if (variant == SHIELD) {
                Check(stringResource(R.string.insomnia_screen_yes), screenYes) { screenYes = it }
            }
            Divide()
            Choice(stringResource(R.string.insomnia_locked), stringResource(R.string.insomnia_locked_text), variant == LOCKED) { variant = LOCKED }
        }
        PhoneText(stringResource(R.string.insomnia_ends, minutes), 14f, color = Ink.detail, modifier = Modifier.padding(top = 12.dp))
        Pill(stringResource(R.string.insomnia_go), Modifier.padding(top = 16.dp), icon = FerbIcons.fingerprint,
            onClick = if ("insomnia" in ui.busy) null else ({ vm.startInsomnia(variant, variant == SHIELD && screenYes) }))
        Pill(stringResource(R.string.insomnia_not_now), Modifier.padding(top = 4.dp), PillKind.PLAIN, height = 44.dp, onClick = close)
    }
}

/** One of the two ways: its name, what it means, a check when chosen. One TalkBack stop, a radio button. */
@Composable
private fun Choice(title: String, text: String, chosen: Boolean, onChoose: () -> Unit) {
    Row(Modifier.fillMaxWidth().pressable(onChoose, Role.RadioButton).semantics(mergeDescendants = true) { selected = chosen }
        .padding(horizontal = 14.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            PhoneText(title, 16.5f, FerbType.Semibold, Ink.black)
            PhoneText(text, 14f, color = Ink.detail, lineSpacing = 1f)
        }
        Spacer(Modifier.width(10.dp))
        Box(Modifier.size(24.dp), contentAlignment = Alignment.Center) {
            if (chosen) FerbIcon(FerbIcons.check, 20f, Ink.black)
        }
    }
}

/** The one Yes under Cover the screen (APP-984): a checkbox. */
@Composable
private fun Check(text: String, on: Boolean, onChange: (Boolean) -> Unit) {
    val state = if (on) "On" else "Off"
    Row(Modifier.fillMaxWidth().pressable({ onChange(!on) }, Role.Checkbox).semantics(mergeDescendants = true) { stateDescription = state }
        .padding(start = 14.dp, end = 14.dp, bottom = 12.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.size(22.dp).background(if (on) Ink.black else Color.Transparent, RoundedCornerShape(6.dp))
            .border(1.5.dp, Ink.black, RoundedCornerShape(6.dp)), contentAlignment = Alignment.Center) {
            if (on) FerbIcon(FerbIcons.check, 16f, Ink.white)
        }
        Spacer(Modifier.width(10.dp))
        PhoneText(text, 14.5f, FerbType.Medium, Ink.black, Modifier.weight(1f))
    }
}

/** Show me (APP-983): the Mac's camera while this shows (MainActivity keeps it out of screenshots). */
@Composable
fun CameraScreen(vm: FerbViewModel, ui: UiState, insets: PhoneInsets) {
    val view by vm.camera.view.collectAsState()
    val v = view ?: return
    BackHandler { vm.closeCamera() }
    val notice = ui.status?.insomnia?.notice
    CameraPage(v, insets, onClose = vm::closeCamera,
        onLock = if ("lock-mac" in ui.busy) null else ({ vm.lockMac() }),
        onFine = notice?.let { n -> { vm.insomniaFine(n.id); vm.closeCamera() } },
        onAgain = if (v.ended != null) ({ vm.cameraAgain() }) else null)
}

/** The page itself (drawn from a [CameraView]). */
@Composable
fun CameraPage(v: CameraView, insets: PhoneInsets, onClose: () -> Unit, onLock: (() -> Unit)?, onFine: (() -> Unit)?, onAgain: (() -> Unit)?) {
    Column(Modifier.presenceFade().fillMaxSize().background(Color.Black).paddingOverBottom(insets, 8.dp, top = insets.top + 8.dp)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
            PhoneText(stringResource(R.string.camera_title), 22f, FerbType.Semibold, Ink.white, Modifier.weight(1f).semantics { heading() })
            CloseButton(onClose)
        }
        Box(Modifier.weight(1f).fillMaxWidth().padding(16.dp), contentAlignment = Alignment.Center) {
            val frame = v.frame
            if (frame != null) {
                val image = remember(frame) { frame.asImageBitmap() }
                Image(image, stringResource(R.string.camera_title), Modifier.fillMaxSize(), contentScale = ContentScale.Fit)
            } else {
                val text = v.ended?.let { noteText(it) } ?: v.text ?: stringResource(R.string.camera_starting)
                PhoneText(text, 16f, color = Ink.onDark, modifier = Modifier.padding(horizontal = 16.dp).semantics { liveRegion = LiveRegionMode.Polite })
            }
        }
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Pill(stringResource(R.string.insomnia_lock_mac), Modifier.weight(1f), PillKind.GREY, FerbIcons.lock, height = 46.dp, onClick = onLock)
                if (onFine != null) Pill(stringResource(R.string.insomnia_fine), Modifier.weight(1f), PillKind.GREY, height = 46.dp, onClick = onFine)
            }
            if (onAgain != null) Pill(stringResource(R.string.live_again), kind = PillKind.GREY, icon = FerbIcons.eye, height = 46.dp, onClick = onAgain)
            else Pill(stringResource(R.string.live_close), kind = PillKind.GREY, height = 46.dp, onClick = onClose)
        }
    }
}
