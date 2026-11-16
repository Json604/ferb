package com.ferb.app.model

import com.ferb.design.sky.SkyMoment
import com.ferb.design.sky.SunDay
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale

/**
 * The home screen's words (the glass lab's words/corpus.json and Words.swift HomeTexts, the owner's copy): each slot
 * has rules; the highest-priority rule whose conditions all hold wins, and one of its lines is picked by the date, so
 * the screen says the same thing all day. A condition on something the phone does not measure here (the weather,
 * paused by PHN-412; the owner's usual hours and late nights, which need a list of open times whose keeping is the
 * owner's to decide) never holds, so those rules are skipped and the next true one speaks.
 */
class Corpus(json: String) {
    private val slots: Map<String, List<Rule>>

    data class Rule(val id: String, val priority: Double, val conditions: Map<String, Any?>, val lines: List<String>)

    init {
        @Suppress("UNCHECKED_CAST")
        val root = Json.parse(json) as Map<String, Any?>
        slots = root.obj("slots").orEmpty().mapValues { (_, rules) ->
            (rules as List<Any?>).map { r ->
                r as Map<String, Any?>
                Rule(r.str("id").orEmpty(), (r["priority"] as? Double) ?: 0.0, r.obj("when").orEmpty(), r.strings("lines"))
            }.sortedByDescending { it.priority }
        }
    }

    /** The slot's line for [m], or null when no rule holds. */
    fun line(slot: String, m: Moment): String? {
        val rule = slots[slot].orEmpty().firstOrNull { r -> r.conditions.all { (k, want) -> holds(k, want, m.signals[k]) } } ?: return null
        if (rule.lines.isEmpty()) return null
        // Words.swift Corpus.line: the day's ordinal in the era plus the slot's length times 7
        val pick = Math.floorMod(m.dayNumber + slot.length * 7, rule.lines.size)
        return fill(rule.lines[pick], m.fills)
    }

    fun texts(m: Moment): HomeWords = HomeWords(
        greeting = line("greeting", m) ?: "Hello",
        subline = line("subline", m).orEmpty(),
        away = line("away", m) ?: "While you were away",
        placeholder = line("placeholder", m) ?: "Ask FERB",
        chat = line("chat", m),
    )

    companion object {
        /** Words.swift Corpus.holds, case by case. */
        fun holds(key: String, want: Any?, have: Any?): Boolean {
            if (have == null || want == null) return false
            if (key == "hour" && want is String && have is Double) return inHours(want, have)
            if (want is List<*> && have is String) return want.contains(have)
            if (want is String && have is Number) return compare(want, have.toDouble())
            if (want is String && have is String) return want == have
            if (want is Boolean || have is Boolean) return want == have
            if (want is Number && have is Number) return want.toDouble() == have.toDouble()
            return false
        }

        private fun compare(s: String, x: Double): Boolean {
            val t = s.trim()
            for (op in listOf(">=", "<=", ">", "<")) if (t.startsWith(op)) {
                val n = t.drop(op.length).toDoubleOrNull() ?: return false
                return when (op) { ">=" -> x >= n; "<=" -> x <= n; ">" -> x > n; else -> x < n }
            }
            val parts = t.split("-").mapNotNull { it.toDoubleOrNull() }
            if (parts.size == 2) return x >= parts[0] && x <= parts[1]
            return t.toDoubleOrNull() == x
        }

        private fun inHours(range: String, h: Double): Boolean {
            val p = range.split("-").mapNotNull { it.toDoubleOrNull() }
            if (p.size != 2) return false
            return if (p[0] <= p[1]) h >= p[0] && h < p[1] else h >= p[0] || h < p[1]
        }

        fun fill(line: String, fills: Map<String, String>): String =
            fills.entries.fold(line) { out, (k, v) -> out.replace("{$k}", v) }
    }
}

data class HomeWords(val greeting: String, val subline: String, val away: String, val placeholder: String, val chat: String?)

/**
 * What the phone knows about the moment (Words.swift Moment, without the weather and the owner's habits): the sun
 * where the phone is, the clock, FERB's counts from the Mac, the link, and the app's own first-run and pairing.
 */
