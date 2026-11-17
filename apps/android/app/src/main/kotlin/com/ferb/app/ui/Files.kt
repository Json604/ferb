package com.ferb.app.ui

import android.text.format.Formatter
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import com.ferb.app.FerbViewModel
import com.ferb.app.R
import com.ferb.app.UiState
import com.ferb.app.model.Batch
import com.ferb.app.model.Phase
import com.ferb.app.model.Transfer
import com.ferb.app.model.Way
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.components.Card
import com.ferb.design.components.CardRow
import com.ferb.design.components.Divide
import com.ferb.design.components.FileIcon
import com.ferb.design.components.FileKind
import com.ferb.design.components.LocalSkyInk
import com.ferb.design.components.Opens
import com.ferb.design.components.PhoneTab
import com.ferb.design.components.PhoneText
import com.ferb.design.components.Pill
import com.ferb.design.components.PillKind
import com.ferb.design.components.RowIcon
import com.ferb.design.components.SkyLabel
import com.ferb.design.components.TopBar
import com.ferb.design.components.pressable
import com.ferb.design.screens.PhoneInsets
import com.ferb.mobile.FileEntry
import com.ferb.mobile.FoundFile
import com.ferb.mobile.LinkPhase
import java.io.File
import java.text.DateFormat
import java.time.OffsetDateTime
import java.util.Date

/**
 * MobileScreens.swift PhoneFilesPage: search, the transfers under way (the lab's progress card), the places, and
 * sending files to the Mac; a place opens as a folder of the lab's rows. A key place's entry shows its lock and asks
 * for the fingerprint for that very path (PHN-121). Search looks through names on the Mac (inside the folder open);
 * Recent is the Mac's list of what FERB made and what this phone fetched or sent (T-1211). A folder goes whole either
 * way (PHN-13).
 */
@Composable
fun Files(vm: FerbViewModel, ui: UiState, insets: PhoneInsets, waiting: PhoneTab?, onTab: (PhoneTab) -> Unit, onProfile: () -> Unit, host: Host) {
    val transfers by vm.transfers.list.collectAsState()
    val batches by vm.transfers.batches.collectAsState()
    val folder = ui.files.folder
    val search = ui.files.search
    BackHandler(enabled = folder != null || search != null) { if (search != null) vm.closeSearch() else vm.folderUp() }
    val ink = LocalSkyInk.current
    Column(Modifier.fillMaxSize()) {
        TopBar(PhoneTab.FILES, waiting = waiting, onTab = onTab, onProfile = onProfile)
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp)) {
            SearchField(vm, ui)
            for (group in groups(transfers)) TransferCard(vm, group, host, batch = group.first().batch?.let { batches[it] })
            if (ui.link?.phase == LinkPhase.ONLINE && ui.link.bulkAllowed == false) {
                PhoneText(stringResource(R.string.files_needs_better_path), 14f, FerbType.Medium, ink.low, Modifier.fillMaxWidth().padding(start = 8.dp, end = 8.dp, top = 12.dp))
            }
            ui.files.problem?.let { PhoneText(noteText(it), 14f, FerbType.Medium, ink.low, Modifier.fillMaxWidth().padding(start = 8.dp, end = 8.dp, top = 12.dp)) }
            when {
                search != null -> Found(vm, ui)
                folder == null -> Places(vm, ui)
                else -> FolderView(vm, ui)
            }
        }
        Row(Modifier.padding(start = 16.dp, end = 16.dp, top = 8.dp, bottom = insets.bottom + 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Pill("Send files to your Mac", Modifier.weight(1f), PillKind.BLACK, FerbIcons.sendFile, onClick = { host.pickFiles(false) })
            Pill(stringResource(R.string.files_send_folder), kind = PillKind.GREY, icon = FerbIcons.folder, fill = false, onClick = { host.pickFolder(false) })
        }
    }
}

