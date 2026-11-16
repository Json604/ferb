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
 * PHN-8). kb PHN-498.
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
    val scope = MainScope()

    /** The activity on screen, for the system's fingerprint prompt (never kept beyond its life). */
    @Volatile var front: WeakReference<Activity> = WeakReference(null)

    override fun onCreate() {
        super.onCreate()
        prefs = Prefs(this)
        session = Session { SystemClock.elapsedRealtime() }
        phone = FerbPhone.create(this)
        api = LibraryPhone(phone)
        signals = AndroidSignals(this, phone)
        signals.stayConnected = prefs.stayConnected && StayConnectedService.running
        notifier = Notifier(this, api, prefs, session)
        local = LocalFiles.forPhone(this)
        scope.launch { phone.start() }
        signals.start()
        // the phone's own facts for FERB on the Mac (T-1214): from Android's callbacks, sent when they change
        reporter = DeviceReporter(DeviceSensors.snapshot(this))
        sensors = DeviceSensors(this, reporter, prefs, scope)
        sensors.start()
        reporter.start(scope, phone.state, phone.link) { api.reportDevice(it) }
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

    /** PHN-8: a phone without a screen lock cannot use the app. */
    fun screenLockSet(): Boolean = getSystemService(KeyguardManager::class.java).isDeviceSecure
}

/**
 * PHN-15: when the app locks (five minutes away, or by hand) the Mac is told (`lock`: its unlock window for this phone
 * ends and the conversation's watch stops until the next unlock, PHN-125) and the fetched files go. A cold start is
 * locked already and tells nothing.
 */
fun tellMacOnLock(scope: kotlinx.coroutines.CoroutineScope, session: Session, api: PhoneApi, local: LocalFiles) = scope.launch {
    var was = session.locked.value
    session.locked.collect { locked ->
        if (locked && !was) {
            api.lock()
            withContext(Dispatchers.IO) { local.wipeFetched() }
        }
        was = locked
    }
}

/**
 * The app's lock over the process's life (PHN-8, PHN-15): [AppLock]'s rule, the foreground as Android says it, and
 * the timer that locks the app when five minutes pass in the background.
 */
class Session(now: () -> Long) {
    val lock = AppLock(now)
    private val _locked = MutableStateFlow(true)
    val locked: StateFlow<Boolean> = _locked.asStateFlow()
    private val _front = MutableStateFlow(false)
    val front: StateFlow<Boolean> = _front.asStateFlow()
    private val handler = Handler(Looper.getMainLooper())
    private val timer = Runnable { if (lock.backgroundTimer()) _locked.value = true }

    fun foreground(on: Boolean) {
        _front.value = on
        if (on) {
            handler.removeCallbacks(timer)
            lock.cameToFront()
        } else {
            lock.wentToBackground()
            handler.postDelayed(timer, AppLock.LOCK_AFTER_MS + 1_000)
        }
        _locked.value = lock.locked
    }

    fun unlocked() {
        lock.unlocked()
        _locked.value = false
    }

    fun lockNow() {
        lock.lock()
        _locked.value = true
    }
}
