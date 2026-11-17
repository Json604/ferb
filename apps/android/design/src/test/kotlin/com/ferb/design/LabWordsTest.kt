package com.ferb.design

import com.ferb.design.preview.labMinutesLeft
import com.ferb.design.preview.labRelay
import org.junit.Assert.assertEquals
import org.junit.Test

class LabWordsTest {
    /**
     * A time left: "45 min" below an hour, "5 h" for whole hours (the owner's "5h", not the lab's "5 h 0 min"), "5 h
     * 20 min" otherwise. The table is shared with the iPhone's WordsTests, so the two phones can't drift.
     */
    // decision: PHN-595
    @Test
    fun timeLeftDropsZeroMinutesAsTheOwnerChose() {
        val cases = (MiniJson.resource("words/time-left.json") as Map<*, *>)["cases"] as List<*>
        check(cases.size >= 6)
        for (c in cases) {
            val (minutes, words) = c as List<*>
            assertEquals("$minutes", words, labMinutesLeft((minutes as Number).toInt()))
        }
    }

    /** The shared relay carries text, questions and Stop only; the sheet must not promise anything else there. */
    // decision: PHN-410, PHN-444
    @Test
    fun theRelaySheetOffersOnlyWhatTheSharedRelayCarries() {
        val how = labRelay.works.toMap()
        assertEquals("Always", how["Messages, questions, answers, Stop"])
        assertEquals("Not on the shared relay", how["Files and pictures"])
        assertEquals("Not on the shared relay", how["Watch live"])
    }
}