/** The lab's black search pill, now a field: the Mac's names, inside the folder open (or the whole home folder). */
@Composable
private fun SearchField(vm: FerbViewModel, ui: UiState) {
    var text by remember { mutableStateOf("") }
    val searching = ui.files.search != null
    val folder = ui.files.folder
    val hint = folder?.let { stringResource(R.string.search_in, nameOf(it.path)) } ?: "Search your Mac"
    Row(
        Modifier.padding(top = 14.dp).fillMaxWidth().heightIn(min = 50.dp).background(Ink.black, CircleShape).padding(horizontal = 16.dp),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        FerbIcon(FerbIcons.search, 17f, Ink.onDark)
        Box(Modifier.weight(1f), contentAlignment = Alignment.CenterStart) {
            BasicTextField(
                text, { text = it }, Modifier.fillMaxWidth().semantics { contentDescription = hint },
                textStyle = FerbType.style(16.5f, color = Ink.onDark), cursorBrush = SolidColor(Ink.onDark), singleLine = true,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                keyboardActions = KeyboardActions(onSearch = { vm.search(text) }),
            )
            if (text.isEmpty()) PhoneText(hint, 16.5f, color = Ink.onDark, maxLines = 1)
        }
        if (searching || text.isNotEmpty()) {
            Box(Modifier.pressable({ text = ""; vm.closeSearch() }, label = stringResource(R.string.search_close))) {
                FerbIcon(FerbIcons.close, 13f, Ink.onDark)
            }
        }
    }
}

/** "Load more" (PHN-609): the next page of 500 from the Mac, added in place under what is shown. */
@Composable
private fun LoadMore(vm: FerbViewModel, ui: UiState, onClick: () -> Unit) {
    val loading = "more" in ui.busy
    Card(Modifier.padding(top = 12.dp)) {
        CardRow(stringResource(if (loading) R.string.files_loading_more else R.string.files_load_more), null,
            onClick = if (loading) null else onClick, leading = { RowIcon(FerbIcons.download) })
    }
}

/** A search's answer: what matched (folders open here, files are fetched), and what the Mac says it left out. */
@Composable
private fun Found(vm: FerbViewModel, ui: UiState) {
    val s = ui.files.search ?: return
    val ink = LocalSkyInk.current
    val context = LocalContext.current
    SkyLabel(s.path?.let { stringResource(R.string.search_found_in, nameOf(it)) } ?: stringResource(R.string.search_found_mac))
    val line: @Composable (String) -> Unit = { PhoneText(it, 14f, FerbType.Medium, ink.low, Modifier.fillMaxWidth().padding(start = 8.dp, end = 8.dp, top = 10.dp)) }
    when {
        s.loading -> line(stringResource(R.string.search_looking))
        s.problem != null -> line(noteText(s.problem))
        else -> {
            val f = s.found ?: return
            if (f.files.isEmpty()) line(stringResource(R.string.search_none, s.query))
            else Card {
                f.files.forEachIndexed { i, e ->
                    if (i > 0) Divide()
                    CardRow(e.name, foundDetail(context, e), onClick = { vm.openFound(e) },
                        leading = { if (e.dir) RowIcon(FerbIcons.folder) else FileIcon(e.name, 26.dp) }, trailing = { Opens() })
                }
            }
            // more pages (PHN-609): "Load more" while the Mac says there are; the plain line only when it doesn't page
            if (f.next != null) LoadMore(vm, ui) { vm.searchMore() }
            else if (f.cut) line(stringResource(R.string.search_cut, f.files.size))
            if (f.incomplete) line(stringResource(R.string.search_incomplete, f.looked.toInt()))
            if (f.skipped > 0u) line(pluralStringResource(R.plurals.search_skipped, f.skipped.toInt(), f.skipped.toInt()))
        }
    }
}

