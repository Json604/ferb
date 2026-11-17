package com.ferb.app

import android.content.Intent
import android.hardware.biometrics.BiometricManager.Authenticators
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.Bundle
import android.os.CancellationSignal
import android.view.WindowInsetsController
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.lifecycle.createSavedStateHandle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import com.ferb.app.model.Corpus
import com.ferb.app.model.Why
import com.ferb.app.ui.FerbRoot
import com.ferb.core.KeyKind
import com.ferb.core.PromptText
import com.ferb.mobile.Signer
import kotlin.coroutines.resume
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.combine
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
                val corpus = runCatching { a.assets.open("corpus.json").bufferedReader().use { Corpus(it.readText()) } }.getOrNull()
                FerbViewModel(a.api, a.session, a.prefs, a::screenLockSet, corpus, createSavedStateHandle(), a.local,
                    locator = AndroidLocator(a), coarse = a.sensors.coarse, say = a::getString, locationAnswered = a.notifier::locationAnswered,
                    voice = AndroidSpeech(a), micAllowed = { a.checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) == android.content.pm.PackageManager.PERMISSION_GRANTED })
            }
        }
    }

    /** The owner's screenshot switch (PHN-420); the view model's mustSecure overrides it (PHN-110, PHN-454). */
    val blockScreenshots = MutableStateFlow(true)

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        blockScreenshots.value = app.prefs.blockScreenshots
        // PHN-15: the app's content is covered in recent apps at once, whatever the screenshot switch says
        if (Build.VERSION.SDK_INT >= 33) setRecentsScreenshotEnabled(false)
        secure(true)
        lifecycleScope.launch {
            combine(blockScreenshots, vm.mustSecure, app.session.locked) { block, must, locked -> block || must || locked }.collect(::secure)
        }
        vm.prompts = ActivityPrompts()
        handle(intent)
        setContent { FerbRoot(vm, this) }
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
        vm.windowReady(hasWindowFocus())
    }

    override fun onPause() {
        vm.windowReady(false)
        super.onPause()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        vm.windowReady(hasFocus && lifecycle.currentState.isAtLeast(androidx.lifecycle.Lifecycle.State.RESUMED))
    }

    override fun onDestroy() {
        if (vm.prompts is ActivityPrompts) vm.prompts = null
        super.onDestroy()
    }

    private fun handle(intent: Intent?) {
        val i = intent ?: return
        i.getStringExtra(EXTRA_QUESTION)?.let { vm.openQuestion(it) }
        i.getStringExtra(EXTRA_ENTRY)?.let { vm.openChat() }
        i.getStringExtra(EXTRA_LOCATION)?.let { vm.openLocation(it) }
        i.removeExtra(EXTRA_QUESTION)
        i.removeExtra(EXTRA_ENTRY)
        i.removeExtra(EXTRA_LOCATION)
    }

    private fun secure(on: Boolean) {
        if (on) window.setFlags(WindowManager.LayoutParams.FLAG_SECURE, WindowManager.LayoutParams.FLAG_SECURE)
        else window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
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
    }
}
