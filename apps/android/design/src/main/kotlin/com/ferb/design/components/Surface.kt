package com.ferb.design.components

import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import com.ferb.design.Ink

// FERB's surface (kb PHN-734; the owner: "why not maintain its dark color??/ it consisten - like in mac app"):
// everything FERB shows in the chat sits on its bubble's black with the on-dark inks; the owner's own messages and the
// sheets stay white. The pieces read it ([LocalOnFerb]), so no card picks a colour of its own.

/** Whether what is drawn here is FERB's: its dark surface. False: the lab's white card. */
val LocalOnFerb = staticCompositionLocalOf { false }

/** Draws [content] on FERB's dark surface. */
@Composable
fun FerbSurface(content: @Composable () -> Unit) = CompositionLocalProvider(LocalOnFerb provides true, content = content)

/**
 * A card's inks: the lab's white card, or FERB's dark surface. Text contrast on black: white 21:1, secondary #9A9A9A
 * 7.4:1, faint #8E8E8E 6.4:1, amber #FFB020 11:1, red #FF6B5E 6.9:1, link #9CC7FF 12:1.
 */
@Immutable
class CardInk(val onFerb: Boolean) {
    val card: Color get() = if (onFerb) Ink.black else Ink.white
    val primary: Color get() = if (onFerb) Ink.white else Ink.black
    val secondary: Color get() = if (onFerb) Ink.onDark else Ink.detail
    val faint: Color get() = if (onFerb) Color(0xFF8E8E8E) else Ink.faint
    val fill: Color get() = if (onFerb) Ink.darkFill else Ink.fill
    val line: Color get() = if (onFerb) Color.White.copy(alpha = 0.12f) else Ink.line
    val red: Color get() = if (onFerb) Color(0xFFFF6B5E) else Ink.red
    val link: Color get() = if (onFerb) Color(0xFF9CC7FF) else Color(0xFF1F6FEB)
    val amberText: Color get() = if (onFerb) Ink.amber else Color(0xFFB86E00)

    companion object {
        val white = CardInk(false)
        val ferb = CardInk(true)
        @Composable fun current(): CardInk = if (LocalOnFerb.current) ferb else white
    }
}

/** The colour a card part's words take (FERB.app's row content). */
enum class PartTone { DIM, RED, AMBER, BLUE;
    fun color(ink: CardInk): Color = when (this) { DIM -> ink.secondary; RED -> ink.red; AMBER -> ink.amberText; BLUE -> ink.link }
}

/** A line under a part's words: a label in its tone ("Couldn't confirm: ") and the words after it. */
@Immutable
data class PartNote(val text: String, val label: String? = null, val tone: PartTone = PartTone.DIM)