@Composable
private fun Places(vm: FerbViewModel, ui: UiState) {
    val ink = LocalSkyInk.current
    val context = LocalContext.current
    val places = listOf(
        Triple(FerbViewModel.PLACES[0], R.string.place_desktop, FerbIcons.desktop),
        Triple(FerbViewModel.PLACES[1], R.string.place_documents, FerbIcons.doc),
        Triple(FerbViewModel.PLACES[2], R.string.place_downloads, FerbIcons.download),
    )
    SkyLabel(stringResource(R.string.files_places))
    Card {
        places.forEachIndexed { i, (path, title, icon) ->
            if (i > 0) Divide()
            CardRow(stringResource(title), if (ui.files.loading == path) stringResource(R.string.files_opening) else null,
                onClick = { vm.openFolder(path) }, leading = { RowIcon(icon) },
                trailing = { Opens(ui.files.counts[path]?.toString()) })
        }
    }
    SkyLabel(stringResource(R.string.files_recent))
    val line: @Composable (String) -> Unit = { PhoneText(it, 14.5f, FerbType.Medium, ink.low, Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 2.dp)) }
    val recent = ui.files.recent
    when {
        recent == null -> ui.files.recentNote?.let { line(noteText(it)) }
        recent.files.isEmpty() -> line(stringResource(R.string.recent_empty))
        else -> Card {
            recent.files.forEachIndexed { i, e ->
                if (i > 0) Divide()
                CardRow(e.name, recentDetail(context, e), onClick = { vm.openFound(e) },
                    leading = { if (e.dir) RowIcon(FerbIcons.folder) else FileIcon(e.name, 26.dp) }, trailing = { Opens() })
            }
        }
    }
    if (recent != null && recent.skipped > 0u) {
        PhoneText(pluralStringResource(R.plurals.recent_skipped, recent.skipped.toInt(), recent.skipped.toInt()), 14f, FerbType.Medium, ink.low, Modifier.fillMaxWidth().padding(start = 8.dp, end = 8.dp, top = 10.dp))
    }
}

@Composable
private fun FolderView(vm: FerbViewModel, ui: UiState) {
    val f = ui.files.folder ?: return
    val ink = LocalSkyInk.current
    val context = LocalContext.current
    Row(Modifier.fillMaxWidth().padding(top = 14.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.pressable({ vm.folderUp() }, label = stringResource(R.string.files_back)).padding(end = 6.dp)) {
            FerbIcon(Icons.AutoMirrored.Rounded.ArrowBack, 18f, ink.top)
        }
        PhoneText(nameOf(f.path), 15f, FerbType.Semibold, ink.low, Modifier.weight(1f).padding(vertical = 8.dp), maxLines = 1)
        if (f.locked) FerbIcon(FerbIcons.lockFill, 14f, ink.low)
    }
    if (f.entries.isEmpty()) {
        PhoneText(stringResource(R.string.files_empty), 15f, FerbType.Medium, ink.low, Modifier.padding(start = 8.dp, top = 8.dp))
        return
    }
    Card(Modifier.padding(top = 4.dp)) {
        f.entries.forEachIndexed { i, e ->
            if (i > 0) Divide()
            CardRow(
                e.name, detail(context, e), onClick = { vm.openEntry(e) },
                leading = { if (e.dir) RowIcon(FerbIcons.folder) else FileIcon(e.name, 26.dp) },
                trailing = { if (e.locked) FerbIcon(FerbIcons.lockFill, 15f, Ink.faint) else Opens() },
            )
        }
    }
    if (f.cut) PhoneText(stringResource(R.string.files_cut, f.entries.size, f.total.toInt()), 14f, FerbType.Medium, ink.low, Modifier.padding(start = 8.dp, top = 10.dp))
    if (f.next != null) LoadMore(vm, ui) { vm.folderMore() }
    // a key place's folder isn't fetched whole: each of its files asks for the fingerprint by itself (PHN-121)
    if (!f.locked && f.entries.any { !it.locked }) {
        Card(Modifier.padding(top = 12.dp)) {
            CardRow(stringResource(R.string.files_get_folder), if ("tree" in ui.busy) stringResource(R.string.files_opening) else null,
                onClick = if ("tree" in ui.busy) null else ({ vm.fetchFolder() }), leading = { RowIcon(FerbIcons.download) })
        }
    }
}

@Composable
private fun detail(context: android.content.Context, e: FileEntry): String? {
    if (e.locked) return stringResource(R.string.files_key_place)
    if (e.dir) return null
    val size = Formatter.formatShortFileSize(context, e.size.toLong())
    val day = if (e.modified > 0) DateFormat.getDateInstance(DateFormat.MEDIUM).format(Date(e.modified * 1000)) else return size
    return stringResource(R.string.file_detail, size, day)
}

