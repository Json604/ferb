package com.ferb.app.ui

import android.text.format.Formatter
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
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
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.ferb.app.Feature
import com.ferb.app.FerbViewModel
import com.ferb.app.R
import com.ferb.app.Sheet
import com.ferb.app.UiState
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
import com.ferb.mobile.LinkPhase
import java.io.File
import java.text.DateFormat
import java.util.Date

/**
 * MobileScreens.swift PhoneFilesPage: search, the transfers under way (the lab's progress card), the places, and
 * sending files to the Mac; a place opens as a folder of the lab's rows. A key place's entry shows its lock and asks
 * for the fingerprint for that very path (PHN-121). Search and Recent have no call on the Mac yet: they say so.
 */
@Composable
fun Files(vm: FerbViewModel, ui: UiState, insets: PhoneInsets, waiting: PhoneTab?, onTab: (PhoneTab) -> Unit, onProfile: () -> Unit, host: Host) {
    val transfers by vm.transfers.list.collectAsState()
    val folder = ui.files.folder
    BackHandler(enabled = folder != null) { vm.folderUp() }
    val ink = LocalSkyInk.current
    Column(Modifier.fillMaxSize()) {
        TopBar(PhoneTab.FILES, waiting = waiting, onTab = onTab, onProfile = onProfile)
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp)) {
            Row(
                Modifier.padding(top = 14.dp).fillMaxWidth().heightIn(min = 50.dp).background(Ink.black, CircleShape)
                    .pressable({ vm.show(Sheet.NotYet(Feature.SEARCH)) }).padding(horizontal = 16.dp)
                    .semantics(mergeDescendants = true) { contentDescription = "Search your Mac" },
                horizontalArrangement = Arrangement.spacedBy(10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                FerbIcon(FerbIcons.search, 17f, Ink.onDark)
                PhoneText("Search your Mac", 16.5f, color = Ink.onDark)
            }
            for (group in groups(transfers)) TransferCard(vm, group, host)
            if (ui.link?.phase == LinkPhase.ONLINE && ui.link.bulkAllowed == false) {
                PhoneText(stringResource(R.string.files_needs_better_path), 14f, FerbType.Medium, ink.low, Modifier.fillMaxWidth().padding(start = 8.dp, end = 8.dp, top = 12.dp))
            }
            ui.files.problem?.let { PhoneText(noteText(it), 14f, FerbType.Medium, ink.low, Modifier.fillMaxWidth().padding(start = 8.dp, end = 8.dp, top = 12.dp)) }
            if (folder == null) Places(vm, ui) else FolderView(vm, ui)
        }
        Pill("Send files to your Mac", Modifier.padding(start = 16.dp, end = 16.dp, top = 8.dp, bottom = insets.bottom + 8.dp), PillKind.BLACK, FerbIcons.sendFile,
            onClick = { host.pickFiles(false) })
    }
}

@Composable
private fun Places(vm: FerbViewModel, ui: UiState) {
    val ink = LocalSkyInk.current
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
    PhoneText(stringResource(R.string.files_recent_not_yet), 14.5f, FerbType.Medium, ink.low, Modifier.fillMaxWidth().padding(horizontal = 8.dp))
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
        PhoneText(f.path.substringAfterLast('/').ifEmpty { f.path }, 15f, FerbType.Semibold, ink.low, Modifier.weight(1f).padding(vertical = 8.dp), maxLines = 1)
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
}

@Composable
private fun detail(context: android.content.Context, e: FileEntry): String? {
    if (e.locked) return stringResource(R.string.files_key_place)
    if (e.dir) return null
    val size = Formatter.formatShortFileSize(context, e.size.toLong())
    val day = if (e.modified > 0) DateFormat.getDateInstance(DateFormat.MEDIUM).format(Date(e.modified * 1000)) else return size
    return stringResource(R.string.file_detail, size, day)
}

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

/** The lab's progress card, with Pause or Go on and Cancel; a fetched file gets Open and Share, on the owner's tap only. */
@Composable
fun TransferCard(vm: FerbViewModel, group: List<Transfer>, host: Host, modifier: Modifier = Modifier.padding(top = 12.dp)) {
    val first = group.first()
    val done = group.all { it.phase == Phase.DONE }
    val total = group.sumOf { it.total }.coerceAtLeast(1)
    val fraction = (group.sumOf { it.done }.toFloat() / total).coerceIn(0f, 1f)
    val photos = group.all { FileKind.of(it.name) == FileKind.IMAGE }
    val title = when {
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
            if (first.way == Way.FETCH && done) FileIcon(first.name, 26.dp, Modifier.padding(end = 12.dp))
            PhoneText(title, 16f, FerbType.Semibold, Ink.black, Modifier.weight(1f), maxLines = 2)
            if (!done) PhoneText(if (paused) stringResource(R.string.tr_paused) else "${(fraction * 100).toInt()}%", 15f, FerbType.Medium, Ink.faint)
        }
        if (!done) {
            Box(Modifier.padding(top = 10.dp).fillMaxWidth().height(6.dp).background(Ink.line, CircleShape)) {
                Box(Modifier.fillMaxHeight().fillMaxWidth(fraction).background(Ink.black, CircleShape))
            }
        }
        val line = when {
            problem != null -> noteText(problem)
            first.way == Way.FETCH && done -> stringResource(if (first.keyPlace) R.string.tr_key_place else R.string.tr_on_phone)
            else -> null
        }
        if (line != null) PhoneText(line, 14f, color = Ink.detail, modifier = Modifier.fillMaxWidth().padding(top = 8.dp))
        Row(Modifier.fillMaxWidth().padding(top = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            when {
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
