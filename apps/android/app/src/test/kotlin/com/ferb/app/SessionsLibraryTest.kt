package com.ferb.app

import com.ferb.app.model.Json
import com.ferb.app.model.Sessions
import com.ferb.design.components.SessionLineKind
import com.ferb.design.components.SessionState
import com.ferb.mobile.Clock
import com.ferb.mobile.FerbException
import com.ferb.mobile.Ids
import com.ferb.mobile.Listener
import com.ferb.mobile.Mobile
import com.ferb.mobile.MobileConfig
import com.ferb.mobile.Signer
import java.io.File
import java.nio.file.Files
import java.util.concurrent.atomic.AtomicInteger
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/**
 * The coding sessions' calls through the real phone library (the host build of ferb-mobile-ffi) against a Mac that
 * answers as the daemon's `sessions.*` methods do (kb HAND-651..666, the contract's shapes): what the app reads from each record
 * (title, folder, host, state, who controls it, the timeline's kinds, a card's choices), a send the Mac asks about, the
 * signed control over the exact words, a signed answer by the choice's id, the start's command, and a refusal that
 * reaches the app as the Mac's own words.
 *
 * Under Robolectric like the other tests through the library: the native library's callbacks (the transport, the
 * listener) are registered once per process by the first class loader that uses them, Robolectric's sandbox in a full
 * run, so a plain JUnit test after one would find its own callbacks unknown.
 */
@org.junit.runner.RunWith(org.robolectric.RobolectricTestRunner::class)
@org.robolectric.annotation.Config(application = android.app.Application::class, sdk = [35])
class SessionsLibraryTest {
    private val conv = Fixtures.conversations().first()

    private fun <T> withPhone(mac: FixtureMac, use: (Mobile) -> T): T {
        val dir = Files.createTempDirectory("ferb-sessions-lib").toFile()
        val n = AtomicInteger()
        val m = Mobile.open(File(dir, "phone.db").path, ByteArray(32) { 7 }, MobileConfig("client-test", 0, null, null, null),
            object : Clock { override fun nowIso() = "2026-11-22T10:00:00+00:00" },
            object : Ids { override fun newId() = "id-${n.incrementAndGet()}" },
            object : Listener { override fun changed(change: com.ferb.mobile.Change) {} })
        try {
            m.connectTransport(mac)
            m.linkEvent("""{"type":"state","phase":"online","tier":"direct"}""")
            return use(m)
        } finally {
            m.shutdown()
            m.close()
            dir.deleteRecursively()
        }
    }

    private val session = mapOf(
        "id" to "claude:abc", "agent" to "claude_code", "cwd" to "/Users/owner/code/web-app", "title" to "",
        "status" to "waiting_for_you", "waiting" to "permission",
        "host" to mapOf("bundle_id" to "com.apple.Terminal", "name" to "Terminal", "pid" to 812.0, "via" to "tty", "tty" to "/dev/ttys004"),
        "send_paths" to listOf("terminal_script"), "last_at" to "2026-11-22T09:59:00Z", "last_text" to "Run the build?",
        "control" to "phone:p1", "yours" to true, "goal" to "Keep going until the tests pass", "delegate" to "task-9",
        "ferb_note" to "FERB is working in this session.",
    )

    private val signer = object : Signer {
        var payloads = ArrayList<String>()
        override fun sign(payload: String): String { payloads += payload; return "sig" }
    }

