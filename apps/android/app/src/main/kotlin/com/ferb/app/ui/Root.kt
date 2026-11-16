package com.ferb.app.ui

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBars
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.max
import com.ferb.app.FerbApplication
import com.ferb.app.FerbViewModel
import com.ferb.app.Gate
import com.ferb.app.MainActivity
import com.ferb.app.PairReason
import com.ferb.app.R
import com.ferb.app.Sheet
import com.ferb.app.StayConnectedService
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.components.Card
import com.ferb.design.components.FerbMark
import com.ferb.design.components.LocalSkyInk
import com.ferb.design.components.PhoneTab
import com.ferb.design.components.PhoneText
import com.ferb.design.components.Pill
import com.ferb.design.screens.CodesPage
import com.ferb.design.screens.PairPage
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.ferb.design.screens.UnlockPage
import com.ferb.design.sky.rememberSkyNow
import kotlinx.coroutines.launch

/**
 * Everything on screen: the live sky behind every page (the time zone's place, PHN-420/PHN-450: no location is asked
 * now), the page the gate allows, and a sheet over it. Nothing of FERB's is drawn before the gate is MAIN (PHN-8).
 */
@Composable
fun FerbRoot(vm: FerbViewModel, activity: MainActivity) {
    val ui by vm.ui.collectAsState()
    val tab by vm.tab.collectAsState()
    val home by vm.home.collectAsState()
    val sheet by vm.sheet.collectAsState()
    val place = remember { vm.skyPlace() }
    val moment = rememberSkyNow(place)
    val density = LocalDensity.current
    val top = with(density) { WindowInsets.statusBars.getTop(this).toDp() }
    val nav = with(density) { WindowInsets.navigationBars.getBottom(this).toDp() }
    val ime = with(density) { WindowInsets.ime.getBottom(this).toDp() }
    val insets = PhoneInsets(top, max(nav, ime))
    val light = moment.ink.topIsDark
    SideEffect { activity.lightBars(light) }
    val stay = rememberStayConnected(activity, vm)
    val askNotifications = rememberNotificationAsk(activity, vm)
    // the system's picker: no storage permission, only the files the owner picks (PHN-115)
    var pickForChat by remember { mutableStateOf(false) }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        vm.sendFiles(uris.map { it.toString() }, pickForChat)
    }
    val host = remember(activity) {
        object : Host {
            override fun stayConnected(on: Boolean) = stay(on)
            override fun blockScreenshots(on: Boolean) = activity.setBlockScreenshots(on)
            override fun askNotifications() = askNotifications()
            override fun pickFiles(forChat: Boolean) {
                pickForChat = forChat
                runCatching { picker.launch(arrayOf("*/*")) }
            }
            override fun handOut(key: String, file: java.io.File, name: String, share: Boolean) = activity.handOut(key, file, name, share)
        }
    }

    PhoneScreen(
        moment,
        insets = insets,
        season = ui.gate == Gate.MAIN && tab == PhoneTab.CHAT && home,
        sheet = { if (ui.gate == Gate.MAIN || ui.gate == Gate.PAIR) Sheets(vm, ui, sheet, insets, host) },
    ) {
        when (ui.gate) {
            Gate.STARTING -> Unit
            Gate.FAILED -> Failed(insets)
            Gate.NO_SCREEN_LOCK -> NoScreenLock(insets)
            Gate.LOCKED -> UnlockPage(insets, onUnlock = { vm.unlockApp() })
            Gate.PAIR -> if (sheet == Sheet.Scan && !ui.pairing) Scanner(vm, insets) { vm.show(null) }
            else Pair(vm, ui.pairing, ui.pairReason, ui.pairError?.let { noteText(it) }, insets)
            Gate.CODES -> CodesPage(ui.link?.pairingCode.orEmpty(), insets, onCancel = { vm.cancelPairing() })
            Gate.REMOVED -> Removed(vm, ui.pairReason, insets)
            Gate.MAIN -> Main(vm, ui, tab, home, insets, moment, host)
        }
    }
}

