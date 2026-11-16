package com.ferb.app

import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.Picked
import com.ferb.app.model.PickedFile
import com.ferb.app.model.PickedFolder
import java.io.File

/** FERB's FileProvider: only `cacheDir/files/fetched` is reachable through it (res/xml/ferb_files.xml). */
class FerbFiles : FileProvider()

/**
 * The phone's own [LocalFiles]: `cacheDir/files`, picked files read through the system's ContentResolver, and a picked
 * folder (ACTION_OPEN_DOCUMENT_TREE: no storage permission, only the folder the owner picks) walked through the
 * documents provider that holds it.
 */
fun LocalFiles.Companion.forPhone(context: Context): LocalFiles {
    val app = context.applicationContext
    return LocalFiles(File(app.cacheDir, "files"), pickFolder = { text -> pickedFolder(app, Uri.parse(text)) }, pick = { text ->
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
    })
}

/** Every file under a picked folder, each with its path inside it; read lazily (each opened only when it is sent). */
private fun pickedFolder(app: Context, tree: Uri): PickedFolder? {
    val resolver = app.contentResolver
    val rootId = DocumentsContract.getTreeDocumentId(tree)
    val rootDoc = DocumentsContract.buildDocumentUriUsingTree(tree, rootId)
    val name = resolver.query(rootDoc, arrayOf(DocumentsContract.Document.COLUMN_DISPLAY_NAME), null, null, null)?.use { c ->
        if (c.moveToFirst()) c.getString(0) else null
    } ?: return null
    val files = ArrayList<PickedFile>()
    val cols = arrayOf(DocumentsContract.Document.COLUMN_DOCUMENT_ID, DocumentsContract.Document.COLUMN_DISPLAY_NAME,
        DocumentsContract.Document.COLUMN_MIME_TYPE, DocumentsContract.Document.COLUMN_SIZE)
    fun walk(docId: String, prefix: String) {
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, docId)
        resolver.query(children, cols, null, null, null)?.use { c ->
            while (c.moveToNext()) {
                val id = c.getString(0) ?: continue
                val n = c.getString(1)?.takeIf { it.isNotBlank() } ?: continue
                if (c.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR) {
                    walk(id, "$prefix$n/")
                } else {
                    val size = if (c.isNull(3)) null else c.getLong(3)
                    val doc = DocumentsContract.buildDocumentUriUsingTree(tree, id)
                    files.add(PickedFile("$prefix$n", size) { resolver.openInputStream(doc) })
                }
            }
        }
    }
    walk(rootId, "")
    return PickedFolder(name, files)
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

    /** A fetched folder's files (a folder can't be opened in another app): one share sheet for all, each read-only. */
    fun shareMany(context: Context, files: List<File>): Intent {
        val uris = ArrayList(files.map { uri(context, it) })
        val types = files.map { type(it.name) }.distinct()
        val type = types.singleOrNull() ?: types.map { it.substringBefore('/') }.distinct().singleOrNull()?.let { "$it/*" } ?: "*/*"
        val send = Intent(Intent.ACTION_SEND_MULTIPLE).setType(type).putParcelableArrayListExtra(Intent.EXTRA_STREAM, uris)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        send.clipData = ClipData.newRawUri(files.firstOrNull()?.name ?: "", uris.first()).also { c -> uris.drop(1).forEach { c.addItem(ClipData.Item(it)) } }
        return Intent.createChooser(send, context.getString(R.string.share_with)).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }
}
