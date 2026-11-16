package com.ferb.design

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.Dp
import com.ferb.design.sky.AstraSky
import com.ferb.design.sky.LabSky
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.roundToInt

/**
 * The Kotlin tokens against tools/lab_numbers.py's copy of the glass lab's numbers (lab-numbers.json, with the lab
 * commit it was read at). When the lab changes: rerun the script, then this test names every value to bring across.
 */
class LabNumbersTest {
    private val lab = MiniJson.resource("lab/lab-numbers.json").obj()

    private fun Color.hex(): String {
        val r = (red * 255).roundToInt()
        val g = (green * 255).roundToInt()
        val b = (blue * 255).roundToInt()
        return "#%02X%02X%02X".format(r, g, b)
    }

    private fun checkColours(section: String, kotlin: Map<String, Color>) {
        val want = lab[section].obj()
        assertEquals("$section: the same names", want.keys, kotlin.keys)
        for ((name, v) in want) {
            val (hex, alpha) = v.list()
            val c = kotlin.getValue(name)
            assertEquals("$section.$name colour", hex, c.hex())
            assertEquals("$section.$name alpha", alpha.num(), c.alpha.toDouble(), 0.002)
        }
    }

    // decision: PHN-416, PHN-457
    @Test
    fun thePhonesInkIsTheLabsInk() = checkColours("ink", mapOf(
        "black" to Ink.black, "white" to Ink.white, "detail" to Ink.detail, "faint" to Ink.faint, "fill" to Ink.fill,
        "line" to Ink.line, "darkFill" to Ink.darkFill, "onDark" to Ink.onDark, "amber" to Ink.amber, "red" to Ink.red,
    ))

    @Test
    fun theWarmRampAndLampsAreTheLabs() {
        checkColours("tone", mapOf(
            "ink0" to Tone.ink0, "ink1" to Tone.ink1, "ink2" to Tone.ink2, "ink3" to Tone.ink3, "ink4" to Tone.ink4,
            "line" to Tone.line, "text" to Tone.text, "textDim" to Tone.textDim, "textFaint" to Tone.textFaint,
            "fill1" to Tone.fill1, "fill2" to Tone.fill2, "fill3" to Tone.fill3, "well" to Tone.well, "hairline" to Tone.hairline,
        ))
        checkColours("lamp", mapOf("amber" to Lamp.amber, "green" to Lamp.green, "red" to Lamp.red))
    }

    /** Every colour literal in the lab's phone files is drawn here, or listed as not drawn on Android with a reason. */
    // decision: PHN-416, PHN-412
    @Test
    fun everyColourTheLabsPhoneDrawsIsAccountedFor() {
        val ours = HashSet<String>()
        listOf(Ink, Tone, Lamp, LabColors).forEach { holder ->
            holder::class.java.declaredMethods.filter { it.parameterCount == 0 && it.name.startsWith("get") }.forEach { m ->
                m.isAccessible = true
                when (val v = runCatching { m.invoke(holder) }.getOrNull()) {
                    is Long -> ours += Color(v.toULong()).hex()
                    is Color -> ours += v.hex()
                    is Triple<*, *, *> -> listOf(v.first, v.second, v.third).forEach { (it as? Color)?.let { c -> ours += c.hex() } }
                    is List<*> -> v.forEach { t -> (t as? Triple<*, *, *>)?.let { listOf(it.first, it.second, it.third).forEach { c -> (c as? Color)?.let { ours += it.hex() } } } }
                }
            }
        }
        // Compose stores a Color as an inline Long; read the fields too.
        listOf(Ink, Tone, Lamp, LabColors).forEach { holder ->
            holder::class.java.declaredFields.filter { it.type == Long::class.javaPrimitiveType }.forEach { f ->
                f.isAccessible = true
                ours += Color((f.get(holder) as Long).toULong()).hex()
            }
        }
        LabSky.SKIES.values.forEach { (stops, glow) -> (stops.toList() + glow).forEach { ours += "#%06X".format(it) } }
        AstraSky.frames.forEach { (_, _, p) ->
            p.colours.forEach { ours += Color(it.r.toFloat(), it.g.toFloat(), it.b.toFloat()).hex() }
            listOf(p.haze, p.glow, p.shade).forEach { f -> ours += Color(f.colour.r.toFloat(), f.colour.g.toFloat(), f.colour.b.toFloat()).hex() }
        }
        ours += "#FFFFFF"
        ours += "#000000"
        val missing = lab["literalColours"].obj().filterKeys { it !in ours && it !in LabColors.notOnAndroid }
        assertTrue("lab colours with no Kotlin counterpart: $missing", missing.isEmpty())
    }

    @Test
    fun theAndroidFrameIsTheLabs() {
        val f = lab["androidFrame"].obj()
        assertEquals((f["top"] as String).toFloat(), Phone.statusBar.value)
        assertEquals((f["bottom"] as String).toFloat(), Phone.gestureArea.value)
        assertEquals((f["corner"] as String).toFloat(), Phone.frameCorner.value)
        assertEquals((f["bezel"] as String).toFloat(), Phone.frameBezel.value)
    }

    @Test
    fun radiiAndSpacingAreTheBooks() {
        val r = lab["radius"].obj()
        val ours: Map<String, Dp> = mapOf(
            "chip" to Radius.chip, "key" to Radius.key, "buttonCompact" to Radius.buttonCompact, "button" to Radius.button,
            "list" to Radius.list, "card" to Radius.card, "bubble" to Radius.bubble, "window" to Radius.window,
            "notchBottom" to Radius.notchBottom, "phoneSheet" to Radius.phoneSheet,
        )
        assertEquals(r.keys, ours.keys)
        r.forEach { (k, v) -> assertEquals("radius $k", v.num(), ours.getValue(k).value.toDouble(), 1e-6) }
        assertEquals(lab["space"].nums(), Space.all.map { it.value.toDouble() })
    }

    // decision: PHN-409
    @Test
    fun theFaceIsTheSystemsAndTrackingFollowsTheLab() {
        assertEquals(FontFamily.Default, FerbType.style(16f).fontFamily)
        assertEquals(-0.4f, FerbType.style(20f).letterSpacing.value)
        assertEquals(-0.1f, FerbType.style(19.9f).letterSpacing.value)
        val type = lab["type"].obj()
        assertEquals(-0.3, type["title"].obj()["tracking"].num(), 0.0)
        assertEquals(-0.3f, FerbType.ink(22f).letterSpacing.value)
        assertEquals(0f, FerbType.ink(13f).letterSpacing.value)
    }

    @Test
    fun theFileBadgesAreTheBooks() {
        val b = lab["fileBadge"].obj()
        assertEquals(b["pdf"], LabColors.badgePdf.hex())
        assertEquals(b["document"], LabColors.badgeDocument.hex())
        assertEquals(b["sheet"], LabColors.badgeSheet.hex())
        assertEquals(b["slides"], LabColors.badgeSlides.hex())
        assertEquals(b["other"], LabColors.badgeOther.hex())
        assertEquals(b["paper"], LabColors.paper.hex())
        assertEquals(b["fold"], LabColors.fold.hex())
    }
}
