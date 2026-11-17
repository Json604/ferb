package com.ferb.app

import android.Manifest
import android.app.KeyguardManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.location.Address
import android.location.Geocoder
import android.location.Location
import android.location.LocationListener
import android.location.LocationManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.BatteryManager
import android.os.Build
import android.os.CancellationSignal
import android.os.Handler
import android.os.LocaleList
import android.os.Looper
import android.os.PowerManager
import android.provider.Settings
import com.ferb.app.model.Coarse
import com.ferb.app.model.DeviceFacts
import com.ferb.app.model.DeviceReporter
import com.ferb.app.model.PlaceKeeper
import com.ferb.mobile.DevicePlace
import com.ferb.mobile.LocationFix
import java.util.Locale
import java.util.TimeZone
import kotlin.coroutines.resume
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

/**
 * The phone's facts from Android, each from its own callback or broadcast, registered while the process runs and never
 * read on a timer (T-1214, kb PHN-530): the time zone (ACTION_TIMEZONE_CHANGED), the battery (ACTION_BATTERY_CHANGED,
 * counted in 5-point steps by [DeviceReporter]) and battery saver (ACTION_POWER_SAVE_MODE_CHANGED), the network's kind
 * (the default-network callback: no Wi-Fi name, which needs location), the language (ACTION_LOCALE_CHANGED), the screen
 * (ACTION_SCREEN_ON/OFF, ACTION_USER_PRESENT with KeyguardManager), the model and Android version, and the phone's name
 * where Android gives it without a permission.
 *
 * The city only when the owner turned it on in This phone and Android allowed ACCESS_COARSE_LOCATION (SENSE-420, kb
 * PHN-638): fixes from the passive provider with a 1 km minimum distance (no cost of its own: it hears fixes other apps
 * asked for) and one fresh network fix when turned on or at start; the city from Android's Geocoder, asked only after a
 * move of a kilometre or more ([PlaceKeeper]). Without ACCESS_BACKGROUND_LOCATION (never asked) Android gives this app
 * little or no location while it is in the background.
 */