/** A found file: its folder on the Mac (where it is) and its size. */
@Composable
private fun foundDetail(context: android.content.Context, e: FoundFile): String? {
    val where = e.path.trimEnd('/').substringBeforeLast('/', "").substringAfterLast('/').ifEmpty { null }
    if (e.dir) return where
    val size = Formatter.formatShortFileSize(context, e.size.toLong())
    return where?.let { stringResource(R.string.file_detail, it, size) } ?: size
}

/** A recent file: made by FERB (for which task), fetched or sent by this phone, and the day. */
@Composable
private fun recentDetail(context: android.content.Context, e: FoundFile): String? {
    val ms = e.at?.let { runCatching { OffsetDateTime.parse(it).toInstant().toEpochMilli() }.getOrNull() }
        ?: e.modified.takeIf { it > 0 }?.times(1000)
    val day = ms?.let { DateFormat.getDateInstance(DateFormat.MEDIUM).format(Date(it)) } ?: return null
    return when (e.source) {
        "made" -> e.taskName?.takeIf { it.isNotBlank() }?.let { stringResource(R.string.recent_made_for, it, day) } ?: stringResource(R.string.recent_made, day)
        "fetched" -> stringResource(R.string.recent_fetched, day)
        "sent" -> stringResource(R.string.recent_sent, day)
        else -> day
    }
}

/** A Mac path's last part (`~` is the Mac's home folder). */
@Composable
internal fun nameOf(path: String): String =
    if (path == "~" || path.isEmpty()) stringResource(R.string.search_mac) else path.trimEnd('/').substringAfterLast('/').ifEmpty { path }

/** A fetch is its own card; the files sent together share one (the lab's "Sending 2 photos to your Mac"). */
private fun groups(all: List<Transfer>): List<List<Transfer>> {
    val out = ArrayList<List<Transfer>>()
    val seen = HashSet<String>()
    for (t in all) {
        val b = t.batch
        if (b == null) out.add(listOf(t)) else if (seen.add(b)) out.add(all.filter { it.batch == b })
    }
    return out
}

/**
 * The lab's progress card, with Pause or Go on and Cancel; a fetched file gets Open and Share, on the owner's tap only.
 * A whole folder says how many of its files and bytes are done, what the Mac left out and where its listing stopped;
 * a folder fetched can't be opened in another app, so its files are shared together.
 */