@Composable
internal fun Pair(vm: FerbViewModel, pairing: Boolean, reason: PairReason?, error: String?, insets: PhoneInsets) {
    val context = LocalContext.current
    val url = stringResource(R.string.mac_app_url)
    Box(Modifier.fillMaxSize()) {
        PairPage(
            insets,
            onScan = { vm.clearPairError(); vm.show(Sheet.Scan) },
            onTypeCode = { vm.clearPairError(); vm.show(Sheet.TypeCode) },
            onGetMacApp = { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url))) },
            onHelp = { vm.show(Sheet.Help) },
        )
        val line = when {
            pairing -> stringResource(R.string.pair_reaching)
            error != null -> error
            reason == PairReason.WIPED -> stringResource(R.string.pair_reason_wiped)
            reason == PairReason.REMOVED -> stringResource(R.string.pair_reason_removed)
            reason == PairReason.KEYS -> stringResource(R.string.pair_reason_keys)
            else -> null
        }
        if (line != null) {
            Card(Modifier.align(Alignment.TopCenter).padding(start = 16.dp, end = 16.dp, top = 70.dp), padding = 16.dp) {
                PhoneText(line, 15.5f, color = Ink.black, modifier = Modifier.fillMaxWidth().semantics { liveRegion = LiveRegionMode.Polite })
            }
        }
    }
}

@Composable
private fun Removed(vm: FerbViewModel, reason: PairReason?, insets: PhoneInsets) {
    Message(
        title = stringResource(R.string.removed_title),
        text = stringResource(if (reason == PairReason.KEYS) R.string.pair_reason_keys else R.string.pair_reason_removed),
        action = stringResource(R.string.pair_again),
        insets = insets,
    ) { vm.forget() }
}

@Composable
private fun Failed(insets: PhoneInsets) {
    val app = LocalContext.current.applicationContext as FerbApplication
    Message(stringResource(R.string.failed_title), stringResource(R.string.failed_text), stringResource(R.string.try_again), insets) {
        app.scope.launch { app.phone.start() }
    }
}

@Composable
private fun NoScreenLock(insets: PhoneInsets) {
    val context = LocalContext.current
    Message(stringResource(R.string.no_lock_title), stringResource(R.string.no_lock_text), stringResource(R.string.no_lock_open_settings), insets) {
        context.startActivity(Intent(Settings.ACTION_SECURITY_SETTINGS))
    }
}

/** UnlockPage's layout (the mark, a title, a card with one button) for the gate's other plain messages. */
@Composable
fun Message(title: String, text: String, action: String, insets: PhoneInsets, onAction: () -> Unit) {
    val ink = LocalSkyInk.current
    Column(Modifier.fillMaxSize(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(14.dp)) {
        Spacer(Modifier.height(150.dp))
        FerbMark(84.dp)
        PhoneText(title, FerbType.Role.lockTitle, FerbType.Semibold, ink.top, Modifier.padding(top = 6.dp, start = 24.dp, end = 24.dp).semantics { heading() })
        Spacer(Modifier.weight(1f))
        Card(Modifier.padding(start = 16.dp, end = 16.dp, bottom = insets.bottom + 8.dp), padding = 16.dp) {
            PhoneText(text, 15.5f, color = Ink.detail, lineSpacing = 2f, modifier = Modifier.fillMaxWidth().padding(bottom = 14.dp, start = 4.dp, end = 4.dp))
            Pill(action, onClick = onAction)
        }
    }
}

/** The notification question, in context first (Android 13+ asks once): turning Stay connected on asks it. */
@Composable
fun rememberStayConnected(activity: MainActivity, vm: FerbViewModel): (Boolean) -> Unit {
    val app = activity.application as FerbApplication
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (!granted) vm.note(com.ferb.app.Note(R.string.notify_denied))
        app.prefs.stayConnected = true
        StayConnectedService.set(activity, true)
        vm.refreshProfile()
    }
    return { on ->
        if (!on) {
            app.prefs.stayConnected = false
            StayConnectedService.set(activity, false)
            vm.refreshProfile()
        } else if (activity.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) {
            app.prefs.stayConnected = true
            StayConnectedService.set(activity, true)
            vm.refreshProfile()
        } else if (!app.prefs.notificationsExplained) {
            vm.show(Sheet.Notifications)
        } else {
            ask.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }
}

/** The launcher for Android's own notification question, after the app's words (Sheet.Notifications). */
@Composable
fun rememberNotificationAsk(activity: MainActivity, vm: FerbViewModel): () -> Unit {
    val app = activity.application as FerbApplication
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (!granted) vm.note(com.ferb.app.Note(R.string.notify_denied))
        app.prefs.stayConnected = true
        StayConnectedService.set(activity, true)
        vm.refreshProfile()
    }
    return {
        app.prefs.notificationsExplained = true
        vm.show(Sheet.Profile)
        ask.launch(Manifest.permission.POST_NOTIFICATIONS)
    }
}
