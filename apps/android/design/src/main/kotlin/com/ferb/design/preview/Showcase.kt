package com.ferb.design.preview

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.LabColors
import com.ferb.design.Lamp
import com.ferb.design.Motion
import com.ferb.design.Phone
import com.ferb.design.Radius
import com.ferb.design.Space
import com.ferb.design.Tone
import com.ferb.design.components.Card
import com.ferb.design.components.CardRow
import com.ferb.design.components.CloseButton
import com.ferb.design.components.Divide
import com.ferb.design.components.ExactCommand
import com.ferb.design.components.ExactItem
import com.ferb.design.components.FerbBubble
import com.ferb.design.components.FerbMark
import com.ferb.design.components.FileIcon
import com.ferb.design.components.FingerprintNote
import com.ferb.design.components.LampDot
import com.ferb.design.components.LinkKind
import com.ferb.design.components.LinkPicture
import com.ferb.design.components.LocalSkyInk
import com.ferb.design.components.Mark
import com.ferb.design.components.MarkKind
import com.ferb.design.components.Opens
import com.ferb.design.components.OwnerBubble
import com.ferb.design.components.PairAction
import com.ferb.design.components.PairStep
import com.ferb.design.components.PairingCode
import com.ferb.design.components.PhoneComposer
import com.ferb.design.components.PhoneSwitch
import com.ferb.design.components.PhoneTab
import com.ferb.design.components.PhoneText
import com.ferb.design.components.Pill
import com.ferb.design.components.PillKind
import com.ferb.design.components.QrFrame
import com.ferb.design.components.RowIcon
import com.ferb.design.components.SecretCodeBoxes
import com.ferb.design.components.SecretKeypad
import com.ferb.design.components.SheetIcon
import com.ferb.design.components.SkyLabel
import com.ferb.design.components.TaskCard
import com.ferb.design.components.TopBar
import com.ferb.design.components.TypingDots
import com.ferb.design.glass.Chip
import com.ferb.design.glass.GlassLevel
import com.ferb.design.glass.GlassPanel
import com.ferb.design.glass.LocalSkyBackdrop
import com.ferb.design.glass.SkyBackdrop
import com.ferb.design.glass.glass
import com.ferb.design.screens.outset
import com.ferb.design.glass.skyBackdropHost
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.offset
import com.ferb.design.sky.AstraPhase
import com.ferb.design.sky.AstraSky
import com.ferb.design.sky.SkyFrameLook
import com.ferb.design.sky.drawSkyLook
import com.ferb.design.sky.SeasonLayer
import com.ferb.design.sky.Season
import com.ferb.design.sky.Sky
import com.ferb.design.sky.SkyMoment
import com.ferb.design.sky.SkyPlace

// Every token and component on its own, with invented data: the gallery's pages and the screenshot tests' subjects.

/** The lab's time buttons (Stage.swift): local hours at the place on show. */
val LAB_TIMES = listOf("Dawn" to 6.1, "Sunrise" to 6.9, "Morning" to 9.0, "Noon" to 12.5, "Golden hour" to 16.6,
    "Sunset" to 17.4, "Dusk" to 17.9, "Night" to 22.5)

/** The day the screenshots are drawn on: the lab's last commit, 2026-11-14. */
fun labMoment(hour: Double, place: SkyPlace = SkyPlace.DELHI, year: Int = 2026, month: Int = 11, day: Int = 14) =
    SkyMoment.at(place, year, month, day, hour)

/** A piece of sky to show a component where it lives: the sky, its ink, and its backdrop for glass. */
@Composable
fun OnSky(moment: SkyMoment, modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    val backdrop = SkyBackdrop(moment.sun.elevation, moment.across, moment.moon)
    CompositionLocalProvider(LocalSkyInk provides moment.ink, LocalSkyBackdrop provides backdrop) {
        Box(modifier) {
            Sky(moment, Modifier.matchParentSize().skyBackdropHost(backdrop), glide = false)
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp), content = content)
        }
    }
}

@Composable
private fun Caption(text: String) = PhoneText(text, 12f, FerbType.Medium, LocalSkyInk.current.low)

@Composable
fun ShowTopBars(moment: SkyMoment) = OnSky(moment, Modifier.width(412.dp)) {
    for (tab in PhoneTab.entries) TopBar(tab, Modifier.outset(16.dp))
}

