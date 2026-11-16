package com.ferb.app

import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.Picked
import java.io.File

/** FERB's FileProvider: only `cacheDir/files/fetched` is reachable through it (res/xml/ferb_files.xml). */
class FerbFiles : FileProvider()

/** The phone's own [LocalFiles]: `cacheDir/files`, picked files read through the system's ContentResolver. */
fun LocalFiles.Companion.forPhone(context: Context): LocalFiles {
    val app = context.applicationContext
    return LocalFiles(File(app.cacheDir, "files")) { text ->
        val uri = Uri.parse(text)
        var name: String? = null
        var size: Long? = null
        runCatching {
            app.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
                if (c.moveToFirst()) {
                    name = c.getString(0)
                    size = if (c.isNull(1)) null else c.getLong(1)
                }
            }
        }
        Picked(name ?: uri.lastPathSegment?.substringAfterLast('/') ?: "file", size) { app.contentResolver.openInputStream(uri) }
    }
}

/**
 * The owner's tap on a fetched file (PHN-113, PHN-115): the system's "Open with" or share sheet for it, with read
 * access for the app he picks and nothing else. Never started by FERB itself.
 */
object Handout {
    fun authority(context: Context) = "${context.packageName}.files"

    fun uri(context: Context, file: File): Uri = FileProvider.getUriForFile(context, authority(context), file)

    fun type(name: String): String =
        MimeTypeMap.getSingleton().getMimeTypeFromExtension(name.substringAfterLast('.', "").lowercase()) ?: "application/octet-stream"

    fun open(context: Context, file: File, name: String): Intent {
        val u = uri(context, file)
        val view = Intent(Intent.ACTION_VIEW).setDataAndType(u, type(name)).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        return Intent.createChooser(view, context.getString(R.string.open_with)).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }

    fun share(context: Context, file: File, name: String): Intent {
        val u = uri(context, file)
        val send = Intent(Intent.ACTION_SEND).setType(type(name)).putExtra(Intent.EXTRA_STREAM, u)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        send.clipData = ClipData.newRawUri(name, u)
        return Intent.createChooser(send, context.getString(R.string.share_with)).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }
}
