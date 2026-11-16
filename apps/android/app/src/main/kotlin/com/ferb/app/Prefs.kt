package com.ferb.app

import android.content.Context

/**
 * The app's own settings on this phone (never FERB's data: no chat, no question words). App-private, in no backup or
 * transfer (:core's manifest turns both off for every domain).
 */
class Prefs(context: Context) {
    private val p = context.getSharedPreferences("ferb.app", Context.MODE_PRIVATE)

    /** PHN-421: Stay connected (the foreground service). Off until the owner turns it on. */
    var stayConnected: Boolean
        get() = p.getBoolean("stay", false)
        set(v) = p.edit().putBoolean("stay", v).apply()

    /** PHN-420: whole-app screenshot blocking, on unless the owner turns it off. */
    var blockScreenshots: Boolean
        get() = p.getBoolean("secure", true)
        set(v) = p.edit().putBoolean("secure", v).apply()

    /** PHN-422: the lock screen never shows the words of FERB's own checks (on by default, a setting). */
    var hideChecksOnLockScreen: Boolean
        get() = p.getBoolean("hide_checks", true)
        set(v) = p.edit().putBoolean("hide_checks", v).apply()

    /**
     * SENSE-420: the owner turned on "Share this phone's city with FERB" (and Android allowed coarse location). Off
     * until he does; the city itself is kept in memory only.
     */
    var shareCity: Boolean
        get() = p.getBoolean("share_city", false)
        set(v) = p.edit().putBoolean("share_city", v).apply()

    /** When this phone was first seen paired (epoch ms), for "Paired with … since". */
    var pairedSince: Long
        get() = p.getLong("paired_since", 0)
        set(v) = p.edit().putLong("paired_since", v).apply()

    /** The first open after pairing has not happened yet (the corpus's justPaired). */
    var justPaired: Boolean
        get() = p.getBoolean("just_paired", false)
        set(v) = p.edit().putBoolean("just_paired", v).apply()

    /** The previous open (epoch seconds) and its time zone: the corpus's awayHours, lastOpen, firstToday, newPlace. */
    var lastOpen: Long
        get() = p.getLong("last_open", 0)
        set(v) = p.edit().putLong("last_open", v).apply()
    var lastZone: String?
        get() = p.getString("last_zone", null)
        set(v) = p.edit().putString("last_zone", v).apply()

    /** The owner was asked once, in context, before Android's notification question. */
    var notificationsExplained: Boolean
        get() = p.getBoolean("notify_explained", false)
        set(v) = p.edit().putBoolean("notify_explained", v).apply()

    /** Answers, results and questions already announced (entry or question ids only: never their words). */
    var announced: Set<String>
        get() = p.getStringSet("announced", emptySet()) ?: emptySet()
        set(v) = p.edit().putStringSet("announced", v).apply()

    /** Finished tasks the owner has had the app open since (entry ids only): the home screen's "while you were away". */
    var seen: Set<String>
        get() = p.getStringSet("seen", emptySet()) ?: emptySet()
        set(v) = p.edit().putStringSet("seen", v).apply()

    /** Forget this Mac: what belonged to that pairing goes; the phone's own switches stay. */
    fun forgetPairing() {
        p.edit().remove("paired_since").remove("just_paired").remove("announced").remove("seen").apply()
    }
}
