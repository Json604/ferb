package com.ferb.app

import android.content.Intent
import android.hardware.biometrics.BiometricManager.Authenticators
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.Bundle
import android.os.CancellationSignal
import android.view.WindowInsetsController
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.runtime.snapshotFlow
import androidx.lifecycle.createSavedStateHandle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import com.ferb.app.model.Why
import com.ferb.app.ui.FerbRoot
import com.ferb.core.KeyKind
import com.ferb.design.LaunchState
import com.ferb.design.reducedMotion
import com.ferb.core.PromptText
import com.ferb.mobile.Signer
import kotlin.coroutines.resume
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine

/**
 * The one activity: edge to edge over the sky, the screen kept out of screenshots and recent apps (PHN-420, PHN-15),
 * the system's fingerprint prompts, and the screens (ui/). A notification's tap lands here with the question or the
 * chat entry to open.
 */
class MainActivity : ComponentActivity() {
    private val app get() = application as FerbApplication

    val vm: FerbViewModel by viewModels {
        viewModelFactory {
            initializer {
                val a = this@MainActivity.application as FerbApplication
                val corpus = a.corpus() // read on a background thread since the Application began (PHN-633)
                FerbViewModel(a.api, a.session, a.prefs, a::screenLockSet, corpus, createSavedStateHandle(), a.local,
                    locator = AndroidLocator(a), coarse = a.sensors.coarse, say = a::getString, locationAnswered = a.notifier::locationAnswered,
                    voice = AndroidSpeech(a), micAllowed = { a.checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) == android.content.pm.PackageManager.PERMISSION_GRANTED })
            }
        }
    }

    /** The owner's screenshot switch (PHN-420, the lock page too: PHN-793); mustSecure overrides it (PHN-110, PHN-454). */
    val blockScreenshots = MutableStateFlow(true)

    /**
     * The launch (kb PHN-631..634): FERB's mark from the first frame, settling into the lock page's mark. Only for a
     * new activity (a cold or warm start, never a turned screen); null once it ended or when there was none.
     */
    var launch: LaunchState? = null
        private set

    /** The launch's mark has settled enough for the system prompt (PHN-633); true when there is no launch. */
    private var launchSettled = true

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        val cold = StartLog.fresh() && !app.launched
        app.launched = true
        StartLog.each("activity", if (cold) "cold" else "warm")
        if (savedInstanceState == null) {
            // a cold start's splash began with the process; a warm one's time comes from the splash itself
            launch = LaunchState(reducedMotion(this), originNanos = if (cold) android.os.Process.getStartUptimeMillis() * 1_000_000 else null)
            launchSettled = false
        }
        if (Build.VERSION.SDK_INT >= 31) {
            launch?.awaitSplash()
            splashScreen.setOnExitAnimationListener { view ->
                // the app's launch continues the splash's icon animation from where it is, then the splash goes
                val began = view.iconAnimationStart
                if (began != null) launch?.syncToSplash(java.time.Duration.between(began, java.time.Instant.now()).toMillis().toFloat())
                else launch?.splashGone()
                view.remove()
            }
        }
        blockScreenshots.value = app.prefs.blockScreenshots
        // PHN-15: the app's content is covered in recent apps at once, whatever the screenshot switch says
        if (Build.VERSION.SDK_INT >= 33) setRecentsScreenshotEnabled(false)
        ScreenGuard.apply(window, true) // until the switch and the screen are read
        lifecycleScope.launch {
            // the lock page follows the switch too (PHN-793); a private screen is kept out whatever it says
            combine(blockScreenshots, vm.mustSecure, ScreenGuard::secure).collect { ScreenGuard.apply(window, it) }
        }
        vm.prompts = ActivityPrompts()
        // kb PHN-765: what keeps FERB's questions from reaching this phone, said in the app with its fix
        vm.notifyBlocker = { NotifyHealth.blocker(app, app.prefs.stayConnected) }
        handle(intent)
        setContent { FerbRoot(vm, this) }
        // the starting window's mark was only for before the first frame; the app draws its own from here
        window.setBackgroundDrawable(android.graphics.drawable.ColorDrawable(getColor(R.color.launch_ink0)))
        watchStart()
    }

    /** The start-up's milestones (StartLog) and the launch's end: the prompt may come once the mark has settled. */
    private fun watchStart() {
        val decor = window.decorView
        decor.viewTreeObserver.addOnDrawListener(object : android.view.ViewTreeObserver.OnDrawListener {
            private var drawn = false
            override fun onDraw() {
                if (drawn) return
                drawn = true
                StartLog.each("first frame")
                decor.post { decor.viewTreeObserver.removeOnDrawListener(this) }
            }
        })
        lifecycleScope.launch {
            val gate = vm.ui.first { it.gate != Gate.STARTING }.gate
            StartLog.each("page", gate.name)
        }
        val l = launch ?: return
        lifecycleScope.launch {
            snapshotFlow { l.promptOk }.first { it }
            launchSettled = true
            StartLog.each("launch settled")
            vm.windowReady(hasWindowFocus() && lifecycle.currentState.isAtLeast(androidx.lifecycle.Lifecycle.State.RESUMED))
            snapshotFlow { l.done }.first { it }
            launch = null
            reportFullyDrawn()
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    override fun onStart() {
        super.onStart()
        // Stay connected comes back with the app if it was on (a foreground service may only start from here)
        if (app.prefs.stayConnected && !StayConnectedService.running) runCatching { StayConnectedService.set(this, true) }
        vm.recheck()
    }

    // PHN-615: the unlock prompt by itself only from a resumed activity holding the window's focus. From onCreate the
    // system drops it; while a dialog, the shade or another prompt has the focus it would stack on top of them.
    override fun onResume() {
        super.onResume()
        vm.windowReady(hasWindowFocus() && launchSettled)
    }

    override fun onPause() {
        vm.windowReady(false)
        super.onPause()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        vm.windowReady(hasFocus && launchSettled && lifecycle.currentState.isAtLeast(androidx.lifecycle.Lifecycle.State.RESUMED))
    }

    override fun onDestroy() {
        if (vm.prompts is ActivityPrompts) vm.prompts = null
        super.onDestroy()
    }

    private fun handle(intent: Intent?) {
        val i = intent ?: return
        i.getStringExtra(EXTRA_QUESTION)?.let { vm.openQuestion(it) }
        // FERB's own question (a clarify) is answered by a message: the box takes the cursor (FERB.app's needs-you)
        i.getStringExtra(EXTRA_ENTRY)?.let { vm.openChat(focus = it == Notifier.CHAT) }
        i.getStringExtra(EXTRA_LOCATION)?.let { vm.openLocation(it) }
        if (i.getBooleanExtra(EXTRA_CAMERA, false)) vm.openCamera()
        i.removeExtra(EXTRA_CAMERA)
        i.removeExtra(EXTRA_QUESTION)
        i.removeExtra(EXTRA_ENTRY)
        i.removeExtra(EXTRA_LOCATION)
    }

    /** The status bar's icons dark over a light sky, light over a dark one. */
    fun lightBars(light: Boolean) {
        val c = window.insetsController ?: return
        val bits = WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS or WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS
        c.setSystemBarsAppearance(if (light) bits else 0, bits)
    }

    /**
     * The owner tapped Open or Share on a fetched file: the system's chooser, with read access to that one file for
     * the app he picks. A key place's copy goes when he is back in FERB (PHN-11). FERB never opens a file by itself.
     */
    fun handOut(key: String, file: java.io.File, name: String, share: Boolean) {
        if (!file.isFile) return
        val i = if (share) Handout.share(this, file, name) else Handout.open(this, file, name)
        runCatching { startActivity(i) }.onSuccess { vm.handedOut(key) }
    }

    /** The owner tapped Share on a fetched folder: one share sheet for its files, each read-only for the app he picks. */
    fun shareMany(files: List<java.io.File>) {
        val here = files.filter { it.isFile }
        if (here.isEmpty()) return
        runCatching { startActivity(Handout.shareMany(this, here)) }
    }

    fun setBlockScreenshots(on: Boolean) {
        app.prefs.blockScreenshots = on
        blockScreenshots.value = on
    }

    /** The system's prompts, in the app's words. */
    private inner class ActivityPrompts : Prompts {
        override fun signer(kind: KeyKind, why: Why): Signer {
            val title = when {
                why == Why.KEYS -> R.string.prompt_keys_title
                kind == KeyKind.STRONG -> R.string.prompt_yes_title
                else -> R.string.prompt_unlock_title
            }
            val subtitle = when (why) {
                Why.OPEN -> R.string.prompt_unlock_subtitle
                Why.YES -> R.string.prompt_yes_subtitle
                Why.FILE -> R.string.prompt_file_subtitle
                Why.SETTING -> R.string.prompt_setting_subtitle
                Why.KEYS -> R.string.prompt_keys_subtitle
                Why.INSOMNIA -> R.string.prompt_insomnia_subtitle
            }
            return app.phone.signer(this@MainActivity, kind, PromptText(getString(title), getString(subtitle), cancel = getString(R.string.prompt_cancel)))
        }

        override suspend fun local(): Boolean = suspendCancellableCoroutine { done ->
            val cancel = CancellationSignal()
            done.invokeOnCancellation { cancel.cancel() }
            val callback = object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) { if (done.isActive) done.resume(true) }
                override fun onAuthenticationError(code: Int, message: CharSequence) { if (done.isActive) done.resume(false) }
            }
            try {
                BiometricPrompt.Builder(this@MainActivity)
                    .setTitle(getString(R.string.prompt_unlock_title))
                    .setAllowedAuthenticators(Authenticators.BIOMETRIC_STRONG or Authenticators.DEVICE_CREDENTIAL)
                    .build()
                    .authenticate(cancel, mainExecutor, callback)
            } catch (e: Exception) {
                if (done.isActive) done.resume(false)
            }
        }
    }

    companion object {
        const val EXTRA_QUESTION = "com.ferb.app.QUESTION"
        const val EXTRA_ENTRY = "com.ferb.app.ENTRY"
        const val EXTRA_LOCATION = "com.ferb.app.LOCATION"
        /** The insomnia notice's Show me (APP-983): the Mac's camera, after the app's unlock. */
        const val EXTRA_CAMERA = "com.ferb.app.CAMERA"
    }
}
