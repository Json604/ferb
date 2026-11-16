package com.ferb.core

import com.ferb.mobile.AnswerInput
import com.ferb.mobile.Change
import com.ferb.mobile.Clock
import com.ferb.mobile.FerbException
import com.ferb.mobile.Ids
import com.ferb.mobile.Item
import com.ferb.mobile.LinkPath
import com.ferb.mobile.Listener
import com.ferb.mobile.Mobile
import com.ferb.mobile.MobileConfig
import com.ferb.mobile.OlderState
import com.ferb.mobile.SecretInput
import com.ferb.mobile.Signer
import com.ferb.mobile.libraryVersion
import java.nio.file.Files
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicInteger
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/**
 * The generated bindings against the host library (build/rust/debug, built by :core:cargoHost), every kind of call
 * across JNA and UniFFI's scaffolding as the phone's will: moved from slice S8's scratch check (journal 112) without
 * its two panic hooks, which need the library's `testing` feature (the Rust unit tests hold them).
 */
// decision: PHN-495 (the library these tests load is the one the Gradle Rust tasks build and generate bindings for)
class BindingsTest {
    private class FixedClock : Clock { override fun nowIso() = "2026-01-02T10:00:00+00:00" }
    private class Counted : Ids { val n = AtomicInteger(0); override fun newId() = "sub-${n.incrementAndGet()}" }
    private class Heard : Listener { val changes = CopyOnWriteArrayList<Change>(); override fun changed(change: Change) { changes.add(change) } }
    private class Throws : Listener { override fun changed(change: Change) = throw RuntimeException("the app's listener failed") }
    private class Signs : Signer { val asked = CopyOnWriteArrayList<String>(); override fun sign(payload: String): String { asked.add(payload); return "SIG" } }

    private fun open(listener: Listener): Mobile {
        val dir = Files.createTempDirectory("ferb-core-bindings")
        return Mobile.open(dir.resolve("phone.db").toString(), ByteArray(32) { 7 }, MobileConfig("phone-1", 0, null, null, null), FixedClock(), Counted(), listener)
    }

    // decision: PHN-446
    // decision: PHN-492
    @Test
    fun every_kind_of_call_crosses_the_bindings() {
        assertEquals("0.28.0", libraryVersion())
        val heard = Heard()
        val mac = FakeMac()
        val m = open(heard)
        m.connectTransport(mac)
        m.linkEvent("""{"type":"state","phase":"online","tier":"direct"}""")
        m.sync()
        assertEquals("c1", m.currentConversation())
        val view = m.transcript("c1")
        assertEquals("Invented question", (view.items.single() as Item.Turn).turn.words?.text)
        assertEquals(OlderState.Complete, view.older)
        assertEquals(LinkPath.DIRECT, m.linkState().path)
        m.send("Hello from Kotlin")
        assertTrue(m.outbox().isEmpty())
        assertTrue(mac.sends.single().contains("\"submission_id\":\"sub-1\""))
        // an answer is signed over the Mac's challenge
        val signer = Signs()
        m.answer("ap-1", AnswerInput.Approve(true, null), signer)
        assertTrue(signer.asked.single(), signer.asked.single().contains("\"challenge\":\"C-"))
        // decision: PHN-536
        // a hidden answer goes from a SecretInput filled one UTF-16 unit at a time (never AnswerInput.Hidden's String),
        // and the library empties it whatever happens (here the fake Mac refuses the answer)
        val secret = SecretInput()
        "invented-secret".forEach { secret.push(it.code.toUShort()) }
        assertEquals(15u, secret.len())
        runCatching { m.answerHidden("ap-2", secret, signer) }
        assertTrue("emptied after the call", secret.isEmpty())
        secret.destroy()
        assertTrue("destroyed", runCatching { secret.len() }.isFailure)
        try {
            Mobile.open(Files.createTempDirectory("ferb-core-x").resolve("x.db").toString(), ByteArray(3), MobileConfig("p", 0, null, null, null), FixedClock(), Counted(), heard)
            fail("a short key")
        } catch (e: FerbException.BadKey) {
            assertEquals("bad_key", e.code)
        }
        // a listener that throws is contained; two cores at once
        val b = open(Throws())
        b.connectTransport(FakeMac())
        b.linkEvent("""{"type":"state","phase":"online","tier":"direct"}""")
        b.sync()
        assertEquals(1, b.transcript("c1").items.size)
        // shutdown: idempotent; every call after it is Closed
        m.shutdown()
        m.shutdown()
        assertTrue(m.isClosed())
        try {
            m.sync()
            fail("sync after shutdown")
        } catch (e: FerbException.Closed) {
            assertEquals("closed", e.code)
        }
        assertFalse(b.isClosed())
        // the bindings' close() frees the handle (and shuts down); a later call throws in the bindings
        b.close()
        try {
            b.sync()
            fail("a call after close")
        } catch (_: IllegalStateException) {
        }
        assertTrue(heard.changes.isNotEmpty())
        m.close()
    }
}