class DeviceSensors(
    context: Context,
    private val reporter: DeviceReporter,
    private val prefs: Prefs,
    private val scope: CoroutineScope,
    geocode: suspend (Double, Double) -> DevicePlace? = platformGeocoder(context.applicationContext),
) {
    private val app = context.applicationContext
    private val keyguard = app.getSystemService(KeyguardManager::class.java)
    private val power = app.getSystemService(PowerManager::class.java)
    private val connectivity = app.getSystemService(ConnectivityManager::class.java)
    private val locations: LocationManager? = app.getSystemService(LocationManager::class.java)
    private val main = Handler(Looper.getMainLooper())
    private val keeper = PlaceKeeper(geocode)

    /** The last coarse fix, rounded (the sky's place while the city is shared), or null. */
    val coarse: StateFlow<Coarse?> = keeper.coarse

    private var listening = false

    private val events = object : BroadcastReceiver() {
        override fun onReceive(c: Context, i: Intent) {
            when (i.action) {
                Intent.ACTION_TIMEZONE_CHANGED -> zone(i.getStringExtra(Intent.EXTRA_TIMEZONE))
                Intent.ACTION_BATTERY_CHANGED -> battery(i)
                PowerManager.ACTION_POWER_SAVE_MODE_CHANGED -> reporter.powerSaver(power.isPowerSaveMode)
                Intent.ACTION_LOCALE_CHANGED -> locale()
                Intent.ACTION_SCREEN_ON -> reporter.screen(true, keyguard.isKeyguardLocked)
                Intent.ACTION_SCREEN_OFF -> reporter.screen(false, keyguard.isKeyguardLocked)
                Intent.ACTION_USER_PRESENT -> reporter.screen(true, false)
                LocationManager.MODE_CHANGED_ACTION -> city()
            }
            // a daylight-saving change has no broadcast of its own: the offset is read again with every other one
            if (i.action != Intent.ACTION_TIMEZONE_CHANGED) zone(null)
        }
    }

    private val network = object : ConnectivityManager.NetworkCallback() {
        override fun onCapabilitiesChanged(n: Network, caps: NetworkCapabilities) = reporter.network(kindOf(caps))
        override fun onLost(n: Network) = reporter.network(OFFLINE)
    }

    private val passive = LocationListener { fix(it) }

    /** The facts Android gives without waiting for an event: what the report starts from. */
    fun snapshot(): DeviceFacts = snapshot(app)

    /** Registers every callback (once per process). The battery's sticky broadcast answers at once. */
    fun start() {
        val filter = IntentFilter().apply {
            addAction(Intent.ACTION_TIMEZONE_CHANGED)
            addAction(Intent.ACTION_BATTERY_CHANGED)
            addAction(PowerManager.ACTION_POWER_SAVE_MODE_CHANGED)
            addAction(Intent.ACTION_LOCALE_CHANGED)
            addAction(Intent.ACTION_SCREEN_ON)
            addAction(Intent.ACTION_SCREEN_OFF)
            addAction(Intent.ACTION_USER_PRESENT)
            addAction(LocationManager.MODE_CHANGED_ACTION)
        }
        app.registerReceiver(events, filter, Context.RECEIVER_NOT_EXPORTED)?.let(::battery)
        runCatching { connectivity.registerDefaultNetworkCallback(network, main) }
        city()
    }

    /**
     * The city's state from the owner's switch, Android's permission and whether location is on; listens for fixes only
     * while all three allow it. Called at start, when the switch changes and when location is turned on or off.
     */
    fun city() {
        val state = when {
            !prefs.shareCity -> DeviceFacts.LOCATION_OFF
            !granted(Manifest.permission.ACCESS_COARSE_LOCATION) -> DeviceFacts.LOCATION_DENIED
            locations == null || !locations.isLocationEnabled -> DeviceFacts.LOCATION_UNAVAILABLE
            else -> DeviceFacts.LOCATION_ON
        }
        if (state != DeviceFacts.LOCATION_ON) {
            stopListening()
            if (state == DeviceFacts.LOCATION_OFF) keeper.forget()
            reporter.location(state, null)
            return
        }
        reporter.location(state, keeper.place)
        if (listening) return
        listening = true
        val lm = locations ?: return
        try {
            lm.requestLocationUpdates(LocationManager.PASSIVE_PROVIDER, 0L, PlaceKeeper.MOVE_M.toFloat(), passive, Looper.getMainLooper())
            lm.getLastKnownLocation(LocationManager.PASSIVE_PROVIDER)?.let(::fix)
        } catch (e: SecurityException) {
            listening = false
            reporter.location(DeviceFacts.LOCATION_DENIED, null)
            return
        } catch (e: IllegalArgumentException) {
            // no passive provider on this phone: the one fresh fix below is all
        }
        // one fresh fix now (the network's: Wi-Fi and cell towers, no satellite)
        val provider = listOf(LocationManager.NETWORK_PROVIDER, FUSED).firstOrNull { p -> runCatching { lm.isProviderEnabled(p) }.getOrDefault(false) } ?: return
        runCatching { lm.getCurrentLocation(provider, CancellationSignal(), app.mainExecutor) { it?.let(::fix) } }
    }

    private fun stopListening() {
        if (!listening) return
        listening = false
        runCatching { locations?.removeUpdates(passive) }
    }

    private fun fix(l: Location) {
        scope.launch {
            val place = keeper.fix(l.latitude, l.longitude)
            if (listening) reporter.location(DeviceFacts.LOCATION_ON, place)
        }
    }

    private fun granted(permission: String) = app.checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED

    private fun zone(id: String?) {
        val tz = id?.let(TimeZone::getTimeZone) ?: TimeZone.getDefault()
        reporter.zone(tz.id, tz.getOffset(System.currentTimeMillis()) / 60_000)
    }

    private fun battery(i: Intent) {
        val level = i.getIntExtra(BatteryManager.EXTRA_LEVEL, -1)
        val scale = i.getIntExtra(BatteryManager.EXTRA_SCALE, -1)
        if (level < 0 || scale <= 0) return
        val status = i.getIntExtra(BatteryManager.EXTRA_STATUS, -1)
        val charging = status == BatteryManager.BATTERY_STATUS_CHARGING || status == BatteryManager.BATTERY_STATUS_FULL
        reporter.battery(level * 100 / scale, charging)
    }

    private fun locale() {
        val l = LocaleList.getDefault()
        reporter.locale(Locale.getDefault().toLanguageTag(), (0 until l.size()).map { l[it].toLanguageTag() })
    }

    companion object {
        const val OFFLINE = "offline"
        const val FUSED = "fused"

        /** The facts Android gives without waiting for an event (no permission needed for any). */
        fun snapshot(app: Context): DeviceFacts {
            val keyguard = app.getSystemService(KeyguardManager::class.java)
            val power = app.getSystemService(PowerManager::class.java)
            val connectivity = app.getSystemService(ConnectivityManager::class.java)
            val tz = TimeZone.getDefault()
            val name = runCatching { Settings.Global.getString(app.contentResolver, Settings.Global.DEVICE_NAME) }.getOrNull()?.trim().orEmpty()
            val langs = LocaleList.getDefault().let { l -> (0 until l.size()).map { l[it].toLanguageTag() } }
            return DeviceFacts(
                name = name,
                model = listOf(Build.MANUFACTURER, Build.MODEL).filter { !it.isNullOrBlank() }.joinToString(" "),
                os = "Android ${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT})",
                timeZone = tz.id,
                utcOffsetMinutes = tz.getOffset(System.currentTimeMillis()) / 60_000,
                powerSaver = power.isPowerSaveMode,
                network = connectivity.activeNetwork?.let { connectivity.getNetworkCapabilities(it) }?.let(::kindOf) ?: OFFLINE,
                locale = Locale.getDefault().toLanguageTag(),
                languages = langs,
                screenOn = power.isInteractive,
                screenLocked = keyguard.isKeyguardLocked,
            )
        }

        fun kindOf(caps: NetworkCapabilities): String = when {
            caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> "wifi"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> "mobile"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> "ethernet"
            else -> "other"
        }

        /** City, region, country and its code: never the street, never the coordinates. */
        fun placeOf(a: Address): DevicePlace? {
            val city = a.locality ?: a.subAdminArea ?: return null
            return DevicePlace(city, a.adminArea.orEmpty(), a.countryName.orEmpty(), a.countryCode.orEmpty())
        }
    }
}

