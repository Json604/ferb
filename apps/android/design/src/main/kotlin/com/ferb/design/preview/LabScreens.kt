package com.ferb.design.preview

import androidx.compose.runtime.Composable
import com.ferb.design.FerbIcons
import com.ferb.design.components.LinkKind
import com.ferb.design.components.MarkKind
import com.ferb.design.components.PlanPart
import com.ferb.design.screens.ChatItem
import com.ferb.design.screens.ChatPage
import com.ferb.design.screens.CodesPage
import com.ferb.design.screens.FileRow
import com.ferb.design.screens.FilesPage
import com.ferb.design.screens.HomePage
import com.ferb.design.screens.HomeTexts
import com.ferb.design.screens.LabAndroidChrome
import com.ferb.design.screens.MacPage
import com.ferb.design.screens.PairPage
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.ferb.design.screens.PlaceRow
import com.ferb.design.screens.ProfileSheet
import com.ferb.design.screens.RelayCopy
import com.ferb.design.screens.RelaySheet
import com.ferb.design.screens.SecretSheet
import com.ferb.design.screens.TasksPage
import com.ferb.design.screens.TerminalQuestion
import com.ferb.design.screens.Transfer
import com.ferb.design.screens.UnlockPage
import com.ferb.design.screens.WorkGroup
import com.ferb.design.screens.WorkRow
import com.ferb.design.sky.SkyMoment

// The glass lab's twelve phone screens (MobileScreens.swift MobileScreen) with the lab's own invented data, for the
// gallery and the screenshot tests. Every name, task, code and place is invented; nothing here is FERB's real data.

enum class LabScreen(val title: String, val note: String) {
    HOME("Home", "Opening FERB: the sky over you right now, its colour from the sun's real height where you are. The sun is FERB's amber dot, where it really is."),
    CHAT("Chat", "Tabs at the top. Your words on white, FERB's on black, a task's parts in one white card with Watch live and Stop."),
    QUESTION("Question", "A question slides up over whatever is open and shows the exact thing. Yes asks for a fingerprint."),
    SECRET("Private answer", "A private answer goes straight to the Mac, encrypted, and is not kept on the phone."),
    TASKS("Tasks", "What waits for you, what runs, what finished: one white card per group."),
    FILES("Files", "Your Mac's files: search, places, recent, and sending files to the Mac."),
    MAC("Mac", "Your phone and the Mac, joined by a still line of dots, and how they are connected. Nothing moves."),
    RELAY("Relay", "On the shared relay the line goes through it; the sheet says what works there and how to get everything."),
    PROFILE("Profile", "The profile button: this phone's own settings, the lock, staying connected, and forgetting the Mac."),
    UNLOCK("Unlock", "FERB opens with a fingerprint every time."),
    PAIR("Pair", "First run: the Mac is a dashed circle until it is paired. One button."),
    CODES("Codes", "Both screens show the same code; you say yes on the Mac only if they match."),
}

/** MobileScreens.swift SampleWork: the home screen's list. */
object SampleWork {
    val waiting = listOf("Renew the domain" to "Waiting for your yes", "Pay the electricity bill" to "Asks before paying $48.20")
    val done = listOf("Backed up the thesis folder" to "Copied to the external drive")
    val away: List<WorkRow> = waiting.take(1).map { WorkRow(MarkKind.ASKING, it.first, it.second) } +
        done.take(1).map { WorkRow(MarkKind.DONE, it.first, it.second) }
}

/** The lab's chat (MobileScreens.swift ChatPage). */
val labChat: List<ChatItem> = listOf(
    ChatItem.Owner("What's left on my calendar today?"),
    ChatItem.Ferb("Two things: the design review at 4, and a call with the bank at 6:30."),
    ChatItem.Owner("Compare the three standing desks I bookmarked and put the cheapest in my cart"),
    ChatItem.Task(
        "On it. Three parts.", "2 of 3",
        listOf(
            PlanPart(MarkKind.DONE, "Open your three desks", "3 pages read"),
            PlanPart(MarkKind.RUNNING, "Compare price and delivery", "Reading delivery options"),
            PlanPart(MarkKind.WAITING, "Put the cheapest in your cart"),
        ),
    ),
)

/** One lab screen at [moment]. [chrome] draws the lab's Android status bar and gesture bar over it (previews only). */
@Composable
fun LabScreenView(
    screen: LabScreen,
    moment: SkyMoment,
    texts: HomeTexts,
    insets: PhoneInsets = PhoneInsets.Lab,
    chrome: Boolean = true,
    seasonTime: Double? = null,
    glide: Boolean = true,
) {
    PhoneScreen(
        moment, insets = insets, season = screen == LabScreen.HOME, seasonTime = seasonTime, glide = glide,
        sheet = {
            when (screen) {
                LabScreen.QUESTION -> TerminalQuestion("brew upgrade --cask figma", "To update Figma before opening the file you asked for.", "1 of 2", insets)
                LabScreen.SECRET -> SecretSheet("Sign-in code", "The 6-digit code Northfield Energy sent you.", 4, insets)
                LabScreen.RELAY -> RelaySheet(labRelay, insets)
                LabScreen.PROFILE -> ProfileSheet("MacBook Pro", "10 November", insets)
                else -> Unit
            }
            if (chrome) LabAndroidChrome()
        },
    ) {
        when (screen) {
            LabScreen.HOME, LabScreen.PROFILE -> HomePage(texts, SampleWork.away, insets)
            LabScreen.CHAT, LabScreen.QUESTION, LabScreen.SECRET -> ChatPage(labChat, insets)
            LabScreen.TASKS -> TasksPage(labTasks)
            LabScreen.FILES -> FilesPage(labPlaces, labRecent, Transfer("Sending 2 photos to your Mac", 0.64f), insets)
            LabScreen.MAC -> MacPage(LinkKind.DIRECT)
            LabScreen.RELAY -> MacPage(LinkKind.RELAY)
            LabScreen.UNLOCK -> UnlockPage(insets)
            LabScreen.PAIR -> PairPage(insets)
            LabScreen.CODES -> CodesPage("482719", insets)
        }
    }
}

