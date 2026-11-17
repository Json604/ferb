package com.ferb.app

import com.ferb.app.model.PhoneApi
import kotlin.coroutines.Continuation
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The shape that keeps the main thread out of the phone library (kb PHN-682): the owner's phone froze because one
 * plain function of [PhoneApi] (`lastStatus`) entered the library on the thread that called it, the main thread, and
 * waited there behind a sync. Everything of [PhoneApi] that reaches the library is a `suspend` function (FerbPhone
 * runs it on its IO dispatcher); a plain function is allowed only when it is named here with why it cannot wait.
 */
class PhoneApiShapeTest {
    /** Plain functions and why each never waits. */
    private val plain = mapOf(
        "setOpen" to "hands its word to the link's own lane (FerbPhone.tellLink) and returns",
        "retryLink" to "hands its word to the link's own lane (FerbPhone.tellLink) and returns",
    )

    // decision: PHN-682
    @Test
    fun everything_that_reaches_the_library_is_a_suspend_function() {
        val offenders = ArrayList<String>()
        val seen = HashSet<String>()
        for (m in PhoneApi::class.java.declaredMethods) {
            // the three flows are read, never called into the library
            if (m.name in setOf("getState", "getLink", "getChanges") || m.isSynthetic) continue
            val suspends = m.parameterTypes.lastOrNull() == Continuation::class.java
            if (suspends) continue
            seen += m.name
            if (m.name !in plain) offenders += m.name
        }
        assertTrue("PhoneApi has plain (non-suspend) functions that can run on the main thread: $offenders. " +
            "Make each a suspend function over FerbPhone.call, or name it in this test with why it cannot wait.", offenders.isEmpty())
        assertEquals("every plain function named here still exists", plain.keys, seen)
    }
}
