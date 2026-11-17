package com.ferb.gallery

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.dp
import androidx.core.view.WindowCompat
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.components.Card
import com.ferb.design.components.CardRow
import com.ferb.design.components.Divide
import com.ferb.design.components.Opens
import com.ferb.design.components.PhoneText
import com.ferb.design.components.pressable
import com.ferb.design.preview.LAB_TIMES
import com.ferb.design.preview.LabScreen
import com.ferb.design.preview.LabScreenView
import com.ferb.design.preview.SHOWCASES
import com.ferb.design.preview.labHomeTexts
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.sky.SkyInk
import com.ferb.design.sky.SkyMoment
import com.ferb.design.sky.SkyPlace
import java.time.ZoneId

/**
 * The gallery: every token, component and lab screen from :design with invented data, for the owner to look at on
 * a phone or the emulator. Debug only; it asks for no permission (the place is picked here, never read).
 */
class GalleryActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent { Gallery() }
    }
}

private sealed interface Page {
    data object Index : Page
    data class Screen(val screen: LabScreen) : Page
    data class Showcase(val name: String) : Page
    data object Motion : Page
    data object Chat : Page
}

/** The places the lab offers, and the phone's own time zone's place (PHN-420's fallback). */
private val places = listOf(
    "This time zone" to SkyPlace.fromZone(ZoneId.systemDefault()),
    "Delhi" to SkyPlace.DELHI, "London" to SkyPlace.LONDON, "San Francisco" to SkyPlace.SAN_FRANCISCO, "Sydney" to SkyPlace.SYDNEY,
)

@Composable
private fun Gallery() {
    var page by remember { mutableStateOf<Page>(Page.Index) }
    var placeIndex by rememberSaveable { mutableStateOf(1) }
    /** Local hour at the place; null follows the real clock. */
    var hour by rememberSaveable { mutableStateOf<Double?>(null) }
    var playing by rememberSaveable { mutableStateOf(false) }
    val place = places[placeIndex].second

    var now by remember { mutableStateOf(System.currentTimeMillis() / 1000.0) }
    LaunchedEffect(Unit) { while (true) { kotlinx.coroutines.delay(60_000); now = System.currentTimeMillis() / 1000.0 } }
    if (playing) {
        // The lab's Play a day: the whole day in twenty seconds.
        LaunchedEffect(Unit) {
            var last = withFrameMillis { it }
            while (true) withFrameMillis { t ->
                val start = hour ?: localHour(place, now)
                hour = (start + (t - last) / 1000.0 * 1.2) % 24
                last = t
            }
        }
    }
    val moment = hour?.let { h ->
        val today = java.time.Instant.ofEpochSecond(now.toLong()).atZone(place.zone).toLocalDate()
        SkyMoment.at(place, today.year, today.monthValue, today.dayOfMonth, h)
    } ?: SkyMoment(now, place)

    BackHandler(enabled = page != Page.Index) { page = Page.Index }
    when (val p = page) {
        Page.Index -> {
            SystemBars(SkyInk.WHITE)
            Index(
                placeIndex, { placeIndex = it }, hour, { hour = it; playing = false }, playing, { playing = !playing },
                moment, onOpen = { page = it },
            )
        }
        is Page.Screen -> {
            SystemBars(moment.ink)
            val bars = WindowInsets.systemBars.asPaddingValues()
            val insets = PhoneInsets(bars.calculateTopPadding(), WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding())
            LabScreenView(p.screen, moment, labHomeTexts(moment), insets, chrome = false, glide = !playing)
        }
        Page.Chat -> {
            SystemBars(moment.ink)
            val bars = WindowInsets.systemBars.asPaddingValues()
            ChatDemo(moment, PhoneInsets(bars.calculateTopPadding(), WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()))
        }
        Page.Motion -> {
            SystemBars(moment.ink)
            val bars = WindowInsets.systemBars.asPaddingValues()
            MotionDemo(moment, PhoneInsets(bars.calculateTopPadding(), WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()))
        }
        is Page.Showcase -> {
            SystemBars(SkyInk.WHITE)
            Box(
                Modifier.fillMaxSize().background(Ink.black).padding(WindowInsets.systemBars.asPaddingValues())
                    .verticalScroll(rememberScrollState()).horizontalScroll(rememberScrollState()),
            ) { SHOWCASES.first { it.first == p.name }.second() }
        }
    }
}

