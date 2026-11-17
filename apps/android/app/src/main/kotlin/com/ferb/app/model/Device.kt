package com.ferb.app.model

import com.ferb.core.Outcome
import com.ferb.core.PhoneState
import com.ferb.mobile.DevicePlace
import com.ferb.mobile.DeviceReport
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.LinkStatus
import kotlin.math.asin
import kotlin.math.cos
import kotlin.math.pow
import kotlin.math.roundToLong
import kotlin.math.sin
import kotlin.math.sqrt
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

// The phone's own facts for FERB on the Mac (T-1214, SENSE-420): what Android's callbacks and broadcasts last said,
// turned into the library's DeviceReport. Pure: the Android side (DeviceSensors) only calls in here, so the dedupe, the
// battery's steps and the place's 1 km rule are tested on the JVM.

/** The phone's facts as last read (empty or null until Android said them). Never coordinates. */
data class DeviceFacts(
    val name: String = "",
    val model: String = "",
    val os: String = "",
    val timeZone: String = "",
    val utcOffsetMinutes: Int = 0,
    /** "on", "off" (the owner's switch), "denied" (on, but Android's permission is gone) or "unavailable". */
    val location: String = LOCATION_OFF,
    val place: DevicePlace? = null,
    val batteryPercent: Int? = null,
    val charging: Boolean? = null,
    val powerSaver: Boolean? = null,
    /** "wifi", "mobile", "ethernet", "other", "offline", or "" before the first callback. */
    val network: String = "",
    val locale: String = "",
    val languages: List<String> = emptyList(),
    val screenOn: Boolean? = null,
    val screenLocked: Boolean? = null,
) {
    fun report(): DeviceReport = DeviceReport(
        name, model, os, timeZone, utcOffsetMinutes, location, place?.takeIf { location == LOCATION_ON },
        batteryPercent?.coerceIn(0, 100)?.toUByte(), charging, powerSaver, network, locale, languages, screenOn, screenLocked,
    )

    companion object {
        const val LOCATION_ON = "on"
        const val LOCATION_OFF = "off"
        const val LOCATION_DENIED = "denied"
        const val LOCATION_UNAVAILABLE = "unavailable"
    }
}

/**
 * Keeps [facts] from Android's events and hands each new report to the library ([start]): when it changed, when the
 * phone opens (a start, a new pairing) and when the link comes online. Nothing here runs on a timer: every change
 * comes from a callback, and the library sends only what this connection hasn't had (and again on a new one).
 * The battery counts only when its percent crosses a 5-point step or charging changes (ACTION_BATTERY_CHANGED fires
 * at every percent and voltage change). kb PHN-530, PHN-531.
 */
class DeviceReporter(initial: DeviceFacts) {
    private val _facts = MutableStateFlow(initial)
    val facts: StateFlow<DeviceFacts> = _facts.asStateFlow()

    /** The first facts, read off the main thread at start (before any callback and before the report starts). */
    fun seed(first: DeviceFacts) {
        _facts.value = first
    }

    fun zone(id: String, offsetMinutes: Int) = _facts.update { it.copy(timeZone = id, utcOffsetMinutes = offsetMinutes) }

    fun battery(percent: Int, charging: Boolean) = _facts.update { f ->
        val step = percent / BATTERY_STEP
        if (f.charging == charging && f.batteryPercent?.let { it / BATTERY_STEP } == step) f
        else f.copy(batteryPercent = percent, charging = charging)
    }

    fun powerSaver(on: Boolean) = _facts.update { it.copy(powerSaver = on) }
    fun network(kind: String) = _facts.update { it.copy(network = kind) }
    fun locale(tag: String, languages: List<String>) = _facts.update { it.copy(locale = tag, languages = languages) }
    fun screen(on: Boolean, locked: Boolean) = _facts.update { it.copy(screenOn = on, screenLocked = locked) }

    /** The owner's switch and Android's answer ([state]); the city once the geocoder named one (kept while on). */
    fun location(state: String, place: DevicePlace?) = _facts.update {
        it.copy(location = state, place = if (state == DeviceFacts.LOCATION_ON) place ?: it.place else null)
    }

    /**
     * Sends the report each time it, the phone's state or the link changes (all three are the library's own flows:
     * no polling), while the store is open. [sent] counts what was handed over (tests).
     */
    fun start(scope: CoroutineScope, state: StateFlow<PhoneState>, link: StateFlow<LinkStatus?>, send: suspend (DeviceReport) -> Outcome<Boolean>): Job =
        scope.launch {
            combine(_facts, state, link) { f, s, l -> Triple(f.report(), s is PhoneState.Open, l?.paired == true && l.phase == LinkPhase.ONLINE) }
                .distinctUntilChanged()
                .collect { (report, open, _) -> if (open) send(report) }
        }

    companion object {
        const val BATTERY_STEP = 5
    }
}

/** A coarse fix, rounded to 2 decimals (about a kilometre): the sky's place, in memory only. */
data class Coarse(val latitude: Double, val longitude: Double) {
    companion object {
        fun of(latitude: Double, longitude: Double) = Coarse(round2(latitude), round2(longitude))
        private fun round2(v: Double) = (v * 100).roundToLong() / 100.0
    }
}

/**
 * The city from coarse fixes: the geocoder is asked only for the first fix and after a move of [MOVE_M] or more from
 * the last place it named (so a phone that stays put asks it once), never on a timer. kb PHN-638.
 */
class PlaceKeeper(private val geocode: suspend (Double, Double) -> DevicePlace?) {
    private var named: Pair<Double, Double>? = null
    var place: DevicePlace? = null
        private set
    private val _coarse = MutableStateFlow<Coarse?>(null)
    val coarse: StateFlow<Coarse?> = _coarse.asStateFlow()

    /** A fix arrived: the sky's place moves; the city is looked up again only after a kilometre or more. */
    suspend fun fix(latitude: Double, longitude: Double): DevicePlace? {
        _coarse.value = Coarse.of(latitude, longitude)
        val last = named
        if (last != null && metres(last.first, last.second, latitude, longitude) < MOVE_M) return place
        named = latitude to longitude
        geocode(latitude, longitude)?.let { place = it }
        return place
    }

    /** The owner turned it off: nothing is kept. */
    fun forget() {
        named = null
        place = null
        _coarse.value = null
    }

    companion object {
        const val MOVE_M = 1_000.0

        /** The great-circle distance in metres (haversine). */
        fun metres(lat1: Double, lon1: Double, lat2: Double, lon2: Double): Double {
            val r = 6_371_000.0
            val dLat = Math.toRadians(lat2 - lat1)
            val dLon = Math.toRadians(lon2 - lon1)
            val a = sin(dLat / 2).pow(2) + cos(Math.toRadians(lat1)) * cos(Math.toRadians(lat2)) * sin(dLon / 2).pow(2)
            return 2 * r * asin(sqrt(a.coerceIn(0.0, 1.0)))
        }
    }
}
