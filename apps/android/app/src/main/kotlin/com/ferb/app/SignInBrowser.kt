package com.ferb.app

import android.app.Activity
import android.content.Intent
import android.net.Uri
import androidx.browser.customtabs.CustomTabsClient
import androidx.browser.customtabs.CustomTabsIntent

/**
 * Where FERB's sign-in opens (kb PHN-827): a Custom Tab of the browser that offers them (the person's own browser,
 * with its saved Google or Apple sign-in; the app never sees the page), else the default browser. The answer comes
 * back as the verified App Link `https://ferb.kartikey.xyz/app/callback` (MainActivity's filter).
 */
object SignInBrowser {
    /** @throws android.content.ActivityNotFoundException when this phone has no browser at all. */
    fun open(activity: Activity, url: String) {
        val uri = Uri.parse(url)
        val tabs = CustomTabsClient.getPackageName(activity, null)
        if (tabs != null) {
            val tab = CustomTabsIntent.Builder()
                .setShowTitle(true)
                .setShareState(CustomTabsIntent.SHARE_STATE_OFF)
                .build()
            tab.intent.setPackage(tabs)
            tab.launchUrl(activity, uri)
        } else {
            activity.startActivity(Intent(Intent.ACTION_VIEW, uri).addCategory(Intent.CATEGORY_BROWSABLE))
        }
    }
}