private fun localHour(place: SkyPlace, epochSeconds: Double): Double {
    val t = java.time.Instant.ofEpochSecond(epochSeconds.toLong()).atZone(place.zone)
    return t.hour + t.minute / 60.0
}

/** Status bar icons in the sky's ink: dark over a light sky, light over a dark one. */
@Composable
private fun SystemBars(ink: SkyInk) {
    val view = LocalView.current
    val activity = view.context as? ComponentActivity ?: return
    SideEffect {
        WindowCompat.getInsetsController(activity.window, view).apply {
            isAppearanceLightStatusBars = ink.topIsDark
            isAppearanceLightNavigationBars = ink.low == androidx.compose.ui.graphics.Color.Black
        }
    }
}

@Composable
private fun Index(
    placeIndex: Int,
    onPlace: (Int) -> Unit,
    hour: Double?,
    onHour: (Double?) -> Unit,
    playing: Boolean,
    onPlay: () -> Unit,
    moment: SkyMoment,
    onOpen: (Page) -> Unit,
) {
    Column(
        Modifier.fillMaxSize().background(Ink.black).verticalScroll(rememberScrollState())
            .padding(WindowInsets.statusBars.asPaddingValues()).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        PhoneText("FERB design gallery", 28f, FerbType.Semibold, Ink.white)
        PhoneText("Slice S10: the glass lab's phone design in Compose. Everything shown is invented.", 14f, color = Ink.onDark)

        PhoneText("Place", 15f, FerbType.Semibold, Ink.white, Modifier.padding(top = 10.dp))
        Chips(places.map { it.first }, placeIndex, onPlace)
        PhoneText("Time", 15f, FerbType.Semibold, Ink.white)
        val times = listOf("Now" to null) + LAB_TIMES.map { it.first to it.second }
        Chips(times.map { it.first } + listOf(if (playing) "Stop the day" else "Play a day"), times.indexOfFirst { it.second == hour }.takeIf { !playing } ?: -1) { i ->
            if (i == times.size) onPlay() else onHour(times[i].second)
        }
        val t = moment.local
        PhoneText(
            "%02d:%02d there; the sun at %.1f degrees, %s.".format(t.hour, t.minute, moment.sun.elevation, moment.season.name.lowercase()),
            13f, color = Ink.onDark,
        )

        PhoneText("Screens", 15f, FerbType.Semibold, Ink.white, Modifier.padding(top = 10.dp))
        Card {
            LabScreen.entries.forEachIndexed { i, s ->
                if (i > 0) Divide()
                CardRow(s.title, s.note, onClick = { onOpen(Page.Screen(s)) }, trailing = { Opens() })
            }
        }
        PhoneText("Motion", 15f, FerbType.Semibold, Ink.white, Modifier.padding(top = 10.dp))
        Card { CardRow("The chat", "Waiting, the stored chat with its days, FERB thinking, a reply followed as it grows (kb PHN-626..628)", onClick = { onOpen(Page.Chat) }, trailing = { Opens() }) }
        Card { CardRow("Tabs and sheets", "The app's motion (kb PHN-620..624) over the lab's pages: tap the tabs, the rows, the Mac", onClick = { onOpen(Page.Motion) }, trailing = { Opens() }) }
        PhoneText("Tokens and components", 15f, FerbType.Semibold, Ink.white, Modifier.padding(top = 10.dp))
        Card {
            SHOWCASES.forEachIndexed { i, (name, _) ->
                if (i > 0) Divide()
                CardRow(name, onClick = { onOpen(Page.Showcase(name)) }, trailing = { Opens() })
            }
        }
        Spacer(Modifier.height(WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding() + 16.dp))
    }
}

@Composable
private fun Chips(labels: List<String>, chosen: Int, onPick: (Int) -> Unit) {
    Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        labels.forEachIndexed { i, label ->
            val on = i == chosen
            Box(
                Modifier.pressable({ onPick(i) }).background(if (on) Ink.white else Ink.darkFill, CircleShape)
                    .padding(horizontal = 14.dp, vertical = 10.dp),
                contentAlignment = Alignment.Center,
            ) { PhoneText(label, 14f, FerbType.Medium, if (on) Ink.black else Ink.white, maxLines = 1) }
        }
    }
}
