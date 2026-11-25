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

    /**
     * kb PHN-835, PHN-846: "Fingerprint to open", on unless the owner turns it off. Off is the Mac's too: set only after
     * the Mac took the phone's signed `phone.lock_off`; the Mac's own record wins when it says on (paired again).
     */
    var lockOnOpen: Boolean
        get() = p.getBoolean("app_lock", true)
        set(v) = p.edit().putBoolean("app_lock", v).apply()

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
     * until he does, or allows the location at the first run (kb PHN-638); the city itself is kept in memory only.
     */
    var shareCity: Boolean
        get() = p.getBoolean("share_city", false)
        set(v) = p.edit().putBoolean("share_city", v).apply()

    /** When this phone was first seen paired (epoch ms), for "Paired with … since". */
    var pairedSince: Long
        get() = p.getLong("paired_since", 0)
        set(v) = p.edit().putLong("paired_since", v).apply()

    /**
     * This install's id for FERB's sign-in server (kb PHN-827): random, made once, not secret (the spec keeps it in
     * plain preferences); the server lists the account's devices by it.
     */
    val deviceId: String
        @Synchronized get() = p.getString("device_id", null) ?: com.ferb.app.model.AccountModel.newDeviceId().also {
            p.edit().putString("device_id", it).apply()
        }

    /** The FERB account id the paired Mac was paired under (kb PHN-828); null before, or for an older pairing. */
    var pairedAccount: String?
        get() = p.getString("paired_account", null)
        set(v) = p.edit().putString("paired_account", v).apply()

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

    /** The first run's asks already answered or passed over (FirstRun.Ask names, kb PHN-636): never asked again. */
    var firstRunAsked: Set<String>
        get() = p.getStringSet("first_run_asked", emptySet()) ?: emptySet()
        set(v) = p.edit().putStringSet("first_run_asked", v).apply()

    /** The owner was asked once, in context, before Android's notification question. */
    var notificationsExplained: Boolean
        get() = p.getBoolean("notify_explained", false)
        set(v) = p.edit().putBoolean("notify_explained", v).apply()

    /**
     * Speaking to FERB (kb PHN-604): the kinds of speech recognition (SpeechKind names) the owner was told about before
     * his first use of each: on the phone itself, or the phone's own speech service.
     */
    var voiceTold: Set<String>
        get() = p.getStringSet("voice_told", emptySet()) ?: emptySet()
        set(v) = p.edit().putStringSet("voice_told", v).apply()

    /** Answers, results and questions already announced (entry or question ids only: never their words). */
    var announced: Set<String>
        get() = p.getStringSet("announced", emptySet()) ?: emptySet()
        // written only when it changed: the notifier sets it after every look, several times a second while a task
        // runs, and each write is the whole file again and one more write the main thread waits out when the app stops
        set(v) { if (v != announced) p.edit().putStringSet("announced", v).apply() }

    /** The kinds of NotifyBlocker already said after a send (kb PHN-765): each is said that way once. */
    var toldBlockers: Set<String>
        get() = p.getStringSet("told_blockers", emptySet()) ?: emptySet()
        set(v) = p.edit().putStringSet("told_blockers", v).apply()

    /** Finished tasks the owner has had the app open since (entry ids only): the home screen's "while you were away". */
    var seen: Set<String>
        get() = p.getStringSet("seen", emptySet()) ?: emptySet()
        set(v) = p.edit().putStringSet("seen", v).apply()

    /**
     * Insomnia mode's ends already said (their ids only, APP-980): each is said once, in the app or as a quiet
     * notification, whichever sees it first. True for the first claim of [id].
     */
    @Synchronized
    fun claimInsomniaEnd(id: String): Boolean {
        val said = p.getStringSet("insomnia_said", emptySet()) ?: emptySet()
        if (id in said) return false
        // an end is kept by the Mac 10 minutes: a short list is enough
        p.edit().putStringSet("insomnia_said", if (said.size >= 32) setOf(id) else said + id).apply()
        return true
    }

    /** Forget this Mac: what belonged to that pairing goes; the phone's own switches stay, but its lock comes back on. */
    fun forgetPairing() {
        // the lock switch's "off" lives with the pairing on the Mac (PHN-846): a new pairing starts with it on
        p.edit().remove("app_lock").remove("paired_since").remove("paired_account").remove("just_paired").remove("announced").remove("seen").remove("insomnia_said").apply()
    }
}