@Composable
fun TransferCard(vm: FerbViewModel, group: List<Transfer>, host: Host, modifier: Modifier = Modifier.padding(top = 12.dp), batch: Batch? = null) {
    val context = LocalContext.current
    val first = group.first()
    val folder = batch?.folder
    val done = group.all { it.phase == Phase.DONE }
    val total = group.sumOf { it.total }.coerceAtLeast(1)
    val fraction = (group.sumOf { it.done }.toFloat() / total).coerceIn(0f, 1f)
    val photos = group.all { FileKind.of(it.name) == FileKind.IMAGE }
    val title = when {
        folder != null && first.way == Way.FETCH -> if (done) stringResource(R.string.tr_folder_here, folder) else stringResource(R.string.tr_getting, folder)
        folder != null -> if (done) stringResource(R.string.tr_sent_one, folder) else stringResource(R.string.tr_sending_one, folder)
        first.way == Way.FETCH -> if (done) first.name else stringResource(R.string.tr_getting, first.name)
        done && group.size == 1 -> stringResource(R.string.tr_sent_one, first.name)
        done -> stringResource(R.string.tr_sent_files, group.size)
        group.size == 1 -> stringResource(R.string.tr_sending_one, first.name)
        photos -> stringResource(R.string.tr_sending_photos, group.size)
        else -> stringResource(R.string.tr_sending_files, group.size)
    }
    val problem = group.firstNotNullOfOrNull { it.problem }
    val paused = group.any { it.phase == Phase.PAUSED || it.phase == Phase.WAITING_PATH || it.phase == Phase.FAILED } && !group.any { it.phase == Phase.RUNNING }
    Card(modifier, padding = 18.dp) {
        Row(Modifier.fillMaxWidth().semantics(mergeDescendants = true) { liveRegion = LiveRegionMode.Polite }, verticalAlignment = Alignment.CenterVertically) {
            if (folder != null) Box(Modifier.padding(end = 12.dp)) { RowIcon(FerbIcons.folder) }
            else if (first.way == Way.FETCH && done) FileIcon(first.name, 26.dp, Modifier.padding(end = 12.dp))
            PhoneText(title, 16f, FerbType.Semibold, Ink.black, Modifier.weight(1f), maxLines = 2)
            if (!done) PhoneText(if (paused) stringResource(R.string.tr_paused) else "${(fraction * 100).toInt()}%", 15f, FerbType.Medium, Ink.faint)
        }
        if (!done) {
            Box(Modifier.padding(top = 10.dp).fillMaxWidth().height(6.dp).background(Ink.line, CircleShape)) {
                Box(Modifier.fillMaxHeight().fillMaxWidth(fraction).background(Ink.black, CircleShape))
            }
        }
        val lines = ArrayList<String>()
        if (folder != null) {
            lines += stringResource(R.string.tr_folder_progress, group.count { it.phase == Phase.DONE }, group.size,
                Formatter.formatShortFileSize(context, group.sumOf { it.done }), Formatter.formatShortFileSize(context, group.sumOf { it.total }))
            if (batch.cut) lines += stringResource(R.string.files_tree_cut, group.size)
            if (batch.skipped > 0) lines += pluralStringResource(R.plurals.files_tree_skipped, batch.skipped.toInt(), batch.skipped.toInt())
            if (batch.unreadable > 0) lines += pluralStringResource(R.plurals.tr_unreadable, batch.unreadable, batch.unreadable)
        }
        when {
            problem != null -> lines += noteText(problem)
            folder != null && first.way == Way.FETCH && done -> lines += stringResource(R.string.tr_on_phone)
            first.way == Way.SEND && done -> lines += stringResource(R.string.tr_place,
                group.firstNotNullOfOrNull { it.macPlace } ?: vm.ui.value.mac?.uploadFolder?.shown ?: stringResource(R.string.tr_place_default))
            first.way == Way.FETCH && done -> lines += stringResource(if (first.keyPlace) R.string.tr_key_place else R.string.tr_on_phone)
        }
        for (line in lines) PhoneText(line, 14f, color = Ink.detail, modifier = Modifier.fillMaxWidth().padding(top = 8.dp))
        Row(Modifier.fillMaxWidth().padding(top = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            when {
                done && first.way == Way.FETCH && folder != null -> {
                    Pill(stringResource(R.string.tr_share), Modifier.weight(1f), PillKind.BLACK, height = 42.dp,
                        onClick = { host.shareMany(group.map { File(it.local) }) })
                    Pill(stringResource(R.string.tr_done), kind = PillKind.GREY, height = 42.dp, fill = false, onClick = { vm.transfers.remove(first.key) })
                }
                done && first.way == Way.FETCH -> {
                    Pill(stringResource(R.string.tr_open), Modifier.weight(1f), PillKind.BLACK, height = 42.dp,
                        onClick = { host.handOut(first.key, File(first.local), first.name, share = false) })
                    Pill(stringResource(R.string.tr_share), Modifier.weight(1f), PillKind.GREY, height = 42.dp,
                        onClick = { host.handOut(first.key, File(first.local), first.name, share = true) })
                    Pill(stringResource(R.string.tr_done), kind = PillKind.GREY, height = 42.dp, fill = false, onClick = { vm.transfers.remove(first.key) })
                }
                done -> {
                    Pill(stringResource(R.string.tr_add_message), Modifier.weight(1f), PillKind.BLACK, height = 42.dp, onClick = { vm.attachToChat(first.batch!!) })
                    Pill(stringResource(R.string.tr_done), kind = PillKind.GREY, height = 42.dp, fill = false, onClick = { vm.transfers.remove(first.key) })
                }
                else -> {
                    if (paused) Pill(stringResource(R.string.tr_resume), Modifier.weight(1f), PillKind.BLACK, height = 42.dp, onClick = { vm.transfers.resume(first.key) })
                    else Pill(stringResource(R.string.tr_pause), Modifier.weight(1f), PillKind.GREY, height = 42.dp, onClick = { vm.pause(first.key) })
                    Pill(stringResource(R.string.tr_cancel), kind = PillKind.GREY, height = 42.dp, fill = false, onClick = { vm.transfers.remove(first.key) })
                }
            }
        }
    }
}
