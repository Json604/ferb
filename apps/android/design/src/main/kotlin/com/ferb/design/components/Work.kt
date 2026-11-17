package com.ferb.design.components

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import kotlinx.coroutines.delay

// The work row above the message box (kb PHN-725; FERB.app ChatBody.swift WorkRowView, TaskActivityBubble.swift):
// "1 active task · 0:42 · 1 paused", the round live pictures of the first two tasks with their amber ring and dot,
// "+N" for the rest, and "View tasks". A picture opens the task's short view: the larger picture, its title, what it
// is doing now, and the way to watch it full screen or stop it.

/** One running task as the work row shows it. [picture]: its live picture while one is shown (in memory only). */
@Immutable
data class WorkTask(
    val key: String,
    val title: String,
    val line: String,
    val waiting: Boolean = false,
    val paused: Boolean = false,
    val picture: ImageBitmap? = null,
)

/** The row's words either side of its time (FERB.app's WorkRowWords): "1 active task" … "1 paused · 1 kept going". */
data class WorkWords(val lead: String, val trail: String) {
    companion object {
        fun of(active: Int, paused: Int, kept: Int): WorkWords {
            fun tasks(n: Int) = if (n == 1) "task" else "tasks"
            val rest = buildList {
                if (paused > 0) add(if (active > 0) "$paused paused" else "$paused paused ${tasks(paused)}")
                if (kept > 0) add("$kept kept going")
            }
            return if (active > 0) WorkWords("$active active ${tasks(active)}", rest.joinToString(" · "))
            else WorkWords(rest.joinToString(" · "), "")
        }
    }
}

/** SwiftUI's `.timer` text: "0:42", "12:05", "1:02:03". */
fun elapsed(seconds: Long): String {
    val s = maxOf(0L, seconds)
    val h = s / 3600
    val m = (s % 3600) / 60
    val r = s % 60
    return if (h > 0) "%d:%02d:%02d".format(h, m, r) else "%d:%02d".format(m, r)
}

/**
 * The work row (FERB.app's WorkRowView): the running mark (a paused glyph when nothing runs), its words, the time since
 * the longest-running active task started ([since], epoch ms; ticking once a second in this text alone), the round
 * pictures and "View tasks". [now] fixes the clock (screenshots).
 */
