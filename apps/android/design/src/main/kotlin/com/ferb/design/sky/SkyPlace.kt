package com.ferb.design.sky

import androidx.compose.ui.graphics.Color
import java.time.Instant
import java.time.ZoneId
import java.time.ZonedDateTime
import kotlin.math.floor
import kotlin.math.sin

/**
 * The sky's only inputs: a coarse place and the time (PHN-450). The place is the phone's one coarse location reading
 * when the owner allowed it, else the time zone's principal place (PHN-420). No weather (PHN-412).
 */
data class SkyPlace(val latitude: Double, val longitude: Double, val zone: ZoneId) {
    companion object {
        /** The time zone's principal place from the tz database, or its offset's meridian on the equator if unknown. */
        fun fromZone(zone: ZoneId, at: Instant = Instant.now()): SkyPlace {
            ZonePlaces.all[zone.id]?.let { (lat, lon) -> return SkyPlace(lat, lon, zone) }
            val offsetHours = zone.rules.getStandardOffset(at).totalSeconds / 3600.0
            return SkyPlace(0.0, offsetHours * 15, zone)
        }

        /** The lab's four places (MobileScreens.swift SkyPlace.all), for the gallery and tests. */
        val DELHI = SkyPlace(28.61, 77.21, ZoneId.of("Asia/Kolkata"))
        val LONDON = SkyPlace(51.51, -0.13, ZoneId.of("Europe/London"))
        val SAN_FRANCISCO = SkyPlace(37.77, -122.42, ZoneId.of("America/Los_Angeles"))
        val SYDNEY = SkyPlace(-33.87, 151.21, ZoneId.of("Australia/Sydney"))
    }
}

/** Everything the sky needs for one moment, computed once (the drawing animates elevation and across). */
data class SkyMoment(val epochSeconds: Double, val place: SkyPlace) {
    val sun: SunPosition = SunPosition.at(epochSeconds, place.latitude, place.longitude)
    val across: Double = SunPosition.across(sun.azimuth, place.latitude)
    val moon: MoonPhase = MoonPhase.at(epochSeconds)
    val look: SkyFrameLook get() = ChosenSky.look(sun.elevation, across)
    val ink: SkyInk get() = SkyInk.of(look)
    val local: ZonedDateTime get() = Instant.ofEpochSecond(floor(epochSeconds).toLong()).atZone(place.zone)
    val season: Season get() = Season.at(place, local.monthValue)

    /** Weather.swift Moment.night: the season layer's fireflies come out below -4 degrees. */
    val night: Boolean get() = sun.elevation < -4

    companion object {
        /** A local clock time on the given day at [place] (the lab's SkyPlace.date(hour:)). */
        fun at(place: SkyPlace, year: Int, month: Int, day: Int, hour: Double): SkyMoment {
            val start = java.time.LocalDate.of(year, month, day).atStartOfDay(place.zone).toEpochSecond()
            return SkyMoment(start + hour * 3600, place)
        }
    }
}

/** Mobile.swift SkyInk: black or white for what sits straight on the sky, by how light the sky is behind it. */
data class SkyInk(val top: Color, val low: Color) {
    val topIsDark: Boolean get() = top == Color.Black

    companion object {
        /** Mobile.swift SkyInk.init: the threshold is OKLab L 0.68. */
        const val THRESHOLD = 0.68
        val WHITE = SkyInk(Color.White, Color.White)

        fun of(look: SkyFrameLook) = SkyInk(
            if (look.lightTop > THRESHOLD) Color.Black else Color.White,
            if (look.lightLow > THRESHOLD) Color.Black else Color.White,
        )
    }
}

/** Weather.swift Season.at: South Asia has its monsoon (June to September); the south of the world is reversed. */
enum class Season {
    SPRING, SUMMER, MONSOON, AUTUMN, WINTER;

    companion object {
        fun at(place: SkyPlace, month: Int) = at(place.latitude, place.longitude, month)

        fun at(latitude: Double, longitude: Double, month: Int): Season {
            if (latitude in 5.0..35.0 && longitude in 60.0..100.0 && month in 6..9) return MONSOON
            val m = if (latitude >= 0) month else (month + 5) % 12 + 1
            return when (m) {
                in 3..5 -> SPRING
                in 6..8 -> SUMMER
                in 9..11 -> AUTUMN
                else -> WINTER
            }
        }
    }
}

/** Sky.swift Stars: fixed stars from a seeded generator, so nothing twinkles or moves on its own. */
object SkyStars {
    /** (x, y) as fractions of the sky, size in points, alpha. Sky.swift draws 38 with seed 7. */
    fun generate(count: Int = 38, seed: ULong = 7uL): List<DoubleArray> {
        var state = seed
        fun next(): Double {
            state = state * 6364136223846793005uL + 1442695040888963407uL
            return (state shr 33).toDouble() / (1L shl 31).toDouble()
        }
        return List(count) {
            val x = next()
            val y = next()
            val s = 0.6 + next() * 1.3
            val a = 0.35 + next() * 0.6
            doubleArrayOf(x, y, s, a)
        }
    }
}

/** Weather.swift WeatherLayer.r: the layer's fixed pseudo-random numbers. */
internal fun seasonRandom(i: Int, k: Int): Double {
    val x = sin(i * 12.9898 + k * 78.233) * 43758.5453
    return x - floor(x)
}
