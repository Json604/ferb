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
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
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
import com.ferb.design.LaunchOverlay
import com.ferb.design.LaunchState
import com.ferb.design.LocalLaunch
import com.ferb.design.MotionKind
import com.ferb.design.reducedMotion
import com.ferb.design.MotionSwitch
import com.ferb.design.heldWhileLeaving
import com.ferb.design.components.Card
import com.ferb.design.components.FerbMark
import com.ferb.design.components.LocalSkyInk
import com.ferb.design.components.PhoneTab
import com.ferb.design.components.PhoneText
import com.ferb.design.components.Pill
import com.ferb.design.screens.CodesPage
import com.ferb.design.screens.PairPage
import com.ferb.design.screens.AskRow
import com.ferb.design.screens.PermissionsPage
import com.ferb.design.FerbIcons
import com.ferb.app.model.FirstRun
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.paddingOverBottom
import com.ferb.design.screens.PhoneScreen
import com.ferb.design.screens.UnlockPage
import com.ferb.design.sky.rememberSkyNow
import kotlinx.coroutines.launch

/**
 * Everything on screen: the live sky behind every page (PHN-420/PHN-450: the phone's coarse place while the owner
 * shares the city, else the time zone's), the page the gate allows, and a sheet over it. Nothing of FERB's is drawn
 * before the gate is MAIN (PHN-8).
 */
@Composable
fun FerbRoot(vm: FerbViewModel, activity: MainActivity) {
    // The whole state is read only inside the page and the sheet that draw it; here only the gate and the pairing flag,
    // so a streamed word or a link's status recomposes the page, never the root, the sky or the hosts (kb PHN-711).
    val uiState = vm.ui.collectAsState()
    val gate by remember { derivedStateOf { uiState.value.gate } }
    val pairing by remember { derivedStateOf { uiState.value.pairing } }
    val tab by vm.tab.collectAsState()
    val home by vm.home.collectAsState()
    val sheet by vm.sheet.collectAsState()
    val fix by vm.skyFix.collectAsState()
    val place = remember(fix) { vm.skyPlace() }
    val moment = rememberSkyNow(place)
    val density = LocalDensity.current
    val top = with(density) { WindowInsets.statusBars.getTop(this).toDp() }
    // the bottom (the gesture bar, or the keyboard over it) is read where it is used, never here: the keyboard's slide
    // changes it on every frame, and read here it recomposed the whole app on each of them (kb PHN-712)
    val navBars = WindowInsets.navigationBars
    val keyboard = WindowInsets.ime
    val insets = remember(top, density, navBars, keyboard) {
        PhoneInsets.live(top) { with(density) { max(navBars.getBottom(this).toDp(), keyboard.getBottom(this).toDp()) } }
    }
    val light = moment.ink.topIsDark
    SideEffect { activity.lightBars(light) }
    val stay = rememberStayConnected(activity, vm)
    val askNotifications = rememberNotificationAsk(activity, vm)
    // the system's picker: no storage permission, only the files the owner picks (PHN-115)
    var pickForChat by remember { mutableStateOf(false) }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        vm.sendFiles(uris.map { it.toString() }, pickForChat)
    }
    // a whole folder: the system's folder picker, which grants this one folder and nothing else
    val folderPicker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
        uri?.let { vm.sendFolder(it.toString(), pickForChat) }
    }
    val city = rememberCity(activity, vm)
    val exact = rememberExact(activity, vm)
    val microphone = rememberMicrophone(activity, vm)
    val asks = rememberFirstRun(activity, vm)
    val host = remember(activity) {
        object : Host {
            override fun stayConnected(on: Boolean) = stay(on)
            override fun blockScreenshots(on: Boolean) = activity.setBlockScreenshots(on)
            override fun askNotifications() = askNotifications()
            // kb PHN-765: Stay connected is the app's own switch; the rest are Android's pages
            override fun fixNotifications(b: com.ferb.app.NotifyBlocker) {
                if (b == com.ferb.app.NotifyBlocker.STAY_CONNECTED_OFF) stay(true)
                else com.ferb.app.NotifyHealth.settings(activity, b)?.let { runCatching { activity.startActivity(it) } }
            }
            override fun pickFiles(forChat: Boolean) {
                pickForChat = forChat
                runCatching { picker.launch(arrayOf("*/*")) }
            }
            override fun handOut(key: String, file: java.io.File, name: String, share: Boolean) = activity.handOut(key, file, name, share)
            override fun pickFolder(forChat: Boolean) {
                pickForChat = forChat
                runCatching { folderPicker.launch(null) }
            }
            override fun shareMany(files: List<java.io.File>) = activity.shareMany(files)
            override fun shareCity(on: Boolean) = city(on)
            override fun shareExact(requestId: String) = exact(requestId)
            override fun microphoneAllowed() =
                activity.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
            override fun allowMicrophone(kind: com.ferb.app.model.SpeechKind?) = microphone(kind)
        }
    }

    // the launch (kb PHN-631..634): FERB's mark from the first frame over everything, settling into the lock page's
    // mark as soon as the gate allows a page; the page's mark is hidden until then (LocalLaunch, launchTarget)
    val launch = activity.launch
    CompositionLocalProvider(LocalLaunch provides launch) {
        PhoneScreen(
            moment,
            insets = insets,
            season = gate == Gate.MAIN && tab == PhoneTab.CHAT && home,
            // one sheet at a time, up from below and back down; a going one shows what it showed (kb PHN-623). Gone at
            // once with its gate (the lock: PHN-8).
            sheet = {
                if (gate == Gate.MAIN || gate == Gate.PAIR) {
                    MotionSwitch(sheet.takeIf { it != Sheet.Scan }, Modifier.fillMaxSize(), MotionKind.SHEET) { s ->
                        Sheets(vm, heldWhileLeaving(uiState.value), s, insets, host)
                    }
                }
            },
        ) {
            // the gate's pages change with a rise and a fade; the lock (and the start) replace everything at once (PHN-8)
            // the first run's asks come once, after the first unlock, before pairing or the app (kb PHN-636)
            val screen: Any = when {
                asks.pending.isNotEmpty() && (gate == Gate.PAIR || gate == Gate.MAIN) -> AsksScreen
                gate == Gate.PAIR && sheet == Sheet.Scan && !pairing -> ScanScreen
                else -> gate
            }
            MotionSwitch(screen, Modifier.fillMaxSize(), instant = { it == Gate.LOCKED || it == Gate.STARTING }) { s ->
                val ui = uiState.value
                when (s) {
                    // after the launch (unlocked before the phone opened): the mark breathes where the launch left it
                    Gate.STARTING -> if (launch == null || launch.done) StartingMark()
                    Gate.FAILED -> Failed(insets)
                    Gate.NO_SCREEN_LOCK -> NoScreenLock(insets)
                    Gate.LOCKED -> UnlockPage(insets, onUnlock = { vm.unlockApp() })
                    ScanScreen -> Scanner(vm, insets) { vm.show(null) }
                    AsksScreen -> Asks(asks, insets)
                    Gate.PAIR -> Pair(vm, ui.pairing, ui.pairReason, ui.pairError?.let { noteText(it) }, insets)
                    Gate.CODES -> CodesPage(ui.link?.pairingCode.orEmpty(), insets, onCancel = { vm.cancelPairing() })
                    Gate.REMOVED -> Removed(vm, ui.pairReason, insets)
                    // the connection control in every page's top bar (kb PHN-751)
                    Gate.MAIN -> androidx.compose.runtime.CompositionLocalProvider(com.ferb.design.components.LocalConnection provides rememberConnection(vm, ui)) {
                        Main(vm, ui, tab, home, insets, moment, host)
                    }
                }
            }
        }
        if (launch != null) LaunchOverlay(launch, ready = gate != Gate.STARTING)
    }
}

