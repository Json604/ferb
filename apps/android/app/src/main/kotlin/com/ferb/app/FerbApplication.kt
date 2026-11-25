package com.ferb.app

import android.app.Activity
import android.app.Application
import android.app.KeyguardManager
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import com.ferb.app.model.AppLock
import com.ferb.app.model.DeviceReporter
import com.ferb.app.model.LibraryPhone
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.PhoneApi
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import com.ferb.core.AndroidSignals
import com.ferb.core.FerbPhone
import java.lang.ref.WeakReference
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * The process's one FERB: the Application owns the phone object (FerbPhone.create, start), the Android signals and
 * the app lock, so a screen turned or rebuilt keeps them and only a new process starts again (and is locked again,
 * PHN-835). kb PHN-498.
 */
class FerbApplication : Application() {
    lateinit var phone: FerbPhone
        private set
    lateinit var api: PhoneApi
        private set
    lateinit var signals: AndroidSignals
        private set
    lateinit var prefs: Prefs
        private set
    lateinit var session: Session
        private set
    lateinit var notifier: Notifier
        private set
    lateinit var local: LocalFiles
        private set
    lateinit var reporter: DeviceReporter
        private set
    lateinit var sensors: DeviceSensors
        private set

    /** FERB's account on this phone (kb PHN-827): sealed under its own Keystore key, read once at the start. */
    lateinit var account: com.ferb.app.model.AccountModel
        private set
    val scope = MainScope()

    /** An activity was made in this process already: the next one is a warm start (StartLog, the launch). */
    var launched = false

    /** The activity on screen, for the system's fingerprint prompt (never kept beyond its life). */
    @Volatile var front: WeakReference<Activity> = WeakReference(null)

    /**
     * The home screen's words (the lab's corpus.json), read on a background thread from the start so the activity's
     * first frame never waits for them (PHN-633); the view model takes them when it is made.
     */
    private val corpusLoad = java.util.concurrent.FutureTask {
        runCatching { assets.open("corpus.json").bufferedReader().use { com.ferb.app.model.Corpus(it.readText()) } }.getOrNull()
    }

    /** The corpus, waiting for its read if it is still under way (null when it can't be read). */
    fun corpus(): com.ferb.app.model.Corpus? = runCatching { corpusLoad.get() }.getOrNull()

    override fun onCreate() {
        super.onCreate()
        // kb PHN-657: on a debuggable build the app's lines are kept in a file too, and a stalled main thread says where
        if (applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE != 0) {
            AppLog.keepIn(java.io.File(filesDir, "ferb-app.log"))
            AppLog.watchMain()
        }
        StartLog.once("application")
        Thread(corpusLoad, "ferb-corpus").start()
        prefs = Prefs(this)
        session = Session { SystemClock.elapsedRealtime() }
        account = com.ferb.app.model.AccountModel(
            com.ferb.app.model.SealedAccountStore(java.io.File(noBackupFilesDir, "account"), com.ferb.core.KeystoreProtector(this, "ferb.key.account")),
            com.ferb.app.model.AccountServer(com.ferb.app.model.UrlConnectionHttp(), com.ferb.app.model.Device(prefs.deviceId, BuildConfig.VERSION_NAME),
                System::currentTimeMillis),
            Dispatchers.IO, System::currentTimeMillis)
        scope.launch { account.load() }
        phone = FerbPhone.create(this)
        api = LibraryPhone(phone)
        signals = AndroidSignals(this, phone)
        signals.stayConnected = prefs.stayConnected && StayConnectedService.running
        notifier = Notifier(this, api, prefs, session)
        local = LocalFiles.forPhone(this)
        scope.launch {
            val began = SystemClock.uptimeMillis()
            val s = phone.start()
            StartLog.once("phone open", "${s.javaClass.simpleName}, ${SystemClock.uptimeMillis() - began} ms: library, keys, link and store")
        }
        signals.start()
        // the phone's own facts for FERB on the Mac (T-1214): from Android's callbacks, sent when they change
        // the first facts ask several system services (a second on a busy phone, kb PHN-633): read off the main thread,
        // then the callbacks and the report start from them
        reporter = DeviceReporter(com.ferb.app.model.DeviceFacts())
        sensors = DeviceSensors(this, reporter, prefs, scope)
        scope.launch {
            reporter.seed(withContext(Dispatchers.Default) { DeviceSensors.snapshot(this@FerbApplication) })
            sensors.start()
            reporter.start(scope, phone.state, phone.link) { api.reportDevice(it) }
        }
        scope.launch { signals.foreground.collect { session.foreground(it) } }
        tellMacOnLock(scope, session, api, local)
        notifier.start(scope)
        registerActivityLifecycleCallbacks(object : ActivityLifecycleCallbacks {
            override fun onActivityResumed(a: Activity) { front = WeakReference(a) }
            override fun onActivityPaused(a: Activity) { if (front.get() === a) front = WeakReference(null) }
            override fun onActivityCreated(a: Activity, b: Bundle?) {}
            override fun onActivityStarted(a: Activity) {}
            override fun onActivityStopped(a: Activity) {}
            override fun onActivitySaveInstanceState(a: Activity, b: Bundle) {}
            override fun onActivityDestroyed(a: Activity) {}
        })
    }

