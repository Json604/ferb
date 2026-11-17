package com.ferb.app.model

import com.ferb.mobile.Uploaded

/**
 * Where a file or folder sent from the phone is on the Mac, as the owner reads it (kb PHN-608): the Mac's own words for
 * the folder it landed in (`Uploaded.savedIn`, "Downloads › FERB from phone"), else the Mac's upload folder as its
 * settings say (`MacInfo.uploadFolder.shown`), else FERB's default place (the string tr_place_default). The full path
 * (`saved`, which names the Mac's user folder) is never shown.
 */
object UploadPlace {
    fun shown(uploaded: Uploaded): String? = uploaded.savedIn?.takeIf { it.isNotBlank() }
}