class Moment(
    sky: SkyMoment,
    waiting: Int,
    done: Int,
    failed: Int,
    running: Int,
    link: String,
    firstRun: Boolean,
    justPaired: Boolean,
    /** The previous open (epoch seconds), when the app knows it. */
    lastOpen: Double?,
    /** The time zone at the previous open. */
    lastZone: String?,
) {
    val signals: Map<String, Any?>
    val fills: Map<String, String>
    val dayNumber: Int

    init {
        val local = sky.local
        val zone: ZoneId = sky.place.zone
        val clock = DateTimeFormatter.ofPattern("h:mm a", Locale.US)
        fun at(t: Double) = Instant.ofEpochSecond(t.toLong()).atZone(zone).format(clock)
        val day = SunDay.around(sky.epochSeconds, sky.place.latitude, sky.place.longitude, zone)
        val now = sky.epochSeconds
        val e = sky.sun.elevation
        val across = sky.across
        // Words.swift Moment: phase, sunsetIn, sunriseIn, weekday (1 Sunday ... 7 Saturday), moon
        val phase = when {
            e < -10 -> "night"
            e < -2 -> if (across < 0) "dawn" else "dusk"
            e < 3 -> if (across < 0) "sunrise" else "sunset"
            across < 0 -> if (e < 12 || across < -0.15) "morning" else "midday"
            e < 12 -> "golden-hour"
            else -> if (across < 0.15) "midday" else "afternoon"
        }
        val sunsetIn = day.sunset?.takeIf { e > -0.833 && now < it }?.let { ((it - now) / 60).toInt() }
        val sunriseIn = day.sunrise?.takeIf { now < it }?.let { ((it - now) / 60).toInt() }
        val weekday = local.dayOfWeek.value % 7 + 1
        val lit = sky.moon.lit
        val awayHours = lastOpen?.let { (now - it) / 3600.0 }
        // the day starts at 4 am (Words.swift usual): an open before today's 4 am is not today's
        val fourAm = local.toLocalDate().atStartOfDay(zone).plusHours(4).let { if (local.hour < 4) it.minusDays(1) else it }.toEpochSecond().toDouble()
        val season = sky.season.name.lowercase()
        signals = buildMap {
            put("phase", phase)
            put("hour", local.hour + local.minute / 60.0)
            put("weekend", weekday == 1 || weekday == 7)
            put("weekday", weekday)
            put("waiting", waiting)
            put("done", done)
            put("failed", failed)
            put("running", running)
            put("link", link)
            put("firstRun", firstRun)
            put("justPaired", justPaired)
            put("season", season)
            put("moon", if (lit > 0.97) "full" else if (lit < 0.03) "new" else "")
            sunsetIn?.let { put("sunsetIn", it) }
            sunriseIn?.let { put("sunriseIn", it) }
            awayHours?.let { put("awayHours", it) }
            lastOpen?.let { put("firstToday", it < fourAm) }
            lastZone?.let { put("newPlace", it != zone.id) }
        }
        fills = buildMap {
            put("sunset", day.sunset?.let(::at).orEmpty())
            put("sunrise", day.sunrise?.let(::at).orEmpty())
            put("sunsetIn", sunsetIn?.toString().orEmpty())
            put("sunriseIn", sunriseIn?.toString().orEmpty())
            put("done", done.toString()); put("waiting", waiting.toString()); put("failed", failed.toString()); put("running", running.toString())
            put("day", local.dayOfWeek.getDisplayName(java.time.format.TextStyle.FULL, Locale.US))
            lastOpen?.let { put("lastOpen", at(it)); put("awayDays", ((now - it) / 86400).toInt().toString()) }
            put("time", local.format(clock))
            put("season", season)
        }
        // Calendar.ordinality(of: .day, in: .era): 0001-01-01 is day 1
        dayNumber = (local.toLocalDate().toEpochDay() + 719163).toInt()
    }

    companion object {
        /** MobileScreens.swift sunLine's time left: "45 min", "5 h 0 min". */
        fun minutes(left: Int): String = if (left >= 60) "${left / 60} h ${left % 60} min" else "$left min"
    }
}