@Composable
fun ActiveWorkRow(
    words: WorkWords,
    running: Boolean,
    since: Long?,
    tasks: List<WorkTask>,
    onPicture: (WorkTask) -> Unit,
    onMore: () -> Unit,
    onViewTasks: () -> Unit,
    modifier: Modifier = Modifier,
    viewTasks: String = "View tasks",
    now: Long? = null,
) {
    val ink = LocalSkyInk.current
    Row(
        modifier.fillMaxWidth().heightIn(min = 40.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        // the words give way first (one line, cut at its end); the pictures follow them and "View tasks" keeps its room
        Row(Modifier.weight(1f), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            if (running) RunningArc(16.dp, ink.top) else PausedGlyph(15.dp)
            Box(Modifier.weight(1f, fill = false)) {
                Ticking(since.takeIf { running }, now) { time ->
                    val words = listOfNotNull(words.lead, time, words.trail.ifEmpty { null }).joinToString(" · ")
                    PhoneText(words, 14f, FerbType.Medium, ink.low, maxLines = 1)
                }
            }
            for (t in tasks.take(2)) RoundPicture(t, 30.dp, Modifier.pressable({ onPicture(t) }, label = "Live preview of ${t.title}"))
            if (tasks.size > 2) {
                Box(Modifier.size(30.dp).background(Ink.black.copy(alpha = 0.55f), CircleShape).pressable(onMore, label = "${tasks.size - 2} more running tasks"),
                    contentAlignment = Alignment.Center) { PhoneText("+${tasks.size - 2}", 13f, FerbType.Semibold, Ink.white) }
            }
        }
        Box(Modifier.heightIn(min = 40.dp).pressable(onViewTasks, label = viewTasks).padding(horizontal = 4.dp), contentAlignment = Alignment.Center) {
            PhoneText(viewTasks, 14f, FerbType.Semibold, ink.top, maxLines = 1)
        }
    }
}

/** The time since [since] (epoch ms) as `.timer` says it, again each second in this text alone; null: no time. */
@Composable
private fun Ticking(since: Long?, fixed: Long?, content: @Composable (String?) -> Unit) {
    if (since == null) { content(null); return }
    val clock = fixed ?: LocalCardClock.current
    var now by remember { mutableLongStateOf(clock ?: System.currentTimeMillis()) }
    if (clock == null) LaunchedEffect(since) {
        while (true) {
            now = System.currentTimeMillis()
            delay(1000 - now % 1000)
        }
    }
    content(elapsed((now - since) / 1000))
}

/** [text] and the time since [since] (epoch ms), again each second in this text alone (FERB.app's LiveLineView). */
@Composable
fun TickingText(text: String, since: Long, size: Float, color: androidx.compose.ui.graphics.Color, modifier: Modifier = Modifier) {
    Ticking(since, null) { time -> PhoneText(if (time == null) text else "$text · $time", size, FerbType.Medium, color, modifier, maxLines = 1) }
}

/** A paused task's mark where a running one has its spinner (FERB.app's PausedGlyph). */
@Composable
fun PausedGlyph(size: androidx.compose.ui.unit.Dp) {
    val ink = LocalSkyInk.current
    Box(Modifier.size(size).border(1.5.dp, ink.low, CircleShape).semantics { contentDescription = "Paused" }, contentAlignment = Alignment.Center) {
        Row(horizontalArrangement = Arrangement.spacedBy(2.dp)) {
            Box(Modifier.size(2.dp, size * 0.4f).background(ink.low))
            Box(Modifier.size(2.dp, size * 0.4f).background(ink.low))
        }
    }
}

/** A task's round live picture with its amber ring and dot (FERB.app's work row circles); the sky's dark until a frame comes. */
@Composable
fun RoundPicture(task: WorkTask, size: androidx.compose.ui.unit.Dp, modifier: Modifier = Modifier) {
    Box(modifier.size(size)) {
        Box(Modifier.fillMaxSize().clip(CircleShape).background(Ink.darkFill).border(1.dp, Ink.amber.copy(alpha = 0.65f), CircleShape)) {
            task.picture?.let { Image(it, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop) }
        }
        Box(Modifier.align(Alignment.BottomEnd).offset(1.dp, 1.dp).size(7.dp).background(Ink.amber, CircleShape))
    }
}

/**
 * A task's short view (FERB.app's TaskShortView): its larger picture (or, with none, why), its title, what it is doing
 * now, and Watch live and Stop. Shown in a sheet; the screen is kept out of screenshots while it shows.
 */
@Composable
fun TaskShortView(
    task: WorkTask,
    empty: String?,
    onWatch: (() -> Unit)?,
    onStop: (() -> Unit)?,
    watchLive: String = "Watch live",
    stop: String = "Stop",
) {
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Box(Modifier.fillMaxWidth().aspectRatio(16f / 9f).clip(RoundedCornerShape(16.dp)).background(Ink.darkFill), contentAlignment = Alignment.Center) {
            task.picture?.let { Image(it, "Live preview of ${task.title}", Modifier.fillMaxSize(), contentScale = ContentScale.Fit) }
                ?: empty?.let { PhoneText(it, 14.5f, FerbType.Medium, Ink.onDark, Modifier.padding(20.dp), align = androidx.compose.ui.text.style.TextAlign.Center) }
        }
        Row(Modifier.padding(horizontal = 4.dp), horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
            if (task.paused) PausedGlyph(18.dp) else RunningArc(18.dp, Ink.black)
            Column(Modifier.weight(1f).semantics(mergeDescendants = true) { liveRegion = LiveRegionMode.Polite }, verticalArrangement = Arrangement.spacedBy(2.dp)) {
                PhoneText(task.title, 16.5f, FerbType.Semibold, maxLines = 2)
                PhoneText(task.line, 14.5f, color = if (task.waiting) Ink.amber else Ink.detail, maxLines = 2)
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Pill(watchLive, Modifier.weight(1f), PillKind.GREY, FerbIcons.eye, 46.dp, onClick = onWatch)
            Pill(stop, Modifier.weight(1f), PillKind.BLACK, FerbIcons.stop, 46.dp, onClick = onStop)
        }
    }
}
