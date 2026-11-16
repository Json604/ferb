package com.ferb.design

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.LineHeightStyle
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.sp

// Type. The phone uses the system face (the owner chose Android's system font, PHN-409) at the sizes the lab's
// phone code gives in points; a point is a dp here and text is in sp so the owner's font size scales it.

object FerbType {
    /**
     * Mobile.swift `View.type(size, weight, color)`: tracking -0.4 at 20 and above, -0.1 below. [lineSpacing] is
     * SwiftUI's `.lineSpacing`: extra points between lines, added to the face's own line height.
     */
    fun style(size: Float, weight: FontWeight = FontWeight.Normal, color: Color = Ink.black, lineSpacing: Float = 0f): TextStyle =
        TextStyle(
            fontFamily = FontFamily.Default,
            fontSize = size.sp,
            fontWeight = weight,
            color = color,
            letterSpacing = (if (size >= 20f) -0.4f else -0.1f).sp,
            lineHeight = if (lineSpacing > 0f) (size * NATURAL_LINE + lineSpacing).sp else TextUnit.Unspecified,
            lineHeightStyle = if (lineSpacing > 0f) LineHeightStyle(LineHeightStyle.Alignment.Center, LineHeightStyle.Trim.None) else null,
        )

    /** Theme.swift `Text.ink(size, weight, tone)`, the Mac's ramp (the glass pieces): tracking -0.3 from 17 up. */
    fun ink(size: Float, weight: FontWeight = FontWeight.Normal, color: Color = Tone.text): TextStyle =
        TextStyle(fontFamily = FontFamily.Default, fontSize = size.sp, fontWeight = weight, color = color,
            letterSpacing = if (size >= 17f) (-0.3f).sp else 0.sp)

    /** The system face's natural line height per point of size (Roboto: ascent 0.927 + descent 0.244). */
    const val NATURAL_LINE = 1.171f

    // SwiftUI's weights by number: .regular 400, .medium 500, .semibold 600, .bold 700.
    val Regular = FontWeight.Normal
    val Medium = FontWeight.Medium
    val Semibold = FontWeight.SemiBold
    val Bold = FontWeight.Bold

    /** The roles the phone screens use, by the lab's own numbers (MobileScreens.swift). */
    object Role {
        /** HomePage greeting. */
        val greeting = 42f
        /** HomePage subline. */
        val subline = 17f
        /** CodesPage digits. */
        val codeDigits = 42f
        /** UnlockPage title. */
        val lockTitle = 28f
        /** LinkPicture's Mac name; PairPage's title. */
        val pageTitle = 24f
        /** Sheet titles (question, relay, profile); CodesPage title. */
        val sheetTitle = 22f
        /** SecretSheet title. */
        val secretTitle = 19f
        /** CardRow title. */
        val rowTitle = 16.5f
        /** Bubbles, composer text, Pill. */
        val body = 16f
        /** Long explanations in sheets and pages. */
        val explain = 15.5f
        /** SkyLabel, tab title, trailing values. */
        val label = 15f
        /** Details, notes. */
        val detail = 14f
    }
}
