package com.ferb.design

import androidx.compose.ui.graphics.Color

// Colours. The phone's colours are Mobile.swift's `Ink` (black, white and neutral greys only where something is
// secondary): on the phone they are the authority over tokens.json's warm ramp, which is the Mac's (Theme.swift Tone,
// Lamp) and is kept here for the glass surfaces. Every colour literal in the lab's phone files is here or in
// LabColors.notOnAndroid with its reason; LabNumbersTest checks both against tools/lab_numbers.py's copy.

internal fun hex(rgb: Long, alpha: Float = 1f): Color = Color(0xFF000000 or rgb).copy(alpha = alpha)

/** Mobile.swift `enum Ink`: the phone's palette. */
object Ink {
    val black = Color.Black
    val white = Color.White
    /** Secondary text on white. */
    val detail = hex(0x666666)
    /** The quietest text on white. */
    val faint = hex(0x8E8E8E)
    /** A quiet surface on white. */
    val fill = hex(0xF2F2F2)
    /** A line on white. */
    val line = hex(0xE8E8E8)
    /** A quiet surface on black. */
    val darkFill = hex(0x2C2C2E)
    /** Secondary text on black. */
    val onDark = hex(0x9A9A9A)
    /** FERB wants you. */
    val amber = hex(0xFFB020)
    /** Stop, forget, failed. */
    val red = hex(0xE5281B)
}

/** Theme.swift `enum Tone`: the Mac's warm ramp (OKLCH hue 80). On the phone only the glass surfaces use it. */
object Tone {
    val ink0 = hex(0x080706)
    val ink1 = hex(0x110F0D)
    val ink2 = hex(0x1B1916)
    val ink3 = hex(0x272521)
    val ink4 = hex(0x383531)
    val line = hex(0x45423E)
    val text = hex(0xF4EFE7)
    val textDim = hex(0xC1BDB7)
    val textFaint = hex(0x95928B)
    val fill1 = hex(0xF4EFE7, 0.055f)
    val fill2 = hex(0xF4EFE7, 0.095f)
    val fill3 = hex(0xF4EFE7, 0.15f)
    val well = Color.Black.copy(alpha = 0.28f)
    val hairline = hex(0xF4EFE7, 0.08f)
}

/** Theme.swift `enum Lamp`: the hues FERB owns, as small indicators. The phone uses Ink.amber and Ink.red instead. */
object Lamp {
    val amber = hex(0xF5AE4B)
    val green = hex(0x60DB89)
    val red = hex(0xF66D62)
}

/** The other literal colours the phone scene draws, each named by where the lab uses it. */
object LabColors {
    /** MobileScreens.swift Mark(.waiting): the dashed ring. */
    val waitingRing = hex(0xBDBDBD)
    /** Mobile.swift Sheet: the grabber. */
    val grabber = hex(0xD1D1D1)
    /** Mobile.swift Switch: the track when off. */
    val switchOff = hex(0xE0E0E0)
    /** MobileScreens.swift SecretSheet: the keypad's ground. */
    val keypad = hex(0xD1D1D6)
    /** Sky.swift MoonDisc: the dark side and the lit side. */
    val moonDark = hex(0x2A2C33)
    val moonLit = hex(0xEDE8DC)
    /** Sky.swift SunDisc: the core high in the sky (Lamp.amber near the horizon). */
    val sunHigh = hex(0xFFF6E5)
    /** Mobile.swift Device: the Android frame's body and edge (previews only). */
    val androidBody = hex(0x0B0B0C)
    val androidEdge = hex(0x2C2C2E)
    /** Conversation.swift Biometric: the Android fingerprint glyph. */
    val androidBiometric = hex(0xA8C7FA)
    /** Data.swift FileIcon: paper, fold and the ink of its hint. */
    val paper = hex(0xF7F5F0)
    val fold = hex(0xD9D5CC)
    val paperInk = hex(0x1D1D1F)
    /** Data.swift FileKind.badge. */
    val badgePdf = hex(0xC8493F)
    val badgeDocument = hex(0x3F78C9)
    val badgeSheet = hex(0x2F8F5B)
    val badgeSlides = hex(0xC97A2B)
    val badgeOther = hex(0x5B5852)
    /** Weather.swift firefly: its glow and body. */
    val fireflyGlow = hex(0xFFE58A)
    val fireflyBody = hex(0x2A2418)

    /** Weather.swift seasonal(): autumn's six (light, mid, dark) leaf tones and spring's petal. */
    val autumnLeaves: List<Triple<Color, Color, Color>> = listOf(
        Triple(hex(0xE39A3F), hex(0xB5532A), hex(0x6B2F14)), Triple(hex(0xEDC155), hex(0xC48A2E), hex(0x7A5218)),
        Triple(hex(0xCF5E31), hex(0x93321B), hex(0x4E1A0C)), Triple(hex(0xD9A04A), hex(0x9D5E28), hex(0x5A3312)),
        Triple(hex(0xB9472C), hex(0x7C2616), hex(0x3F120A)), Triple(hex(0xC9A64A), hex(0x8E7A2C), hex(0x4F4517)),
    )
    val springPetal: Triple<Color, Color, Color> = Triple(hex(0xFFF6F8), hex(0xF9D0DA), hex(0xE58CA5))

    /**
     * Lab colours Android does not draw, with the reason. Weather is paused (PHN-412); the iPhone frame is the other
     * phone's hardware.
     */
    val notOnAndroid: Map<String, String> = mapOf(
        "#A3ABB5" to "Weather.swift weathered(): the day veil under cloud, fog and storm; weather is paused (PHN-412)",
        "#1A1E24" to "Weather.swift weathered(): the night veil under weather; paused (PHN-412)",
        "#CFE0FF" to "Weather.swift lightning(): storms are weather; paused (PHN-412)",
        "#1E1D1B" to "Mobile.swift Device: the iPhone frame's body (iPhone hardware, not drawn on Android)",
        "#6E6A63" to "Mobile.swift Device: the iPhone frame's edge (iPhone hardware, not drawn on Android)",
    )
}