/** The phone still opening after the launch ended: the launch's mark, breathing over the sky (no ink0 ground). */
@Composable
private fun StartingMark() {
    val context = LocalContext.current
    val state = remember { LaunchState(reducedMotion(context)) }
    LaunchOverlay(state, ready = false, ground = false)
}

/** The scanner's page, a screen of the pairing gate of its own (MotionSwitch's key). */
private object ScanScreen

/** The first run's asks, a page of its own before pairing or the app (MotionSwitch's key). */
private object AsksScreen

/** What the first run's page needs: the asks still to make, Continue and Not now (kb PHN-636). */
class FirstRunUi(val pending: List<FirstRun.Ask>, val onContinue: () -> Unit, val onNotNow: () -> Unit)

@Composable
internal fun Asks(asks: FirstRunUi, insets: PhoneInsets) {
    val rows = asks.pending.map { a ->
        when (a) {
            FirstRun.Ask.NOTIFICATIONS -> AskRow(FerbIcons.bell, stringResource(R.string.first_run_notifications), stringResource(R.string.first_run_notifications_why))
            FirstRun.Ask.LOCATION -> AskRow(FerbIcons.location, stringResource(R.string.first_run_location), stringResource(R.string.first_run_location_why))
            FirstRun.Ask.CAMERA -> AskRow(FerbIcons.camera, stringResource(R.string.first_run_camera), stringResource(R.string.first_run_camera_why))
            FirstRun.Ask.MICROPHONE -> AskRow(FerbIcons.mic, stringResource(R.string.first_run_microphone), stringResource(R.string.first_run_microphone_why))
        }
    }
    PermissionsPage(stringResource(R.string.first_run_title), stringResource(R.string.first_run_intro), rows,
        stringResource(R.string.first_run_continue), stringResource(R.string.first_run_not_now), insets,
        onContinue = asks.onContinue, onNotNow = asks.onNotNow)
}

/**
 * The first run's asks (owner kb PHN-635; PHN-636..639): Continue brings Android's own questions one after another;
 * each answer, yes or no, is kept so it is never asked again; the city's Yes turns on sharing the city (what he is
 * granting). Coarse location only, never in the background; the exact spot is still asked per task (GATE-613).
 */