/**
 * The relay sheet's words. The lab's rows ("Small ones", "Off") were its old rule; the owner's is that the shared relay
 * carries text, questions and Stop only (PHN-410, PHN-444): files, pictures and Watch live wait for a direct path or
 * the owner's own relay. The phone keeps trying for a direct path meanwhile (PHN-425, PHN-445).
 */
val labRelay = RelayCopy(
    explanation = "Your phone and Mac can't reach each other directly on these networks, so they meet at a free shared relay. " +
        "It only passes on what is already encrypted, and only messages, questions and Stop: files, pictures and Watch live " +
        "wait until your phone reaches your Mac directly or through your own relay.",
    works = listOf(
        "Messages, questions, answers, Stop" to "Always",
        "Files and pictures" to "Not on the shared relay",
        "Watch live" to "Not on the shared relay",
    ),
    fullSpeed = "For everything at full speed: the same Wi-Fi as your Mac, or your own relay.",
)

/**
 * MobileScreens.swift SkyNow.sunLine's time left: "45 min", "2 h 12 min", and whole hours without the zero minutes
 * ("5 h", where the lab wrote "5 h 0 min"): the owner's choice, kb PHN-595. The design's spacing is kept. The iPhone's
 * twin is FerbDesign's labMinutesLeft; both are held to test resources words/time-left.json.
 */
fun labMinutesLeft(left: Int): String = when {
    left < 60 -> "$left min"
    left % 60 == 0 -> "${left / 60} h"
    else -> "${left / 60} h ${left % 60} min"
}

val labTasks = listOf(
    WorkGroup("Waiting for you", listOf(WorkRow(MarkKind.ASKING, "Renew the domain", "Asks before paying $12.99"))),
    WorkGroup("Running", listOf(
        WorkRow(MarkKind.RUNNING, "Compare standing desks", "Comparing price and delivery", "2 of 3"),
        WorkRow(MarkKind.RUNNING, "Summarise this week's newsletters", "Reading 4 of 6"),
    )),
    WorkGroup("Earlier today", listOf(
        WorkRow(MarkKind.DONE, "Back up the thesis folder", "Copied to the external drive"),
        WorkRow(MarkKind.FAILED, "Download the October invoice", "The site asked for a code that never came"),
        WorkRow(MarkKind.DONE, "Clean up Downloads", "214 files moved to Archive"),
    )),
)

val labPlaces = listOf(
    PlaceRow(FerbIcons.desktop, "Desktop", "38"), PlaceRow(FerbIcons.doc, "Documents", "112"), PlaceRow(FerbIcons.download, "Downloads", "9"),
)

val labRecent = listOf(FileRow("Thesis draft v7.docx", "2.4 MB, edited today"), FileRow("Lease agreement.pdf", "1.1 MB, Monday"))

/**
 * MobileScreens.swift SkyNow.greeting and sunLine: the lab's stand-in words for the home screen (the app's own words
 * come from words/corpus.json, S11). Times are written as the lab writes them ("h:mm a").
 */
fun labHomeTexts(moment: SkyMoment): HomeTexts {
    val hour = moment.local.hour
    val greeting = when (hour) {
        in 5..11 -> "Good morning"
        in 12..16 -> "Good afternoon"
        in 17..21 -> "Good evening"
        else -> "Still up?"
    }
    val day = com.ferb.design.sky.SunDay.around(moment.epochSeconds, moment.place.latitude, moment.place.longitude, moment.place.zone)
    val clock = java.time.format.DateTimeFormatter.ofPattern("h:mm a", java.util.Locale.US)
    fun at(t: Double) = java.time.Instant.ofEpochSecond(t.toLong()).atZone(moment.place.zone).format(clock)
    val rise = day.sunrise
    val set = day.sunset
    val line = when {
        rise == null || set == null -> "No sunrise or sunset here today."
        moment.epochSeconds < rise -> "Sunrise at ${at(rise)}."
        moment.epochSeconds < set -> {
            val left = ((set - moment.epochSeconds) / 60).toInt()
            "Sunset at ${at(set)}, in ${labMinutesLeft(left)}."
        }
        else -> "The sun set at ${at(set)}."
    }
    return HomeTexts(greeting, line)
}