    /** kb PHN-845: a phone without a screen lock cannot use the app, whether the app's own lock is on or off. */
    fun screenLockSet(): Boolean = getSystemService(KeyguardManager::class.java).isDeviceSecure
}

/**
 * PHN-15: when the app locks (five minutes away, or by hand) the Mac is told (`lock`: its unlock window for this phone
 * ends and the conversation's watch stops until the next unlock, PHN-125) and the fetched files go. A cold start is
 * locked already and tells nothing. Counted by [Session.locks], not read from [Session.locked]: with the owner's lock
 * switch off (kb PHN-845) the app opens again at once, and a lock seen only as a state could be missed between the two.
 */
fun tellMacOnLock(scope: kotlinx.coroutines.CoroutineScope, session: Session, api: PhoneApi, local: LocalFiles) = scope.launch {
    var told = session.locks.value
    session.locks.collect { n ->
        if (n != told) {
            told = n
            api.lock()
            withContext(Dispatchers.IO) { local.wipeFetched() }
        }
    }
}

/**
 * The app's lock over the process's life (PHN-835, PHN-15): [AppLock]'s rule, the foreground as Android says it, and
 * the timer that locks the app when five minutes pass in the background. With the owner's lock switch off (PHN-845)
 * it locks the same way, so the Mac is told and the fetched files go; the view model then opens it without the prompt.
 */
class Session(now: () -> Long) {
    val lock = AppLock(now)
    private val _locked = MutableStateFlow(true)
    val locked: StateFlow<Boolean> = _locked.asStateFlow()
    private val _front = MutableStateFlow(false)
    val front: StateFlow<Boolean> = _front.asStateFlow()
    private val _promptDue = MutableStateFlow(false)
    private val _locks = MutableStateFlow(0)

    /** How many times the app has locked since the process began (a cold start's lock isn't one): [tellMacOnLock]. */
    val locks: StateFlow<Int> = _locks.asStateFlow()

    /** The unlock prompt should appear by itself (PHN-615): the app arrived in front locked. */
    val promptDue: StateFlow<Boolean> = _promptDue.asStateFlow()
    private val handler = Handler(Looper.getMainLooper())
    private val timer = Runnable { if (lock.backgroundTimer()) publish() }

    private fun publish() {
        if (lock.locked && !_locked.value) _locks.value += 1
        _locked.value = lock.locked
        _promptDue.value = lock.promptDue
    }

    fun foreground(on: Boolean) {
        _front.value = on
        if (on) {
            handler.removeCallbacks(timer)
            lock.cameToFront()
        } else {
            lock.wentToBackground()
            handler.postDelayed(timer, AppLock.LOCK_AFTER_MS + 1_000)
        }
        publish()
    }

    fun unlocked() {
        lock.unlocked()
        publish()
    }

    /** The owner's own lock from inside the app: the lock page waits for his tap (no prompt by itself, PHN-616). */
    fun lockNow() {
        lock.lock()
        publish()
    }

    /** Takes the due prompt (once per arrival). */
    fun takePrompt(): Boolean = lock.takePrompt().also { publish() }

    /** The taken prompt wasn't shown (the screen went elsewhere first): due again. */
    fun promptAgain() {
        lock.promptAgain()
        publish()
    }
}