@Composable
fun ShowMarksAndPills() = Column(Modifier.background(Ink.white).padding(16.dp).width(380.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
    Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
        MarkKind.entries.forEach { Mark(it) }
        Mark(MarkKind.RUNNING, 20.dp)
    }
    Pill("Unlock with fingerprint", icon = FerbIcons.fingerprint)
    Pill("Not now", kind = PillKind.GREY)
    Pill("Cancel", kind = PillKind.PLAIN, height = 44.dp)
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Pill("Watch live", Modifier.weight(1f), PillKind.GREY, FerbIcons.eye, 46.dp)
        Pill("Stop", Modifier.weight(1f), PillKind.BLACK, FerbIcons.stop, 46.dp)
    }
    Row(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
        SheetIcon(FerbIcons.terminal)
        SheetIcon(FerbIcons.key, asking = false)
        CloseButton()
        PhoneSwitch(true)
        PhoneSwitch(false)
    }
    FingerprintNote()
}

@Composable
fun ShowRows(moment: SkyMoment) = OnSky(moment, Modifier.width(412.dp)) {
    SkyLabel("Waiting for you")
    Card {
        CardRow("Renew the domain", "Asks before paying $12.99", leading = { Mark(MarkKind.ASKING) }, trailing = { Opens() })
        Divide()
        CardRow("Compare standing desks", "Comparing price and delivery", leading = { Mark(MarkKind.RUNNING) }, trailing = { Opens("2 of 3") })
        Divide()
        CardRow("Thesis draft v7.docx", "2.4 MB, edited today", leading = { FileIcon("Thesis draft v7.docx", 26.dp) }, trailing = { Opens() })
        Divide()
        CardRow("Stay connected", "Questions arrive while the app is closed", leading = { RowIcon(FerbIcons.bell) }, trailing = { PhoneSwitch(true) })
        Divide()
        CardRow("Forget this Mac", danger = true, leading = { RowIcon(FerbIcons.link, danger = true) })
    }
}

@Composable
fun ShowConversation(moment: SkyMoment) = OnSky(moment, Modifier.width(412.dp)) {
    OwnerBubble("What's left on my calendar today?")
    FerbBubble("Two things: the design review at 4, and a call with the bank at 6:30.")
    TypingDots(timeMillis = 250)
    TaskCard("On it. Three parts.", "2 of 3", (labChat[3] as com.ferb.design.screens.ChatItem.Task).parts)
    PhoneComposer(placeholder = "Add to this")
    PhoneComposer(typed = "Compare the three standing desks")
}