/**
 * Android's own Geocoder (the phone's geocoding service; on most phones Google's): it is sent the coarse coordinates
 * and answers the address, of which only the city, region and country are kept. The listener form on Android 13 and
 * later, else the blocking one off the main thread. Null when the phone has none or it doesn't answer.
 */
fun platformGeocoder(context: Context): suspend (Double, Double) -> DevicePlace? = geocode@{ lat, lon ->
    if (!Geocoder.isPresent()) return@geocode null
    val g = Geocoder(context, Locale.getDefault())
    if (Build.VERSION.SDK_INT >= 33) {
        withTimeoutOrNull(GEOCODE_WAIT_MS) {
            suspendCancellableCoroutine { done ->
                g.getFromLocation(lat, lon, 1, object : Geocoder.GeocodeListener {
                    override fun onGeocode(list: MutableList<Address>) { if (done.isActive) done.resume(list.firstOrNull()?.let(DeviceSensors::placeOf)) }
                    override fun onError(message: String?) { if (done.isActive) done.resume(null) }
                })
            }
        }
    } else {
        withContext(Dispatchers.IO) {
            @Suppress("DEPRECATION")
            runCatching { g.getFromLocation(lat, lon, 1)?.firstOrNull()?.let(DeviceSensors::placeOf) }.getOrNull()
        }
    }
}

private const val GEOCODE_WAIT_MS = 15_000L

/** One exact fix for the Mac's request (GATE-613): the phone's own permission, then one reading. */
interface Locator {
    fun fine(): Boolean
    fun coarse(): Boolean

    /** One fresh fix (satellites where allowed, else the network), bounded by [timeoutMs]; null when none came. */
    suspend fun exact(timeoutMs: Long): LocationFix?
}

class AndroidLocator(context: Context) : Locator {
    private val app = context.applicationContext
    private val lm: LocationManager? = app.getSystemService(LocationManager::class.java)

    override fun fine() = app.checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED
    override fun coarse() = fine() || app.checkSelfPermission(Manifest.permission.ACCESS_COARSE_LOCATION) == PackageManager.PERMISSION_GRANTED

    override suspend fun exact(timeoutMs: Long): LocationFix? {
        val m = lm ?: return null
        val order = if (fine()) listOf(LocationManager.GPS_PROVIDER, LocationManager.NETWORK_PROVIDER, DeviceSensors.FUSED)
        else listOf(LocationManager.NETWORK_PROVIDER, DeviceSensors.FUSED)
        val provider = order.firstOrNull { p -> runCatching { m.isProviderEnabled(p) }.getOrDefault(false) } ?: return null
        val loc = withTimeoutOrNull(timeoutMs) {
            suspendCancellableCoroutine<Location?> { done ->
                val cancel = CancellationSignal()
                done.invokeOnCancellation { cancel.cancel() }
                try {
                    m.getCurrentLocation(provider, cancel, app.mainExecutor) { l -> if (done.isActive) done.resume(l) }
                } catch (e: SecurityException) {
                    if (done.isActive) done.resume(null)
                }
            }
        } ?: return null
        return LocationFix(loc.latitude, loc.longitude, if (loc.hasAccuracy()) loc.accuracy.toDouble() else -1.0)
    }
}
