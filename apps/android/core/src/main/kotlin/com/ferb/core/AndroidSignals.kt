package com.ferb.core

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Handler
import android.os.Looper
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import java.io.Closeable
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * The phone's own signals to the link: whether the app is in front (ProcessLifecycleOwner: the first activity started,
 * the last stopped) and the default network's kind (ConnectivityManager). The link closes in the background unless
 * [stayConnected] (S11's foreground service, PHN-421) keeps it active, and dials once when a new network appears.
 * The unlock toward the Mac (PHN-125) needs a prompt, so it is the app's to start when [foreground] turns true.
 */
class AndroidSignals(context: Context, private val phone: FerbPhone) : Closeable {
    private val connectivity = context.applicationContext.getSystemService(ConnectivityManager::class.java)
    private val mainHandler = Handler(Looper.getMainLooper())
    private val _foreground = MutableStateFlow(false)
    val foreground: StateFlow<Boolean> = _foreground.asStateFlow()

    @Volatile var stayConnected: Boolean = false
        set(value) {
            field = value
            phone.setActive(_foreground.value || value)
        }

    /** The default network last seen; the one the phone is on when [start] runs is where the link began, not a change. */
    @Volatile private var current: Network? = null
    private val changed = Runnable { phone.networkChanged(current?.let(::kindOf) ?: "none") }

    private val lifecycle = object : DefaultLifecycleObserver {
        override fun onStart(owner: LifecycleOwner) {
            _foreground.value = true
            phone.setActive(true)
        }

        override fun onStop(owner: LifecycleOwner) {
            _foreground.value = false
            phone.setActive(stayConnected)
        }
    }

    // Only another default network than the last one seen, or its loss, is a change (Wi-Fi to mobile data, one Wi-Fi to
    // another); reported once after 400 ms of quiet, as the phone lab did (Link.kt), since a handover arrives as several
    // callbacks. The link tells iroh, which rebinds; it rebuilds nothing.
    private val network = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            if (network == current) return
            current = network
            mainHandler.removeCallbacks(changed)
            mainHandler.postDelayed(changed, 400)
        }

        override fun onLost(network: Network) {
            if (network != current) return
            current = null
            mainHandler.removeCallbacks(changed)
            mainHandler.postDelayed(changed, 400)
        }
    }

    fun start() {
        current = connectivity.activeNetwork
        mainHandler.post { ProcessLifecycleOwner.get().lifecycle.addObserver(lifecycle) }
        connectivity.registerDefaultNetworkCallback(network, mainHandler)
    }

    override fun close() {
        mainHandler.removeCallbacks(changed)
        mainHandler.post { ProcessLifecycleOwner.get().lifecycle.removeObserver(lifecycle) }
        runCatching { connectivity.unregisterNetworkCallback(network) }
    }

    private fun kindOf(network: Network): String =
        connectivity.getNetworkCapabilities(network)?.let(::kindOf) ?: "unknown"

    companion object {
        fun kindOf(caps: NetworkCapabilities): String = when {
            caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN) -> "vpn"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> "wifi"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> "cellular"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> "ethernet"
            else -> "other"
        }
    }
}
