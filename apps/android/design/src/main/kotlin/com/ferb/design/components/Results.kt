package com.ferb.design.components

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Alarm
import androidx.compose.material.icons.outlined.CalendarMonth
import androidx.compose.material.icons.outlined.CreditCard
import androidx.compose.material.icons.outlined.Language
import androidx.compose.material.icons.outlined.Psychology
import androidx.compose.material.icons.outlined.Schedule
import androidx.compose.material.icons.outlined.Timer
import androidx.compose.material.icons.rounded.Call
import androidx.compose.material.icons.rounded.Email
import androidx.compose.material.icons.rounded.Repeat
import androidx.compose.material.icons.rounded.Send
import androidx.compose.material.icons.automirrored.rounded.Reply
import androidx.compose.material.icons.rounded.Edit
import androidx.compose.material.icons.rounded.Verified
import com.ferb.design.FerbIcon
import com.ferb.design.FerbType
import com.ferb.design.Ink

// What a task did, drawn as FERB.app draws it (kb PHN-724; FERB.app ResultCards.swift, ArtifactPreview.swift): one
// card per kind, in the phone's white cards. The app gives the words ready (its dates in the phone's own format); a card
// of a kind this build doesn't know is never drawn (its line stays in FERB's words, as on the Mac).

/** The icons the cards use (FERB.app's SF Symbols, by their Material counterparts). */
object CardIcons {
    val globe: ImageVector = Icons.Outlined.Language
    val reminder: ImageVector = Icons.Outlined.Alarm
    val timer: ImageVector = Icons.Outlined.Timer
    val calendar: ImageVector = Icons.Outlined.CalendarMonth
    val clock: ImageVector = Icons.Outlined.Schedule
    val repeat: ImageVector = Icons.Rounded.Repeat
    val note: ImageVector = Icons.Outlined.Psychology
    val card: ImageVector = Icons.Outlined.CreditCard
    val approved: ImageVector = Icons.Rounded.Verified
    val send: ImageVector = Icons.Rounded.Send
    val call: ImageVector = Icons.Rounded.Call
    val mail: ImageVector = Icons.Rounded.Email
    val post: ImageVector = Icons.Rounded.Edit
    val reply: ImageVector = Icons.AutoMirrored.Rounded.Reply
}

/**
 * The sites row (APP-700: "one site plus rest under +x"): a globe, the first site as a link, and "+N" opening the list
 * of all of them. A tap on the name opens that site; on "+N" or anywhere else on the row, the list (or the one site).
 */
@Composable
fun SitesChip(sites: List<String>, onOpen: (String) -> Unit, onAll: () -> Unit, modifier: Modifier = Modifier) {
    if (sites.isEmpty()) return
    val ink = LocalSkyInk.current
    Row(
        modifier.background(Ink.black.copy(alpha = 0.55f), CircleShape)
            .pressable({ if (sites.size > 1) onAll() else onOpen(sites[0]) }, label = if (sites.size > 1) "Show all ${sites.size} visited sites" else "Open ${sites[0]}")
            .heightIn(min = 36.dp).padding(horizontal = 12.dp, vertical = 7.dp),
        horizontalArrangement = Arrangement.spacedBy(7.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        FerbIcon(CardIcons.globe, 15f, ink.low)
        Box(Modifier.pressable({ onOpen(sites[0]) }, label = "Open ${sites[0]}")) {
            androidx.compose.foundation.text.BasicText(
                sites[0], style = FerbType.style(14f, FerbType.Medium, onBlack.link).copy(textDecoration = TextDecoration.Underline), maxLines = 1,
            )
        }
        if (sites.size > 1) {
            Box(Modifier.background(Ink.white.copy(alpha = 0.14f), RoundedCornerShape(6.dp)).pressable(onAll, label = "Show all ${sites.size} visited sites")
                .padding(horizontal = 7.dp, vertical = 2.dp)) {
                PhoneText("+${sites.size - 1}", 13f, FerbType.Semibold, Ink.white)
            }
        }
    }
}

/** The list the sites row opens ("Sites visited"): each site as a link. */
@Composable
fun SitesList(title: String, sites: List<String>, onOpen: (String) -> Unit) {
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        PhoneText(title, 20f, FerbType.Semibold, modifier = Modifier.padding(horizontal = 8.dp, vertical = 6.dp))
        Card {
            sites.forEachIndexed { i, host ->
                if (i > 0) Divide()
                CardRow(host, onClick = { onOpen(host) }, leading = { FerbIcon(CardIcons.globe, 20f, Ink.black) }, trailing = { Opens() })
            }
        }
    }
}

