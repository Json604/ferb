package com.ferb.app

import android.Manifest
import android.app.ActivityManager
import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.Settings
import androidx.annotation.StringRes

/**
 * What keeps FERB's questions from reaching this phone that only the owner can change (kb PHN-765): the app never
 * posts nothing silently. Said in the profile sheet, with the one tap that fixes it, and once after he sends something
 * from the phone. In order: the first one found is the one said.
 */
enum class NotifyBlocker(@StringRes val words: Int, @StringRes val fix: Int) {
    /** Android's notifications for FERB are off (the first run's "Not now", or turned off in Android's settings). */
    NOTIFICATIONS_OFF(R.string.cant_notify_off, R.string.cant_notify_turn_on),

    /** FERB's Questions or Checks channel is turned off in Android's settings. */
    QUESTIONS_OFF(R.string.cant_notify_channel, R.string.cant_notify_turn_on),

    /** Stay connected is off: with the app closed the link is closed (PHN-2, PHN-421), so nothing can arrive. */
    STAY_CONNECTED_OFF(R.string.cant_notify_stay, R.string.cant_notify_turn_on),

    /** Android restricts FERB in the background (the battery setting "Restricted"): Stay connected can't keep the link. */
    BACKGROUND_RESTRICTED(R.string.cant_notify_battery, R.string.cant_notify_open_settings),
}

object NotifyHealth {
    fun blocker(context: Context, stayConnected: Boolean): NotifyBlocker? {
        val nm = context.getSystemService(NotificationManager::class.java)
        val granted = android.os.Build.VERSION.SDK_INT < 33 ||
            context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
        return when {
            !granted || !nm.areNotificationsEnabled() -> NotifyBlocker.NOTIFICATIONS_OFF
            listOf(Notifier.CH_QUESTIONS, Notifier.CH_CHECKS).any { nm.getNotificationChannel(it)?.importance == NotificationManager.IMPORTANCE_NONE } ->
                NotifyBlocker.QUESTIONS_OFF
            !stayConnected -> NotifyBlocker.STAY_CONNECTED_OFF
            context.getSystemService(ActivityManager::class.java).isBackgroundRestricted -> NotifyBlocker.BACKGROUND_RESTRICTED
            else -> null
        }
    }

    /** Android's own page that fixes [b] (Stay connected is the app's own switch: none). */
    fun settings(context: Context, b: NotifyBlocker): Intent? {
        val pkg = context.packageName
        return when (b) {
            NotifyBlocker.NOTIFICATIONS_OFF -> Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, pkg)
            NotifyBlocker.QUESTIONS_OFF -> {
                val nm = context.getSystemService(NotificationManager::class.java)
                val off = listOf(Notifier.CH_QUESTIONS, Notifier.CH_CHECKS).firstOrNull { nm.getNotificationChannel(it)?.importance == NotificationManager.IMPORTANCE_NONE }
                Intent(Settings.ACTION_CHANNEL_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, pkg)
                    .putExtra(Settings.EXTRA_CHANNEL_ID, off ?: Notifier.CH_QUESTIONS)
            }
            NotifyBlocker.BACKGROUND_RESTRICTED -> Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", pkg, null))
            NotifyBlocker.STAY_CONNECTED_OFF -> null
        }
    }
}
