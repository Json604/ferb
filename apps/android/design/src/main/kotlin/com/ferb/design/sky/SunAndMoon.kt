package com.ferb.design.sky

import java.time.Instant
import java.time.ZoneId
import kotlin.math.asin
import kotlin.math.atan2
import kotlin.math.cos
import kotlin.math.floor
import kotlin.math.max
import kotlin.math.min
import kotlin.math.sin
import kotlin.math.tan

// Where the sun and the moon really are. From the glass lab's Sources/FerbGlass/Sky.swift (SunPosition, SunDay,
// MoonPhase, SkyView.across): the low-precision formulas of the U.S. Naval Observatory and NOAA, good to about a
// tenth of a degree. Pure arithmetic on the phone; the place it is given never leaves the device (PHN-450).

/** The sun's place in the sky. [elevation] in degrees above the horizon; [azimuth] degrees clockwise from north. */
data class SunPosition(val elevation: Double, val azimuth: Double) {
    companion object {
        /** Sky.swift SunPosition.init(date:latitude:longitude:), with the date as seconds since 1970. */
        fun at(epochSeconds: Double, latitude: Double, longitude: Double): SunPosition {
            val rad = Math.PI / 180
            val n = epochSeconds / 86400 + 2440587.5 - 2451545.0
            val meanLongitude = (280.460 + 0.9856474 * n) % 360
            val anomaly = ((357.528 + 0.9856003 * n) % 360) * rad
            val eclipticLongitude = (meanLongitude + 1.915 * sin(anomaly) + 0.020 * sin(2 * anomaly)) * rad
            val obliquity = (23.439 - 0.0000004 * n) * rad
            val rightAscension = atan2(cos(obliquity) * sin(eclipticLongitude), cos(eclipticLongitude))
            val declination = asin(sin(obliquity) * sin(eclipticLongitude))
            val siderealHours = (18.697374558 + 24.06570982441908 * n) % 24
            val hourAngle = (siderealHours * 15 + longitude) * rad - rightAscension
            val lat = latitude * rad
            val altitude = asin(sin(lat) * sin(declination) + cos(lat) * cos(declination) * cos(hourAngle))
            var azimuth = atan2(-sin(hourAngle), tan(declination) * cos(lat) - sin(lat) * cos(hourAngle)) / rad
            if (azimuth < 0) azimuth += 360
            return SunPosition(elevation = altitude / rad, azimuth = azimuth)
        }

        /**
         * Sky.swift SkyView.across: facing the equator, the sun rises on the left and sets on the right; -1 far left,
         * 0 the middle, 1 far right.
         */
        fun across(azimuth: Double, latitude: Double): Double {
            var offset = azimuth - (if (latitude >= 0) 180 else 0)
            if (offset > 180) offset -= 360
            if (offset < -180) offset += 360
            return max(-1.0, min(1.0, (if (latitude >= 0) offset else -offset) / 120))
        }
    }
}

/** Today's sunrise and sunset in seconds since 1970, found by walking the day in 5-minute steps (Sky.swift SunDay). */
data class SunDay(val sunrise: Double?, val sunset: Double?) {
    companion object {
        /** The sun's disc touching the horizon. */
        const val HORIZON = -0.833

        fun around(epochSeconds: Double, latitude: Double, longitude: Double, zone: ZoneId): SunDay {
            val day = Instant.ofEpochSecond(floor(epochSeconds).toLong()).atZone(zone).toLocalDate()
            val start = day.atStartOfDay(zone).toEpochSecond().toDouble()
            var rise: Double? = null
            var set: Double? = null
            var before = SunPosition.at(start, latitude, longitude).elevation
            var minute = 5
            while (minute <= 1440) {
                val t = start + minute * 60.0
                val now = SunPosition.at(t, latitude, longitude).elevation
                if (before < HORIZON && now >= HORIZON && rise == null) rise = t
                if (before >= HORIZON && now < HORIZON && set == null) set = t
                before = now
                minute += 5
            }
            return SunDay(rise, set)
        }
    }
}

/** How much of the moon is lit and whether it is growing, from its age in the 29.53-day cycle (Sky.swift MoonPhase). */
data class MoonPhase(val age: Double) {
    val lit: Double get() = (1 - cos(age * 2 * Math.PI)) / 2
    val waxing: Boolean get() = age < 0.5

    companion object {
        fun at(epochSeconds: Double): MoonPhase {
            val days = epochSeconds / 86400 + 2440587.5 - 2451550.1
            val cycles = days / 29.530588853
            return MoonPhase(cycles - floor(cycles))
        }
    }
}