/** One of a card's filled fields: a soft capsule with its icon (FERB.app's CardChip). */
@Composable
fun CardChip(icon: ImageVector?, text: String) {
    val ink = CardInk.current()
    Row(Modifier.background(ink.fill, CircleShape).padding(horizontal = 10.dp, vertical = 5.dp),
        horizontalArrangement = Arrangement.spacedBy(5.dp), verticalAlignment = Alignment.CenterVertically) {
        if (icon != null) FerbIcon(icon, 13f, ink.secondary)
        PhoneText(text, 13.5f, FerbType.Medium, ink.primary, maxLines = 1)
    }
}

/** "Repeats", its rule in words under it where the card has room (FERB.app's RepeatsBadge). */
@Composable
fun RepeatsBadge() {
    val ink = CardInk.current()
    Row(Modifier.background(ink.primary, CircleShape).padding(horizontal = 9.dp, vertical = 4.dp),
        horizontalArrangement = Arrangement.spacedBy(4.dp), verticalAlignment = Alignment.CenterVertically) {
        FerbIcon(CardIcons.repeat, 12f, ink.card)
        PhoneText("Repeats", 12.5f, FerbType.Semibold, ink.card)
    }
}

/** What kind of card this is, above its title (FERB.app's CardMeta). */
@Composable
fun CardMeta(icon: ImageVector, text: String) {
    val ink = CardInk.current()
    Row(horizontalArrangement = Arrangement.spacedBy(5.dp), verticalAlignment = Alignment.CenterVertically) {
        FerbIcon(icon, 13f, ink.secondary)
        PhoneText(text, 13f, FerbType.Medium, ink.secondary)
    }
}

/** A card's surface: the phone's white card with room around its words. */
@Composable
fun ResultSurface(modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    val ink = CardInk.current()
    Column(modifier.fillMaxWidth().background(ink.card, RoundedCornerShape(22.dp)).padding(horizontal = 16.dp, vertical = 14.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp)) { content() }
}

/** One result card's words, as the app read them from the card (FERB.app's views by `type`). */
@Immutable
sealed interface ResultShown {
    /** A reminder, timer or schedule run that came due: "Reminder"/"Timer", its title, the reminder's own words, when it was due, Repeats, the countdown. */
    data class Reminder(val timer: Boolean, val title: String, val words: String?, val due: String?, val repeats: Boolean, val countdown: CountdownShown?) : ResultShown
    /** A reminder, timer or schedule just set: its meta ("Reminder set", "Timer set", "Scheduled", "Schedule changed"), what, the day and time, the rule, the change. */
    data class Event(val meta: String, val icon: ImageVector, val what: String, val day: String?, val time: String?, val repeats: Boolean, val rule: String?, val changes: List<Pair<String, String>>, val countdown: CountdownShown?) : ResultShown
    /** A remembered fact: its state's words and the fact (struck through once forgotten). */
    data class Note(val meta: String, val text: String, val forgotten: Boolean) : ResultShown
    /** A payment that ran: merchant, "You approved", the site, the saved card's name, the amount or "Amount not read". */
    data class Payment(val title: String, val site: String?, val savedCard: String?, val amount: String?) : ResultShown
    /** A send that ran: initials, recipient, "Sent/Emailed/Called/Posted/Replied · place", "From account". */
    data class Send(val initials: String, val title: String, val verb: String, val icon: ImageVector, val account: String?) : ResultShown
    /** The owner's schedules: "N schedules" and each one's name, timing, state and next run. */
    data class Schedules(val meta: String, val rows: List<ScheduleShown>) : ResultShown
}

@Immutable
data class ScheduleShown(val name: String, val rule: String, val state: String?, val day: String?, val time: String?)

