package com.ferb.design

import com.ferb.design.preview.labMinutesLeft
import com.ferb.design.preview.labRelay
import org.junit.Assert.assertEquals
import org.junit.Test

class LabWordsTest {
    /** MobileScreens.swift SkyNow.sunLine: "\(left / 60) h \(left % 60) min" from an hour, "\(left) min" below it. */
    @Test
    fun timeLeftIsWrittenAsTheLabWritesIt() {
        assertEquals("0 min", labMinutesLeft(0))
        assertEquals("45 min", labMinutesLeft(45))
        assertEquals("1 h 0 min", labMinutesLeft(60))
        assertEquals("5 h 0 min", labMinutesLeft(300))
        assertEquals("2 h 12 min", labMinutesLeft(132))
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
