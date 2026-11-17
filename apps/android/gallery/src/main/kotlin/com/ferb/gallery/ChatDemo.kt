package com.ferb.gallery

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.ferb.design.FerbType
import com.ferb.design.components.ChatWaiting
import com.ferb.design.components.DayDivider
import com.ferb.design.components.FerbBubble
import com.ferb.design.components.LocalSkyInk
import com.ferb.design.components.NewMessagePill
import com.ferb.design.components.OwnerBubble
import com.ferb.design.components.PhoneComposer
import com.ferb.design.components.PhoneTab
import com.ferb.design.components.PhoneText
import com.ferb.design.components.TopBar
import com.ferb.design.components.TypingDots
import com.ferb.design.components.rememberChatFollow
import com.ferb.design.motionItem
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.ime
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.ferb.design.sky.SkyMoment
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * The chat's list as the app draws it (kb PHN-626..PHN-628), played with invented words: the waiting look while the
 * first page comes, the stored chat with its days, FERB thinking, a reply that grows as it streams (its end followed),
 * and, scrolled up, the "New message" pill. The same :design pieces the app's chat uses; nothing here reaches a Mac.
 */
@Composable
fun ChatDemo(moment: SkyMoment, insets: PhoneInsets) {
    data class Row(val key: String, val kind: String, val text: String)
    var rows by remember { mutableStateOf(emptyList<Row>()) }
    var waiting by remember { mutableStateOf(true) }
    LaunchedEffect(Unit) {
        delay(1800)
        waiting = false
        rows = listOf(
            Row("d1", "day", "Yesterday"),
        ) + (1..10).map { Row("y$it", if (it % 2 == 0) "ferb" else "owner", if (it % 2 == 0) "Done: step $it of the plan, the files are in your Downloads folder." else "Next one, please ($it)") } + listOf(
            Row("a", "owner", "Compare the three standing desks I bookmarked"),
            Row("b", "ferb", "The Flexispot E7 is cheapest at \$389, delivered Thursday."),
            Row("d2", "day", "Today"),
            Row("c", "owner", "Did the desk ship?"),
        )
        delay(1500)
        rows = rows + Row("t", "typing", "")
        delay(1600)
        val reply = "Yes, it shipped this morning. It arrives Thursday between 9 and 12, and the courier will call first. " +
            "I added the delivery to your calendar and put the tracking link in Notes."
        for (n in 12..reply.length step 12) {
            rows = rows.filter { it.key != "t" && it.key != "r" } + Row("r", "ferb", reply.take(n))
            delay(90)
        }
        rows = rows.filter { it.key != "r" } + Row("r", "ferb", reply)
    }
    val list = rememberLazyListState()
    val last = rows.lastOrNull()
    val follow = rememberChatFollow(list, last?.key, last, mine = false)
    val scope = rememberCoroutineScope()
    val ink = LocalSkyInk.current
    // a real message box and keyboard, as the app has them: the keyboard's height under the box (kb PHN-670)
    var typed by remember { mutableStateOf("") }
    val density = androidx.compose.ui.platform.LocalDensity.current
    val ime = with(density) { WindowInsets.ime.getBottom(this).toDp() }
    val bottom = if (ime > insets.bottom) ime else insets.bottom
    // only rows new since the last draw fade in, as in the app (kb PHN-670)
    val drawn = remember { arrayOfNulls<Set<String>>(1) }
    val keys = remember(rows) { rows.mapTo(HashSet()) { it.key } }
    val arriving = drawn[0]?.let { keys - it }.orEmpty()
    androidx.compose.runtime.SideEffect { drawn[0] = keys }
    PhoneScreen(moment, insets = insets) {
        Column(Modifier.fillMaxSize()) {
            TopBar(PhoneTab.CHAT, waiting = null)
            Box(Modifier.weight(1f).fillMaxWidth()) {
                if (waiting) ChatWaiting("Getting your chat from your Mac…")
                LazyColumn(Modifier.fillMaxSize(), state = list, contentPadding = PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    items(rows, key = { it.key }) { r ->
                        Box(motionItem(arriving = r.key in arriving)) {
                            when (r.kind) {
                                "day" -> DayDivider(r.text)
                                "owner" -> Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.End) { OwnerBubble(r.text) }
                                "typing" -> Row(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
                                    TypingDots()
                                    PhoneText("FERB is thinking…", 14f, FerbType.Medium, ink.low)
                                }
                                else -> FerbBubble(r.text)
                            }
                        }
                    }
                }
                if (!follow.following && follow.unseen) {
                    NewMessagePill("New message", Modifier.align(Alignment.BottomCenter).padding(bottom = 10.dp)) { scope.launch { follow.toEnd(true) } }
                }
            }
            PhoneComposer(Modifier.padding(start = 16.dp, end = 16.dp, bottom = bottom + 8.dp), placeholder = "Add to this",
                typed = typed, onValueChange = { typed = it })
        }
    }
}