/** One card by its kind (FERB.app's ResultCardView). */
@Composable
fun ResultCard(card: ResultShown, modifier: Modifier = Modifier) {
    val ink = CardInk.current()
    ResultSurface(modifier.semantics(mergeDescendants = true) {}) {
        when (card) {
            is ResultShown.Reminder -> {
                CardMeta(if (card.timer) CardIcons.timer else CardIcons.reminder, if (card.timer) "Timer" else "Reminder")
                PhoneText(card.title, 17f, FerbType.Semibold, ink.primary)
                card.words?.let { PhoneText(it, 15f, color = ink.primary, lineSpacing = 2f) }
                ChipRow {
                    card.due?.let { CardChip(CardIcons.clock, it) }
                    if (card.repeats) RepeatsBadge()
                }
                card.countdown?.let { Countdown(it) }
            }
            is ResultShown.Event -> {
                CardMeta(card.icon, card.meta)
                PhoneText(card.what, 17f, FerbType.Semibold, ink.primary)
                for ((label, text) in card.changes) PhoneText("$label: $text", 13.5f, color = ink.secondary, maxLines = 3)
                ChipRow {
                    card.day?.let { CardChip(CardIcons.calendar, it) }
                    card.time?.let { CardChip(CardIcons.clock, it) }
                    if (card.repeats) RepeatsBadge()
                }
                card.rule?.let { PhoneText(it, 13.5f, color = ink.secondary) }
                card.countdown?.let { Countdown(it) }
            }
            is ResultShown.Note -> {
                CardMeta(CardIcons.note, card.meta)
                Box {
                    androidx.compose.foundation.text.BasicText(card.text, style = FerbType.style(15.5f, color = if (card.forgotten) ink.secondary else ink.primary, lineSpacing = 2f)
                        .copy(textDecoration = if (card.forgotten) TextDecoration.LineThrough else null))
                }
            }
            is ResultShown.Payment -> Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(40.dp).background(ink.fill, RoundedCornerShape(10.dp)), contentAlignment = Alignment.Center) { FerbIcon(CardIcons.card, 19f, ink.primary) }
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    PhoneText(card.title, 17f, FerbType.Semibold, ink.primary, maxLines = 2)
                    ChipRow {
                        CardChip(CardIcons.approved, "You approved")
                        card.site?.let { CardChip(CardIcons.globe, it) }
                        card.savedCard?.let { CardChip(CardIcons.card, it) }
                    }
                }
                PhoneText(card.amount ?: "Amount not read", if (card.amount == null) 13.5f else 22f, FerbType.Semibold, if (card.amount == null) ink.secondary else ink.primary, maxLines = 1)
            }
            is ResultShown.Send -> Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.Top) {
                Box(Modifier.size(40.dp).background(ink.fill, CircleShape), contentAlignment = Alignment.Center) { PhoneText(card.initials, 15f, FerbType.Semibold, ink.primary) }
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    PhoneText(card.title, 17f, FerbType.Semibold, ink.primary, maxLines = 2)
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                        FerbIcon(card.icon, 13f, ink.secondary)
                        PhoneText(card.verb, 14f, color = ink.secondary, maxLines = 1)
                    }
                    card.account?.let { PhoneText("From $it", 13.5f, color = ink.faint) }
                }
            }
            is ResultShown.Schedules -> {
                CardMeta(CardIcons.calendar, card.meta)
                card.rows.forEach { r ->
                    Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
                        PhoneText(r.name, 16f, FerbType.Semibold, ink.primary)
                        PhoneText(r.rule, 13.5f, color = ink.secondary)
                        ChipRow {
                            r.state?.let { CardChip(null, it) }
                            r.day?.let { CardChip(CardIcons.calendar, it) }
                            r.time?.let { CardChip(CardIcons.clock, it) }
                        }
                    }
                }
            }
        }
    }
}

@OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
private fun ChipRow(content: @Composable () -> Unit) {
    androidx.compose.foundation.layout.FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) { content() }
}

/** What a card counts down to (FERB.app's CountdownTarget): its time, and what it says before and once it is due. */
@Immutable
data class CountdownShown(val at: Long, val caption: String, val dueCaption: String)

/** FERB.app's Countdown.text without "in ": "2h 59m", "3d 4h", "12m", "40s"; "Now" once due. */
fun countdownText(at: Long, now: Long): String {
    val s = (at - now) / 1000
    if (s <= 0) return "Now"
    if (s < 60) return "${s}s"
    val m = s / 60
    if (m < 60) return "${m}m"
    val h = m / 60
    if (h < 24) return "${h}h ${m % 60}m"
    return "${h / 24}d ${h % 24}h"
}

/** The time left large and what it counts to under it, again every 30 s (FERB.app's CountdownLine); [now] fixes it. */
@Composable
private fun Countdown(c: CountdownShown) {
    // another day's chat is a record: no countdown (FERB.app's `cardsLive`, APP-769)
    if (!LocalCardsLive.current) return
    val fixed = LocalCardClock.current
    var now by androidx.compose.runtime.remember { androidx.compose.runtime.mutableLongStateOf(fixed ?: System.currentTimeMillis()) }
    if (fixed == null) androidx.compose.runtime.LaunchedEffect(c.at) {
        while (true) { now = System.currentTimeMillis(); kotlinx.coroutines.delay(30_000) }
    }
    val due = c.at <= now
    val ink = CardInk.current()
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Bottom) {
        PhoneText(countdownText(c.at, now), 24f, FerbType.Semibold)
        PhoneText(if (due) c.dueCaption else c.caption, 13.5f, FerbType.Medium, ink.secondary, Modifier.padding(bottom = 3.dp))
    }
}

/** Whether the cards draw live state (today's chat); the earlier days' are a record (APP-769). */
val LocalCardsLive = androidx.compose.runtime.staticCompositionLocalOf { true }

/** A fixed clock for the cards' countdowns (screenshots); null: the phone's. */
val LocalCardClock = androidx.compose.runtime.staticCompositionLocalOf<Long?> { null }

/** A button that opens a page (FERB.app's page cards: a notice's "Open the Memory page"). */
@Composable
fun PageButton(text: String, onClick: (() -> Unit)?) {
    Pill(text, Modifier.widthIn(max = 320.dp), PillKind.GREY, null, 40.dp, onClick = onClick)
}