@Composable
fun ShowSheetPieces() = Column(Modifier.background(Ink.white).padding(16.dp).width(380.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
    ExactCommand("brew upgrade --cask figma")
    ExactItem("Northfield Desk Pro", "Delivers Tuesday", "$379")
    SecretCodeBoxes(4)
    SecretCodeBoxes(0)
    PairingCode("482719")
    PairStep(1, "On your Mac, open FERB and choose Pair a phone.")
    Row { PairAction(FerbIcons.keyboard, "Type code", Modifier.weight(1f)); PairAction(FerbIcons.laptop, "Get the Mac app", Modifier.weight(1f)); PairAction(FerbIcons.help, "Help", Modifier.weight(1f)) }
}

@Composable
fun ShowKeypad() = Box(Modifier.width(412.dp)) { SecretKeypad() }

@Composable
fun ShowQrFrame(moment: SkyMoment) = OnSky(moment, Modifier.width(412.dp)) { QrFrame() }

@Composable
fun ShowLinkPictures(moment: SkyMoment) = OnSky(moment, Modifier.width(412.dp)) {
    LinkKind.entries.forEach { LinkPicture(it); Spacer(Modifier.height(8.dp)) }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun ShowMarksAndFiles() = Column(Modifier.background(Tone.ink1).padding(16.dp).width(380.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
    Row(horizontalArrangement = Arrangement.spacedBy(16.dp), verticalAlignment = Alignment.CenterVertically) {
        FerbMark(84.dp); FerbMark(42.dp)
        LampDot(Lamp.amber, 8.dp); LampDot(Lamp.green, 8.dp); LampDot(Lamp.red, 8.dp); LampDot(Tone.text, 8.dp)
    }
    FlowRow(horizontalArrangement = Arrangement.spacedBy(14.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
        listOf("Lease agreement.pdf", "Thesis draft v7.docx", "Budget.xlsx", "Pitch.key", "Beach.heic", "Clip.mov", "Memo.m4a",
            "main.rs", "Backup.zip", "notes.md", "README").forEach { Box(Modifier.padding(start = 6.dp)) { FileIcon(it, 34.dp) } }
    }
}

/** The glass depths over the sky (Theme.swift GlassBackground): a panel with a card, a control, an inset and chips. */
@Composable
fun ShowGlass(moment: SkyMoment) = OnSky(moment, Modifier.width(412.dp).height(420.dp)) {
    GlassPanel(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(18.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            PhoneText("A panel over the sky", 17f, FerbType.Semibold, Tone.text)
            Box(Modifier.fillMaxWidth().glass(RoundedCornerShape(Radius.card), GlassLevel.CARD).padding(14.dp)) {
                PhoneText("A card inside it: fill1", 13f, FerbType.Medium, Tone.text)
            }
            Box(Modifier.fillMaxWidth().glass(RoundedCornerShape(Radius.button), GlassLevel.CONTROL).padding(10.dp)) {
                PhoneText("A control: fill2", 12.5f, FerbType.Medium, Tone.text)
            }
            Box(Modifier.fillMaxWidth().glass(RoundedCornerShape(Radius.list), GlassLevel.INSET).padding(10.dp)) {
                PhoneText("An inset: the well", 12.5f, color = Tone.textDim)
            }
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Chip("Asking", lamp = Lamp.amber)
                Chip("Done", lamp = Lamp.green)
                Chip("Couldn't do it", lamp = Lamp.red)
                Chip("Working", lamp = Tone.text)
            }
        }
    }
}

/** The day's skies at the lab's eight times, at one place. */
@Composable
fun ShowSkies(place: SkyPlace, month: Int = 11, day: Int = 14) = Column(Modifier.background(Color.Black).padding(8.dp)) {
    Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        for ((name, hour) in LAB_TIMES) {
            val m = labMoment(hour, place, month = month, day = day)
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                Sky(m, Modifier.size(100.dp, 222.dp), glide = false)
                PhoneText(name, 10f, color = Color.White)
            }
        }
    }
}

/**
 * Astra's eight presets laid out as sky/astra/ferb-sky-contact-sheet.png lays them out (560 by 350 tiles, 40 margin,
 * 24 apart, at 2x), so the two renders can be compared pixel for pixel. No labels.
 */
@Composable
fun ShowAstraPresets() = Box(Modifier.size(1196.dp, 464.dp).background(Color(0xFF11141B))) {
    AstraPhase.entries.forEachIndexed { i, phase ->
        val look = SkyFrameLook.of(AstraSky.fixed(phase))
        Canvas(Modifier.offset((20 + 292 * (i % 4)).dp, (20 + 218 * (i / 4)).dp).size(280.dp, 175.dp)) { drawSkyLook(look) }
    }
}

/** The season's layer over the sky: autumn leaves (Delhi, November), spring petals (Sydney), fireflies (a summer night). */
@Composable
fun ShowSeasons() = Row(Modifier.background(Color.Black).padding(4.dp), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
    val cases = listOf(
        Triple(labMoment(9.0), Season.AUTUMN, false),
        Triple(labMoment(9.0, SkyPlace.SYDNEY), Season.SPRING, false),
        Triple(labMoment(22.5, SkyPlace.LONDON, month = 7, day = 14), Season.SUMMER, true),
        Triple(labMoment(22.5, month = 8, day = 14), Season.MONSOON, true),
    )
    for ((m, season, night) in cases) {
        Box(Modifier.size(200.dp, 440.dp)) {
            Sky(m, Modifier.fillMaxSize(), glide = false)
            SeasonLayer(season, night, Modifier.fillMaxSize(), time = 1000.0)
        }
    }
}

/** Tonight's moon on four nights of one cycle (the moon is drawn at 0.8 across, 0.17 down). */
@Composable
fun ShowMoons() = Row(Modifier.background(Color.Black).padding(4.dp), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
    for (day in listOf(10, 17, 24, 30)) {
        Sky(labMoment(22.5, month = 11, day = day), Modifier.size(150.dp, 300.dp), glide = false)
    }
}

/** The tokens: the phone's ink, the warm ramp and lamps, the type sizes, radii, spacing and the motion numbers. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun ShowTokens() = Column(Modifier.background(Ink.white).padding(16.dp).width(412.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
    PhoneText("Ink (the phone's colours)", 15f, FerbType.Semibold)
    Swatches(listOf("black" to Ink.black, "white" to Ink.white, "detail" to Ink.detail, "faint" to Ink.faint, "fill" to Ink.fill,
        "line" to Ink.line, "darkFill" to Ink.darkFill, "onDark" to Ink.onDark, "amber" to Ink.amber, "red" to Ink.red))
    PhoneText("Tone and Lamp (the glass pieces)", 15f, FerbType.Semibold)
    Swatches(listOf("ink0" to Tone.ink0, "ink1" to Tone.ink1, "ink2" to Tone.ink2, "ink3" to Tone.ink3, "ink4" to Tone.ink4,
        "line" to Tone.line, "textFaint" to Tone.textFaint, "textDim" to Tone.textDim, "text" to Tone.text,
        "amber" to Lamp.amber, "green" to Lamp.green, "red" to Lamp.red, "ring" to LabColors.waitingRing, "keypad" to LabColors.keypad))
    PhoneText("Type (system face)", 15f, FerbType.Semibold)
    for ((size, name) in listOf(42f to "Greeting 42 medium", 28f to "Locked 28 semibold", 24f to "Page title 24 semibold",
        22f to "Sheet title 22 semibold", 16.5f to "Row title 16.5 semibold", 16f to "Body 16", 15.5f to "Explain 15.5",
        15f to "Label 15 semibold", 14f to "Detail 14")) {
        PhoneText(name, size, if (size >= 22f || size == 16.5f || size == 15f) FerbType.Semibold else FerbType.Regular, maxLines = 1)
    }
    PhoneText("Radii", 15f, FerbType.Semibold)
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        for ((name, r) in listOf("card" to Phone.cardCorner, "sheet" to Phone.sheetCorner, "task" to Phone.taskCorner,
            "code" to Phone.codeCorner, "bubble" to Phone.bubbleCorner, "part" to Phone.partCorner, "pill" to Phone.pillCorner,
            "icon" to Phone.sheetIconCorner, "box" to Phone.secretBoxCorner)) RadiusSample(name, r)
    }
    PhoneText("Space ${Space.all.joinToString(", ") { it.value.toInt().toString() }}", 14f, color = Ink.detail)
    PhoneText("Motion: press ${Motion.PRESS_SCALE} on a ${Motion.press.duration} s spring (bounce ${Motion.press.bounce}); sheets ${Motion.panel.duration} s, bounce ${Motion.panel.bounce}; screens ${Motion.screen.duration} s; the sky glides ${Motion.SKY_GLIDE_MS / 1000.0} s; the season fades ${Motion.SEASON_FADE_MS / 1000.0} s.",
        13f, color = Ink.detail)
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Swatches(colours: List<Pair<String, Color>>) {
    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        for ((name, c) in colours) {
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                Box(Modifier.size(52.dp, 36.dp).background(Ink.line, RoundedCornerShape(8.dp)).padding(1.dp).background(c, RoundedCornerShape(7.dp)))
                PhoneText(name, 10f, color = Ink.detail, maxLines = 1)
            }
        }
    }
}

@Composable
private fun RadiusSample(name: String, r: Dp) {
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Box(Modifier.size(64.dp, 48.dp).background(Ink.fill, RoundedCornerShape(r)))
        PhoneText("$name ${r.value}", 10f, color = Ink.detail, maxLines = 1)
    }
}

/** The showcases by name, as the gallery lists them and the screenshot tests take them. */
val SHOWCASES: List<Pair<String, @Composable () -> Unit>> = listOf(
    "tokens" to { ShowTokens() },
    "topbar" to { ShowTopBars(labMoment(12.5)) },
    "marks-pills" to { ShowMarksAndPills() },
    "rows" to { ShowRows(labMoment(12.5)) },
    "conversation" to { ShowConversation(labMoment(12.5)) },
    "sheet-pieces" to { ShowSheetPieces() },
    "keypad" to { ShowKeypad() },
    "qr-frame" to { ShowQrFrame(labMoment(12.5)) },
    "link-pictures" to { ShowLinkPictures(labMoment(17.4)) },
    "marks-files" to { ShowMarksAndFiles() },
    "glass" to { ShowGlass(labMoment(17.4)) },
    "skies-delhi" to { ShowSkies(SkyPlace.DELHI) },
    "skies-london-june" to { ShowSkies(SkyPlace.LONDON, month = 6, day = 21) },
    "skies-sydney" to { ShowSkies(SkyPlace.SYDNEY) },
    "seasons" to { ShowSeasons() },
    "astra-presets" to { ShowAstraPresets() },
    "moons" to { ShowMoons() },
)
