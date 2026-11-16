package com.ferb.app.model

import java.io.File
import java.io.InputStream
import java.util.UUID

/** A file the owner picked in the system's picker: its name, size if known, and its bytes. */
class Picked(val name: String, val size: Long?, val open: () -> InputStream?)

/** A picked file copied into the app's own storage, ready to send. */
data class Staged(val path: String, val name: String, val size: Long)

/** A folder the owner picked in the system's folder picker: its name and every file under it (its path inside). */
class PickedFolder(val name: String, val files: List<PickedFile>)

/** One file of a [PickedFolder]: [rel] is its path inside the folder (`Scans/2026/a.pdf`). */
class PickedFile(val rel: String, val size: Long?, val open: () -> InputStream?)

/**
 * Where files live on the phone (PHN-11, PHN-15, PHN-16; kb ruling in the S11/T-1221 report): `cacheDir/files`, which
 * no backup or device transfer ever carries (the platform's rule for the cache, besides :core's manifest), private to
 * the app, and which the system may empty under storage pressure (a fetch then starts over: its offset is the file's
 * length). `fetched/<transfer>/<name>`: files from the Mac, kept at most [MAX_AGE_MS] and removed when the app locks;
 * `outgoing/<id>/<name>`: copies of what the owner picked, until sent. Only `fetched` is shared with other apps, and
 * only through FERB's FileProvider on the owner's tap.
 */
class LocalFiles(
    val root: File,
    private val pickFolder: (String) -> PickedFolder? = { null },
    private val pick: (String) -> Picked?,
) {
    val fetched: File get() = File(root, "fetched")
    val outgoing: File get() = File(root, "outgoing")

    /** Where a fetch of [name] for transfer [key] is written (a name from the Mac never climbs out). */
    fun fetchTarget(key: String, name: String): File = File(File(fetched, key), safe(name)).also { it.parentFile?.mkdirs() }

    /** Copies a picked file (a content Uri as text) into `outgoing`; null when it can't be read. Blocking. */
    fun stage(uri: String): Staged? {
        val p = runCatching { pick(uri) }.getOrNull() ?: return null
        val dir = File(outgoing, UUID.randomUUID().toString()).also { it.mkdirs() }
        val out = File(dir, safe(p.name))
        return try {
            val input = p.open() ?: return null.also { dir.deleteRecursively() }
            input.use { i -> out.outputStream().use { o -> i.copyTo(o) } }
            Staged(out.path, p.name, out.length())
        } catch (e: Exception) {
            dir.deleteRecursively()
            null
        }
    }

    /** Where a file of a whole folder fetched as [batch] is written: `fetched/<batch>/<folder>/<rel>`, each part made safe. */
    fun folderTarget(batch: String, folder: String, rel: String): File =
        File(File(File(fetched, batch), safe(folder)), safeRel(rel)).also { it.parentFile?.mkdirs() }

    /** Where a file of a folder being sent waits, copied just before it goes: `outgoing/<batch>/<rel>`. */
    fun sendTarget(batch: String, rel: String): File = File(File(outgoing, batch), safeRel(rel))

    /** Reads a picked folder (a tree Uri as text): its name and files. Null when it can't be read. Blocking. */
    fun folder(uri: String): PickedFolder? = runCatching { pickFolder(uri) }.getOrNull()

    /** Copies one file of a picked folder to [target] (just before it is sent); false when it can't be read. Blocking. */
    fun stageTo(target: File, open: () -> InputStream?): Boolean = try {
        target.parentFile?.mkdirs()
        val input = open() ?: return false
        input.use { i -> target.outputStream().use { o -> i.copyTo(o) } }
        true
    } catch (e: Exception) {
        target.delete()
        false
    }

    /** Removes everything of one batch (a folder fetched or sent whole). */
    fun removeBatch(batch: String) {
        File(fetched, batch).deleteRecursively()
        File(outgoing, batch).deleteRecursively()
    }

    /** Removes one transfer's file and its folder. */
    fun remove(path: String) {
        val f = File(path)
        f.delete()
        f.parentFile?.takeIf { it.parentFile == fetched || it.parentFile == outgoing }?.deleteRecursively()
    }

    /** PHN-15: the app locked: the fetched files go (copies the owner saved or shared elsewhere are his). */
    fun wipeFetched() {
        fetched.deleteRecursively()
    }

    /** Forget this Mac: everything. */
    fun wipeAll() {
        root.deleteRecursively()
    }

    /** PHN-16: nothing fetched is kept longer than a day; a copy waiting to be sent goes after a day too. */
    fun dropOld(nowMs: Long) {
        for (dir in listOf(fetched, outgoing)) {
            dir.listFiles()?.forEach { f -> if (nowMs - newest(f) > MAX_AGE_MS) f.deleteRecursively() }
        }
    }

    private fun newest(f: File): Long = if (f.isDirectory) f.listFiles()?.maxOfOrNull(::newest) ?: f.lastModified() else f.lastModified()

    companion object {
        /** PHN-13, PHN-16: FETCHED_MAX_AGE_MS. */
        const val MAX_AGE_MS = 24 * 60 * 60 * 1000L

        /** A file name as given, without folders, and never `.` or `..`. */
        fun safe(name: String): String {
            val n = name.substringAfterLast('/').substringAfterLast('\\').replace("\u0000", "").trim()
            return if (n.isEmpty() || n == "." || n == "..") "file" else n.take(200)
        }

        /** A path inside a folder: each part made [safe], so nothing climbs out. */
        fun safeRel(rel: String): String = rel.split('/', '\\').filter { it.isNotEmpty() }.joinToString("/") { safe(it) }.ifEmpty { "file" }
    }
}