    // decision: GATE-622, HAND-663
    @Test
    fun the_sessions_calls_read_the_macs_answers_as_the_app_uses_them() {
        val mac = FixtureMac(conv)
        mac.answers["sessions.list"] = { Fixtures.write(mapOf("enabled" to true, "sessions" to listOf(session,
            mapOf("id" to "codex:def", "agent" to "codex", "cwd" to "/Users/owner/code/blog", "title" to "Blog theme", "status" to "idle",
                "waiting" to "none", "host" to null, "send_paths" to emptyList<String>(), "no_send" to "No app shows this session.",
                "last_at" to "2026-11-22T08:00:00Z", "last_text" to "", "yours" to false),
            // HAND-710: one open window of an agent app: FERB reads none of its words; typed into its window
            mapOf("id" to "app:cursor:1", "agent" to "app_chat", "cwd" to "", "title" to "checkout.ts — shop", "window" to "checkout.ts — shop",
                "program" to "", "status" to "idle", "waiting" to "none", "host" to mapOf("name" to "Cursor", "bundle_id" to "com.todesktop.cursor"),
                "send_paths" to listOf("window"), "last_at" to "", "last_text" to "", "yours" to false, "watch" to true)))) }
        mac.answers["sessions.timeline"] = { p ->
            assertEquals("claude:abc", p["session"])
            Fixtures.write(mapOf("session" to session, "more" to true, "items" to listOf(
                mapOf("seq" to 7.0, "at" to "2026-11-22T09:58:00Z", "kind" to "user", "text" to "Fix the build"),
                mapOf("seq" to 8.0, "at" to "2026-11-22T09:58:05Z", "kind" to "assistant", "text" to "Running it now."),
                mapOf("seq" to 9.0, "at" to "2026-11-22T09:58:06Z", "kind" to "tool", "text" to "Bash: npm run build", "tool" to "Bash"),
                mapOf("seq" to 10.0, "at" to "2026-11-22T09:58:09Z", "kind" to "interrupted", "text" to ""),
                mapOf("seq" to 11.0, "at" to "2026-11-22T09:58:12Z", "kind" to "assistant", "text" to "The build passes.", "parts" to listOf(
                    mapOf("kind" to "thinking", "text" to "", "label" to "Thought 1s", "detail" to ""),
                    mapOf("kind" to "tool", "text" to "ok", "label" to "Run the build", "detail" to "npm"),
                    mapOf("kind" to "text", "text" to "| File | Size |\n|---|---|\n| a.zip | 3 MB |", "label" to "", "detail" to ""),
                    mapOf("kind" to "meta", "text" to "Just now", "label" to "", "detail" to ""),
                )),
            ), "cards" to listOf(mapOf("id" to "card-1", "session" to "claude:abc", "kind" to "permission", "tool" to "Bash",
                "detail" to "npm run build", "choices" to listOf(mapOf("id" to "yes", "label" to "Yes"), mapOf("id" to "no", "label" to "No")),
                "asked_at" to "2026-11-22T09:58:06Z", "approval_id" to "ap-1"))))
        }
        mac.answers["sessions.send"] = { p -> Fixtures.write(mapOf("needs_yes" to mapOf("question" to "Send this to Claude Code in Terminal?",
            "detail" to p["text"], "covers" to "Yes lets you send to this session from here until you stop controlling it."))) }
        mac.answers["sessions.control"] = { Fixtures.write(mapOf("sent" to true, "path" to "terminal_script", "delivered" to false, "seconds" to 4.0, "control" to "phone:p1")) }
        mac.answers["sessions.answer"] = { Fixtures.write(mapOf("answered" to true)) }
        mac.answers["sessions.release"] = { Fixtures.write(mapOf("released" to 1.0)) }
        mac.answers["sessions.prepare"] = { p ->
            val base = mapOf("folders" to listOf("/Users/owner/code/web-app", "/Users/owner/code/blog"), "host" to "Terminal")
            Fixtures.write(if (p["folder"] == null || p["command"] == "") base else base + ("needs_yes" to mapOf(
                "question" to "Open a new Terminal window in blog and type this?", "detail" to p["command"], "covers" to "")))
        }
        mac.answers["sessions.start"] = { p -> Fixtures.write(mapOf("started" to true, "command" to p["command"], "session" to "term:300", "host" to "Terminal")) }
        mac.answers["sessions.delegate"] = { Fixtures.write(mapOf("handed_over" to true, "message" to "FERB is on it.")) }
        mac.refuse["sessions.interrupt"] = "ferb_error"

        withPhone(mac) { m ->
            val list = m.sessionsList()
            assertTrue(list.enabled)
            val (a, b) = list.sessions
            assertEquals("web-app", Sessions.title(a))
            assertEquals("Claude Code", a.agentName)
            assertEquals("Terminal", a.host)
            assertEquals(SessionState.WAITING, Sessions.state(a.status))
            assertTrue(a.canSend && a.controlledHere && a.withFerb)
            // decision: HAND-702 — FERB's task (Take it back stops it) and its latest word, as the Mac sends them
            assertEquals("task-9", a.ferbTask)
            assertEquals("FERB is working in this session.", a.ferbNote)
            assertEquals("", b.ferbTask)
            assertEquals("Blog theme", Sessions.title(b))
            assertEquals("", b.host)
            assertFalse(b.canSend)
            assertEquals("No app shows this session.", b.noSend)
            // decision: HAND-710 — who is watched comes from the Mac; absent means not
            assertFalse(a.watch || b.watch)
            val w = list.sessions[2]
            assertTrue(w.watch && w.canSend && w.foreground)
            assertEquals("checkout.ts — shop", Sessions.title(w))
            assertEquals("Cursor", w.host)
            assertEquals("Cursor", w.agentName)

            val t = m.sessionTimeline("claude:abc", 0u, 0u, 50u)
            assertTrue(t.more)
            assertEquals(listOf(SessionLineKind.OWNER, SessionLineKind.AGENT, SessionLineKind.TOOL, SessionLineKind.INTERRUPTED, SessionLineKind.AGENT),
                t.items.map { Sessions.line(it, "stopped").kind })
            // decision: HAND-716 — a reply's parts in order, its text part's table read by the library as a table
            val reply = Sessions.line(t.items[4], "")
            assertEquals(listOf("thinking", "tool", "text", "meta"), t.items[4].parts.map { it.kind })
            assertEquals(listOf(com.ferb.design.components.SessionPartKind.THINKING, com.ferb.design.components.SessionPartKind.TOOL,
                com.ferb.design.components.SessionPartKind.TEXT, com.ferb.design.components.SessionPartKind.META), reply.parts.map { it.kind })
            assertEquals("npm", reply.parts[1].detail)
            assertTrue(reply.parts[2].blocks.single() is com.ferb.design.components.RichBlock.Table)
            assertEquals("Bash: npm run build", Sessions.line(t.items[2], "").text)
            assertEquals("stopped", Sessions.line(t.items[3], "stopped").text)
            assertEquals(listOf("yes", "no"), t.cards.single().choices.map { it.id })
            assertEquals("npm run build", t.cards.single().detail)
            m.sessionTimeline("claude:abc", 9u, 0u, 100u)
            assertEquals(9.0, (Json.parse(mac.params.last { it.first == "sessions.timeline" }.second) as Map<*, *>)["after"])

            val sent = m.sessionSend("claude:abc", "Run the tests again")
            val yes = sent.needsYes
            assertNotNull("the Mac asked: nothing was sent", yes)
            assertFalse(sent.sent)
            assertEquals("Run the tests again", yes!!.detail)
            val controlled = m.sessionControl("claude:abc", yes.detail, "", signer)
            assertTrue(controlled.sent)
            assertFalse("not seen arriving yet", controlled.delivered)
            val proof = Json.parse(mac.proofs[mac.calls.lastIndexOf("sessions.control")]!!) as Map<*, *>
            val payload = Json.parse(proof["payload"] as String) as Map<*, *>
            assertEquals("sessions.control", payload["method"])
            assertEquals(mapOf("session" to "claude:abc", "text" to "Run the tests again"), payload["params"])

            assertTrue(m.sessionAnswer("claude:abc", "card-1", "no", signer))
            val answered = Json.parse(mac.params.last { it.first == "sessions.answer" }.second) as Map<*, *>
            assertEquals("no", answered["choice"])
            assertNotNull("signed", mac.proofs[mac.calls.lastIndexOf("sessions.answer")])

            assertEquals(1u, m.sessionRelease("claude:abc"))
            assertEquals(listOf("/Users/owner/code/web-app", "/Users/owner/code/blog"), m.sessionPrepare("", "", "").folders)
            // decision: HAND-695 — a new terminal: the folder and the command only, no agent
            assertNull(m.sessionPrepare("terminal", "/Users/owner/code/blog", "").needsYes)
            val plan = m.sessionPrepare("terminal", "/Users/owner/code/blog", "agy")
            assertEquals("agy", plan.needsYes?.detail)
            assertEquals(mapOf("folder" to "/Users/owner/code/blog", "command" to "agy"),
                Json.parse(mac.params.last { it.first == "sessions.prepare" }.second))
            // the new session, to open it at once (HAND-702)
            assertEquals(com.ferb.mobile.SessionStarted("term:300", "Terminal", "agy"), m.sessionStart("terminal", "/Users/owner/code/blog", "agy", signer))
            assertEquals("FERB is on it.", m.sessionDelegate("claude:abc", "Keep going until the tests pass", signer))

            try {
                m.sessionInterrupt("claude:abc")
                fail("refused")
            } catch (e: FerbException.Refused) {
                // what the app's Stop its turn relies on: a refusal of the Mac's, then its send's question
                assertEquals("ferb_error", e.linkCode)
            }
        }
        assertNull("no session call went unanswered", mac.calls.firstOrNull { it.startsWith("sessions.") && it !in mac.answers.keys && it != "sessions.interrupt" })
    }
}