@Composable
fun rememberFirstRun(activity: MainActivity, vm: FerbViewModel): FirstRunUi {
    val app = activity.application as FerbApplication
    val run = remember(activity) {
        FirstRun(FirstRun.asksFor(android.os.Build.VERSION.SDK_INT), { a -> activity.checkSelfPermission(permissionOf(a)) == PackageManager.PERMISSION_GRANTED },
            { app.prefs.firstRunAsked }, { app.prefs.firstRunAsked = it }) {
            // owner kb PHN-771: his Yes to notifications here keeps FERB connected, so questions reach him with it closed
            app.prefs.notificationsExplained = true
            app.prefs.stayConnected = true
            StayConnectedService.set(activity, true)
            vm.refreshProfile()
        }
    }
    var pending by remember(run) { mutableStateOf(run.pending()) }
    val asking = remember { arrayOfNulls<FirstRun.Ask>(1) }
    val next = remember { arrayOf<() -> Unit>({}) }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        val a = asking[0] ?: return@rememberLauncherForActivityResult
        asking[0] = null
        run.answered(a, granted)
        if (a == FirstRun.Ask.LOCATION && granted) {
            app.prefs.shareCity = true
            app.sensors.city()
            vm.refreshProfile()
        }
        next[0]()
    }
    next[0] = {
        val a = run.pending().firstOrNull()
        if (a == null) {
            pending = emptyList()
        } else {
            asking[0] = a
            runCatching { launcher.launch(permissionOf(a)) }.onFailure { run.answered(a); next[0]() }
        }
    }
    return FirstRunUi(pending, onContinue = { next[0]() }, onNotNow = { run.notNow(); pending = emptyList() })
}

private fun permissionOf(a: FirstRun.Ask): String = when (a) {
    FirstRun.Ask.NOTIFICATIONS -> Manifest.permission.POST_NOTIFICATIONS
    FirstRun.Ask.LOCATION -> Manifest.permission.ACCESS_COARSE_LOCATION
    FirstRun.Ask.CAMERA -> Manifest.permission.CAMERA
    FirstRun.Ask.MICROPHONE -> Manifest.permission.RECORD_AUDIO
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
        text = stringResource(when (reason) {
            PairReason.KEYS -> R.string.pair_reason_keys
            PairReason.UNKNOWN -> R.string.pair_reason_unknown
            else -> R.string.pair_reason_removed
        }),
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
        Card(Modifier.paddingOverBottom(insets, 8.dp, start = 16.dp, end = 16.dp), padding = 16.dp) {
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

/**
 * The owner's switch for sharing this phone's city (SENSE-420, kb PHN-638): off at once; on asks Android's coarse
 * location question in context, after FERB's own words (Sheet.City), and only the city then goes to the Mac.
 */
@Composable
fun rememberCity(activity: MainActivity, vm: FerbViewModel): (Boolean) -> Unit {
    val app = activity.application as FerbApplication
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        app.prefs.shareCity = granted
        app.sensors.city()
        vm.show(Sheet.Profile)
        vm.cityChanged(granted)
    }
    return { on ->
        when {
            !on -> { app.prefs.shareCity = false; app.sensors.city(); vm.refreshProfile() }
            activity.checkSelfPermission(Manifest.permission.ACCESS_COARSE_LOCATION) == PackageManager.PERMISSION_GRANTED -> {
                app.prefs.shareCity = true; app.sensors.city(); vm.refreshProfile()
            }
            vm.sheet.value != Sheet.City -> vm.show(Sheet.City)
            else -> ask.launch(Manifest.permission.ACCESS_COARSE_LOCATION)
        }
    }
}

/**
 * The owner said yes to the Mac's request for this phone's exact location (GATE-613, kb PHN-537): Android's own
 * question for the precise location only now, and only if it isn't given yet; then one fix, or why there is none.
 */
@Composable
fun rememberExact(activity: MainActivity, vm: FerbViewModel): (String) -> Unit {
    var asking by remember { mutableStateOf<String?>(null) }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { result ->
        val id = asking ?: return@rememberLauncherForActivityResult
        asking = null
        // the owner may allow only the approximate location: a fix still goes, with its (coarse) accuracy
        vm.shareLocation(id, result.values.any { it })
    }
    return { id ->
        if (activity.checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED) {
            vm.shareLocation(id, true)
        } else {
            asking = id
            ask.launch(arrayOf(Manifest.permission.ACCESS_FINE_LOCATION, Manifest.permission.ACCESS_COARSE_LOCATION))
        }
    }
}

/**
 * Speaking to FERB (PHN-604): the owner read FERB's words for this phone's kind of recognition ([kind]; null when he
 * had already); Android's microphone question follows only if it isn't answered. Listening starts at his next hold,
 * never by itself after the question.
 */
@Composable
fun rememberMicrophone(activity: MainActivity, vm: FerbViewModel): (com.ferb.app.model.SpeechKind?) -> Unit {
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted -> vm.microphoneAnswered(granted) }
    return { kind ->
        kind?.let(vm::voiceTold)
        if (activity.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) {
            if (kind != null) vm.microphoneAnswered(true)
        } else {
            ask.launch(Manifest.permission.RECORD_AUDIO)
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
