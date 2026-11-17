package com.ferb.app

import java.io.File
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * A control never does nothing (kb PHN-656), as far as the app's source shows it: every switch the app draws is given
 * what it changes, no tappable row or button is given an empty action, and every job a tap can start that runs one at
 * a time says so when tapped again (`busy(..., tap = true)`). What it can't see: an action that returns early inside
 * its own code; those are each one's tests (ViewModelTest, FeaturesTest).
 */
class ControlsActTest {
    private val src = File("src/main/kotlin/com/ferb/app")

    // decision: PHN-656
    @Test
    fun every_switch_has_what_it_changes_and_no_tap_is_empty() {
        val problems = ArrayList<String>()
        src.walkTopDown().filter { it.extension == "kt" }.forEach { f ->
            f.readLines().forEachIndexed { i, line ->
                val at = "${f.name}:${i + 1}"
                // PhoneSwitch(on) alone draws a switch that can't be changed
                Regex("""PhoneSwitch\(([^(),]+)\)""").find(line)?.let { problems += "$at a switch with nothing to change: ${it.value}" }
                if (Regex("""onClick\s*=\s*\{\s*}""").containsMatchIn(line)) problems += "$at an empty tap action"
                if (Regex("""pressable\(\s*\{\s*}\s*\)""").containsMatchIn(line)) problems += "$at an empty pressable"
            }
        }
        assertTrue(problems.joinToString("\n"), problems.isEmpty())
    }

    // decision: PHN-656
    @Test
    fun every_one_at_a_time_job_a_tap_starts_says_so_when_tapped_again() {
        val vm = File(src, "FerbViewModel.kt").readText()
        // the actions the screens call that run one at a time
        for (name in listOf("openSettings", "setSetting", "registerKeys", "unlockMac", "forget", "stopEverything")) {
            val body = Regex("""fun $name\([^)]*\)[^=]*=\s*busy\(([^)]*)\)""").find(vm) ?: continue
            assertTrue("$name: ${body.value}", body.groupValues[1].contains("tap = true"))
        }
    }
}
